//! 排序键等价性回归测试：u64 打包键 `sort_unstable_by_key` 与旧三键闭包 `sort_by`
//! 输出顺序必须完全一致（视觉零差异回归护栏）。

use super::*;
use crate::is_black_key;

/// 排序等价性：u64 打包键 sort_unstable_by_key 与旧三键闭包 sort_by
/// 输出顺序必须完全一致（视觉零差异回归护栏）。
#[test]
fn test_note_instances_sort_key_equivalent() {
    let mut positions = Vec::new();
    let mut widths = Vec::new();
    let mut last = 0u32;
    update_key_positions(128, &mut last, &mut positions, &mut widths);

    let uniform = MiditrailUniformGpu {
        tick: 500_000,
        ppq: 480,
        speed: 1.0,
        z_far_distance: 7.5,
        ..MiditrailUniformGpu::default()
    };
    // 构造覆盖各种排序情形的音符：黑白键混合、远近交错、同深度（同 z）不同 key
    let mut notes = Vec::new();
    for i in 0..200u32 {
        let key = i % 88;
        let start = uniform.tick + i * 97 % 3_000;
        notes.push(MiditrailNoteGpu {
            key,
            start_tick: start,
            end_tick: start + 240,
            color_packed: 0xFF0000FF,
            track_idx: i % 4,
            velocity: 100,
            channel: i % 16,
            _padding: 0,
        });
    }
    // 完全同键（同 key + 同 start_tick + 同可见性）的和弦叠音：
    // 旧稳定排序按输入序，新排序键相同 → 必须保持输入序（稳定语义回归护栏）。
    // start_tick 设在 tick 附近保证可见（不被 is_visible_at 过滤）。
    for (i, (key, track)) in [(60u32, 1u32), (61, 2), (60, 3)].into_iter().enumerate() {
        let start = uniform.tick.saturating_sub(100) + (i as u32) * 50;
        notes.push(MiditrailNoteGpu {
            key,
            start_tick: start,
            end_tick: start + 240,
            color_packed: 0x00FF00FF,
            track_idx: track,
            velocity: 100,
            channel: 0,
            _padding: 0,
        });
    }

    // 参考：旧三键闭包排序
    let mut expected_instances = Vec::new();
    build_note_instances_old_style(
        &uniform,
        &notes,
        &positions,
        &widths,
        &mut expected_instances,
    );
    // 新实现
    let mut actual_instances = Vec::new();
    let mut scratch = NoteBuildScratch::default();
    build_note_instances(
        &uniform,
        &notes,
        &positions,
        &widths,
        &mut actual_instances,
        &mut scratch,
    );

    assert_eq!(
        expected_instances.len(),
        actual_instances.len(),
        "实例数量必须一致"
    );
    for (i, (a, b)) in expected_instances
        .iter()
        .zip(actual_instances.iter())
        .enumerate()
    {
        assert_eq!(
            a.translation, b.translation,
            "第 {i} 个实例 translation 不一致"
        );
        assert_eq!(a.scale, b.scale, "第 {i} 个实例 scale 不一致");
        assert_eq!(a.color_packed, b.color_packed, "第 {i} 个实例颜色不一致");
        assert_eq!(a.is_key, b.is_key, "第 {i} 个实例 is_key 不一致");
    }
}

/// radix 与 `sort_by_key` 大 N 等价性：5 万条目（含大量重复键测稳定性），
/// 输出下标序列必须逐元素一致。
#[test]
fn test_radix_sort_order_matches_stable_sort() {
    // 确定性伪随机（含重复键，覆盖稳定性路径）。
    let mut state = 0x1234_5678_9ABCu64;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    let n = 50_000usize;
    let mut order: Vec<(u64, u32)> = (0..n as u32).map(|i| (next() % 10_000, i)).collect();
    let mut expected = order.clone();
    expected.sort_by_key(|(k, _)| *k);
    let mut tmp = Vec::new();
    let mut hist = Vec::new();
    super::super::radix_sort_order(&mut order, &mut tmp, &mut hist);
    assert_eq!(order, expected, "radix 输出必须与稳定排序逐元素一致");
}

/// 旧三键闭包排序的参考实现（用于等价性回归测试）。
fn build_note_instances_old_style(
    uniform: &MiditrailUniformGpu,
    notes: &[MiditrailNoteGpu],
    key_positions: &[f32],
    key_widths: &[f32],
    out: &mut Vec<MiditrailInstanceGpu>,
) {
    let tick = uniform.tick;
    let ppq = uniform.ppq.max(1);
    let speed = uniform.speed.max(0.1);
    let ticks_per_measure = ppq * 4;
    let visible_measure_count = ((4.0 / speed).round()).max(1.0) as u32;
    let viewport_tick_span = (ticks_per_measure * visible_measure_count).max(1) as f32;
    let z_far_distance = uniform.z_far_distance.max(0.1);
    let z_far = NOTE_Z_OFFSET - z_far_distance;

    let mut entries: Vec<(u32, f32, MiditrailInstanceGpu)> = Vec::new();
    for note in notes {
        if !note.is_visible_at(tick) {
            continue;
        }
        let key = note.key as usize;
        if key >= key_positions.len() {
            continue;
        }
        let left = key_positions[key];
        let width = key_widths[key];
        let visible_start = note.start_tick.max(tick);
        let visible_end = note.end_tick;
        let z_start = NOTE_Z_OFFSET
            - ((visible_start.saturating_sub(tick)) as f32 / viewport_tick_span
                * MIDITRAIL_SCENE_DEPTH);
        let mut z_end = NOTE_Z_OFFSET
            - ((visible_end.saturating_sub(tick)) as f32 / viewport_tick_span
                * MIDITRAIL_SCENE_DEPTH);
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
    entries.sort_by(|a, b| {
        let a_black = is_black_key(a.0 as isize);
        let b_black = is_black_key(b.0 as isize);
        a_black
            .cmp(&b_black)
            .then_with(|| a.1.total_cmp(&b.1))
            .then_with(|| a.0.cmp(&b.0))
    });
    out.extend(entries.into_iter().map(|(_, _, instance)| instance));
}
