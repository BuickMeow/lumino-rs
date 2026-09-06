//! 视频导出辅助函数
//!
//! 包含视频帧渲染参数构建、键盘贴图生成、标尺小节号合成等工具函数。

mod counter_font;
mod counter_font_data;
mod counter_font_ttf;
mod counter_font_ttf_load;
mod counter_format;
mod counter_frame;
mod counter_frame_layout;
mod counter_stats;
mod counter_template;
mod data_curve_draw;
mod data_curve_frame;
mod data_curve_math;
mod midi_console;
mod render_params;
mod waterfall_frame;

pub(super) use counter_font::CounterFontRenderer;
pub(super) use counter_frame::CounterFrameInput;
pub(super) use counter_frame::render_counter_frame;
pub(super) use counter_stats::{CounterRenderConfig, CounterStats, current_bpm};
pub(super) use data_curve_frame::{
    DataCurveRenderConfig, DataCurveRenderer, render_data_curve_frame,
};
pub use midi_console::{
    MidiConsoleFrameArgs, MidiConsoleRenderConfig, MidiConsoleRenderer, render_midicomsole_frame,
    render_midicomsole_frame_gpu,
};
pub use render_params::RenderParamsInput;
pub use render_params::SortableNote;
pub use render_params::WindowCollectState;
pub(super) use render_params::build_video_export_render_params;

use self::waterfall_frame::{DIGIT_H, DIGIT_SPACING, DIGIT_W, draw_digit};

pub mod cli_progress;
pub mod keyboard;
pub mod streaming;

// ── 时间计算 ──

/// 从 tempo_changes 计算总时长（秒）
pub(super) fn compute_duration_secs(
    tempo_changes: &[(u32, f32)],
    total_ticks: u32,
    ppq: u32,
) -> f64 {
    if tempo_changes.is_empty() || ppq == 0 {
        return total_ticks as f64 / ppq.max(1) as f64 * 0.5; // 120 BPM
    }
    let mut secs = 0.0;
    let mut prev_tick = 0u32;
    let mut prev_bpm = tempo_changes[0].1 as f64;
    for &(tick, bpm) in tempo_changes {
        if tick > prev_tick {
            let delta_ticks = (tick - prev_tick) as f64;
            secs += delta_ticks / ppq as f64 * 60.0 / prev_bpm;
        }
        prev_tick = tick;
        prev_bpm = bpm as f64;
    }
    if total_ticks > prev_tick {
        let delta_ticks = (total_ticks - prev_tick) as f64;
        secs += delta_ticks / ppq as f64 * 60.0 / prev_bpm;
    }
    secs
}

/// 从秒转换到 tick
pub(super) fn seconds_to_tick(secs: f64, tempo_changes: &[(u32, f32)], ppq: u32) -> u32 {
    if tempo_changes.is_empty() || ppq == 0 {
        return (secs * ppq.max(1) as f64 * 2.0) as u32; // 120 BPM
    }
    let mut remaining = secs;
    let mut prev_tick = 0u32;
    let mut prev_bpm = tempo_changes[0].1 as f64;
    for &(tick, bpm) in tempo_changes {
        if tick > prev_tick {
            let delta_ticks = (tick - prev_tick) as f64;
            let delta_secs = delta_ticks / ppq as f64 * 60.0 / prev_bpm;
            if remaining <= delta_secs {
                return prev_tick + (remaining * ppq as f64 * prev_bpm / 60.0) as u32;
            }
            remaining -= delta_secs;
        }
        prev_tick = tick;
        prev_bpm = bpm as f64;
    }
    prev_tick + (remaining * ppq as f64 * prev_bpm / 60.0) as u32
}

/// 从 tick 转换到秒（tempo 分段积分；默认 120 BPM）
///
/// 统一入口：内存模式 / 流式模式 / 计数器渲染共用此实现，
/// 原 `streaming.rs`、`counter_stats.rs` 中的重复实现已收敛于此。
pub(super) fn ticks_to_seconds(tick: u64, ppqn: u32, tempos: &[(u32, f32)]) -> f64 {
    if ppqn == 0 {
        return tick as f64;
    }
    let mut total_secs = 0.0_f64;
    let mut prev_tick: u32 = 0;
    let mut prev_bpm: f32 = 120.0;

    for &(t, bpm) in tempos {
        let segment_ticks = (t.saturating_sub(prev_tick)) as u64;
        let segment_secs = segment_ticks as f64 * 60.0 / (prev_bpm as f64 * ppqn as f64);
        total_secs += segment_secs;

        if tick <= t as u64 {
            let within_ticks = tick.saturating_sub(prev_tick as u64);
            let within_secs = within_ticks as f64 * 60.0 / (prev_bpm as f64 * ppqn as f64);
            return total_secs - segment_secs + within_secs;
        }

        prev_tick = t;
        prev_bpm = bpm;
    }

    let remaining = tick.saturating_sub(prev_tick as u64);
    total_secs + remaining as f64 * 60.0 / (prev_bpm as f64 * ppqn as f64)
}

// ── 键盘贴图 ──

pub use keyboard::generate_keyboard_texture;

// ── 标尺小节号数字渲染 ──

/// 在 BGRA 帧数据上绘制一个正整数
///
/// 数字渲染在 `(x, y)` 位置，各数字间有 `DIGIT_SPACING` 像素间距。
fn draw_number(
    frame: &mut [u8],
    frame_width: u32,
    number: u32,
    mut x: u32,
    y: u32,
    color: [u8; 4],
) {
    // 使用固定大小数组避免 Vec 堆分配（最大支持 10 位数）
    let mut digits = [0u8; 10];
    let mut len = 0usize;

    if number == 0 {
        digits[0] = 0;
        len = 1;
    } else {
        let mut n = number;
        while n > 0 {
            digits[len] = (n % 10) as u8;
            len += 1;
            n /= 10;
        }
        digits[..len].reverse();
    }

    for &digit in &digits[..len] {
        draw_digit(frame, frame_width, digit, x, y, color);
        x += DIGIT_W + DIGIT_SPACING;
    }
}

/// 将标尺小节号合成到视频帧上（BGRA 格式，in-place 修改）
pub(super) fn composite_ruler_numbers(
    frame: &mut [u8],
    frame_width: u32,
    frame_height: u32,
    scroll_x: f32,
    zoom_x: f32,
    keyboard_width: f32,
    ppq: u32,
) {
    if frame_width == 0 || frame_height == 0 || zoom_x <= 0.0 {
        return;
    }

    let ruler_h = 30u32;
    let ticks_per_measure = (ppq * 4) as f32;

    // 计算可见的小节范围
    let visible_tick_start = scroll_x / zoom_x;
    let visible_tick_end = (scroll_x + frame_width as f32) / zoom_x;

    let measure_start = (visible_tick_start / ticks_per_measure).floor() as u32;
    let measure_end = (visible_tick_end / ticks_per_measure).ceil() as u32;

    // 文本颜色：浅灰（BGRA 格式）
    let text_color: [u8; 4] = [220, 220, 220, 255];

    for measure in measure_start..=measure_end {
        let tick = measure as f32 * ticks_per_measure;
        let screen_x = keyboard_width + tick * zoom_x - scroll_x;

        if screen_x >= keyboard_width && screen_x <= frame_width as f32 {
            // 小节号从 1 开始
            let bar_number = measure + 1;
            // 文本位置：距离刻度线左侧 4px，距离顶部 4px
            let text_x = (screen_x as u32 + 4).min(frame_width.saturating_sub(1));
            let text_y = 4u32.min(ruler_h.saturating_sub(DIGIT_H));

            draw_number(frame, frame_width, bar_number, text_x, text_y, text_color);
        }
    }
}

// ── 键盘合成 ──

pub use keyboard::composite_keyboard;
