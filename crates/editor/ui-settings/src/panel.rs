//! 设置面板状态模型与事件处理
//!
//! 内含 `SettingsPanel` 的构造与 `Event` 分发逻辑，从 `lib.rs` 拆分而来。

use crate::{
    AutoScrollSettings, CloudSettings, DisplaySettings, EditingSettings, Event, HiresSettings,
    LoggingSettings, MidiSettings, SettingsPanel, SynthSettings,
};
use lumino_core::storage::config::SynthBackend;
use lumino_ui_core::settings_event::OutputType;

/// 解析设置输入字符串并应用；解析失败记录 warn 并忽略（保持旧行为不变）。
///
/// 收敛 `update()` 中大量 `if let Ok(v) = s.parse::<T>()` 样板。
fn parse_setting<T>(raw: &str, apply: impl FnOnce(T))
where
    T: std::str::FromStr,
{
    match raw.parse::<T>() {
        Ok(v) => apply(v),
        Err(_) => tracing::warn!("设置输入非法，已忽略: '{}'", raw),
    }
}

impl SettingsPanel {
    /// 根据给定的 UI 配置创建设置面板状态。
    ///
    /// 从 `ui_config` 中初始化各子设置，解析并填充可用/选中的调色板名称。
    ///
    /// # 参数
    /// * `ui_config` — 应用 UI 配置来源
    ///
    /// # 返回值
    /// 返回按配置类别初始化完成的 `SettingsPanel` 实例
    pub fn new(ui_config: &lumino_core::storage::config::UiConfig) -> Self {
        let palette_mgr = &*lumino_extras::palette::PALETTE_MANAGER;
        let available_palettes = palette_mgr.names().to_vec();
        let selected_palette = if ui_config.selected_palette.is_empty() {
            palette_mgr.default().name.to_string()
        } else {
            palette_mgr
                .resolve_name(&ui_config.selected_palette)
                .to_string()
        };

        Self {
            selected_menu_index: 0,
            synth: SynthSettings {
                backend: ui_config.preferred_backend,
                audio_engine: ui_config.audio_engine,
                soundfont_path: ui_config.soundfont_path.clone(),
                use_native_titlebar: ui_config.use_native_titlebar,
                xsynth_buffer_ms: ui_config.xsynth_buffer_ms,
                xsynth_sample_rate: ui_config.xsynth_sample_rate,
                xsynth_threads: ui_config.xsynth_threads,
                xsynth_fade_out: ui_config.xsynth_fade_out_killing,
                xsynth_max_voices_per_key: ui_config.xsynth_max_voices_per_key,
                lgs_block_size: ui_config.lgs_block_size,
                lgs_max_voices_per_key: ui_config.lgs_max_voices_per_key,
                lgs_velocity_filter_threshold: ui_config.lgs_velocity_filter_threshold,
                audio_output_devices: Vec::new(),
                selected_audio_output_device: ui_config.audio_output_device.clone(),
            },
            editing: EditingSettings {
                eraser_behavior: ui_config.eraser_behavior,
                selection_box_mode: ui_config.selection_box_mode,
                program_font_name: ui_config.program_font_name.clone(),
                program_font_path: ui_config.program_font_path.clone(),
                history_total_limit: ui_config.history_total_limit,
                history_entry_limit: ui_config.history_entry_limit,
                merge_window_ms: ui_config.merge_window_ms,
                intercept_notification_enabled: ui_config.intercept_notification_enabled,
                automation_line_thickness: ui_config.automation_line_thickness,
                tempo_max_bpm: ui_config.tempo_max_bpm,
                tempo_custom_open: false,
                tempo_custom_input: String::new(),
                track_add_behavior: ui_config.track_add_behavior,
            },
            display: DisplaySettings {
                icon_hidpi: ui_config.icon_hidpi,
                enable_256key: ui_config.enable_256key,
                velocity_curve_style: ui_config.velocity_curve_style,
                language: ui_config.language,
                playback_key_colors_enabled: ui_config.playback_key_colors_enabled,
                selected_palette,
                available_palettes,
            },
            auto_scroll: AutoScrollSettings {
                fixed_position: ui_config.auto_scroll.fixed_indicator_position,
                page_trigger_offset: ui_config.auto_scroll.page_trigger_offset,
                page_return_position: ui_config.auto_scroll.page_return_position,
            },
            midi: MidiSettings {
                devices: Vec::new(),
                selected_device: None,
                velocity_filter_threshold: ui_config.velocity_filter_threshold,
                winmm_outputs: Vec::new(),
                selected_winmm_output: ui_config.system_output_device_id,
            },
            hires: HiresSettings {
                onion_enabled: ui_config.hires_onion_enabled,
                measures_per_group: ui_config.hires_measures_per_group,
                tile_width_px: ui_config.hires_tile_width_px,
                cooldown_secs: ui_config.hires_cooldown_secs,
                gpu_mem_limit_mb: ui_config.hires_gpu_mem_limit_mb,
            },
            logging: LoggingSettings {
                log_retention_count: ui_config.log_retention_count,
                monitor_refresh_interval_ms: ui_config.monitor_refresh_interval_ms,
            },
            cloud: CloudSettings {
                connections: Vec::new(),
                alert: None,
            },
        }
    }

    /// 处理来自 UI 的设置变更事件并更新面板状态。
    ///
    /// 根据事件类型分发到对应的设置子结构，数值类变化通过字符串解析
    /// 并自动忽略非法输入。
    ///
    /// # 参数
    /// * `event` — 需要处理的设置变更事件
    pub fn update(&mut self, event: Event) {
        match event {
            Event::MenuSelected(idx) => {
                self.selected_menu_index = idx;
            }
            Event::SynthBackendChanged(backend) => {
                self.synth.backend = backend;
            }
            Event::OutputTypeChanged(ot) => {
                // 顶层输出类型：内置合成器归为一类，进入后由 SynthBackendChanged 选择具体引擎；
                // 从外部类型切回内置时，默认落到 XSynth（用户可在子下拉里改选 LGS）。
                self.synth.backend = match ot {
                    OutputType::Builtin => {
                        if matches!(
                            self.synth.backend,
                            SynthBackend::Kdmapi | SynthBackend::System
                        ) {
                            SynthBackend::XSynth
                        } else {
                            self.synth.backend
                        }
                    }
                    OutputType::Kdmapi => SynthBackend::Kdmapi,
                    OutputType::System => SynthBackend::System,
                };
            }
            Event::AudioEngineChanged(kind) => {
                self.synth.audio_engine = kind;
            }
            Event::SoundfontPathChanged(path) => {
                self.synth.soundfont_path = path;
            }
            Event::BrowseSoundfont => {
                if let Some(path) = rfd::FileDialog::new()
                    .add_filter("音色库文件", &["sf2", "sfz"])
                    .add_filter("SF2 文件", &["sf2"])
                    .add_filter("SFZ 文件", &["sfz"])
                    .add_filter("所有文件", &["*"])
                    .pick_file()
                {
                    self.synth.soundfont_path = path.to_string_lossy().into_owned();
                }
            }
            Event::NativeTitlebarChanged(enabled) => {
                self.synth.use_native_titlebar = enabled;
            }
            Event::XSynthBufferChanged(ms) => {
                self.synth.xsynth_buffer_ms = ms;
            }
            Event::XSynthSampleRateChanged(sr) => {
                self.synth.xsynth_sample_rate = sr;
            }
            Event::XSynthFadeOutChanged(f) => {
                self.synth.xsynth_fade_out = f;
            }
            Event::XSynthMaxVoicesChanged(v) => {
                self.synth.xsynth_max_voices_per_key = v;
            }
            Event::XSynthMaxVoicesCustomInput(s) => {
                let t = s.trim();
                if t.is_empty() || t.eq_ignore_ascii_case("unlimited") || t == "0" {
                    self.synth.xsynth_max_voices_per_key = None;
                } else if let Ok(v) = t.parse::<usize>() {
                    if v == 0 {
                        self.synth.xsynth_max_voices_per_key = None;
                    } else {
                        self.synth.xsynth_max_voices_per_key = Some(v.clamp(1, 128));
                    }
                }
            }
            Event::LgsBlockSizeChanged(size) => {
                // 仅接受 2 的幂（GUI 滑块已约束，这里兜底夹紧到 [64, 8192] 内的 2 的幂）
                let clamped = size.clamp(64, 8192);
                self.synth.lgs_block_size = clamped.next_power_of_two();
            }
            Event::LgsMaxVoicesChanged(v) => {
                self.synth.lgs_max_voices_per_key = v.clamp(0, 128);
            }
            Event::LgsVelocityFilterChanged(v) => {
                self.synth.lgs_velocity_filter_threshold = v.clamp(0, 127);
            }
            Event::ThemeChanged(_) => {
                // 主题变更由外部处理
            }
            Event::EraserBehaviorChanged(behavior) => {
                self.editing.eraser_behavior = behavior;
            }
            Event::SelectionBoxModeChanged(mode) => {
                self.editing.selection_box_mode = mode;
            }
            Event::ProgramFontNameChanged(name) => {
                self.editing.program_font_name = name;
            }
            Event::ProgramFontPathChanged(path) => {
                self.editing.program_font_path = path;
            }
            Event::BrowseProgramFont => {
                if let Some(path) = rfd::FileDialog::new()
                    .add_filter("字体文件", &["ttf", "otf", "ttc", "woff", "woff2"])
                    .add_filter("TrueType 字体", &["ttf"])
                    .add_filter("OpenType 字体", &["otf"])
                    .add_filter("所有文件", &["*"])
                    .pick_file()
                {
                    self.editing.program_font_path = path.to_string_lossy().into_owned();
                }
            }
            // 自动滚动配置事件
            Event::AutoScrollFixedPositionChanged(value) => {
                parse_setting(&value, |val| self.auto_scroll.fixed_position = val);
            }
            Event::AutoScrollPageTriggerOffsetChanged(value) => {
                parse_setting(&value, |val| self.auto_scroll.page_trigger_offset = val);
            }
            Event::AutoScrollPageReturnPositionChanged(value) => {
                parse_setting(&value, |val| self.auto_scroll.page_return_position = val);
            }
            // 力度过滤
            Event::VelocityFilterThresholdChanged(value) => {
                parse_setting(&value, |val| self.midi.velocity_filter_threshold = val);
            }
            Event::IconHiDPIChanged(enabled) => {
                self.display.icon_hidpi = enabled;
            }
            Event::Enable256keyChanged(enabled) => {
                self.display.enable_256key = enabled;
            }
            Event::VelocityCurveStyleChanged(enabled) => {
                self.display.velocity_curve_style = enabled;
            }
            Event::DeviceSelected(id) => {
                self.midi.selected_device = Some(id);
                tracing::debug!("设置: MIDI 输入设备选择为 #{}", id);
            }
            Event::WinmmOutputSelected(id) => {
                self.midi.selected_winmm_output = Some(id);
                tracing::debug!("设置: WinMM 输出设备(播表)选择为 #{}", id);
            }
            Event::ScanWinmmOutputs => {
                // 实际扫描由 editor/ui 的 Root::scan_winmm_outputs 执行并写回设置面板
                tracing::debug!("设置: 收到 WinMM 播表扫描请求");
            }
            Event::AudioOutputSelected(name) => {
                // 空串/默认项表示使用系统默认输出设备
                self.synth.selected_audio_output_device =
                    if name.is_empty() { None } else { Some(name) };
                tracing::debug!(
                    "设置: 音频播放输出设备选择为 {:?}",
                    self.synth.selected_audio_output_device
                );
            }
            Event::ScanAudioOutputs => {
                // 实际扫描由 editor/ui 的 Root::scan_audio_outputs 执行并写回设置面板
                tracing::debug!("设置: 收到音频播放输出设备扫描请求");
            }
            Event::LanguageChanged(lang) => {
                self.display.language = lang;
                tracing::debug!("设置: 界面语言切换为 {:?}", lang);
            }
            // 高精度洋葱皮贴图设置
            Event::HiresOnionEnabledChanged(v) => {
                self.hires.onion_enabled = v;
            }
            Event::HiresMeasuresPerGroupChanged(s) => {
                parse_setting(&s, |v: u32| self.hires.measures_per_group = v.clamp(1, 16));
            }
            Event::HiresTileWidthChanged(s) => {
                parse_setting(&s, |v: u32| self.hires.tile_width_px = v.clamp(480, 7680));
            }
            Event::HiresCooldownChanged(s) => {
                parse_setting(&s, |v: u64| self.hires.cooldown_secs = v.clamp(3, 60));
            }
            Event::HiresGpuMemLimitChanged(s) => {
                // 用户硬约束：不得限制 GPU 内存使用——移除 clamp(128, 4096)
                parse_setting(&s, |v: u32| self.hires.gpu_mem_limit_mb = v);
            }
            Event::PlaybackKeyColorsEnabledChanged(v) => {
                self.display.playback_key_colors_enabled = v;
            }
            Event::TrackAddBehaviorChanged(v) => {
                self.editing.track_add_behavior = v;
            }
            Event::PaletteChanged(name) => {
                if let Some(p) = self.display.available_palettes.iter().find(|n| **n == name) {
                    self.display.selected_palette = p.to_string();
                    tracing::debug!("设置: 调色板切换为 '{}'", name);
                }
            }
            // 编辑设置
            Event::HistoryTotalLimitChanged(s) => {
                parse_setting(&s, |v: usize| {
                    self.editing.history_total_limit = v.clamp(10, 1000)
                });
            }
            Event::HistoryEntryLimitChanged(s) => {
                parse_setting(&s, |v: usize| {
                    self.editing.history_entry_limit = v.clamp(100, 10000)
                });
            }
            Event::MergeWindowMsChanged(s) => {
                parse_setting(&s, |v: u64| self.editing.merge_window_ms = v.min(5000));
            }
            Event::InterceptNotificationChanged(enabled) => {
                self.editing.intercept_notification_enabled = enabled;
            }
            Event::AutomationLineThicknessChanged(v) => {
                self.editing.automation_line_thickness = v.clamp(1.0, 10.0);
            }
            Event::TempoMaxBpmChanged(v) => {
                self.editing.tempo_max_bpm = v;
                self.editing.tempo_custom_open = false;
            }
            Event::TempoMaxBpmCustomOpen => {
                // 打开面板时预填当前值，方便微调
                if self.editing.tempo_custom_input.is_empty() {
                    self.editing.tempo_custom_input = format!("{:.0}", self.editing.tempo_max_bpm);
                }
                self.editing.tempo_custom_open = true;
            }
            Event::TempoMaxBpmCustomClose => {
                self.editing.tempo_custom_open = false;
            }
            Event::TempoMaxBpmCustomInput(value) => {
                self.editing.tempo_custom_input = value;
            }
            Event::TempoMaxBpmCustomConfirm => {
                if let Ok(v) = self.editing.tempo_custom_input.trim().parse::<f64>() {
                    self.editing.tempo_max_bpm = v;
                    self.editing.tempo_custom_open = false;
                    tracing::info!("设置: 自定义 Tempo BPM 上限为 {:.0}", v);
                } else {
                    tracing::warn!("设置: 自定义 Tempo BPM 上限输入无效");
                }
            }
            Event::LogRetentionCountChanged(s) => {
                parse_setting(&s, |v: usize| self.logging.log_retention_count = v);
            }
            Event::MonitorRefreshIntervalChanged(v) => {
                self.logging.monitor_refresh_interval_ms = v.clamp(50.0, 2000.0);
            }
        }
    }
}
