//! 视频导出面板处理的辅助函数
//!
//! 纯函数（hex 解析、数值格式化、枚举/浮点解析回退）与配置组装逻辑，
//! 从 `video_export.rs` 拆分而来。

use std::str::FromStr;

use crate::host::DialogResult;
use crate::root::Root;
use crate::state::root_state::VideoExportOverlayState;
use crate::view::video_export_dialog::state::VideoExportDialogState;

/// 重置视频导出对话框的 overlay / 预览帧 / 缓存，并在对话框窗口中登记取消结果。
pub(crate) fn reset_video_export_overlay(root: &mut Root) {
    root.state.video_export_dialog.overlay = VideoExportOverlayState::None;
    root.state.video_export_dialog.preview_frame = None;
    root.state.video_export_dialog.cached_image_handle = None;
    if root.state.is_dialog_window {
        root.state.dialog_result = Some(DialogResult::Cancel);
    }
}

/// 解析 hex 颜色字符串为 RGBA（移植自 MIDIGraphRenderer 的 hexToRGB）。
///
/// 支持 `#RRGGBB`（alpha=255）与 `#RRGGBBAA` 两种格式；解析失败返回 `None`。
fn hex_to_rgba(hex: &str) -> Option<[u8; 4]> {
    let hex = hex.trim().trim_start_matches('#');
    let bytes = match hex.len() {
        6 => u32::from_str_radix(hex, 16).ok()?,
        8 => u32::from_str_radix(hex, 16).ok()?,
        _ => return None,
    };
    match hex.len() {
        6 => Some([
            ((bytes >> 16) & 0xFF) as u8,
            ((bytes >> 8) & 0xFF) as u8,
            (bytes & 0xFF) as u8,
            255,
        ]),
        _ => Some([
            ((bytes >> 24) & 0xFF) as u8,
            ((bytes >> 16) & 0xFF) as u8,
            ((bytes >> 8) & 0xFF) as u8,
            (bytes & 0xFF) as u8,
        ]),
    }
}

/// 将数值格式化为设置面板回显字符串（整数类字段不带小数位）。
pub(crate) fn fmt_num(value: f32, decimals: usize) -> String {
    if decimals == 0 {
        format!("{}", value as i64)
    } else {
        format!("{value:.decimals$}")
    }
}

/// 解析带日志回退的枚举（失败不再静默：记录 warn 便于诊断非法输入）。
fn parse_enum_or_log<T>(raw: &str, name: &str) -> T
where
    T: FromStr + Default,
{
    match raw.parse::<T>() {
        Ok(v) => v,
        Err(_) => {
            tracing::warn!("视频导出配置 {name} 非法值 '{raw}'，回退默认");
            T::default()
        }
    }
}

/// 解析带日志回退的 f32（默认值随调用点语义）。
fn parse_f32_or_log(raw: &str, name: &str, default: f32) -> f32 {
    match raw.parse::<f32>() {
        Ok(v) => v,
        Err(_) => {
            tracing::warn!("视频导出配置 {name} 非法值 '{raw}'，回退 {default}");
            default
        }
    }
}

/// 从对话框状态组装视频导出配置（纯函数，可单测）。
///
/// `ppq`/`key_count` 来自编辑器视图；非法输入回退默认值并记录 warn（见 `parse_enum_or_log`）。
pub(crate) fn build_video_config(
    st: &VideoExportDialogState,
    ppq: u16,
    key_count: u16,
) -> lumino_message::events::window::dialog::VideoExportConfig {
    use lumino_message::events::window::video::*;
    let counter_font = |mode: &str, family: String, path: String| match mode {
        "系统字体" => CounterFont::System { family },
        "自定义字体" => CounterFont::File { path },
        _ => CounterFont::Bitmap,
    };
    VideoExportConfig {
        output_path: st.output_path.clone(),
        midi_path: st.midi_path.clone(),
        width: st.width,
        height: st.height,
        fps: st.fps,
        ppq,
        key_count,
        container: parse_enum_or_log(&st.container, "container"),
        codec: parse_enum_or_log(&st.codec, "codec"),
        backend: parse_enum_or_log(&st.backend, "backend"),
        quality: parse_enum_or_log(&st.quality, "quality"),
        render_mode: parse_enum_or_log(&st.render_mode, "render_mode"),
        waterfall_scroll_speed: st.waterfall_speed,
        miditrail_view_mode: parse_enum_or_log(&st.miditrail_view_mode, "miditrail_view_mode"),
        miditrail_normal_speed: st.miditrail_normal_speed.max(0.1),
        miditrail_top_speed: st.miditrail_top_speed.max(0.1),
        miditrail_z_far: st.miditrail_z_far,
        miditrail_3d_notes: st.miditrail_3d_notes,
        note_counter: NoteCounterConfig {
            text: st.counter_text.clone(),
            alignment: parse_enum_or_log(&st.counter_alignment, "counter_alignment"),
            font_size: st.counter_font_size,
            font: counter_font(
                &st.counter_font_mode,
                st.counter_font_family.clone(),
                st.counter_font_path.clone(),
            ),
            separator: if st.counter_use_commas {
                CounterSeparator::Comma
            } else {
                CounterSeparator::Nothing
            },
            padding_zeroes: st.counter_padding_zeroes,
            bpm_int_pad: st.counter_bpm_int_pad,
            bpm_dec_pad: st.counter_bpm_dec_pad,
            note_count_pad: st.counter_note_count_pad,
            polyphony_pad: st.counter_polyphony_pad,
            nps_pad: st.counter_nps_pad,
            ticks_pad: st.counter_ticks_pad,
            bars_pad: st.counter_bars_pad,
            frames_pad: st.counter_frames_pad,
            save_csv: st.counter_save_csv,
            csv_output: st.counter_csv_output.clone(),
            csv_format: st.counter_csv_format.clone(),
        },
        data_curve: DataCurveConfig {
            metric: parse_enum_or_log(&st.dc_metric, "dc_metric"),
            graph_duration: parse_f32_or_log(&st.dc_graph_duration, "dc_graph_duration", 2.0)
                .clamp(0.5, 30.0),
            zoom_smoothness: parse_f32_or_log(&st.dc_zoom_smoothness, "dc_zoom_smoothness", 8.0)
                .clamp(1.0, 64.0),
            graph_smoothness: parse_f32_or_log(&st.dc_graph_smoothness, "dc_graph_smoothness", 0.0)
                .clamp(0.0, 32.0) as u32,
            padding_mul: parse_f32_or_log(&st.dc_padding_mul, "dc_padding_mul", 0.1)
                .clamp(0.0, 1.0),
            bg_color: hex_to_rgba(&st.dc_bg_color).unwrap_or([0, 0, 0, 255]),
            line_color: hex_to_rgba(&st.dc_line_color).unwrap_or([0, 255, 255, 255]),
            text_color: hex_to_rgba(&st.dc_text_color).unwrap_or([255, 255, 255, 127]),
            bar_color: hex_to_rgba(&st.dc_bar_color).unwrap_or([255, 255, 255, 127]),
            line_thickness: parse_f32_or_log(&st.dc_line_thickness, "dc_line_thickness", 3.0)
                .clamp(1.0, 20.0) as u32,
            bar_thickness: parse_f32_or_log(&st.dc_bar_thickness, "dc_bar_thickness", 1.0)
                .clamp(1.0, 10.0) as u32,
            font_size: st.dc_font_size,
            font: counter_font(
                &st.dc_font_mode,
                st.dc_font_family.clone(),
                st.dc_font_path.clone(),
            ),
            text_x_offset: parse_f32_or_log(&st.dc_text_x_offset, "dc_text_x_offset", 2.0)
                .clamp(0.0, 100.0) as u32,
            text_y_offset: parse_f32_or_log(&st.dc_text_y_offset, "dc_text_y_offset", 2.0)
                .clamp(0.0, 100.0) as u32,
            milestone_scale_mul: parse_f32_or_log(
                &st.dc_milestone_scale_mul,
                "dc_milestone_scale_mul",
                1.5,
            )
            .clamp(1.0, 5.0),
            abbreviate: st.dc_abbreviate,
            abbreviate_digits: parse_f32_or_log(
                &st.dc_abbreviate_digits,
                "dc_abbreviate_digits",
                3.0,
            )
            .clamp(0.0, 10.0) as u32,
            show_text: st.dc_show_text,
            show_bars: st.dc_show_bars,
        },
        midi_console: MidiConsoleConfig {
            render_backend: parse_enum_or_log(&st.midi_console_backend, "midi_console_backend"),
            ..MidiConsoleConfig::default()
        },
    }
}
