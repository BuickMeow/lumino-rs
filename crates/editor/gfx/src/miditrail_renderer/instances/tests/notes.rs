//! 音符实例测试（深度排序 / 黑白键分组 / Z 远平面裁剪 / 窗口缩放等价性）

use super::*;

#[test]
fn test_note_instances_sorted_by_depth() {
    let mut positions = Vec::new();
    let mut widths = Vec::new();
    let mut last = 0u32;
    update_key_positions(128, &mut last, &mut positions, &mut widths);

    let uniform = MiditrailUniformGpu::default();
    let notes = vec![
        MiditrailNoteGpu {
            key: 60,
            start_tick: 0,
            end_tick: 1000,
            color_packed: 0xFF0000FF,
            track_idx: 0,
            velocity: 100,
            channel: 0,
            _padding: 0,
        },
        MiditrailNoteGpu {
            key: 64,
            start_tick: 1000,
            end_tick: 2000,
            color_packed: 0x00FF00FF,
            track_idx: 0,
            velocity: 100,
            channel: 0,
            _padding: 0,
        },
    ];
    let mut out = Vec::new();
    let mut scratch = NoteBuildScratch::default();
    build_note_instances(
        &uniform,
        &notes,
        &positions,
        &widths,
        &mut out,
        &mut scratch,
    );
    assert_eq!(out.len(), 2);
    let front_z = |i: &MiditrailInstanceGpu| i.translation[2] + i.scale[2];
    assert!(
        front_z(&out[0]) <= front_z(&out[1]),
        "音符应按 far-to-near 排序，远的先绘制"
    );
}

#[test]
fn test_note_instances_grouped_by_key_color() {
    let mut positions = Vec::new();
    let mut widths = Vec::new();
    let mut last = 0u32;
    update_key_positions(128, &mut last, &mut positions, &mut widths);

    let uniform = MiditrailUniformGpu::default();
    // 黑键音符更远，白键音符更近；按 Comet 顺序应白键先绘制、黑键后绘制。
    let notes = vec![
        MiditrailNoteGpu {
            key: 61, // C#，黑键
            start_tick: 1000,
            end_tick: 2000,
            color_packed: 0xFFFF0000,
            track_idx: 0,
            velocity: 100,
            channel: 0,
            _padding: 0,
        },
        MiditrailNoteGpu {
            key: 60, // C，白键
            start_tick: 0,
            end_tick: 1000,
            color_packed: 0x00FF0000,
            track_idx: 0,
            velocity: 100,
            channel: 0,
            _padding: 0,
        },
    ];
    let mut out = Vec::new();
    let mut scratch = NoteBuildScratch::default();
    build_note_instances(
        &uniform,
        &notes,
        &positions,
        &widths,
        &mut out,
        &mut scratch,
    );
    assert_eq!(out.len(), 2);

    let white_left = positions[60] + widths[60] * 0.04;
    let black_left = positions[61] + widths[61] * 0.04;
    eprintln!("white_left={}, black_left={}", white_left, black_left);
    for (i, inst) in out.iter().enumerate() {
        eprintln!("out[{}].translation[0] = {}", i, inst.translation[0]);
    }
    assert!(
        (out[0].translation[0] - white_left).abs() < 1e-5,
        "白键音符应先绘制"
    );
    assert!(
        (out[1].translation[0] - black_left).abs() < 1e-5,
        "黑键音符应后绘制，覆盖白键音符"
    );
}

#[test]
fn test_note_instances_clipped_at_z_far_distance() {
    let mut positions = Vec::new();
    let mut widths = Vec::new();
    let mut last = 0u32;
    update_key_positions(128, &mut last, &mut positions, &mut widths);

    let uniform = MiditrailUniformGpu {
        z_far_distance: 3.0,
        ..MiditrailUniformGpu::default()
    };
    // 音符从当前 tick 开始，长度刚好覆盖整个视口，因此无裁剪时会延伸到 -SCENE_DEPTH。
    let notes = vec![MiditrailNoteGpu {
        key: 60,
        start_tick: 0,
        end_tick: 480 * 16,
        color_packed: 0xFF0000FF,
        track_idx: 0,
        velocity: 100,
        channel: 0,
        _padding: 0,
    }];
    let mut out = Vec::new();
    let mut scratch = NoteBuildScratch::default();
    build_note_instances(
        &uniform,
        &notes,
        &positions,
        &widths,
        &mut out,
        &mut scratch,
    );
    assert_eq!(out.len(), 1);
    // 立方体锚定在 translation，向 +Z 方向延伸 scale[2]，因此后端直接为 translation[2]。
    let back_z = out[0].translation[2];
    let expected_z_far = NOTE_Z_OFFSET - uniform.z_far_distance;
    assert!(
        (back_z - expected_z_far).abs() < 1e-5,
        "音符应被截断到 Z 远平面，期望 {expected_z_far}, 实际 {back_z}"
    );
}

/// 窗口缩放等价性：2.0× 收集窗口（旧行为）与 1.0× 窗口（新行为）输入下，
/// `build_note_instances` 输出必须完全一致——即缩小收集窗口不漏掉任何可见音符。
#[test]
fn test_note_instances_window_scale_equivalence() {
    let mut positions = Vec::new();
    let mut widths = Vec::new();
    let mut last = 0u32;
    update_key_positions(128, &mut last, &mut positions, &mut widths);

    let uniform = MiditrailUniformGpu {
        tick: 1_000_000,
        ppq: 480,
        speed: 1.0,
        z_far_distance: 7.5, // 默认：z_far_scale = 1.0
        ..MiditrailUniformGpu::default()
    };
    // span = ppq*4 * (4/speed) = 7680
    let span = 7680u32;

    // 边界音符：分布在 [tick, tick + 2.0×span] 区间
    let mut notes = Vec::new();
    for (i, off) in [
        0u32,           // 正好当前 tick
        span / 4,       // 0.25×
        span / 2,       // 0.5×
        span * 3 / 4,   // 0.75×
        span,           // 1.0×（窗口边界，新行为恰好覆盖）
        span * 6 / 5,   // 1.2×（新行为窗口外）
        span * 3 / 2,   // 1.5×
        span * 19 / 10, // 1.9×
        span * 2,       // 2.0×（旧行为窗口边界）
    ]
    .into_iter()
    .enumerate()
    {
        notes.push(MiditrailNoteGpu {
            key: 40 + i as u32,
            start_tick: uniform.tick + off,
            end_tick: uniform.tick + off + 240,
            color_packed: 0xFF0000FF,
            track_idx: 0,
            velocity: 100,
            channel: 0,
            _padding: 0,
        });
    }

    // 1.0× 窗口输入：只包含 start_tick <= tick + 1.0×span 的音符
    let notes_1x: Vec<MiditrailNoteGpu> = notes
        .iter()
        .filter(|n| n.start_tick <= uniform.tick + span)
        .copied()
        .collect();
    assert!(notes_1x.len() < notes.len(), "1.0× 窗口应裁剪掉部分音符");

    let mut out_full = Vec::new();
    let mut scratch = NoteBuildScratch::default();
    build_note_instances(
        &uniform,
        &notes,
        &positions,
        &widths,
        &mut out_full,
        &mut scratch,
    );
    let mut out_1x = Vec::new();
    build_note_instances(
        &uniform,
        &notes_1x,
        &positions,
        &widths,
        &mut out_1x,
        &mut scratch,
    );

    assert_eq!(
        out_1x.len(),
        out_full.len(),
        "缩小收集窗口后可见音符数必须一致：1x={}, 2x={}",
        out_1x.len(),
        out_full.len()
    );
    for (a, b) in out_1x.iter().zip(out_full.iter()) {
        assert_eq!(a.translation, b.translation, "translation 必须一致");
        assert_eq!(a.scale, b.scale, "scale 必须一致");
        assert_eq!(a.color_packed, b.color_packed, "颜色必须一致");
    }
}
