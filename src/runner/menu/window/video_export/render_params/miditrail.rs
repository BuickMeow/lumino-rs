//! Miditrail 3D 模式参数（产出统一 `note_instances` + 3D uniforms）
//!
//! 单一权威飞行格式：首帧全量收集（`collect_all`，无窗口过滤）并打包为
//! `NoteInstance`——渲染侧一次上传自有常驻，全局桶建其上；后续帧跳过收集
//!（`note_instances` 为空）只发 uniforms，窗口过滤走 GPU cull 并回读
//!（与旧窗口收集同谓词，legacy 渲染像素逐位一致）。legacy 回退路径
//!（cull 不可用）消费首帧全量（已排序，回退正确）。
//!
//! 旧 390MB 常驻否决项（390MB 显存＋390MB 镜像＋每帧两次全扫 100ms）已消解：
//! 现常驻仅 GPU 侧一份（无 CPU 镜像），CPU 每帧零扫描（cull 在 GPU 侧，
//! 回读 V×16B），UI 侧 collect/sort/pack 稳态归零。

use lumino_gfx::{RenderParams, miditrail_renderer::MiditrailViewMode};
use lumino_message::events::window::video::MiditrailViewMode as EventViewMode;

use super::{MiditrailRenderInput, collect_all_notes, pack_note_instances};

/// 事件层视图枚举 → GPU 层视图枚举（同构映射，无静默降级）。
fn map_view_mode(view: EventViewMode) -> MiditrailViewMode {
    match view {
        EventViewMode::Normal => MiditrailViewMode::Normal,
        EventViewMode::Top => MiditrailViewMode::Top,
    }
}

/// Miditrail 3D 模式参数
pub(crate) fn build_miditrail_render_params(input: MiditrailRenderInput) -> RenderParams {
    let MiditrailRenderInput {
        width,
        height,
        tick,
        document,
        ppq,
        key_count,
        miditrail_speed,
        miditrail_view_mode,
        miditrail_z_far,
        miditrail_3d_notes,
        fps,
        visible_notes,
        note_instances_out,
        window_state,
        collect_all,
    } = input;
    let miditrail_width = width.max(1) as f32;
    let miditrail_height = height.max(1) as f32;
    // 窗口上界不再用于 CPU 收集（首帧全量无过滤，稳态帧跳过）：cull 窗口公式见
    // `miditrail_viewport_span`（渲染侧同函数，保证谓词一致）。

    if collect_all {
        // 首帧全量：无窗口过滤（cull 在 GPU 侧做），排序 + 打包与窗口路径同函数。
        let t_collect = std::time::Instant::now();
        collect_all_notes(document, key_count, visible_notes);
        let collect_us = t_collect.elapsed().as_micros() as u64;

        let t_sort = std::time::Instant::now();
        super::sort_visible_notes(visible_notes, &mut window_state.sort_scratch);
        let sort_us = t_sort.elapsed().as_micros() as u64;
        let t_pack = std::time::Instant::now();
        pack_note_instances(visible_notes, 0, note_instances_out);
        let pack_us = t_pack.elapsed().as_micros() as u64;
        super::diag_window_collect(
            "miditrail-full",
            collect_us,
            sort_us,
            pack_us,
            visible_notes.len(),
        );
    } else {
        // 稳态帧：跳过收集/排序/打包（渲染侧复用 GPU 常驻 + cull），只发 uniforms。
        visible_notes.clear();
        note_instances_out.clear();
    }

    // 光晕环动画时间基准：当前 tick 处每秒 tick 数（BPM × ppq / 60）。
    // 参考 Zenith-MIDI MidiTrailRender 的 tempoFrameStep（每帧 tick 数），
    // 使光晕的按下闪光/收缩动画只随真实时间变化，与滚动速度/帧率无关。
    let ticks_per_second = (ppq as f64 * super::super::current_bpm(&document.tempo_changes, tick)
        / 60.0)
        .max(1.0) as f32;

    RenderParams {
        viewport_size: (width.max(1), height.max(1)),
        logical_size: (miditrail_width, miditrail_height),
        scale_factor: 1.0,
        ppq: ppq as f32,
        max_key_index: (key_count.saturating_sub(1)) as f32,
        canvas_size: (miditrail_width, miditrail_height),
        miditrail_enabled: true,
        miditrail_speed: miditrail_speed.max(0.1),
        miditrail_view_mode: map_view_mode(miditrail_view_mode),
        miditrail_current_tick: tick,
        miditrail_z_far: miditrail_z_far.max(0.1),
        miditrail_3d_notes,
        miditrail_ticks_per_second: ticks_per_second,
        fps,
        note_instances: std::mem::take(note_instances_out),
        time_signatures: document.time_signatures.clone(),
        ..Default::default()
    }
}
