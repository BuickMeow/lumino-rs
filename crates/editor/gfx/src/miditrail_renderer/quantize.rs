//! Top 视图音符时间量化：逐音起止对齐，不合并
//!
//! Top 为俯视（参考 Comet MIDITrail `Top Down Above` 预设）。本模块只做一件事：
//! 把可见音符的起止 tick 对齐到 [`TOP_QUANT_TICKS`] 网格（边缘 artifact 即来源
//! 于此，验收时需目视确认可接受）。
//!
//! 关键不变量：**永不合并**。任何合并（无论在量化域还是原始域判定）都会改写
//! 音符时值：量化域合并曾把间隙 < 2 步长的断奏链式吞成超长音符（“密集段拉长”
//! bug）；原始域合并虽保间隙，但仍会吞掉连奏/重叠音的各自颜色与边界。用户已
//! 明确合并不可取——本模块输出与可见输入一一对应，只降对齐精度，不改音符个数。
//! - Normal 路径不经过本模块（零改动，防两套设置互相污染）。

use super::types::{MiditrailNoteGpu, MiditrailUniformGpu};

/// Top 视图时间量化步长（tick；ppq=480 时相当于 32 分音符）。
///
/// 步长越大边缘 artifact 越明显；96 为经验折中
/// （120BPM 下约 25ms，远小于人眼可察觉的时值偏差）。
pub const TOP_QUANT_TICKS: u32 = 96;

/// 对可见音符做 Top 视图降精度：按 `tick` 过滤可见音符 → 逐音起止量化到网格。
///
/// 输出与可见输入一一对应（保序：与输入顺序一致），可直接送入
/// `build_note_instances`（几何映射与 Comet 绘制顺序排序逻辑复用，不另起一套）。
#[must_use]
pub fn quantize_notes_for_top(
    uniform: &MiditrailUniformGpu,
    notes: &[MiditrailNoteGpu],
) -> Vec<MiditrailNoteGpu> {
    let tick = uniform.tick;
    let quant = TOP_QUANT_TICKS.max(1);

    let mut out = Vec::with_capacity(notes.len());
    for note in notes {
        if !note.is_visible_at(tick) {
            continue;
        }
        if note.key as usize >= 128 {
            continue;
        }
        out.push(with_quantized_span(*note, quant));
    }
    out
}

/// 用量化起止替换音符的 tick 范围，其余字段原样保留（颜色/力度/通道不动）。
fn with_quantized_span(note: MiditrailNoteGpu, quant: u32) -> MiditrailNoteGpu {
    let start = note.start_tick;
    let end = note.end_tick.max(start + 1);
    let start_q = start - start % quant;
    let end_q = end.saturating_add((quant - end % quant) % quant);
    MiditrailNoteGpu {
        start_tick: start_q,
        end_tick: end_q.max(start_q + 1),
        ..note
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(key: u32, start: u32, end: u32, color: u32) -> MiditrailNoteGpu {
        MiditrailNoteGpu {
            key,
            start_tick: start,
            end_tick: end,
            color_packed: color,
            track_idx: 0,
            velocity: 100,
            channel: 0,
            _padding: 0,
        }
    }

    fn uniform_at(tick: u32) -> MiditrailUniformGpu {
        MiditrailUniformGpu {
            tick,
            ..MiditrailUniformGpu::default()
        }
    }

    #[test]
    fn test_top_quantize_never_merges_overlapping() {
        // 合并已删除：重叠的同键音符各自对齐、各自保留颜色。
        let notes = vec![
            note(60, 0, 200, 0xFF0000FF),
            note(60, 100, 300, 0x00FF00FF),
            note(60, 500, 600, 0x0000FFFF),
        ];
        let out = quantize_notes_for_top(&uniform_at(0), &notes);
        assert_eq!(out.len(), 3, "永不合并，实际 {out:?}");
        assert_eq!((out[0].start_tick, out[0].end_tick), (0, 288));
        assert_eq!((out[1].start_tick, out[1].end_tick), (96, 384));
        assert_eq!((out[2].start_tick, out[2].end_tick), (480, 672));
        // 各自颜色原样保留（重叠音不再被首个区间吞掉）。
        assert_eq!(out[0].color_packed, 0xFF0000FF);
        assert_eq!(out[1].color_packed, 0x00FF00FF);
        assert_eq!(out[2].color_packed, 0x0000FFFF);
    }

    #[test]
    fn test_top_quantize_keeps_staccato_separate() {
        // 回归：“密集段拉长” bug —— 合并已彻底删除，断奏天然独立。
        let notes = vec![
            note(60, 0, 100, 1),
            note(60, 110, 210, 2), // 间隙仅 10tick
            note(60, 220, 320, 3),
            note(60, 400, 500, 4), // 间隙 80tick（< 96 步长）
        ];
        let out = quantize_notes_for_top(&uniform_at(0), &notes);
        assert_eq!(out.len(), 4, "断奏不应合并，实际 {out:?}");
        // 各自对齐到网格，间隙依然存在（允许单音 ±95tick 的声明 artifact）。
        assert_eq!((out[0].start_tick, out[0].end_tick), (0, 192));
        assert_eq!((out[1].start_tick, out[1].end_tick), (96, 288));
        assert_eq!(out[0].color_packed, 1, "未合并时颜色保持各自的");
        assert_eq!(out[1].color_packed, 2);
    }

    #[test]
    fn test_top_quantize_snaps_edges_to_grid() {
        let out = quantize_notes_for_top(&uniform_at(0), &[note(60, 10, 200, 1)]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].start_tick, 0, "起始应向下取整到网格");
        assert_eq!(out[0].end_tick, 288, "结束应向上取整到网格");
    }

    #[test]
    fn test_top_quantize_keeps_touching_legato_separate() {
        // 合并已删除：首尾相接的连奏也各自独立，只做对齐。
        let notes = vec![note(60, 0, 200, 7), note(60, 200, 400, 8)];
        let out = quantize_notes_for_top(&uniform_at(0), &notes);
        assert_eq!(out.len(), 2, "永不合并，实际 {out:?}");
        assert_eq!((out[0].start_tick, out[0].end_tick), (0, 288));
        assert_eq!((out[1].start_tick, out[1].end_tick), (192, 480));
        assert_eq!(out[0].color_packed, 7);
        assert_eq!(out[1].color_packed, 8);
    }

    #[test]
    fn test_top_quantize_keeps_keys_separate() {
        let notes = vec![note(60, 0, 500, 1), note(61, 0, 500, 2)];
        let out = quantize_notes_for_top(&uniform_at(0), &notes);
        assert_eq!(out.len(), 2, "不同键永不合并");
    }

    #[test]
    fn test_top_quantize_filters_invisible_and_clamps_key() {
        let notes = vec![
            note(60, 0, 100, 1),   // tick=1000 时已结束
            note(200, 0, 2000, 2), // 非法 key
            note(61, 900, 2000, 3),
        ];
        let out = quantize_notes_for_top(&uniform_at(1000), &notes);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].key, 61);
    }

    #[test]
    fn test_top_quantize_preserves_visible_count() {
        // 合并已删除：输出与可见输入一一对应（只过滤不可见/非法键）。
        // 确定性伪随机密集输入：tick=0 时全部可见（end > 0），键 60–67 合法。
        let mut state = 0x9E37_79B9_7F4A_7C15u64;
        let mut next = move || {
            state ^= state >> 12;
            state ^= state << 25;
            state ^= state >> 27;
            state.wrapping_mul(0x2545_F491_4F6C_DD1D)
        };
        let notes: Vec<MiditrailNoteGpu> = (0..5000)
            .map(|_| {
                let start = (next() % 50_000) as u32;
                note(
                    (next() % 8) as u32 + 60,
                    start,
                    start + (next() % 960) as u32 + 1,
                    1,
                )
            })
            .collect();
        let out = quantize_notes_for_top(&uniform_at(0), &notes);
        assert_eq!(
            out.len(),
            notes.len(),
            "无合并无丢失：{} → {}",
            notes.len(),
            out.len()
        );
    }
}
