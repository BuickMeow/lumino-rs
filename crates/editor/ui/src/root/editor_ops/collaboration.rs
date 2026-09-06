//! 编辑器操作 - 协作功能

use crate::root::Root;
use crate::state::root_state::{CollaborationViewState, DialogType};

impl Root {
    /// 设置协作对话框是否打开
    pub fn set_collaboration_dialog_open(&mut self, open: bool) {
        self.state.collaboration_dialog.is_open = open;
        if open {
            self.state.dialog_type = DialogType::Collaboration;
            self.state.collaboration_dialog.view_state = CollaborationViewState::Connect;
            self.state.collaboration_dialog.connection_status = "未连接".to_string();
        }
        tracing::info!("协作对话框状态: {}", open);
    }

    /// 设置协作视图状态
    ///
    /// 返回视图状态是否发生变更（供 runner 决定是否广播），避免无谓的对话框刷新。
    pub fn set_collaboration_view_state(
        &mut self,
        state: CollaborationViewState,
        invite_code: Option<String>,
        room_name: Option<String>,
    ) -> bool {
        let changed = self.state.collaboration_dialog.view_state != state;
        self.state.collaboration_dialog.view_state = state;
        if let Some(code) = invite_code {
            self.state.collaboration_dialog.invite_code = code;
        }
        if let Some(name) = &room_name {
            self.state.collaboration_dialog.room_name = name.clone();
        }
        match state {
            CollaborationViewState::Connect => {
                // 连接失败时通过 room_name 参数透传具体原因（如"用户不存在"/"密码错误"）；
                // 否则保持默认"未连接"提示。
                self.state.collaboration_dialog.connection_status =
                    room_name.unwrap_or_else(|| "未连接".to_string());
            }
            CollaborationViewState::Connecting => {
                self.state.collaboration_dialog.connection_status = "正在连接...".to_string();
            }
            CollaborationViewState::RoomActions => {
                self.state.collaboration_dialog.connection_status =
                    "已连接，请创建或加入房间".to_string();
            }
            CollaborationViewState::InRoom => {
                self.state.collaboration_dialog.connection_status = format!(
                    "房间: {} | 邀请码: {}",
                    self.state.collaboration_dialog.room_name,
                    self.state.collaboration_dialog.invite_code
                );
            }
        }
        tracing::info!("协作视图状态已更新: {:?}", state);
        changed
    }

    /// 更新远程光标位置
    pub fn update_remote_cursor(
        &mut self,
        user_id: u64,
        position: iced_core::Point,
        color: [f32; 4],
        username: String,
    ) {
        // 将颜色数组转换为十六进制字符串
        let color_str = format!(
            "{:02X}{:02X}{:02X}",
            (color[0] * 255.0) as u8,
            (color[1] * 255.0) as u8,
            (color[2] * 255.0) as u8
        );
        self.editor.update_remote_cursor(
            user_id.to_string().into(),
            position.x,
            position.y,
            color_str.into(),
            username.into(),
        );
    }

    /// 更新远程音符
    pub fn update_remote_note(&mut self, user_id: u64, operation: String) {
        // 这里将来可以解析 JSON 并应用到编辑器
        tracing::info!(
            "协作: 处理远端音符更新 - 用户: {}, 操作: {}",
            user_id,
            operation
        );
    }

    /// 应用远端用户的选择更新到本地编辑器（高亮 + 冲突判定）
    pub fn apply_remote_selection(&mut self, user_id: String, selection: String, color: String) {
        self.editor
            .apply_remote_selection(&user_id, &selection, &color);
    }

    /// 协作音轨对齐：确保本地 `document` 与侧边栏都覆盖到 `track_idx`
    /// （含中间缺失索引），使来自对端的音符/音轨操作落到正确的音轨，
    /// 避免两方音轨数量不一致时音符落到缺失/错误音轨（音轨错位）或静默丢弃。
    ///
    /// 仅补齐、幂等：已存在则跳过；`document` 为空时静默返回
    /// （协作通常在工程已初始化后进行）。
    pub(crate) fn ensure_collab_track(&mut self, track_idx: usize) {
        // 先扩 document（音符权威源），保持音轨索引与侧边栏一致。
        self.editor.editor_state.data.ensure_track(track_idx);
        while self.sidebar.tracks.len() <= track_idx {
            let id = self.sidebar.tracks.len();
            self.sidebar.tracks.push(crate::sidebar::Track {
                id,
                name: format!("Track {}", id),
                port: 0,
                channel: 0,
                display_label: format!("A{:02}", (id + 1).min(16)),
                is_conductor: false,
                can_delete: true,
                is_muted: false,
                is_soloed: false,
                color: None,
            });
        }
        self.sync_track_visual_order();
    }

    /// 应用远程笔记操作到本地编辑器
    pub fn apply_remote_note_operation(
        &mut self,
        operation: &lumino_collaboration::types::NoteBatchOperation,
    ) {
        use lumino_collaboration::types::NoteAction;

        // 协作音轨对齐：远端操作引用的音轨索引（含移动/复制的源轨与目标轨）
        // 若本地尚不存在，先补齐对应音轨，避免两方音轨数量不一致时音符错位。
        let mut tracks_to_ensure: Vec<usize> =
            operation.notes.iter().map(|n| n.track_index).collect();
        if let Some(s) = operation.source_track {
            tracks_to_ensure.push(s);
        }
        if let Some(t) = operation.target_track {
            tracks_to_ensure.push(t);
        }
        for t in tracks_to_ensure {
            self.ensure_collab_track(t);
        }

        match operation.action {
            NoteAction::Add => self.handle_remote_notes_add(operation),
            NoteAction::Update => self.handle_remote_notes_update(operation),
            NoteAction::Delete => self.handle_remote_notes_delete(operation),
            NoteAction::Move => self.handle_remote_notes_move(operation),
            _ => {
                tracing::debug!("协作: 未处理的笔记操作类型: {:?}", operation.action);
            }
        }

        // 标记音符已变化，重建当前音轨的空间索引
        self.editor.mark_notes_changed();
    }

    fn handle_remote_notes_add(
        &mut self,
        operation: &lumino_collaboration::types::NoteBatchOperation,
    ) {
        use std::collections::{HashMap, HashSet};

        // 按音轨分组批量插入，避免对 100K 音符逐条 insert（O(K·log N) → O(N log N) 单次归并）
        // 预建每轨现有 id 集合，避免每音符全轨线性扫（O(K·M) → O(M+K)）
        let mut existing_ids_by_track: HashMap<usize, HashSet<u64>> = HashMap::new();
        for n in &operation.notes {
            existing_ids_by_track
                .entry(n.track_index)
                .or_insert_with(|| {
                    self.editor
                        .editor_state
                        .data
                        .track_notes(n.track_index)
                        .iter()
                        .map(|ev| ev.id)
                        .collect()
                });
        }
        let mut by_track: HashMap<usize, Vec<crate::editor::note::Note>> = HashMap::new();
        let mut max_id: u64 = 0;
        for note in &operation.notes {
            max_id = max_id.max(note.id);
            // 去重：若该 id 已存在于本地（重传），跳过插入避免重复 id
            if existing_ids_by_track
                .get(&note.track_index)
                .is_some_and(|s| s.contains(&note.id))
            {
                continue;
            }
            let mut editor_note = crate::editor::note::Note::from_raw(
                note.tick,
                note.key,
                note.length,
                note.velocity,
                note.channel,
            );
            editor_note.id = note.id;
            by_track
                .entry(note.track_index)
                .or_default()
                .push(editor_note);
        }
        // 分轨批量写入（复用 EditorData 的批量接口，自动处理 dirty/增量）
        for (track_idx, notes) in by_track {
            // batch_insert_notes_to_track_with_ids 会保留已设置的 id（非 0 则原样），并做排序归并
            let _ids = self
                .editor
                .editor_state
                .data
                .batch_insert_notes_to_track_with_ids(track_idx, &notes);
        }
        // 抬升分配器只需一次
        if max_id != 0 {
            self.editor.editor_state.data.ensure_note_id_above(max_id);
        }
        // 精确标记受影响音轨（洋葱皮事件级增量）——若已在循环内标记，此处再补全
        let affected: HashSet<usize> = operation.notes.iter().map(|n| n.track_index).collect();
        self.editor
            .editor_state
            .data
            .mark_track_notes_changed_for(Some(affected));
        tracing::info!("协作: 已添加 {} 个远程音符（批量）", operation.notes.len());
    }

    fn handle_remote_notes_update(
        &mut self,
        operation: &lumino_collaboration::types::NoteBatchOperation,
    ) {
        // 批量更新：预建 id→index 映射，避免每音符全轨扫
        let mut id_map_by_track: std::collections::HashMap<
            usize,
            std::collections::HashMap<u64, usize>,
        > = std::collections::HashMap::new();
        for n in &operation.notes {
            id_map_by_track.entry(n.track_index).or_insert_with(|| {
                self.editor
                    .editor_state
                    .data
                    .track_notes(n.track_index)
                    .iter()
                    .enumerate()
                    .map(|(i, ev)| (ev.id, i))
                    .collect()
            });
        }
        for note in &operation.notes {
            let track_idx = note.track_index;
            let match_idx = if let Some(map) = id_map_by_track.get(&track_idx)
                && let Some(&idx) = map.get(&note.id)
            {
                let notes = self.editor.editor_state.data.track_notes(track_idx);
                if idx < notes.len() && notes[idx].id == note.id {
                    Some(idx)
                } else {
                    notes.iter().position(|n| n.id == note.id).or_else(|| {
                        notes.iter().position(|n| {
                            (n.start_tick as f32 - note.tick).abs() < 1.0
                                && n.key as u16 == note.key
                        })
                    })
                }
            } else {
                let notes = self.editor.editor_state.data.track_notes(track_idx);
                notes.iter().position(|n| n.id == note.id).or_else(|| {
                    notes.iter().position(|n| {
                        (n.start_tick as f32 - note.tick).abs() < 1.0 && n.key as u16 == note.key
                    })
                })
            };
            let Some(match_idx) = match_idx else {
                continue;
            };
            // 保持其他字段不变，仅更新长度（NoteEvent 为 Copy，先取值再写回）
            let notes = self.editor.editor_state.data.track_notes(track_idx);
            let current = notes[match_idx];
            self.editor.editor_state.data.update_note(
                track_idx,
                match_idx,
                crate::editor::note::Note::from_raw(
                    current.start_tick as f32,
                    current.key as u16,
                    note.length,
                    current.velocity,
                    current.channel,
                ),
            );
        }
        // 精确标记受影响音轨（洋葱皮事件级增量）
        let affected: std::collections::HashSet<usize> =
            operation.notes.iter().map(|n| n.track_index).collect();
        self.editor
            .editor_state
            .data
            .mark_track_notes_changed_for(Some(affected));
        tracing::info!(
            "协作: 已更新 {} 个远程音符（批量索引）",
            operation.notes.len()
        );
    }

    fn handle_remote_notes_delete(
        &mut self,
        operation: &lumino_collaboration::types::NoteBatchOperation,
    ) {
        // 批量删除：按轨聚合待删 id 集合，单次扫描收集索引，降序批量删除
        // 避免对 100K 删除每条全轨扫（O(K·M)）和多次 sort
        use std::collections::{HashMap, HashSet};
        let mut ids_by_track: HashMap<usize, HashSet<u64>> = HashMap::new();
        let mut fallback_by_track: HashMap<usize, Vec<(f32, u16)>> = HashMap::new();
        for n in &operation.notes {
            ids_by_track.entry(n.track_index).or_default().insert(n.id);
            // 同时记录位置兜底（id 未命中时按 tick/key 删）
            fallback_by_track
                .entry(n.track_index)
                .or_default()
                .push((n.tick, n.key));
        }
        for (track_idx, id_set) in ids_by_track {
            let notes = self.editor.editor_state.data.track_notes(track_idx);
            // 单次扫描收集匹配索引（id 优先，id 未命中则位置兜底）
            let mut to_delete: Vec<usize> = Vec::new();
            let fallback = fallback_by_track.get(&track_idx);
            for (i, ev) in notes.iter().enumerate() {
                if id_set.contains(&ev.id) {
                    to_delete.push(i);
                    continue;
                }
                if let Some(list) = fallback {
                    for (ftick, fkey) in list {
                        if (ev.start_tick as f32 - *ftick).abs() < 1.0 && ev.key as u16 == *fkey {
                            to_delete.push(i);
                            break;
                        }
                    }
                }
            }
            // 时间戳先来后到：按 operation.timestamp 排序的思想
            // 当前为批量到达，内部按索引降序删已保证不偏移；跨批次的时序由服务器到达序保证
            to_delete.sort_unstable_by(|a, b| b.cmp(a));
            for idx in to_delete {
                self.editor.editor_state.data.remove_note(track_idx, idx);
            }
        }
        // 精确标记受影响音轨
        let affected: HashSet<usize> = operation.notes.iter().map(|n| n.track_index).collect();
        self.editor
            .editor_state
            .data
            .mark_track_notes_changed_for(Some(affected));
        tracing::info!("协作: 已删除 {} 个远程音符（批量）", operation.notes.len());
    }

    fn handle_remote_notes_move(
        &mut self,
        operation: &lumino_collaboration::types::NoteBatchOperation,
    ) {
        let tick_offset = operation.tick_offset.unwrap_or(0.0);
        let key_offset = operation.key_offset.unwrap_or(0);
        tracing::debug!(
            "协作: Move 操作 - tick_offset={}, key_offset={}, notes数量={}, source_track={:?}",
            tick_offset,
            key_offset,
            operation.notes.len(),
            operation.source_track
        );
        // 预建每轨 id→index 映射，避免每音符全轨线性扫（100K*1M → 建表一次）
        let mut id_index_by_track: std::collections::HashMap<
            usize,
            std::collections::HashMap<u64, usize>,
        > = std::collections::HashMap::new();
        for note in &operation.notes {
            id_index_by_track
                .entry(note.track_index)
                .or_insert_with(|| {
                    self.editor
                        .editor_state
                        .data
                        .track_notes(note.track_index)
                        .iter()
                        .enumerate()
                        .map(|(i, n)| (n.id, i))
                        .collect()
                });
        }
        let mut matched_count = 0;
        // 收集待更新操作，避免在循环中因 update_note 导致索引失效
        // 策略：先按原始索引快照匹配，更新时注意 update_note 会重排，需重新解析但 id 仍唯一
        // 为简化，每次取当前快照的 position（仍 O(1) 查表 + 一次 position 回退），但比全轨扫快
        for note in &operation.notes {
            tracing::trace!(
                "协作: Move 查找音符 - target_tick={}, target_key={}, track={}",
                note.tick,
                note.key,
                note.track_index
            );
            let track_idx = note.track_index;
            // 优先按 id 索引 O(1) 命中
            let match_idx = if let Some(map) = id_index_by_track.get(&track_idx)
                && let Some(&idx) = map.get(&note.id)
            {
                // 验证索引仍有效且 id 匹配（update 可能已重排，前次 map 已过期需回退线性扫）
                let notes = self.editor.editor_state.data.track_notes(track_idx);
                if idx < notes.len() && notes[idx].id == note.id {
                    Some(idx)
                } else {
                    notes.iter().position(|n| n.id == note.id).or_else(|| {
                        notes.iter().position(|n| {
                            (n.start_tick as f32 - note.tick).abs() < 1.0
                                && n.key as u16 == note.key
                        })
                    })
                }
            } else {
                let notes = self.editor.editor_state.data.track_notes(track_idx);
                notes.iter().position(|n| n.id == note.id).or_else(|| {
                    notes.iter().position(|n| {
                        (n.start_tick as f32 - note.tick).abs() < 1.0 && n.key as u16 == note.key
                    })
                })
            };
            let Some(match_idx) = match_idx else {
                tracing::warn!("协作: track {} 不存在或音符未匹配", track_idx);
                continue;
            };
            let notes = self.editor.editor_state.data.track_notes(track_idx);
            tracing::trace!(
                "协作:   [{}] tick={}, key={}",
                match_idx,
                notes[match_idx].start_tick,
                notes[match_idx].key
            );
            // NoteEvent 为 Copy：先取值再写回，避免借用冲突
            let current = notes[match_idx];
            let new_tick = current.start_tick as f32 + tick_offset;
            let new_key = (current.key as i16 + key_offset).max(0) as u16;
            tracing::debug!(
                "协作:   匹配成功! 更新: tick {} -> {}, key {} -> {}",
                current.start_tick,
                new_tick,
                current.key,
                new_key
            );
            self.editor.editor_state.data.update_note(
                track_idx,
                match_idx,
                crate::editor::note::Note::from_raw(
                    new_tick,
                    new_key,
                    current.length() as f32,
                    current.velocity,
                    current.channel,
                ),
            );
            matched_count += 1;
        }
        // 精确标记受影响音轨（洋葱皮事件级增量）
        let affected: std::collections::HashSet<usize> =
            operation.notes.iter().map(|n| n.track_index).collect();
        self.editor
            .editor_state
            .data
            .mark_track_notes_changed_for(Some(affected));
        // 音符由 wgpu 渲染，不需要清 grid cache
        tracing::info!(
            "协作: Move 完成 - 匹配 {}/{} 个音符, current_track={}",
            matched_count,
            operation.notes.len(),
            self.editor.editor_state.data.current_track
        );
    }
}
