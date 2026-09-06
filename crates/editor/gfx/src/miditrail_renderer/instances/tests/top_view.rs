//! Top 视图实例构建测试：量化一致性与 Normal 等价
//!
//! Top 路径 = 时间量化（`quantize`，逐音对齐、永不合并）+ 复用
//! `build_note_instances` 几何/排序逻辑；本模块用数据证明 Top 与 Normal
//! 渲染同一音符集合（只降对齐精度，不丢音、不吞音）。

use super::super::super::quantize::{TOP_QUANT_TICKS, quantize_notes_for_top};
use super::*;
use std::collections::BTreeSet;

fn uniform_at(tick: u32) -> MiditrailUniformGpu {
    MiditrailUniformGpu {
        tick,
        ppq: 480,
        speed: 1.0,
        z_far_distance: 7.5,
        ..MiditrailUniformGpu::default()
    }
}

fn layout() -> (Vec<f32>, Vec<f32>) {
    let mut positions = Vec::new();
    let mut widths = Vec::new();
    let mut last = 0u32;
    update_key_positions(128, &mut last, &mut positions, &mut widths);
    (positions, widths)
}

/// 密集fixture：8 个键 × 300 个重叠音符（模拟高密度段落视口负载）。
fn dense_notes() -> Vec<MiditrailNoteGpu> {
    let mut notes = Vec::with_capacity(2400);
    for key in 60..68u32 {
        for i in 0..300u32 {
            let start = i * 30;
            notes.push(MiditrailNoteGpu {
                key,
                start_tick: start,
                end_tick: start + 240,
                color_packed: 0xFF0000FF,
                track_idx: 0,
                velocity: 100,
                channel: 0,
                _padding: 0,
            });
        }
    }
    notes
}

fn build_all(
    uniform: &MiditrailUniformGpu,
    notes: &[MiditrailNoteGpu],
) -> Vec<MiditrailInstanceGpu> {
    let (positions, widths) = layout();
    let mut out = Vec::new();
    let mut scratch = NoteBuildScratch::default();
    build_note_instances(uniform, notes, &positions, &widths, &mut out, &mut scratch);
    out
}

/// Top 实例数与 Normal 完全一致（合并已删除：同一音符集合，只差网格对齐）。
#[test]
fn test_top_instances_match_normal() {
    let uniform = uniform_at(0);
    let notes = dense_notes();
    let normal = build_all(&uniform, &notes);
    let quantized = quantize_notes_for_top(&uniform, &notes);
    let top = build_all(&uniform, &quantized);
    // Normal 自带 z_far 裁剪（超显示距离的音符不建实例），只断言相对关系。
    assert!(!normal.is_empty(), "密集输入 Normal 应有实例");
    assert_eq!(
        top.len(),
        normal.len(),
        "Top 与 Normal 应渲染同一音符集合：{} vs {}",
        top.len(),
        normal.len()
    );
}

/// Top 只降精度不丢音：覆盖的键集合与 Normal 一致。
#[test]
fn test_top_covers_same_keys_as_normal() {
    let uniform = uniform_at(0);
    let notes = dense_notes();
    let quantized = quantize_notes_for_top(&uniform, &notes);
    let normal_keys: BTreeSet<u32> = notes.iter().map(|n| n.key).collect();
    let top_keys: BTreeSet<u32> = quantized.iter().map(|n| n.key).collect();
    assert_eq!(top_keys, normal_keys, "Top 不应丢失任何键的音符");
}

/// 量化输出的起止均对齐到网格（边缘 artifact 的来源，验收时目视确认）。
#[test]
fn test_top_quantized_output_aligned_to_grid() {
    let uniform = uniform_at(0);
    let quantized = quantize_notes_for_top(&uniform, &dense_notes());
    assert!(!quantized.is_empty());
    for n in &quantized {
        assert_eq!(n.start_tick % TOP_QUANT_TICKS, 0, "起始未对齐网格");
        assert_eq!(n.end_tick % TOP_QUANT_TICKS, 0, "结束未对齐网格");
        assert!(n.end_tick > n.start_tick, "量化后音符不应退化为空");
    }
}
