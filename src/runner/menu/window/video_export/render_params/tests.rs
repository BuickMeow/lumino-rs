use super::*;
use lumino_midi_loader::{NoteEvent, TrackManager};

fn make_track(notes: &[(u32, u32, u8)]) -> Vec<NoteEvent> {
    let mut v: Vec<NoteEvent> = notes
        .iter()
        .map(|&(s, e, k)| NoteEvent::new(s, e, k, 100, 0))
        .collect();
    v.sort_unstable_by_key(|n| n.start_tick);
    v
}

/// 正确性护栏：下界固定为 0 后，窗口从文件头开始，但上界仍通过二分查找
/// 限制在 `tick_end` 以内，不会退化为全量扫描。
#[test]
fn test_note_search_bounds_window_is_small() {
    // 100 万音符均匀分布在 [0, 10_000_000) tick
    const TOTAL: usize = 1_000_000;
    let mut track = Vec::with_capacity(TOTAL);
    for i in 0..TOTAL {
        let t = (i as u32) * 10;
        track.push(NoteEvent::new(t, t + 240, 60, 100, 0));
    }

    // 视口：tick 5_000_000 起，窗口 4 小节（ppq=480 → 7680 ticks）
    let chunked = lumino_midi_loader::ChunkedList::from_sorted(track);
    let (start, end) = note_search_bounds(&chunked, 5_000_000, 5_007_680);
    let window_len = end - start;

    // 下界为 0，窗口从文件头开始
    assert_eq!(start, 0, "下界应固定为 0");
    // 上界仍通过二分查找限制在 tick_end 以内，不会扫描文件末尾
    assert!(window_len < TOTAL, "窗口不应覆盖全部音符");
    assert!(window_len > 0, "窗口不应为空");
    // 窗口应包含所有 start_tick <= tick_end 的音符
    assert!(chunked.get(end - 1).expect("窗口内应有音符").start_tick <= 5_007_680);
    if end < TOTAL {
        assert!(
            chunked
                .get(end)
                .expect("end < TOTAL 时 end 处应有音符")
                .start_tick
                > 5_007_680
        );
    }
}

/// 正确性：二分窗口收集结果必须与全量遍历完全一致
/// （覆盖：视口前已结束、跨视口长音符、视口内、视口后未开始）
#[test]
fn test_visible_notes_collection_matches_full_scan() {
    let doc = MidiDocument {
        next_note_id: 1,
        notes: vec![
            lumino_midi_loader::ChunkedList::from_sorted(make_track(&[
                (0, 480, 40),               // 视口前很远，已结束
                (4_985_000, 5_001_000, 50), // 跨视口长音符（时长 16000 < BUFFER）
                (5_000_100, 5_001_000, 60), // 视口内
                (5_007_000, 5_009_000, 62), // 跨视口右边界
                (5_007_680, 5_008_000, 64), // 视口上界恰好开始
                (6_000_000, 6_000_480, 70), // 视口后很远，未开始
            ])),
            lumino_midi_loader::ChunkedList::from_sorted(make_track(&[(5_000_200, 5_000_700, 65)])),
        ],
        tempo_changes: vec![(0, 120.0)],
        time_signatures: vec![(0, 4, 4)],
        key_signatures: vec![(0, 0, false)],
        control_events: lumino_midi_loader::ChunkedList::new(),
        lyrics: vec![],
        markers: vec![],
        sys_ex: vec![],
        track_names: vec![Some("T1".into()), Some("T2".into())],
        total_ticks: 6_000_480,
        track_count: 2,
        tracks: TrackManager::new(2),
        division: 480,
        track_ports: vec![],
        track_max_end_ticks: vec![],
    };

    let tick_start = 5_000_000;
    let tick_end = tick_start + 7680;
    const KEY_COUNT: u16 = 128;

    // 窗口版（被测窗口语义：上界二分 + 可见过滤）
    let mut windowed = Vec::new();
    for (track_idx, track_notes) in doc.notes.iter().enumerate() {
        let (_, search_end) = note_search_bounds(track_notes, tick_start, tick_end);
        for n in track_notes.iter().take(search_end) {
            if n.end_tick > tick_start && n.start_tick < tick_end && n.key < KEY_COUNT as u8 {
                windowed.push((n.key, n.start_tick, n.end_tick, track_idx as u16));
            }
        }
    }

    // 全量遍历版（参考实现）
    let mut full = Vec::new();
    for (track_idx, track_notes) in doc.notes.iter().enumerate() {
        for n in track_notes {
            if n.end_tick > tick_start && n.start_tick < tick_end && n.key < KEY_COUNT as u8 {
                full.push((n.key, n.start_tick, n.end_tick, track_idx as u16));
            }
        }
    }

    assert_eq!(windowed, full, "二分窗口收集结果与全量遍历不一致");
    // 预期可见：跨视口长音符 + 视口内 2 个 + 跨右边界 1 个
    assert_eq!(windowed.len(), 4);
}

// 注：`collect_window_notes` 已随 GPU cull 删除，其滑动收集测试同步删除
//（git 历史可查）；窗口语义等价性由 `lumino-gfx` 的 `global_bucket::cull_tests`
// + 像素 harness 保证。
#[test]
fn test_resolve_miditrail_speed_isolated_per_view() {
    assert_eq!(
        resolve_miditrail_speed(MiditrailViewMode::Normal, 2.0, 5.0),
        2.0,
        "Normal 应取 Normal 速度"
    );
    assert_eq!(
        resolve_miditrail_speed(MiditrailViewMode::Top, 2.0, 5.0),
        5.0,
        "Top 应取 Top 速度"
    );
    // 下限钳制（与旧 waterfall_scroll_speed.max(0.1) 语义一致）。
    assert_eq!(
        resolve_miditrail_speed(MiditrailViewMode::Top, 1.0, 0.0),
        0.1
    );
}

/// 视图枚举解析：UI 字符串 → 强类型（非法值回退 Normal，不静默产生第三种状态）。
#[test]
fn test_miditrail_view_mode_from_str() {
    use std::str::FromStr;
    assert_eq!(
        MiditrailViewMode::from_str("Top").expect("Top 应可解析"),
        MiditrailViewMode::Top
    );
    assert_eq!(
        MiditrailViewMode::from_str("普通").expect("普通应可解析"),
        MiditrailViewMode::Normal
    );
    assert!(MiditrailViewMode::from_str("侧视").is_err());
}

/// 首帧全量 + 稳态跳过（瀑布流）：`collect_all=true` 发全文档有序集（无窗口过滤，
/// 越界 key 除外），`false` 发空集 + uniforms（渲染侧复用 GPU 常驻 + cull）。
#[test]
fn test_waterfall_collect_all_first_frame_full_then_skip() {
    let doc = MidiDocument {
        next_note_id: 1,
        notes: vec![
            lumino_midi_loader::ChunkedList::from_sorted(make_track(&[
                (0, 240, 60),               // 视口前很远（窗口会过滤，全量保留）
                (5_000_100, 5_001_000, 62), // 视口内
                (9_000_000, 9_000_480, 64), // 视口后很远（窗口会过滤，全量保留）
                (100, 200, 200),            // 越界 key，全量也过滤
            ])),
            lumino_midi_loader::ChunkedList::from_sorted(make_track(&[(5_000_200, 5_000_700, 60)])),
        ],
        tempo_changes: vec![(0, 120.0)],
        time_signatures: vec![(0, 4, 4)],
        key_signatures: vec![(0, 0, false)],
        control_events: lumino_midi_loader::ChunkedList::new(),
        lyrics: vec![],
        markers: vec![],
        sys_ex: vec![],
        track_names: vec![Some("T1".into()), Some("T2".into())],
        total_ticks: 9_000_480,
        track_count: 2,
        tracks: TrackManager::new(2),
        division: 480,
        track_ports: vec![],
        track_max_end_ticks: vec![],
    };
    let mut visible = Vec::new();
    let mut out = Vec::new();
    let mut state = WindowCollectState::default();

    // 首帧：全量 4 个有效音符（越界 key 已滤），(key, start) 有序。
    let params = build_waterfall_render_params(WaterfallRenderInput {
        width: 640,
        height: 360,
        tick: 5_000_000,
        document: &doc,
        ppq: 480,
        key_count: 128,
        waterfall_scroll_speed: 1.0,
        visible_notes: &mut visible,
        note_instances_out: &mut out,
        window_state: &mut state,
        collect_all: true,
    });
    assert!(params.is_waterfall_mode, "瀑布流标志");
    assert_eq!(
        params.note_instances.len(),
        4,
        "首帧全量：不过滤窗口（视口前后 + 跨视口全保留），只滤越界 key"
    );
    let keys_starts: Vec<(u32, f32)> = params
        .note_instances
        .iter()
        .map(|n| (n.key_color & 0xFF, n.start_length[0]))
        .collect();
    let mut sorted = keys_starts.clone();
    sorted.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.total_cmp(&b.1)));
    assert_eq!(
        keys_starts, sorted,
        "全量集须 (key, start) 有序（legacy 回退前提）"
    );

    // 稳态帧：空集 + uniforms（tick 照常推进）。
    let params2 = build_waterfall_render_params(WaterfallRenderInput {
        width: 640,
        height: 360,
        tick: 5_000_100,
        document: &doc,
        ppq: 480,
        key_count: 128,
        waterfall_scroll_speed: 1.0,
        visible_notes: &mut visible,
        note_instances_out: &mut out,
        window_state: &mut state,
        collect_all: false,
    });
    assert!(
        params2.note_instances.is_empty(),
        "稳态帧跳过收集（渲染侧 cull）"
    );
    assert_eq!(
        params2.waterfall_current_tick, 5_000_100,
        "uniforms 照常推进"
    );
}

/// 首帧全量 + 稳态跳过（Miditrail）：语义同瀑布流，uniforms 含 3D 参数。
#[test]
fn test_miditrail_collect_all_first_frame_full_then_skip() {
    let doc = MidiDocument {
        next_note_id: 1,
        notes: vec![lumino_midi_loader::ChunkedList::from_sorted(make_track(&[
            (0, 240, 60),
            (5_000_100, 5_001_000, 62),
            (9_000_000, 9_000_480, 64),
        ]))],
        tempo_changes: vec![(0, 120.0)],
        time_signatures: vec![(0, 4, 4)],
        key_signatures: vec![(0, 0, false)],
        control_events: lumino_midi_loader::ChunkedList::new(),
        lyrics: vec![],
        markers: vec![],
        sys_ex: vec![],
        track_names: vec![Some("T1".into())],
        total_ticks: 9_000_480,
        track_count: 1,
        tracks: TrackManager::new(1),
        division: 480,
        track_ports: vec![],
        track_max_end_ticks: vec![],
    };
    let mut visible = Vec::new();
    let mut out = Vec::new();
    let mut state = WindowCollectState::default();

    let params = build_miditrail_render_params(MiditrailRenderInput {
        width: 640,
        height: 360,
        tick: 5_000_000,
        document: &doc,
        ppq: 480,
        key_count: 128,
        miditrail_speed: 1.0,
        miditrail_view_mode: lumino_message::events::window::video::MiditrailViewMode::Normal,
        miditrail_z_far: 7.5,
        miditrail_3d_notes: false,
        fps: 60.0,
        visible_notes: &mut visible,
        note_instances_out: &mut out,
        window_state: &mut state,
        collect_all: true,
    });
    assert!(params.miditrail_enabled, "3D 标志");
    assert!(
        !params.miditrail_3d_notes,
        "开关默认关闭（平面）透传渲染参数"
    );
    assert_eq!(params.note_instances.len(), 3, "首帧全量：视口前后全保留");
    assert!(
        params.miditrail_ticks_per_second > 0.0,
        "光晕时间基准照常计算"
    );

    let params2 = build_miditrail_render_params(MiditrailRenderInput {
        width: 640,
        height: 360,
        tick: 5_000_100,
        document: &doc,
        ppq: 480,
        key_count: 128,
        miditrail_speed: 1.0,
        miditrail_view_mode: lumino_message::events::window::video::MiditrailViewMode::Normal,
        miditrail_z_far: 7.5,
        miditrail_3d_notes: true,
        fps: 60.0,
        visible_notes: &mut visible,
        note_instances_out: &mut out,
        window_state: &mut state,
        collect_all: false,
    });
    assert!(
        params2.note_instances.is_empty(),
        "稳态帧跳过收集（渲染侧 cull + 回读）"
    );
    assert!(params2.miditrail_3d_notes, "开关打开（盒子）透传渲染参数");
    assert_eq!(params2.miditrail_current_tick, 5_000_100);
}
