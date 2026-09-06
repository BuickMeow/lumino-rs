//! 瀑布流帧渲染：首帧全量常驻 + 每帧 GPU cull 窗口 → legacy 精确渲染。
//!
//! UI 首帧发送全量 `note_instances`（`collect_all`，一次上传共享缓冲）；后续帧只发
//! uniforms，窗口由 cull 内核从常驻提取（与 UI `collect_window_notes` 同谓词、同序），
//! 输出 compact 直绑 legacy shader（回溯预算语义不变，像素等价 harness 保证）；
//! 活跃键色由 GPU 内核推导（零回读）。渲染侧每帧上传仅 offsets（1KB）+ uniforms。
//! cull 不可用（空文档/构建失败）时回退 legacy 上传路径（所带音符；首帧全量已排序，
//! 回退正确，慢一次）。

use super::common::{CopyDrainInput, copy_texture_and_drain, note_instances_to_key_offsets};
use crate::miditrail_renderer::pack_color;
use crate::render_thread::params::RenderParams;
use crate::render_thread::render_loop::runner::context::{RenderContext, RenderFrameState};
use crate::waterfall_renderer::{CullRenderOutcome, waterfall_viewport_span};
use crate::{CullWindow, WaterfallRenderer, WaterfallUniformGpu, unpack_key_color};

/// 处理视频导出瀑布流帧：常驻上传（首帧）→ cull 窗口 → 内核键色 → legacy 渲染 → 读回。
pub(super) fn handle_waterfall_frame(
    ctx: &RenderContext,
    params: RenderParams,
    frame: &mut RenderFrameState,
    width: u32,
    height: u32,
) {
    let notes = &params.note_instances;

    // 初始化瀑布流渲染器
    if frame.waterfall_renderer.is_none() {
        *frame.waterfall_renderer = Some(WaterfallRenderer::new(&ctx.device));
    }
    // 不变式：上面 is_none 判断后必然已创建；release 下若异常缺失则跳过本帧而非崩溃
    let renderer = match frame.waterfall_renderer.as_mut() {
        Some(r) => r,
        None => {
            debug_assert!(false, "waterfall_renderer 应已初始化（is_none 分支已创建）");
            return;
        }
    };

    // 键盘高度：帧高的 12%
    let kb_height = ((height as f64) * 0.12).round() as u32;
    let kb_height = kb_height.max(20).min(height / 3);

    // 构建 uniform 参数
    // 注意：使用 waterfall_current_tick（MIDI tick 值），而非 scroll.0（像素位置 = tick * zoom_x）
    let uniform = WaterfallUniformGpu {
        tick: params.waterfall_current_tick,
        ppq: params.ppq as u32,
        key_count: (params.max_key_index + 1.0) as u32,
        frame_width: width,
        frame_height: height,
        kb_height,
        speed: params.waterfall_speed.max(0.1),
        _padding: 0,
    };

    // cull 窗口与 UI 收集同公式（`waterfall_viewport_span` 共享，保证谓词一致）。
    let tick = params.waterfall_current_tick;
    let key_count = (params.max_key_index + 1.0).max(1.0) as usize;
    let tick_end = tick.saturating_add(waterfall_viewport_span(
        params.ppq as u32,
        params.waterfall_speed,
    ));

    // 创建编码器（cull FILL / 内核 / 主渲染 compute pass 追加到此 encoder）。
    let mut encoder = ctx
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("waterfall_encoder"),
        });

    // 常驻：非空帧上传（首帧全量；后续帧为空，跳过零上传）+ 世代递增。
    if !notes.is_empty() {
        frame.renderers.note.upload_shared_instances(notes);
        renderer.mark_resident_updated();
    }
    let (resident_buf, resident_count) = frame.renderers.note.gpu_note_buffer_for_sharing();

    let t_work = std::time::Instant::now();
    let window = CullWindow {
        tick_start: tick,
        tick_end,
        key_count,
    };
    let (path_label, note_total) = match renderer.render_culled(
        &ctx.device,
        &ctx.queue,
        &mut encoder,
        &uniform,
        &resident_buf,
        resident_count as usize,
        window,
    ) {
        CullRenderOutcome::Culled { visible } => ("cull", visible),
        CullRenderOutcome::FallbackNeeded => {
            // ── legacy 回退：上传所带音符 → 派生分桶偏移/活跃键色 → compute dispatch。
            // 首帧全量已 (key, start) 有序，回退正确（慢一次：派生 O(N)）；后续空帧
            // 回退（cull 持续失败）为空集黑帧 + 错误日志，属防御路径。
            if !notes.is_empty() {
                frame.renderers.note.upload_shared_instances(notes);
                renderer.mark_resident_updated();
            }
            let (shared_buffer, _) = frame.renderers.note.gpu_note_buffer_for_sharing();
            let key_offsets = note_instances_to_key_offsets(notes, key_count);
            // 活跃键颜色数组（128 个 u32，0 表示无高亮；仅 legacy 需要 CPU 循环，
            // cull 路径由 GPU 活跃键内核推导）。
            let tick_u = params.waterfall_current_tick;
            let mut active_key_colors = [0u32; 128];
            for n in notes.iter() {
                let (key, rgb) = unpack_key_color(n.key_color);
                let start = n.start_length[0].max(0.0) as u32;
                let end = start.saturating_add(n.start_length[1].max(1.0) as u32);
                if start <= tick_u && end > tick_u && (key as usize) < 128 {
                    // color_packed 为 0xRRGGBBAA，alpha=153 表示 60% 混合
                    // （与 shader 中 blend_key_color 的 alpha 参数匹配）
                    active_key_colors[key as usize] =
                        pack_color([rgb[0], rgb[1], rgb[2], 1.0]) & 0xFFFF_FF00 | 153u32;
                }
            }
            // dispatch compute shader（音符读共享缓冲，不再自有拷贝）
            renderer.render(
                &ctx.device,
                &ctx.queue,
                &mut encoder,
                &uniform,
                &shared_buffer,
                notes.len(),
                &key_offsets,
                &active_key_colors,
            );
            ("legacy-fallback", notes.len())
        }
    };
    let work_us = t_work.elapsed().as_micros() as u64;
    // 渲染侧分段打点（首 3 帧 + 每 300 帧）：path 区分 cull/回落；
    // cull 路径 work 含 COUNT 回读 + FILL 调度 + 内核 + 主渲染调度（submit 后执行）。
    {
        static RENDER_DIAG: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = RENDER_DIAG.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        if n < 3 || n.is_multiple_of(300) {
            tracing::info!(
                "waterfall渲染打点[{n}]: path={path_label} work={work_us}us notes={note_total}"
            );
        }
    }

    // 获取输出纹理并拷贝到 staging buffer（流水线模式由 pipeline_ready 保证已初始化）
    let pipeline = match frame.export_pipeline.as_mut() {
        Some(p) => p,
        None => {
            debug_assert!(false, "export_pipeline 应已初始化（pipeline_ready 已保证）");
            return;
        }
    };
    let tx = match frame.export_frame_tx.as_ref() {
        Some(t) => t,
        None => {
            debug_assert!(false, "export_frame_tx 应已初始化（pipeline_ready 已保证）");
            return;
        }
    };

    if let Some(tex) = renderer.output_texture() {
        copy_texture_and_drain(CopyDrainInput {
            pipeline,
            tx,
            encoder,
            texture: tex,
            queue: &ctx.queue,
            width,
            height,
            label: "瀑布流",
        });
    } else {
        tracing::warn!("瀑布流输出纹理未就绪");
        // 即使没有输出纹理，也必须提交空编码器，否则 GPU 队列死锁
        ctx.queue.submit(std::iter::once(encoder.finish()));
    }
}
