//! NoteRectangle 模式：传统钢琴卷帘音符矩形

use lumino_gfx::RenderParams;

use super::{
    NoteRectangleParamsInput, NoteRectangleRenderInput, SortableNote,
    build_note_rectangle_params_from_visible,
};

/// NoteRectangle 模式：传统 GPU 音符矩形渲染
pub(crate) fn build_note_rectangle_render_params(input: NoteRectangleRenderInput) -> RenderParams {
    let NoteRectangleRenderInput {
        width,
        height,
        tick,
        document,
        ppq,
        visible_notes,
        note_instances_out,
        collect_all,
    } = input;
    // 首帧全量收集（一次上传常驻 GPU）；后续帧跳过收集，裁剪走 GPU cull。
    // 注：视口缩放由共享函数按 tick 重算，此处无需窗口。
    visible_notes.clear();
    note_instances_out.clear();
    if collect_all {
        for (track_idx, track_notes) in document.notes.iter().enumerate() {
            for n in track_notes.iter() {
                visible_notes.push(SortableNote {
                    key: n.key,
                    start_tick: n.start_tick,
                    length: n.length(),
                    track_idx: track_idx as u16,
                });
            }
        }
    }

    // 排序分桶 + NoteInstance 构建 + RenderParams 组装（与流式模式共享）
    build_note_rectangle_params_from_visible(NoteRectangleParamsInput {
        width,
        height,
        tick,
        visible_notes,
        note_instances_out,
        ppq,
        time_signatures: &document.time_signatures,
    })
}
