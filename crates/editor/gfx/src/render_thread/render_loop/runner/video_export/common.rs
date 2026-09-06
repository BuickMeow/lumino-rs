use crate::render_thread::commands::FrameSender;
use crate::render_thread::export_pipeline::ExportPipeline;
use crate::render_thread::render_loop::Renderers;
use crate::render_thread::render_loop::runner::context::{
    RenderFrameState, VideoExportFrameContext, VideoExportSetupContext,
};
use crate::{NoteInstance, unpack_key_color};

use super::handle_video_frame;

/// 启动视频导出：初始化 GPU→CPU 读回管线与专用 2D 渲染器。
pub(crate) fn start_video_export(context: VideoExportSetupContext<'_>) {
    tracing::info!(
        "视频导出开始: {}x{}, 初始化 GPU→CPU 读回管线",
        context.width,
        context.height
    );
    let mut pipeline = ExportPipeline::new(&context.ctx.device, context.width, context.height);
    pipeline.set_recycle_receiver(context.recycle_rx);
    *context.export_pipeline = Some(pipeline);
    *context.export_frame_tx = Some(context.frame_tx);
    // 创建无 depth attachment 的视频导出专用渲染器
    *context.export_renderers = Some(Renderers::new_for_video_export(
        &context.ctx.device,
        &context.ctx.queue,
        context.ctx.texture_format,
    ));
}

/// 渲染一帧视频导出：构造每帧状态并交给对应渲染模式。
pub(crate) fn render_video_frame_command(context: VideoExportFrameContext<'_>) {
    // 主缓冲直绑源：onion 流式上传完成后，主 onion 缓冲即全文档权威数据，
    // 钢琴模式直接绑定它，零上传。进行中则为 None，调用方走上传回退路径。
    // 瀑布流索引路径同样需要（全局桶建在常驻缓冲上）；legacy 瀑布流忽略本字段。
    let want_onion = !context.params.miditrail_enabled;
    let onion_source = if want_onion && !context.onion_streaming_in_progress {
        Some(context.renderers.onion_skin.gpu_note_buffer_for_sharing())
    } else {
        None
    };
    // 世代与 onion_source 同快照：先拷贝（u64 Copy），再可变借用 renderers。
    // （video_renderers 可能 alias 主 renderers，借用顺序不可颠倒。）
    let onion_epoch = context.renderers.onion_epoch;
    // 视频导出帧使用纯 2D 渲染器，确保 pipeline 与无 depth 的 RenderPass 兼容。
    let video_renderers = context
        .export_renderers
        .as_mut()
        .unwrap_or(&mut *context.renderers);
    let mut frame = RenderFrameState {
        renderers: video_renderers,
        onion_source,
        // 与 onion_source 同快照的世代计数（全局桶重建判定用）。
        onion_epoch,
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
        waterfall_renderer: context.waterfall_renderer,
        miditrail_renderer: context.miditrail_renderer,
    };
    handle_video_frame(context.ctx, context.params, &mut frame, context.channels);
}

/// 从权威 `note_instances` 换算瀑布流分桶偏移表（派生索引，可忽略）。
///
/// 调用方须保证输入已按 (key, start) 有序（`sort_visible_notes` 语义），
/// 输出 `offsets[k]` = 第一个 `key >= k` 的音符索引，满足 shader 桶内二分前提。
/// 音符本体零拷贝——shader 直接读共享缓冲，本表是唯一的每帧派生数据。
pub(crate) fn note_instances_to_key_offsets(notes: &[NoteInstance], key_count: usize) -> Vec<u32> {
    let key_count = key_count.max(1);
    let mut counts = vec![0u32; key_count];
    for n in notes {
        let (key, _) = unpack_key_color(n.key_color);
        // 不变式：瀑布流生产侧已按 key_count 过滤（见 render_params/waterfall.rs），
        // 共享缓冲内不应出现越界 key，否则分桶边界错位。
        debug_assert!(
            (key as usize) < key_count,
            "共享缓冲含越界 key {key}（key_count={key_count}）"
        );
        if (key as usize) < key_count {
            counts[key as usize] += 1;
        }
    }
    let mut offsets = vec![0u32; key_count + 1];
    for k in 0..key_count {
        offsets[k + 1] = offsets[k] + counts[k];
    }
    offsets
}

/// 导出管线收尾：尺寸对齐 → 背压读最早帧 → copy 离屏纹理到 staging 并提交 → 非阻塞读回。
///
/// 瀑布流 / 3D 两个 compute/3D 处理器共用，差异仅在日志标签。
/// 调用方须保证 `encoder` 内已写入目标纹理的渲染命令。
pub(crate) fn copy_texture_and_drain(input: CopyDrainInput<'_>) {
    let CopyDrainInput {
        pipeline,
        tx,
        encoder,
        texture,
        queue,
        width,
        height,
        label,
    } = input;
    pipeline.ensure_size(width, height);
    while !pipeline.can_write() {
        let data = pipeline.wait_read();
        if tx.0.send(data).is_err() {
            tracing::warn!("{label}帧发送失败：Runner 通道已关闭");
            return;
        }
    }
    pipeline.copy_and_submit(encoder, texture, queue);
    while let Some(data) = pipeline.try_read() {
        if tx.0.send(data).is_err() {
            tracing::warn!("{label}帧发送失败：Runner 通道已关闭");
            return;
        }
    }
}

/// `copy_texture_and_drain` 入参（替代 8 个位置参数，消除 `too_many_arguments`）
pub(crate) struct CopyDrainInput<'a> {
    pub pipeline: &'a mut ExportPipeline,
    pub tx: &'a FrameSender,
    pub encoder: wgpu::CommandEncoder,
    pub texture: &'a wgpu::Texture,
    pub queue: &'a wgpu::Queue,
    pub width: u32,
    pub height: u32,
    pub label: &'static str,
}
