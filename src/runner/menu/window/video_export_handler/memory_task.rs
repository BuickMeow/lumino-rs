//! 内存模式（完整 MIDI 文档）视频导出后台任务。

use std::io::BufWriter;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::{Sender, channel};
use std::time::Instant;

use lumino_export::video::{FfmpegEncoder, VideoExportConfig};
use lumino_gfx::render_thread::{ControlCommand, RenderCommand};
use lumino_message::events::window::video::{MidiConsoleBackend, MiditrailViewMode, RenderMode};
use tokio::sync::mpsc::UnboundedSender;

use super::super::video_export::{
    self, CounterFontRenderer, CounterRenderConfig, CounterStats, DataCurveRenderConfig,
    DataCurveRenderer, MidiConsoleRenderConfig, MidiConsoleRenderer, SortableNote, keyboard,
};
use super::commands::{finalize_video_export, send_export_error, send_initial_render_commands};
use super::composite::{CompositeEncodeFrameInput, composite_and_encode_frame};
use super::frame::{EncodeFrameQueue, FrameParams};
use super::pipeline::FramePipeline;

/// 进度消息载荷：(文本, 进度 0..1, 总帧数, 平滑 FPS, 已用秒)
type ProgressMsg = (String, f64, u64, f64, f64);

/// 内存模式入队阶段共享状态（原 `run_video_export_task` 内嵌闭包捕获的全部变量）。
struct MemoryEnqueueCtx<'a> {
    cmd_sender: &'a Sender<RenderCommand>,
    document: &'a Arc<lumino_midi_loader::MidiDocument>,
    fps_f64: f64,
    ppq: u32,
    width: u32,
    height: u32,
    key_count: u16,
    is_cpu_renderer: bool,
    is_gpu_compute_style: bool,
    render_mode: RenderMode,
    counter_config: &'a Option<CounterRenderConfig>,
    counter_stats: &'a mut Option<CounterStats>,
    counter_renderer: &'a mut Option<CounterFontRenderer>,
    data_curve_config: &'a Option<DataCurveRenderConfig>,
    data_curve_renderer: &'a mut Option<DataCurveRenderer>,
    midi_console_config: &'a Option<MidiConsoleRenderConfig>,
    midi_console_renderer: &'a mut Option<MidiConsoleRenderer>,
    duration_secs: f64,
    waterfall_scroll_speed: f32,
    miditrail_z_far: f32,
    miditrail_view_mode: MiditrailViewMode,
    miditrail_normal_speed: f32,
    miditrail_top_speed: f32,
    miditrail_3d_notes: bool,
    frame_tx_waterfall: &'a Sender<Vec<u8>>,
    progress_tx: &'a UnboundedSender<ProgressMsg>,
    key_colors: &'a mut [u8; keyboard::KEY_COLOR_BYTES],
    key_color_state: &'a mut keyboard::PlaybackKeyColorState,
    csv_writer: &'a mut Option<BufWriter<std::fs::File>>,
    visible_note_buf: &'a mut Vec<SortableNote>,
    note_instances_buf: &'a mut Vec<lumino_gfx::NoteInstance>,
    /// 首帧全量上传标记：首帧收集全文档音符常驻 GPU，后续帧跳过收集（uniform 驱动重裁剪）。
    notes_uploaded: &'a mut bool,
    /// 瀑布流/MIDITrail 窗口收集滑动状态（同一导出任务内复用，tick 单调递增）。
    window_state: &'a mut video_export::WindowCollectState,
}

/// 内存模式单帧入队（原 `run_video_export_task` 内嵌 `enqueue_frame` 闭包抽出的自由函数）。
///
/// 计算当前 tick 的按键高亮色与标尺偏移并入队 `FrameParams`；CPU 渲染模式
/// （瀑布流/计数器/数据曲线）在此直接生成帧数据并送入编码通道，跳过 GPU 路径。
/// 返回 `true` 表示应终止渲染循环（配置缺失/通道关闭/取消）。
fn enqueue_memory_frame(
    ctx: &mut MemoryEnqueueCtx,
    queue: &mut EncodeFrameQueue,
    frame_idx: u64,
) -> bool {
    let time_sec = frame_idx as f64 / ctx.fps_f64;
    let tempo_changes = &ctx.document.tempo_changes;
    let tick = video_export::seconds_to_tick(time_sec, tempo_changes, ctx.ppq);

    // 根据当前播放 tick 增量计算按键高亮颜色（仅钢琴卷帘合成路径消费）。
    // GPU compute（瀑布流/MIDITrail）与 CPU 渲染（计数器等）走 FrameParams::default()
    // + 空键盘贴图，key_colors 无人读取——跳过整次增量扫描（含每轨 O(前缀) skip），
    // 高数据量下这是每帧数十毫秒的死工作（见 recv=20~63ms 根因）。
    if !ctx.is_cpu_renderer && !ctx.is_gpu_compute_style {
        video_export::keyboard::update_playback_key_colors(
            ctx.document,
            tick,
            ctx.key_color_state,
            ctx.key_colors,
        );
    }

    // 计算 scroll_x / zoom_x，用于标尺小节号合成
    let video_kb_width = 60.0f32;
    let video_viewport_tick_span = (ctx.ppq * 16).max(1) as f32;
    let video_zoom_x = (ctx.width as f32 - video_kb_width) / video_viewport_tick_span;
    let video_scroll_x = tick as f32 * video_zoom_x;

    // 入队帧合成参数（与帧数据 FIFO 对应）
    queue.push_back(FrameParams {
        scroll_x: video_scroll_x,
        zoom_x: video_zoom_x,
        keyboard_width: video_kb_width,
        ppq: ctx.ppq,
        key_colors: *ctx.key_colors,
    });

    // 计数器/数据曲线/MidiConsole 模式（CPU 端渲染）：绕过 GPU 开销，BGRA 直出。
    // 注：瀑布流走 GPU compute 管线（见 RenderMode::Waterfall 的 GPU 分支），此处无 CPU 分支。
    if ctx.is_cpu_renderer {
        let mut frame_data = vec![0u8; (ctx.width as usize) * (ctx.height as usize) * 4];
        match ctx.render_mode {
            RenderMode::Waterfall => {
                send_export_error(
                    ctx.progress_tx,
                    "导出失败：Waterfall 模式不应进入 CPU 渲染分支（内部错误）",
                );
                return true;
            }
            RenderMode::NoteCounter => {
                // 计数器模式：统计推进 + 文本模板渲染（无卷帘/键盘/标尺）
                // 配置缺失 = 内部状态不一致：优雅终止导出，不 panic 渲染线程
                let Some(cfg) = ctx.counter_config.as_ref() else {
                    send_export_error(
                        ctx.progress_tx,
                        "导出失败：计数器模式缺少渲染配置（内部错误）",
                    );
                    return true;
                };
                let Some(stats) = ctx.counter_stats.as_mut() else {
                    send_export_error(
                        ctx.progress_tx,
                        "导出失败：计数器模式缺少统计状态（内部错误）",
                    );
                    return true;
                };
                let Some(renderer) = ctx.counter_renderer.as_mut() else {
                    send_export_error(
                        ctx.progress_tx,
                        "导出失败：计数器模式缺少字体渲染器（内部错误）",
                    );
                    return true;
                };
                let out = video_export::render_counter_frame(video_export::CounterFrameInput {
                    frame: &mut frame_data,
                    frame_width: ctx.width,
                    frame_height: ctx.height,
                    document: ctx.document,
                    tick,
                    ppq: ctx.ppq,
                    fps: ctx.fps_f64 as u32,
                    duration_secs: ctx.duration_secs,
                    config: cfg,
                    stats,
                    renderer,
                });
                // CSV 行写入（失败仅告警，不中断渲染）
                if let (Some(line), Some(writer)) = (out.csv_line, ctx.csv_writer.as_mut()) {
                    use std::io::Write;
                    if let Err(e) = writeln!(writer, "{line}") {
                        tracing::warn!("计数器 CSV 写入失败: {e}");
                    }
                }
                // 首帧诊断：确认模板渲染内容（与流式模式诊断风格一致）
                static COUNTER_DIAG: std::sync::atomic::AtomicU32 =
                    std::sync::atomic::AtomicU32::new(0);
                let diag_idx = COUNTER_DIAG.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                if diag_idx < 3 {
                    tracing::info!(
                        "计数器模式诊断[{diag_idx}]: tick={tick} stats=({},poly {},nps {}) text=\"{}\"",
                        stats.note_count,
                        stats.polyphony,
                        stats.nps,
                        out.text.replace('\n', "\\n"),
                    );
                }
            }
            RenderMode::NoteRectangle => {
                send_export_error(
                    ctx.progress_tx,
                    "导出失败：NoteRectangle 模式不应进入 CPU 渲染分支（内部错误）",
                );
                return true;
            }
            RenderMode::MIDITrail => {
                send_export_error(
                    ctx.progress_tx,
                    "导出失败：MIDITrail 模式不应进入 CPU 渲染分支（内部错误）",
                );
                return true;
            }
            RenderMode::DataCurve => {
                // 数据曲线模式：统计推进 → 取指标值 → 环形窗口 → 帧渲染
                // 配置缺失 = 内部状态不一致：优雅终止导出，不 panic 渲染线程
                let Some(cfg) = ctx.data_curve_config.as_ref() else {
                    send_export_error(
                        ctx.progress_tx,
                        "导出失败：数据曲线模式缺少渲染配置（内部错误）",
                    );
                    return true;
                };
                let Some(stats) = ctx.counter_stats.as_mut() else {
                    send_export_error(
                        ctx.progress_tx,
                        "导出失败：数据曲线模式缺少统计状态（内部错误）",
                    );
                    return true;
                };
                let Some(renderer) = ctx.data_curve_renderer.as_mut() else {
                    send_export_error(
                        ctx.progress_tx,
                        "导出失败：数据曲线模式缺少渲染器（内部错误）",
                    );
                    return true;
                };
                // 关键：推进统计到当前 tick（与计数器分支一致）。
                // 缺失此调用会导致 NPS/复音数/音符数永远停留在 0 → 曲线为 0 值直线。
                stats.advance(ctx.document, tick, ctx.fps_f64 as u32);
                let value = match cfg.metric {
                    lumino_message::events::window::video::DataCurveMetric::Nps => stats.nps as f64,
                    lumino_message::events::window::video::DataCurveMetric::Polyphony => {
                        stats.polyphony as f64
                    }
                    lumino_message::events::window::video::DataCurveMetric::NoteCount => {
                        stats.note_count as f64
                    }
                    lumino_message::events::window::video::DataCurveMetric::Bpm => {
                        video_export::current_bpm(&ctx.document.tempo_changes, tick)
                    }
                };
                renderer.push_value(value);
                let out = video_export::render_data_curve_frame(
                    &mut frame_data,
                    ctx.width,
                    ctx.height,
                    renderer,
                    cfg,
                );
                // 首帧诊断：确认指标值与缩放状态
                static DATA_CURVE_DIAG: std::sync::atomic::AtomicU32 =
                    std::sync::atomic::AtomicU32::new(0);
                let diag_idx = DATA_CURVE_DIAG.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                if diag_idx < 3 {
                    tracing::info!(
                        "数据曲线模式诊断[{diag_idx}]: tick={tick} value={} zoom=({}, {})",
                        out.value,
                        out.min,
                        out.max,
                    );
                }
            }
            RenderMode::MidiConsole => {
                // MidiConsole 风格：状态化渲染器直出 BGRA；后端由配置决定（GPU/CPU）
                let Some(renderer) = ctx.midi_console_renderer.as_mut() else {
                    send_export_error(
                        ctx.progress_tx,
                        "导出失败：MidiConsole 模式缺少渲染器（内部错误）",
                    );
                    return true;
                };
                let Some(cfg) = ctx.midi_console_config.as_ref() else {
                    send_export_error(
                        ctx.progress_tx,
                        "导出失败：MidiConsole 模式缺少渲染配置（内部错误）",
                    );
                    return true;
                };
                match cfg.render_backend {
                    MidiConsoleBackend::Gpu => {
                        video_export::render_midicomsole_frame_gpu(
                            video_export::MidiConsoleFrameArgs {
                                renderer,
                                frame: &mut frame_data,
                                frame_width: ctx.width,
                                frame_height: ctx.height,
                                document: ctx.document,
                                tick,
                                ppq: ctx.ppq,
                                fps: ctx.fps_f64 as u32,
                            },
                        );
                    }
                    MidiConsoleBackend::Cpu => {
                        video_export::render_midicomsole_frame(
                            video_export::MidiConsoleFrameArgs {
                                renderer,
                                frame: &mut frame_data,
                                frame_width: ctx.width,
                                frame_height: ctx.height,
                                document: ctx.document,
                                tick,
                                ppq: ctx.ppq,
                                fps: ctx.fps_f64 as u32,
                            },
                        );
                    }
                }
            }
        }

        // 帧数据直接送入编码通道，跳过渲染线程的 GPU 路径
        if ctx.frame_tx_waterfall.send(frame_data).is_err() {
            tracing::error!("CPU 渲染帧发送失败：通道已关闭");
            send_export_error(ctx.progress_tx, "导出失败：帧通道通信错误");
            return true;
        }
    } else {
        let collect_all = !*ctx.notes_uploaded;
        let Some(params) =
            video_export::build_video_export_render_params(video_export::RenderParamsInput {
                width: ctx.width,
                height: ctx.height,
                tick,
                document: ctx.document,
                ppq: ctx.ppq,
                key_count: ctx.key_count,
                render_mode: ctx.render_mode,
                waterfall_scroll_speed: ctx.waterfall_scroll_speed,
                miditrail_z_far: ctx.miditrail_z_far,
                miditrail_view_mode: ctx.miditrail_view_mode,
                miditrail_normal_speed: ctx.miditrail_normal_speed,
                miditrail_top_speed: ctx.miditrail_top_speed,
                miditrail_3d_notes: ctx.miditrail_3d_notes,
                fps: ctx.fps_f64 as f32,
                visible_notes: ctx.visible_note_buf,
                note_instances_out: ctx.note_instances_buf,
                collect_all,
                window_state: ctx.window_state,
            })
        else {
            send_export_error(
                ctx.progress_tx,
                "导出失败：当前渲染模式不应进入此分支（内部错误）",
            );
            return true;
        };
        // 首帧全量数据已随本帧发出，后续帧只发 uniforms，复用 GPU 常驻数据。
        *ctx.notes_uploaded = true;

        if ctx
            .cmd_sender
            .send(RenderCommand::Control(ControlCommand::RenderVideoFrame {
                params: Box::new(params),
            }))
            .is_err()
        {
            tracing::error!("发送 RenderVideoFrame 命令失败");
            send_export_error(ctx.progress_tx, "导出失败：渲染线程通信错误");
            return true;
        }
    }
    false
}

/// 后台线程主流程：创建编码器、发送初始渲染命令、逐帧渲染 + 编码、收尾。
///
/// 该函数整体等价于原 `handle_start_video_export` 中 `move` 闭包体内的逻辑，
/// 仅将各阶段进一步拆分成下方私有步骤函数，行为保持一致。
pub(super) struct RunVideoExportTaskInput {
    pub config: VideoExportConfig,
    pub cmd_sender: Sender<RenderCommand>,
    pub progress_tx: UnboundedSender<ProgressMsg>,
    pub preview_tx: UnboundedSender<(Vec<u8>, u32, u32)>,
    pub document: Arc<lumino_midi_loader::MidiDocument>,
    pub ppq: u32,
    pub fps_f64: f64,
    pub key_count: u16,
    pub width: u32,
    pub height: u32,
    pub cancel_flag: Arc<AtomicBool>,
    pub input_pix_fmt: &'static str,
    pub is_cpu_renderer: bool,
    pub is_gpu_compute_style: bool,
    pub waterfall_scroll_speed: f32,
    pub miditrail_z_far: f32,
    pub miditrail_view_mode: MiditrailViewMode,
    pub miditrail_normal_speed: f32,
    pub miditrail_top_speed: f32,
    pub miditrail_3d_notes: bool,
    pub render_mode: RenderMode,
    pub counter_config: Option<CounterRenderConfig>,
    pub data_curve_config: Option<DataCurveRenderConfig>,
    pub midi_console_config: Option<MidiConsoleRenderConfig>,
}

pub(super) fn run_video_export_task(input: RunVideoExportTaskInput) {
    let RunVideoExportTaskInput {
        config,
        cmd_sender,
        progress_tx,
        preview_tx,
        document,
        ppq,
        fps_f64,
        key_count,
        width,
        height,
        cancel_flag,
        input_pix_fmt,
        is_cpu_renderer,
        is_gpu_compute_style,
        waterfall_scroll_speed,
        miditrail_z_far,
        miditrail_view_mode,
        miditrail_normal_speed,
        miditrail_top_speed,
        miditrail_3d_notes,
        render_mode,
        counter_config,
        data_curve_config,
        midi_console_config,
        ..
    } = input;
    let start = Instant::now();

    // 按键颜色增量扫描状态（与编辑器 PlaybackScanState 等价）
    let mut key_color_state = keyboard::PlaybackKeyColorState::default();
    let mut key_colors = [0u8; keyboard::KEY_COLOR_BYTES];

    // 创建帧数据通道与回收通道
    let (frame_tx, frame_rx) = channel::<Vec<u8>>();
    let (recycle_tx, recycle_rx) = channel::<Vec<u8>>();

    // 创建 FFmpeg 编码器（直连写入模式，缓冲区由调用方在 write_frame 后归还对象池）
    let mut encoder = match FfmpegEncoder::new(&config, input_pix_fmt) {
        Ok(e) => e,
        Err(e) => {
            tracing::error!("FFmpeg 创建失败: {e}");
            send_export_error(&progress_tx, format!("导出失败: {e}"));
            return;
        }
    };

    // 发送初始渲染命令（StartVideoExport），携带帧缓冲回收通道
    // clone frame_tx：send_initial_render_commands 会消费原始 frame_tx（移至渲染线程），
    // 瀑布流 CPU 路径需在 enqueue_frame 中保留发送能力。
    let frame_tx_waterfall = frame_tx.clone();
    if send_initial_render_commands(
        &cmd_sender,
        width,
        height,
        frame_tx,
        recycle_rx,
        &progress_tx,
    ) {
        return;
    }

    // 计算总帧数
    let tempo_changes = &document.tempo_changes;
    let total_ticks = document.total_ticks;
    let duration_secs = video_export::compute_duration_secs(tempo_changes, total_ticks, ppq);
    let total_frames = config.total_frames(duration_secs);

    // 计数器模式：统计状态 + 字体渲染器 + CSV 写入器
    // 数据曲线模式：统计状态（共用 CounterStats）+ 数据曲线渲染器
    let mut counter_stats: Option<CounterStats> = None;
    let mut counter_renderer: Option<CounterFontRenderer> = None;
    let mut csv_writer: Option<std::io::BufWriter<std::fs::File>> = None;
    let mut data_curve_renderer: Option<DataCurveRenderer> = None;
    // 统计状态：计数器与数据曲线共用同一数据源
    if counter_config.is_some() || data_curve_config.is_some() {
        let mut stats = CounterStats::default();
        stats.reset(&document);
        counter_stats = Some(stats);
    }
    if let Some(cfg) = &counter_config {
        // 字体渲染器：TTF 加载失败时回退内置点阵（导出流程不中断）
        match CounterFontRenderer::new(&cfg.font, cfg.font_size) {
            Ok(r) => {
                tracing::info!("计数器字体加载成功：{}", r.describe());
                counter_renderer = Some(r);
            }
            Err(e) => {
                tracing::warn!("计数器字体加载失败（回退内置点阵）: {e}");
                counter_renderer = match CounterFontRenderer::new(
                    &lumino_message::events::window::video::CounterFont::Bitmap,
                    cfg.font_size,
                ) {
                    Ok(r) => Some(r),
                    Err(fallback_e) => {
                        // 内置点阵理论上不会失败；若异常（资源/渲染初始化问题），
                        // 降级为不渲染计数器，避免导出任务崩溃。
                        tracing::error!("内置点阵字体渲染器也失败（计数器将不渲染）: {fallback_e}");
                        None
                    }
                };
            }
        }
        if cfg.save_csv && !cfg.csv_output.as_os_str().is_empty() {
            match std::fs::File::create(&cfg.csv_output) {
                Ok(f) => csv_writer = Some(std::io::BufWriter::new(f)),
                Err(e) => tracing::warn!("计数器 CSV 文件创建失败: {e}"),
            }
        }
    }
    if let Some(cfg) = &data_curve_config {
        let fps_u32 = fps_f64.max(1.0) as u32;
        match DataCurveRenderer::new(cfg, fps_u32) {
            Ok(r) => {
                tracing::info!("数据曲线渲染器就绪（窗口 {} 帧）", r.window_cap());
                data_curve_renderer = Some(r);
            }
            Err(e) => {
                tracing::warn!("数据曲线字体加载失败（回退内置点阵）: {e}");
                let fallback = DataCurveRenderConfig {
                    font: lumino_message::events::window::video::CounterFont::Bitmap,
                    ..cfg.clone()
                };
                data_curve_renderer = match DataCurveRenderer::new(&fallback, fps_u32) {
                    Ok(r) => Some(r),
                    Err(fallback_e) => {
                        // 内置点阵理论上不会失败；异常时降级为不渲染数据曲线。
                        tracing::error!(
                            "内置点阵数据曲线渲染器也失败（数据曲线将不渲染）: {fallback_e}"
                        );
                        None
                    }
                };
            }
        }
    }

    // MidiConsole 风格渲染器（全文档模式，需完整 MIDI 数据）
    let mut midi_console_renderer: Option<MidiConsoleRenderer> = None;
    if let Some(cfg) = &midi_console_config {
        midi_console_renderer = Some(MidiConsoleRenderer::new(&document, cfg));
    }

    let mut render_bar = video_export::cli_progress::CliProgressBar::new(30, "视频渲染");
    render_bar.update(
        0.0,
        &format!(
            "总时长 {:.1}s | 总帧数 {} | PPQ {}",
            duration_secs, total_frames, ppq
        ),
    );

    let mut last_preview_time = Instant::now();
    let mut preview_sent = false;

    // ★ 生成键盘贴图（使用 CPU 贴图方式，在帧数据上合成）
    let (keyboard_pixels, kb_w, kb_h) =
        video_export::generate_keyboard_texture(width, height, key_count);

    // 流水线渲染：Runner 预填充 4 帧命令，让 staging ring 从开始就满载，
    // 之后每处理完一帧立即补发下一帧，保持 GPU/CPU 流水线持续运转。
    // 每帧参数携带该帧的按键高亮颜色（RGBAx256 键），用于后台线程合成键盘。
    let mut param_queue: EncodeFrameQueue = EncodeFrameQueue::with_capacity(16);

    // 复用缓冲区避免每帧堆分配
    let mut visible_note_buf: Vec<SortableNote> = Vec::with_capacity(4096);
    let mut note_instances_buf: Vec<lumino_gfx::NoteInstance> = Vec::with_capacity(4096);
    // 首帧全量上传标记（GPU 常驻后后续帧只发 uniforms）
    let mut notes_uploaded = false;
    // 窗口收集滑动状态（瀑布流/MIDITrail 同一任务内复用）
    let mut window_state = video_export::WindowCollectState::default();

    // 闭包不捕获 param_queue，而是作为参数传入，避免与主循环中的 pop_front 产生可变借用冲突。
    let mut ctx = MemoryEnqueueCtx {
        cmd_sender: &cmd_sender,
        document: &document,
        fps_f64,
        ppq,
        width,
        height,
        key_count,
        is_cpu_renderer,
        is_gpu_compute_style,
        render_mode,
        counter_config: &counter_config,
        counter_stats: &mut counter_stats,
        counter_renderer: &mut counter_renderer,
        data_curve_config: &data_curve_config,
        data_curve_renderer: &mut data_curve_renderer,
        midi_console_config: &midi_console_config,
        midi_console_renderer: &mut midi_console_renderer,
        duration_secs,
        waterfall_scroll_speed,
        miditrail_z_far,
        miditrail_view_mode,
        miditrail_normal_speed,
        miditrail_top_speed,
        miditrail_3d_notes,
        frame_tx_waterfall: &frame_tx_waterfall,
        progress_tx: &progress_tx,
        key_colors: &mut key_colors,
        key_color_state: &mut key_color_state,
        csv_writer: &mut csv_writer,
        visible_note_buf: &mut visible_note_buf,
        note_instances_buf: &mut note_instances_buf,
        notes_uploaded: &mut notes_uploaded,
        window_state: &mut window_state,
    };
    let mut enqueue_frame = |queue: &mut EncodeFrameQueue, frame_idx: u64| -> bool {
        enqueue_memory_frame(&mut ctx, queue, frame_idx)
    };

    // 预填充 + 主循环 + drain 由公共 FramePipeline 驱动（与流式路径共用同一循环骨架）
    let mut pipeline = FramePipeline {
        total_frames,
        cancel_flag: &cancel_flag,
        frame_rx: &frame_rx,
        param_queue: &mut param_queue,
        progress_tx: &progress_tx,
        render_bar: &mut render_bar,
        start,
        // 内存路径：进度直接映射（无解析阶段）
        progress_map: |p| p,
    };
    let mut process_frame = |frame_data: Vec<u8>, frame_params: FrameParams| {
        if is_gpu_compute_style || is_cpu_renderer {
            // GPU compute（瀑布流/MIDITrail）帧已完整渲染；
            // CPU 渲染（计数器）帧也不含键盘/标尺，均直接编码。
            composite_and_encode_frame(CompositeEncodeFrameInput {
                data: frame_data,
                params: FrameParams::default(),
                encoder: &mut encoder,
                progress_tx: &progress_tx,
                preview_tx: &preview_tx,
                cancel_flag: &cancel_flag,
                last_preview_time: &mut last_preview_time,
                preview_sent: &mut preview_sent,
                width,
                height,
                keyboard_pixels: &[],
                kb_w: 0,
                kb_h: 0,
                recycle_tx: &recycle_tx,
            })
        } else {
            composite_and_encode_frame(CompositeEncodeFrameInput {
                data: frame_data,
                params: frame_params,
                encoder: &mut encoder,
                progress_tx: &progress_tx,
                preview_tx: &preview_tx,
                cancel_flag: &cancel_flag,
                last_preview_time: &mut last_preview_time,
                preview_sent: &mut preview_sent,
                width,
                height,
                keyboard_pixels: &keyboard_pixels,
                kb_w,
                kb_h,
                recycle_tx: &recycle_tx,
            })
        }
    };
    let (processed_frames, cancelled, smoothed_fps) =
        pipeline.run(&mut enqueue_frame, &mut process_frame);
    // enqueue_frame 在此处释放（持有 csv_writer 可变借用），随后可 flush 计数器 CSV。

    // 完成编码：无论是否取消都必须调用 finish()，
    // 否则 FFmpeg 收不到 EOF，视频文件头未写入导致损坏。
    // 用户取消时已写入的帧仍可生成可播放的部分视频。
    let elapsed = start.elapsed().as_secs_f64();
    if let Some(writer) = ctx.csv_writer.as_mut() {
        use std::io::Write;
        if let Err(e) = writer.flush() {
            tracing::warn!("计数器 CSV 收尾失败: {e}");
        }
    }
    if cancelled {
        render_bar.finish(&format!(
            "已取消 | 已处理 {}/{} 帧 | 耗时 {:.1}s",
            processed_frames, total_frames, elapsed
        ));
    } else {
        render_bar.finish(&format!(
            "完成 {}/{} 帧 | 耗时 {:.1}s",
            processed_frames, total_frames, elapsed
        ));
    }
    finalize_video_export(
        &cmd_sender,
        encoder,
        cancelled,
        elapsed,
        total_frames,
        smoothed_fps,
        &progress_tx,
    );
}
