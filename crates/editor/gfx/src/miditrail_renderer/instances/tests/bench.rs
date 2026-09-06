//! 性能基准：10 万密集音符（视频导出场景）下实例构建总耗时。
//!
//! 该测试不断言耗时（CI 机器抖动不可控），仅输出测量数据供人工比对：
//! - 优化前基线：全量扫描 ×2 + 每帧 O(V log V) 排序
//! - 优化后目标：单次扫描 + 桶内有序（排序消除或降级）
//!
//! 运行：`cargo test -p lumino-gfx test_miditrail_instances_bench -- --nocapture`

use super::*;
use crate::is_black_key;

#[test]
fn test_miditrail_instances_bench() {
    use std::time::Instant;

    let mut positions = Vec::new();
    let mut widths = Vec::new();
    let mut last = 0u32;
    update_key_positions(128, &mut last, &mut positions, &mut widths);

    let uniform = MiditrailUniformGpu {
        tick: 1_000_000,
        ppq: 480,
        key_count: 128,
        speed: 1.0,
        z_far_distance: 7.5,
        fps: 60.0,
        ..MiditrailUniformGpu::default()
    };

    // 10 万音符：密集分布在旧收集窗口（2.0× span + TICK_SEARCH_BUFFER 下界）内，
    // 模拟 collect_visible_notes_for_gpu 的输出
    const N: usize = 100_000;
    let span = 7680u32;
    let window_start = uniform.tick.saturating_sub(19_200);
    let window_end = uniform.tick + span * 2; // 旧 2.0× 窗口
    let mut notes = Vec::with_capacity(N);
    for i in 0..N {
        let t = window_start + (i as u64 * (window_end - window_start) as u64 / N as u64) as u32;
        notes.push(MiditrailNoteGpu {
            key: i as u32 % 88,
            start_tick: t,
            end_tick: t + 240,
            color_packed: 0xFF0000FF,
            track_idx: 0,
            velocity: 100,
            channel: 0,
            _padding: 0,
        });
    }
    // 新窗口输入：1.0× span 上界（start_tick <= tick + span）
    let notes_1x: Vec<MiditrailNoteGpu> = notes
        .iter()
        .filter(|n| n.start_tick <= uniform.tick + span)
        .copied()
        .collect();
    eprintln!(
        "[miditrail_bench] 窗口缩放: 旧2.0×={} 音符, 新1.0×={} 音符 (-{:.0}%)",
        notes.len(),
        notes_1x.len(),
        (1.0 - notes_1x.len() as f64 / notes.len() as f64) * 100.0
    );

    // 热身
    let mut scratch = NoteBuildScratch::default();
    for _ in 0..3 {
        let active_keys = compute_active_keys(uniform.tick, &notes_1x);
        let mut out = Vec::with_capacity(notes_1x.len());
        build_note_instances(
            &uniform,
            &notes_1x,
            &positions,
            &widths,
            &mut out,
            &mut scratch,
        );
        std::hint::black_box((active_keys, out));
    }

    const ITERS: u32 = 30;
    let mut t_active = 0u64;
    let mut t_build = 0u64;
    let mut t_build_1x = 0u64;
    let mut t_visible = 0u64;
    let mut t_sort = 0u64;
    let mut t_sort_x_total = 0u64;
    let mut t_sort_s_total = 0u64;
    let mut scratch = NoteBuildScratch::default();
    for _ in 0..ITERS {
        let t0 = Instant::now();
        let active_keys = compute_active_keys(uniform.tick, &notes);
        t_active += t0.elapsed().as_micros() as u64;

        // 旧行为：2.0× 窗口全量输入
        let t1 = Instant::now();
        let mut out = Vec::with_capacity(notes.len());
        build_note_instances(
            &uniform,
            &notes,
            &positions,
            &widths,
            &mut out,
            &mut scratch,
        );
        t_build += t1.elapsed().as_micros() as u64;

        // 新行为：1.0× 窗口输入
        let t2 = Instant::now();
        let mut out2 = Vec::with_capacity(notes_1x.len());
        build_note_instances(
            &uniform,
            &notes_1x,
            &positions,
            &widths,
            &mut out2,
            &mut scratch,
        );
        t_build_1x += t2.elapsed().as_micros() as u64;

        // 内部拆解：遍历+可见过滤 vs 排序（仅可见实例参与排序）
        let mut entries: Vec<(u32, f32, MiditrailInstanceGpu)> = Vec::with_capacity(notes_1x.len());
        let t3 = Instant::now();
        let tick = uniform.tick;
        let ppq = uniform.ppq.max(1);
        let speed = uniform.speed.max(0.1);
        let vtm = (ppq * 4 * ((4.0 / speed).round()).max(1.0) as u32).max(1) as f32;
        let z_far = NOTE_Z_OFFSET - uniform.z_far_distance.max(0.1);
        let positions = &positions;
        let widths = &widths;
        for note in &notes_1x {
            if !note.is_visible_at(tick) {
                continue;
            }
            let key = note.key as usize;
            if key >= positions.len() {
                continue;
            }
            let left = positions[key];
            let width = widths[key];
            let visible_start = note.start_tick.max(tick);
            let visible_end = note.end_tick;
            let z_start = NOTE_Z_OFFSET
                - ((visible_start.saturating_sub(tick)) as f32 / vtm * MIDITRAIL_SCENE_DEPTH);
            let mut z_end = NOTE_Z_OFFSET
                - ((visible_end.saturating_sub(tick)) as f32 / vtm * MIDITRAIL_SCENE_DEPTH);
            z_end = z_end.max(z_far);
            if z_end >= z_start {
                continue;
            }
            let z_center = (z_start + z_end) * 0.5;
            let z_length = z_start - z_end;
            let scale = [width * 0.92, NOTE_HEIGHT, z_length];
            let translation = [left + width * 0.04, NOTE_Y, z_center - z_length * 0.5];
            let color = if note.is_active_at(tick) {
                boost_color_packed(note.color_packed, 0.5)
            } else {
                note.color_packed
            };
            entries.push((
                note.key,
                z_start,
                MiditrailInstanceGpu::new(translation, scale, color, false, 0.0, 0.0),
            ));
        }
        t_visible += t3.elapsed().as_micros() as u64;
        let t4 = Instant::now();
        entries.sort_by(|a, b| {
            let a_black = is_black_key(a.0 as isize);
            let b_black = is_black_key(b.0 as isize);
            a_black
                .cmp(&b_black)
                .then_with(|| a.1.total_cmp(&b.1))
                .then_with(|| a.0.cmp(&b.0))
        });
        t_sort += t4.elapsed().as_micros() as u64;
        std::hint::black_box(entries);

        // 方案 X：单一 u64 键 sort_unstable_by_key
        let mut entries_x: Vec<(u64, MiditrailInstanceGpu)> = Vec::new();
        let tick = uniform.tick;
        let ppq = uniform.ppq.max(1);
        let speed = uniform.speed.max(0.1);
        let vtm = (ppq * 4 * ((4.0 / speed).round()).max(1.0) as u32).max(1) as f32;
        let z_far = NOTE_Z_OFFSET - uniform.z_far_distance.max(0.1);
        for note in &notes_1x {
            if !note.is_visible_at(tick) {
                continue;
            }
            let key = note.key as usize;
            if key >= positions.len() {
                continue;
            }
            let left = positions[key];
            let width = widths[key];
            let visible_start = note.start_tick.max(tick);
            let visible_end = note.end_tick;
            let z_start = NOTE_Z_OFFSET
                - ((visible_start.saturating_sub(tick)) as f32 / vtm * MIDITRAIL_SCENE_DEPTH);
            let mut z_end = NOTE_Z_OFFSET
                - ((visible_end.saturating_sub(tick)) as f32 / vtm * MIDITRAIL_SCENE_DEPTH);
            z_end = z_end.max(z_far);
            if z_end >= z_start {
                continue;
            }
            let z_center = (z_start + z_end) * 0.5;
            let z_length = z_start - z_end;
            let scale = [width * 0.92, NOTE_HEIGHT, z_length];
            let translation = [left + width * 0.04, NOTE_Y, z_center - z_length * 0.5];
            let color = if note.is_active_at(tick) {
                boost_color_packed(note.color_packed, 0.5)
            } else {
                note.color_packed
            };
            // f32 位重排 → 可排序 u32（与 total_cmp 全序等价）：
            // 正数：最高位翻转；负数：全位翻转
            let zb = z_start.to_bits();
            let z_sortable = if zb & 0x8000_0000 != 0 {
                !zb
            } else {
                zb ^ 0x8000_0000
            };
            let packed = ((is_black_key(note.key as isize) as u64) << 63)
                | ((z_sortable as u64) << 7)
                | (note.key as u64);
            entries_x.push((
                packed,
                MiditrailInstanceGpu::new(translation, scale, color, false, 0.0, 0.0),
            ));
        }
        let t5 = Instant::now();
        entries_x.sort_unstable_by_key(|(packed, _)| *packed);
        let t_sort_x = t5.elapsed().as_micros() as u64;
        std::hint::black_box(&entries_x);
        t_sort_x_total += t_sort_x;

        // 方案 X2：稳定 sort_by_key（保留旧稳定语义，避免同键叠音顺序不确定）
        let mut entries_s = entries_x.clone();
        let t6 = Instant::now();
        entries_s.sort_by_key(|(packed, _)| *packed);
        let t_sort_s = t6.elapsed().as_micros() as u64;
        std::hint::black_box(entries_s);
        t_sort_s_total += t_sort_s;

        std::hint::black_box((active_keys, out, out2));
    }
    eprintln!(
        "[miditrail_bench] active {:.2}ms | 旧2.0×build {:.2}ms | 新1.0×build {:.2}ms (省 {:.1}%) | 遍历+过滤 {:.2}ms | sort_by三键 {:.2}ms | unstable_by_key {:.2}ms | stable_by_key {:.2}ms",
        t_active as f64 / ITERS as f64 / 1000.0,
        t_build as f64 / ITERS as f64 / 1000.0,
        t_build_1x as f64 / ITERS as f64 / 1000.0,
        (1.0 - t_build_1x as f64 / t_build as f64) * 100.0,
        t_visible as f64 / ITERS as f64 / 1000.0,
        t_sort as f64 / ITERS as f64 / 1000.0,
        t_sort_x_total as f64 / ITERS as f64 / 1000.0,
        t_sort_s_total as f64 / ITERS as f64 / 1000.0
    );
}
