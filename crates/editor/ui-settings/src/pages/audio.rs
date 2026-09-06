//! 设置页面 - 音频设置

use iced_core::{Alignment, Length};
use iced_widget::{column, pick_list, row, text, text_input};
use lumino_ui_core::{Element, Message, Theme};

use super::super::components::constants::*;
use super::super::components::styles::{create_content_text_style, create_placeholder_text_style};
use crate::SettingsPanel;
use lumino_core::storage::config::SynthBackend;
use lumino_extras::i18n::Language;
use lumino_extras::i18n::settings_translations;
use lumino_ui_core::settings_event::OutputType;

/// 本地化输出类型（顶层 MIDI 输出类型）
#[derive(Debug, Clone, Copy)]
struct LocalizedOutputType {
    inner: OutputType,
    name: &'static str,
}

impl PartialEq for LocalizedOutputType {
    fn eq(&self, other: &Self) -> bool {
        self.inner == other.inner
    }
}

impl Eq for LocalizedOutputType {}

impl std::fmt::Display for LocalizedOutputType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name)
    }
}

impl LocalizedOutputType {
    fn new(ot: OutputType, lang: Language) -> Self {
        let name = match ot {
            OutputType::Builtin => match lang {
                Language::ZhCn => "内置合成器",
                Language::EnUs => "Built-in Synth",
            },
            OutputType::Kdmapi => "KDMAPI",
            OutputType::System => match lang {
                Language::ZhCn => "系统 MIDI",
                Language::EnUs => "System MIDI",
            },
        };
        Self { inner: ot, name }
    }
}

/// 本地化内置合成器引擎（内置类型下的子下拉，与 xsynth-realtime 共用同一列表）
#[derive(Debug, Clone, Copy)]
struct LocalizedBuiltinEngine {
    inner: SynthBackend,
    name: &'static str,
}

impl PartialEq for LocalizedBuiltinEngine {
    fn eq(&self, other: &Self) -> bool {
        self.inner == other.inner
    }
}

impl Eq for LocalizedBuiltinEngine {}

impl std::fmt::Display for LocalizedBuiltinEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name)
    }
}

impl LocalizedBuiltinEngine {
    fn new(b: SynthBackend, lang: Language) -> Self {
        let name = match b {
            SynthBackend::XSynth => match lang {
                Language::ZhCn => "XSynth (Realtime)",
                Language::EnUs => "XSynth (Realtime)",
            },
            SynthBackend::Lgs => "LGS (GPU)",
            _ => "Unknown",
        };
        Self { inner: b, name }
    }
}

/// 渲染音频设置页面
pub fn view<'a>(settings: &'a SettingsPanel) -> Element<'a> {
    let t = settings_translations(settings.display.language);

    // 顶层：MIDI 输出类型（内置合成器 / KDMAPI / 系统 MIDI）
    let current_output_type = if matches!(
        settings.synth.backend,
        SynthBackend::XSynth | SynthBackend::Lgs
    ) {
        LocalizedOutputType::new(OutputType::Builtin, settings.display.language)
    } else {
        LocalizedOutputType::new(
            match settings.synth.backend {
                SynthBackend::Kdmapi => OutputType::Kdmapi,
                SynthBackend::System => OutputType::System,
                _ => OutputType::Builtin,
            },
            settings.display.language,
        )
    };
    let output_type_options = vec![
        LocalizedOutputType::new(OutputType::Builtin, settings.display.language),
        LocalizedOutputType::new(OutputType::Kdmapi, settings.display.language),
        LocalizedOutputType::new(OutputType::System, settings.display.language),
    ];

    // 内置合成器引擎子下拉（与 xsynth-realtime 共用同一列表）
    let show_builtin_engine = matches!(
        settings.synth.backend,
        SynthBackend::XSynth | SynthBackend::Lgs
    );
    let builtin_engine_options = vec![
        LocalizedBuiltinEngine::new(SynthBackend::XSynth, settings.display.language),
        LocalizedBuiltinEngine::new(SynthBackend::Lgs, settings.display.language),
    ];
    let current_builtin_engine = LocalizedBuiltinEngine::new(
        if show_builtin_engine {
            settings.synth.backend
        } else {
            SynthBackend::XSynth
        },
        settings.display.language,
    );

    let mut col = column![
        text(t.audio_title)
            .size(TEXT_SIZE_TITLE)
            .style(create_content_text_style()),
        iced_widget::space().height(20),
        // 输出类型选择
        row![
            text(t.synthesizer)
                .size(TEXT_SIZE_CONTENT)
                .style(create_content_text_style()),
            iced_widget::space().width(SPACING_MAIN),
            pick_list(output_type_options, Some(current_output_type), |ot| {
                Message::Settings(crate::Event::OutputTypeChanged(ot.inner))
            })
            .width(200.0),
        ]
        .spacing(SPACING_ICON_LABEL)
        .align_y(Alignment::Center),
        iced_widget::space().height(SPACING_CONTENT),
    ];

    // 内置合成器引擎子下拉（仅内置类型显示）
    if show_builtin_engine {
        col = col.push(
            row![
                text(t.builtin_engine)
                    .size(TEXT_SIZE_CONTENT)
                    .style(create_content_text_style()),
                iced_widget::space().width(SPACING_MAIN),
                pick_list(builtin_engine_options, Some(current_builtin_engine), |be| {
                    Message::Settings(crate::Event::SynthBackendChanged(be.inner))
                })
                .width(200.0),
            ]
            .spacing(SPACING_ICON_LABEL)
            .align_y(Alignment::Center),
        );
        col = col.push(iced_widget::space().height(SPACING_CONTENT));
    }

    // 音频播放输出设备（CPAL 音频设备）选择器：仅对软件合成器（内置引擎）生效
    if show_builtin_engine {
        col = col.push(render_audio_output_selector(settings, t));
        col = col.push(iced_widget::space().height(SPACING_CONTENT));
    }

    // MIDI 输入设备选择
    col = col.push(render_midi_device_selector(settings, t));
    col = col.push(iced_widget::space().height(SPACING_CONTENT));

    // 只在对应模式下显示音色库选择 / 提示
    if settings.synth.backend == SynthBackend::XSynth {
        col = col.push(render_xsynth_options(settings, t));
    } else if settings.synth.backend == SynthBackend::Lgs {
        col = col.push(render_lgs_options(settings, t));
    } else if settings.synth.backend == SynthBackend::Kdmapi {
        col = col.push(
            text(t.kdmapi_hint)
                .size(TEXT_SIZE_CONTENT)
                .style(create_placeholder_text_style()),
        );
    } else if settings.synth.backend == SynthBackend::System {
        col = col.push(render_winmm_output_selector(settings, t));
    }

    col.spacing(SPACING_CONTENT).padding(PADDING_CONTENT).into()
}

/// 渲染 XSynth 选项
fn render_xsynth_options<'a>(
    settings: &SettingsPanel,
    t: &lumino_extras::i18n::SettingsTranslations,
) -> iced_widget::Column<'a, Message, Theme, lumino_ui_core::Renderer> {
    let mut col = column![];

    // 音色库选择
    col = col.push(
        row![
            text(t.soundfont)
                .size(TEXT_SIZE_CONTENT)
                .style(create_content_text_style()),
            iced_widget::space().width(SPACING_MAIN),
            text_input(t.soundfont_placeholder, &settings.synth.soundfont_path)
                .width(Length::Fill)
                .on_input(|s| Message::Settings(crate::Event::SoundfontPathChanged(s))),
        ]
        .spacing(SPACING_ICON_LABEL)
        .align_y(Alignment::Center),
    );
    col = col.push(iced_widget::space().height(SPACING_CONTENT));
    col = col.push(
        iced_widget::button(t.browse).on_press(Message::Settings(crate::Event::BrowseSoundfont)),
    );
    col = col.push(iced_widget::space().height(20));

    // 缓冲区大小（Realtime 引擎使用毫秒粒度）
    col = col.push(
        row![
            text(format!(
                "{}: {:.1} ms",
                t.buffer_latency, settings.synth.xsynth_buffer_ms
            ))
            .size(TEXT_SIZE_CONTENT)
            .style(create_content_text_style())
            .width(160.0),
            iced_widget::slider(5.0..=100.0, settings.synth.xsynth_buffer_ms, |ms| {
                Message::Settings(crate::Event::XSynthBufferChanged(ms))
            })
            .step(1.0_f32)
            .width(200.0),
        ]
        .spacing(SPACING_ICON_LABEL)
        .align_y(Alignment::Center),
    );
    col = col.push(iced_widget::space().height(SPACING_CONTENT));

    // 音符释放淡出
    col = col.push(
        iced_widget::Checkbox::new(settings.synth.xsynth_fade_out)
            .label(t.fade_out_label)
            .on_toggle(|f| Message::Settings(crate::Event::XSynthFadeOutChanged(f))),
    );
    col = col.push(iced_widget::space().height(SPACING_CONTENT));

    // 每键最大同音数：0=不限制，1..64 拖拽 + 1..128 自定义输入
    let slider_val = match settings.synth.xsynth_max_voices_per_key {
        None => 0.0,
        Some(v) => (v as f32).clamp(0.0, 64.0),
    };
    let display_val = match settings.synth.xsynth_max_voices_per_key {
        None => "不限制".to_string(),
        Some(v) => v.to_string(),
    };
    col = col.push(
        row![
            text(format!("{}: {}", t.max_voices, display_val))
                .size(TEXT_SIZE_CONTENT)
                .style(create_content_text_style())
                .width(180.0),
            iced_widget::slider(0.0..=64.0, slider_val, |v| {
                let opt = if v < 0.5 { None } else { Some(v as usize) };
                Message::Settings(crate::Event::XSynthMaxVoicesChanged(opt))
            })
            .step(1.0_f32)
            .width(160.0),
            text_input("0=不限制 1-128", &display_val)
                .width(80.0)
                .on_input(|s| { Message::Settings(crate::Event::XSynthMaxVoicesCustomInput(s)) }),
        ]
        .spacing(SPACING_ICON_LABEL)
        .align_y(Alignment::Center),
    );
    col = col.push(iced_widget::space().height(SPACING_CONTENT));
    col = col.push(
        text(t.max_voices_hint)
            .size(12.0)
            .style(create_placeholder_text_style()),
    );
    col = col.push(iced_widget::space().height(20));

    // 力度过滤
    col = col.push(
        row![
            text(format!(
                "{}: {}",
                t.velocity_filter, settings.midi.velocity_filter_threshold
            ))
            .size(TEXT_SIZE_CONTENT)
            .style(create_content_text_style())
            .width(180.0),
            iced_widget::slider(0..=127, settings.midi.velocity_filter_threshold, |v| {
                Message::Settings(crate::Event::VelocityFilterThresholdChanged(v.to_string()))
            })
            .step(1)
            .width(200.0),
        ]
        .spacing(SPACING_ICON_LABEL)
        .align_y(Alignment::Center),
    );
    col = col.push(iced_widget::space().height(SPACING_CONTENT));
    col = col.push(
        text(t.velocity_filter_hint)
            .size(12.0)
            .style(create_placeholder_text_style()),
    );
    col = col.push(iced_widget::space().height(20));

    // 帮助文本
    col = col.push(
        text(t.xsynth_hint)
            .size(12.0)
            .style(create_placeholder_text_style()),
    );
    col = col.push(
        text(t.kdmapi_hint)
            .size(12.0)
            .style(create_placeholder_text_style()),
    );
    col = col.push(
        text(t.system_hint)
            .size(12.0)
            .style(create_placeholder_text_style()),
    );

    col
}

/// 渲染 LGS (GPU) 选项
///
/// 与 XSynth 共用 `soundfont_path`；GPU 专属参数（渲染采样率/块大小/插值）
/// 通过内置 MIDI 输出组的统一控件暴露：缓冲区大小、每键最大同音数、响度过滤。
fn render_lgs_options<'a>(
    settings: &SettingsPanel,
    t: &lumino_extras::i18n::SettingsTranslations,
) -> iced_widget::Column<'a, Message, Theme, lumino_ui_core::Renderer> {
    let mut col = column![];

    // 音色库选择（与 XSynth 共用 soundfont_path）
    col = col.push(
        row![
            text(t.soundfont)
                .size(TEXT_SIZE_CONTENT)
                .style(create_content_text_style()),
            iced_widget::space().width(SPACING_MAIN),
            text_input(t.soundfont_placeholder, &settings.synth.soundfont_path)
                .width(Length::Fill)
                .on_input(|s| Message::Settings(crate::Event::SoundfontPathChanged(s))),
        ]
        .spacing(SPACING_ICON_LABEL)
        .align_y(Alignment::Center),
    );
    col = col.push(iced_widget::space().height(SPACING_CONTENT));
    col = col.push(
        iced_widget::button(t.browse).on_press(Message::Settings(crate::Event::BrowseSoundfont)),
    );
    col = col.push(iced_widget::space().height(20));

    // 缓冲区大小（GPU 块大小，2 的幂）：滑块以 2 的指数表示（64=2^6 … 8192=2^13）
    let block_index = ((settings.synth.lgs_block_size as f64).log2().round() as usize).clamp(6, 13);
    col = col.push(
        row![
            text(format!(
                "{}: {}",
                t.lgs_buffer, settings.synth.lgs_block_size
            ))
            .size(TEXT_SIZE_CONTENT)
            .style(create_content_text_style())
            .width(200.0),
            iced_widget::slider(6.0..=13.0, block_index as f32, |i| {
                Message::Settings(crate::Event::LgsBlockSizeChanged(1usize << i as u32))
            })
            .step(1.0_f32)
            .width(200.0),
        ]
        .spacing(SPACING_ICON_LABEL)
        .align_y(Alignment::Center),
    );
    col = col.push(iced_widget::space().height(SPACING_CONTENT));
    col = col.push(
        text(t.lgs_buffer_hint)
            .size(12.0)
            .style(create_placeholder_text_style()),
    );
    col = col.push(iced_widget::space().height(20));

    // 每键最大同音数：0=不限制，1..128 拖拽 + 自定义输入
    let lgs_voices = settings.synth.lgs_max_voices_per_key;
    let display_voices = if lgs_voices == 0 {
        "不限制".to_string()
    } else {
        lgs_voices.to_string()
    };
    col = col.push(
        row![
            text(format!("{}: {}", t.max_voices, display_voices))
                .size(TEXT_SIZE_CONTENT)
                .style(create_content_text_style())
                .width(180.0),
            iced_widget::slider(0.0..=128.0, lgs_voices as f32, |v| {
                Message::Settings(crate::Event::LgsMaxVoicesChanged(v as usize))
            })
            .step(1.0_f32)
            .width(160.0),
        ]
        .spacing(SPACING_ICON_LABEL)
        .align_y(Alignment::Center),
    );
    col = col.push(iced_widget::space().height(SPACING_CONTENT));
    col = col.push(
        text(t.max_voices_hint)
            .size(12.0)
            .style(create_placeholder_text_style()),
    );
    col = col.push(iced_widget::space().height(20));

    // LGS (GPU) 专属响度过滤（与 XSynth 全局力度过滤相互独立；LGS 输出连接在 note_on 处实时丢弃过轻音符）
    col = col.push(
        row![
            text(format!(
                "{}: {}",
                t.velocity_filter, settings.synth.lgs_velocity_filter_threshold
            ))
            .size(TEXT_SIZE_CONTENT)
            .style(create_content_text_style())
            .width(180.0),
            iced_widget::slider(0..=127, settings.synth.lgs_velocity_filter_threshold, |v| {
                Message::Settings(crate::Event::LgsVelocityFilterChanged(v))
            },)
            .step(1)
            .width(200.0),
        ]
        .spacing(SPACING_ICON_LABEL)
        .align_y(Alignment::Center),
    );
    col = col.push(iced_widget::space().height(SPACING_CONTENT));
    col = col.push(
        text(t.velocity_filter_hint)
            .size(12.0)
            .style(create_placeholder_text_style()),
    );
    col = col.push(iced_widget::space().height(20));

    // LGS (GPU) 提示
    col = col.push(
        text(t.lgs_hint)
            .size(12.0)
            .style(create_placeholder_text_style()),
    );

    col
}

/// 渲染 WinMM (系统 MIDI) 输出设备（播表）选择器
///
/// 展示系统播表自动扫描结果，通过下拉菜单选择指定的 WINMM 播表；
/// 提供「刷新」按钮触发重新扫描。
fn render_winmm_output_selector<'a>(
    settings: &'a SettingsPanel,
    t: &lumino_extras::i18n::SettingsTranslations,
) -> Element<'a> {
    let label = text(t.winmm_output_device)
        .size(TEXT_SIZE_CONTENT)
        .style(create_content_text_style());

    let refresh_btn =
        iced_widget::button(t.refresh).on_press(Message::Settings(crate::Event::ScanWinmmOutputs));

    let body: Element<'a> = if settings.midi.winmm_outputs.is_empty() {
        row![
            label,
            iced_widget::space().width(SPACING_MAIN),
            text(t.winmm_no_device)
                .size(TEXT_SIZE_CONTENT)
                .style(create_placeholder_text_style()),
            iced_widget::space().width(SPACING_ICON_LABEL),
            refresh_btn,
        ]
        .spacing(SPACING_ICON_LABEL)
        .align_y(Alignment::Center)
        .into()
    } else {
        let options: Vec<&str> = settings
            .midi
            .winmm_outputs
            .iter()
            .map(|(_, name)| name.as_str())
            .collect();
        let selected = settings.midi.selected_winmm_output.and_then(|id| {
            settings
                .midi
                .winmm_outputs
                .iter()
                .find(|(oid, _)| *oid == id)
                .map(|(_, name)| name.as_str())
        });

        row![
            label,
            iced_widget::space().width(SPACING_MAIN),
            pick_list(options, selected, move |name| {
                if let Some((id, _)) = settings
                    .midi
                    .winmm_outputs
                    .iter()
                    .find(|(_, n)| n.as_str() == name)
                {
                    Message::Settings(crate::Event::WinmmOutputSelected(*id))
                } else {
                    Message::Null
                }
            })
            .placeholder(t.select_device_placeholder)
            .padding([4, 8])
            .width(200.0),
            iced_widget::space().width(SPACING_ICON_LABEL),
            refresh_btn,
        ]
        .spacing(SPACING_ICON_LABEL)
        .align_y(Alignment::Center)
        .into()
    };

    column![
        body,
        iced_widget::space().height(SPACING_CONTENT),
        text(t.system_hint)
            .size(12.0)
            .style(create_placeholder_text_style()),
    ]
    .spacing(SPACING_CONTENT)
    .into()
}

/// 渲染音频播放输出设备（CPAL 音频设备）选择器
///
/// 展示 CPAL 音频输出设备扫描结果，通过下拉菜单选择指定的播放输出设备；
/// 默认项为「系统默认输出设备」，提供「刷新」按钮触发重新扫描。
/// 该设置仅对软件合成器（XSynth / LGS）生效。
fn render_audio_output_selector<'a>(
    settings: &'a SettingsPanel,
    t: &lumino_extras::i18n::SettingsTranslations,
) -> Element<'a> {
    let default_label = t.audio_output_default;
    let refresh_btn =
        iced_widget::button(t.refresh).on_press(Message::Settings(crate::Event::ScanAudioOutputs));

    let body: Element<'a> = if settings.synth.audio_output_devices.is_empty() {
        row![
            text(t.audio_output_device)
                .size(TEXT_SIZE_CONTENT)
                .style(create_content_text_style()),
            iced_widget::space().width(SPACING_MAIN),
            text(t.audio_output_no_device)
                .size(TEXT_SIZE_CONTENT)
                .style(create_placeholder_text_style()),
            iced_widget::space().width(SPACING_ICON_LABEL),
            refresh_btn,
        ]
        .spacing(SPACING_ICON_LABEL)
        .align_y(Alignment::Center)
        .into()
    } else {
        // 选项：默认项 + 所有扫描到的音频输出设备名
        let mut options: Vec<String> = vec![default_label.to_string()];
        options.extend(settings.synth.audio_output_devices.iter().cloned());
        // 选中态：未选择（None）时显示默认项
        let selected: Option<String> = settings
            .synth
            .selected_audio_output_device
            .clone()
            .or_else(|| Some(default_label.to_string()));

        row![
            text(t.audio_output_device)
                .size(TEXT_SIZE_CONTENT)
                .style(create_content_text_style()),
            iced_widget::space().width(SPACING_MAIN),
            pick_list(options, selected, move |name| {
                // 选择默认项 → 清空（使用系统默认）；否则记录设备名
                if name == default_label {
                    Message::Settings(crate::Event::AudioOutputSelected(String::new()))
                } else {
                    Message::Settings(crate::Event::AudioOutputSelected(name))
                }
            })
            .placeholder(t.select_device_placeholder)
            .padding([4, 8])
            .width(200.0),
            iced_widget::space().width(SPACING_ICON_LABEL),
            refresh_btn,
        ]
        .spacing(SPACING_ICON_LABEL)
        .align_y(Alignment::Center)
        .into()
    };

    column![
        body,
        iced_widget::space().height(SPACING_CONTENT),
        text(t.audio_output_hint)
            .size(12.0)
            .style(create_placeholder_text_style()),
    ]
    .spacing(SPACING_CONTENT)
    .into()
}

/// 渲染 MIDI 输入设备选择器
fn render_midi_device_selector<'a>(
    settings: &'a SettingsPanel,
    t: &lumino_extras::i18n::SettingsTranslations,
) -> Element<'a> {
    let device_count = settings.midi.devices.len();
    if device_count == 0 {
        return row![
            text(t.midi_input_device)
                .size(TEXT_SIZE_CONTENT)
                .style(create_content_text_style()),
            iced_widget::space().width(SPACING_MAIN),
            text(t.no_device)
                .size(TEXT_SIZE_CONTENT)
                .style(create_placeholder_text_style()),
        ]
        .spacing(SPACING_ICON_LABEL)
        .align_y(Alignment::Center)
        .into();
    }

    let device_options: Vec<&str> = settings
        .midi
        .devices
        .iter()
        .map(|(_, name)| name.as_str())
        .collect();
    let selected_idx = settings
        .midi
        .selected_device
        .and_then(|id| settings.midi.devices.iter().position(|(did, _)| *did == id));
    let selected = selected_idx.map(|i| device_options[i]);

    row![
        text(t.midi_input_device)
            .size(TEXT_SIZE_CONTENT)
            .style(create_content_text_style()),
        iced_widget::space().width(SPACING_MAIN),
        pick_list(device_options, selected, move |name| {
            if let Some((id, _)) = settings
                .midi
                .devices
                .iter()
                .find(|(_, n)| n.as_str() == name)
            {
                Message::Settings(crate::Event::DeviceSelected(*id))
            } else {
                Message::Null
            }
        })
        .placeholder(t.select_device_placeholder)
        .padding([4, 8])
        .width(200.0),
    ]
    .spacing(SPACING_ICON_LABEL)
    .align_y(Alignment::Center)
    .into()
}
