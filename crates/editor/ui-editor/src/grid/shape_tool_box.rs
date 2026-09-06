//! 形状工具渲染：待确认图形 + 实时拖拽预览 + 共享 √× 按钮
//!
//! 与 `line_tool_box` 同构：拖拽拉出外接框 → 矢量预览（矩形/圆/三角）→
//! 待确认图形悬停 √× 按钮（确认生成音符 / 取消清空）。
//!
//! 形状以矢量绘制（与网格精度无关），填充桶开启时内部以半透明色块显示，
//! √ 确认后成为实心音符（见 `interaction::shape_tool::confirm_shape_tool`）。

use crate::Editor;
use crate::grid::confirm_buttons::{BUTTON_SIZE, CANCEL_ICON, CONFIRM_ICON, draw_button};
use crate::grid::utils::content_bounds;
use iced_core::{Color, Point, Rectangle, Size};
use iced_widget::canvas::{self, Geometry, Path, Stroke};
use lumino_editor_state::ShapeKind;
use lumino_editor_state::shape_tool::{effective_rect, shape_vertices};
use lumino_ui_core::Renderer;

/// 填充色块透明度（半透明叠加在网格上，√ 确认后成为实心音符）
const FILL_ALPHA: f32 = 0.35;
/// 按钮组与图形包围盒的间距
const BUTTON_SPACING: f32 = 8.0;

/// 悬浮按钮矩形（画布坐标）
#[derive(Debug, Clone, Copy)]
pub struct ShapeButtonRects {
    /// √ 确认按钮
    pub confirm: Rectangle,
    /// × 取消按钮
    pub cancel: Rectangle,
}

/// 单个图形在屏幕坐标下的包围盒 (min_x, max_x, min_y, max_y)
fn shape_screen_aabb(
    editor: &Editor,
    kind: ShapeKind,
    rect: (f32, f32, f32, f32),
    shift: bool,
) -> (f32, f32, f32, f32) {
    let mut min_x = f32::INFINITY;
    let mut max_x = f32::NEG_INFINITY;
    let mut min_y = f32::INFINITY;
    let mut max_y = f32::NEG_INFINITY;
    let mut acc = |p: Point| {
        min_x = min_x.min(p.x);
        max_x = max_x.max(p.x);
        min_y = min_y.min(p.y);
        max_y = max_y.max(p.y);
    };
    // 统一应用屏幕空间 Shift 正图形约束，使包围盒与生成音符一致
    let px_per_tick = editor.editor_state.view.zoom_x;
    let px_per_key = editor.editor_state.view.zoom_y;
    let rect = effective_rect(kind, rect, shift, px_per_tick, px_per_key);
    match kind {
        ShapeKind::Circle => {
            let (cx0, cy0, cx1, cy1) = rect;
            let mx = (cx0 + cx1) / 2.0;
            let my = (cy0 + cy1) / 2.0;
            acc(editor.line_pos_screen_pos((cx0, my)));
            acc(editor.line_pos_screen_pos((cx1, my)));
            acc(editor.line_pos_screen_pos((mx, cy0)));
            acc(editor.line_pos_screen_pos((mx, cy1)));
        }
        _ => {
            let verts =
                shape_vertices(kind, rect, false, px_per_tick, px_per_key).unwrap_or_default();
            for (t, k) in verts {
                acc(editor.line_pos_screen_pos((t, k)));
            }
        }
    }
    (min_x, max_x, min_y, max_y)
}

/// 计算待确认图形 + 拖拽预览的共享悬浮按钮位置（垂直居中于包围盒中心）
pub fn shape_button_rects(editor: &Editor) -> Option<ShapeButtonRects> {
    // Conductor 音轨（track 0）：整工具不可用，不计算 √× 按钮命中区
    if editor.editor_state.data.current_track == 0 {
        return None;
    }
    if editor.current_tool() != lumino_message::Tool::Shape {
        return None;
    }
    let shape_tool = &editor.editor_state.shape_tool;
    if !shape_tool.has_pending() {
        return None;
    }

    let mut bounds: Option<(f32, f32, f32, f32)> = None;
    for shape in &shape_tool.shapes {
        let b = shape_screen_aabb(editor, shape.kind, shape.rect, shape.shift_constrained);
        bounds = Some(match bounds {
            None => b,
            Some((min_x, max_x, min_y, max_y)) => (
                min_x.min(b.0),
                max_x.max(b.1),
                min_y.min(b.2),
                max_y.max(b.3),
            ),
        });
    }

    let (min_x, max_x, min_y, max_y) = bounds?;
    let content = content_bounds(editor);
    if content.height < BUTTON_SIZE {
        return None;
    }
    let mid_x = (min_x + max_x) * 0.5;
    let mid_y = (min_y + max_y) * 0.5;

    let group_w = BUTTON_SIZE * 2.0 + BUTTON_SPACING;
    let center_y = mid_y.clamp(
        content.y + BUTTON_SIZE * 0.5,
        content.y + content.height - BUTTON_SIZE * 0.5,
    );
    let x0 = (mid_x + BUTTON_SPACING).min(content.x + content.width - group_w - BUTTON_SPACING);
    if x0 < content.x + BUTTON_SPACING {
        return None;
    }
    let y0 = center_y - BUTTON_SIZE * 0.5;
    let confirm = Rectangle::new(Point::new(x0, y0), Size::new(BUTTON_SIZE, BUTTON_SIZE));
    let cancel = Rectangle::new(
        Point::new(x0 + BUTTON_SIZE + BUTTON_SPACING, y0),
        Size::new(BUTTON_SIZE, BUTTON_SIZE),
    );
    Some(ShapeButtonRects { confirm, cancel })
}

/// 生成椭圆路径（本 iced 版本无 `Path::ellipse`，改用多边形逼近）
fn ellipse_path(center: Point, rx: f32, ry: f32) -> Path {
    const SEGMENTS: usize = 64;
    Path::new(|p| {
        for i in 0..=SEGMENTS {
            let a = (i as f32 / SEGMENTS as f32) * 2.0 * std::f32::consts::PI;
            let x = center.x + rx * a.cos();
            let y = center.y + ry * a.sin();
            if i == 0 {
                p.move_to(Point::new(x, y));
            } else {
                p.line_to(Point::new(x, y));
            }
        }
        p.close();
    })
}

/// 绘制单个矢量形状（轮廓 + 可选填充），在 `draw` 内联调用（避免泛型渲染器约束）
#[inline]
fn draw_one_shape(
    frame: &mut canvas::Frame<Renderer>,
    editor: &Editor,
    kind: ShapeKind,
    rect: (f32, f32, f32, f32),
    shift: bool,
    filled: bool,
    color: Color,
) {
    // 统一应用屏幕空间 Shift 正图形约束，使预览与生成音符一致
    let px_per_tick = editor.editor_state.view.zoom_x;
    let px_per_key = editor.editor_state.view.zoom_y;
    let rect = effective_rect(kind, rect, shift, px_per_tick, px_per_key);
    match kind {
        ShapeKind::Circle => {
            let (cx0, cy0, cx1, cy1) = rect;
            let mx = (cx0 + cx1) / 2.0;
            let my = (cy0 + cy1) / 2.0;
            let center = editor.line_pos_screen_pos((mx, my));
            let left = editor.line_pos_screen_pos((cx0, my));
            let right = editor.line_pos_screen_pos((cx1, my));
            let top = editor.line_pos_screen_pos((mx, cy1));
            let bottom = editor.line_pos_screen_pos((mx, cy0));
            let rx = (right.x - left.x).abs() * 0.5;
            let ry = (bottom.y - top.y).abs() * 0.5;
            let path = ellipse_path(center, rx, ry);
            if filled {
                let fill = Color::from_rgba(color.r, color.g, color.b, FILL_ALPHA);
                frame.fill(&path, fill);
            }
            let stroke = Stroke::default().with_width(2.5).with_color(color);
            frame.stroke(&path, stroke);
        }
        _ => {
            let verts = match shape_vertices(kind, rect, false, px_per_tick, px_per_key) {
                Some(v) => v,
                None => return,
            };
            let points: Vec<Point> = verts
                .iter()
                .map(|&(t, k)| editor.line_pos_screen_pos((t, k)))
                .collect();
            let path = Path::new(|p| {
                if let Some(first) = points.first() {
                    p.move_to(*first);
                    for pt in points.iter().skip(1) {
                        p.line_to(*pt);
                    }
                    p.close();
                }
            });
            if filled {
                let fill = Color::from_rgba(color.r, color.g, color.b, FILL_ALPHA);
                frame.fill(&path, fill);
            }
            let stroke = Stroke::default().with_width(2.5).with_color(color);
            frame.stroke(&path, stroke);
        }
    }
}

/// 绘制全部待确认图形 + 实时拖拽预览 + 共享 √× 悬浮按钮
///
/// 仅在形状工具激活且有内容（拖拽中或已有待确认图形）时绘制。
pub fn draw(
    editor: &Editor,
    renderer: &Renderer,
    theme: &lumino_ui_core::Theme,
    bounds: Rectangle,
) -> Option<Geometry<Renderer>> {
    // Conductor 音轨（track 0）：整工具不可用，不绘制
    if editor.editor_state.data.current_track == 0 {
        return None;
    }
    if editor.current_tool() != lumino_message::Tool::Shape {
        return None;
    }
    let shape_tool = &editor.editor_state.shape_tool;
    if !shape_tool.has_pending() && !shape_tool.is_dragging() {
        return None;
    }

    let mut frame = canvas::Frame::new(renderer, bounds.size());
    let mut has_content = false;
    let base_color = theme.extended_palette().primary.strong.color;

    // 待确认图形（虚线感由填充/轮廓区分，这里统一实线轮廓）
    for shape in &shape_tool.shapes {
        draw_one_shape(
            &mut frame,
            editor,
            shape.kind,
            shape.rect,
            shape.shift_constrained,
            shape.filled,
            base_color,
        );
        has_content = true;
    }

    // 实时拖拽预览
    if let Some((kind, rect, shift, filled)) = shape_tool.preview_rect(editor.shift_pressed()) {
        draw_one_shape(&mut frame, editor, kind, rect, shift, filled, base_color);
        has_content = true;
    }

    // 共享悬浮按钮（存在待确认图形后显示）
    if let Some(btns) = shape_button_rects(editor) {
        draw_button(
            &mut frame,
            btns.confirm,
            &CONFIRM_ICON,
            Color::from_rgb8(46, 125, 50),
        );
        draw_button(
            &mut frame,
            btns.cancel,
            &CANCEL_ICON,
            Color::from_rgb8(198, 40, 40),
        );
        has_content = true;
    }

    if has_content {
        Some(frame.into_geometry())
    } else {
        None
    }
}
