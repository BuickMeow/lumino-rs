//! lumino-message — 消息与共享类型定义
//!
//! 本 crate 定义了整个 lumino 应用的消息传递系统和跨模块共享类型。
//! Message 枚举定义在 `types::message` 子模块中，通过 pub use 链重新导出。
//! Message 是泛型的，由上层 crate（lumino-ui）实例化具体的 UI 事件类型。

pub mod audio_export;
pub mod batch_edit;
pub mod brush_settings;
pub mod cloud_action;
pub mod collaboration;
pub mod context_menu;
pub mod custom_precision;
/// 事件系统模块
pub mod events;
pub mod load_confirm;
pub mod loop_range;
pub mod project_settings;
pub mod recover_track;
pub mod right_sidebar;
pub mod save_confirm;
pub mod settings_dialog;
pub mod speed_change;
pub mod types;
pub mod velocity;
pub mod video_clip;
pub mod video_export;

pub use audio_export::AudioExportAction;
pub use batch_edit::{BatchEditAction, BatchEditField};
pub use brush_settings::BrushSettingsAction;
pub use cloud_action::{CloudAction, CloudProtocolUi};
pub use collaboration::CollaborationAction;
pub use context_menu::{
    MaterialContextMenuItem, PanelContextMenuItem, PianoRollContextMenuAction,
    PianoRollContextMenuItem, TrackContextMenuItem,
};
pub use custom_precision::CustomPrecisionAction;
pub use load_confirm::LoadConfirmAction;
pub use loop_range::LoopRangeAction;
pub use project_settings::ProjectSettingsAction;
pub use recover_track::RecoverTrackAction;
pub use right_sidebar::{I2mConfigField, RightSidebarAction};
pub use save_confirm::SaveConfirmAction;
pub use settings_dialog::SettingsDialogAction;
pub use speed_change::SpeedChangeAction;
pub use types::*;
pub use velocity::VelocityAction;
pub use video_clip::VideoClipAction;
pub use video_export::VideoExportAction;

pub use lumino_core::{AudioAction, DotType, NotePrecision, Tool};
