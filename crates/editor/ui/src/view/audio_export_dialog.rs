//! 音频导出对话框
//!
//! 所有 widget 使用 row!/column! 宏构建（含类型推断），
//! 避免手动构造 `iced_widget::Row::<M>::new()` 等导致的
//! 缺少 Theme/Renderer 泛型参数问题。
//! 类型别名一律使用 crate::Element<'a>（4 泛型参数已对齐）。

mod buttons;
mod event_filter;
mod output_path;
mod project_info;
mod title;

use iced_widget::{column, container, scrollable, space};

use crate::state::root_state::AudioExportDialogState;

use self::{
    buttons::buttons_section, event_filter::event_filter_section, output_path::output_path_section,
    project_info::project_info_section, title::title_section,
};

/// 渲染音频导出对话框
pub fn view_audio_export_dialog<'a>(
    state: &'a AudioExportDialogState,
    theme: &'a iced_core::Theme,
) -> crate::Element<'a> {
    let palette = theme.extended_palette();

    let main_content = column![
        title_section(palette),
        space().height(16),
        project_info_section(state, palette),
        space().height(16),
        audio_settings_section(state, palette),
        space().height(16),
        event_filter_section(state, palette),
        space().height(16),
        output_path_section(state, palette),
        space().height(24),
        buttons_section(state, palette),
    ];

    let scrollable_content = scrollable(main_content)
        .width(iced_core::Length::Fill)
        .height(iced_core::Length::Fill);

    let dialog_content = container(scrollable_content)
        .width(iced_core::Length::Fill)
        .height(iced_core::Length::Fill)
        .padding(24)
        .style(move |_t: &iced_core::Theme| {
            container::Style::default().background(palette.background.base.color)
        });

    dialog_content.into()
}

// ---------------------------------------------------------------------------
// 音频设置区域 — 内联实现
// 直接使用 row!/column! 宏（与 video_export_dialog/layout.rs 相同的模式）
// ---------------------------------------------------------------------------

use iced_core::{Alignment, Length};
use iced_widget::{checkbox, pick_list, row, text, text_input};

use crate::message::{AudioExportAction, Message};
use crate::state::root_state::{
    AudioBackend, AudioChannels, AudioFormat, Interpolation, ThreadingOption,
};
use crate::view::widgets;

fn audio_settings_section<'a>(
    state: &'a AudioExportDialogState,
    palette: &'a iced_core::theme::palette::Extended,
) -> crate::Element<'a> {
    let label_color = palette.background.neutral.text;
    let label_style = move |_t: &iced_core::Theme| text::Style {
        color: Some(label_color),
    };

    column![
        text("音频设置")
            .size(18)
            .font(iced_core::Font::with_name("Microsoft YaHei"))
            .style(widgets::dialog_label_style(palette)),
        space().height(12),
        // 渲染后端
        row![
            text("渲染后端:").size(14).style(label_style).width(120),
            pick_list(
                vec![AudioBackend::Cpu, AudioBackend::Gpu],
                Some(state.backend),
                |v| Message::AudioExport(AudioExportAction::BackendChanged(v)),
            )
            .width(Length::Fixed(200.0)),
        ]
        .spacing(8)
        .align_y(Alignment::Center),
        space().height(4),
        text("CPU 兼容性最好；GPU 需 Vulkan/Metal，无适配器时自动回退到 CPU，速度提升 2-5 倍")
            .size(11)
            .style(move |_t: &iced_core::Theme| text::Style {
                color: Some(palette.background.neutral.text.scale_alpha(0.7)),
            }),
        space().height(12),
        // 输出格式
        row![
            text("输出格式:").size(14).style(label_style).width(120),
            pick_list(
                vec![
                    AudioFormat::WAV,
                    AudioFormat::FLAC,
                    AudioFormat::MP3,
                    AudioFormat::Ogg,
                    AudioFormat::WavPack
                ],
                Some(state.format),
                |v| Message::AudioExport(AudioExportAction::FormatChanged(v)),
            )
            .width(Length::Fixed(200.0)),
        ]
        .spacing(8)
        .align_y(Alignment::Center),
        space().height(8),
        // 比特率
        row![
            text("比特率 (kbps):")
                .size(14)
                .style(label_style)
                .width(120),
            text_input("320", &state.audio_bitrate.to_string())
                .on_input(|v| Message::AudioExport(AudioExportAction::BitrateChanged(v)))
                .padding([6, 10])
                .width(Length::Fixed(200.0)),
        ]
        .spacing(8)
        .align_y(Alignment::Center),
        space().height(4),
        text("仅 MP3/Ogg 有效；WAV/FLAC 忽略此值")
            .size(11)
            .style(move |_t: &iced_core::Theme| text::Style {
                color: Some(palette.background.neutral.text.scale_alpha(0.6)),
            }),
        space().height(8),
        // 采样率
        row![
            text("采样率 (Hz):").size(14).style(label_style).width(120),
            pick_list(
                vec![22050u32, 44100, 48000, 96000],
                Some(state.sample_rate),
                |v| Message::AudioExport(AudioExportAction::SampleRateChanged(v)),
            )
            .width(Length::Fixed(200.0)),
        ]
        .spacing(8)
        .align_y(Alignment::Center),
        space().height(4),
        text("MP3 最高 48000 Hz，FLAC 最高 384000 Hz，超出会自动报错")
            .size(11)
            .style(move |_t: &iced_core::Theme| text::Style {
                color: Some(palette.background.neutral.text.scale_alpha(0.6)),
            }),
        space().height(8),
        // 通道数
        row![
            text("通道数:").size(14).style(label_style).width(120),
            pick_list(
                vec![AudioChannels::Mono, AudioChannels::Stereo],
                Some(state.channels),
                |v| Message::AudioExport(AudioExportAction::ChannelsChanged(v)),
            )
            .width(Length::Fixed(200.0)),
        ]
        .spacing(8)
        .align_y(Alignment::Center),
        space().height(8),
        // 层数限制
        row![
            text("最大复音数:").size(14).style(label_style).width(120),
            text_input("32", &state.layers.to_string())
                .on_input(|v| Message::AudioExport(AudioExportAction::LayersChanged(v)))
                .padding([6, 10])
                .width(Length::Fixed(200.0)),
        ]
        .spacing(8)
        .align_y(Alignment::Center),
        space().height(4),
        text("CPU 达到上限时丢弃最旧音符；GPU 上为物理池大小，0=无限制（黑MIDI推荐）")
            .size(11)
            .style(move |_t: &iced_core::Theme| text::Style {
                color: Some(palette.background.neutral.text.scale_alpha(0.6)),
            }),
        space().height(8),
        // 通道多线程（仅 CPU 有效）
        threading_row(
            "通道多线程 (CPU):",
            state.channel_threading,
            |v| Message::AudioExport(AudioExportAction::ChannelThreadingChanged(v)),
            palette
        ),
        space().height(8),
        // 按键多线程（仅 CPU 有效）
        threading_row(
            "按键多线程 (CPU):",
            state.key_threading,
            |v| Message::AudioExport(AudioExportAction::KeyThreadingChanged(v)),
            palette
        ),
        space().height(8),
        // 插值算法
        row![
            text("插值算法:").size(14).style(label_style).width(120),
            pick_list(
                vec![Interpolation::None, Interpolation::Linear],
                Some(state.interpolation),
                |v| Message::AudioExport(AudioExportAction::InterpolationChanged(v)),
            )
            .width(Length::Fixed(200.0)),
        ]
        .spacing(8)
        .align_y(Alignment::Center),
        space().height(4),
        text("无插值有混叠，线性为默认（GPU 固定线性，与 CPU 线性一致）")
            .size(11)
            .style(move |_t: &iced_core::Theme| text::Style {
                color: Some(palette.background.neutral.text.scale_alpha(0.6)),
            }),
        space().height(12),
        // 复选框
        checkbox(state.apply_limiter)
            .label("启用限幅器（大复音防削波，会轻微影响响度，建议开启）")
            .on_toggle(|v| Message::AudioExport(AudioExportAction::ApplyLimiterChanged(v)))
            .style(widgets::dialog_checkbox_style(palette)),
        space().height(4),
        checkbox(state.disable_fade_out)
            .label("禁用淡出（voice 被抢占时硬切，会产生咔哒声，不建议开启）")
            .on_toggle(|v| Message::AudioExport(AudioExportAction::DisableFadeOutChanged(v)))
            .style(widgets::dialog_checkbox_style(palette)),
        space().height(4),
        checkbox(state.linear_envelope)
            .label("线性包络（CPU 衰减/释音用线性，GPU 默认线性，关闭则全指数）")
            .on_toggle(|v| Message::AudioExport(AudioExportAction::LinearEnvelopeChanged(v)))
            .style(widgets::dialog_checkbox_style(palette)),
    ]
    .width(Length::Fill)
    .into()
}

fn threading_row<'a>(
    label_str: &'a str,
    selected: ThreadingOption,
    on_change: impl Fn(ThreadingOption) -> Message + 'a,
    palette: &'a iced_core::theme::palette::Extended,
) -> crate::Element<'a> {
    let label_color = palette.background.neutral.text;
    let label_style = move |_t: &iced_core::Theme| text::Style {
        color: Some(label_color),
    };

    row![
        text(label_str).size(14).style(label_style).width(120),
        pick_list(
            vec![
                ThreadingOption::None,
                ThreadingOption::Auto,
                ThreadingOption::Manual(2),
                ThreadingOption::Manual(4),
                ThreadingOption::Manual(8),
            ],
            Some(selected),
            on_change,
        )
        .width(Length::Fixed(200.0)),
    ]
    .spacing(8)
    .align_y(Alignment::Center)
    .into()
}
