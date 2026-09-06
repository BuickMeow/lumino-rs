//! 视频导出键盘渲染
//!
//! 包含：静态键盘贴图生成、按键颜色增量计算、带演奏高亮的键盘合成。
//!
//! 拆分原因：原 `video_export.rs` 超过 400 行限制，
//! 键盘相关逻辑独立成子模块，便于维护和测试。

use lumino_extras::palette::current_track_color_f32;
use lumino_gfx::is_black_key;
use lumino_midi_loader::MidiDocument;

/// 导出视频使用的按键颜色缓冲区大小
///
/// 与编辑器 `playback_key_colors` 保持一致（256 键 × 4 通道）。
pub const KEY_COLOR_BYTES: usize = 1024;

/// 导出视频固定使用标准 128 键 MIDI 键盘
const EXPORT_KEY_COUNT: usize = 128;

/// 判定 seek 阈值（单位：tick）
///
/// 超过此阈值视为非连续播放，需要全量重建活跃音符集合。
const SEEK_THRESHOLD_TICKS: u32 = 5000;

/// 洋葱皮覆盖层的不透明度（0.6 × 255）
const OVERLAY_ALPHA: u8 = 153;

/// 播放键色增量扫描状态
///
/// 与编辑器 `PlaybackScanState` 等价，避免视频导出每帧 O(N) 全量扫描。
/// 视频导出帧按时间顺序生成，正常路径为增量扫描；仅当出现回退或跳变时重建。
#[derive(Default)]
pub struct PlaybackKeyColorState {
    /// 上次扫描到的 tick
    pub last_tick: u32,
    /// 每条音轨上次扫描到的索引
    pub scan_idx: Vec<usize>,
    /// 当前活跃音符缓存：(end_tick, key_color_offset, color)
    pub active_notes: Vec<(u32, usize, [u8; 4])>,
}

/// 获取指定音轨的演奏高亮颜色（RGBA `[u8; 4]`）
///
/// 使用设置面板的调色板样式，保证琴键颜色与音符颜色一致。
fn track_color_rgba(track_idx: usize) -> [u8; 4] {
    let color_rgba = current_track_color_f32(track_idx);
    [
        (color_rgba[0] * 255.0).round() as u8,
        (color_rgba[1] * 255.0).round() as u8,
        (color_rgba[2] * 255.0).round() as u8,
        255,
    ]
}

/// 根据当前播放 tick 计算每个 key 的覆盖颜色
///
/// 直接从 `MidiDocument.notes` 读取，数据在 MIDI 导入时已按 track 分组并按
/// `start_tick` 升序排列。使用 `partition_point` 二分查找当前 tick 的活动音符。
///
/// # 性能策略（增量扫描）
///
/// - 正常播放：增量扫描新进入的音符 + retain 清理已结束音符，每帧 O(活跃音符数)。
/// - 回退 / 跳变：触发全量重建。
pub fn update_playback_key_colors(
    document: &MidiDocument,
    tick: u32,
    state: &mut PlaybackKeyColorState,
    out: &mut [u8; KEY_COLOR_BYTES],
) {
    let track_count = document.notes.len();

    // 检测是否需要全量重建：文档结构变化、tick 回退、大幅前跳
    let need_full_rebuild = state.scan_idx.len() != track_count
        || tick < state.last_tick
        || tick.saturating_sub(state.last_tick) > SEEK_THRESHOLD_TICKS;

    if need_full_rebuild {
        *state = PlaybackKeyColorState {
            last_tick: tick,
            scan_idx: vec![0; track_count],
            active_notes: Vec::new(),
        };

        for (track_idx, notes) in document.notes.iter().enumerate() {
            if notes.is_empty() {
                continue;
            }
            let color = track_color_rgba(track_idx);
            // ChunkedList::partition_point(tick+1) = 第一个 tick > tick 的索引
            let end = notes.partition_point(tick.wrapping_add(1));
            state.scan_idx[track_idx] = end;
            // `iter_window(0, end)` 经块偏移直接定位，与 `iter().take(end)` 同集合，
            // 规避从头平铺扫描（高数据量下每帧 O(前缀) 是已知热点）。
            for (_, n) in notes.iter_window(0, end) {
                if n.end_tick > tick {
                    state
                        .active_notes
                        .push((n.end_tick, (n.key as usize) * 4, color));
                }
            }
        }
    } else {
        if state.scan_idx.len() < track_count {
            state.scan_idx.resize(track_count, 0);
        }

        for (track_idx, notes) in document.notes.iter().enumerate() {
            if notes.is_empty() {
                continue;
            }
            let color = track_color_rgba(track_idx);
            let start = state.scan_idx[track_idx];
            // ChunkedList::partition_point(tick+1) = 第一个 tick > tick 的索引
            let end = notes.partition_point(tick.wrapping_add(1));
            state.scan_idx[track_idx] = end;
            // 增量区间 `[start, end)` 经块偏移直接定位：`iter().skip(start)` 会在
            // 窗口前平铺丢弃 O(start) 个元素（36% 进度下数百万/轨/帧），`iter_window`
            // 同集合但 O(log 块数 + 区间)。输出 active_notes 与旧路径逐元素一致。
            for (_, n) in notes.iter_window(start, end) {
                if n.end_tick > tick {
                    state
                        .active_notes
                        .push((n.end_tick, (n.key as usize) * 4, color));
                }
            }
        }

        state
            .active_notes
            .retain(|(end_tick, _, _)| *end_tick > tick);
    }

    state.last_tick = tick;

    out.fill(0);
    for (_, offset, color) in &state.active_notes {
        let off = *offset;
        if off + 4 <= out.len() {
            out[off..off + 4].copy_from_slice(color);
        }
    }
}

/// 根据当前播放 tick 和一组音符直接计算每个 key 的覆盖颜色
///
/// 与 [`update_playback_key_colors`] 行为一致，但不维护增量扫描状态。
/// 用于流式读取模式：每帧从硬盘读取的音符列表已经过滤到视口范围，
/// 直接遍历其中在当前 tick 活跃的音符着色即可。
///
/// `notes` 为元组 `(start_tick, end_tick, key, track_idx)`。
pub fn update_playback_key_colors_from_notes(
    notes: &[(u32, u32, u16, u16)],
    tick: u32,
    out: &mut [u8; KEY_COLOR_BYTES],
) {
    out.fill(0);
    for (start_tick, end_tick, key, track_idx) in notes {
        if *start_tick <= tick && *end_tick > tick {
            let color = track_color_rgba(*track_idx as usize);
            let off = (*key as usize) * 4;
            if off + 4 <= out.len() {
                out[off..off + 4].copy_from_slice(&color);
            }
        }
    }
}

/// 生成完整键盘贴图（BGRA 像素数据，与视频帧格式一致）
///
/// 生成一个从最高键到最低键的完整键盘图像，与 note shader 的 Y 轴方向一致
/// （高键在上，低键在下）。返回 (pixels, width, height)。
///
/// 注意：key_count 固定为 128 以匹配标准 MIDI 键盘。
/// 使用 ceil() 进行像素到键位的映射，确保键盘边界与 note shader 对齐。
/// 直接生成 BGRA 格式以避免每帧合成时做 RGBA→BGRA 转换。
pub fn generate_keyboard_texture(_width: u32, height: u32, key_count: u16) -> (Vec<u8>, u32, u32) {
    const KB_WIDTH: f32 = 60.0;
    const RULER_HEIGHT: f32 = 30.0;
    const KEY_COUNT: u16 = 128;
    let kb_w = KB_WIDTH as u32;

    // 键盘区域从 ruler 下方开始
    let ruler_h = RULER_HEIGHT as u32;
    if height <= ruler_h || key_count == 0 {
        return (Vec::new(), 0, 0);
    }
    let kb_h = height - ruler_h;
    let key_count_f = KEY_COUNT as f32;
    let zoom_y = kb_h as f32 / key_count_f;

    let mut pixels = vec![0u8; (kb_w * kb_h * 4) as usize];

    for py in 0..kb_h {
        // Y 向：键 0 在底部，最高键在顶部（与 note shader 一致）
        // 使用 ceil() 确保键盘边界与 note shader 的精确边界匹配：
        // note shader: screen_y = (max_key_index - key) * zoom_y + ruler_height
        // 键盘像素 py 映射到 key = ceil(max_key_index - py / zoom_y)
        // 这保证了每个键占据的像素范围与 note shader 渲染的矩形完全一致
        let key_f = (key_count_f - 1.0) - py as f32 / zoom_y;
        let key_idx = key_f.ceil() as i32;
        if key_idx < 0 || key_idx >= KEY_COUNT as i32 {
            continue;
        }
        let is_black = is_black_key(key_idx as isize);

        // 标准黑白色：白键纯白，黑键纯黑
        // 白键底部添加 1px 浅灰边框作键位分隔
        let is_bottom_border = {
            let next_key_f = (key_count_f - 1.0) - (py as f32 + 1.0) / zoom_y;
            let next_key_idx = next_key_f.ceil() as i32;
            next_key_idx != key_idx
        };

        for px in 0..kb_w {
            let idx = ((py * kb_w + px) * 4) as usize;

            let (r, g, b) = if is_black {
                // 黑键：纯黑，底部加 1px 深灰边框
                if is_bottom_border {
                    (40, 40, 40)
                } else {
                    (0, 0, 0) // 标准黑键
                }
            } else {
                // 白键：纯白，底部加 1px 浅灰边框
                if is_bottom_border {
                    (200, 200, 200)
                } else {
                    (255, 255, 255) // 标准白键
                }
            };

            pixels[idx] = b; // BGRA: B 通道
            pixels[idx + 1] = g; // BGRA: G 通道
            pixels[idx + 2] = r; // BGRA: R 通道
            pixels[idx + 3] = 255;
        }
    }

    (pixels, kb_w, kb_h)
}

/// 将键盘贴图合成到视频帧上（BGRA 格式，in-place 修改），并叠加演奏高亮颜色
///
/// 贴图与帧均为 BGRA 格式，无高亮时直接逐行 memcpy；有高亮时按 60% 不透明度
/// 叠加对应音轨颜色，与编辑器左侧键盘的洋葱皮效果保持一致。
///
/// # 性能说明
/// 非高亮行走 `copy_from_slice` 快速路径（逐行 memcpy）。
/// 高亮行走标量 blend 循环，通过预计算权重系数避免每像素重复除法。
pub fn composite_keyboard(
    frame: &mut [u8],
    frame_width: u32,
    frame_height: u32,
    keyboard_pixels: &[u8],
    kb_width: u32,
    kb_height: u32,
    key_colors: &[u8; KEY_COLOR_BYTES],
) {
    const RULER_HEIGHT: u32 = 30;
    if frame_width == 0 || frame_height == 0 || keyboard_pixels.is_empty() {
        return;
    }
    let kb_w = kb_width.min(frame_width);
    let kb_h = kb_height.min(frame_height.saturating_sub(RULER_HEIGHT));
    if kb_w == 0 || kb_h == 0 {
        return;
    }
    let row_bytes = (kb_w * 4) as usize;
    let frame_stride = (frame_width * 4) as usize;
    let kb_stride = (kb_width * 4) as usize;
    let key_count_f = EXPORT_KEY_COUNT as f32;
    let zoom_y = kb_h as f32 / key_count_f;

    // 预计算每像素所需颜色分量（按 key_idx 索引）
    // 结构: (overlay_b, overlay_g, overlay_r, overlay_alpha)
    // key_colors 为 RGBA 格式，frame 为 BGRA，读取时交换 R↔B
    let mut per_key_overlay = [(0i32, 0i32, 0i32, 0u8); 128];
    for (key_idx, colors) in key_colors.as_chunks::<4>().0.iter().enumerate().take(128) {
        let alpha = colors[3];
        if alpha != 0 {
            let scaled_alpha = (alpha as u16 * OVERLAY_ALPHA as u16 / 255) as u8;
            per_key_overlay[key_idx] = (
                colors[2] as i32, // B (key_colors RGBA → overlay_b)
                colors[1] as i32, // G
                colors[0] as i32, // R (key_colors RGBA → overlay_r)
                scaled_alpha,
            );
        }
    }

    for py in 0..kb_h {
        let frame_y = RULER_HEIGHT + py;
        if frame_y >= frame_height {
            break;
        }
        let frame_start = frame_y as usize * frame_stride;
        let kb_start = py as usize * kb_stride;
        if frame_start + row_bytes > frame.len() || kb_start + row_bytes > keyboard_pixels.len() {
            continue;
        }
        let frame_row_end = frame_start + row_bytes;

        let key_f = (key_count_f - 1.0) - py as f32 / zoom_y;
        let key_idx = key_f.ceil() as i32;
        if key_idx < 0 || key_idx >= EXPORT_KEY_COUNT as i32 {
            frame[frame_start..frame_row_end]
                .copy_from_slice(&keyboard_pixels[kb_start..kb_start + row_bytes]);
            continue;
        }

        let (ob, og, or_, overlay_alpha) = per_key_overlay[key_idx as usize];
        if overlay_alpha == 0 {
            frame[frame_start..frame_row_end]
                .copy_from_slice(&keyboard_pixels[kb_start..kb_start + row_bytes]);
            continue;
        }

        // 高亮行：预计算权重，避免每像素除法
        let alpha_i = overlay_alpha as i32;
        // blend = (base * (255 - alpha) + overlay * alpha) / 255
        // 展开为: base + (overlay - base) * alpha / 255
        let frame_row = &mut frame[frame_start..frame_row_end];
        let kb_row = &keyboard_pixels[kb_start..kb_start + row_bytes];

        // 使用 as_chunks 自动向量化友好的方式处理每像素 4 字节
        for (fchunk, kchunk) in frame_row
            .as_chunks_mut::<4>()
            .0
            .iter_mut()
            .zip(kb_row.as_chunks::<4>().0.iter())
        {
            let blue_ch = kchunk[0] as i32;
            let green_ch = kchunk[1] as i32;
            let red_ch = kchunk[2] as i32;
            fchunk[0] = (blue_ch + (ob - blue_ch) * alpha_i / 255).clamp(0, 255) as u8;
            fchunk[1] = (green_ch + (og - green_ch) * alpha_i / 255).clamp(0, 255) as u8;
            fchunk[2] = (red_ch + (or_ - red_ch) * alpha_i / 255).clamp(0, 255) as u8;
            fchunk[3] = 255;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lumino_midi_loader::{MidiDocument, NoteEvent, TrackManager};

    fn make_test_doc() -> MidiDocument {
        let mut track0 = vec![
            NoteEvent::new(480, 960, 64, 100, 0), // E4
            NoteEvent::new(0, 480, 60, 100, 0),   // C4
        ];
        track0.sort_unstable_by_key(|n| n.start_tick);

        let track1 = vec![NoteEvent::new(0, 1920, 67, 100, 1)]; // G4

        MidiDocument {
            next_note_id: 1,
            notes: vec![
                lumino_midi_loader::ChunkedList::from_sorted(track0),
                lumino_midi_loader::ChunkedList::from_sorted(track1),
            ],
            tempo_changes: vec![(0, 120.0)],
            time_signatures: vec![(0, 4, 4)],
            key_signatures: vec![(0, 0, false)],
            control_events: lumino_midi_loader::ChunkedList::new(),
            lyrics: vec![],
            markers: vec![],
            sys_ex: vec![],
            track_names: vec![Some("Track 1".into()), Some("Track 2".into())],
            total_ticks: 1920,
            track_count: 2,
            tracks: TrackManager::new(2),
            division: 480,
            track_ports: vec![],
            track_max_end_ticks: vec![],
        }
    }

    #[test]
    fn test_playback_key_colors_basic() {
        let doc = make_test_doc();
        let mut state = PlaybackKeyColorState::default();
        let mut colors = [0u8; KEY_COLOR_BYTES];

        update_playback_key_colors(&doc, 240, &mut state, &mut colors);

        // C4 (key=60) 与 G4 (key=67) 在 tick 240 活跃
        assert_ne!(colors[60 * 4 + 3], 0, "C4 应被着色");
        assert_ne!(colors[67 * 4 + 3], 0, "G4 应被着色");
        // E4 (key=64) 尚未开始
        assert_eq!(colors[64 * 4 + 3], 0, "E4 不应被着色");
    }

    #[test]
    fn test_playback_key_colors_at_boundary() {
        let doc = make_test_doc();
        let mut state = PlaybackKeyColorState::default();
        let mut colors = [0u8; KEY_COLOR_BYTES];

        // tick=480：C4 刚结束，E4 刚开始，G4 仍活跃
        update_playback_key_colors(&doc, 480, &mut state, &mut colors);

        assert_eq!(colors[60 * 4 + 3], 0, "C4 已结束");
        assert_ne!(colors[64 * 4 + 3], 0, "E4 应活跃");
        assert_ne!(colors[67 * 4 + 3], 0, "G4 仍活跃");
    }

    #[test]
    fn test_playback_key_colors_incremental_consistency() {
        let doc = make_test_doc();
        let mut state = PlaybackKeyColorState::default();
        let mut colors = [0u8; KEY_COLOR_BYTES];

        // 首次全量扫描
        update_playback_key_colors(&doc, 240, &mut state, &mut colors);
        assert_ne!(colors[60 * 4 + 3], 0);
        assert_ne!(colors[67 * 4 + 3], 0);

        // 增量前进到 480
        update_playback_key_colors(&doc, 480, &mut state, &mut colors);
        assert_eq!(colors[60 * 4 + 3], 0);
        assert_ne!(colors[64 * 4 + 3], 0);
        assert_ne!(colors[67 * 4 + 3], 0);

        // 回退触发全量重建，结果应与全量一致
        update_playback_key_colors(&doc, 120, &mut state, &mut colors);
        assert_ne!(colors[60 * 4 + 3], 0);
        assert_eq!(colors[64 * 4 + 3], 0);
        assert_ne!(colors[67 * 4 + 3], 0);
    }

    #[test]
    fn test_playback_key_colors_no_overflow() {
        // 验证 key 索引在 127 以上时不会越界写入
        let doc = MidiDocument {
            next_note_id: 1,
            notes: vec![lumino_midi_loader::ChunkedList::from_sorted(vec![
                NoteEvent::new(0, 100, 200, 100, 0),
            ])],
            tempo_changes: vec![(0, 120.0)],
            time_signatures: vec![(0, 4, 4)],
            key_signatures: vec![(0, 0, false)],
            control_events: lumino_midi_loader::ChunkedList::new(),
            lyrics: vec![],
            markers: vec![],
            sys_ex: vec![],
            track_names: vec![Some("T".into())],
            total_ticks: 100,
            track_count: 1,
            tracks: TrackManager::new(1),
            division: 480,
            track_ports: vec![],
            track_max_end_ticks: vec![],
        };
        let mut state = PlaybackKeyColorState::default();
        let mut colors = [0u8; KEY_COLOR_BYTES];
        update_playback_key_colors(&doc, 50, &mut state, &mut colors);
        // 虽然 key=200 超出 128，但 offset=800，仍在 1024 缓冲区内
        assert_ne!(colors[200 * 4 + 3], 0);
        // 合成函数只读取 0..127，不会因此崩溃
    }

    #[test]
    fn test_composite_keyboard_colors_blend() {
        const WIDTH: u32 = 60;
        // 使用 zoom_y=2，确保最高键（key=127）至少有两行像素，
        // 可避开白键底部的 1px 边框行（基础色为浅灰而非纯白）。
        const HEIGHT: u32 = 30 + 256; // ruler 30 + 128 键 × 2
        let (kb_pixels, kb_w, kb_h) = generate_keyboard_texture(WIDTH, HEIGHT, 128);
        let mut frame = vec![0u8; (WIDTH * HEIGHT * 4) as usize];

        let mut key_colors = [0u8; KEY_COLOR_BYTES];
        // 将最高键（key=127，顶部 py=0）着为纯红
        key_colors[127 * 4] = 255;
        key_colors[127 * 4 + 3] = 255;

        composite_keyboard(
            &mut frame,
            WIDTH,
            HEIGHT,
            &kb_pixels,
            kb_w,
            kb_h,
            &key_colors,
        );

        // ruler 下方第一行（key=127 的非边框行）最左侧像素应被混合为红色
        let idx = (30 * WIDTH as usize) * 4;
        // 白键基础色为 (255,255,255)，叠加 (255,0,0) × 0.6 → (102,102,255)
        assert_eq!(frame[idx], 102, "B 通道应被混合");
        assert_eq!(frame[idx + 1], 102, "G 通道应被混合");
        assert_eq!(frame[idx + 2], 255, "R 通道应保持 255");
        assert_eq!(frame[idx + 3], 255, "Alpha 应为不透明");
    }
}
