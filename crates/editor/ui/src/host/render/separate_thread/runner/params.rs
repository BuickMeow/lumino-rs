//! 渲染参数构建 — build_render_params

use crate::RenderParams;
use crate::host::Host;
use crate::host::render::data::{GridColors, RenderData};
use crate::titlebar::mode_toggle::AppMode;
use lumino_gfx::ArrangementUniform;

impl Host {
    /// 构建渲染参数
    pub(crate) fn build_render_params(&mut self, data: RenderData) -> RenderParams {
        use crate::editor::velocity::PANEL_PADDING_Y;
        let es = &self.root.editor.editor_state;
        let physical_size = self.render_ctx.viewport.physical_size();
        let theme = self.root.theme();
        let colors = GridColors::from_theme(&theme);

        let bg_color = [
            colors.bg[0] as f64,
            colors.bg[1] as f64,
            colors.bg[2] as f64,
            colors.bg[3] as f64,
        ];
        let ppq = es.view.ppq;
        let is_arrangement_mode = self.root.is_arrangement_mode();
        let max_key_index = if is_arrangement_mode {
            let av = &self.root.arrangement_view.viewport;
            let track_count = self.root.sidebar.tracks.len().max(1) as f32;
            track_count * av.track_height - av.track_height / 128.0
        } else {
            (es.view.visible_key_count.saturating_sub(1)) as f32
        };

        // 从 collect_viewport_info 获取 canvas_offset 和 canvas_size
        let viewport_info = self.collect_viewport_info();
        let (canvas_offset, canvas_size, keyboard_width, ruler_height) = if is_arrangement_mode {
            (
                (viewport_info.canvas_offset.x, viewport_info.canvas_offset.y),
                (viewport_info.canvas_size.x, viewport_info.canvas_size.y),
                0.0,
                0.0,
            )
        } else {
            (
                (es.canvas.offset_x, es.canvas.offset_y),
                (es.canvas.size_x, es.canvas.size_y),
                es.view.keyboard_width,
                es.view.ruler_height,
            )
        };

        // 构建 arrangement uniform
        let bg_color_arr = colors.bg;
        let bar_color = colors.bar_line;
        let arrangement_uniform = if is_arrangement_mode {
            let av = &self.root.arrangement_view.viewport;
            let track_count = self.root.sidebar.tracks.len().max(1) as f32;
            let mut track_colors = [[0.0_f32; 4]; 16];
            // 使用当前调色板的颜色（来自 PaletteManager），
            // 超出调色板颜色数的轨道循环取色
            for (i, slot) in track_colors.iter_mut().enumerate() {
                *slot = lumino_extras::palette::current_track_color_f32(i);
            }
            let playhead_x = if self.root.editor.playback_position > 0.0 {
                self.root.editor.playback_position * av.zoom_x - data.scroll.0
            } else {
                -1.0
            };
            ArrangementUniform {
                scroll: [data.scroll.0, data.scroll.1],
                zoom: data.zoom.0,
                track_height: av.track_height,
                // 垂直缩放必须透传：音符 GPU 裁剪/绘制管线用 track_height*zoom_y
                // 计算泳道高度，与 CPU 端 lane 背景（arrangement_instances.rs 用同一
                // viewport.zoom_y）保持一致，否则缩放时音符泳道高度与背景失配。
                zoom_y: av.zoom_y,
                viewport_size: [data.viewport_size.width, data.viewport_size.height],
                canvas_offset: [canvas_offset.0, canvas_offset.1],
                playhead_x,
                bg_color: [
                    bg_color_arr[0],
                    bg_color_arr[1],
                    bg_color_arr[2],
                    bg_color_arr[3],
                ],
                bar_color: [bar_color[0], bar_color[1], bar_color[2], bar_color[3]],
                playhead_color: [
                    lumino_gfx::colors::AR_PLAYHEAD_COLOR.0,
                    lumino_gfx::colors::AR_PLAYHEAD_COLOR.1,
                    lumino_gfx::colors::AR_PLAYHEAD_COLOR.2,
                    lumino_gfx::colors::AR_PLAYHEAD_COLOR.3,
                ],
                track_colors,
                track_count,
                ..Default::default()
            }
        } else {
            ArrangementUniform::default()
        };

        // 计算力度面板矩形（用于 wgpu scissor 裁剪）
        // 仅在自动化面板可见时设置，否则跳过 CC bar 渲染器 prepare/draw
        let velocity_panel_rect =
            if is_arrangement_mode || !self.root.sidebar.automation_panel_visible {
                None
            } else {
                let es = &self.root.editor.editor_state;
                // velocity 面板在 grid Canvas 下方，间隔 0px
                const H_SCROLLBAR_HEIGHT: f32 = 20.0;
                Some((
                    es.canvas.offset_x,
                    es.canvas.offset_y + es.canvas.size_y + H_SCROLLBAR_HEIGHT,
                    es.canvas.size_x,
                    self.root.visual.velocity_panel_height + PANEL_PADDING_Y + 10.0,
                ))
            };

        RenderParams::builder()
            .viewport_size((physical_size.width, physical_size.height))
            .logical_size((data.viewport_size.width, data.viewport_size.height))
            .scale_factor(self.render_ctx.viewport.scale_factor())
            .scroll(data.scroll)
            .zoom(data.zoom)
            .keyboard_width(keyboard_width)
            .ruler_height(ruler_height)
            .canvas_offset((canvas_offset.0, canvas_offset.1))
            .canvas_size((canvas_size.0, canvas_size.1))
            .background_color(bg_color)
            .color_bg(colors.bg)
            .color_bg_black_key(colors.black_key)
            .color_bar(colors.bar_line)
            .color_beat(colors.beat_line)
            .color_half_beat(colors.half_beat_line)
            .color_grid(colors.grid_line)
            .color_key_line(colors.key_line)
            .ppq(ppq as f32)
            .max_key_index(max_key_index)
            .is_arrangement_mode(is_arrangement_mode)
            .ruler_instances(data.ruler_instances)
            .time_signatures(es.data.time_signatures.clone())
            .arrangement_overlay_instances(data.arrangement_overlay_instances)
            .arrangement_overlay_back_len(data.arrangement_overlay_back_len)
            .arrangement_track_order(
                data.arrangement_track_order
                    .iter()
                    .map(|&t| t as u32)
                    .collect(),
            )
            .arrangement_track_visible(data.arrangement_track_visible)
            .arrangement_uniform(arrangement_uniform)
            .cc_bar_instances(data.cc_bar_instances)
            .velocity_panel_rect(velocity_panel_rect)
            .skip_scene_render(self.root.state.current_mode == AppMode::Waterfall)
            .is_vertical_roll(self.root.editor.editor_state.is_vertical_roll)
            .build()
    }
}
