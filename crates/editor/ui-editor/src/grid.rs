//! 钢琴卷帘网格绘制模块
//!
//! 该模块已拆分为以下子模块：
//! - `state`: Canvas状态管理
//! - `theme`: 主题颜色工具
//! - `utils`: 工具函数（is_key_dark, parse_color）
//! - `keyboard`: 钢琴键盘绘制
//! - `keys`: 琴键分隔线绘制
//! - `ruler`: 时间轴标尺绘制
//! - `bars`: 小节线/网格线绘制
//! - `remote_cursors`: 远程光标渲染
//! - `remote_selection`: 远程选择高亮渲染
//! - `selection_box`: 选择框渲染
//! - `playback_indicator`: 播放指示线渲染
//! - `program`: PianoRollGrid 结构体定义
//! - `program_impl`: Program trait 实现（事件处理、绘制）

pub mod bars;
pub mod confirm_buttons;
pub mod i2m_box;
pub mod keyboard;
pub mod keys;
pub mod line_tool_box;
pub mod loop_range;
pub mod playback_indicator;
pub mod program;
pub mod program_impl;
pub mod remote_cursors;
pub mod remote_selection;
pub mod ruler;
pub mod selection_box;
pub mod shape_tool_box;
pub mod state;
pub(crate) mod text_tool_box;
pub mod theme;
pub mod utils;
pub mod vertical_bars;
pub mod vertical_keyboard;
pub mod vertical_labels;
pub mod vertical_playback;
pub mod vertical_program;

pub use vertical_program::VerticalRollGrid;

pub use loop_range::{LoopHitTest, LoopRange};
pub use program::PianoRollGrid;
pub use state::GridInteractionState;
