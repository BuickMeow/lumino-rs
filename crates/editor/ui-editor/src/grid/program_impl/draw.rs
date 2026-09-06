//! Program trait `draw` 实现逻辑 — 各图层绘制

use crate::grid::{
    keyboard, playback_indicator, remote_cursors, remote_selection, ruler, selection_box,
};
use crate::{Editor, Renderer, Theme};
use iced_core::Rectangle;
use iced_widget::canvas::Geometry;

pub(crate) fn draw(
    editor: &Editor,
    renderer: &Renderer,
    theme: &Theme,
    bounds: Rectangle,
) -> Vec<Geometry<Renderer>> {
    puffin::profile_scope!("grid_widget_draw");
    crate::puffin_profiler::grid_widget_draw();
    let mut geometries = Vec::new();

    {
        puffin::profile_scope!("draw::keyboard");
        let keyboard_geom = editor
            .keyboard_cache
            .draw(renderer, bounds.size(), |frame| {
                keyboard::draw(editor, frame, bounds, theme);
            });
        geometries.push(keyboard_geom);
    }

    // 洋葱皮颜色覆盖层（不使用缓存，每帧独立绘制）
    {
        puffin::profile_scope!("draw::onion_overlay");
        if let Some(onion_geom) = keyboard::draw_onion_overlay(editor, renderer, bounds) {
            geometries.push(onion_geom);
        }
    }

    // 左上角缝隙（ruler × keyboard 交叉）：填充背景避免透明穿透，对齐 yinhe widgets/quantize_button.rs:30 track_bg
    // 当前阶段仅做填充，后续可在此放量化按钮；用即时 Frame 不走缓存，避免污染 ruler_cache
    {
        puffin::profile_scope!("draw::corner");
        let view = &editor.editor_state.view;
        if view.keyboard_width > 0.0 && view.ruler_height > 0.0 {
            let mut corner_frame = iced_widget::canvas::Frame::new(renderer, bounds.size());
            {
                use crate::grid::theme::ThemeExt;
                use iced_core::{Point, Rectangle, Size};
                use iced_widget::canvas::{Path, Stroke};
                let corner_rect = Rectangle::new(
                    Point::new(0.0, 0.0),
                    Size::new(view.keyboard_width, view.ruler_height),
                );
                let path = Path::rectangle(corner_rect.position(), corner_rect.size());
                corner_frame.fill(&path, theme.ruler_background_color());
                corner_frame.stroke(
                    &path,
                    Stroke::default()
                        .with_width(1.0)
                        .with_color(theme.border_color()),
                );
            }
            geometries.push(corner_frame.into_geometry());
        }
    }

    {
        puffin::profile_scope!("draw::ruler");
        let ruler_geom = editor.ruler_cache.draw(renderer, bounds.size(), |frame| {
            ruler::draw(editor, frame, bounds, theme);
        });
        geometries.push(ruler_geom);
    }

    {
        puffin::profile_scope!("draw::selection_box");
        if let Some(selection_geom) = selection_box::draw(editor, renderer, theme, bounds) {
            geometries.push(selection_geom);
        }
    }

    {
        puffin::profile_scope!("draw::i2m_box");
        if let Some(i2m_geom) = crate::grid::i2m_box::draw(editor, renderer, theme, bounds) {
            geometries.push(i2m_geom);
        }
    }

    {
        puffin::profile_scope!("draw::line_tool_box");
        if let Some(line_geom) = crate::grid::line_tool_box::draw(editor, renderer, theme, bounds) {
            geometries.push(line_geom);
        }
    }

    {
        puffin::profile_scope!("draw::shape_tool_box");
        if let Some(shape_geom) = crate::grid::shape_tool_box::draw(editor, renderer, theme, bounds)
        {
            geometries.push(shape_geom);
        }
    }

    {
        puffin::profile_scope!("draw::text_tool_box");
        if let Some(tt_geom) = crate::grid::text_tool_box::draw(editor, renderer, theme, bounds) {
            geometries.push(tt_geom);
        }
    }

    {
        puffin::profile_scope!("draw::remote_cursors");
        let remote_cursor_geometries = remote_cursors::draw(editor, renderer, bounds);
        geometries.extend(remote_cursor_geometries);
    }

    {
        puffin::profile_scope!("draw::remote_selection");
        let remote_selection_geometries = remote_selection::draw(editor, renderer, bounds);
        geometries.extend(remote_selection_geometries);
    }

    {
        puffin::profile_scope!("draw::playback_indicator");
        let playback_indicator_geom = playback_indicator::draw(editor, renderer, bounds);
        geometries.push(playback_indicator_geom);
    }

    geometries
}
