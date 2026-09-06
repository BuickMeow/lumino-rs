use lumino_midiplayer::texture_waterfall::{
    WaterfallGpuCtx, WaterfallViewportParams, update_waterfall_viewport,
};

use crate::RenderParams;

use super::super::prepare::prepare_renderers;
use super::super::render_pass::execute_render_pass;
use super::super::textures::{OffscreenTextureResources, ensure_textures};
use super::context::{PreviewPassContext, PreviewUploadContext, RenderFrameState};

/// 确保离屏纹理已创建。
///
/// 统一全量渲染（2026-08-06）：主音轨不再走「可见列表 + SwappableBuffer 版本
/// 驱动上传」——GPU buffer 常驻所有轨全部音符（洋葱皮全量会话），主音轨由
/// ViewState uniform 着色、滚动/切轨零重传。故本函数仅保留纹理确保。
pub(super) fn ensure_offscreen_textures_and_upload_notes(context: &mut PreviewUploadContext<'_>) {
    let width = context.params.viewport_size.0.max(1);
    let height = context.params.viewport_size.1.max(1);

    let mut tex_resources = OffscreenTextureResources {
        device: &context.ctx.device,
        texture_format: context.ctx.texture_format,
        width,
        height,
        current_size: &mut *context.current_size,
        current_texture: &mut *context.current_texture,
        depth_texture: &mut *context.depth_texture,
        depth_texture_view: &mut *context.depth_texture_view,
        texture_view: &mut *context.texture_view,
        latest_texture_clone: &context.channels.latest_texture_clone,
        params: context.params,
    };
    // 普通离屏渲染保留 depth attachment（UI 预览可能需要）
    ensure_textures(&mut tex_resources, true);
}

/// 将宿主 `RenderParams` 转换为贴图瀑布流视口参数（仅提取所需字段子集）
fn waterfall_viewport_params(params: &RenderParams) -> WaterfallViewportParams {
    WaterfallViewportParams {
        viewport_size: params.viewport_size,
        scale_factor: params.scale_factor,
        scroll: params.scroll,
        zoom: params.zoom,
        keyboard_width: params.keyboard_width,
        ruler_height: params.ruler_height,
        canvas_offset: params.canvas_offset,
        canvas_size: params.canvas_size,
        is_arrangement_mode: params.is_arrangement_mode,
    }
}

/// 执行离屏渲染通道（在离屏纹理就绪时）：准备渲染器、更新贴图瀑布流视口、提交命令。
pub(super) fn render_offscreen_pass(context: &mut PreviewPassContext<'_>) {
    if let (Some(_texture), Some(_depth_view)) =
        (&*context.current_texture, &*context.depth_texture_view)
    {
        let mut encoder =
            context
                .ctx
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("offscreen_render_encoder"),
                });

        prepare_renderers(
            &mut *context.renderers,
            context.params,
            &context.ctx.device,
            &context.ctx.queue,
        );

        let gpu = WaterfallGpuCtx {
            device: &context.ctx.device,
            queue: &context.ctx.queue,
            texture_format: context.ctx.texture_format,
        };
        let waterfall_visible = update_waterfall_viewport(
            &mut *context.texture_waterfall_renderer,
            &*context.texture_waterfall_meta,
            &*context.texture_waterfall_config,
            &waterfall_viewport_params(context.params),
            &gpu,
        );
        let waterfall_visible_coords: Vec<crate::WaterfallTileCoord> =
            waterfall_visible.iter().map(|(c, _)| *c).collect();

        let mut frame = RenderFrameState {
            renderers: context.renderers,
            current_texture: context.current_texture,
            depth_texture: context.depth_texture,
            depth_texture_view: context.depth_texture_view,
            texture_view: context.texture_view,
            current_size: context.current_size,
            last_note_version: context.last_note_version,
            latest_texture_clone: &context.channels.latest_texture_clone,
            texture_waterfall_renderer: context.texture_waterfall_renderer,
            texture_waterfall_meta: context.texture_waterfall_meta,
            texture_waterfall_config: context.texture_waterfall_config,
            export_pipeline: context.export_pipeline,
            export_frame_tx: context.export_frame_tx,
            waterfall_renderer: &mut None,
            miditrail_renderer: &mut None,
            onion_source: None,
            onion_epoch: 0,
        };
        execute_render_pass(
            &mut encoder,
            context.ctx,
            context.params,
            &waterfall_visible_coords,
            true,
            &mut frame,
        );

        context.ctx.queue.submit(std::iter::once(encoder.finish()));
    }
}
