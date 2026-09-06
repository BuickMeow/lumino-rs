//! 视频导出窗口事件处理：在后台线程执行逐帧渲染 + FFmpeg 编码，进度通过通道回传主线程。

use crate::runner::{RunnerInner, dialog_manager::DialogType};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use lumino_export::video::{
    Container, EncoderBackend, QualityPreset, VideoCodec, VideoExportConfig,
};
use lumino_gfx::TextureFormat;
use lumino_message::events::window::video::VideoExportConfig as EventVideoExportConfig;

use memory_task::{RunVideoExportTaskInput, run_video_export_task};

mod commands;
mod composite;
mod frame;
mod memory_task;
mod pipeline;
/// 流式导出已暂时禁用（数据源统一期间保留代码供后续重做，见分支内 TODO）。
#[allow(dead_code)]
mod streaming_task;

impl RunnerInner {
    pub(crate) fn handle_start_video_export(
        &mut self,
        config: Box<EventVideoExportConfig>,
        document: Option<Arc<lumino_midi_loader::MidiDocument>>,
    ) {
        let EventVideoExportConfig {
            output_path,
            midi_path: _midi_path,
            width,
            height,
            fps,
            ppq,
            key_count,
            container,
            codec,
            backend,
            quality,
            render_mode,
            waterfall_scroll_speed,
            miditrail_z_far,
            miditrail_view_mode,
            miditrail_normal_speed,
            miditrail_top_speed,
            miditrail_3d_notes,
            note_counter,
            data_curve,
            midi_console,
        } = *config;

        // 事件层枚举 → 导出层枚举（总映射，无字符串解析、无静默降级）
        let container = match container {
            lumino_message::events::window::video::Container::Mp4 => Container::Mp4,
            lumino_message::events::window::video::Container::Mov => Container::Mov,
            lumino_message::events::window::video::Container::Mkv => Container::Mkv,
            lumino_message::events::window::video::Container::Avi => Container::Avi,
        };
        let codec = match codec {
            lumino_message::events::window::video::VideoCodec::H264 => VideoCodec::H264,
            lumino_message::events::window::video::VideoCodec::H265 => VideoCodec::H265,
            lumino_message::events::window::video::VideoCodec::ProRes => VideoCodec::ProRes,
            lumino_message::events::window::video::VideoCodec::Vp9 => VideoCodec::Vp9,
            lumino_message::events::window::video::VideoCodec::Av1 => VideoCodec::Av1,
        };
        let backend = match backend {
            lumino_message::events::window::video::EncoderBackend::Software => {
                EncoderBackend::Software
            }
            lumino_message::events::window::video::EncoderBackend::VideoToolbox => {
                EncoderBackend::VideoToolbox
            }
            lumino_message::events::window::video::EncoderBackend::Nvenc => EncoderBackend::Nvenc,
            lumino_message::events::window::video::EncoderBackend::Amf => EncoderBackend::Amf,
            lumino_message::events::window::video::EncoderBackend::Qsv => EncoderBackend::Qsv,
            lumino_message::events::window::video::EncoderBackend::Vaapi => EncoderBackend::Vaapi,
        };
        let quality = match quality {
            lumino_message::events::window::video::QualityPreset::High => QualityPreset::High,
            lumino_message::events::window::video::QualityPreset::Medium => QualityPreset::Medium,
            lumino_message::events::window::video::QualityPreset::Low => QualityPreset::Low,
        };

        tracing::info!(
            "开始视频导出: {}x{} @ {}fps, 容器={:?}, 编解码器={:?}",
            width,
            height,
            fps,
            container,
            codec
        );

        // 打开视频导出对话框（进度显示）
        self.window_state
            .dialog_manager
            .open_dialog(DialogType::VideoExport);

        // 获取渲染线程命令发送端与表面纹理格式（非 GPU compute 样式使用）
        let main_ui = self.window_state.window.ui();
        let cmd_sender = main_ui.render_command_sender();
        let surface_pix_fmt = match main_ui.texture_format() {
            TextureFormat::Bgra8Unorm | TextureFormat::Bgra8UnormSrgb => "bgra",
            TextureFormat::Rgba8Unorm | TextureFormat::Rgba8UnormSrgb => "rgba",
            _ => "bgra",
        };

        let Some(cmd_sender) = cmd_sender else {
            tracing::error!("视频导出失败：渲染线程未启动");
            let main_ui = self.window_state.window.ui_mut();
            main_ui.set_video_export_failed("渲染线程未启动".to_string());
            return;
        };

        // 创建进度通道（复用音频导出的进度通道机制）
        let (progress_tx, progress_rx) = tokio::sync::mpsc::unbounded_channel();
        self.window_state.export_progress_rx = Some(progress_rx);

        // 创建预览帧通道
        let (preview_tx, preview_rx) = tokio::sync::mpsc::unbounded_channel();
        self.window_state.video_preview_rx = Some(preview_rx);

        // 构建 VideoExportConfig
        let config = VideoExportConfig {
            width,
            height,
            fps: fps as f64,
            container,
            codec,
            backend,
            output_path: std::path::PathBuf::from(&output_path),
            quality,
        };

        let ppq = ppq.max(1) as u32;
        let fps_f64 = fps as f64;

        // 用编辑器 tempo_points 覆盖 doc 的加载时原始 tempo（与音频导出/保存一致）：
        // doc.tempo_changes 是加载文件时的值，用户经工程设置/速度面板修改的 BPM
        // 只写入 tempo_points、不回写 doc——不覆盖视频导出的总时长/帧调度/BPM 显示用旧值。
        let editor_tempos: Vec<(u32, f32)> = self
            .window_state
            .window
            .ui()
            .root()
            .editor
            .editor_state
            .data
            .tempo_points
            .iter()
            .map(|tp| (tp.tick.max(0.0) as u32, tp.bpm as f32))
            .collect();

        // 创建取消标志
        let cancel_flag = Arc::new(AtomicBool::new(false));
        self.window_state.video_export_cancel = Arc::clone(&cancel_flag);

        // 后台线程：逐帧渲染 + FFmpeg 编码
        let _ = std::thread::Builder::new()
            .name("video-render".into())
            .spawn(move || {
                let is_gpu_compute_style = matches!(
                    render_mode,
                    lumino_message::events::window::video::RenderMode::Waterfall
                        | lumino_message::events::window::video::RenderMode::MIDITrail
                );
                // 计数器模式：CPU 端渲染（无卷帘/键盘/标尺），帧数据为 BGRA 直出。
                let is_cpu_renderer = matches!(
                    render_mode,
                    lumino_message::events::window::video::RenderMode::NoteCounter
                        | lumino_message::events::window::video::RenderMode::DataCurve
                        | lumino_message::events::window::video::RenderMode::MidiConsole
                );
                // GPU compute / 3D 渲染输出为 rgba8unorm storage texture，
                // 因此编码器输入像素格式必须为 "rgba"；
                // 计数器 CPU 渲染直出 BGRA → "bgra"；
                // 其余（NoteRectangle）使用 UI 表面纹理，按表面格式选择。
                let input_pix_fmt = if is_gpu_compute_style {
                    "rgba"
                } else if is_cpu_renderer {
                    "bgra"
                } else {
                    surface_pix_fmt
                };
                // 计数器渲染配置（仅 NoteCounter 模式有效）
                let counter_config = if matches!(
                    render_mode,
                    lumino_message::events::window::video::RenderMode::NoteCounter
                ) {
                    Some(super::video_export::CounterRenderConfig::from(
                        &note_counter,
                    ))
                } else {
                    None
                };
                // 数据曲线渲染配置（仅 DataCurve 模式有效）
                let data_curve_config = if matches!(
                    render_mode,
                    lumino_message::events::window::video::RenderMode::DataCurve
                ) {
                    Some(super::video_export::DataCurveRenderConfig::from(
                        &data_curve,
                    ))
                } else {
                    None
                };
                // MidiConsole 风格渲染配置（仅 MidiConsole 模式有效）
                let midi_console_config = if matches!(
                    render_mode,
                    lumino_message::events::window::video::RenderMode::MidiConsole
                ) {
                    Some(super::video_export::MidiConsoleRenderConfig::from(
                        &midi_console,
                    ))
                } else {
                    None
                };
                if let Some(document) = document {
                    // Arc 快照不可变：克隆 owned 副本后覆盖 tempo。
                    // ChunkedList 为块级浅拷贝（O(块数) 指针拷贝），不复制音符数据。
                    let mut snapshot = (*document).clone();
                    if !editor_tempos.is_empty() {
                        snapshot.tempo_changes = editor_tempos;
                    }
                    run_video_export_task(RunVideoExportTaskInput {
                        config,
                        cmd_sender,
                        progress_tx,
                        preview_tx,
                        document: Arc::new(snapshot),
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
                    });
                } else if !_midi_path.is_empty() {
                    // TODO(数据源统一): 流式读取模式视频导出已暂时禁用，后续基于
                    // 单一权威数据源（GpuNoteBuffer / note_instances）重做流式路径。
                    tracing::error!("视频导出失败：流式读取模式已暂时禁用，请先加载工程");
                    send_export_error(
                        &progress_tx,
                        "导出失败：流式读取模式已暂时禁用，请先加载工程",
                    );
                } else {
                    tracing::error!("视频导出失败：无 MidiDocument 且未指定 MIDI 路径");
                    send_export_error(&progress_tx, "导出失败：无 MIDI 数据");
                }
            });
    }
}

/// 发送导出失败进度消息（progress=-1 表示失败，UI 据此弹出错误）。
///
/// 收敛各处重复的 5 元组 `("导出失败: ..", -1.0, 0, 0.0, 0.0)` 发送。
fn send_export_error(
    progress_tx: &tokio::sync::mpsc::UnboundedSender<(String, f64, u64, f64, f64)>,
    message: impl Into<String>,
) {
    let _ = progress_tx.send((message.into(), -1.0, 0, 0.0, 0.0));
}
