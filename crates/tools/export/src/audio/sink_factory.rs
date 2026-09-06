//! 输出接收器工厂 — 根据配置创建音频输出 Sink
//!
//! 参考 OmniConverter 的 ISampleWriter 工厂逻辑：
//! - 非 PCM 格式使用 FFmpeg 子进程
//! - PCM WAV 使用 hound 直接写入

use crate::error::{ExportError, ExportResult};

use super::codec;
use super::config::AudioRenderConfig;
use super::stream::{FfmpegSink, SampleSink, WavFileSink};

/// 根据配置创建输出接收器
pub(super) fn create_output_sink(config: &AudioRenderConfig) -> ExportResult<Box<dyn SampleSink>> {
    // 参数校验（采样率/比特率越界提前报错，避免 ffmpeg 隐式失败）
    if let Err(msg) = config
        .audio_codec
        .validate(config.sample_rate, config.audio_bitrate)
    {
        return Err(ExportError::AudioWrite(msg));
    }
    let codec = config.audio_codec;

    if codec.needs_ffmpeg() {
        let ffmpeg_path = codec::find_ffmpeg().ok_or_else(|| {
            ExportError::AudioWrite(format!(
                "需要 ffmpeg 来编码 {} 格式，但未找到 ffmpeg",
                codec.extension()
            ))
        })?;

        let sink = FfmpegSink::new(
            &ffmpeg_path,
            &config.output_path,
            codec,
            config.sample_rate,
            config.channels.channel_count(),
            config.audio_bitrate,
        )?;
        Ok(Box::new(sink))
    } else {
        let channels = config.channels.channel_count();
        let sink = WavFileSink::new(&config.output_path, config.sample_rate, channels)?;
        Ok(Box::new(sink))
    }
}
