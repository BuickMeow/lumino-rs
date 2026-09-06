//! 工具栏工具选择与音符编辑操作
//!
//! 包括工具选择、精度同步、撤销/重做、量化、变速、翻转、移调、分割/合并等。
//!
//! 子模块组织（保持本文件 < 400 行）：
//! - `note_ops`: 音符编辑操作（量化/变速/翻转/移调/连奏/分割合并）

use super::ToolbarHandler;
use crate::root::Root;

mod note_ops;

impl ToolbarHandler {
    /// 同步工具状态到编辑器
    pub(crate) fn sync_toolbar_tool_state(&self, root: &mut Root, event: &crate::toolbar::Event) {
        if let crate::toolbar::Event::ToolSelected(tool) = event {
            root.editor.set_tool(*tool);
        }
        // 面板内选择工具：把工具栏已更新的 current_tool / fill_enabled 镜像到编辑器。
        // 注意：`view_arrangement` 每帧会用 `editor.current_tool()` 反向覆盖
        // `toolbar.current_tool`，若此处不同步到编辑器，面板选择就会被瞬间覆盖、
        // 表现为「点了工具却不切换」。这是与上一条 `ToolSelected` 完全一致的处理路径。
        if let crate::toolbar::Event::ToolPanelItemSelected(item) = event {
            match item {
                crate::toolbar::ToolPanelItem::StrokeSettings => {}
                _ => {
                    root.editor.set_tool(root.toolbar.current_tool);
                    root.editor.set_fill_enabled(root.toolbar.fill_enabled);
                }
            }
        }
        // 颜料桶填充模式开关（仅曲线工具激活时可操作，非 Curve 时按钮禁用）
        if let crate::toolbar::Event::FillToggled(enabled) = event {
            root.editor.set_fill_enabled(*enabled);
            tracing::info!("Root: 颜料桶填充模式切换为 {}", enabled);
        }
        // 形状工具类型切换（矩形/圆/三角）：把工具栏 current_shape 同步到编辑器
        if let crate::toolbar::Event::ShapeTypeSelected(shape) = event {
            let kind = match shape {
                crate::toolbar::ShapeType::Rectangle => lumino_editor_state::ShapeKind::Rectangle,
                crate::toolbar::ShapeType::Circle => lumino_editor_state::ShapeKind::Circle,
                crate::toolbar::ShapeType::Triangle => lumino_editor_state::ShapeKind::Triangle,
            };
            root.editor.set_shape(kind);
            tracing::debug!("Root: 形状工具类型切换为 {:?}", kind);
        }
    }

    /// 同步精度设置到编辑器
    pub(crate) fn sync_toolbar_precision(&self, root: &mut Root, event: &crate::toolbar::Event) {
        if let crate::toolbar::Event::PrecisionChanged(precision) = event {
            let ticks = (*precision).as_ticks(root.editor.editor_state.view.ppq);
            root.editor.set_snap_precision(ticks);
            root.editor.set_default_note_length(ticks);
            tracing::debug!(
                "Root: 音符精度同步为 {} ticks (PPQ={})",
                ticks,
                root.editor.editor_state.view.ppq
            );
        }
    }

    /// 同步自动滚动模式到编辑器
    pub(crate) fn sync_auto_scroll_mode(&self, root: &mut Root, event: &crate::toolbar::Event) {
        if matches!(event, crate::toolbar::Event::AutoScrollModeChanged) {
            // 同步自动滚动模式到 editor（toolbar 已经切换了模式，这里同步到 editor）
            root.editor
                .set_auto_scroll_config(lumino_core::storage::config::AutoScrollConfig {
                    mode: root.toolbar.auto_scroll_mode,
                    ..root.editor.editor_state.auto_scroll
                });
            tracing::debug!(
                "Root: 自动滚动模式同步为 {:?}",
                root.toolbar.auto_scroll_mode
            );
        }
    }

    /// 处理撤销/重做
    pub(crate) fn handle_toolbar_undo_redo(&self, _root: &mut Root, event: &crate::toolbar::Event) {
        if matches!(event, crate::toolbar::Event::Undo) {
            tracing::info!("Root: 触发撤销操作");
            crate::event::emit(crate::event::Event::Menu(crate::event::menu::Event::Edit(
                crate::event::menu::edit::Event::Undo,
            )));
        }
        if matches!(event, crate::toolbar::Event::Redo) {
            tracing::info!("Root: 触发重做操作");
            crate::event::emit(crate::event::Event::Menu(crate::event::menu::Event::Edit(
                crate::event::menu::edit::Event::Redo,
            )));
        }
    }

    /// 处理协作对话框
    pub(crate) fn handle_toolbar_collaboration(
        &self,
        _root: &mut Root,
        event: &crate::toolbar::Event,
    ) {
        if matches!(event, crate::toolbar::Event::OpenCollaborationDialog) {
            tracing::info!("Root: 触发打开协作对话框");
            crate::event::emit(crate::event::Event::Window(
                crate::event::window::Event::open_collaboration_dialog(),
            ));
        }
    }

    /// 处理内存监控对话框
    pub(crate) fn handle_toolbar_memory_monitor(
        &self,
        _root: &mut Root,
        event: &crate::toolbar::Event,
    ) {
        if matches!(event, crate::toolbar::Event::OpenMemoryMonitorDialog) {
            tracing::info!("Root: 触发打开内存监控对话框");
            crate::event::emit(crate::event::Event::Window(
                crate::event::window::Event::open_memory_monitor_dialog(),
            ));
        }
    }
}
