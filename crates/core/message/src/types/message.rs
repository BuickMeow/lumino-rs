//! 应用消息枚举定义
//!
//! Message 枚举是整个应用的消息中枢。拆分在此文件以保持 lib.rs 精简。
//! 通过 types.rs → lib.rs 的 pub use 链重新导出到 crate 根路径。

use crate::events::Event;

use crate::audio_export::AudioExportAction;
use crate::batch_edit::BatchEditAction;
use crate::brush_settings::BrushSettingsAction;
use crate::cloud_action::CloudAction;
use crate::collaboration::CollaborationAction;
use crate::custom_precision::CustomPrecisionAction;
use crate::load_confirm::LoadConfirmAction;
use crate::loop_range::LoopRangeAction;
use crate::project_settings::ProjectSettingsAction;
use crate::recover_track::RecoverTrackAction;
use crate::right_sidebar::RightSidebarAction;
use crate::save_confirm::SaveConfirmAction;
use crate::settings_dialog::SettingsDialogAction;
use crate::speed_change::SpeedChangeAction;
use crate::types::editor::EditorAction;
use crate::types::geometry::{Point2, Size2};
use crate::types::ui::PerfData;
use crate::velocity::VelocityAction;
use crate::video_clip::VideoClipAction;
use crate::video_export::VideoExportAction;

use crate::context_menu::PianoRollContextMenuAction;

/// 应用消息
///
/// 泛型参数：
/// - `W`: 窗口事件类型（由 lumino-ui 的 window::Event 实例化）
/// - `S`: 侧边栏事件类型（由 lumino-ui 的 sidebar::Event 实例化）
/// - `Se`: 设置事件类型（由 lumino-ui 的 settings::Event 实例化）
/// - `T`: 工具栏事件类型（由 lumino-ui 的 toolbar::Event 实例化）
// 体积：`window::Event::Dialog` Box 化后 `Core(Event)` 已显著缩小，
// 由 `test_message_size_bound` 守卫（≤200 bytes，clippy large_enum_variant 阈值）。
#[derive(Debug, Clone)]
pub enum Message<W, S, Se, T> {
    /// 核心事件
    Core(Event),
    /// 窗口事件
    Window(W),
    /// 侧边栏事件
    Sidebar(S),
    /// 进度更新（消息与进度值）
    Progress(Option<(String, f64)>),
    /// 横向滚动条滚动
    ScrollbarScrolled(f32),
    /// 纵向滚动条滚动
    ScrollbarScrolledY(f32),
    /// 工程走带水平滚动
    ArrangementScrollX(f32),
    /// 工程走带垂直滚动
    ArrangementScrollY(f32),
    /// 工程走带水平缩放
    ArrangementZoomX {
        /// 缩放倍率
        zoom: f32,
        /// 固定比例基准
        fixed_ratio: f32,
    },
    /// 工程走带垂直缩放
    ArrangementZoomY {
        /// 缩放倍率
        zoom: f32,
        /// 固定比例基准
        fixed_ratio: f32,
    },
    /// 横向缩放变化
    ZoomXChanged {
        /// 缩放倍率
        zoom: f32,
        /// 固定比例基准
        fixed_ratio: f32,
    },
    /// 纵向缩放变化
    ZoomYChanged {
        /// 缩放倍率
        zoom: f32,
        /// 固定比例基准
        fixed_ratio: f32,
    },
    /// Canvas 位置和尺寸更新
    CanvasBoundsChanged {
        /// Canvas 偏移
        offset: Point2,
        /// Canvas 尺寸
        size: Size2,
    },
    /// 菜单状态更新
    MenuStateChanged(bool),
    /// 编辑器动作
    EditorAction(EditorAction),
    /// 音频控制动作
    AudioAction(lumino_core::AudioAction),
    /// 设置面板事件
    Settings(Se),
    /// 切换设置面板显示状态
    ToggleSettings,
    /// 工具栏事件
    Toolbar(T),
    /// 自定义精度对话框动作
    CustomPrecision(CustomPrecisionAction),
    /// 画刷「绘制行为」对话框动作
    BrushSettings(BrushSettingsAction),
    /// 协作动作
    Collaboration(CollaborationAction),
    /// 加载确认对话框动作
    LoadConfirm(LoadConfirmAction),
    /// 保存确认对话框动作（关闭/打开/退出前的未保存更改确认）
    SaveConfirm(SaveConfirmAction),
    /// 工程设置对话框动作
    ProjectSettings(ProjectSettingsAction),
    /// 设置对话框动作
    SettingsDialog(SettingsDialogAction),
    /// 力度编辑面板动作
    Velocity(VelocityAction),
    /// 力度面板高度调整
    VelocityPanelResize(f32),
    /// 性能监控数据更新
    PerfUpdate(PerfData),
    /// 空消息标记
    Null,
    /// Ctrl 键状态变更
    CtrlKeyChanged(bool),
    /// Shift 键状态变更
    ShiftKeyChanged(bool),
    /// 模式切换（编辑器/瀑布流）
    ModeToggled,
    /// 动画帧更新（用于弹簧物理模拟）
    AnimationTick,
    /// 循环区域事件
    LoopRange(LoopRangeAction),
    /// MIDI 输入事件（从 MIDI 设备收到的原始数据）
    MidiInputEvent {
        /// 原始 MIDI 数据字节
        data: Vec<u8>,
    },
    /// 音频导出动作
    AudioExport(AudioExportAction),
    /// 音符变速动作
    SpeedChange(SpeedChangeAction),
    /// 批量编辑动作
    BatchEdit(BatchEditAction),
    /// 找回删除音轨对话框动作
    RecoverTrack(RecoverTrackAction),
    /// 视频导出动作
    VideoExport(VideoExportAction),
    /// 视频剪辑动作（瀑布流预览交互）
    VideoClip(VideoClipAction),
    /// 批量消息（用于 canvas 等一次事件需要发布多条消息的场景）
    Batch(Vec<Message<W, S, Se, T>>),
    /// 钢琴卷帘右键上下文菜单动作
    PianoRollContextMenu(PianoRollContextMenuAction),
    /// 右侧栏动作
    RightSidebar(RightSidebarAction),
    /// 云存储动作
    Cloud(CloudAction),
    /// 工程走带：设置演奏指示线位置
    ArrangementCursorSet(f64),
    /// 工程走带：选择矩形变更（tick_start, tick_end, track_lo, track_hi）
    ArrangementSelectionChanged(Option<(f64, f64, usize, usize)>),
    /// 工程走带：清空选择
    ArrangementSelectionCleared,
    /// 工程走带：移动选中的音符
    ArrangementMoveNotes {
        /// tick 偏移量
        delta_ticks: i64,
        /// 音轨偏移量
        delta_tracks: i32,
    },
    /// 工程走带：擦除矩形范围内的音符
    ArrangementErase {
        /// 起始 tick
        tick_start: f64,
        /// 结束 tick
        tick_end: f64,
        /// 起始音轨
        track_lo: usize,
        /// 结束音轨
        track_hi: usize,
    },
    /// 工程走带：在指定 tick/track 处分割音符
    ArrangementRazor {
        /// 分割位置 tick
        tick: f64,
        /// 分割音轨
        track: usize,
    },
    /// 工程走带：在指定音轨 tick 处添加音符
    ArrangementAddNote {
        /// 音轨索引
        track: usize,
        /// 起始 tick
        tick: f64,
        /// 音符持续时间（tick）
        duration: f64,
        /// 键位
        key: u8,
        /// 力度
        velocity: u8,
    },
    /// 工程走带：ghost 音符预览列表更新
    ArrangementGhostNotesUpdated(Vec<(f64, f64, usize)>),
    /// 工程走带：拖拽中的框选矩形（实时预览，由 GPU 渲染）
    ArrangementDragSelectionRect(Option<(f64, f64, usize, usize)>),
    /// 工程走带：复制选中音符到剪贴板
    ArrangementCopy,
    /// 工程走带：从剪贴板粘贴音符
    ArrangementPaste,
    /// 工程走带：剪切选中音符（复制 + 删除）
    ArrangementCut,
    /// 工程走带：删除选中音符
    ArrangementDeleteSelection,
}

/// 构造空消息
pub const fn null<W, S, Se, T>() -> Message<W, S, Se, T> {
    Message::Null
}

#[cfg(test)]
mod tests {
    use super::*;

    // ─── Message null helper ───

    #[test]
    fn test_null_message() {
        let msg: Message<(), (), (), ()> = null();
        assert!(matches!(msg, Message::Null));
    }

    /// 回归测试：Message 枚举体积守卫。
    ///
    /// 历史：`Core(Event)` variant 曾达 328 bytes（`window::Event::Dialog` 内
    /// `StartAudioExport`/`StartVideoExport` 携带大 config 结构体），高频 UI 消息
    /// 循环每次传递都整块拷贝。Box 化 config 字段后应显著缩小；若未来有人把
    /// 大字段直接塞回枚举，此测试立即报警。
    #[test]
    fn test_message_size_bound() {
        let size = std::mem::size_of::<Message<(), (), (), ()>>();
        assert!(
            size <= 200,
            "Message 枚举体积回退：当前 {size} bytes（Box 化大 config 后应 ≤ 200）"
        );
    }

    // ─── PianoRollContextMenu ───

    #[test]
    fn test_piano_roll_context_menu_message() {
        let msg: Message<(), (), (), ()> = Message::PianoRollContextMenu(
            crate::context_menu::PianoRollContextMenuAction::ItemClicked(
                crate::context_menu::PianoRollContextMenuItem::Copy,
            ),
        );
        assert!(matches!(
            msg,
            Message::PianoRollContextMenu(
                crate::context_menu::PianoRollContextMenuAction::ItemClicked(
                    crate::context_menu::PianoRollContextMenuItem::Copy
                )
            )
        ));
    }
}
