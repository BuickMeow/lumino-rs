//! [`RenderParams`] 的 Builder 实现

use super::RenderParams;
use crate::{
    ArrangementNoteInstance, ArrangementNoteUniform, ArrangementUniform, CcBarInstance,
    GridLineInstance, RulerTickInstance, miditrail_renderer::MiditrailViewMode,
};

/// [`RenderParams`] 的 Builder。
///
/// 所有字段均有合理默认值，只需设置需要变更的字段即可。
#[derive(Debug, Clone)]
pub struct RenderParamsBuilder {
    viewport_size: (u32, u32),
    logical_size: (f32, f32),
    scale_factor: f32,
    scroll: (f32, f32),
    zoom: (f32, f32),
    keyboard_width: f32,
    ruler_height: f32,
    background_color: [f64; 4],
    color_bg: [f32; 4],
    color_bg_black_key: [f32; 4],
    color_bar: [f32; 4],
    color_beat: [f32; 4],
    color_half_beat: [f32; 4],
    color_grid: [f32; 4],
    color_key_line: [f32; 4],
    grid_instances: Vec<GridLineInstance>,
    ruler_instances: Vec<RulerTickInstance>,
    ppq: f32,
    max_key_index: f32,
    is_arrangement_mode: bool,
    arrangement_overlay_instances: Vec<ArrangementNoteInstance>,
    arrangement_overlay_back_len: usize,
    arrangement_track_order: Vec<u32>,
    arrangement_track_visible: Vec<bool>,
    arrangement_lane_index: Vec<f32>,
    arrangement_note_uniform: ArrangementNoteUniform,
    arrangement_uniform: ArrangementUniform,
    cc_bar_instances: Vec<CcBarInstance>,
    canvas_offset: (f32, f32),
    canvas_size: (f32, f32),
    velocity_panel_rect: Option<(f32, f32, f32, f32)>,
    time_signatures: Vec<(u32, u8, u8)>,
    is_waterfall_mode: bool,
    waterfall_speed: f32,
    waterfall_current_tick: u32,
    miditrail_enabled: bool,
    miditrail_speed: f32,
    miditrail_view_mode: MiditrailViewMode,
    miditrail_current_tick: u32,
    miditrail_z_far: f32,
    miditrail_3d_notes: bool,
    miditrail_ticks_per_second: f32,
    fps: f32,
    skip_scene_render: bool,
    is_vertical_roll: bool,
}

impl Default for RenderParamsBuilder {
    /// 默认值单一来源：从 [`RenderParams::default`] 拷贝，
    /// 避免与主类型的手写默认值漂移（新增字段只需改主类型一处）。
    fn default() -> Self {
        let base = RenderParams::default();
        Self {
            viewport_size: base.viewport_size,
            logical_size: base.logical_size,
            scale_factor: base.scale_factor,
            scroll: base.scroll,
            zoom: base.zoom,
            keyboard_width: base.keyboard_width,
            ruler_height: base.ruler_height,
            background_color: base.background_color,
            color_bg: base.color_bg,
            color_bg_black_key: base.color_bg_black_key,
            color_bar: base.color_bar,
            color_beat: base.color_beat,
            color_half_beat: base.color_half_beat,
            color_grid: base.color_grid,
            color_key_line: base.color_key_line,
            grid_instances: base.grid_instances,
            ruler_instances: base.ruler_instances,
            ppq: base.ppq,
            max_key_index: base.max_key_index,
            is_arrangement_mode: base.is_arrangement_mode,
            arrangement_overlay_instances: base.arrangement_overlay_instances,
            arrangement_overlay_back_len: base.arrangement_overlay_back_len,
            arrangement_track_order: base.arrangement_track_order,
            arrangement_track_visible: base.arrangement_track_visible,
            arrangement_lane_index: base.arrangement_lane_index,
            arrangement_note_uniform: base.arrangement_note_uniform,
            arrangement_uniform: base.arrangement_uniform,
            cc_bar_instances: base.cc_bar_instances,
            canvas_offset: base.canvas_offset,
            canvas_size: base.canvas_size,
            velocity_panel_rect: base.velocity_panel_rect,
            time_signatures: base.time_signatures,
            is_waterfall_mode: base.is_waterfall_mode,
            waterfall_speed: base.waterfall_speed,
            waterfall_current_tick: base.waterfall_current_tick,
            miditrail_enabled: base.miditrail_enabled,
            miditrail_speed: base.miditrail_speed,
            miditrail_view_mode: base.miditrail_view_mode,
            miditrail_current_tick: base.miditrail_current_tick,
            miditrail_z_far: base.miditrail_z_far,
            miditrail_3d_notes: base.miditrail_3d_notes,
            miditrail_ticks_per_second: base.miditrail_ticks_per_second,
            fps: base.fps,
            skip_scene_render: base.skip_scene_render,
            is_vertical_roll: base.is_vertical_roll,
        }
    }
}

impl RenderParamsBuilder {
    /// 设置物理视口大小
    pub fn viewport_size(mut self, size: (u32, u32)) -> Self {
        self.viewport_size = size;
        self
    }

    /// 设置逻辑视口大小
    pub fn logical_size(mut self, size: (f32, f32)) -> Self {
        self.logical_size = size;
        self
    }

    /// 设置缩放因子
    pub fn scale_factor(mut self, factor: f32) -> Self {
        self.scale_factor = factor;
        self
    }

    /// 设置滚动位置
    pub fn scroll(mut self, scroll: (f32, f32)) -> Self {
        self.scroll = scroll;
        self
    }

    /// 设置缩放
    pub fn zoom(mut self, zoom: (f32, f32)) -> Self {
        self.zoom = zoom;
        self
    }

    /// 设置键盘宽度
    pub fn keyboard_width(mut self, width: f32) -> Self {
        self.keyboard_width = width;
        self
    }

    /// 设置标尺高度
    pub fn ruler_height(mut self, height: f32) -> Self {
        self.ruler_height = height;
        self
    }

    /// 设置背景颜色
    pub fn background_color(mut self, color: [f64; 4]) -> Self {
        self.background_color = color;
        self
    }

    /// 设置网格背景色
    pub fn color_bg(mut self, color: [f32; 4]) -> Self {
        self.color_bg = color;
        self
    }

    /// 设置黑键背景色
    pub fn color_bg_black_key(mut self, color: [f32; 4]) -> Self {
        self.color_bg_black_key = color;
        self
    }

    /// 设置小节线颜色
    pub fn color_bar(mut self, color: [f32; 4]) -> Self {
        self.color_bar = color;
        self
    }

    /// 设置拍线颜色
    pub fn color_beat(mut self, color: [f32; 4]) -> Self {
        self.color_beat = color;
        self
    }

    /// 设置半拍线颜色
    pub fn color_half_beat(mut self, color: [f32; 4]) -> Self {
        self.color_half_beat = color;
        self
    }

    /// 设置网格线颜色
    pub fn color_grid(mut self, color: [f32; 4]) -> Self {
        self.color_grid = color;
        self
    }

    /// 设置键位线颜色
    pub fn color_key_line(mut self, color: [f32; 4]) -> Self {
        self.color_key_line = color;
        self
    }

    /// 设置标尺刻度实例
    pub fn ruler_instances(mut self, instances: Vec<RulerTickInstance>) -> Self {
        self.ruler_instances = instances;
        self
    }

    /// 设置 PPQ (Pulses Per Quarter note)
    pub fn ppq(mut self, ppq: f32) -> Self {
        self.ppq = ppq;
        self
    }

    /// 设置最大键索引
    pub fn max_key_index(mut self, index: f32) -> Self {
        self.max_key_index = index;
        self
    }

    /// 设置是否为音轨总览模式
    pub fn is_arrangement_mode(mut self, mode: bool) -> Self {
        self.is_arrangement_mode = mode;
        self
    }

    /// 设置音轨总览模式覆盖层实例（背景/lane/网格/框选/指示线），每帧重建
    pub fn arrangement_overlay_instances(
        mut self,
        instances: Vec<ArrangementNoteInstance>,
    ) -> Self {
        self.arrangement_overlay_instances = instances;
        self
    }

    /// 设置覆盖层中"背景层"实例数（背景/lane/网格），绘制在音符之下
    pub fn arrangement_overlay_back_len(mut self, len: usize) -> Self {
        self.arrangement_overlay_back_len = len;
        self
    }

    /// 设置音轨总览模式侧栏音轨顺序（文档音轨 id 列表，索引=泳道序号）
    pub fn arrangement_track_order(mut self, order: Vec<u32>) -> Self {
        self.arrangement_track_order = order;
        self
    }

    /// 设置音轨总览模式各泳道可见性（与 `arrangement_track_order` 对齐）
    pub fn arrangement_track_visible(mut self, visible: Vec<bool>) -> Self {
        self.arrangement_track_visible = visible;
        self
    }

    /// 设置音轨总览模式 uniform
    pub fn arrangement_uniform(mut self, uniform: ArrangementUniform) -> Self {
        self.arrangement_uniform = uniform;
        self
    }

    /// 设置 CC 柱状条实例
    pub fn cc_bar_instances(mut self, instances: Vec<CcBarInstance>) -> Self {
        self.cc_bar_instances = instances;
        self
    }

    /// 设置 Canvas 偏移
    pub fn canvas_offset(mut self, offset: (f32, f32)) -> Self {
        self.canvas_offset = offset;
        self
    }

    /// 设置 Canvas 大小
    pub fn canvas_size(mut self, size: (f32, f32)) -> Self {
        self.canvas_size = size;
        self
    }

    /// 设置力度面板区域
    pub fn velocity_panel_rect(mut self, rect: Option<(f32, f32, f32, f32)>) -> Self {
        self.velocity_panel_rect = rect;
        self
    }

    /// 设置拍号变化列表
    pub fn time_signatures(mut self, time_signatures: Vec<(u32, u8, u8)>) -> Self {
        self.time_signatures = time_signatures;
        self
    }

    /// 设置是否跳过钢琴卷帘 3D 场景绘制（全屏瀑布流播放器模式用）。
    ///
    /// `true` 时渲染线程仍上传/发布音符缓冲，但不再执行 `render_offscreen_pass`。
    pub fn skip_scene_render(mut self, skip: bool) -> Self {
        self.skip_scene_render = skip;
        self
    }

    /// 设置是否为纵向卷帘（网格与音符转置，复用同 MIDI GPU 数据）
    pub fn is_vertical_roll(mut self, is_vertical: bool) -> Self {
        self.is_vertical_roll = is_vertical;
        self
    }

    /// 构建 [`RenderParams`]。
    ///
    /// 从首个拍号推导默认 `ticks_per_measure` 和 `ticks_per_beat`
    ///（供背景 shader 使用；变化拍号由 CPU 标尺实例处理）。
    /// `note_instances` 使用默认值（builder 不暴露该字段）。
    pub fn build(self) -> RenderParams {
        let (ticks_per_measure, ticks_per_beat) =
            compute_ticks_from_first_time_signature(self.ppq, &self.time_signatures);
        RenderParams {
            viewport_size: self.viewport_size,
            logical_size: self.logical_size,
            scale_factor: self.scale_factor,
            scroll: self.scroll,
            zoom: self.zoom,
            keyboard_width: self.keyboard_width,
            ruler_height: self.ruler_height,
            background_color: self.background_color,
            color_bg: self.color_bg,
            color_bg_black_key: self.color_bg_black_key,
            color_bar: self.color_bar,
            color_beat: self.color_beat,
            color_half_beat: self.color_half_beat,
            color_grid: self.color_grid,
            color_key_line: self.color_key_line,
            grid_instances: self.grid_instances,
            note_instances: Vec::new(),
            ruler_instances: self.ruler_instances,
            ticks_per_measure,
            ticks_per_beat,
            canvas_offset: self.canvas_offset,
            canvas_size: self.canvas_size,
            ppq: self.ppq,
            max_key_index: self.max_key_index,
            is_arrangement_mode: self.is_arrangement_mode,
            arrangement_overlay_instances: self.arrangement_overlay_instances,
            arrangement_overlay_back_len: self.arrangement_overlay_back_len,
            arrangement_track_order: self.arrangement_track_order,
            arrangement_track_visible: self.arrangement_track_visible,
            arrangement_lane_index: self.arrangement_lane_index,
            arrangement_note_uniform: self.arrangement_note_uniform,
            arrangement_uniform: self.arrangement_uniform,
            cc_bar_instances: self.cc_bar_instances,
            velocity_panel_rect: self.velocity_panel_rect,
            time_signatures: self.time_signatures,
            is_waterfall_mode: self.is_waterfall_mode,
            waterfall_speed: self.waterfall_speed,
            waterfall_current_tick: self.waterfall_current_tick,
            miditrail_enabled: self.miditrail_enabled,
            miditrail_speed: self.miditrail_speed,
            miditrail_view_mode: self.miditrail_view_mode,
            miditrail_current_tick: self.miditrail_current_tick,
            miditrail_z_far: self.miditrail_z_far,
            miditrail_3d_notes: self.miditrail_3d_notes,
            miditrail_ticks_per_second: self.miditrail_ticks_per_second,
            fps: self.fps,
            skip_scene_render: self.skip_scene_render,
            is_vertical_roll: self.is_vertical_roll,
        }
    }
}

/// 根据首个拍号计算每小节/每拍 tick 数
fn compute_ticks_from_first_time_signature(
    ppq: f32,
    time_signatures: &[(u32, u8, u8)],
) -> (u32, u32) {
    let (_, numerator, denominator) = time_signatures.first().copied().unwrap_or((0, 4, 4));
    let beat_ticks = ppq * 4.0 / denominator.max(1) as f32;
    let measure_ticks = beat_ticks * numerator.max(1) as f32;
    (measure_ticks as u32, beat_ticks as u32)
}
