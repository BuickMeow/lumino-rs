use crate::{
    ArrangementNoteInstance, ArrangementNoteUniform, ArrangementUniform, CcBarInstance,
    GridLineInstance, NoteInstance, RulerTickInstance, miditrail_renderer::MiditrailViewMode,
};

mod builder;

pub use builder::RenderParamsBuilder;

/// 渲染参数 - 从 UI 线程传递到 WGPU 线程
#[derive(Debug, Clone)]
pub struct RenderParams {
    /// 物理视口大小
    pub viewport_size: (u32, u32),
    /// 逻辑视口大小
    pub logical_size: (f32, f32),
    /// 缩放因子
    pub scale_factor: f32,
    /// 滚动位置 (x, y)
    pub scroll: (f32, f32),
    /// 缩放 (x, y)
    pub zoom: (f32, f32),
    /// 键盘宽度
    pub keyboard_width: f32,
    /// 标尺高度
    pub ruler_height: f32,
    /// 背景颜色
    pub background_color: [f64; 4],
    /// 网格相关颜色 (用于 Shader)
    pub color_bg: [f32; 4],
    /// 黑键区域背景色 (RGBA)
    pub color_bg_black_key: [f32; 4],
    /// 小节线颜色 (RGBA)
    pub color_bar: [f32; 4],
    /// 拍子线颜色 (RGBA)
    pub color_beat: [f32; 4],
    /// 半拍线颜色 (RGBA)
    pub color_half_beat: [f32; 4],
    /// 细分网格线颜色 (RGBA)
    pub color_grid: [f32; 4],
    /// 琴键分隔线颜色 (RGBA)
    pub color_key_line: [f32; 4],
    /// 网格线实例
    pub grid_instances: Vec<GridLineInstance>,
    /// 音符实例
    pub note_instances: Vec<NoteInstance>,
    /// 标尺刻度实例
    pub ruler_instances: Vec<RulerTickInstance>,
    /// 每小节 tick 数
    pub ticks_per_measure: u32,
    /// 每拍 tick 数
    pub ticks_per_beat: u32,
    /// 拍号变化列表 (tick, 分子, 分母)
    pub time_signatures: Vec<(u32, u8, u8)>,
    /// Canvas 偏移
    pub canvas_offset: (f32, f32),
    /// Canvas 大小
    pub canvas_size: (f32, f32),
    /// 分辨率 (Pulses Per Quarter note)
    pub ppq: f32,
    /// 最大键索引 (visible_key_count - 1)
    pub max_key_index: f32,
    /// 是否为音轨总览模式（音轨总览模式下不渲染钢琴卷帘网格）
    pub is_arrangement_mode: bool,
    /// 音轨总览模式：覆盖层实例（背景/lane/网格/框选/指示线），每帧重建
    pub arrangement_overlay_instances: Vec<ArrangementNoteInstance>,
    /// 音轨总览模式：覆盖层中"背景层"实例数（背景/lane/网格），绘制在音符之下
    pub arrangement_overlay_back_len: usize,
    /// 音轨总览模式：侧栏音轨顺序（文档音轨 id 列表，索引=泳道序号）
    ///
    /// 走带音符复用钢琴卷帘常驻 GPU 音符缓冲（零第二份显存），本字段用于把
    /// 文档音轨映射到泳道序号（见 `arrangement_lane_index`）。
    pub arrangement_track_order: Vec<u32>,
    /// 音轨总览模式：各泳道可见性（与 `arrangement_track_order` 对齐，`false`=静音不绘制）
    pub arrangement_track_visible: Vec<bool>,
    /// 音轨总览模式：文档音轨 → 泳道序号 映射（存储缓冲，着色器按 doc track 索引）
    pub arrangement_lane_index: Vec<f32>,
    /// 音轨总览模式：音符着色器 uniform（滚动/缩放/泳道高/画布偏移）
    pub arrangement_note_uniform: ArrangementNoteUniform,
    /// 音轨总览模式：uniform
    pub arrangement_uniform: ArrangementUniform,
    /// CC 柱状条实例（力度面板所有模式：Velocity/CC/Bend）
    pub cc_bar_instances: Vec<CcBarInstance>,
    /// 力度面板区域 (x, y, width, height) — 屏幕坐标，用于 scissor
    pub velocity_panel_rect: Option<(f32, f32, f32, f32)>,
    /// 是否为瀑布流渲染模式
    pub is_waterfall_mode: bool,
    /// 瀑布流滚动速度
    pub waterfall_speed: f32,
    /// 瀑布流当前 MIDI tick 值（与 scroll.0 不同，scroll.0 是像素位置）
    pub waterfall_current_tick: u32,
    /// Miditrail 渲染开关（非 None 时走 Miditrail 3D GPU 渲染器）
    pub miditrail_enabled: bool,
    /// Miditrail 滚动速度
    pub miditrail_speed: f32,
    /// Miditrail 视图模式（Normal 普通 / Top 顶部，见 VIEW-001）
    pub miditrail_view_mode: MiditrailViewMode,
    /// Miditrail 当前 MIDI tick 值
    pub miditrail_current_tick: u32,
    /// Miditrail Z 方向显示距离（音符在多远被截断）。
    pub miditrail_z_far: f32,
    /// Miditrail 3D 音符开关（默认 false = 平面）：true 还原盒子（12 三角形/音符），
    /// false 只绘制朝向相机的单面（2 三角形/音符，6 倍几何削减）；实例/顺序/颜色零改动。
    pub miditrail_3d_notes: bool,
    /// Miditrail 光晕环动画时间基准：当前 tick 处每秒 tick 数（BPM × ppq / 60）。
    ///
    /// 0 表示未知（由渲染线程回退到 120 BPM 估算），供非导出路径的默认参数使用。
    pub miditrail_ticks_per_second: f32,
    /// Miditrail / 视频导出目标帧率（用于按键动画时间步长）。
    pub fps: f32,
    /// 全屏瀑布流播放器模式：跳过钢琴卷帘 3D 场景绘制（仅发布活体音符缓冲），解放 GPU。
    ///
    /// `true` 时渲染线程仍上传/发布音符缓冲（`note_data_pub`）供播放器复用，
    /// 但不再执行 `render_offscreen_pass`（网格/音符/洋葱皮的离屏绘制），
    /// 与钢琴卷帘完全隔离，避免每帧空转绘制整张卷帘。
    pub skip_scene_render: bool,
    /// 纵向卷帘模式：网格与音符使用转置着色器（复用同 MIDI GPU 数据，瀑布流风格纵向流动）
    pub is_vertical_roll: bool,
}

impl Default for RenderParams {
    fn default() -> Self {
        Self {
            viewport_size: (800, 600),
            logical_size: (800.0, 600.0),
            scale_factor: 1.0,
            scroll: (0.0, 0.0),
            zoom: (0.1, 20.0),
            keyboard_width: 60.0,
            ruler_height: 30.0,
            background_color: [0.1, 0.1, 0.1, 1.0],
            color_bg: [0.1, 0.1, 0.1, 1.0],
            color_bg_black_key: [0.07, 0.07, 0.07, 1.0],
            color_bar: [0.3, 0.3, 0.3, 1.0],
            color_beat: [0.2, 0.2, 0.2, 1.0],
            color_half_beat: [0.15, 0.15, 0.15, 1.0],
            color_grid: [0.15, 0.15, 0.15, 1.0],
            color_key_line: [0.4, 0.4, 0.4, 1.0],
            grid_instances: Vec::new(),
            note_instances: Vec::new(),
            ruler_instances: Vec::new(),
            ticks_per_measure: 7680,
            ticks_per_beat: 1920,
            canvas_offset: (0.0, 0.0),
            canvas_size: (800.0, 600.0),
            ppq: 1920.0,
            max_key_index: 127.0,
            is_arrangement_mode: false,
            arrangement_overlay_instances: Vec::new(),
            arrangement_overlay_back_len: 0,
            arrangement_track_order: Vec::new(),
            arrangement_track_visible: Vec::new(),
            arrangement_lane_index: Vec::new(),
            arrangement_note_uniform: ArrangementNoteUniform::default(),
            arrangement_uniform: ArrangementUniform::default(),
            cc_bar_instances: Vec::new(),
            velocity_panel_rect: None,
            time_signatures: vec![(0, 4, 4)],
            is_waterfall_mode: false,
            waterfall_speed: 1.0,
            waterfall_current_tick: 0,
            miditrail_enabled: false,
            miditrail_speed: 1.0,
            miditrail_view_mode: MiditrailViewMode::Normal,
            miditrail_current_tick: 0,
            miditrail_z_far: 7.5,
            miditrail_3d_notes: false,
            miditrail_ticks_per_second: 0.0,
            fps: 60.0,
            skip_scene_render: false,
            is_vertical_roll: false,
        }
    }
}

impl RenderParams {
    /// 创建 Builder，推荐的构造方式。
    ///
    /// ```ignore
    /// RenderParams::builder()
    ///     .viewport_size((1920, 1080))
    ///     .scroll((100.0, 50.0))
    ///     .zoom((0.5, 10.0))
    ///     .build()
    /// ```
    pub fn builder() -> RenderParamsBuilder {
        RenderParamsBuilder::default()
    }
}
