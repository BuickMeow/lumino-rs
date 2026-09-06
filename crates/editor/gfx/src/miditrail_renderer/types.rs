//! Miditrail 3D 渲染器共享类型
//!
//! 用于视频导出的 3D MIDI 轨迹可视化风格，
//! 参考经典 3D MIDI 轨迹渲染的相机、键盘与音符布局，但实现于 wgpu 渲染管线中。

/// 单个音符的 CPU 侧数据（与 `GpuVisibleNote` 对应）
#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct MiditrailNoteGpu {
    /// MIDI 键号（0-127）
    pub key: u32,
    /// 音符起始 tick
    pub start_tick: u32,
    /// 音符结束 tick
    pub end_tick: u32,
    /// 打包 RGBA 颜色（`0xRRGGBBAA`）
    pub color_packed: u32,
    /// 音轨索引（用于调色板）
    pub track_idx: u32,
    /// 音符力度（0-127）
    pub velocity: u32,
    /// MIDI 通道（0-15）
    pub channel: u32,
    /// 对齐填充
    pub _padding: u32,
}

impl MiditrailNoteGpu {
    /// 判断该音符在当前 tick 是否处于激活状态（按下并持续）
    #[must_use]
    pub fn is_active_at(&self, tick: u32) -> bool {
        self.start_tick <= tick && self.end_tick > tick
    }

    /// 判断该音符在当前 tick 是否仍可见（尚未结束）
    #[must_use]
    pub fn is_visible_at(&self, tick: u32) -> bool {
        self.end_tick > tick
    }
}

/// Miditrail 可见 tick 跨度（与 UI 收集窗口同公式，速度越快/远裁越小窗口越窄）。
///
/// UI 窗口收集（`collect_window_notes` 上界）与渲染侧 cull 窗口共用，保证谓词一致：
/// 收集范围按实际 Z 显示距离缩放（GPU 可见条件 `start - tick < span × z_far/SCENE_DEPTH`），
/// 默认 `z_far = SCENE_DEPTH` 时退化为全跨度。公式与 UI 侧逐 op 一致（含 f32 截断）。
#[must_use]
pub fn miditrail_viewport_span(ppq: u32, speed: f32, z_far_distance: f32) -> u32 {
    let z_far_scale = (z_far_distance.max(0.1) / super::MIDITRAIL_SCENE_DEPTH).clamp(
        0.1 / super::MIDITRAIL_SCENE_DEPTH,
        super::MIDITRAIL_MAX_Z_FAR_DISTANCE / super::MIDITRAIL_SCENE_DEPTH,
    );
    let speed = speed.max(0.1);
    let ticks_per_measure = ppq * 4;
    let visible_measure_count = ((4.0 / speed).round()).max(1.0) as u32;
    ((ticks_per_measure * visible_measure_count).max(1) as f32 * z_far_scale) as u32
}

/// MIDITrail 视图模式（GPU 层，与事件层枚举同构，见 VIEW-001）。
///
/// - `Normal`：现有 3D 斜视实现（由旧单一视图迁移而来）；
/// - `Top`：俯视实现（参考 Comet MIDITrail `Top Down Above` 预设），
///   音符起止对齐到时间网格（永不合并），键盘无按压位移只变色。
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MiditrailViewMode {
    /// 普通视图（默认）。
    #[default]
    Normal = 0,
    /// 顶部视图。
    Top = 1,
}

impl MiditrailViewMode {
    /// 是否为顶部视图。
    #[must_use]
    pub fn is_top(self) -> bool {
        matches!(self, MiditrailViewMode::Top)
    }

    /// 视图模式的规范字符串（与事件层 `as_str` 同构）。
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            MiditrailViewMode::Normal => "normal",
            MiditrailViewMode::Top => "top",
        }
    }

    /// 从 `u32` 还原（未知值回退 `Normal`，不静默产生第三种状态）。
    #[must_use]
    pub fn from_u32(value: u32) -> Self {
        match value {
            1 => MiditrailViewMode::Top,
            _ => MiditrailViewMode::Normal,
        }
    }
}

impl std::fmt::Display for MiditrailViewMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MiditrailViewMode::Normal => f.write_str("Normal"),
            MiditrailViewMode::Top => f.write_str("Top"),
        }
    }
}

/// 每帧渲染参数（CPU 侧使用，不直接上传 GPU）
#[derive(Debug, Clone, Copy)]
pub struct MiditrailUniformGpu {
    /// 当前播放 tick
    pub tick: u32,
    /// 每四分音符 tick 数
    pub ppq: u32,
    /// 键盘键数（固定 128）
    pub key_count: u32,
    /// 帧宽度（像素）
    pub frame_width: u32,
    /// 帧高度（像素）
    pub frame_height: u32,
    /// 键盘高度（像素，占位）
    pub kb_height: u32,
    /// 保留
    pub _reserved: u32,
    /// 滚动速度倍率
    pub speed: f32,
    /// 样式附加参数 1
    pub param1: f32,
    /// 样式附加参数 2
    pub param2: f32,
    /// 目标帧率（用于琴键按下/回弹动画的时间步长）。
    pub fps: f32,
    /// Z 方向显示距离（决定音符在多远被截断）。
    pub z_far_distance: f32,
    /// 视图模式（Normal 普通 / Top 顶部；音符显示距离除外，其余设置按视图隔离）。
    pub view_mode: MiditrailViewMode,
    /// 当前 tick 处每秒 tick 数（BPM × ppq / 60）。
    ///
    /// 作为 Aura 光晕环动画的时间基准（参考 Zenith-MIDI 的
    /// `tempoFrameStep = 每帧 tick 数` 与 `maxAuraLen = 每秒 tick 数`），
    /// 使光晕的放大/收缩速度与速度/帧率无关，仅取决于真实时间。
    pub ticks_per_second: f32,
    /// 对齐填充
    pub _padding1: u32,
}

impl Default for MiditrailUniformGpu {
    fn default() -> Self {
        Self {
            tick: 0,
            ppq: 480,
            key_count: 128,
            frame_width: 1920,
            frame_height: 1080,
            kb_height: 128,
            _reserved: 0,
            speed: 1.0,
            param1: 0.0,
            param2: 0.0,
            fps: 60.0,
            z_far_distance: 7.5,
            view_mode: MiditrailViewMode::Normal,
            // 默认按 ppq=480 @ 120 BPM（480 × 2）
            ticks_per_second: 960.0,
            _padding1: 0,
        }
    }
}

/// 相机 Uniform（上传 GPU）
#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct MiditrailCameraGpu {
    /// 投影 * 视图矩阵（column-major）
    pub view_proj: [[f32; 4]; 4],
    /// 指向光源的方向（已归一化）
    pub light_dir: [f32; 3],
    /// 环境光强度
    pub ambient: f32,
}

/// GPU-Driven 音符管线参数（`miditrail_note_driven.wgsl` group1 uniform）。
///
/// CPU 每帧只填这 1KB（tick 相关 7 个 f32/u32 ＋ 128 键位表），位姿推导
/// 进 vertex shader。与 WGSL `DrivenParams` 字段一一对应，顺序一致。
#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct MiditrailDrivenParamsGpu {
    /// 当前 tick（`visible_start = max(start, tick)` 的基准）。
    pub tick: u32,
    /// 视口 tick 跨度（`ticks_per_measure × visible_measure_count`）。
    pub viewport_tick_span: f32,
    /// 场景深度（tick→Z 映射比例）。
    pub scene_depth: f32,
    /// 音符 Z 原点（键盘处）。
    pub note_z_offset: f32,
    /// 远裁剪 Z（`note_z_offset - z_far_distance`）。
    pub z_far: f32,
    /// 音符高度（Y）。
    pub note_height: f32,
    /// 音符 Y 基准。
    pub note_y: f32,
    /// 键盘键数（shader 侧 `key < 128` 硬约束为主，此处仅信息冗余）。
    pub key_count: u32,
    /// `[left, width]` 键位表（与 CPU `key_positions`/`key_widths` 同源；
    /// vec4 满足 uniform 数组 16 字节步长对齐，z/w 保留未用）。
    pub key_table: [[f32; 4]; 128],
}

/// 每实例数据（上传 GPU，与 WGSL 中的 `Instance` 对应）
#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct MiditrailInstanceGpu {
    /// 模型平移
    pub translation: [f32; 3],
    /// 对齐填充（保持 16 字节对齐）
    pub _padding0: f32,
    /// 模型缩放（决定立方体大小）
    pub scale: [f32; 3],
    /// 对齐填充（保持 16 字节对齐）
    pub _padding1: f32,
    /// 打包 RGBA 颜色
    pub color_packed: u32,
    /// 1 表示琴键，0 表示音符
    pub is_key: u32,
    /// 琴键按下系数（0.0 ~ 1.0）
    pub press_factor: f32,
    /// 按下最大位移量（0.5 倍白键高度或 0.5 倍黑键露出高度）
    pub press_depth: f32,
}

impl MiditrailInstanceGpu {
    /// 创建新的实例数据
    #[must_use]
    pub fn new(
        translation: [f32; 3],
        scale: [f32; 3],
        color_packed: u32,
        is_key: bool,
        press_factor: f32,
        press_depth: f32,
    ) -> Self {
        Self {
            translation,
            _padding0: 0.0,
            scale,
            _padding1: 0.0,
            color_packed,
            is_key: u32::from(is_key),
            press_factor,
            press_depth,
        }
    }
}

/// Aura 每实例数据
#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct MiditrailAuraInstanceGpu {
    /// 视觉半径
    pub size: f32,
    /// 键中心 x 位置
    pub pos: f32,
    /// 打包 RGBA 颜色
    pub color_packed: u32,
    /// 对齐填充（保持 16 字节对齐）
    pub _padding: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_miditrail_note_active() {
        let note = MiditrailNoteGpu {
            key: 60,
            start_tick: 100,
            end_tick: 200,
            color_packed: 0,
            track_idx: 0,
            velocity: 100,
            channel: 0,
            _padding: 0,
        };
        assert!(note.is_active_at(150));
        assert!(!note.is_active_at(99));
        assert!(!note.is_active_at(200));
        assert!(note.is_visible_at(150));
        assert!(!note.is_visible_at(250));
    }

    #[test]
    fn test_instance_size() {
        assert_eq!(std::mem::size_of::<MiditrailInstanceGpu>(), 48);
        assert_eq!(std::mem::size_of::<MiditrailAuraInstanceGpu>(), 16);
        assert_eq!(std::mem::size_of::<MiditrailCameraGpu>(), 80);
    }

    #[test]
    fn test_view_mode_default_and_roundtrip() {
        // 默认视图为 Normal（现有行为迁移，切换不丢状态的基准）。
        assert_eq!(MiditrailViewMode::default(), MiditrailViewMode::Normal);
        assert_eq!(
            MiditrailUniformGpu::default().view_mode,
            MiditrailViewMode::Normal
        );
        assert!(!MiditrailViewMode::Normal.is_top());
        assert!(MiditrailViewMode::Top.is_top());
        assert_eq!(MiditrailViewMode::from_u32(0), MiditrailViewMode::Normal);
        assert_eq!(MiditrailViewMode::from_u32(1), MiditrailViewMode::Top);
        // 未知值回退 Normal（不产生第三种状态，避免渲染分支漏覆盖）。
        assert_eq!(MiditrailViewMode::from_u32(99), MiditrailViewMode::Normal);
        assert_eq!(MiditrailViewMode::Normal.as_str(), "normal");
        assert_eq!(MiditrailViewMode::Top.as_str(), "top");
    }
}
