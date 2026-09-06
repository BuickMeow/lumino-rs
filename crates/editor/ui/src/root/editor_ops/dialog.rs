//! 编辑器操作 - 对话框管理
//!
//! 该模块已按职责拆分为以下子模块：
//! - `dialog_toggles`: 对话框打开/关闭状态切换
//! - `settings`: 设置面板配置同步
//! - `project_settings`: 工程设置对话框数据管理
//! - `export`: 导出对话框和加载确认对话框管理
//! - `operations`: 音符变速、批量编辑和自定义精度操作

use crate::root::Root;

pub mod dialog_toggles;
pub mod export;
pub mod operations;
pub mod project_settings;
pub mod save_confirm;
pub mod settings;

pub use project_settings::ProjectSettingsDialogData;

impl Root {
    /// 设置菜单打开状态（菜单打开时不渲染预览音符）
    pub fn set_menu_open(&mut self, open: bool) {
        self.state.is_menu_open = open;
    }

    /// 获取当前是否应该渲染预览音符
    pub fn should_render_preview_note(&self) -> bool {
        !self.state.is_menu_open && !self.is_progress_window
    }

    /// 更新编辑器鼠标位置
    pub fn update_editor_cursor(&mut self, position: Option<iced_core::Point>) {
        self.editor.update_cursor_position(position);
    }

    /// 更新编辑器 Canvas 偏移量
    pub fn set_editor_canvas_offset(&mut self, offset: iced_core::Point) {
        self.editor.set_canvas_offset(offset);
    }

    /// 获取并清空对话框结果
    pub fn take_dialog_result(&mut self) -> Option<crate::host::DialogResult> {
        self.state.dialog_result.take()
    }
}

#[cfg(test)]
mod dialog_tests;
