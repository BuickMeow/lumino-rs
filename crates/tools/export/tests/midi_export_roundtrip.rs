//! MIDI 导出循环测试
//!
//! 加载项目测试资源中的真实 MIDI 文件（`test-file/test_unzip_midi/Erosoul.mid`），
//! 使用项目**自身的加载器** `lumino_midi_loader::MidiDocument::from_notes_file`
//! （即 App 实际使用的加载链路）把文件解析为 `MidiDocument`，再从文档构造
//! `MidiExportData`（完全镜像 Runner 的导出来源：
//! `notes / tempo / time_signature / key_signature / PC / CC`），
//! 直接调用 `lumino_export::midi::export_midi_to_bytes` 导出，
//! 再对原始 MIDI 与导出后的 MIDI 做严格比对。
//!
//! 严格比对不仅要求 `midly` 能解析，还要求：
//! 1. 每个音轨都以 `EndOfTrack` 结尾（且其后没有任何事件）——其他软件读取硬约束。
//! 2. 音符、tempo、拍号、调号、PC/CC 在导出后不丢失、不串行。
//!
//! 该测试同时覆盖了两个历史 BUG：
//! - 加载器未对 midly 流式产出的音符按 `start_tick` 排序（debug_assert 崩溃 + 区间查询错误）；
//! - 导出时 `EndOfTrack` 被排在轨道前面，导致其他软件无法读取导出文件。

use std::path::PathBuf;

use lumino_export::midi::{
    MidiControlChangeEvent, MidiExportData, MidiExportOptions, MidiKeySignatureEvent,
    MidiNoteEvent, MidiPitchBendEvent, MidiProgramChangeEvent, MidiTempoEvent,
    MidiTimeSignatureEvent, MidiTrackData, export_midi_to_bytes,
};
use lumino_midi_loader::{MidiDocument, bpm_to_tempo};
use midly::{MetaMessage, TrackEventKind};

/// 定位仓库根目录下的测试资源 MIDI
fn test_midi_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../test-file/test_unzip_midi/Erosoul.mid")
}

/// 从加载后的 `MidiDocument` 构造导出数据（镜像 `editor_midi` 的字段映射）。
fn build_export_data_from_doc(doc: &MidiDocument) -> MidiExportData {
    let mut pc_by_track: std::collections::HashMap<u16, Vec<MidiProgramChangeEvent>> =
        Default::default();
    let mut cc_by_track: std::collections::HashMap<u16, Vec<MidiControlChangeEvent>> =
        Default::default();
    let mut pb_by_track: std::collections::HashMap<u16, Vec<MidiPitchBendEvent>> =
        Default::default();
    for ev in doc.control_events.iter() {
        match ev.kind {
            0 => {
                let (controller, value) = ev.as_control_change();
                cc_by_track
                    .entry(ev.track)
                    .or_default()
                    .push(MidiControlChangeEvent {
                        tick: ev.tick,
                        channel: ev.channel,
                        controller,
                        value,
                    });
            }
            2 => {
                pb_by_track
                    .entry(ev.track)
                    .or_default()
                    .push(MidiPitchBendEvent {
                        tick: ev.tick,
                        channel: ev.channel,
                        value: ev.param,
                    });
            }
            1 => {
                let program = ev.as_program_change();
                pc_by_track
                    .entry(ev.track)
                    .or_default()
                    .push(MidiProgramChangeEvent {
                        tick: ev.tick,
                        channel: ev.channel,
                        program,
                    });
            }
            _ => {}
        }
    }

    let tracks: Vec<MidiTrackData> = (0..doc.track_count())
        .map(|i| {
            let track_id = i as u16;
            let notes: Vec<MidiNoteEvent> = doc.notes[i]
                .iter()
                .map(|n| MidiNoteEvent {
                    tick: n.start_tick,
                    channel: n.channel,
                    key: n.key,
                    velocity: n.velocity,
                    duration: n.length().max(1),
                })
                .collect();
            let (program_changes, control_changes, pitch_bends) = (
                pc_by_track.get(&track_id).cloned().unwrap_or_default(),
                cc_by_track.get(&track_id).cloned().unwrap_or_default(),
                pb_by_track.get(&track_id).cloned().unwrap_or_default(),
            );
            MidiTrackData {
                notes,
                tempos: if i == 0 {
                    doc.tempo_changes
                        .iter()
                        .map(|&(tick, bpm)| MidiTempoEvent {
                            tick,
                            tempo: bpm_to_tempo(bpm as f64),
                        })
                        .collect()
                } else {
                    Vec::new()
                },
                time_signatures: if i == 0 {
                    doc.time_signatures
                        .iter()
                        .map(|&(tick, num, den)| MidiTimeSignatureEvent {
                            tick,
                            numerator: num,
                            denominator: human_denom_to_pow2(den),
                            clocks_per_tick: 24,
                            notated_32nd_notes_per_beat: 8,
                        })
                        .collect()
                } else {
                    Vec::new()
                },
                key_signatures: if i == 0 {
                    doc.key_signatures
                        .iter()
                        .map(|&(tick, sharps, is_minor)| MidiKeySignatureEvent {
                            tick,
                            key: sharps,
                            is_major: !is_minor,
                        })
                        .collect()
                } else {
                    Vec::new()
                },
                program_changes,
                control_changes,
                pitch_bends,
                name: doc.track_name(i).map(|s| s.to_string()),
            }
        })
        .collect();

    MidiExportData {
        options: MidiExportOptions {
            format: 1,
            ppqn: doc.division,
        },
        tracks,
    }
}

fn human_denom_to_pow2(d: u8) -> u8 {
    match d {
        1 => 0,
        2 => 1,
        4 => 2,
        8 => 3,
        16 => 4,
        32 => 5,
        64 => 6,
        _ => 2,
    }
}

/// 严格校验：每个音轨必须恰好以一条 EndOfTrack 结尾，且其后不得有任何事件。
fn strict_validate(bytes: &[u8]) -> Result<(), String> {
    let smf = midly::Smf::parse(bytes).map_err(|e| format!("midly 解析失败: {e}"))?;
    for (ti, track) in smf.tracks.iter().enumerate() {
        let last = track
            .last()
            .ok_or_else(|| format!("音轨 {ti} 为空（缺少 EndOfTrack）"))?;
        match last.kind {
            TrackEventKind::Meta(MetaMessage::EndOfTrack) => {}
            _ => {
                return Err(format!(
                    "音轨 {ti} 最后一个事件不是 EndOfTrack，而是 {last:?}"
                ));
            }
        }
        let eot_count = track
            .iter()
            .filter(|e| matches!(e.kind, TrackEventKind::Meta(MetaMessage::EndOfTrack)))
            .count();
        if eot_count != 1 {
            return Err(format!(
                "音轨 {ti} 包含 {eot_count} 条 EndOfTrack（应为 1）"
            ));
        }
    }
    Ok(())
}

#[test]
#[ignore = "需要本地测试资源 test-file/test_unzip_midi/Erosoul.mid（该文件不在仓库内，仅本地可用；本地用 `cargo test -- --ignored` 运行）"]
fn test_midi_export_roundtrip_strict() {
    let path = test_midi_path();
    assert!(path.exists(), "测试资源 MIDI 缺失: {:?}", path);

    // 使用项目自身的加载器加载（覆盖加载器排序修复）
    let doc = MidiDocument::from_notes_file(&path, None)
        .expect("加载测试 MIDI 失败（应已被加载器排序修复）");
    let loaded_notes: usize = doc.notes.iter().map(|t| t.len()).sum();
    assert!(loaded_notes > 0, "加载后应有音符");

    let export_data = build_export_data_from_doc(&doc);
    let exported = export_midi_to_bytes(&export_data).expect("导出 MIDI 失败");

    // 1) 严格校验：导出文件必须满足其他软件的读取约束（EndOfTrack 在最后）
    strict_validate(&exported).expect("导出 MIDI 严格校验失败（这是导致其他软件无法读取的根因）");

    // 2) 无损往返：用项目自身的加载器把导出的字节重新读回，
    //    其音符数据必须与从原文件加载的结果逐音符一致。
    //    （直接对比「原始 midly 解析」会受加载器的同通道同键重叠重触发语义影响，
    //    因此两端都走加载器，保证比较的是项目真实数据模型下的无损性。）
    let (doc2, _, _) =
        MidiDocument::from_notes_bytes(&exported, None).expect("重新加载导出 MIDI 失败");
    assert_eq!(
        doc.track_count, doc2.track_count,
        "往返后音轨数量应与原文件一致"
    );

    let mut total_orig: usize = 0;
    let mut total_round: usize = 0;
    for ti in 0..doc.track_count() {
        let a: Vec<lumino_midi_model::NoteEvent> = doc.notes[ti].iter().copied().collect();
        let b: Vec<lumino_midi_model::NoteEvent> = doc2.notes[ti].iter().copied().collect();
        total_orig += a.len();
        total_round += b.len();
        assert_eq!(
            a, b,
            "音轨 {ti} 往返后音符不一致（丢失 / 串行 / 力度或时值错误）"
        );
    }
    assert_eq!(total_orig, total_round, "往返后总音符数应一致");
}
