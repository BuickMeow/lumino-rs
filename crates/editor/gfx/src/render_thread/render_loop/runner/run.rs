use std::sync::{Arc, atomic::Ordering};
use std::time::Instant;

use super::super::super::commands::ControlCommand;
use super::super::super::export_pipeline::ExportPipeline;
use super::super::super::params::RenderParams;
use super::super::commands::process_commands;
use super::super::render_pass::update_stats;
use super::context::{
    DeferredCommandContext, PreviewPassContext, PreviewUploadContext, RenderContext,
    RenderThreadChannels,
};
use super::deferred::handle_deferred_command;
use super::onion_segments::{OnionSegment, apply_onion_track_delta, process_main_track_events};
use super::preview::{ensure_offscreen_textures_and_upload_notes, render_offscreen_pass};
use super::video_export::advance_export_inflight;
use crate::ArrangementNoteUniform;
use crate::gpu_resource_tracker::TrackedTexture;
use lumino_midiplayer::texture_waterfall::{
    WaterfallGpuCtx, WaterfallStreamMsg, drain_waterfall_stream,
};

/// 运行渲染线程主循环
pub fn run_render_thread(ctx: RenderContext, channels: RenderThreadChannels) {
    tracing::info!("Render thread started");

    // 初始化渲染器
    let mut renderers = super::super::Renderers::new(&ctx.device, &ctx.queue, ctx.texture_format);
    // 视频导出使用独立的纯 2D 渲染器，避免 depth-stencil 状态与普通预览不一致。
    let mut export_renderers: Option<super::super::Renderers> = None;

    // 渲染循环状态
    let mut frame_count = 0u64;
    let mut fps_update_time = Instant::now();
    // 记录本轮回环处理到的最后一条 Render 命令的 frame_id，供渲染完成后通知 UI
    let mut latest_frame_id = 0u64;
    let mut current_texture: Option<Arc<TrackedTexture>> = None;
    let mut depth_texture: Option<TrackedTexture> = None;
    let mut depth_texture_view: Option<wgpu::TextureView> = None;
    let mut texture_view: Option<wgpu::TextureView> = None;
    let mut current_size = (0, 0);
    let mut last_note_version: u64 = 0;

    // 高精度贴图瀑布流渲染器状态
    let mut texture_waterfall_renderer: Option<crate::TextureWaterfallRenderer> = None;
    let mut texture_waterfall_meta = None;
    let mut texture_waterfall_config = None;
    let mut deferred: Vec<ControlCommand> = Vec::new();

    // 视频导出读回管线状态
    let mut export_pipeline: Option<ExportPipeline> = None;
    let mut export_frame_tx = None;

    // 视频导出专用 GPU 渲染器（跨帧复用，避免每帧重建 pipeline）
    let mut waterfall_renderer: Option<crate::WaterfallRenderer> = None;
    let mut miditrail_renderer: Option<crate::MiditrailRenderer> = None;

    // 贴图瀑布流流式上传状态：true 表示正在接收 chunk（已 begin_streaming_upload）
    let mut onion_skin_streaming_in_progress = false;
    // 贴图瀑布流 GPU 布局段表（全量会话构建，增量替换时更新）
    let mut onion_segments: Vec<OnionSegment> = Vec::new();
    // 当前音轨编码（track_idx+1，0=无）：统一全量渲染的视图状态（切轨零重传）
    let mut current_track_encoded: u32 = 0;

    // ★ 后台生成线程通过有界同步通道流式传回贴图（容量1，背压）★
    // sync_channel(1)：channel 满时 send 阻塞，强制后台等渲染线程消费，
    // 防止无界积压导致 CPU 内存峰值（对应"装袋期间工人等着"）
    let (texture_waterfall_result_tx, texture_waterfall_result_rx) =
        std::sync::mpsc::sync_channel::<WaterfallStreamMsg>(1);

    while channels.running.load(Ordering::Relaxed) {
        // 处理命令
        let mut latest_params: Option<RenderParams> = None;
        let mut should_shutdown = false;

        let has_params = process_commands(
            &channels.command_receiver,
            &mut latest_params,
            &mut latest_frame_id,
            &mut should_shutdown,
            &mut deferred,
        );

        // 处理延迟的控制命令
        process_deferred_commands(
            &mut DeferredCommandContext {
                ctx: &ctx,
                channels: &channels,
                renderers: &mut renderers,
                export_renderers: &mut export_renderers,
                current_texture: &mut current_texture,
                depth_texture: &mut depth_texture,
                depth_texture_view: &mut depth_texture_view,
                texture_view: &mut texture_view,
                current_size: &mut current_size,
                last_note_version: &mut last_note_version,
                texture_waterfall_renderer: &mut texture_waterfall_renderer,
                texture_waterfall_meta: &mut texture_waterfall_meta,
                texture_waterfall_config: &mut texture_waterfall_config,
                export_pipeline: &mut export_pipeline,
                export_frame_tx: &mut export_frame_tx,
                waterfall_renderer: &mut waterfall_renderer,
                miditrail_renderer: &mut miditrail_renderer,
                texture_waterfall_result_tx: &texture_waterfall_result_tx,
                onion_streaming_in_progress: onion_skin_streaming_in_progress,
            },
            &mut deferred,
        );

        // 推进视频导出 inflight：即使没有新的 RenderVideoFrame 命令，
        // 也需要 try_read 已就绪的帧数据并发回 Runner，否则 inflight 满后
        // Runner 阻塞在 frame_rx.recv()，渲染线程也不再调用 try_read，形成死锁。
        advance_export_inflight(&mut export_pipeline, &export_frame_tx);

        // ★ 流式接收：每帧循环 try_recv，收到已合并像素立即 upload（GPU DMA，非阻塞）★
        let gpu = WaterfallGpuCtx {
            device: &ctx.device,
            queue: &ctx.queue,
            texture_format: ctx.texture_format,
        };
        drain_waterfall_stream(
            &texture_waterfall_result_rx,
            &gpu,
            &mut texture_waterfall_renderer,
            &channels.waterfall_progress,
        );

        // ★ 贴图瀑布流流式上传：drain channel，逐块 streaming_append 到 GPU ★
        drain_onion_skin_stream(
            &ctx,
            &mut renderers,
            &mut onion_segments,
            &mut onion_skin_streaming_in_progress,
            &mut current_track_encoded,
            &channels.onion_skin_streaming_rx,
        );

        if should_shutdown {
            break;
        }

        // ★ 主音轨事件级增量（段内应用）：drain note_events_rx → 当前音轨段
        // GPU 布局 = 全量轨段，事件 index = notes 索引（保序，无需可见列表映射）。
        // 段表定位依赖 SetViewState（切轨消息）先于编辑事件到达（mpsc 顺序保证）。
        if current_track_encoded != 0 {
            let updated = process_main_track_events(
                &mut renderers,
                &mut onion_segments,
                current_track_encoded,
                &channels.note_events_rx,
                &ctx.device,
                &ctx.queue,
            );
            if updated {
                renderers
                    .onion_skin
                    .update_cull_info(&ctx.device, &ctx.queue);
                let total_gpu_mb =
                    lumino_diagnostics::memtrace::Snapshot::capture().total_with_gpu_mb();
                tracing::debug!(
                    "MainTrack: 事件应用后 instance_count={} instance_buf={}MB visible_index_buf={}MB total_gpu={:.1}MB",
                    renderers.onion_skin.gpu_instance_count(),
                    renderers.onion_skin.instance_buffer_size() / 1024 / 1024,
                    renderers.onion_skin.visible_buffer_size() / 1024 / 1024,
                    total_gpu_mb
                );
            }
        }

        // 执行渲染（离屏纹理）
        if has_params && let Some(ref mut params) = latest_params {
            puffin::profile_scope!("wgpu_render_thread_frame");
            let frame_start = Instant::now();

            // 走带音符层：复用钢琴卷帘常驻 GPU 音符缓冲（零第二份显存）。
            // 依据侧栏音轨顺序预计算 lane 映射 / uniform，写入 params，
            // 供 prepare_renderers 直接驱动走带音符 GPU 裁剪 + draw_indirect 管线。
            if params.is_arrangement_mode {
                prepare_arrangement_note_data(params);
            }

            // 全屏瀑布流播放器模式：与钢琴卷帘完全隔离，跳过卷帘 3D 场景绘制
            // （网格/音符/洋葱皮的离屏绘制，最贵的 GPU 工作），解放 GPU。
            // 下方仍照常发布活体音符缓冲（note_data_pub）供播放器复用，禁止第二份拷贝。
            if !params.skip_scene_render {
                ensure_offscreen_textures_and_upload_notes(&mut PreviewUploadContext {
                    ctx: &ctx,
                    channels: &channels,
                    current_texture: &mut current_texture,
                    depth_texture: &mut depth_texture,
                    depth_texture_view: &mut depth_texture_view,
                    texture_view: &mut texture_view,
                    current_size: &mut current_size,
                    params,
                });

                render_offscreen_pass(&mut PreviewPassContext {
                    ctx: &ctx,
                    params,
                    channels: &channels,
                    renderers: &mut renderers,
                    current_texture: &mut current_texture,
                    depth_texture: &mut depth_texture,
                    depth_texture_view: &mut depth_texture_view,
                    texture_view: &mut texture_view,
                    current_size: &mut current_size,
                    last_note_version: &mut last_note_version,
                    texture_waterfall_renderer: &mut texture_waterfall_renderer,
                    texture_waterfall_meta: &mut texture_waterfall_meta,
                    texture_waterfall_config: &mut texture_waterfall_config,
                    export_pipeline: &mut export_pipeline,
                    export_frame_tx: &mut export_frame_tx,
                });
            }

            // 发布活体音符实例缓冲给 UI 线程（侧边瀑布流面板复用，禁止第二份拷贝）
            {
                let (buf, count) = renderers.onion_skin.gpu_note_buffer_for_sharing();
                if let Ok(mut guard) = channels.note_data_pub.lock() {
                    *guard = Some((buf, count));
                }
            }

            // 更新统计
            let frame_time = frame_start.elapsed();
            update_stats(
                &mut frame_count,
                &mut fps_update_time,
                frame_time,
                params,
                &channels.stats_clone,
                renderers.ruler.instance_count(),
            );

            // 通知 UI 线程：本帧（frame_id）已渲染完成，可安全 present（copy 到 Surface）。
            // 修复音符放置后不立即显示：UI 线程 present 前需 wait_for_frame，确保拷到
            // 含本次编辑（如音符 Insert）的最新离屏帧，而非尚未被本线程处理的旧帧。
            let (mtx, cvar) = &*channels.frame_sync;
            if let Ok(mut guard) = mtx.lock() {
                *guard = latest_frame_id;
                cvar.notify_all();
            }
        }
    }

    tracing::info!("Render thread stopped");
}

/// drain 贴图瀑布流流式上传通道：逐块 streaming_append 到 GPU。
///
/// UI 线程分块构建 NoteInstance（每块 ≤ 800 万实例 = 128 MB），通过 sync_channel(3) 传输。
/// 消息协议（事件级增量 2026-08-05）：
/// - `Chunk{track_id}`：全量会话数据块，按到达顺序构建段表（同轨续写、异轨新段）
/// - `Done`：全量会话完成 → finish_streaming_upload 更新 cull info + 清空段表
/// - `TrackDelta`：单音轨增量替换 → 等长 write_segment / 变长 GPU 搬移后续段
/// - `SetViewState`：切轨/静音零重传，只更新 ViewState uniform，GPU 数据不动
/// - `PreviewInstances`：预览音符（Drawing/hover/i2m），独立预览渲染器整体替换
///
/// 性能：消除旧方案 RenderParams.onion_skin_instances 全量 Vec 的 9.6 GB CPU 峰值；
/// 黑乐谱编辑非主音轨时不再全量重传。
fn drain_onion_skin_stream(
    ctx: &RenderContext,
    renderers: &mut super::super::Renderers,
    onion_segments: &mut Vec<OnionSegment>,
    onion_skin_streaming_in_progress: &mut bool,
    current_track_encoded: &mut u32,
    rx: &std::sync::mpsc::Receiver<crate::OnionSkinStreamMsg>,
) {
    loop {
        match rx.try_recv() {
            Ok(crate::OnionSkinStreamMsg::Done) => {
                // 全量会话结束（无论是否有块：0 音轨会话也需清空段表）
                if *onion_skin_streaming_in_progress {
                    renderers
                        .onion_skin
                        .finish_streaming_upload(&ctx.device, &ctx.queue);
                    *onion_skin_streaming_in_progress = false;
                    // 全量会话结束 → 常驻数据换代，派生索引（全局桶）必须重建。
                    renderers.onion_epoch = renderers.onion_epoch.wrapping_add(1);
                    let total_gpu_mb =
                        lumino_diagnostics::memtrace::Snapshot::capture().total_with_gpu_mb();
                    tracing::debug!(
                        "OnionSkin: 全量上传完成 instance_count={} instance_buf={}MB visible_index_buf={}MB total_gpu={:.1}MB",
                        renderers.onion_skin.gpu_instance_count(),
                        renderers.onion_skin.instance_buffer_size() / 1024 / 1024,
                        renderers.onion_skin.visible_buffer_size() / 1024 / 1024,
                        total_gpu_mb
                    );
                }
                // 段表必须保留：后续 `TrackDelta` 与 `process_main_track_events`
                // 依赖它定位音轨段。新的全量会话开始时会由首个 `Chunk` 重建段表。
                break;
            }
            Ok(crate::OnionSkinStreamMsg::Reserve { total }) => {
                // 预分配容量：消除流式 append 的 2× 倍增余量
                // （2.9 亿音符容量从 ~8.6GB 收到 ~4.6GB）
                if total > renderers.onion_skin.gpu_capacity() {
                    renderers.onion_skin.grow_gpu(total);
                    tracing::debug!("OnionSkin: 预分配容量 {} 实例（消除倍增余量）", total);
                }
            }
            Ok(crate::OnionSkinStreamMsg::Chunk {
                track_id,
                instances,
            }) => {
                // 首次收到块时 begin_streaming_upload + 清空段表
                if !*onion_skin_streaming_in_progress {
                    renderers.onion_skin.begin_streaming_upload();
                    onion_segments.clear();
                    *onion_skin_streaming_in_progress = true;
                }
                // 段表：同轨续写 len，异轨新开段（offset = 追加前实例数）
                let count_before_append = renderers.onion_skin.gpu_instance_count();
                if let Some(last) = onion_segments.last_mut() {
                    if last.track_id == track_id {
                        last.len += instances.len();
                    } else {
                        onion_segments.push(OnionSegment {
                            track_id,
                            offset: count_before_append,
                            len: instances.len(),
                        });
                    }
                } else {
                    onion_segments.push(OnionSegment {
                        track_id,
                        offset: count_before_append,
                        len: instances.len(),
                    });
                }
                renderers.onion_skin.streaming_append(&instances);
            }
            Ok(crate::OnionSkinStreamMsg::TrackDelta {
                track_id,
                instances,
            }) => {
                if *onion_skin_streaming_in_progress {
                    // UI 不应在全量会话中夹带增量（防御性：状态不一致时跳过）
                    tracing::warn!(
                        "OnionSkin: TrackDelta(track={}) 与全量流式会话交错，跳过该增量",
                        track_id
                    );
                } else {
                    apply_onion_track_delta(
                        renderers,
                        onion_segments,
                        track_id,
                        &instances,
                        &ctx.device,
                        &ctx.queue,
                    );
                }
            }
            Ok(crate::OnionSkinStreamMsg::SetViewState {
                current_track,
                muted_tracks,
            }) => {
                // 切轨/静音零重传：只更新 ViewState uniform，GPU 数据不动
                *current_track_encoded = current_track;
                renderers
                    .onion_skin
                    .set_view_state(&ctx.queue, current_track, &muted_tracks);
            }
            Ok(crate::OnionSkinStreamMsg::PreviewInstances(instances)) => {
                // 预览音符（Drawing/hover/i2m）：独立预览渲染器整体替换
                renderers
                    .note
                    .upload_instances(&instances, &ctx.device, &ctx.queue);
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => break,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                // UI 线程关闭 channel（shutdown），停止 drain
                break;
            }
        }
    }
}

/// 处理延迟队列中的控制命令（视频导出 / 贴图瀑布流控制）。
fn process_deferred_commands(
    context: &mut DeferredCommandContext<'_>,
    deferred: &mut Vec<ControlCommand>,
) {
    for cmd in deferred.drain(..) {
        handle_deferred_command(cmd, context);
    }
}

/// 预计算走带音符层所需的 GPU 数据（复用钢琴卷帘常驻 GPU 音符缓冲，零第二份显存）。
///
/// 输入：`params.arrangement_track_order`（侧栏音轨顺序，元素=文档音轨 id，索引=泳道序号）。
///
/// 输出（写入 `params`）：
/// - `arrangement_lane_index`：`lane_index[doc_track] = 泳道序号`（着色器按 doc track 索引）；
///   静音/隐藏轨置 `f32::MAX` 哨兵，GPU 裁剪着色器据此自动剔除其音符。
/// - `arrangement_note_uniform`：滚动/缩放/泳道高/画布偏移等走带专属 uniform。
///
/// 横向/纵向滚动只需更新 uniform（scroll.x / scroll.y），无需任何重建；GPU 裁剪
/// 计算着色器在每帧一次性完成可视范围剔除，彻底消除此前每帧 ~67ms 的 CPU 音符重建。
fn prepare_arrangement_note_data(params: &mut RenderParams) {
    puffin::profile_scope!("arrangement::note_data");
    let track_order = &params.arrangement_track_order;
    let nt = track_order.len();
    if nt == 0 {
        params.arrangement_lane_index.clear();
        return;
    }

    // 1. lane_index[doc_track] = 泳道序号；静音/隐藏轨置哨兵值，GPU 裁剪自动剔除
    let max_doc = track_order.iter().cloned().max().unwrap_or(0) as usize;
    let mut lane_index = vec![0.0f32; max_doc.saturating_add(1).max(1)];
    for (lane, &doc) in track_order.iter().enumerate() {
        let visible = params
            .arrangement_track_visible
            .get(lane)
            .copied()
            .unwrap_or(true);
        lane_index[doc as usize] = if visible { lane as f32 } else { f32::MAX };
    }

    // 2. 走带专属 uniform（滚动/缩放/泳道高/画布偏移）
    // 泳道高度必须与 CPU 端 lane 背景（arrangement_instances.rs 的
    // `track_height * zoom_y`）完全一致，否则垂直缩放时音符与背景泳道失配。
    let au = &params.arrangement_uniform;
    let lh = au.track_height * au.zoom_y;
    params.arrangement_lane_index = lane_index;
    params.arrangement_note_uniform = ArrangementNoteUniform {
        scroll: au.scroll,
        zoom: [au.zoom, 1.0],
        viewport_size: au.viewport_size,
        canvas_offset: au.canvas_offset,
        lane_height: lh,
        note_height: 4.0,
        _pad: [0.0, 0.0],
    };
}
