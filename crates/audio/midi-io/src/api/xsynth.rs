use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicU32;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use cpal::traits::DeviceTrait;
use xsynth_core::{
    AudioStreamParams, ChannelCount,
    channel::{ChannelAudioEvent, ChannelConfigEvent, ChannelEvent},
    soundfont::SoundfontBase,
};

use crate::realtime::{
    ChannelMixHandle, RealtimeEventSender, RealtimeSynth, StreamRestartError, SynthEvent,
    ThreadCount as LuminoThreadCount, XSynthRealtimeConfig,
};

use super::xsynth_output::XSynthOutputConn;
use crate::soundfont_cache;
use crate::{
    Api, Error, InputConnection, InputInfo, MidiInputCallback, OutputConnection, OutputInfo,
};

/// XSynth 运行时统计信息
#[derive(Debug, Clone, Copy, Default)]
pub struct XSynthStats {
    /// 当前活跃 voice 数量
    pub voice_count: u64,
    /// 渲染器平均负载 (0.0 - 1.0)
    pub average_renderer_load: f64,
    /// 缓冲区样本数
    pub buffer_samples: i64,
}

/// XSynth 后端打开选项
#[derive(Debug, Clone)]
pub struct XSynthOptions {
    /// 缓冲区时长（毫秒）
    pub buffer_ms: f64,
    /// 渲染线程数
    pub threads: i32,
    /// 采样率
    pub sample_rate: u32,
    /// 是否淡出被杀掉的 voice
    pub fade_out_killing: bool,
    /// 音频播放输出设备（CPAL 音频设备名；None = 使用系统默认输出设备）
    pub audio_output_device: Option<String>,
}

/// XSynth 软件合成后端，基于 realtime 合成管线提供实时 MIDI 播放
pub struct XSynth {
    synth: RealtimeSynth,
    /// 共享事件发送器（全量重建时替换，所有已创建的输出连接自动跟随）
    sender_shared: Arc<Mutex<RealtimeEventSender>>,
    /// 混音参数共享句柄（重建稳定：外层 `Arc` 指针不变，重建时替换内层 `Vec`）。
    /// 所有已创建的 `XSynthOutputConn` 通过它设置每通道增益/声像，
    /// 与 `sender_shared` 同生命周期语义。
    mixer_shared: ChannelMixHandle,
    /// 主输出实时响度峰值共享句柄（重建稳定：`clone_master_peak` 的 `Arc` 不变，
    /// 重建时跟随新管线），供输出连接读取主输出电平。
    master_peak_shared: Arc<AtomicU32>,
    /// 音色库路径（重建管线时重用）
    soundfont_path: PathBuf,
    /// 打开选项（重建管线时重用）
    options: Option<XSynthOptions>,
    version: String,
}

impl XSynth {
    /// 使用指定音色库路径创建 XSynth 后端
    pub fn new(soundfont_path: &Path, options: Option<XSynthOptions>) -> Result<Self, Error> {
        tracing::info!("XSynth: 初始化，音色库路径: {:?}", soundfont_path);

        // 检查音色库文件是否存在
        if !soundfont_path.exists() {
            return Err(Error::InitFailed(format!(
                "Soundfont file not found: {:?}",
                soundfont_path
            )));
        }

        let (synth, sender) = Self::init_synth(soundfont_path, options.as_ref())?;
        let sender_shared = Arc::new(Mutex::new(sender));
        let mixer_shared = ChannelMixHandle::new(Mutex::new(synth.clone_channel_mix()));
        let master_peak_shared = synth.clone_master_peak();

        let version = "xsynth-realtime 0.4.0 (lumino-realtime)".to_string();
        tracing::info!("XSynth: 初始化完成");

        Ok(Self {
            synth,
            sender_shared,
            mixer_shared,
            master_peak_shared,
            soundfont_path: soundfont_path.to_path_buf(),
            options,
            version,
        })
    }

    /// 初始化合成管线：预加载音色库 → 打开音频流 → 配置音色库事件。
    ///
    /// 被 `new` 与 `rebuild`（设备参数变化后全量重建）复用。
    fn init_synth(
        soundfont_path: &Path,
        options: Option<&XSynthOptions>,
    ) -> Result<(RealtimeSynth, RealtimeEventSender), Error> {
        // 采样率对齐原则：配置中的 `xsynth_sample_rate` 仅作提示，
        // 真正决定音高的是设备实际采样率。`RealtimeSynth`（xsynth-realtime）
        // 总是以 cpal 设备原生采样率渲染，因此音色库必须以同一采样率预处理，
        // 否则 SampleSoundfont 内部重采样/包络时间错配会产生音高偏移（跑调）。
        //
        // 加载策略（性能修复）：先打开音频流取得设备原生采样率，再「只」按实际采样率
        // 加载一次。
        //  - 缓存键为 (path, sample_rate)，若先按请求采样率(默认 44100)占位预加载、
        //    再按实际采样率(多为 48000)加载，采样率不一致时会触发一次完整的
        //    SampleSoundfont 全量构建冗余，并在全局缓存留下一份永不被使用的数十~数百 MB 条目；
        //  - 占位预加载的 soundfont 从未经 SetSoundfonts 下发合成器，对「流启动防 underrun」
        //    零作用（流启动后本就静音，直到下方 SetSoundfonts 到达，与 Core 后端空 Group 一致）。
        // 故仅加载实际采样率一份，零冗余。
        let mut rt_config = XSynthRealtimeConfig::default();

        if let Some(opt) = options {
            rt_config.render_window_ms = opt.buffer_ms;

            // 解析线程数
            let thread_count = match opt.threads {
                -1 => LuminoThreadCount::None,
                0 => LuminoThreadCount::Auto,
                n if n > 0 => LuminoThreadCount::Manual(n as usize),
                _ => LuminoThreadCount::Auto,
            };
            rt_config.multithreading = thread_count;
            rt_config.channel_init_options.fade_out_killing = opt.fade_out_killing;
        }

        // 解析音频播放输出设备：指定设备有效则直接对其打开流，
        // 否则回退到系统默认输出设备（设备已移除 / 改名时优雅降级）。
        let target_device = crate::audio_devices::resolve_audio_output_device(
            options.and_then(|o| o.audio_output_device.as_deref()),
        );
        let synth = match target_device {
            Some(device) => {
                let stream_config = device
                    .default_output_config()
                    .map_err(|e| Error::InitFailed(format!("音频设备默认配置失败: {}", e)))?;
                RealtimeSynth::open(rt_config, &device, stream_config)
                    .map_err(|e| Error::InitFailed(format!("xsynth-realtime: {}", e)))?
            }
            None => RealtimeSynth::open_with_default_output(rt_config)
                .map_err(|e| Error::InitFailed(format!("xsynth-realtime: {}", e)))?,
        };

        // 设备实际采样率（cpal 决定，可能与请求的不同）
        let actual_sample_rate = synth.stream_params().sample_rate;

        // 仅按设备实际采样率加载一次正式音色库（无占位预加载）。SampleSoundfont::new()
        // 的预处理（采样率转换、包络时间等）与 AudioStreamParams.sample_rate 强相关，
        // 混用会导致音高/速度错误（跑调），故无条件以 actual_sample_rate 加载。
        let actual_params = AudioStreamParams::new(actual_sample_rate, ChannelCount::Stereo);
        let load_start = Instant::now();
        let soundfont = soundfont_cache::load_soundfont_cached(soundfont_path, actual_params)
            .map_err(Error::InitFailed)?;
        tracing::info!(
            "XSynth: 音色库已按设备实际采样率 {}Hz 加载，耗时: {:.2} 秒",
            actual_sample_rate,
            load_start.elapsed().as_secs_f64()
        );

        // 获取 sender — 在 open 后立即配置通道，确保音色库在 callback 首次触发前就位
        let mut sender = synth.get_sender_ref().clone();

        // 配置音色库
        let soundfonts: Vec<Arc<dyn SoundfontBase>> = vec![soundfont];
        sender.send_event(SynthEvent::AllChannels(ChannelEvent::Config(
            ChannelConfigEvent::SetSoundfonts(soundfonts),
        )));

        // 重置所有通道，确保音色库生效
        sender.send_event(SynthEvent::AllChannels(ChannelEvent::Audio(
            ChannelAudioEvent::AllNotesKilled,
        )));

        sender.send_event(SynthEvent::AllChannels(ChannelEvent::Audio(
            ChannelAudioEvent::ResetControl,
        )));

        Ok((synth, sender))
    }

    /// 获取运行时统计信息
    pub fn stats(&self) -> XSynthStats {
        let stats = self.synth.get_stats();
        XSynthStats {
            voice_count: stats.voice_count(),
            average_renderer_load: stats.buffer().average_renderer_load(),
            buffer_samples: stats.buffer().last_samples_after_read(),
        }
    }

    /// 检查音频流是否因设备移除等不可用，需要恢复。
    ///
    /// 底层（xsynth-realtime）在音频设备被拔出/更换时自动尝试重定向到
    /// 系统默认输出设备；仅当自愈失败（如新设备参数与管线不一致）时才返回 `true`。
    pub fn poll_stream_recovery_needed(&self) -> bool {
        self.synth.poll_recovery_error().is_some()
    }

    /// 恢复音频流：优先直接重定向到系统默认输出设备（合成管线不变），
    /// 重定向不可行（设备参数变化）时全量重建合成管线。
    pub fn recover_stream(&mut self) -> Result<(), String> {
        match self.synth.restart_stream() {
            Ok(()) => {
                tracing::info!("XSynth: 音频流已重定向到默认输出设备（合成管线保持不变）");
                Ok(())
            }
            Err(StreamRestartError::ConfigChanged(msg)) => {
                tracing::warn!("XSynth: 设备参数已改变 ({msg})，重建合成管线");
                self.rebuild()
            }
            Err(e) => {
                tracing::warn!("XSynth: 音频流重定向失败 ({e})，重建合成管线");
                self.rebuild()
            }
        }
    }

    /// 全量重建合成管线（使用当前系统默认输出设备）。
    ///
    /// 重建后替换共享事件发送器，所有已创建的 `XSynthOutputConn` 自动跟随新管线；
    /// 无需上层重建输出连接。
    fn rebuild(&mut self) -> Result<(), String> {
        let (synth, sender) = Self::init_synth(&self.soundfont_path, self.options.as_ref())
            .map_err(|e| format!("重建合成管线失败: {e}"))?;

        // 替换合成器（旧实例 drop：发送 Shutdown 并 join 全部线程）
        self.synth = synth;
        // 替换共享发送器：已创建的输出连接通过 Arc 读取，自动指向新管线
        *self.sender_shared.lock().unwrap_or_else(|e| e.into_inner()) = sender;
        // 替换混音句柄：已创建的输出连接通过外层 Arc 读取，自动指向新管线
        *self.mixer_shared.lock().unwrap_or_else(|e| e.into_inner()) =
            self.synth.clone_channel_mix();

        tracing::info!("XSynth: 合成管线已重建");
        Ok(())
    }
}

impl Api for XSynth {
    fn version(&self) -> Option<String> {
        Some(self.version.clone())
    }

    fn inputs(&self) -> Result<Vec<InputInfo>, Error> {
        Ok(Vec::new())
    }

    fn outputs(&self) -> Result<Vec<OutputInfo>, Error> {
        Ok(vec![OutputInfo {
            id: 0,
            name: "XSynth".to_string(),
        }])
    }

    fn open_output(&self, id: u32) -> Result<Box<dyn OutputConnection>, Error> {
        if id != 0 {
            return Err(Error::DeviceNotFound(id));
        }
        Ok(Box::new(XSynthOutputConn {
            sender: Arc::clone(&self.sender_shared),
            mixer: Arc::clone(&self.mixer_shared),
            master: Arc::clone(&self.master_peak_shared),
        }))
    }

    fn open_input(
        &self,
        _id: u32,
        _callback: MidiInputCallback,
    ) -> Result<Box<dyn InputConnection>, Error> {
        Err(Error::InitFailed(
            "XSynth does not support MIDI input".into(),
        ))
    }
}
