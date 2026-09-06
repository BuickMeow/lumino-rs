use crate::EditState;
use crate::Editor;
use crate::note::Note;
use crate::tests::test_helpers;
use lumino_editor_state::DragState;

// ===== handle_pointer_pressed Ctrl + 选择框内部 → 复制拖拽 =====

/// 计算选中音符选择框的屏幕中心点（保证 Inside 命中）
fn selection_box_center(editor: &Editor) -> iced_core::Point {
    let (x1, x2, y1, y2) = editor
        .get_selection_box_bounds()
        .expect("有选中音符时应能计算选择框");
    iced_core::Point::new((x1 + x2) / 2.0, (y1 + y2) / 2.0)
}

#[test]
fn test_pointer_pressed_ctrl_inside_enters_copy_state() {
    let mut editor = Editor::new();
    test_helpers::seed_notes(&mut editor, 1, 0, &[Note::new(0.0, 60, 480.0)]);
    editor.selection_insert(0);
    editor.set_ctrl_pressed(true);

    // 点击选择框内部 → Ctrl 按下 → DraggingSelectionCopy
    let center = selection_box_center(&editor);
    editor.handle_tool_pressed(center, false, 0.0, 60);

    match editor.editor_state.interaction.edit_state {
        EditState::DraggingSelectionCopy { .. } => {}
        other => panic!("Ctrl 拖动应进入 DraggingSelectionCopy，实际 {:?}", other),
    }
}

#[test]
fn test_pointer_pressed_without_ctrl_enters_move_state() {
    let mut editor = Editor::new();
    test_helpers::seed_notes(&mut editor, 1, 0, &[Note::new(0.0, 60, 480.0)]);
    editor.selection_insert(0);
    editor.set_ctrl_pressed(false);

    // 点击选择框内部 → 未按 Ctrl → DraggingSelection（移动）
    let center = selection_box_center(&editor);
    editor.handle_tool_pressed(center, false, 0.0, 60);

    match editor.editor_state.interaction.edit_state {
        EditState::DraggingSelection { .. } => {}
        other => panic!("非 Ctrl 拖动应进入 DraggingSelection，实际 {:?}", other),
    }
}

// ===== 状态判定 / 自动提交 =====

#[test]
fn test_is_editing_with_pending_copy_returns_true() {
    let mut editor = Editor::new();
    test_helpers::seed_notes(&mut editor, 1, 0, &[Note::new(0.0, 60, 480.0)]);
    editor.pending_copy_drag_state = Some(DragState::from_single(0, 1, 0, 60));
    assert!(editor.is_editing(), "pending copy 应视为编辑状态");
    assert!(editor.has_pending_drag());
}

#[test]
fn test_commit_current_edit_commits_pending_copy() {
    let mut editor = Editor::new();
    test_helpers::seed_notes(&mut editor, 1, 0, &[Note::new(0.0, 60, 480.0)]);

    // 模拟：复制拖动松手后（pending_copy 存在），用户直接触发 Save/Play/Export
    let mut drag = DragState::from_indices([0], 1, 0, 60);
    drag.set_delta(100, 0);
    editor.pending_copy_drag_state = Some(drag);

    assert!(editor.commit_current_edit());
    // 副本已写入内存层
    assert_eq!(editor.editor_state.data.current_track_note_count(), 2);
    assert_eq!(
        editor
            .editor_state
            .data
            .get_note_view(1)
            .expect("note should exist")
            .tick,
        100.0
    );
    assert!(editor.pending_copy_drag_state.is_none());
}

#[test]
fn test_flush_pending_drag_commits_copy_on_empty_click() {
    let mut editor = Editor::new();
    test_helpers::seed_notes(&mut editor, 1, 0, &[Note::new(0.0, 60, 480.0)]);

    // 复制松手后 pending_copy 存在
    let mut drag = DragState::from_indices([0], 1, 0, 60);
    drag.set_delta(100, 0);
    editor.pending_copy_drag_state = Some(drag);

    // 模拟点击空白处（flush_pending_drag 由 handle_pointer_pressed 空白分支调用）
    // 直接调用私有路径不可达，改用公开入口：点击空白会先 flush 再开始新框选
    editor.handle_tool_pressed(iced_core::Point::new(9999.0, 9999.0), false, 9999.0, 0);

    // 副本已写入内存层（点击空白处退出框选状态 → 写入）
    assert_eq!(editor.editor_state.data.current_track_note_count(), 2);
    assert_eq!(
        editor
            .editor_state
            .data
            .get_note_view(1)
            .expect("note should exist")
            .tick,
        100.0
    );
    assert!(
        editor.pending_copy_drag_state.is_none(),
        "复制提交后 pending 应清空"
    );
}

// ===== 回归：窗口级 Ctrl 通道失效时，canvas 上报的 Ctrl 仍应触发复制 =====
// 复现用户 BUG：窗口级 CtrlKeyChanged 因焦点丢失未送达（editor.ctrl_pressed()
// 为 false），但 canvas 本地 ModifiersChanged 可靠检测到 Ctrl。修复前复制判定
// 仅依赖窗口通道，会漏判成「移动」，副本既不入内存也不显示；修复后 canvas 的
// ctrl 随 Pressed 消息进入 handle_action 入口的双通道 OR，复制拖拽正常触发。

#[test]
fn test_pointer_pressed_ctrl_via_canvas_message_triggers_copy_when_window_channel_dead() {
    let mut editor = Editor::new();
    editor.editor_state.canvas.size_x = 2000.0;
    editor.editor_state.canvas.size_y = 4000.0;
    test_helpers::seed_notes(&mut editor, 1, 0, &[Note::new(0.0, 60, 480.0)]);
    editor.selection_insert(0);
    // 关键：不调用 set_ctrl_pressed(true)，模拟窗口级 Ctrl 通道失效
    assert!(
        !editor.ctrl_pressed(),
        "前置：窗口级 ctrl 通道应处于失效状态"
    );

    let center = selection_box_center(&editor);
    // 通过 EditorAction::Pressed 携带 canvas 可靠检测到的 ctrl（窗口通道为 false）
    editor.handle_action(lumino_ui_core::message::EditorAction::Pressed {
        pos: lumino_ui_core::message::Point2::new(center.x, center.y),
        shift: false,
        ctrl: true,
    });

    match editor.editor_state.interaction.edit_state {
        EditState::DraggingSelectionCopy { .. } => {}
        other => panic!(
            "canvas 上报 Ctrl 时应进入 DraggingSelectionCopy（复制），实际 {:?}",
            other
        ),
    }
}

// 端到端回归：经真实 handle_action(Pressed/Moved/Released) 全链路，
// 断言副本「写入内存（数量 1→2）且可见（显示）」，对齐用户「复制 + 写入内存 + 显示」三要素。
#[test]
fn test_ctrl_drag_copy_full_flow_via_handle_action_writes_to_memory_and_display() {
    let mut editor = Editor::new();
    editor.editor_state.canvas.size_x = 2000.0;
    editor.editor_state.canvas.size_y = 4000.0;
    test_helpers::seed_notes(&mut editor, 1, 0, &[Note::new(0.0, 60, 480.0)]);
    editor.editor_state.view.set_snap_precision(10.0);
    editor.selection_insert(0);

    // 模拟 canvas 把 ctrl（可靠通道）随 Pressed 消息上报，窗口通道失效
    let center = selection_box_center(&editor);
    editor.handle_action(lumino_ui_core::message::EditorAction::Pressed {
        pos: lumino_ui_core::message::Point2::new(center.x, center.y),
        shift: false,
        ctrl: true,
    });
    assert!(
        matches!(
            editor.editor_state.interaction.edit_state,
            EditState::DraggingSelectionCopy { .. }
        ),
        "应进入复制拖拽状态"
    );

    // 向右移动：center 对应 tick 240，移向 tick 290（+50）
    let moved_x = editor.editor_state.view.tick_to_x(290.0);
    editor.handle_action(lumino_ui_core::message::EditorAction::Moved(
        lumino_ui_core::message::Point2::new(moved_x, center.y),
    ));
    editor.handle_action(lumino_ui_core::message::EditorAction::Released);

    // 副本已写入内存（从 1 → 2）
    assert_eq!(
        editor.editor_state.data.current_track_note_count(),
        2,
        "副本应写入内存层"
    );
    // 副本同时可见（渲染数据层包含原件 + 副本）
    let mut visible: Vec<(f32, u16, f32)> = Vec::new();
    editor.collect_visible_note_data(&mut visible, None, 0.0);
    assert_eq!(visible.len(), 2, "副本应同时可见（写入显示）");
}
