//! 视频导出对话框主视图区块
//!
//! 分解 `render_settings_section` 为多个 ≤50 行的子函数。

use iced_core::{Alignment, Length};
use iced_widget::{checkbox, column, container, row, scrollable, slider, space, text, text_input};

use crate::message::{Message, VideoExportAction};
use crate::view::widgets;

use super::helpers;
use super::layout::{
    buttons_section, midi_source_section, output_path_section, pick_list_row, title_section,
};
use super::state::{MIDITRAIL_Z_FAR_MAX, VideoExportDialogState, VideoExportOverlayState};

/// 渲染设置区域（容器格式、编码器、硬件加速、质量、分辨率、帧率）
fn render_settings_section<'a>(
    state: &'a VideoExportDialogState,
    palette: &'a iced_core::theme::palette::Extended,
) -> crate::Element<'a> {
    let mut content: crate::Element<'a> = column![
        text("渲染设置")
            .size(16)
            .font(iced_core::Font::with_name("Microsoft YaHei"))
            .style(widgets::dialog_label_style(palette)),
        space().height(12),
        render_format_options(state, palette),
        space().height(8),
        render_quality_mode_options(state, palette),
        space().height(8),
        waterfall_speed_slider_row(state, palette),
        miditrail_view_mode_row(state, palette),
        miditrail_z_far_slider_row(state, palette),
        miditrail_3d_notes_checkbox_row(state, palette),
        space().height(8),
        resolution_input_row(state, palette),
        pick_list_row(
            "帧率:",
            100.0,
            vec![24u32, 30, 60, 120],
            Some(state.fps),
            |v| Message::VideoExport(VideoExportAction::FpsChanged(v)),
            palette,
        ),
    ]
    .width(Length::Fill)
    .into();

    // 计数器模式附加设置（参考 Zenith-MIDI NoteCountRender 设置面板）
    if state.render_mode == "计数器" {
        content = column![
            content,
            space().height(12),
            super::counter_settings::counter_settings_section(state, palette),
        ]
        .width(Length::Fill)
        .into();
    }
    // 数据曲线模式附加设置（参考 MIDIGraphRenderer graph 设置面板）
    if state.render_mode == "数据曲线" {
        content = column![
            content,
            space().height(12),
            super::data_curve_settings::data_curve_settings_section(state, palette),
        ]
        .width(Length::Fill)
        .into();
    }
    // MidiConsole 模式附加设置（渲染后端 GPU/CPU 切换，默认 GPU）
    if state.render_mode == "MidiConsole" {
        content = column![
            content,
            space().height(12),
            midi_console_settings_section(state, palette),
        ]
        .width(Length::Fill)
        .into();
    }
    content
}

/// 渲染格式选项（容器格式、编码器、加速后端）
fn render_format_options<'a>(
    state: &'a VideoExportDialogState,
    palette: &'a iced_core::theme::palette::Extended,
) -> crate::Element<'a> {
    let containers = vec!["MP4", "MOV", "MKV", "AVI"]
        .into_iter()
        .map(String::from)
        .collect::<Vec<_>>();
    let codecs = vec!["H.264", "H.265 / HEVC", "ProRes", "VP9", "AV1"]
        .into_iter()
        .map(String::from)
        .collect::<Vec<_>>();
    let backends = helpers::available_backends();

    column![
        pick_list_row(
            "渲染格式:",
            100.0,
            containers,
            Some(state.container.clone()),
            |v| Message::VideoExport(VideoExportAction::ContainerChanged(v)),
            palette,
        ),
        space().height(8),
        pick_list_row(
            "编码器:",
            100.0,
            codecs,
            Some(state.codec.clone()),
            |v| Message::VideoExport(VideoExportAction::CodecChanged(v)),
            palette,
        ),
        space().height(8),
        pick_list_row(
            "加速:",
            100.0,
            backends,
            Some(state.backend.clone()),
            |v| Message::VideoExport(VideoExportAction::BackendChanged(v)),
            palette,
        ),
    ]
    .into()
}

/// 渲染质量与模式选项（质量、渲染风格）
fn render_quality_mode_options<'a>(
    state: &'a VideoExportDialogState,
    palette: &'a iced_core::theme::palette::Extended,
) -> crate::Element<'a> {
    let qualities = vec!["高", "中", "低"]
        .into_iter()
        .map(String::from)
        .collect::<Vec<_>>();

    column![
        pick_list_row(
            "质量:",
            100.0,
            qualities,
            Some(state.quality.clone()),
            |v| Message::VideoExport(VideoExportAction::QualityChanged(v)),
            palette,
        ),
        space().height(8),
        pick_list_row(
            "渲染风格:",
            100.0,
            vec![
                "Lumino瀑布流".to_string(),
                "音符矩形".to_string(),
                "MIDITrail".to_string(),
                "计数器".to_string(),
                "数据曲线".to_string(),
                "MidiConsole".to_string(),
            ],
            Some(state.render_mode.clone()),
            |v| Message::VideoExport(VideoExportAction::RenderModeChanged(v)),
            palette,
        ),
    ]
    .into()
}

/// MidiConsole 模式附加设置（渲染后端 GPU/CPU 切换）
fn midi_console_settings_section<'a>(
    state: &'a VideoExportDialogState,
    palette: &'a iced_core::theme::palette::Extended,
) -> crate::Element<'a> {
    column![
        text("MidiConsole 设置")
            .size(16)
            .font(iced_core::Font::with_name("Microsoft YaHei"))
            .style(widgets::dialog_label_style(palette)),
        space().height(12),
        pick_list_row(
            "渲染后端:",
            100.0,
            vec!["GPU".to_string(), "CPU".to_string()],
            Some(state.midi_console_backend.clone()),
            |v| Message::VideoExport(VideoExportAction::MidiConsoleBackendChanged(v)),
            palette,
        ),
    ]
    .into()
}

/// 滚动速度滑杆（按渲染模式/视图隔离绑定，避免两套设置互相污染）。
///
/// - Waterfall 模式 → `waterfall_speed`
/// - MIDITrail + Normal → `miditrail_normal_speed`（由旧共享速度迁移而来）
/// - MIDITrail + Top → `miditrail_top_speed`
fn waterfall_speed_slider_row<'a>(
    state: &'a VideoExportDialogState,
    palette: &'a iced_core::theme::palette::Extended,
) -> crate::Element<'a> {
    let label_color = palette.background.neutral.text;
    let label_style = move |_t: &iced_core::Theme| text::Style {
        color: Some(label_color),
    };

    // 同一个滑杆按当前模式/视图绑定到各自独立的速度值（音符显示距离除外，
    // 其余设置均按视图隔离；见 VIEW-001）。
    let is_miditrail = state.render_mode == "MIDITrail";
    let is_top = state.miditrail_view_mode == "Top";
    let value = if is_miditrail {
        if is_top {
            state.miditrail_top_speed
        } else {
            state.miditrail_normal_speed
        }
    } else {
        state.waterfall_speed
    };

    row![
        text("下落速度:").size(14).style(label_style).width(100),
        slider(0.1..=10.0, value, move |v| {
            Message::VideoExport(if is_miditrail {
                if is_top {
                    VideoExportAction::MiditrailTopSpeedChanged(v)
                } else {
                    VideoExportAction::MiditrailNormalSpeedChanged(v)
                }
            } else {
                VideoExportAction::WaterfallSpeedChanged(v)
            })
        })
        .step(0.1_f32)
        .width(200.0),
        text(format!("{value:.1}x")).size(14).style(label_style),
    ]
    .spacing(8)
    .align_y(Alignment::Center)
    .into()
}

/// MIDITrail 视图模式切换入口（Normal 普通 / Top 顶部，仅 MIDITrail 时显示）。
fn miditrail_view_mode_row<'a>(
    state: &'a VideoExportDialogState,
    palette: &'a iced_core::theme::palette::Extended,
) -> crate::Element<'a> {
    if state.render_mode == "MIDITrail" {
        pick_list_row(
            "视图模式:",
            100.0,
            vec!["Normal".to_string(), "Top".to_string()],
            Some(state.miditrail_view_mode.clone()),
            |v| Message::VideoExport(VideoExportAction::MiditrailViewModeChanged(v)),
            palette,
        )
    } else {
        space().height(0).into()
    }
}

/// MIDITrail Z 方向显示距离滑杆（仅在选择 MIDITrail 时显示）
fn miditrail_z_far_slider_row<'a>(
    state: &'a VideoExportDialogState,
    palette: &'a iced_core::theme::palette::Extended,
) -> crate::Element<'a> {
    let label_color = palette.background.neutral.text;
    let label_style = move |_t: &iced_core::Theme| text::Style {
        color: Some(label_color),
    };

    if state.render_mode == "MIDITrail" {
        row![
            text("Z 显示距离:").size(14).style(label_style).width(100),
            slider(0.1..=MIDITRAIL_Z_FAR_MAX, state.miditrail_z_far, |v| {
                Message::VideoExport(VideoExportAction::MiditrailZFarChanged(v))
            })
            .step(0.1_f32)
            .width(200.0),
            text(format!("{:.1}", state.miditrail_z_far))
                .size(14)
                .style(label_style),
        ]
        .spacing(8)
        .align_y(Alignment::Center)
        .into()
    } else {
        space().height(0).into()
    }
}

/// MIDITrail 3D 音符开关（仅 MIDITrail 时显示；默认关 = 平面，12→2 三角形/音符）。
fn miditrail_3d_notes_checkbox_row<'a>(
    state: &'a VideoExportDialogState,
    palette: &'a iced_core::theme::palette::Extended,
) -> crate::Element<'a> {
    if state.render_mode == "MIDITrail" {
        checkbox(state.miditrail_3d_notes)
            .label("3D 音符（盒子，关闭则为平面）")
            .on_toggle(|v| Message::VideoExport(VideoExportAction::Miditrail3DNotesChanged(v)))
            .style(widgets::dialog_checkbox_style(palette))
            .into()
    } else {
        space().height(0).into()
    }
}
fn resolution_input_row<'a>(
    state: &'a VideoExportDialogState,
    palette: &'a iced_core::theme::palette::Extended,
) -> crate::Element<'a> {
    let width_str = state.width.to_string();
    let height_str = state.height.to_string();

    row![
        text("分辨率:")
            .size(14)
            .style(widgets::dialog_label_style(palette))
            .width(100),
        container(
            text_input("1920", &width_str)
                .on_input(|v| Message::VideoExport(VideoExportAction::WidthChanged(v)))
                .padding([6, 10])
                .width(Length::Fixed(80.0)),
        )
        .style(widgets::dialog_input_style(palette)),
        text("x")
            .size(14)
            .style(widgets::dialog_label_style(palette)),
        container(
            text_input("1080", &height_str)
                .on_input(|v| Message::VideoExport(VideoExportAction::HeightChanged(v)))
                .padding([6, 10])
                .width(Length::Fixed(80.0)),
        )
        .style(widgets::dialog_input_style(palette)),
    ]
    .spacing(8)
    .align_y(Alignment::Center)
    .into()
}

// ── 公开入口 ────────────────────────────────────────────────

/// 渲染视频导出配置面板（侧边栏面板）
pub fn view_video_export_dialog<'a>(
    state: &'a VideoExportDialogState,
    theme: &'a iced_core::Theme,
) -> crate::Element<'a> {
    let palette = theme.extended_palette();

    let main_content = column![
        title_section(palette),
        space().height(16),
        midi_source_section(state, palette),
        space().height(16),
        render_settings_section(state, palette),
        space().height(16),
        output_path_section(state, palette),
        space().height(24),
        buttons_section(palette),
    ];

    let scrollable_content = scrollable(main_content)
        .width(Length::Fill)
        .height(Length::Fill);

    container(scrollable_content)
        .width(Length::Fill)
        .height(Length::Fill)
        .padding(24)
        .style(move |_t: &iced_core::Theme| {
            container::Style::default().background(palette.background.base.color)
        })
        .into()
}

/// 渲染视频导出覆盖层（浮动 dialog 弹出样式）
pub fn view_video_export_overlay<'a>(
    state: &'a VideoExportDialogState,
    theme: &'a iced_core::Theme,
) -> Option<crate::Element<'a>> {
    if matches!(state.overlay, VideoExportOverlayState::None) {
        return None;
    }
    let palette = theme.extended_palette();

    let content: crate::Element<'a> = match &state.overlay {
        VideoExportOverlayState::Exporting => {
            super::handlers::exporting_overlay(state, theme, palette)
        }
        VideoExportOverlayState::Finalizing => {
            super::handlers::finalizing_overlay(state, theme, palette)
        }
        VideoExportOverlayState::Completed {
            total_frames,
            elapsed_secs,
            avg_fps,
        } => super::handlers::completed_overlay(
            state,
            *total_frames,
            *elapsed_secs,
            *avg_fps,
            palette,
        ),
        VideoExportOverlayState::Error(err) => super::handlers::error_overlay(err.clone(), palette),
        VideoExportOverlayState::None => return None,
    };

    let full = container(content)
        .width(Length::Fill)
        .height(Length::Fill)
        .padding(20)
        .style(move |_t: &iced_core::Theme| container::Style {
            background: Some(palette.background.base.color.into()),
            ..Default::default()
        });

    Some(full.into())
}
