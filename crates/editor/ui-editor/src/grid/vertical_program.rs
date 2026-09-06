//! 纵向卷帘 Canvas Program — 底部横向钢琴键盘 + 网格线（水平时间 + 垂直音高）
//!
//! 复用横向 `PianoRollGrid` 的交互状态与缩放/滚动语义，仅绘制层转置。

use super::state::GridInteractionState;
use crate::Editor;
use iced_core::{Point, Rectangle, mouse};
use iced_widget::canvas::{Action, Event, Geometry, Program};
use lumino_message::Tool;
use lumino_ui_core::{Message, Renderer, Theme};

/// 纵向卷帘网格绘制程序
pub struct VerticalRollGrid<'a> {
    pub editor: &'a Editor,
}

impl<'a> VerticalRollGrid<'a> {
    pub fn new(editor: &'a Editor) -> Self {
        Self { editor }
    }
}

impl VerticalRollGrid<'_> {
    fn detect_double_click(&self, state: &mut GridInteractionState, local_pos: Point) -> bool {
        use lumino_ui_core::constants::editor as editor_constants;
        let now = std::time::Instant::now();
        let is_double_click = state.last_click_pos.is_some_and(|last_pos| {
            let time_delta = now.duration_since(state.last_click_time).as_millis();
            let pos_delta =
                ((local_pos.x - last_pos.x).powi(2) + (local_pos.y - last_pos.y).powi(2)).sqrt();
            time_delta < editor_constants::DOUBLE_CLICK_TIME_MS
                && pos_delta < editor_constants::DOUBLE_CLICK_DISTANCE_PX
        });
        if !is_double_click {
            state.last_click_time = now;
            state.last_click_pos = Some(local_pos);
        }
        is_double_click
    }

    fn handle_left_press_vertical(
        &self,
        state: &mut GridInteractionState,
        local_pos: Point,
    ) -> Option<Action<Message>> {
        use lumino_ui_core::message::EditorAction;
        // 纵向：头部在键盘顶部，无顶部标尺，循环区域与固定指示线逻辑沿用横向（暂不转置）
        // 仅保留核心 Pressed/DoubleClicked 分发，纵向坐标由 Editor 层转置处理
        if self.detect_double_click(state, local_pos) {
            return Some(Action::publish(Message::EditorAction(
                EditorAction::DoubleClicked(lumino_ui_core::message::Point2::new(
                    local_pos.x,
                    local_pos.y,
                )),
            )));
        }
        Some(Action::publish(Message::EditorAction(
            EditorAction::Pressed {
                pos: lumino_ui_core::message::Point2::new(local_pos.x, local_pos.y),
                shift: state.shift_pressed,
                // 携带 canvas 本地 ModifiersChanged 的 Ctrl 状态（可靠通道），
                // 用于复制拖拽判定（见横向 handle_left_press 注释）。
                ctrl: state.control_pressed,
            },
        )))
    }
}

impl Program<Message, Theme, Renderer> for VerticalRollGrid<'_> {
    type State = GridInteractionState;

    fn update(
        &self,
        state: &mut Self::State,
        event: &Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> Option<Action<Message>> {
        let bounds_pos = Point::new(bounds.x, bounds.y);
        let bounds_size = iced_core::Size::new(bounds.width, bounds.height);

        let canvas = &self.editor.editor_state.canvas;
        let new_size = Point::new(bounds.width, bounds.height);
        if canvas.size_x != new_size.x
            || canvas.size_y != new_size.y
            || canvas.offset_x != bounds_pos.x
            || canvas.offset_y != bounds_pos.y
        {
            return Some(Action::publish(
                lumino_ui_core::Message::CanvasBoundsChanged {
                    offset: lumino_ui_core::message::Point2::new(bounds_pos.x, bounds_pos.y),
                    size: lumino_ui_core::message::Size2::new(
                        bounds_size.width,
                        bounds_size.height,
                    ),
                },
            ));
        }

        if let Some(position) = cursor.position() {
            let local_pos = Point::new(position.x - bounds.x, position.y - bounds.y);
            state.position = Some(local_pos);
        }

        if let Event::Keyboard(iced_core::keyboard::Event::ModifiersChanged(modifiers)) = event {
            state.shift_pressed = modifiers.shift();
            state.control_pressed = modifiers.control();
        }

        let cursor_over_bounds = cursor.position_over(bounds);
        match event {
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                if let Some(position) = cursor_over_bounds {
                    let local_pos = Point::new(position.x - bounds.x, position.y - bounds.y);
                    // 图片转 MIDI 与曲线工具悬浮按钮优先（复用横向逻辑）
                    if self.editor.editor_state.image_to_midi.mode
                        == lumino_editor_state::ImageToMidiMode::Placing
                        && let Some(btns) = crate::grid::i2m_box::i2m_button_rects(self.editor)
                    {
                        if btns.confirm.contains(local_pos) {
                            return Some(Action::publish(Message::RightSidebar(
                                lumino_message::RightSidebarAction::PlacementConfirm,
                            )));
                        }
                        if btns.cancel.contains(local_pos) {
                            return Some(Action::publish(Message::RightSidebar(
                                lumino_message::RightSidebarAction::PlacementCancel,
                            )));
                        }
                    }
                    if self.editor.current_tool() == lumino_message::Tool::Curve
                        && let Some(btns) =
                            crate::grid::line_tool_box::line_button_rects(self.editor)
                    {
                        if btns.confirm.contains(local_pos) {
                            return Some(Action::publish(Message::EditorAction(
                                lumino_ui_core::message::EditorAction::LineToolConfirm,
                            )));
                        }
                        if btns.cancel.contains(local_pos) {
                            return Some(Action::publish(Message::EditorAction(
                                lumino_ui_core::message::EditorAction::LineToolCancel,
                            )));
                        }
                    }
                    if self.editor.current_tool() == lumino_message::Tool::Shape
                        && let Some(btns) =
                            crate::grid::shape_tool_box::shape_button_rects(self.editor)
                    {
                        if btns.confirm.contains(local_pos) {
                            return Some(Action::publish(Message::EditorAction(
                                lumino_ui_core::message::EditorAction::ShapeToolConfirm,
                            )));
                        }
                        if btns.cancel.contains(local_pos) {
                            return Some(Action::publish(Message::EditorAction(
                                lumino_ui_core::message::EditorAction::ShapeToolCancel,
                            )));
                        }
                    }
                    if self.editor.is_inside_canvas(local_pos) {
                        return self.handle_left_press_vertical(state, local_pos);
                    }
                }
            }
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Right)) => {
                if let Some(position) = cursor_over_bounds {
                    let local_pos = Point::new(position.x - bounds.x, position.y - bounds.y);
                    if self.editor.is_inside_canvas(local_pos) {
                        return Some(Action::publish(Message::PianoRollContextMenu(
                            lumino_message::PianoRollContextMenuAction::Open {
                                position: lumino_message::Point2::new(local_pos.x, local_pos.y),
                            },
                        )));
                    }
                }
            }
            Event::Mouse(mouse::Event::CursorMoved { position }) => {
                let local_pos = Point::new(position.x - bounds.x, position.y - bounds.y);
                // 更新框选框平滑动画（复用 Editor 逻辑）
                crate::grid::program::PianoRollGrid::new(self.editor)
                    .update_selection_box_animation(Some(local_pos));
                if cursor_over_bounds.is_some() {
                    return Some(Action::publish(Message::EditorAction(
                        lumino_ui_core::message::EditorAction::Moved(
                            lumino_ui_core::message::Point2::new(local_pos.x, local_pos.y),
                        ),
                    )));
                }
            }
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                self.editor.selection_box_anim.set(None);
                return Some(Action::publish(Message::EditorAction(
                    lumino_ui_core::message::EditorAction::Released,
                )));
            }
            _ => {}
        }

        // 键盘区域滚轮：支持缩放（Ctrl+滚轮）与左右滚动（音高轴 X）
        if let Event::Mouse(mouse::Event::WheelScrolled { delta }) = event
            && let Some(pos) = cursor.position_over(bounds)
        {
            let local = Point::new(pos.x - bounds.x, pos.y - bounds.y);
            let view = &self.editor.editor_state.view;
            let keyboard_h = view.keyboard_width;
            let is_over_keyboard = local.y >= bounds.height - keyboard_h;
            if is_over_keyboard {
                // 便捷缩放：键盘区滚轮直接缩放音高 Y（无需 Ctrl），对齐 yinhe 左区逻辑
                if let Some(factor) = crate::zoom::zoom_factor_from_delta(delta) {
                    let fixed_ratio = (local.x / bounds.width).clamp(0.0, 1.0);
                    return Some(Action::publish(lumino_ui_core::Message::ZoomYChanged {
                        zoom: view.zoom_y * factor,
                        fixed_ratio,
                    }));
                }
            } else if local.y < bounds.height - keyboard_h {
                // 网格区域：Y 向时间轴（头部在键盘顶部，向上递增，纵向隐藏横向标尺故 grid_top=0）支持滚动与缩放
                let ctrl_pressed = state.control_pressed || self.editor.ctrl_pressed();
                let grid_top = 0.0;
                let grid_bottom = bounds.height - keyboard_h;
                let grid_h = (grid_bottom - grid_top).max(1.0);
                if ctrl_pressed {
                    if let Some(factor) = crate::zoom::zoom_factor_from_delta(delta) {
                        // 锚点距底部比例：0=键盘顶部，1=顶部标尺
                        let fixed_ratio = ((grid_bottom - local.y) / grid_h).clamp(0.0, 1.0);
                        return Some(Action::publish(lumino_ui_core::Message::ZoomXChanged {
                            zoom: view.zoom_x * factor,
                            fixed_ratio,
                        }));
                    }
                } else {
                    // 普通滚轮：垂直滚动时间轴（Y，头部在底部），水平滚动音高轴（X）
                    use lumino_ui_core::constants::editor::{SCROLL_LINES_SCALE, SCROLL_MAX_DELTA};
                    let (delta_x, delta_y) = match delta {
                        mouse::ScrollDelta::Lines { x, y } => {
                            (*x * SCROLL_LINES_SCALE, *y * SCROLL_LINES_SCALE)
                        }
                        mouse::ScrollDelta::Pixels { x, y } => (*x, *y),
                    };
                    let limit = match delta {
                        mouse::ScrollDelta::Pixels { .. } => 400.0,
                        _ => SCROLL_MAX_DELTA,
                    };
                    let mut out_delta_x = 0.0;
                    let mut out_delta_y = 0.0;
                    // 垂直增量 -> 时间轴（scroll_x），取反使向上滚显示更后时间（与横向一致）
                    if delta_y.abs() > f32::EPSILON {
                        out_delta_x = (-delta_y).clamp(-limit, limit);
                    }
                    // 水平增量 -> 音高轴（scroll_y），取反使向右滚显示更高音
                    if delta_x.abs() > f32::EPSILON {
                        out_delta_y = (-delta_x).clamp(-limit, limit);
                    }
                    // Shift+滚轮：垂直转水平（触控板兼容）
                    if state.shift_pressed
                        && out_delta_x.abs() < f32::EPSILON
                        && delta_y.abs() > f32::EPSILON
                    {
                        out_delta_y = (-delta_y).clamp(-limit, limit);
                        out_delta_x = 0.0;
                    }
                    if out_delta_x.abs() > f32::EPSILON || out_delta_y.abs() > f32::EPSILON {
                        return Some(Action::publish(lumino_ui_core::Message::EditorAction(
                            lumino_ui_core::message::EditorAction::Scrolled {
                                delta_x: out_delta_x,
                                delta_y: out_delta_y,
                            },
                        )));
                    }
                }
            }
        }

        None
    }

    fn mouse_interaction(
        &self,
        state: &Self::State,
        _bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> mouse::Interaction {
        // 纵向卷帘光标反馈（与横向 `program_impl/mouse_interaction.rs` 对齐）：
        // 橡皮擦十字、曲线工具命中锚点/段可拖动（Pointer）、其余十字。
        if matches!(self.editor.current_tool(), Tool::Eraser | Tool::DrawEraser) {
            return mouse::Interaction::Crosshair;
        }
        if self.editor.current_tool() == Tool::Curve {
            if let Some(local_pos) = state.position
                && self.editor.line_tool_hit_test(local_pos).is_some()
            {
                return mouse::Interaction::Pointer;
            }
            return mouse::Interaction::Crosshair;
        }
        mouse::Interaction::Idle
    }

    fn draw(
        &self,
        _state: &Self::State,
        renderer: &Renderer,
        theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry<Renderer>> {
        let mut geometries = Vec::new();

        // 1. 网格线与音符已迁移至 wgpu（infinite_grid_vertical.wgsl + note_vertical.wgsl），
        //    复用加载 MIDI 后同批 NoteInstance GPU 数据，仅转置坐标（瀑布流风格纵向流动），
        //    样式完移植横向 LOD 且 Key 范围八度分割更明显（C 音 2px/0.95 alpha）；
        //    Canvas 层不再绘制网格/音符，避免与 wgpu 离屏纹理重叠，仅保留键盘与指示线。
        //    （保留 vertical_bars.rs 作离线校验与单元测试，不再参与实时绘制）

        // 2. 底部横向钢琴键盘（缓存与横向一致：复用 keyboard_cache 语义，但此处每帧直绘）
        let kb_geom = {
            let mut frame = iced_widget::canvas::Frame::new(renderer, bounds.size());
            super::vertical_keyboard::draw(self.editor, &mut frame, bounds, theme);
            frame.into_geometry()
        };
        geometries.push(kb_geom);

        // 2.2 小节号文本与边框（网格线已由 wgpu 绘制，此处仅保留文本，避免重复）
        let label_geom = {
            let mut frame = iced_widget::canvas::Frame::new(renderer, bounds.size());
            super::vertical_labels::draw_labels(self.editor, &mut frame, bounds, theme);
            frame.into_geometry()
        };
        geometries.push(label_geom);

        // 2.1 洋葱皮覆盖层
        if let Some(geom) =
            super::vertical_keyboard::draw_onion_overlay(self.editor, renderer, bounds)
        {
            geometries.push(geom);
        }

        // 2.3 曲线工具 / 图片转 MIDI / 选框 的 Canvas 图层（纵向卷帘 BUG 修复：
        // 旧实现漏挂曲线工具图层，导致路径/锚点/控制柄/√×按钮全部不可见）。
        // 与横向 `program_impl/draw.rs` 图层顺序对齐（指示线保持最顶层）。
        if let Some(geom) = crate::grid::selection_box::draw(self.editor, renderer, theme, bounds) {
            geometries.push(geom);
        }
        if let Some(geom) = crate::grid::i2m_box::draw(self.editor, renderer, theme, bounds) {
            geometries.push(geom);
        }
        if let Some(geom) = crate::grid::line_tool_box::draw(self.editor, renderer, theme, bounds) {
            geometries.push(geom);
        }
        if let Some(geom) = crate::grid::shape_tool_box::draw(self.editor, renderer, theme, bounds)
        {
            geometries.push(geom);
        }
        if let Some(geom) = crate::grid::text_tool_box::draw(self.editor, renderer, theme, bounds) {
            geometries.push(geom);
        }

        // 3. 播放指示线（水平红线，时间轴在 Y）
        if let Some(geom) = super::vertical_playback::draw(self.editor, renderer, bounds) {
            geometries.push(geom);
        }

        geometries
    }
}
