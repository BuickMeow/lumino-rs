//! Root 组件状态 — 定义与子模块 re-export（子模块已迁至 lumino-ui-core）

// 保持路径不变：crate::state::root_state::Xxx 通过 core 重导出
pub use lumino_ui_core::state::{
    AudioExportDialogState, BatchEditDialogState, COUNTER_DEFAULT_CSV_FORMAT, COUNTER_DEFAULT_TEXT,
    COUNTER_FULL_TEXT, CollaborationDialogState, CollaborationViewState,
    CustomPrecisionDialogState, ExportProgressDialogState, LoadConfirmDialogState,
    MIDITRAIL_SPEED_DEFAULT, MIDITRAIL_VIEW_MODE_DEFAULT, MIDITRAIL_Z_FAR_DEFAULT,
    MIDITRAIL_Z_FAR_MAX, MemoryMonitorDialogState, ProjectSettingsDialogState,
    RecoverTrackDialogState, RecoverTrackEntry, SaveConfirmDialogState, SpeedChangeDialogState,
    ToggleAnimationState, VideoClipState, VideoExportDialogState, VideoExportOverlayState,
};

use crate::app_mode::AppMode;
use lumino_core::BrushConfig;

/// 对话框类型
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DialogType {
    /// 无对话框
    #[default]
    None,
    /// 自定义精度对话框
    CustomPrecision,
    /// 协作对话框
    Collaboration,
    /// 加载确认对话框
    LoadConfirm,
    /// 工程设置对话框
    ProjectSettings,
    /// 设置面板
    Settings,
    /// 音符变速对话框
    SpeedChange,
    /// 批量编辑对话框
    BatchEdit,
    /// 导出进度对话框
    ExportProgress,
    /// 视频导出对话框
    VideoExport,
    /// 内存监控对话框
    MemoryMonitor,
    /// 找回删除音轨
    RecoverTrack,
    /// 保存确认对话框（关闭工程 / 打开另一个工程 / 退出前的未保存更改确认）
    SaveConfirm,
    /// 云存储连接面板（地址/协议/用户名/密码）
    CloudConnect,
    /// 云文件浏览面板（仿资源管理器）
    CloudBrowser,
    /// 云存储断连/失败提醒面板（每次会话只弹一次）
    CloudNotice,
    /// 画刷「绘制行为」对话框（独立 OS 窗口）
    BrushSettings,
}

/// Root 组件的状态
pub struct RootState {
    /// 是否有菜单/下拉框打开（打开时不渲染预览音符）
    pub is_menu_open: bool,
    /// 对话框结果（用于独立窗口模式）
    pub dialog_result: Option<crate::host::DialogResult>,
    /// 是否是对话框窗口（用于自定义精度对话框等）
    pub is_dialog_window: bool,
    /// 对话框类型
    pub dialog_type: DialogType,
    /// 自定义精度对话框状态
    pub custom_precision_dialog: CustomPrecisionDialogState,
    /// 加载确认对话框状态
    pub load_confirm_dialog: LoadConfirmDialogState,
    /// 协作对话框状态
    pub collaboration_dialog: CollaborationDialogState,
    /// 工程设置对话框状态
    pub project_settings_dialog: ProjectSettingsDialogState,
    /// 音频导出对话框状态
    pub audio_export_dialog: AudioExportDialogState,
    /// 视频导出对话框状态
    pub video_export_dialog: VideoExportDialogState,
    /// 音频导出进度对话框状态
    pub export_progress_dialog: ExportProgressDialogState,
    /// 音符变速对话框状态
    pub speed_change_dialog: SpeedChangeDialogState,
    /// 批量编辑对话框状态
    pub batch_edit_dialog: BatchEditDialogState,
    /// 内存监控对话框状态
    pub memory_monitor_dialog: MemoryMonitorDialogState,
    /// 找回删除音轨对话框状态
    pub recover_track_dialog: RecoverTrackDialogState,
    /// 保存确认对话框状态
    pub save_confirm_dialog: SaveConfirmDialogState,
    /// 视频剪辑面板状态（瀑布流预览 zoom/pan）
    pub video_clip: VideoClipState,
    /// 当前应用模式（编辑器/瀑布流）
    pub current_mode: AppMode,
    /// 模式切换按钮动画状态
    pub toggle_animation: ToggleAnimationState,
    /// 画刷「绘制行为」对话框本地草稿（独立窗口编辑，Save 前不影响实际配置）
    pub brush_settings_draft: BrushConfig,
    /// 画刷「绘制行为」对话框可选音轨列表（仅普通音轨，排除指挥轨），(track_id, name)
    pub brush_settings_tracks: Vec<(usize, String)>,
}

impl Default for RootState {
    fn default() -> Self {
        Self::new()
    }
}

impl RootState {
    /// 创建一个默认的根组件状态
    pub fn new() -> Self {
        puffin::profile_scope!("root_state_new");
        Self {
            is_menu_open: false,
            dialog_result: None,
            is_dialog_window: false,
            dialog_type: DialogType::None,
            custom_precision_dialog: CustomPrecisionDialogState::new(),
            load_confirm_dialog: LoadConfirmDialogState::default(),
            collaboration_dialog: CollaborationDialogState::new(),
            project_settings_dialog: ProjectSettingsDialogState::new(),
            audio_export_dialog: AudioExportDialogState::new(),
            video_export_dialog: VideoExportDialogState::new(),
            export_progress_dialog: ExportProgressDialogState::new(),
            speed_change_dialog: SpeedChangeDialogState::new(),
            batch_edit_dialog: BatchEditDialogState::new(),
            memory_monitor_dialog: MemoryMonitorDialogState::new(),
            recover_track_dialog: RecoverTrackDialogState::default(),
            save_confirm_dialog: SaveConfirmDialogState::default(),
            video_clip: VideoClipState::new(),
            current_mode: AppMode::default(),
            toggle_animation: ToggleAnimationState::new(),
            brush_settings_draft: BrushConfig::new(),
            brush_settings_tracks: Vec::new(),
        }
    }
}

/// 音频导出 UI 类型 re-export（保持兼容）
pub use lumino_message::{
    AudioBackend, AudioChannels, AudioFormat, Interpolation, ThreadingOption,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_speed_change_dialog_default() {
        let state = SpeedChangeDialogState::new();
        assert!(!state.is_open);
        assert_eq!(state.factor_input, "0.5");
    }

    #[test]
    fn test_speed_change_parse_factor_decimal() {
        let mut state = SpeedChangeDialogState::new();
        state.factor_input = "2.0".to_string();
        assert!(
            (state
                .parse_factor()
                .expect("解析 \"2.0\" 应为有效小数，返回 Some")
                - 2.0)
                .abs()
                < 0.001
        );

        state.factor_input = "0.25".to_string();
        assert!(
            (state
                .parse_factor()
                .expect("解析 \"0.25\" 应为有效小数，返回 Some")
                - 0.25)
                .abs()
                < 0.001
        );
    }

    #[test]
    fn test_speed_change_parse_factor_fraction() {
        let mut state = SpeedChangeDialogState::new();
        state.factor_input = "1/3".to_string();
        assert!(
            (state
                .parse_factor()
                .expect("解析 \"1/3\" 应为有效分数，返回 Some")
                - 1.0 / 3.0)
                .abs()
                < 0.001
        );

        state.factor_input = "3/2".to_string();
        assert!(
            (state
                .parse_factor()
                .expect("解析 \"3/2\" 应为有效分数，返回 Some")
                - 1.5)
                .abs()
                < 0.001
        );
    }

    #[test]
    fn test_speed_change_parse_factor_invalid() {
        let mut state = SpeedChangeDialogState::new();
        state.factor_input = "".to_string();
        assert!(state.parse_factor().is_none());

        state.factor_input = "-1.0".to_string();
        assert!(state.parse_factor().is_none());

        state.factor_input = "abc".to_string();
        assert!(state.parse_factor().is_none());

        state.factor_input = "1/0".to_string();
        assert!(state.parse_factor().is_none());
    }

    #[test]
    fn test_toggle_animation_default() {
        let anim = ToggleAnimationState::default();
        assert!(!anim.active);
        assert!((anim.position - 0.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_toggle_animation_animate_to() {
        let mut anim = ToggleAnimationState::new();
        anim.animate_to(1.0);
        assert!(anim.active);
        assert!((anim.target - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_custom_precision_dialog_default() {
        let state = CustomPrecisionDialogState::new();
        assert!(!state.is_open);
        assert_eq!(state.divisor, "2");
        assert_eq!(state.note_value, "4");
        assert_eq!(state.tuplet_count, "3");
    }

    #[test]
    fn test_project_settings_dialog_default() {
        let state = ProjectSettingsDialogState::new();
        assert!(!state.is_open);
    }

    #[test]
    fn test_collaboration_dialog_default() {
        let state = CollaborationDialogState::new();
        assert!(!state.is_open);
    }

    #[test]
    fn test_root_state_default() {
        let state = RootState::new();
        assert!(!state.is_menu_open);
        assert!(!state.is_dialog_window);
        assert_eq!(state.dialog_type, DialogType::None);
    }
}
