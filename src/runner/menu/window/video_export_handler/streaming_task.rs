//! 流式 MIDI 视频导出后台任务（边解析边渲染，无完整文档驻留内存）。

use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::channel;
use std::time::Instant;

use lumino_export::video::{FfmpegEncoder, VideoExportConfig};
use lumino_gfx::render_thread::{ControlCommand, RenderCommand};
use tokio::sync::mpsc::UnboundedSender;

use super::super::video_export::{
    self, generate_keyboard_texture, seconds_to_tick, streaming::StreamingNoteSource,
};
use super::commands::{finalize_video_export, send_export_error, send_initial_render_commands};
use super::composite::{CompositeEncodeFrameInput, composite_and_encode_frame};
use super::frame::{EncodeFrameQueue, FrameParams};
use super::pipeline::FramePipeline;

/// 进度消息载荷：(文本, 进度 0..1, 总帧数, 平滑 FPS, 已用秒)
type ProgressMsg = (String, f64, u64, f64, f64);

/// 流式 MIDI 视频导出后台任务。
///
/// 1. 解析 MIDI 文件并写入硬盘缓存，同时通过 `progress_tx` 回传解析进度。
/// 2. 打开流式音符数据源，按帧 seek+read 读取可见音符。
/// 3. 其余渲染/编码/合成流程与内存模式保持一致。
pub(super) struct RunStreamingVideoExportTaskInput {
    pub config: VideoExportConfig,
    pub cmd_sender: std::sync::mpsc::Sender<RenderCommand>,
    pub progress_tx: UnboundedSender<ProgressMsg>,
    pub preview_tx: UnboundedSender<(Vec<u8>, u32, u32)>,
    pub midi_path: String,
    pub fps_f64: f64,
    pub key_count: u16,
    pub width: u32,
    pub height: u32,
    pub cancel_flag: Arc<AtomicBool>,
    pub input_pix_fmt: &'static str,
}

pub(super) fn run_streaming_video_export_task(input: RunStreamingVideoExportTaskInput) {
    let RunStreamingVideoExportTaskInput {
        config,
        cmd_sender,
        progress_tx,
        preview_tx,
        midi_path,
        fps_f64,
        key_count,
        width,
        height,
        cancel_flag,
        input_pix_fmt,
    } = input;
    let start = std::time::Instant::now();

    // 阶段 1：解析 MIDI → 硬盘缓存（终端进度条）
    let parse_bar = Arc::new(std::sync::Mutex::new(
        video_export::cli_progress::CliProgressBar::new(30, "MIDI解析"),
    ));
    let progress_tx_for_parse = progress_tx.clone();
    let parse_bar_for_cb = Arc::clone(&parse_bar);
    let parse_progress: std::sync::Arc<dyn Fn(String, f64) + Send + Sync> =
        std::sync::Arc::new(move |message: String, value: f64| {
            if let Ok(mut bar) = parse_bar_for_cb.lock() {
                bar.update(value, &message);
            }
            // 解析阶段进度映射到 0.0 ~ 0.3，与渲染阶段 0.3 ~ 1.0 衔接
            let scaled = value * 0.3;
            let _ = progress_tx_for_parse.send((message, scaled, 0, 0.0, 0.0));
        });

    let parse_result = video_export::streaming::parse_midi_to_cache(
        std::path::Path::new(&midi_path),
        fps_f64,
        16.0, // 视口小节数，与内存模式一致（ppq * 16）
        Some(parse_progress),
    );

    let streaming_result = match parse_result {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("视频导出 MIDI 解析失败: {e}");
            send_export_error(&progress_tx, format!("导出失败: {e}"));
            return;
        }
    };
    if let Ok(mut bar) = parse_bar.lock() {
        bar.finish("缓存就绪");
    }

    let mut source = match StreamingNoteSource::open(streaming_result) {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("视频导出打开流式数据源失败: {e}");
            send_export_error(&progress_tx, format!("导出失败: {e}"));
            return;
        }
    };

    let ppq = source.ppqn();
    let total_frames = source.total_frames();
    let total_ticks = source.total_ticks();
    let duration_secs = source.compute_duration_secs();

    let mut render_bar = video_export::cli_progress::CliProgressBar::new(30, "视频渲染");
    render_bar.update(
        0.0,
        &format!(
            "总时长 {:.1}s | 总帧数 {} | PPQN {} | total_ticks {}",
            duration_secs, total_frames, ppq, total_ticks
        ),
    );

    // 创建帧数据通道与回收通道
    let (frame_tx, frame_rx) = channel::<Vec<u8>>();
    let (recycle_tx, recycle_rx) = channel::<Vec<u8>>();

    // 创建 FFmpeg 编码器
    let mut encoder = match FfmpegEncoder::new(&config, input_pix_fmt) {
        Ok(e) => e,
        Err(e) => {
            tracing::error!("FFmpeg 创建失败: {e}");
            send_export_error(&progress_tx, format!("导出失败: {e}"));
            return;
        }
    };

    // 发送初始渲染命令
    if send_initial_render_commands(
        &cmd_sender,
        width,
        height,
        frame_tx,
        recycle_rx,
        &progress_tx,
    ) {
        return;
    }

    // 生成键盘贴图
    let (keyboard_pixels, kb_w, kb_h) = generate_keyboard_texture(width, height, key_count);

    let mut last_preview_time = Instant::now();
    let mut preview_sent = false;

    let mut param_queue: EncodeFrameQueue = EncodeFrameQueue::new();

    // 入队闭包：读取流式音符、计算键色、发送渲染命令
    let (processed_frames, cancelled, smoothed_fps) = {
        let mut enqueue_frame = |queue: &mut EncodeFrameQueue, frame_idx: u64| -> bool {
            let (notes, params) =
                match source.read_notes_and_params_for_frame(frame_idx, width, height, fps_f64) {
                    Ok(v) => v,
                    Err(e) => {
                        tracing::error!("读取流式音符失败: {e}");
                        send_export_error(&progress_tx, format!("导出失败: {e}"));
                        return true;
                    }
                };

            let tick = seconds_to_tick(
                frame_idx as f64 / fps_f64,
                source.tempo_changes(),
                source.ppqn(),
            );

            // 计算按键高亮颜色
            let mut key_colors = [0u8; video_export::keyboard::KEY_COLOR_BYTES];
            let note_tuples: Vec<(u32, u32, u16, u16)> = notes
                .iter()
                .map(|n| (n.start_tick, n.end_tick, n.key, n.track))
                .collect();
            video_export::keyboard::update_playback_key_colors_from_notes(
                &note_tuples,
                tick,
                &mut key_colors,
            );

            // 计算 scroll_x / zoom_x，用于标尺小节号合成
            let video_kb_width = 60.0f32;
            let video_viewport_tick_span = (ppq * 16).max(1) as f32;
            let video_zoom_x = (width as f32 - video_kb_width) / video_viewport_tick_span;
            let video_scroll_x = tick as f32 * video_zoom_x;

            queue.push_back(FrameParams {
                scroll_x: video_scroll_x,
                zoom_x: video_zoom_x,
                keyboard_width: video_kb_width,
                ppq,
                key_colors,
            });

            if cmd_sender
                .send(RenderCommand::Control(ControlCommand::RenderVideoFrame {
                    params: Box::new(params),
                }))
                .is_err()
            {
                tracing::error!("发送 RenderVideoFrame 命令失败");
                let _ =
                    progress_tx.send(("导出失败：渲染线程通信错误".to_string(), -1.0, 0, 0.0, 0.0));
                return true;
            }
            false
        };

        // 预填充 + 主循环 + drain 由公共 FramePipeline 驱动（与内存路径共用同一循环骨架）
        let mut pipeline = FramePipeline {
            total_frames,
            cancel_flag: &cancel_flag,
            frame_rx: &frame_rx,
            param_queue: &mut param_queue,
            progress_tx: &progress_tx,
            render_bar: &mut render_bar,
            start,
            // 流式路径：解析阶段进度 0-0.3，渲染阶段映射到 0.3-1.0
            progress_map: |raw| 0.3 + raw * 0.7,
        };
        let mut process_frame = |stream_frame: Vec<u8>, stream_params: FrameParams| {
            composite_and_encode_frame(CompositeEncodeFrameInput {
                data: stream_frame,
                params: stream_params,
                encoder: &mut encoder,
                progress_tx: &progress_tx,
                preview_tx: &preview_tx,
                cancel_flag: &cancel_flag,
                last_preview_time: &mut last_preview_time,
                preview_sent: &mut preview_sent,
                width,
                height,
                keyboard_pixels: &keyboard_pixels,
                kb_w,
                kb_h,
                recycle_tx: &recycle_tx,
            })
        };
        pipeline.run(&mut enqueue_frame, &mut process_frame)
    }; // 块结束：enqueue_frame/process_frame/pipeline 释放，后续可访问 source

    let elapsed = start.elapsed().as_secs_f64();
    if cancelled {
        render_bar.finish(&format!(
            "已取消 | 已处理 {}/{} 帧 | 耗时 {:.1}s",
            processed_frames, total_frames, elapsed
        ));
    } else {
        render_bar.finish(&format!(
            "完成 {}/{} 帧 | 耗时 {:.1}s",
            processed_frames, total_frames, elapsed
        ));
    }
    finalize_video_export(
        &cmd_sender,
        encoder,
        cancelled,
        elapsed,
        total_frames,
        smoothed_fps,
        &progress_tx,
    );

    // 清理流式 MIDI 缓存文件（先关闭文件句柄再删除）
    let cache_path = source.cache_path().to_path_buf();
    drop(source);
    if let Err(e) = std::fs::remove_file(&cache_path) {
        tracing::warn!("清理 MIDI 缓存文件失败: {e}");
    }
}
