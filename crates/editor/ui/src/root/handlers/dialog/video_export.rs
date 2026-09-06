//! 视频导出面板处理器
//!
//! 事件分发逻辑集中在 `handle_video_export`，辅助函数见 `helpers` 子模块。

mod helpers;

use helpers::*;

use crate::message::{Message, VideoExportAction};
use crate::root::Root;
use crate::state::root_state::VideoExportOverlayState;
use crate::util::parse_uint;

use super::DialogHandler;

impl DialogHandler {
    pub(super) fn handle_video_export(
        &self,
        root: &mut Root,
        action: VideoExportAction,
    ) -> Option<Message> {
        use VideoExportAction as V;
        match action {
            V::OpenPanel => {
                root.sidebar.video_export_visible = true;
                root.sidebar.route = crate::sidebar::Route::VideoExport;
            }
            V::ClosePanel => {
                root.sidebar.video_export_visible = false;
                root.sidebar.route = crate::sidebar::Route::Arrangement;
            }
            V::StartExport => {
                // 内存模式优先：从 EditorData.document 克隆快照（ChunkedList Arc 共享）；
                // 否则若指定了 MIDI 路径，进入流式读取模式。
                let document = root
                    .editor
                    .editor_state
                    .data
                    .document
                    .as_ref()
                    .map(|doc| std::sync::Arc::new(doc.clone()));
                let midi_source = {
                    let st = &root.state.video_export_dialog;
                    if document.is_some() {
                        tracing::info!("视频导出使用内存 MidiDocument（零拷贝模式）");
                        "内存模式".to_string()
                    } else if !st.midi_path.is_empty() {
                        tracing::info!("视频导出使用 MIDI 文件流式读取: {}", st.midi_path);
                        "流式读取".to_string()
                    } else {
                        tracing::warn!("视频导出未配置 MIDI 数据源");
                        "未配置".to_string()
                    }
                };

                // 设置导出中状态
                root.state.video_export_dialog.overlay = VideoExportOverlayState::Exporting;
                root.state.video_export_dialog.progress = 0.0;
                root.state.video_export_dialog.status_message =
                    format!("正在初始化... [{}]", midi_source);
                root.state.video_export_dialog.current_frame = 0;
                root.state.video_export_dialog.total_frames = 0;
                root.state.video_export_dialog.render_fps = 0.0;
                root.state.video_export_dialog.cached_image_handle = None;

                let st = &root.state.video_export_dialog;
                let video_config = build_video_config(
                    st,
                    root.editor.editor_state.view.ppq,
                    root.editor.editor_state.view.visible_key_count,
                );
                let ev = crate::event::window::Event::start_video_export(video_config, document);
                crate::event::emit(crate::event::Event::Window(ev));
            }
            V::CancelExport => {
                reset_video_export_overlay(root);
                // 通知 Runner 取消导出（关闭对话框 → 设置取消标志 → 后台线程退出）
                crate::event::emit(crate::event::Event::Window(
                    crate::event::window::Event::close_video_export_dialog(),
                ));
            }
            V::ForceFinish => {
                let st = &root.state.video_export_dialog;
                root.state.video_export_dialog.overlay = VideoExportOverlayState::Completed {
                    total_frames: st.total_frames,
                    elapsed_secs: st.elapsed_secs,
                    avg_fps: st.render_fps,
                };
            }
            V::DismissOverlay => {
                reset_video_export_overlay(root);
            }
            V::ContainerChanged(v) => {
                root.state.video_export_dialog.container = v;
            }
            V::CodecChanged(v) => {
                root.state.video_export_dialog.codec = v;
            }
            V::BackendChanged(v) => {
                root.state.video_export_dialog.backend = v;
            }
            V::QualityChanged(v) => {
                root.state.video_export_dialog.quality = v;
            }
            V::WidthChanged(v) => {
                if let Some(w) = parse_uint(&v) {
                    root.state.video_export_dialog.width = w;
                }
            }
            V::HeightChanged(v) => {
                if let Some(h) = parse_uint(&v) {
                    root.state.video_export_dialog.height = h;
                }
            }
            V::RenderModeChanged(v) => {
                root.state.video_export_dialog.render_mode = v;
            }
            V::MidiConsoleBackendChanged(v) => {
                root.state.video_export_dialog.midi_console_backend = v;
            }
            V::WaterfallSpeedChanged(v) => {
                root.state.video_export_dialog.waterfall_speed = v;
            }
            V::MiditrailViewModeChanged(v) => {
                // 仅接受 Normal/Top，非预期值回退 Normal（防两套设置污染）。
                root.state.video_export_dialog.miditrail_view_mode = match v.as_str() {
                    "Top" => "Top".to_string(),
                    _ => "Normal".to_string(),
                };
            }
            V::MiditrailNormalSpeedChanged(v) => {
                root.state.video_export_dialog.miditrail_normal_speed = v;
            }
            V::MiditrailTopSpeedChanged(v) => {
                root.state.video_export_dialog.miditrail_top_speed = v;
            }
            V::MiditrailZFarChanged(v) => {
                root.state.video_export_dialog.miditrail_z_far = v;
            }
            V::Miditrail3DNotesChanged(v) => {
                root.state.video_export_dialog.miditrail_3d_notes = v;
            }
            V::CounterTextAction(action) => {
                let st = &mut root.state.video_export_dialog;
                st.counter_editor.perform(action);
                st.counter_text = st.counter_editor.text();
            }
            V::CounterAlignmentChanged(v) => {
                root.state.video_export_dialog.counter_alignment = v;
            }
            V::CounterFontSizeChanged(v) => {
                root.state.video_export_dialog.counter_font_size = v.clamp(7, 512);
            }
            V::CounterFontModeChanged(v) => {
                root.state.video_export_dialog.counter_font_mode = v;
            }
            V::CounterFontFamilyChanged(v) => {
                root.state.video_export_dialog.counter_font_family = v;
            }
            V::CounterFontPathChanged(v) => {
                root.state.video_export_dialog.counter_font_path = v;
            }
            V::CounterBrowseFont => {
                // 浏览字体文件（TTF/OTF/TTC），选择后写入路径
                if let Some(path) = rfd::FileDialog::new()
                    .add_filter("字体文件", &["ttf", "otf", "ttc"])
                    .pick_file()
                {
                    root.state.video_export_dialog.counter_font_path =
                        path.to_string_lossy().to_string();
                }
            }
            V::CounterUseCommasChanged(v) => {
                root.state.video_export_dialog.counter_use_commas = v;
            }
            V::CounterPaddingZeroesChanged(v) => {
                root.state.video_export_dialog.counter_padding_zeroes = v;
            }
            V::CounterSaveCsvChanged(v) => {
                root.state.video_export_dialog.counter_save_csv = v;
            }
            V::CounterCsvPathChanged(v) => {
                root.state.video_export_dialog.counter_csv_output = v;
            }
            V::CounterCsvFormatChanged(v) => {
                root.state.video_export_dialog.counter_csv_format = v;
            }
            V::CounterPadChanged { field, value } => {
                let st = &mut root.state.video_export_dialog;
                match field.as_str() {
                    "bpm_int" => st.counter_bpm_int_pad = value,
                    "bpm_dec" => st.counter_bpm_dec_pad = value.min(12),
                    "nc" => st.counter_note_count_pad = value,
                    "plph" => st.counter_polyphony_pad = value,
                    "nps" => st.counter_nps_pad = value,
                    "ticks" => st.counter_ticks_pad = value,
                    "bars" => st.counter_bars_pad = value,
                    "frames" => st.counter_frames_pad = value,
                    _ => {}
                }
            }
            V::CounterResetPadding => {
                let st = &mut root.state.video_export_dialog;
                st.counter_bpm_int_pad = 3;
                st.counter_bpm_dec_pad = 2;
                st.counter_note_count_pad = 5;
                st.counter_polyphony_pad = 3;
                st.counter_nps_pad = 3;
                st.counter_ticks_pad = 5;
                st.counter_bars_pad = 3;
                st.counter_frames_pad = 5;
            }
            V::CounterLoadTemplate(name) => {
                let st = &mut root.state.video_export_dialog;
                let template = match name.as_str() {
                    "full" => crate::state::root_state::COUNTER_FULL_TEXT,
                    _ => crate::state::root_state::COUNTER_DEFAULT_TEXT,
                };
                st.counter_text = template.to_string();
                st.counter_editor = iced_widget::text_editor::Content::with_text(template);
            }
            V::CounterResetText => {
                let st = &mut root.state.video_export_dialog;
                st.counter_text = crate::state::root_state::COUNTER_DEFAULT_TEXT.to_string();
                st.counter_editor = iced_widget::text_editor::Content::with_text(&st.counter_text);
            }
            V::CounterBrowseCsv => {
                if let Some(path) = rfd::FileDialog::new()
                    .set_file_name("notecounter.csv")
                    .add_filter("CSV 数据文件", &["csv"])
                    .save_file()
                {
                    root.state.video_export_dialog.counter_csv_output =
                        path.to_string_lossy().to_string();
                }
            }
            V::DataCurveNumberChanged { field, value } => {
                let st = &mut root.state.video_export_dialog;
                match field.as_str() {
                    "graph_duration" => st.dc_graph_duration = fmt_num(value, 2),
                    "zoom_smoothness" => st.dc_zoom_smoothness = fmt_num(value, 2),
                    "graph_smoothness" => st.dc_graph_smoothness = fmt_num(value, 0),
                    "padding_mul" => st.dc_padding_mul = fmt_num(value, 2),
                    "line_thickness" => st.dc_line_thickness = fmt_num(value, 0),
                    "bar_thickness" => st.dc_bar_thickness = fmt_num(value, 0),
                    "text_x_offset" => st.dc_text_x_offset = fmt_num(value, 0),
                    "text_y_offset" => st.dc_text_y_offset = fmt_num(value, 0),
                    "milestone_scale_mul" => st.dc_milestone_scale_mul = fmt_num(value, 2),
                    "abbreviate_digits" => st.dc_abbreviate_digits = fmt_num(value, 0),
                    _ => {}
                }
            }
            V::DataCurveBoolChanged { field, value } => {
                let st = &mut root.state.video_export_dialog;
                match field.as_str() {
                    "abbreviate" => st.dc_abbreviate = value,
                    "show_text" => st.dc_show_text = value,
                    "show_bars" => st.dc_show_bars = value,
                    _ => {}
                }
            }
            V::DataCurveTextChanged { field, value } => {
                let st = &mut root.state.video_export_dialog;
                match field.as_str() {
                    "metric" => st.dc_metric = value,
                    "bg_color" => st.dc_bg_color = value,
                    "line_color" => st.dc_line_color = value,
                    "text_color" => st.dc_text_color = value,
                    "bar_color" => st.dc_bar_color = value,
                    "font_family" => st.dc_font_family = value,
                    "font_path" => st.dc_font_path = value,
                    _ => {}
                }
            }
            V::DataCurveFontSizeChanged(v) => {
                root.state.video_export_dialog.dc_font_size = v.clamp(7, 256);
            }
            V::DataCurveFontModeChanged(v) => {
                root.state.video_export_dialog.dc_font_mode = v;
            }
            V::DataCurveBrowseFont => {
                if let Some(path) = rfd::FileDialog::new()
                    .add_filter("字体文件", &["ttf", "otf", "ttc"])
                    .pick_file()
                {
                    root.state.video_export_dialog.dc_font_path =
                        path.to_string_lossy().to_string();
                }
            }
            V::FpsChanged(v) => {
                root.state.video_export_dialog.fps = v;
            }
            V::OutputPathChanged(v) => {
                root.state.video_export_dialog.output_path = v;
            }
            V::BrowseOutput => {
                let st = &root.state.video_export_dialog;
                let ext = st.container.to_lowercase();
                let default_name = format!("output.{}", ext);
                if let Some(path) = rfd::FileDialog::new()
                    .set_file_name(&default_name)
                    .add_filter(&st.container, &[ext.as_str()])
                    .save_file()
                {
                    root.state.video_export_dialog.output_path = path.to_string_lossy().to_string();
                }
            }
            V::MidiPathChanged(v) => {
                root.state.video_export_dialog.midi_path = v;
            }
            V::BrowseMidi => {
                if let Some(path) = rfd::FileDialog::new()
                    .add_filter("音乐文件", &["mid", "midi", "lmpj"])
                    .add_filter("MIDI 文件", &["mid", "midi"])
                    .add_filter("Lumino 项目", &["lmpj"])
                    .add_filter("所有文件", &["*"])
                    .pick_file()
                {
                    root.state.video_export_dialog.midi_path = path.to_string_lossy().to_string();
                }
            }
            V::UpdateProgress {
                message,
                progress,
                current_frame,
                total_frames,
                fps,
            } => {
                let st = &mut root.state.video_export_dialog;
                st.status_message = message;
                st.progress = progress;
                st.current_frame = current_frame;
                st.total_frames = total_frames;
                st.render_fps = fps;
            }
            V::ExportCompleted => {
                // 由 Runner 回调设置具体字段，此处不处理
            }
            V::ExportFailed(err) => {
                root.state.video_export_dialog.overlay = VideoExportOverlayState::Error(err);
            }
            V::UpdatePreviewFrame { .. } => {
                // 由 Host 直接处理，此处不需要
            }
        }
        None
    }
}
