//! 瀑布流模式参数（产出统一 `note_instances` + 瀑布流 uniforms）
//!
//! 单一权威飞行格式：首帧全量收集（`collect_all`，无窗口过滤）并打包为
//! `NoteInstance`——渲染侧一次上传导出常驻，全局桶建其上；后续帧跳过收集
//! （`note_instances` 为空）只发 uniforms，窗口过滤走 GPU cull
//!（与旧窗口收集同谓词、同序，像素等价 harness 保证）。legacy 回退路径
//!（cull 不可用）消费首帧全量（已排序，回退正确）。

use lumino_gfx::RenderParams;

use super::{WaterfallRenderInput, collect_all_notes, pack_note_instances};

/// 瀑布流模式参数
pub(crate) fn build_waterfall_render_params(input: WaterfallRenderInput) -> RenderParams {
    let WaterfallRenderInput {
        width,
        height,
        tick,
        document,
        ppq,
        key_count,
        waterfall_scroll_speed,
        visible_notes,
        note_instances_out,
        window_state,
        collect_all,
    } = input;
    let waterfall_width = width.max(1) as f32;
    let waterfall_height = height.max(1) as f32;

    if collect_all {
        // 首帧全量：无窗口过滤（cull 在 GPU 侧做），排序 + 打包与窗口路径同函数。
        let t_collect = std::time::Instant::now();
        collect_all_notes(document, key_count, visible_notes);
        let collect_us = t_collect.elapsed().as_micros() as u64;

        let t_sort = std::time::Instant::now();
        super::sort_visible_notes(visible_notes, &mut window_state.sort_scratch);
        let sort_us = t_sort.elapsed().as_micros() as u64;
        // 边框仅钢琴卷帘矩形管线使用，瀑布流换算忽略，填 0。
        let t_pack = std::time::Instant::now();
        pack_note_instances(visible_notes, 0, note_instances_out);
        let pack_us = t_pack.elapsed().as_micros() as u64;
        super::diag_window_collect(
            "waterfall-full",
            collect_us,
            sort_us,
            pack_us,
            visible_notes.len(),
        );
    } else {
        // 稳态帧：跳过收集/排序/打包（渲染侧复用 GPU 常驻 + cull），只发 uniforms。
        visible_notes.clear();
        note_instances_out.clear();
    }

    RenderParams {
        viewport_size: (width.max(1), height.max(1)),
        logical_size: (waterfall_width, waterfall_height),
        scale_factor: 1.0,
        ppq: ppq as f32,
        max_key_index: (key_count.saturating_sub(1)) as f32,
        canvas_size: (waterfall_width, waterfall_height),
        is_waterfall_mode: true,
        waterfall_speed: waterfall_scroll_speed.max(0.1),
        waterfall_current_tick: tick,
        note_instances: std::mem::take(note_instances_out),
        time_signatures: document.time_signatures.clone(),
        ..Default::default()
    }
}
