//! MIDI 事件处理器 — 参考 OmniConverter 的 EventsProcesser
//!
//! 将 MIDI 事件流转换为 xsynth 音频样本。
//! 基于渲染时间驱动，支持进度回调。

use std::sync::Arc;

use midly::{MidiMessage, TrackEventKind};
use tracing::info;
use xsynth_core::{
    AudioPipe,
    channel::{ChannelAudioEvent, ChannelConfigEvent, ChannelEvent, ControlEvent},
    channel_group::{ChannelGroup, SynthEvent},
    soundfont::{SampleSoundfont, SoundfontBase},
};

use crate::error::{ExportError, ExportResult};

use super::{
    config::AudioRenderConfig, limiter::AudioLimiter, stream::SampleSink, tick_conv::TickToTime,
};

/// 事件处理器 — 将 MIDI 事件流式渲染到 SampleSink
///
/// 参考 OmniConverter 的 EventsProcesser 设计：
/// - 以渲染时间（delta seconds）驱动，而非依赖外部定时器
/// - 使用 Vec 回收池减少分配
pub struct MidiEventProcessor<'a> {
    config: &'a AudioRenderConfig,
    channel_group: &'a mut ChannelGroup,
    tick_conv: &'a mut TickToTime,
    sink: &'a mut dyn SampleSink,
    sample_rate: u32,
    channel_count: u16,
    /// Vec 回收池
    vec_pool: Vec<Vec<f32>>,
    /// 限幅器（启用时）
    limiter: Option<AudioLimiter>,
}

/// 进度回调
pub type ProgressFn = Arc<dyn Fn(String, f64) + Send + Sync>;

impl<'a> MidiEventProcessor<'a> {
    /// 创建 MIDI 事件处理器。
    ///
    /// 将 MIDI 事件按时间轴播放到给定的合成通道组与采样输出。
    pub fn new(
        config: &'a AudioRenderConfig,
        channel_group: &'a mut ChannelGroup,
        tick_conv: &'a mut TickToTime,
        sink: &'a mut dyn SampleSink,
    ) -> Self {
        let params = *channel_group.stream_params();
        let limiter = if config.apply_limiter {
            Some(AudioLimiter::new(
                params.sample_rate,
                params.channels.count(),
                0.95,
            ))
        } else {
            None
        };
        MidiEventProcessor {
            config,
            channel_group,
            tick_conv,
            sink,
            sample_rate: params.sample_rate,
            channel_count: params.channels.count(),
            vec_pool: Vec::new(),
            limiter,
        }
    }

    /// 从 Vec 池获取或创建新 Vec
    fn acquire_buffer(&mut self, capacity: usize) -> Vec<f32> {
        self.vec_pool
            .pop()
            .unwrap_or_else(|| Vec::with_capacity(capacity))
    }

    /// 归还 Vec 到池
    fn release_buffer(&mut self, buf: Vec<f32>) {
        if self.vec_pool.len() < 4 {
            self.vec_pool.push(buf);
        }
    }

    /// 渲染指定时长的音频（带暂停/中止检查）
    fn render_duration(&mut self, delta_seconds: f64) -> ExportResult<()> {
        if delta_seconds <= 0.0 {
            return Ok(());
        }
        // 暂停/中止检查点
        if let Some(ctrl) = &self.config.control {
            ctrl.wait_if_paused();
            ctrl.check_abort()?;
        }

        let total_samples = (delta_seconds * self.sample_rate as f64) as usize;
        if total_samples == 0 {
            return Ok(());
        }

        let frame_size = self.channel_count as usize;
        let mut remaining = total_samples;

        const MAX_BATCH: usize = 4096;

        while remaining > 0 {
            if let Some(ctrl) = &self.config.control {
                ctrl.wait_if_paused();
                ctrl.check_abort()?;
            }
            let batch = remaining.min(MAX_BATCH);
            let count = batch * frame_size;

            let mut buffer = self.acquire_buffer(count);
            buffer.resize(count, 0.0);

            // SAFETY: read_samples_unchecked 会填充所有样本
            self.channel_group.read_samples_unchecked(&mut buffer);

            // 应用限制器（如果配置）
            if let Some(limiter) = self.limiter.as_mut() {
                limiter.process(&mut buffer);
            }

            self.sink.write_samples(&buffer)?;
            self.release_buffer(buffer);

            remaining -= batch;
        }

        Ok(())
    }

    /// 判断音符是否应被过滤（力度/键位）
    #[inline]
    fn is_note_filtered(&self, key: u8, velocity: u8) -> bool {
        if self.config.filter_key && (key < self.config.key_low || key > self.config.key_high) {
            return true;
        }
        if self.config.filter_velocity
            && (velocity < self.config.velocity_low || velocity > self.config.velocity_high)
        {
            return true;
        }
        false
    }

    /// 处理一个 MIDI 事件，渲染到该事件的时间点
    pub fn process_midi_event(
        &mut self,
        tick: u64,
        event_kind: &TrackEventKind,
    ) -> ExportResult<()> {
        if let Some(ctrl) = &self.config.control {
            ctrl.wait_if_paused();
            ctrl.check_abort()?;
        }
        // 计算到该事件的时间增量
        let delta = self.tick_conv.advance_to(tick);
        self.render_duration(delta)?;

        // 发送 MIDI 事件到合成器
        if let TrackEventKind::Midi { channel, message } = event_kind {
            let ch = channel.as_int() as u32;
            match message {
                MidiMessage::NoteOn { key, vel } => {
                    let vel_u8 = vel.as_int();
                    // velocity 0 的 NoteOn 按 MIDI 规范视为 NoteOff
                    if vel_u8 == 0 {
                        if self.config.filter_key
                            && (*key < self.config.key_low || *key > self.config.key_high)
                        {
                            return Ok(());
                        }
                        if self.config.note_force_end_delay > 0 {
                            self.render_duration(self.config.note_force_end_delay as f64 / 1000.0)?;
                        }
                        self.channel_group.send_event(SynthEvent::Channel(
                            ch,
                            ChannelEvent::Audio(ChannelAudioEvent::NoteOff { key: *key }),
                        ));
                        return Ok(());
                    }
                    if self.is_note_filtered(*key, vel_u8) {
                        return Ok(());
                    }
                    self.channel_group.send_event(SynthEvent::Channel(
                        ch,
                        ChannelEvent::Audio(ChannelAudioEvent::NoteOn {
                            key: *key,
                            vel: vel_u8,
                        }),
                    ));
                }
                MidiMessage::NoteOff { key, .. } => {
                    if self.config.filter_key
                        && (*key < self.config.key_low || *key > self.config.key_high)
                    {
                        return Ok(());
                    }
                    // note_force_end_delay：延长音符，延迟发送 NoteOff
                    if self.config.note_force_end_delay > 0 {
                        self.render_duration(self.config.note_force_end_delay as f64 / 1000.0)?;
                    }
                    self.channel_group.send_event(SynthEvent::Channel(
                        ch,
                        ChannelEvent::Audio(ChannelAudioEvent::NoteOff { key: *key }),
                    ));
                }
                MidiMessage::Controller { controller, value } => {
                    self.channel_group.send_event(SynthEvent::Channel(
                        ch,
                        ChannelEvent::Audio(ChannelAudioEvent::Control(ControlEvent::Raw(
                            controller.as_int(),
                            value.as_int(),
                        ))),
                    ));
                }
                MidiMessage::ProgramChange { program } => {
                    if self.config.ignore_program_changes {
                        return Ok(());
                    }
                    self.channel_group.send_event(SynthEvent::Channel(
                        ch,
                        ChannelEvent::Audio(ChannelAudioEvent::ProgramChange(program.as_int())),
                    ));
                }
                MidiMessage::PitchBend { bend } => {
                    self.channel_group.send_event(SynthEvent::Channel(
                        ch,
                        ChannelEvent::Audio(ChannelAudioEvent::Control(
                            ControlEvent::PitchBendValue(bend.as_int() as f32 / 8192.0 - 1.0),
                        )),
                    ));
                }
                _ => {}
            }
        }

        Ok(())
    }

    /// 完成渲染：发送 NoteOff，渲染尾部直到静音
    pub fn finalize(&mut self) -> ExportResult<()> {
        if let Some(ctrl) = &self.config.control {
            ctrl.wait_if_paused();
            ctrl.check_abort()?;
        }
        // 发送所有音符关闭
        self.channel_group
            .send_event(SynthEvent::AllChannels(ChannelEvent::Audio(
                ChannelAudioEvent::AllNotesOff,
            )));
        self.channel_group
            .send_event(SynthEvent::AllChannels(ChannelEvent::Audio(
                ChannelAudioEvent::ResetControl,
            )));

        // 持续渲染尾部直到静音（带 120s 安全上限，防止无限循环）
        let frame_size = self.channel_count as usize;
        let batch_size = self.sample_rate as usize * frame_size; // 1秒
        let max_batches = 120; // 120 秒上限，对齐 GPU 侧 max_tail_seconds

        for _ in 0..max_batches {
            if let Some(ctrl) = &self.config.control {
                ctrl.wait_if_paused();
                ctrl.check_abort()?;
            }
            let mut buffer = vec![0.0f32; batch_size];
            self.channel_group.read_samples_unchecked(&mut buffer);

            if let Some(limiter) = self.limiter.as_mut() {
                limiter.process(&mut buffer);
            }

            // 检测是否静音
            let is_silent = buffer.iter().all(|&s| s.abs() < 0.0001);

            self.sink.write_samples(&buffer)?;

            if is_silent {
                break;
            }
        }

        info!("音频渲染完成");
        Ok(())
    }
}

/// 简单的限幅器（兜底，已被 AudioLimiter 替代，保留用于独立调用）
#[allow(dead_code)]
fn apply_limiter(samples: &mut [f32], _channels: u16) {
    // 简单的峰值限制
    let threshold = 0.95;
    for sample in samples.iter_mut() {
        if sample.abs() > threshold {
            *sample = sample.signum() * threshold;
        }
    }
}

fn validate_sf2_path(path: &std::path::Path) -> ExportResult<()> {
    if !path.exists() {
        return Err(ExportError::AudioWrite(format!(
            "音色库文件不存在: {:?}",
            path
        )));
    }
    if let Some(ext) = path.extension().and_then(|s| s.to_str()) {
        if ext.eq_ignore_ascii_case("sfz") {
            // CPU 的 SampleSoundfont 支持 SFZ（按扩展名分发到 new_sfz），允许通过；
            // 仅校验文件可读，不做 RIFF 检查
            return Ok(());
        }
        if !ext.eq_ignore_ascii_case("sf2") {
            return Err(ExportError::AudioWrite(format!(
                "不支持的音色库格式 {:?}：仅支持 .sf2/.sfz",
                path
            )));
        }
    }
    // 仅对 SF2 快速校验 RIFF 头，避免底层 soundfont crate 直接 panic（abort）
    if let Some(ext) = path.extension().and_then(|s| s.to_str())
        && ext.eq_ignore_ascii_case("sf2")
        && let Ok(mut f) = std::fs::File::open(path)
    {
        use std::io::Read;
        let mut header = [0u8; 4];
        if f.read_exact(&mut header).is_ok() && header != *b"RIFF" {
            return Err(ExportError::AudioWrite(format!(
                "音色库不是合法的 SF2 文件 {:?}：头应为 RIFF，实际 {:02X?}",
                path, header
            )));
        }
    }
    Ok(())
}

/// 加载 SF2 音色库到 ChannelGroup
pub fn load_soundfonts(
    channel_group: &mut ChannelGroup,
    config: &AudioRenderConfig,
) -> ExportResult<()> {
    if config.soundfonts.is_empty() {
        return Err(ExportError::AudioWrite(
            "未指定音色库文件，请在导出面板选择 .sf2".into(),
        ));
    }
    for p in &config.soundfonts {
        validate_sf2_path(p)?;
    }

    let stream_params = *channel_group.stream_params();
    let sf_options = config.build_sf_options();

    let soundfonts: Vec<Arc<dyn SoundfontBase>> = config
        .soundfonts
        .iter()
        .map(|sf_path| {
            // 使用音色库标签追踪每个音色库加载时的内存分配
            lumino_diagnostics::memtrace::with_tag(
                lumino_diagnostics::memtrace::AllocTag::SoundFont,
                || {
                    let sf: Arc<dyn SoundfontBase> = Arc::new(
                        SampleSoundfont::new(sf_path, stream_params, sf_options).map_err(|e| {
                            ExportError::AudioWrite(format!("音色库 {sf_path:?}: {e}"))
                        })?,
                    );
                    Ok(sf)
                },
            )
        })
        .collect::<ExportResult<Vec<_>>>()?;

    channel_group.send_event(SynthEvent::AllChannels(ChannelEvent::Config(
        ChannelConfigEvent::SetSoundfonts(soundfonts),
    )));
    channel_group.send_event(SynthEvent::AllChannels(ChannelEvent::Config(
        ChannelConfigEvent::SetLayerCount(config.layer_limit),
    )));

    Ok(())
}
