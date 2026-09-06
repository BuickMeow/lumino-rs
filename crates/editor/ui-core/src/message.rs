//! 应用消息 — 重新导出自 lumino-message
//!
//! 将泛型 Message<W, S, Se, T> 实例化为具体的 UI 事件类型，
//! 将子系统事件类型绑定为确定类型参数。

// 重新导出 UI 特有的事件类型（这些不能放入 lumino-message）
pub use crate::{
    settings_event::Event as Settings, sidebar_event::Event as Sidebar,
    toolbar_event::Event as Toolbar, window_event::Event as Window,
};

// 重新导出自 lumino-message 的所有公共类型
pub use lumino_message::{
    AudioAction, AudioChannels, AudioExportAction, AudioFormat, BatchEditAction, BatchEditField,
    BrushSettingsAction, CcOption, CustomPrecisionAction, DotType, EditorAction, Interpolation,
    LoadConfirmAction, LoopRangeAction, Message as GenericMessage, NotePrecision, PerfData, Point2,
    ProjectSettingsAction, RecoverTrackAction, RightSidebarAction, SaveConfirmAction,
    SettingsDialogAction, Size2, SpeedChangeAction, SpeedFactor, ThreadingOption, Tool, TupletType,
    VelocityAction, VideoClipAction, VideoExportAction,
};

/// 具体化的消息类型
///
/// 将泛型 Message 绑定到 UI crate 的具体事件类型。
pub type Message = GenericMessage<Window, Sidebar, Settings, Toolbar>;

/// 创建空消息
///
/// 委托至 lumino-message 的泛型 null()，由类型别名确定具体类型参数。
pub const fn null() -> Message {
    lumino_message::null::<Window, Sidebar, Settings, Toolbar>()
}
