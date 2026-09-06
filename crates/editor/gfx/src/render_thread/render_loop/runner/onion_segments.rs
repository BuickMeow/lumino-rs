//! 洋葱皮 GPU 布局段表与事件级增量应用
//!
//! 黑乐谱场景（单音轨海量音符）的增量上传核心：
//! - 全量流式会话中，WGPU 侧按 `Chunk { track_id }` 流构建段表（track_id → offset/len）
//! - 编辑其他音轨（洋葱皮显示的音轨）时，UI 只发送 `TrackDelta { track_id, instances }`，
//!   本模块完成段替换：
//!   - 等长替换 → 仅 write_segment（音符级增量，只传该音轨）
//!   - 变长替换 → grow（如需）→ GPU 内部搬移后续段 → 写段 → 更新计数与段表
//!
//! 主音轨段内增量（2026-08-06 统一全量渲染）：GPU 布局 = 所有轨全部音符，
//! 主音轨编辑事件（`NoteEvent`，index = notes 索引）映射到「当前音轨段」的
//! 段内位置（`seg.offset + index`）：
//! - `UpdateMany` / `Update`：等长原位写（不改变段长）
//! - `RemoveAt` / `Insert`：段内删/插 → GPU 搬移 + 段表长度/偏移联动
//!
//! 正确性保障：
//! - `compute_move_blocks` 为纯函数，搬移分块序列已用 `Vec::copy_within`
//!   对照单测
//! - 段表偏移更新为纯计算，单测覆盖前插/删除/尾部增删/无后续段等边界

use crate::NoteEvent;
use crate::NoteInstance;
use crate::render_thread::render_loop::Renderers;

/// 洋葱皮 GPU 缓冲区中的音轨段
///
/// 布局：所有洋葱皮音轨按全量会话到达顺序紧凑排列在单 buffer 中，
/// 段间无间隙。`offset` = 首实例索引，`len` = 实例数。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OnionSegment {
    pub track_id: usize,
    pub offset: usize,
    pub len: usize,
}

/// 主音轨事件级增量：将 `NoteEvent` 应用到当前音轨段（段内位置 = `seg.offset + index`）
///
/// 返回 `true` 表示有事件被消费（调用方据此决定是否需要更新 cull info）。
/// 段不存在（如全量会话尚未到达 / 切轨瞬间事件错位）→ 防御性忽略并记录警告；
/// 下次全量会话（加载/布局变化）会重建段表兜底。
///
/// 事件语义（统一全量渲染后）：
/// - `UpdateMany` / `Update`：index = 当前轨 document notes 索引（保序）→ 段内原位写
/// - `RemoveAt` / `Remove`：段内保序删除，段长与后续段偏移联动
/// - `Insert` / `Add`：段内保序插入，段长与后续段偏移联动
/// - `Reset` / `Clear`：整段替换 / 清空（防御性兜底，正常路径 UI 不再发送）
pub fn process_main_track_events(
    renderers: &mut Renderers,
    segments: &mut [OnionSegment],
    current_track_encoded: u32,
    rx: &std::sync::mpsc::Receiver<NoteEvent>,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
) -> bool {
    if current_track_encoded == 0 {
        return false;
    }
    let track_id = current_track_encoded as usize - 1;
    let Some(idx) = segments.iter().position(|s| s.track_id == track_id) else {
        return false;
    };

    let mut updated = false;
    while let Ok(event) = rx.try_recv() {
        updated = true;
        // 每轮重新读取段元数据：段长可能被前序事件（删/插）改变
        let seg = segments[idx];
        match event {
            NoteEvent::UpdateMany {
                start_index,
                instances,
            } => {
                // 等长原位写：不改变段长，无需段表联动
                renderers
                    .onion_skin
                    .update_notes(seg.offset + start_index, &instances);
            }
            NoteEvent::Update { index, instance } => {
                renderers
                    .onion_skin
                    .update_notes(seg.offset + index, &[instance]);
            }
            NoteEvent::RemoveAt { index, count } => {
                let count = count.min(seg.len.saturating_sub(index));
                if count == 0 {
                    continue;
                }
                renderers.onion_skin.remove_at(seg.offset + index, count);
                shift_segment_len(renderers, segments, idx, -(count as isize), device, queue);
            }
            NoteEvent::Remove(index) => {
                if index >= seg.len {
                    continue;
                }
                renderers.onion_skin.remove_at(seg.offset + index, 1);
                shift_segment_len(renderers, segments, idx, -1, device, queue);
            }
            NoteEvent::Insert { index, instances } => {
                let index = index.min(seg.len);
                if instances.is_empty() {
                    continue;
                }
                renderers
                    .onion_skin
                    .insert_at(seg.offset + index, &instances);
                shift_segment_len(
                    renderers,
                    segments,
                    idx,
                    instances.len() as isize,
                    device,
                    queue,
                );
            }
            NoteEvent::Add(instance) => {
                renderers
                    .onion_skin
                    .insert_at(seg.offset + seg.len, &[instance]);
                shift_segment_len(renderers, segments, idx, 1, device, queue);
            }
            NoteEvent::Reset(instances) => {
                // 防御性兜底：整段替换（正常路径 UI 不再发送 Reset）
                apply_onion_track_delta(renderers, segments, track_id, &instances, device, queue);
            }
            NoteEvent::Clear => {
                apply_onion_track_delta(renderers, segments, track_id, &[], device, queue);
            }
        }
    }
    updated
}

/// 段长变化后的段表联动：更新本段长度 + 后续段偏移平移，并刷新 cull info
fn shift_segment_len(
    renderers: &mut Renderers,
    segments: &mut [OnionSegment],
    idx: usize,
    delta: isize,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
) {
    segments[idx].len = (segments[idx].len as isize + delta) as usize;
    for seg in &mut segments[idx + 1..] {
        seg.offset = (seg.offset as isize + delta) as usize;
    }
    renderers.onion_skin.update_cull_info(device, queue);
}

/// 应用单音轨增量替换：将 `track_id` 段整体替换为 `instances`
///
/// 返回 `true` 表示成功；段表中无该音轨（UI/WGPU 状态不一致）返回 `false`，
/// 调用方记录警告并跳过——下次全量会话（音轨进出洋葱皮 / mute / 调色板变化）
/// 会重建段表兜底。
///
/// 等长（new_len == old_len）：
///     write_segment 原位覆盖（cull uniform 无需更新，bind group 有效）
/// 变长（new_len != old_len）：
///     1. 若新总实例数超容量 → grow（重建 buffer，GPU 内部复制现有数据）
///     2. GPU 内部搬移后续段（[old_end, old_count) → [new_end, new_count)，
///        staging 分块，见 `GpuNoteBuffer::move_range`）
///     3. 写新段 → 更新计数 → 更新段表偏移 → update_cull_info（count 变化
///        会重建 bind group）
pub fn apply_onion_track_delta(
    renderers: &mut Renderers,
    segments: &mut [OnionSegment],
    track_id: usize,
    instances: &[NoteInstance],
    device: &wgpu::Device,
    queue: &wgpu::Queue,
) -> bool {
    let Some(idx) = segments.iter().position(|s| s.track_id == track_id) else {
        tracing::warn!(
            "OnionSkin: TrackDelta 的 track_id={} 不在段表中（状态不一致），跳过；等待下次全量会话修复",
            track_id
        );
        return false;
    };

    let old_len = segments[idx].len;
    let new_len = instances.len();
    if old_len == new_len {
        // 等长替换：音符级增量，无需动段表 / cull info
        renderers
            .onion_skin
            .write_segment(segments[idx].offset, instances);
        // 常驻字节变更 → 换代（全局桶重建）。
        renderers.onion_epoch = renderers.onion_epoch.wrapping_add(1);
        return true;
    }

    let old_count = renderers.onion_skin.gpu_instance_count();
    let delta = new_len as isize - old_len as isize;
    let new_count = (old_count as isize + delta) as usize;

    // 1. 扩容（grow 复制旧数据 → 之后的搬移/写段都在新 buffer 上）
    if new_count > renderers.onion_skin.gpu_capacity() && !renderers.onion_skin.grow_gpu(new_count)
    {
        tracing::error!(
            "OnionSkin: TrackDelta grow 失败（track={}, 需要容量 {}），跳过增量",
            track_id,
            new_count
        );
        return false;
    }

    // 2. 搬移后续段（GPU 内部，无 CPU 镜像）
    let tail_start = segments[idx].offset + old_len;
    let tail_count = old_count.saturating_sub(tail_start);
    if tail_count > 0 {
        renderers.onion_skin.move_gpu_range(
            tail_start,
            (tail_start as isize + delta) as usize,
            tail_count,
        );
    }

    // 3. 写新段（目标区间 = [offset, offset + new_len)，与搬移后的后续段相邻不重叠）
    renderers
        .onion_skin
        .write_segment(segments[idx].offset, instances);

    // 4. 更新计数与段表
    renderers.onion_skin.set_gpu_instance_count(new_count);
    segments[idx].len = new_len;
    for seg in &mut segments[idx + 1..] {
        seg.offset = (seg.offset as isize + delta) as usize;
    }

    // 5. cull info（count 变化 → 重建 bind group；did_grow 时 buffer 句柄变化也会重建）
    renderers.onion_skin.update_cull_info(device, queue);

    // 常驻字节变更（搬移/写段）→ 换代（全局桶重建）。
    renderers.onion_epoch = renderers.onion_epoch.wrapping_add(1);

    tracing::debug!(
        "OnionSkin: TrackDelta track={} {} 实例 (原 {} 实例，{}) → 增量完成",
        track_id,
        new_len,
        old_len,
        if delta == 0 {
            "等长替换"
        } else {
            "变长搬移"
        }
    );
    true
}

/// 变长替换后，计算新的段表偏移（纯函数，可单测）
///
/// `segments` 为替换前的段表，`idx` 为被替换段，`new_len` 为替换后段长。
/// 返回替换后的完整段表（后续段 offset 平移 delta）。
#[cfg(test)]
pub(crate) fn shifted_segments_after_replace(
    segments: &[OnionSegment],
    idx: usize,
    new_len: usize,
) -> Vec<OnionSegment> {
    let old_len = segments[idx].len;
    let delta = new_len as isize - old_len as isize;
    segments
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let mut s = *s;
            if i == idx {
                s.len = new_len;
            }
            if i > idx {
                s.offset = (s.offset as isize + delta) as usize;
            }
            s
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(track_id: usize, offset: usize, len: usize) -> OnionSegment {
        OnionSegment {
            track_id,
            offset,
            len,
        }
    }

    fn layout() -> Vec<OnionSegment> {
        vec![
            seg(0, 0, 100),
            seg(1, 100, 50),
            seg(2, 150, 30),
            seg(3, 180, 20),
        ]
    }

    #[test]
    fn shift_equal_len_keeps_offsets() {
        let after = shifted_segments_after_replace(&layout(), 1, 50);
        assert_eq!(after[1].len, 50);
        assert_eq!(after[2].offset, 150);
        assert_eq!(after[3].offset, 180);
    }

    #[test]
    fn shift_grow_moves_following_tracks_forward() {
        // 段1 从 50 变 70（delta=+20）：后续段 offset 全部 +20
        let after = shifted_segments_after_replace(&layout(), 1, 70);
        assert_eq!(after[1].len, 70);
        assert_eq!(after[2].offset, 170);
        assert_eq!(after[3].offset, 200);
    }

    #[test]
    fn shift_shrink_moves_following_tracks_backward() {
        // 段1 从 50 变 20（delta=-30）：后续段 offset 全部 -30
        let after = shifted_segments_after_replace(&layout(), 1, 20);
        assert_eq!(after[1].len, 20);
        assert_eq!(after[2].offset, 120);
        assert_eq!(after[3].offset, 150);
    }

    #[test]
    fn shift_first_segment() {
        // 段0（首段）增长：delta = +40
        let after = shifted_segments_after_replace(&layout(), 0, 140);
        assert_eq!(after[0].len, 140);
        assert_eq!(after[1].offset, 140);
        assert_eq!(after[2].offset, 190);
        assert_eq!(after[3].offset, 220);
    }

    #[test]
    fn shift_last_segment_no_followers() {
        // 段3（末段）缩短：无后续段可平移
        let after = shifted_segments_after_replace(&layout(), 3, 5);
        assert_eq!(after[3].len, 5);
        assert_eq!(after[3].offset, 180);
    }

    #[test]
    fn shift_shrink_to_zero_len() {
        // 段变 0（整轨清空）：delta = -len
        let after = shifted_segments_after_replace(&layout(), 1, 0);
        assert_eq!(after[1].len, 0);
        assert_eq!(after[2].offset, 100);
        assert_eq!(after[3].offset, 130);
    }
}
