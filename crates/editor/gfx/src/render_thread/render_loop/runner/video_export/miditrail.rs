//! Miditrail 3D 帧渲染：首帧全量常驻 + 每帧 GPU cull 窗口 → legacy 精确渲染。
//!
//! UI 首帧发送全量 `note_instances`（`collect_all`，一次上传自有常驻）；后续帧只发
//! uniforms，窗口由 cull 内核提取并回读（V×16B），走未经修改的 legacy
//! `render_from_instances`——像素与现状逐位一致（集合等价 harness 保证），
//! 2026-09-05 的 driven 视觉 veto 不触发。渲染侧 CPU 只剩 compact 换算与实例构建
//!（窗口级），UI 侧 collect/sort/pack 归零。
//! cull 不可用时回退所带音符（首帧全量/空帧黑帧 + 错误日志，属防御路径）。
//!
//! 注意：钢琴卷帘共享缓冲（`frame.renderers.note`）此处不碰——3D 管线自有实例缓冲，
//! 往共享缓冲上传是纯死工作（memcpy＋write_buffer＋cull bind group 重建）。

use super::common::{CopyDrainInput, copy_texture_and_drain};
use crate::CullWindow;
use crate::MiditrailRenderer;
use crate::MiditrailUniformGpu;
use crate::miditrail_renderer::miditrail_viewport_span;
use crate::render_thread::params::RenderParams;
use crate::render_thread::render_loop::runner::context::{RenderContext, RenderFrameState};

/// Miditrail 3D 帧渲染：使用 3D 渲染管线写入颜色纹理后读回。
pub(super) fn handle_miditrail_frame(
    ctx: &RenderContext,
    params: RenderParams,
    frame: &mut RenderFrameState,
    width: u32,
    height: u32,
) {
    // 3D 管线自有实例缓冲，不经过钢琴共享缓冲（旧 `upload_instances` 是死工作，已删）。
    if frame.miditrail_renderer.is_none() {
        *frame.miditrail_renderer = Some(MiditrailRenderer::new(&ctx.device));
    }
    // 不变式：上面 is_none 判断后必然已创建；release 下若异常缺失则跳过本帧而非崩溃
    let renderer = match frame.miditrail_renderer.as_mut() {
        Some(r) => r,
        None => {
            debug_assert!(false, "miditrail_renderer 应已初始化（is_none 分支已创建）");
            return;
        }
    };

    let kb_height = ((height as f64) * 0.12).round() as u32;
    let kb_height = kb_height.max(20).min(height / 3);

    // 光晕环动画时间基准：导出参数已按当前 BPM 计算；
    // 0（未知，如默认参数）回退到 120 BPM（ppq × 2）。
    let ticks_per_second = if params.miditrail_ticks_per_second > 0.0 {
        params.miditrail_ticks_per_second
    } else {
        params.ppq.max(1.0) * 2.0
    };

    let uniform = MiditrailUniformGpu {
        tick: params.miditrail_current_tick,
        ppq: params.ppq as u32,
        key_count: (params.max_key_index + 1.0) as u32,
        frame_width: width,
        frame_height: height,
        kb_height,
        _reserved: 0,
        speed: params.miditrail_speed.max(0.1),
        param1: params.waterfall_speed.max(0.1),
        param2: 0.0,
        fps: params.fps.max(1.0),
        z_far_distance: params.miditrail_z_far.max(0.1),
        view_mode: params.miditrail_view_mode,
        ticks_per_second,
        _padding1: 0,
    };

    // 常驻：非空帧播种（首帧全量；后续帧为空，跳过零上传）。
    if !params.note_instances.is_empty() {
        renderer.seed_resident(&ctx.device, &ctx.queue, &params.note_instances);
    }
    // 3D 音符开关（默认关 = 平面）：只切换音符 draw 的索引缓冲，实例/顺序/管线不动。
    renderer.flat_notes = !params.miditrail_3d_notes;
    let geo_label = if renderer.flat_notes { "quad" } else { "box" };
    // cull 窗口与 UI 收集同公式（`miditrail_viewport_span` 共享，保证谓词一致）。
    let tick = params.miditrail_current_tick;
    let key_count = (params.max_key_index + 1.0).max(1.0) as usize;
    let tick_end = tick.saturating_add(miditrail_viewport_span(
        params.ppq as u32,
        params.miditrail_speed,
        params.miditrail_z_far,
    ));

    let t_work = std::time::Instant::now();
    let mut encoder = ctx
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("miditrail_encoder"),
        });

    // cull 提取 + 回读 → legacy 精确渲染（Normal/Top 双视图保持现状像素）。
    // cull 失败回退所带音符（首帧全量已排序，回退正确；空帧黑帧 + 错误日志）。
    let window = CullWindow {
        tick_start: tick,
        tick_end,
        key_count,
    };
    let (path_label, note_total, count_us, fill_us, legacy_us) =
        match renderer.cull_window(&ctx.device, &ctx.queue, window) {
            Ok((window, timing)) => {
                let n = window.len();
                let t_legacy = std::time::Instant::now();
                renderer.render_from_instances(
                    &ctx.device,
                    &ctx.queue,
                    &mut encoder,
                    &uniform,
                    &window,
                );
                let legacy_us = t_legacy.elapsed().as_micros() as u64;
                renderer.restore_window(window);
                (
                    "cull-legacy",
                    n,
                    timing.count_us,
                    timing.fill_readback_us,
                    legacy_us,
                )
            }
            Err(e) => {
                tracing::error!("Miditrail cull 失败，回退所带音符: {e}");
                let n = params.note_instances.len();
                let t_legacy = std::time::Instant::now();
                renderer.render_from_instances(
                    &ctx.device,
                    &ctx.queue,
                    &mut encoder,
                    &uniform,
                    &params.note_instances,
                );
                let legacy_us = t_legacy.elapsed().as_micros() as u64;
                ("legacy-fallback", n, 0, 0, legacy_us)
            }
        };
    let work_us = t_work.elapsed().as_micros() as u64;
    // 渲染侧分段打点（首 3 帧 + 每 300 帧）：work 含 cull 两次提交 + 回读 + legacy 渲染调度；
    // count/fill/legacy 三段拆开，下次诊断不再盲人摸象。
    {
        static RENDER_DIAG: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = RENDER_DIAG.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        if n < 3 || n.is_multiple_of(300) {
            tracing::info!(
                "miditrail渲染打点[{n}]: path={path_label} geo={geo_label} work={work_us}us notes={note_total} count={count_us}us fill_readback={fill_us}us legacy={legacy_us}us"
            );
        }
    }

    let pipeline = match frame.export_pipeline.as_mut() {
        Some(p) => p,
        None => {
            debug_assert!(false, "export_pipeline 应已初始化（pipeline_ready 已保证）");
            return;
        }
    };
    let tx = match frame.export_frame_tx.as_ref() {
        Some(t) => t,
        None => {
            debug_assert!(false, "export_frame_tx 应已初始化（pipeline_ready 已保证）");
            return;
        }
    };

    if let Some(tex) = renderer.output_texture() {
        copy_texture_and_drain(CopyDrainInput {
            pipeline,
            tx,
            encoder,
            texture: tex,
            queue: &ctx.queue,
            width,
            height,
            label: "Miditrail",
        });
    } else {
        tracing::warn!("Miditrail 输出纹理未就绪");
        ctx.queue.submit(std::iter::once(encoder.finish()));
    }
}
