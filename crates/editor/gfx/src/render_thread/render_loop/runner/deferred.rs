use lumino_midiplayer::texture_waterfall::{
    WaterfallCommand, WaterfallGenerateContext, WaterfallGpuCtx, WaterfallUploadTileParams,
    handle_regenerate_waterfall_track, handle_waterfall_dirty_overlay, handle_waterfall_dispose,
    handle_waterfall_generate, upload_waterfall_video_tiles,
};

use super::super::super::commands::ControlCommand;
use super::context::{DeferredCommandContext, VideoExportFrameContext, VideoExportSetupContext};
use super::video_export::{render_video_frame_command, start_video_export};

/// 处理单个延迟控制命令（视频导出 / 贴图瀑布流）。
pub(super) fn handle_deferred_command(
    cmd: ControlCommand,
    context: &mut DeferredCommandContext<'_>,
) {
    match cmd {
        // ── 视频导出命令：在此内联处理（需要 GPU 资源 + 离屏纹理）──
        ControlCommand::StartVideoExport {
            width,
            height,
            frame_tx,
            recycle_rx,
        } => {
            start_video_export(VideoExportSetupContext {
                ctx: context.ctx,
                width,
                height,
                frame_tx,
                recycle_rx,
                export_pipeline: context.export_pipeline,
                export_frame_tx: context.export_frame_tx,
                export_renderers: context.export_renderers,
            });
        }
        ControlCommand::RenderVideoFrame { params } => {
            render_video_frame_command(VideoExportFrameContext {
                ctx: context.ctx,
                channels: context.channels,
                params: *params,
                export_renderers: context.export_renderers,
                renderers: context.renderers,
                onion_streaming_in_progress: context.onion_streaming_in_progress,
                current_texture: context.current_texture,
                depth_texture: context.depth_texture,
                depth_texture_view: context.depth_texture_view,
                texture_view: context.texture_view,
                current_size: context.current_size,
                last_note_version: context.last_note_version,
                texture_waterfall_renderer: context.texture_waterfall_renderer,
                texture_waterfall_meta: context.texture_waterfall_meta,
                texture_waterfall_config: context.texture_waterfall_config,
                export_pipeline: context.export_pipeline,
                export_frame_tx: context.export_frame_tx,
                waterfall_renderer: context.waterfall_renderer,
                miditrail_renderer: context.miditrail_renderer,
            });
        }
        ControlCommand::FinishVideoExport => {
            tracing::info!("视频导出完成，释放读回管线与导出常驻");
            // 常驻释放：两个渲染器与渲染线程同寿命，resident/cull/实例缓冲
            // 在导出间永驻显存（24M 文档约 700MB+），且此前本命令无人发送
            // （死代码）——此处与发送侧（finalize 成功/取消双路径）一起复活。
            if let Some(r) = context.miditrail_renderer.as_mut() {
                r.release_export_resources();
            }
            if let Some(r) = context.waterfall_renderer.as_mut() {
                r.release_export_resources();
            }
            *context.export_pipeline = None;
            *context.export_frame_tx = None;
            *context.export_renderers = None;
        }
        // ── 贴图瀑布流命令：转发到 lumino-midiplayer runner ──
        ControlCommand::Waterfall(cmd) => handle_waterfall_command(cmd, context),
        // 其余命令在 classify_command 阶段已处理，不会走到这里
        ControlCommand::Resize { .. } | ControlCommand::Shutdown => {}
    }
}

/// 分发贴图瀑布流命令到 midiplayer runner（构造 WaterfallGpuCtx 解耦宿主上下文）。
fn handle_waterfall_command(cmd: WaterfallCommand, context: &mut DeferredCommandContext<'_>) {
    let gpu = WaterfallGpuCtx {
        device: &context.ctx.device,
        queue: &context.ctx.queue,
        texture_format: context.ctx.texture_format,
    };

    match cmd {
        WaterfallCommand::Generate {
            notes,
            ppq,
            key_count,
            total_ticks,
            config,
            midi_hash,
        } => {
            handle_waterfall_generate(WaterfallGenerateContext {
                gpu: &gpu,
                notes,
                ppq,
                key_count,
                total_ticks,
                config,
                midi_hash,
                result_tx: context.texture_waterfall_result_tx,
                progress: &context.channels.waterfall_progress,
                renderer: context.texture_waterfall_renderer,
                meta: context.texture_waterfall_meta,
                renderer_config: context.texture_waterfall_config,
            });
        }
        WaterfallCommand::Dispose => {
            handle_waterfall_dispose(
                context.texture_waterfall_renderer,
                context.texture_waterfall_meta,
                context.texture_waterfall_config,
                &context.channels.waterfall_progress,
            );
        }
        WaterfallCommand::RegenerateTrack(params) => {
            handle_regenerate_waterfall_track(
                params,
                &gpu,
                context.texture_waterfall_result_tx,
                context.texture_waterfall_renderer,
                context.texture_waterfall_meta,
                context.texture_waterfall_config,
            );
        }
        WaterfallCommand::ShowDirtyOverlay(params) => {
            handle_waterfall_dirty_overlay(
                params,
                &gpu,
                context.texture_waterfall_renderer,
                context.texture_waterfall_meta,
                context.texture_waterfall_config,
                &context.channels.waterfall_progress,
            );
        }
        WaterfallCommand::UploadVideoTiles {
            tiles,
            config,
            track_count,
            key_count,
            total_ticks,
            ppq,
        } => {
            upload_waterfall_video_tiles(
                &gpu,
                context.texture_waterfall_renderer,
                context.texture_waterfall_meta,
                context.texture_waterfall_config,
                WaterfallUploadTileParams {
                    tiles,
                    config,
                    track_count,
                    key_count,
                    total_ticks,
                    ppq,
                },
            );
        }
    }
}
