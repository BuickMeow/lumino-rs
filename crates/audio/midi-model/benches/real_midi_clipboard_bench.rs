//! 真实 MIDI 大数据复制/粘贴基准（JSON 旧路径 vs 紧凑二进制优化路径）
//!
//! 复现性能日志里的「复制 2s / 粘贴 9s」：走带剪贴板用 JSON 序列化 100W+ 音符，
//! 复制端 `write!` 拼 1M 个音符对象、粘贴端 `serde_json` 解析 1M 个对象，是纯 CPU 悬崖。
//!
//! 优化路径：复用 `midi-model` 的紧凑二进制剪贴板（`encode_clipboard` / `decode_clipboard_chunks`），
//! 与钢琴卷帘已采用的快速路径一致——delta 变长 tick + 定长字段，粘贴端按音轨分组批量插入。
//!
//! 数据：默认读真实文件 `Ouranos - HDSQ & The Romanticist [v1.6.6].mid`（24,337,991 音符）。
//! 基准主体取「1,043,936 音符的选区」——正是性能日志里复制/粘贴的实际量级（复制 2s / 粘贴 9s）。
//! 另附全文件 24M 音符的二进制上下文行（JSON 在 24M 量级约需 3 分钟，不列入常规基准）。
//!
//! 运行：
//! ```bash
//! cargo bench -p lumino-midi-model --bench real_midi_clipboard_bench
//! ```
//!
//! 目标：复制 / 粘贴均 < 100ms（硬指标），优秀线 < 50ms（针对 1M 选区量级）。

use std::env;
use std::path::Path;
use std::time::Instant;

use lumino_midi_model::MidiDocument;
use lumino_midi_model::NoteEvent;
use lumino_midi_model::clipboard::{
    ClipRecord, decode_clipboard_records, encode_clipboard, parse_clipboard_header,
};

/// 真实 MIDI 文件路径（用户提供的性能日志来源文件）。
const DEFAULT_MIDI: &str = r"D:\BM-DATA\MIDI File\Ouranos - HDSQ & The Romanticist [v1.6.6].mid";

/// 选区量级：对齐日志「已粘贴 1043936 个音符」（复制 2s / 粘贴 9s 的实际数据量）。
const SELECTION_NOTES: u32 = 1_043_936;

/// 目标阈值（硬指标）：复制 / 粘贴各自 < 100ms。
const TARGET_MS: f64 = 100.0;
/// 优秀线：< 50ms。
const EXCELLENT_MS: f64 = 50.0;

/// 加载真实 MIDI；文件缺失则回退合成数据（合成 1,043,936 音符选区 + 等价全量）。
fn load_doc() -> (MidiDocument, String) {
    let path = env::var("LUMINO_BENCH_MIDI").unwrap_or_else(|_| DEFAULT_MIDI.to_string());
    if Path::new(&path).exists() {
        match MidiDocument::from_notes_file(&path, None) {
            Ok(doc) => {
                let n: u64 = (0..doc.track_count())
                    .map(|t| doc.track_note_count(t as u16))
                    .sum();
                return (doc, format!("real:{path} (notes={n})"));
            }
            Err(e) => eprintln!("⚠ 加载真实 MIDI 失败: {e}，回退合成数据"),
        }
    }
    (synth_doc(), "synthetic".into())
}

/// 合成文档：跨 16 轨铺满 ~1,043,936 音符（升序 tick，模拟黑乐谱密集排布）。
fn synth_doc() -> MidiDocument {
    let per = SELECTION_NOTES / 16;
    let mut doc = MidiDocument::empty_with_tracks(16, 480);
    for t in 0..16u16 {
        let mut notes: Vec<NoteEvent> = Vec::with_capacity(per as usize);
        for i in 0..per {
            let start = i * 4;
            let key = ((i * 7) % 128) as u8;
            notes.push(NoteEvent::new(start, start + 2, key, 100, (t % 16) as u8));
        }
        doc.batch_insert_sorted_notes_with_ids(t as usize, notes);
    }
    doc
}

/// 取前 `n` 个音符作为「选区」（真实数据顺序：逐轨、轨内按 tick 升序）。
/// 这正是 `arrangement_ops::clipboard::collect_selected_notes_for_clipboard` 产出的形态。
fn build_selection(doc: &MidiDocument, n: u32) -> Vec<(usize, NoteEvent)> {
    let mut sel: Vec<(usize, NoteEvent)> = Vec::with_capacity(n as usize);
    for t in 0..doc.track_count() {
        for note in doc.track_notes(t).iter() {
            sel.push((t, *note));
            if sel.len() as u32 >= n {
                return sel;
            }
        }
    }
    sel
}

/// 选区总音符数。
fn total_notes(doc: &MidiDocument) -> usize {
    (0..doc.track_count())
        .map(|t| doc.track_note_count(t as u16) as usize)
        .sum()
}

/// 预计算 origin（复制端两遍扫描的第一遍，不计入复制耗时）。
fn compute_origin(sel: &[(usize, NoteEvent)]) -> (u32, u8) {
    let mut min_tick = u32::MAX;
    let mut min_key = u8::MAX;
    for (_, n) in sel {
        if n.start_tick < min_tick {
            min_tick = n.start_tick;
        }
        if n.key < min_key {
            min_key = n.key;
        }
    }
    (min_tick, min_key)
}

/// ── JSON 旧路径：复制（手写 `write!` 拼 JSON 字符串）──
///
/// 与 `arrangement_ops::clipboard::write_arrangement_clipboard` 同构：1M 音符对象逐条格式化。
/// 计时区间覆盖整段字符串构建（即复制端真实 CPU 成本）。
fn copy_json(sel: &[(usize, NoteEvent)], origin_tick: u32, origin_key: u8) -> (String, f64) {
    let ot = origin_tick as f32;
    let ok = origin_key as u16;
    let t0 = Instant::now();
    let mut s = String::with_capacity(sel.len().saturating_mul(48) + 180);
    use std::fmt::Write as _;
    let _ = write!(
        s,
        "{{\"type\":\"arrangement\",\"origin_tick\":{ot},\"origin_key\":{ok},\"division\":480,\"notes\":["
    );
    let mut first = true;
    for (t, n) in sel {
        let tick = (n.start_tick as f32 - ot).max(0.0);
        let key = (n.key as i32 - ok as i32).max(0) as u16;
        let length = (n.end_tick - n.start_tick) as f32;
        if !first {
            s.push(',');
        }
        first = false;
        let _ = write!(
            s,
            "{{\"tick\":{tick},\"key\":{key},\"length\":{length},\"velocity\":{},\"channel\":{},\"track\":{t}}}",
            n.velocity, n.channel
        );
    }
    s.push(']');
    s.push('}');
    let ms = t0.elapsed().as_nanos() as f64 / 1e6;
    (s, ms)
}

/// ── JSON 旧路径：粘贴（`serde_json` 解析 + 按轨批量插入）──
fn paste_json(doc: &mut MidiDocument, text: &str) -> (usize, f64) {
    let t0 = Instant::now();
    let value: serde_json::Value = serde_json::from_str(text).expect("JSON 解析失败");
    let notes = value
        .get("notes")
        .and_then(|v| v.as_array())
        .expect("notes 缺失");
    let mut by_track: std::collections::HashMap<usize, Vec<NoteEvent>> =
        std::collections::HashMap::new();
    for item in notes {
        let tick = item.get("tick").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
        let key = item.get("key").and_then(|v| v.as_u64()).unwrap_or(0) as u8;
        let length = item.get("length").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
        let velocity = item.get("velocity").and_then(|v| v.as_u64()).unwrap_or(100) as u8;
        let channel = item.get("channel").and_then(|v| v.as_u64()).unwrap_or(0) as u8;
        let track = item.get("track").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
        let note = NoteEvent::new(tick as u32, (tick + length) as u32, key, velocity, channel);
        by_track.entry(track).or_default().push(note);
    }
    let mut inserted = 0usize;
    for (track, notes) in by_track {
        inserted += doc.batch_insert_notes_with_ids(track, notes).len();
    }
    let ms = t0.elapsed().as_nanos() as f64 / 1e6;
    (inserted, ms)
}

/// ── 二进制优化路径：复制（构建 `ClipRecord` 流 + 流式 `encode_clipboard`）──
///
/// 与钢琴卷帘 `build_clipboard_binary` 同构：每音符 `ClipRecord`（delta 变长 tick + 定长字段），
/// 显式传入 count 精确预分配 `Vec<u8>`，避免 filter_map/flat_map 的 size_hint=0 反复 realloc 悬崖。
fn copy_bin(sel: &[(usize, NoteEvent)], origin_tick: u32, origin_key: u8) -> (Vec<u8>, f64) {
    let t0 = Instant::now();
    // 选区已物化为 Vec，故 ClipRecord 也直接物化（与应用侧 collect_selected_notes 同构），
    // 换来精确 size_hint 与无闭包链开销的编码。
    let records: Vec<ClipRecord> = sel
        .iter()
        .map(|(t, n)| {
            ClipRecord::new(
                n.start_tick - origin_tick,
                n.end_tick - n.start_tick,
                (n.key as i32 - origin_key as i32).max(0) as u8,
                n.velocity,
                n.channel,
                *t as u16,
            )
        })
        .collect();
    let n = records.len();
    let bytes = encode_clipboard(records.into_iter(), n, 480, origin_tick, origin_key, 0);
    let ms = t0.elapsed().as_nanos() as f64 / 1e6;
    (bytes, ms)
}

/// ── 二进制优化路径：粘贴（分块解码 + 按音轨连续刷入 + 已排序批量插入）──
///
/// 关键优化：解码单遍完成，**按音轨连续 flush**（同轨子序列天然 tick 升序），直接走
/// `batch_insert_sorted_notes_with_ids` 免排序、**无 per-note HashMap 哈希**。
fn paste_bin(doc: &mut MidiDocument, bytes: &[u8]) -> (usize, f64) {
    let meta = parse_clipboard_header(bytes).expect("头部解析失败");
    let t0 = Instant::now();
    let mut cur_track: Option<usize> = None;
    let mut cur_vec: Vec<NoteEvent> = Vec::new();
    let mut inserted = 0usize;
    // 同 PPQN（ratio=1）：纯整数快路径，就地构造 NoteEvent 免去中间结构体二次构造。
    let origin_tick = meta.origin_tick;
    let origin_key = meta.origin_key as u32;
    decode_clipboard_records(
        bytes,
        |tick_offset, length, key_offset, velocity, channel, track| {
            let track = track as usize;
            if cur_track != Some(track) {
                if let Some(t) = cur_track {
                    inserted += doc
                        .batch_insert_sorted_notes_with_ids(t, std::mem::take(&mut cur_vec))
                        .len();
                }
                cur_track = Some(track);
            }
            let start = origin_tick.saturating_add(tick_offset);
            let note = NoteEvent::new(
                start,
                start.saturating_add(length),
                (origin_key + key_offset as u32).min(127) as u8,
                velocity,
                channel,
            );
            cur_vec.push(note);
        },
    )
    .expect("decode 失败");
    if let Some(t) = cur_track {
        inserted += doc.batch_insert_sorted_notes_with_ids(t, cur_vec).len();
    }
    let ms = t0.elapsed().as_nanos() as f64 / 1e6;
    (inserted, ms)
}

fn bar(label: &str, ms: f64) -> String {
    let mark = if ms < EXCELLENT_MS {
        "★★★ 优秀"
    } else if ms < TARGET_MS {
        "✓ 达标"
    } else {
        "✗ 超标"
    };
    format!("{label:<22} {:>10.2} ms  {mark}", ms)
}

/// 多次运行取平均（消除调度噪声，给出稳定可复现的数字）。
fn avg_ms(mut f: impl FnMut() -> f64, n: usize) -> f64 {
    let mut sum = 0.0;
    for _ in 0..n {
        sum += f();
    }
    sum / n as f64
}

/// 多次运行取最优（最贴近纯 CPU 成本，排除偶发调度抖动）。
fn min_ms(mut f: impl FnMut() -> f64, n: usize) -> f64 {
    let mut m = f64::INFINITY;
    for _ in 0..n {
        m = m.min(f());
    }
    m
}

fn main() {
    println!("=== Lumino 真实 MIDI 复制/粘贴基准（JSON vs 二进制）===");
    let (doc, src) = load_doc();
    let total = total_notes(&doc);
    let tracks = doc.track_count() as u16;
    let div = doc.division();
    println!("数据源: {src}");
    println!(
        "音轨数: {}  division: {}  总音符: {}",
        doc.track_count(),
        div,
        total
    );
    println!(
        "基准选区量级: {} 音符（对齐性能日志复制/粘贴实际数据量）",
        SELECTION_NOTES
    );
    println!();

    let sel = build_selection(&doc, SELECTION_NOTES);
    let origin = compute_origin(&sel);
    println!("选区 origin_tick={}  origin_key={}", origin.0, origin.1);
    println!();

    // —— JSON 旧路径（1M 选区，复现「复制 2s / 粘贴 9s」）——
    let copy_json_avg = avg_ms(|| copy_json(&sel, origin.0, origin.1).1, 3);
    let (json_str, _) = copy_json(&sel, origin.0, origin.1);
    let json_payload_mb = json_str.len() as f64 / (1024.0 * 1024.0);
    let paste_json_avg = avg_ms(
        || {
            let mut d = MidiDocument::empty_with_tracks(tracks, div);
            paste_json(&mut d, &json_str).1
        },
        3,
    );
    let (ins_json, _) = {
        let mut d = MidiDocument::empty_with_tracks(tracks, div);
        paste_json(&mut d, &json_str)
    };

    // —— 二进制优化路径（1M 选区，目标 < 100ms）——
    let copy_bin_avg = avg_ms(|| copy_bin(&sel, origin.0, origin.1).1, 7);
    let copy_bin_min = min_ms(|| copy_bin(&sel, origin.0, origin.1).1, 7);
    let (bin_bytes, _) = copy_bin(&sel, origin.0, origin.1);
    let bin_payload_mb = bin_bytes.len() as f64 / (1024.0 * 1024.0);
    let paste_bin_avg = avg_ms(
        || {
            let mut d = MidiDocument::empty_with_tracks(tracks, div);
            paste_bin(&mut d, &bin_bytes).1
        },
        7,
    );
    let paste_bin_min = min_ms(
        || {
            let mut d = MidiDocument::empty_with_tracks(tracks, div);
            paste_bin(&mut d, &bin_bytes).1
        },
        7,
    );
    let (ins_bin, _) = {
        let mut d = MidiDocument::empty_with_tracks(tracks, div);
        paste_bin(&mut d, &bin_bytes)
    };

    println!("（每组取多次运行平均；二进制附最优值）");
    println!("── 复制（序列化）──");
    println!("{}", bar("JSON write!", copy_json_avg));
    println!("{}", bar("二进制 encode", copy_bin_avg));
    println!("── 粘贴（解析+插入）──");
    println!("{}", bar("JSON parse", paste_json_avg));
    println!("{}", bar("二进制 decode", paste_bin_avg));
    println!(
        "二进制最优: 复制 {:.2} ms / 粘贴 {:.2} ms",
        copy_bin_min, paste_bin_min
    );
    println!();
    println!(
        "JSON 载荷 {:>8.2} MB | 二进制载荷 {:>8.2} MB | 压缩比 {:>5.1}x",
        json_payload_mb,
        bin_payload_mb,
        json_payload_mb / bin_payload_mb.max(1e-6)
    );
    println!(
        "JSON 复制+粘贴 合计 {:>9.2} ms | 二进制复制+粘贴 合计 {:>9.2} ms",
        copy_json_avg + paste_json_avg,
        copy_bin_avg + paste_bin_avg
    );
    println!("粘贴音符数: JSON={ins_json}  二进制={ins_bin}  (应相等，校验往返一致性)",);
    let pass = copy_bin_avg < TARGET_MS && paste_bin_avg < TARGET_MS && ins_bin == ins_json;
    println!(
        "结论(1M 选区): {}",
        if pass {
            "✓ 二进制路径复制/粘贴均 < 100ms，且往返音符数一致"
        } else {
            "✗ 未达标，需继续优化"
        }
    );

    // —— 全文件 24M 二进制上下文（不跑 JSON，避免 3 分钟级耗时）——
    println!();
    println!("── 全文件二进制上下文（{} 音符）──", total);
    let t0 = Instant::now();
    let all_bytes = encode_clipboard(
        (0..doc.track_count()).flat_map(|t| {
            doc.track_notes(t).iter().map(move |n| {
                ClipRecord::new(
                    n.start_tick,
                    n.end_tick - n.start_tick,
                    n.key,
                    n.velocity,
                    n.channel,
                    t as u16,
                )
            })
        }),
        total,
        div,
        0,
        0,
        0,
    );
    let enc_all = t0.elapsed().as_nanos() as f64 / 1e6;
    let mut doc_all = MidiDocument::empty_with_tracks(tracks, div);
    let (ins_all, dec_all) = paste_bin(&mut doc_all, &all_bytes);
    println!("{}", bar("二进制 encode", enc_all));
    println!("{}", bar("二进制 decode", dec_all));
    println!(
        "全文件二进制复制+粘贴 合计 {:>9.2} ms | 载荷 {:>8.2} MB | 插入 {}",
        enc_all + dec_all,
        all_bytes.len() as f64 / (1024.0 * 1024.0),
        ins_all
    );
    println!("=== 完成 ===");
}
