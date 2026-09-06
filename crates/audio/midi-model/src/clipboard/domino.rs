//! Domino（TAKABO SOFT）剪贴板互通支持。
//!
//! 实测剪贴板格式名为 `MidiPortalSequence`，载荷结构：
//! ```text
//!   ASCII "PortalSequenceData" (18 字节固定头)
//!   + u32 LE 未压缩长度
//!   + zlib 压缩体（deflate，窗口 15）
//! ```
//! 压缩体是一个递归分块流：每块 `[id:u8][kind:u8][len:u32 LE][payload]`。
//!
//! 音符记录在 `eb`(0xeb, kind 0x03) 块的子记录中，类型为 `d1`(0xd1)/kind 0x07，每条约 34 字节：
//! - `e9 03` (len 4, u32 LE) = 音符起始 tick
//! - `d1 07` (len 1)         = MIDI 音高键位（直接以该字节数值作为 key，如 0x3C=60=C4）
//! - `d2 07` (len 1)         = 力度 (0-127)
//! - `d3 07` (len 4, u32 LE) = 时值（gate）长度，单位 tick
//!
//! 其余顶层/子块的元数据保持 Domino 原样，编码时以实测样本为模板保留。

use crate::note_event::NoteEvent;

use std::io::{Read, Write};

use flate2::Compression;
use flate2::read::ZlibDecoder;
use flate2::write::ZlibEncoder;
use tracing::warn;

/// 剪贴板载荷固定头。
pub const PORTAL_MAGIC: &[u8; 18] = b"PortalSequenceData";
/// Domino 在系统剪贴板注册的自定义格式名。
pub const CLIPBOARD_FORMAT: &str = "MidiPortalSequence";

/// 实测捕获的 Domino 剪贴板样本（5 个音符 do-re-mi-fa-so），用作编码模板。
/// 编码时解析该模板、仅替换其中的音符记录，其余元数据原样保留，最大化 Domino 兼容性。
const TEMPLATE: &[u8] = include_bytes!("domino_template.bin");

/// 单块头部长度：id(1) + kind(1) + len(4)。
const CHUNK_HDR: usize = 6;

/// 一块 `[id][kind][payload]`。
#[derive(Debug, Clone)]
struct Chunk {
    id: u8,
    kind: u8,
    payload: Vec<u8>,
}

fn read_u32(b: &[u8]) -> Result<u32, String> {
    if b.len() < 4 {
        return Err(format!("u32 读取越界：长度 {}", b.len()));
    }
    Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

/// 把一段字节流解析为分块序列。要求流被完整消费，否则报错。
fn parse_chunks(buf: &[u8]) -> Result<Vec<Chunk>, String> {
    let mut out = Vec::new();
    let mut pos = 0;
    while pos + CHUNK_HDR <= buf.len() {
        let id = buf[pos];
        let kind = buf[pos + 1];
        let len =
            u32::from_le_bytes([buf[pos + 2], buf[pos + 3], buf[pos + 4], buf[pos + 5]]) as usize;
        if pos + CHUNK_HDR + len > buf.len() {
            return Err(format!(
                "分块长度越界 id=0x{id:02x} kind=0x{kind:02x} len={len} pos={pos} buflen={}",
                buf.len()
            ));
        }
        let payload = buf[pos + CHUNK_HDR..pos + CHUNK_HDR + len].to_vec();
        out.push(Chunk { id, kind, payload });
        pos += CHUNK_HDR + len;
    }
    if pos != buf.len() {
        return Err(format!("分块流未完整消费：剩余 {}", buf.len() - pos));
    }
    Ok(out)
}

/// 序列化单块。
fn build_chunk(id: u8, kind: u8, payload: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(CHUNK_HDR + payload.len());
    v.push(id);
    v.push(kind);
    v.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    v.extend_from_slice(payload);
    v
}

fn zlib_decompress(comp: &[u8], expected: usize) -> Result<Vec<u8>, String> {
    let mut dec = ZlibDecoder::new(comp);
    let mut out = Vec::with_capacity(expected.max(64));
    dec.read_to_end(&mut out)
        .map_err(|e| format!("zlib 解压失败：{e}"))?;
    if expected != 0 && out.len() != expected {
        warn!(
            "Domino 解压长度预期 {expected} 与实际 {} 不符，仍继续解析",
            out.len()
        );
    }
    Ok(out)
}

fn zlib_compress(raw: &[u8]) -> Result<Vec<u8>, String> {
    let mut enc = ZlibEncoder::new(Vec::new(), Compression::default());
    enc.write_all(raw)
        .map_err(|e| format!("zlib 压缩写入失败：{e}"))?;
    enc.finish().map_err(|e| format!("zlib 压缩结束失败：{e}"))
}

/// 解析 Domino 剪贴板原始载荷（以 `PortalSequenceData` 开头）为音符列表。
///
/// 返回的音符 `channel` 统一为 0（Domino 该格式未在音符记录内携带通道信息）。
pub fn decode_domino_clipboard(raw: &[u8]) -> Result<Vec<NoteEvent>, String> {
    if raw.len() < 22 || &raw[..18] != PORTAL_MAGIC {
        return Err("不是 Domino PortalSequenceData 剪贴板数据".into());
    }
    let size = read_u32(&raw[18..22])? as usize;
    let body = zlib_decompress(&raw[22..], size)?;
    let top = parse_chunks(&body)?;
    let eb = top
        .iter()
        .find(|c| c.id == 0xEB && c.kind == 0x03)
        .ok_or_else(|| "缺少 eb(0xeb) 音符容器块".to_string())?;
    let inner = parse_chunks(&eb.payload)?;

    let mut notes = Vec::new();
    for c in inner.iter() {
        if c.id != 0xD1 || c.kind != 0x07 {
            continue;
        }
        let fields = parse_chunks(&c.payload)?;
        let mut start = 0u32;
        let mut key = 0u8;
        let mut vel = 100u8;
        let mut gate = 0u32;
        for f in fields.iter() {
            match (f.id, f.kind) {
                (0xE9, 0x03) => start = read_u32(&f.payload)?,
                (0xD1, 0x07) => {
                    if f.payload.len() == 1 {
                        key = f.payload[0];
                    }
                }
                (0xD2, 0x07) => {
                    if f.payload.len() == 1 {
                        vel = f.payload[0];
                    }
                }
                (0xD3, 0x07) => gate = read_u32(&f.payload)?,
                _ => {}
            }
        }
        let end = start.saturating_add(gate);
        // 注意：Domino 音符记录（34 字节 = e9/d1/d2/d3 四个子块，10+7+7+10=34）
        // 不含逐音符通道字段，故通道统一归一为 0。这是格式本身的限制，非解析缺陷。
        notes.push(NoteEvent::new(start, end, key, vel, 0));
    }
    if notes.is_empty() {
        return Err("Domino 数据中未找到可解析的音符记录".into());
    }
    Ok(notes)
}

/// 把 Lumino 音符编码为 Domino 可粘贴的剪贴板原始载荷。
///
/// 采用「模板替换」策略：以实测 Domino 样本为模板解析出分块树，
/// 仅替换 `eb` 块内的音符记录（`d1`/kind 0x07），其余元数据原样保留，
/// 重新计算各层长度后序列化为 `PortalSequenceData` + size + zlib。
///
/// 注：该方向的 Domino 端接受度需在 Domino 内实测确认（本仓无法运行 Domino）。
pub fn encode_domino_clipboard(notes: &[NoteEvent]) -> Result<Vec<u8>, String> {
    let tpl_size = read_u32(&TEMPLATE[18..22])? as usize;
    let body = zlib_decompress(&TEMPLATE[22..], tpl_size)?;
    let mut top = parse_chunks(&body)?;

    let eb_idx = top
        .iter()
        .position(|c| c.id == 0xEB && c.kind == 0x03)
        .ok_or_else(|| "模板缺少 eb 块".to_string())?;
    let eb_inner = parse_chunks(&top[eb_idx].payload)?;

    // 去掉模板原有的音符记录，保留其余常量记录
    let mut new_inner: Vec<Chunk> = eb_inner
        .into_iter()
        .filter(|c| !(c.id == 0xD1 && c.kind == 0x07))
        .collect();

    // 在第一个「音符之后的常量块」(d9) 之前插入新音符记录
    let split_at = new_inner
        .iter()
        .position(|c| c.id == 0xD9 && c.kind == 0x07)
        .unwrap_or(new_inner.len());
    let tail = new_inner.split_off(split_at);

    for n in notes.iter() {
        let start = n.start_tick;
        let key = n.key;
        let vel = n.velocity.min(127);
        let gate = n.length();
        let mut rec = Vec::new();
        rec.extend_from_slice(&build_chunk(0xE9, 0x03, &start.to_le_bytes()));
        rec.extend_from_slice(&build_chunk(0xD1, 0x07, &[key]));
        rec.extend_from_slice(&build_chunk(0xD2, 0x07, &[vel]));
        rec.extend_from_slice(&build_chunk(0xD3, 0x07, &gate.to_le_bytes()));
        new_inner.push(Chunk {
            id: 0xD1,
            kind: 0x07,
            payload: rec,
        });
    }
    new_inner.extend(tail);

    let eb_payload: Vec<u8> = new_inner
        .iter()
        .flat_map(|c| build_chunk(c.id, c.kind, &c.payload))
        .collect();
    top[eb_idx] = Chunk {
        id: 0xEB,
        kind: 0x03,
        payload: eb_payload,
    };

    let body_out: Vec<u8> = top
        .iter()
        .flat_map(|c| build_chunk(c.id, c.kind, &c.payload))
        .collect();
    let comp = zlib_compress(&body_out)?;

    let mut out = Vec::with_capacity(22 + comp.len());
    out.extend_from_slice(PORTAL_MAGIC);
    out.extend_from_slice(&(body_out.len() as u32).to_le_bytes());
    out.extend_from_slice(&comp);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_decode_real_capture() {
        let raw = TEMPLATE; // 嵌入样本即真实捕获
        let notes = decode_domino_clipboard(raw).expect("应解析出音符");
        assert_eq!(notes.len(), 5, "样本应含 5 个音符");
        let keys: Vec<u8> = notes.iter().map(|n| n.key).collect();
        assert_eq!(keys, vec![60, 62, 64, 65, 67], "do re mi fa so");
        let vels: Vec<u8> = notes.iter().map(|n| n.velocity).collect();
        assert!(vels.iter().all(|&v| v == 100), "力度应为 100");
        let starts: Vec<u32> = notes.iter().map(|n| n.start_tick).collect();
        assert_eq!(starts, vec![0, 480, 960, 1440, 1920], "起始 tick 间隔 480");
        let gates: Vec<u32> = notes.iter().map(|n| n.length()).collect();
        assert!(gates.iter().all(|&g| g == 480), "时值 480");
    }

    #[test]
    fn test_encode_decode_roundtrip() {
        let src = vec![
            NoteEvent::new(0, 480, 60, 100, 0),
            NoteEvent::new(480, 960, 62, 90, 0),
            NoteEvent::new(960, 1200, 67, 110, 0),
        ];
        let blob = encode_domino_clipboard(&src).expect("编码应成功");
        assert!(
            blob.starts_with(PORTAL_MAGIC),
            "应以 PortalSequenceData 开头"
        );
        let out = decode_domino_clipboard(&blob).expect("回环应可解析");
        assert_eq!(out.len(), 3, "回环音符数应一致");
        for (a, b) in src.iter().zip(out.iter()) {
            assert_eq!(a.start_tick, b.start_tick, "start 应一致");
            assert_eq!(a.key, b.key, "key 应一致");
            assert_eq!(a.velocity, b.velocity, "velocity 应一致");
            assert_eq!(a.length(), b.length(), "gate 应一致");
        }
    }

    /// 全音调 / 全参数泛化测试：覆盖 0-127 全部音高键位 + 极端力度/时值/位置，
    /// 证明解析并不依赖样本里的那 5 个音符（解码按原始字节读 key，与具体音高无关）。
    #[test]
    fn test_roundtrip_full_pitch_range() {
        // 覆盖所有 12 个半音 + 八度边界 + 极值键位
        let keys: Vec<u8> = (0u8..=127).collect();
        let mut src: Vec<NoteEvent> = Vec::new();
        let vels = [0u8, 1, 64, 100, 127];
        let gates = [1u32, 5, 480, 9999, 1_000_000];
        let starts = [0u32, 1, 333, 100_000];
        for (i, &k) in keys.iter().enumerate() {
            let vel = vels[i % vels.len()];
            let gate = gates[i % gates.len()];
            let start = starts[i % starts.len()] + (i as u32 % 50) * 10;
            src.push(NoteEvent::new(start, start + gate, k, vel, (i % 16) as u8));
        }
        let blob = encode_domino_clipboard(&src).expect("全音高编码应成功");
        let out = decode_domino_clipboard(&blob).expect("全音高解码应成功");
        assert_eq!(out.len(), src.len(), "音符数应一致");
        // Domino 音符记录不含逐音符通道，解码统一为 0（格式限制）
        assert!(
            out.iter().all(|n| n.channel == 0),
            "Domino 解码结果通道应统一为 0"
        );
        for (a, b) in src.iter().zip(out.iter()) {
            assert_eq!(a.key, b.key, "key {} 应一致", a.key);
            assert_eq!(a.start_tick, b.start_tick, "key {} start 应一致", a.key);
            assert_eq!(a.velocity, b.velocity, "key {} velocity 应一致", a.key);
            assert_eq!(a.length(), b.length(), "key {} gate 应一致", a.key);
        }
        // 强断言：0..=127 全部 128 个音高键位都被正确还原
        let got_keys: Vec<u8> = out.iter().map(|n| n.key).collect();
        assert_eq!(
            got_keys,
            (0u8..=127).collect::<Vec<u8>>(),
            "全部 128 个音高键位（含所有半音与八度）都应被正确解析"
        );
    }

    #[test]
    fn test_reject_non_domino() {
        let junk = b"hello world this is not domino";
        assert!(decode_domino_clipboard(junk).is_err());
    }
}
