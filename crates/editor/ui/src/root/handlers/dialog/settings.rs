//! 设置对话框处理器

use crate::host::DialogResult;
use crate::message::{Message, SettingsDialogAction};
use crate::root::Root;

use super::DialogHandler;

impl DialogHandler {
    pub(super) fn handle_settings_dialog(
        &self,
        root: &mut Root,
        action: SettingsDialogAction,
    ) -> Option<Message> {
        match action {
            SettingsDialogAction::OpenDialog => {
                root.state.dialog_type = crate::state::root_state::DialogType::Settings;
                // 若当前为 WinMM 模式，打开设置面板时自动扫描播表（输出设备）
                if root.settings.synth.backend == lumino_core::storage::config::SynthBackend::System
                {
                    root.scan_winmm_outputs();
                }
            }
            SettingsDialogAction::CloseDialog => {
                // 返回设置结果，将设置同步到主窗口
                root.state.dialog_result = Some(DialogResult::Settings {
                    settings: Box::new(root.settings.clone()),
                    theme: root.window.theme.to_string(),
                });
            }
        }
        None
    }
}
