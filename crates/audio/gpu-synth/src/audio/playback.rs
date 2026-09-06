//! Realtime playback of a [`crate::GpuSynth`] through `cpal`.
//!
//! # Architecture
//!
//! The engine is owned by a background **render thread** that renders
//! continuously, paced by the block's realtime budget (90% of wall-clock
//! duration). MIDI events are sent through a channel and drained before
//! each block. Rendered blocks are pushed to the audio callback through a
//! bounded queue; when the render thread is ahead of the consumer by more
//! than 10% it sleeps, otherwise it keeps rendering — it **never drops a
//! block**. The audio callback runs on the OS audio thread and must never
//! block: it only copies from the queue and writes silence on underrun
//! (counting them in the stats).
//!
//! # Sample-rate negotiation
//!
//! The engine renders at its configured sample rate (e.g. 64 kHz). Most
//! output devices do not run at 64 kHz, so the playback layer picks the
//! device's default configuration first and falls back to any supported
//! config whose sample rate matches the engine; if none matches, it
//! resamples the engine output to the device rate with a small linear
//! interpolator. Use [`AudioPlayback::device_sample_rates`] to list what
//! a device supports before constructing the engine.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::mpsc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

/// DIAG: throttle for the `[UNDERRUN]` stderr marker below.
static LAST_UD_LOG: AtomicI64 = AtomicI64::new(0);

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

use super::resample::SincResampler;
use crate::GpuSynth;
use crate::SynthError;
use crate::midi::MidiEvent;

/// Read-only view of the realtime playback statistics.
///
/// Mirrors the stats exposed by XSynth's `BufferedRenderer` so a status
/// line like "Voice Count / Buffer / Render time" can be printed while
/// playing.
///
/// All counters are lock-free (atomics + a fixed ring of `AtomicU64` slots):
/// the render thread must never block on a lock, so stats are published
/// with relaxed stores and readers take a best-effort snapshot.
#[derive(Clone)]
pub struct PlaybackStatsReader {
    samples: Arc<AtomicI64>,
    last_request_samples: Arc<AtomicI64>,
    last_samples_after_read: Arc<AtomicI64>,
    /// Ring of recent render-load percentages (0..n), stored as f64 bits in
    /// `AtomicU64`. `render_time_head` is the next slot to write.
    render_time: Arc<[AtomicU64; STATS_RING]>,
    render_time_head: Arc<AtomicU64>,
    render_size: Arc<AtomicU64>,
    voice_count: Arc<AtomicU64>,
    underruns: Arc<AtomicU64>,
}

/// Number of recent render-load samples kept for the moving average.
const STATS_RING: usize = 128;

impl PlaybackStatsReader {
    /// The number of samples currently buffered (rendered but not yet
    /// consumed). Can be negative if the reader is waiting for samples.
    pub fn samples(&self) -> i64 {
        self.samples.load(Ordering::Relaxed)
    }

    /// The number of samples requested by the last audio callback.
    pub fn last_request_samples(&self) -> i64 {
        self.last_request_samples.load(Ordering::Relaxed)
    }

    /// The number of samples that were in the buffer after the last read.
    pub fn last_samples_after_read(&self) -> i64 {
        self.last_samples_after_read.load(Ordering::Relaxed)
    }

    /// The number of samples rendered per iteration.
    pub fn render_size(&self) -> usize {
        self.render_size.load(Ordering::Relaxed) as usize
    }

    /// The average render-time percentage (0 to 1) of how long the render
    /// thread spent rendering, relative to the max allowed time. Values
    /// above 1.0 mean the render thread cannot keep up with realtime.
    pub fn average_renderer_load(&self) -> f64 {
        let head = self.render_time_head.load(Ordering::Relaxed) as usize;
        let mut sum = 0.0f64;
        let mut n = 0usize;
        for i in 0..STATS_RING {
            let bits =
                self.render_time[(head + STATS_RING - 1 - i) % STATS_RING].load(Ordering::Relaxed);
            if bits != 0 {
                sum += f64::from_bits(bits);
                n += 1;
            }
        }
        if n == 0 { 0.0 } else { sum / n as f64 }
    }

    /// The last render-time percentage (0 to 1).
    pub fn last_renderer_load(&self) -> f64 {
        let head = self.render_time_head.load(Ordering::Relaxed) as usize;
        let slot = (head + STATS_RING - 1) % STATS_RING;
        let bits = self.render_time[slot].load(Ordering::Relaxed);
        if bits == 0 { 0.0 } else { f64::from_bits(bits) }
    }

    /// Publishes one render-load sample (render thread side; lock-free).
    pub(crate) fn push_render_load(&self, load: f64) {
        let head = self.render_time_head.load(Ordering::Relaxed) as usize;
        self.render_time[head].store(load.to_bits(), Ordering::Relaxed);
        let next = (head + 1) % STATS_RING;
        self.render_time_head.store(next as u64, Ordering::Relaxed);
    }

    /// The active voice count reported by the engine on the last render.
    pub fn voice_count(&self) -> u64 {
        self.voice_count.load(Ordering::Relaxed)
    }

    /// The number of underruns (audio callbacks that found no buffered
    /// samples and wrote silence). Zero is the healthy state.
    pub fn underruns(&self) -> u64 {
        self.underruns.load(Ordering::Relaxed)
    }
}

/// A running realtime playback session.
///
/// # Example
///
/// ```no_run
/// use lumino_gpu_synth::{GpuSynth, SynthConfig};
/// use lumino_gpu_synth::audio::playback::AudioPlayback;
///
/// let mut synth = GpuSynth::new(SynthConfig::default())?;
/// synth.load_soundfont("assets/test.sf2", 0, 0)?;
/// let mut playback = AudioPlayback::start(synth, None)?;
/// playback.note_on(0, 60, 100);
/// std::thread::sleep(std::time::Duration::from_millis(500));
/// playback.note_off(0, 60);
/// playback.stop();
/// # Ok::<(), lumino_gpu_synth::SynthError>(())
/// ```
pub struct AudioPlayback {
    stop_flag: Arc<AtomicBool>,
    stop_tx: Option<mpsc::Sender<()>>,
    event_tx: Option<mpsc::Sender<(u8, MidiEvent)>>,
    stream_tx: Option<mpsc::Sender<Vec<crate::midi::TimedEvent>>>,
    thread: Option<JoinHandle<()>>,
    sample_rate: u32,
    engine_rate: u32,
    stats: PlaybackStatsReader,
    /// 持有「stream owner」线程的句柄。
    ///
    /// 与 xsynth-realtime 相同：cpal 0.15 的 `Stream` 在 Windows 上是 `!Send`
    /// （携带 `NotSendSyncAcrossAllPlatforms`），不能在 `Send` 结构上跨线程移动。
    /// 因此 `Stream` 在 stream owner 线程内部创建并存活，这里只持有其 `JoinHandle`
    /// （`Send + Sync`），从而让整个后端满足 `Api: Send + Sync`。
    _stream_owner: Option<JoinHandle<()>>,
}

impl AudioPlayback {
    /// Opens the default output device and starts the render/playback
    /// thread.
    ///
    /// The device is opened with the engine's configured sample rate if the
    /// device supports it, otherwise with the device default sample rate and
    /// the engine output is linearly resampled to match.
    ///
    /// # Errors
    ///
    /// Returns [`SynthError::Gpu`] if no audio output device is available or
    /// the stream cannot be opened.
    pub fn start(mut synth: GpuSynth, device: Option<cpal::Device>) -> Result<Self, SynthError> {
        let engine_rate = synth.config().sample_rate;
        let channels = synth.config().channels.channel_count();
        let block = synth.config().block_size;

        // 优先使用调用方解析出的指定输出设备；否则回退到系统默认输出设备。
        let device = match device {
            Some(d) => d,
            None => cpal::default_host()
                .default_output_device()
                .ok_or_else(|| SynthError::Gpu("no default audio output device".into()))?,
        };

        // Negotiate: prefer the engine rate, else the device default, and
        // remember which we got so the render thread can resample.
        let (stream_config, resample_needed) =
            negotiate_config(&device, engine_rate, channels, block)?;
        let device_rate = stream_config.sample_rate.0;
        let needs_resample = resample_needed || device_rate != engine_rate;

        let (stop_tx, stop_rx) = mpsc::channel::<()>();
        let (event_tx, event_rx) = mpsc::channel::<(u8, MidiEvent)>();
        let (stream_tx, stream_rx) = mpsc::channel::<Vec<crate::midi::TimedEvent>>();
        let (sample_tx, sample_rx) = mpsc::sync_channel::<Vec<f32>>(32);
        let stop_flag = Arc::new(AtomicBool::new(false));

        // Stats shared between the callback, the render thread and the caller.
        let stats = PlaybackStatsReader {
            samples: Arc::new(AtomicI64::new(0)),
            last_request_samples: Arc::new(AtomicI64::new(0)),
            last_samples_after_read: Arc::new(AtomicI64::new(0)),
            render_time: Arc::new(std::array::from_fn(|_| AtomicU64::new(0))),
            render_time_head: Arc::new(AtomicU64::new(0)),
            render_size: Arc::new(AtomicU64::new((block * channels) as u64)),
            voice_count: Arc::new(AtomicU64::new(0)),
            underruns: Arc::new(AtomicU64::new(0)),
        };
        let cb_stats = stats.clone();

        // 注意：cpal 0.15 的 `Stream` 在 Windows 上是 `!Send`（携带
        // `NotSendSyncAcrossAllPlatforms`），不能在 `Send` 结构上跨线程移动。
        // 与 xsynth-realtime 相同，音频流在下方 start 末尾的「stream owner」
        // 线程内部创建并持有，`AudioPlayback` 只保留其 `JoinHandle`。

        // Pre-fill the queue with a few silent blocks BEFORE the render
        // thread and the audio callback race each other: the callback can
        // fire as soon as `stream.play()` above returns, and the first real
        // (dense) block takes tens of ms to render - without this cushion
        // the opening of black-MIDI underruns. The blocks are silence (no
        // events are loaded yet) and stream out at the normal pace. 8
        // blocks (~170ms at 2048/48k) covers the first dense block render
        // plus the render thread's catch-up burst.
        {
            let mut warm_buf = vec![0.0f32; block * channels];
            let mut warm_resampler = SincResampler::new(engine_rate, device_rate, channels);
            for _ in 0..8 {
                let _ = synth.render_block(&mut warm_buf);
                let out = if needs_resample {
                    warm_resampler.process(&warm_buf)
                } else {
                    warm_buf.clone()
                };
                stats.samples.fetch_add(out.len() as i64, Ordering::SeqCst);
                sample_tx
                    .send(out)
                    .expect("queue prefill: audio queue must be empty at start");
            }
        }

        // Render thread: owns the engine and renders continuously. The
        // cadence is the block's wall-clock duration * 90% so the thread can
        // catch up when a block is slow. If it is more than 10% ahead of the
        // consumer it sleeps; otherwise it keeps rendering. Blocks are never
        // dropped: if the queue is full we wait (the consumer is draining).
        let thread_stop = stop_flag.clone();
        let thread_stats = stats.clone();
        let thread = thread::Builder::new()
            .name("lumino-gpu-synth-render".into())
            .spawn(move || {
                let mut synth = synth;
                let mut buf = vec![0.0f32; block * channels];
                let mut resampler = SincResampler::new(engine_rate, device_rate, channels);
                let mut last_err = false;
                // Max allowed render time per block: 90% of realtime so the
                // thread runs slightly ahead and the queue accumulates a
                // cushion that absorbs peak blocks (dense black-MIDI). The
                // queue's `try_send` wait throttles when we run too far
                // ahead. NOTE: based on `block` (one frame, all channels) —
                // using `block * channels` would double the budget.
                let delay = Duration::from_secs_f64(block as f64 / engine_rate.max(1) as f64 * 0.9);

                // If a full event stream is supplied, the engine consumes it
                // internally by `global_frame` (no per-event channel traffic)
                // - the only way to keep up with dense black-MIDI.
                let mut has_stream = false;

                loop {
                    // Accept an event stream (usually once, at startup).
                    if let Ok(events) = stream_rx.try_recv() {
                        synth.set_events(events);
                        has_stream = true;
                    }
                    // Drain pending MIDI events (non-blocking).
                    while let Ok((ch, ev)) = event_rx.try_recv() {
                        synth.send_event(ch, ev);
                    }
                    if thread_stop.load(Ordering::Relaxed) || stop_rx.try_recv().is_ok() {
                        break;
                    }

                    // When a full stream is loaded, stop the render thread
                    // once the stream is exhausted (plus a decay tail) so the
                    // playback ends on its own instead of idling forever.
                    if has_stream {
                        let done = synth.stream_exhausted();
                        if done {
                            break;
                        }
                    }

                    // No explicit backpressure loop here: the bounded queue
                    // itself is the throttle. The cadence sleep below paces
                    // rendering at 90% of realtime (so we stay slightly
                    // ahead), and when the queue fills the `try_send` wait
                    // below naturally slows us to the consumer's pace. An
                    // explicit "samples > requested * k" check would compare
                    // one block (~8k samples) against a single callback
                    // request (~1k samples) and sleep after every block,
                    // keeping the queue nearly empty - the cause of the
                    // periodic underruns.

                    let start = Instant::now();
                    if let Err(e) = synth.render_block(&mut buf) {
                        // Never die silently: a wedged GPU surfaces here every
                        // block; print it once so the freeze is diagnosable
                        // instead of looking like a hung process.
                        if !last_err {
                            eprintln!("[render] block error: {e}");
                            last_err = true;
                        }
                        std::thread::sleep(delay / 10);
                        continue;
                    }
                    last_err = false;

                    // NOTE: lookahead sample pre-upload is NOT done here.
                    // Resampling a large SF2 sample takes ~300 ms no matter
                    // how it is chunked (the total work is fixed), so
                    // spreading it across blocks makes EVERY block slow
                    // instead of a few. The correct fix is `prewarm_midi_file`
                    // before playback (see examples/realtime_midi.rs); the
                    // engine keeps `prefetch_samples` for callers that want
                    // bounded incremental uploads.
                    thread_stats
                        .voice_count
                        .store(synth.voice_count() as u64, Ordering::Relaxed);

                    let out = if needs_resample {
                        resampler.process(&buf)
                    } else {
                        buf.clone()
                    };
                    thread_stats
                        .samples
                        .fetch_add(out.len() as i64, Ordering::SeqCst);

                    // Record the actual GPU/CPU render cost (render_block +
                    // resample), NOT including the cadence sleep below - the
                    // sleep is deliberate pacing, not render load.
                    let elapsed = start.elapsed().as_secs_f64();
                    let total = delay.as_secs_f64();
                    thread_stats.push_render_load(elapsed / total);

                    // Push without dropping: wait while the queue is full.
                    // The wait below is backpressure (the consumer is
                    // draining), NOT render load - so the render-load
                    // percentage is recorded BEFORE it, once per block,
                    // from the actual `render_block` cost only.
                    loop {
                        if sample_tx.try_send(out.clone()).is_ok() {
                            break;
                        }
                        if thread_stop.load(Ordering::Relaxed) || stop_rx.try_recv().is_ok() {
                            return;
                        }
                        std::thread::sleep(delay / 10);
                    }

                    // Sleep until the next cadence point (90% of realtime) to
                    // play in real time - UNLESS the queue is below the
                    // target cushion (about two blocks buffered): then render
                    // back-to-back to refill it so peak blocks never starve
                    // the audio callback.
                    let now = Instant::now();
                    let end = start + delay;
                    let cushion = (block * channels * 2) as i64;
                    if thread_stats.samples.load(Ordering::SeqCst) >= cushion && end > now {
                        std::thread::sleep(end - now);
                    }
                }
            })
            .map_err(SynthError::Io)?;

        // ── stream owner 线程：在「线程内部」创建 cpal Stream ──
        // 绝不在 Send 结构上跨线程移动 !Send 的 `Stream`；线程内建流、播放、
        // 然后持活直到 `stop_flag` 置位（Stream 析构即停止音频回调）。
        let (stream_tx_result, stream_rx_result) = mpsc::channel::<Result<(), SynthError>>();
        let owner_stop = stop_flag.clone();
        let stream_owner = thread::Builder::new()
            .name("lumino-gpu-synth-stream-owner".into())
            .spawn(move || {
                let err_fn = |e| eprintln!("lumino-gpu-synth playback error: {e}");
                let mut next_block: Vec<f32> = Vec::new();
                let mut next_pos = 0usize;
                let stream = match device.build_output_stream(
                    &stream_config,
                    move |data: &mut [f32], _info: &cpal::OutputCallbackInfo| {
                        cb_stats
                            .last_request_samples
                            .store(data.len() as i64, Ordering::SeqCst);
                        let mut i = 0;
                        while i < data.len() {
                            if next_pos >= next_block.len() {
                                match sample_rx.try_recv() {
                                    Ok(b) => {
                                        next_block = b;
                                        next_pos = 0;
                                    }
                                    Err(_) => {
                                        data[i..].fill(0.0);
                                        cb_stats.underruns.fetch_add(1, Ordering::Relaxed);
                                        let ms = std::time::SystemTime::now()
                                            .duration_since(std::time::UNIX_EPOCH)
                                            .unwrap_or_default()
                                            .as_millis()
                                            as i64;
                                        let prev = LAST_UD_LOG.fetch_max(ms, Ordering::Relaxed);
                                        if ms - prev > 500 {
                                            eprintln!(
                                                "[UNDERRUN] queue empty (total: {})",
                                                cb_stats.underruns.load(Ordering::Relaxed)
                                            );
                                        }
                                        break;
                                    }
                                }
                            } else {
                                let n = (next_block.len() - next_pos).min(data.len() - i);
                                data[i..i + n].copy_from_slice(&next_block[next_pos..next_pos + n]);
                                i += n;
                                next_pos += n;
                            }
                        }
                        cb_stats.samples.fetch_sub(i as i64, Ordering::SeqCst);
                        cb_stats
                            .last_samples_after_read
                            .store(cb_stats.samples.load(Ordering::SeqCst), Ordering::Relaxed);
                    },
                    err_fn,
                    None,
                ) {
                    Ok(s) => s,
                    Err(e) => {
                        let _ = stream_tx_result
                            .send(Err(SynthError::Gpu(format!("audio stream: {e}"))));
                        return;
                    }
                };
                if let Err(e) = stream.play() {
                    let _ = stream_tx_result
                        .send(Err(SynthError::Gpu(format!("audio stream play: {e}"))));
                    return;
                }
                // 通知主线程流已就绪；随后保持 Stream 存活直到停止。
                let _ = stream_tx_result.send(Ok(()));
                let _stream = stream;
                while !owner_stop.load(Ordering::Relaxed) {
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
            })
            .map_err(SynthError::Io)?;

        match stream_rx_result.recv() {
            Ok(Ok(())) => {}
            Ok(Err(e)) => return Err(e),
            Err(_) => {
                return Err(SynthError::Gpu(
                    "audio stream thread terminated during startup".into(),
                ));
            }
        }

        Ok(Self {
            stop_flag,
            stop_tx: Some(stop_tx),
            event_tx: Some(event_tx),
            stream_tx: Some(stream_tx),
            thread: Some(thread),
            sample_rate: device_rate,
            engine_rate,
            stats,
            _stream_owner: Some(stream_owner),
        })
    }

    /// Plays a full, sample-accurate event stream (from
    /// [`MidiFile::load`]) in real time.
    ///
    /// The events are consumed internally by the render thread's block
    /// progression, so this scales to dense black-MIDI (millions of note
    /// events) without per-event channel traffic. The render thread ends by
    /// itself once the stream is exhausted and the voices decay.
    pub fn play_events(&mut self, events: Vec<crate::midi::TimedEvent>) {
        if let Some(tx) = &self.stream_tx {
            let _ = tx.send(events);
        }
    }

    /// Returns a snapshot reader of the playback statistics.
    ///
    /// Useful for printing a live status line (voice count / buffer /
    /// render load) while playing.
    pub fn stats(&self) -> PlaybackStatsReader {
        self.stats.clone()
    }

    /// Returns a cloneable handle to the realtime event sender.
    ///
    /// Lumino's `OutputConnection` uses this to inject MIDI events into the
    /// render thread without owning the `AudioPlayback` (which also owns the
    /// cpal audio stream). Returns `None` once playback has been stopped.
    pub fn event_sender(&self) -> Option<mpsc::Sender<(u8, MidiEvent)>> {
        self.event_tx.clone()
    }

    /// Lists the sample rates the default output device supports (empty if
    /// the device cannot be queried).
    pub fn device_sample_rates() -> Vec<u32> {
        let host = cpal::default_host();
        let Some(device) = host.default_output_device() else {
            return Vec::new();
        };
        let mut rates = Vec::new();
        if let Ok(iter) = device.supported_output_configs() {
            for cfg in iter {
                rates.push(cfg.min_sample_rate().0);
                rates.push(cfg.max_sample_rate().0);
            }
        }
        rates.sort_unstable();
        rates.dedup();
        rates
    }

    /// Sends a MIDI event to the engine (applied at the next block).
    pub fn send_event(&mut self, channel: u8, event: MidiEvent) {
        if let Some(tx) = &self.event_tx {
            let _ = tx.send((channel, event));
        }
    }

    /// Convenience: sends a note-on.
    pub fn note_on(&mut self, channel: u8, key: u8, vel: u8) {
        self.send_event(channel, MidiEvent::NoteOn { key, vel });
    }

    /// Convenience: sends a note-off.
    pub fn note_off(&mut self, channel: u8, key: u8) {
        self.send_event(channel, MidiEvent::NoteOff { key });
    }

    /// Convenience: sends a control change.
    pub fn control_change(&mut self, channel: u8, controller: u8, value: u8) {
        self.send_event(channel, MidiEvent::ControlChange { controller, value });
    }

    /// Convenience: sends a program change (instrument selection).
    pub fn program_change(&mut self, channel: u8, program: u8) {
        self.send_event(channel, MidiEvent::ProgramChange { program });
    }

    /// Convenience: sends a pitch bend. `value` is the raw 14-bit value
    /// (0-16383, 8192 = center).
    pub fn pitch_bend(&mut self, channel: u8, value: u16) {
        self.send_event(channel, MidiEvent::PitchBend { value });
    }

    /// Sends a control change to a channel with 14-bit MSB/LSB splitting
    /// (e.g. CC1/CC33 for vibrato depth).
    pub fn control_change_14bit(&mut self, channel: u8, msb: u8, lsb: u8, value: u16) {
        let hi = (value >> 7) as u8 & 0x7F;
        let lo = (value & 0x7F) as u8;
        self.control_change(channel, msb, hi);
        self.control_change(channel, lsb, lo);
    }

    /// Damper pedal (CC64): `down` holds all released notes until lifted.
    pub fn damper(&mut self, channel: u8, down: bool) {
        self.control_change(channel, 0x40, if down { 127 } else { 0 });
    }

    /// All notes off (CC123): releases every note on the channel.
    pub fn all_notes_off(&mut self, channel: u8) {
        self.control_change(channel, 0x7B, 0);
    }

    /// All sounds off (CC120): kills every voice on the channel instantly.
    pub fn all_sounds_off(&mut self, channel: u8) {
        self.control_change(channel, 0x78, 0);
    }

    /// Reset all controllers (CC121).
    pub fn reset_controllers(&mut self, channel: u8) {
        self.control_change(channel, 0x79, 0);
    }

    /// The device sample rate in use.
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// The engine's render sample rate (events scheduled against this).
    pub fn engine_sample_rate(&self) -> u32 {
        self.engine_rate
    }

    /// Returns true while the render thread is still alive (playing).
    pub fn thread_running(&self) -> bool {
        self.thread.is_some()
    }

    /// Stops the render thread (and closes the audio stream).
    pub fn stop(&mut self) {
        self.stop_flag.store(true, Ordering::Relaxed);
        if let Some(tx) = self.stop_tx.take() {
            let _ = tx.send(());
        }
        if let Some(h) = self.thread.take() {
            let _ = h.join();
        }
        if let Some(h) = self._stream_owner.take() {
            let _ = h.join();
        }
        self.event_tx = None;
        self.stream_tx = None;
    }
}

impl Drop for AudioPlayback {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Negotiates the `StreamConfig` for `device`.
///
/// Strategy (verified against WASAPI shared mode): the *enumerated* sample
/// rates are a broad claim and opening a stream at a non-default rate often
/// fails with "not supported in shared mode". The robust choice is the
/// device's *default* configuration: it is guaranteed to open. When the
/// device default rate differs from the engine rate, the render thread
/// resamples (see [`SincResampler`]). `resampled` reports that case.
fn negotiate_config(
    device: &cpal::Device,
    engine_rate: u32,
    channels: usize,
    block: usize,
) -> Result<(cpal::StreamConfig, bool), SynthError> {
    let default = device
        .default_output_config()
        .map_err(|e| SynthError::Gpu(format!("default output config: {e}")))?;
    let mut stream: cpal::StreamConfig = default.into();
    // The device default is authoritative; keep the engine's channel count
    // only when the device has at least that many (mono on a stereo device
    // is up-mixed by the callback writing L/R, so we keep stereo).
    if (stream.channels as usize) < channels {
        stream.channels = channels as u16;
    }
    // Fixed buffer size is not guaranteed either; prefer the device default
    // unless the device explicitly supports our block size.
    if let Ok(mut iter) = device.supported_output_configs() {
        let compatible = iter.any(|cfg| {
            matches!(
                cfg.buffer_size(),
                cpal::SupportedBufferSize::Range { min, max }
                    if block as u32 >= *min && block as u32 <= *max
            )
        });
        if compatible {
            stream.buffer_size = cpal::BufferSize::Fixed(block as u32);
        }
    }
    let resampled = stream.sample_rate.0 != engine_rate;
    Ok((stream, resampled))
}
