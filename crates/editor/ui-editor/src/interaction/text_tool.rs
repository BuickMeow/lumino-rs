//! 文字工具交互与字形采样生成音符
//!
//! 交互流程（与曲线工具 / 图片转 MIDI 一致的「拉框 → 按钮确认」范式）：
//! - 选中文字工具后，在画布拖拽拉出文本框（X 向吸附音符精度、Y 向吸附 key 线）；
//! - 松手进入编辑态，画布覆盖层出现 `TextInput` 供输入文字；
//! - 框右侧 √（确认生成）/ ×（取消）/ 模式按钮（正常 / key 范围合并）；
//! - 确认时按字形占位采样生成音符。

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use crate::grid::text_tool_box::button_rects;
use crate::{EditState, Editor, Note};
use ab_glyph::{Font, FontArc, FontVec, Point as AbPoint, PxScale, ScaleFont};
use iced_core::Point;
use lumino_editor_state::note_to_event;
use lumino_editor_state::text_tool::TextToolMode;
use lumino_note_core::history::CreateOp;

/// 字形光栅化超采样倍数（每个 key 列 / key 行细分为 SS×SS 子像素，提升细笔画捕获）
const SS: u32 = 4;
/// 单元格占用判定阈值：墨水子像素占比高于此值视为「有墨水」
const COVERAGE_THRESHOLD: f32 = 0.08;

/// 字体缓存：字形解析较重，按家族名缓存 `FontArc`（内部为 `Arc`，克隆廉价），
/// 避免实时预览每帧重复读盘与解析。
static FONT_CACHE: OnceLock<Mutex<HashMap<String, FontArc>>> = OnceLock::new();

/// 点是否落在矩形内
fn point_in_rect(p: Point, r: iced_core::Rectangle) -> bool {
    p.x >= r.x && p.x <= r.x + r.width && p.y >= r.y && p.y <= r.y + r.height
}

/// 加载字体（按家族名查找系统字体缓存；缺失时回退首个可用字体）
fn load_font(family: &str) -> Option<FontArc> {
    // 命中缓存直接返回（Arc 克隆廉价）
    if let Some(cached) = FONT_CACHE
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .ok()
        .and_then(|m| m.get(family).cloned())
    {
        return Some(cached);
    }
    use lumino_note_core::font_scanner::get_cached_fonts;
    let fonts = get_cached_fonts();
    let target = fonts
        .iter()
        .find(|f| f.name == family)
        .or_else(|| fonts.iter().find(|f| f.name.eq_ignore_ascii_case(family)))
        .or_else(|| {
            fonts
                .iter()
                .find(|f| f.name.to_lowercase().contains(&family.to_lowercase()))
        })
        .or(fonts.first())?;
    let bytes = std::fs::read(&target.path).ok()?;
    let font_vec = FontVec::try_from_vec(bytes).ok()?;
    let font = FontArc::from(font_vec);
    if let Ok(mut m) = FONT_CACHE.get_or_init(|| Mutex::new(HashMap::new())).lock() {
        m.insert(family.to_string(), font.clone());
    }
    Some(font)
}

/// 将文字光栅化为灰度位图（行优先，**行 0 = 文字顶部**，底部对齐到框底）。
///
/// 返回 `(width, height, buf)`：`width = cols * SS`、`height = rows * SS`，`buf` 按行主序存储，
/// 每像素为 0..255 的墨水墨度。预览渲染与音符采样共用同一份栅格，保证「看到的就是生成的」。
pub(crate) fn rasterize_glyph_alpha(
    text: &str,
    cols: usize,
    rows: usize,
    family: &str,
) -> Option<(u32, u32, Vec<u8>)> {
    if text.is_empty() || cols == 0 || rows == 0 {
        return None;
    }
    let font = load_font(family)?;
    let h = (rows as u32) * SS;
    let w = (cols as u32) * SS;

    // 垂直缩放：用字体 ascent+descent（不含行距）充满框高，使文字填满框且不留顶部空白。
    let unit = font.as_scaled(PxScale::from(1.0));
    let h1 = (unit.ascent() + unit.descent()).max(1e-3);
    let scale = PxScale::from(h as f32 / h1);
    let scaled = font.as_scaled(scale);

    // 第一遍：计算自然布局总推进宽度，并记录所有字形中最低墨水的 y（渲染像素，y 向下）。
    let mut total_advance = 0f32;
    let mut max_bottom = 0f32;
    for ch in text.chars() {
        let gid = font.glyph_id(ch);
        total_advance += scaled.h_advance(gid);
        if let Some(outline) =
            font.outline_glyph(gid.with_scale_and_position(scale, AbPoint::default()))
        {
            let b = outline.px_bounds();
            if b.max.y > max_bottom {
                max_bottom = b.max.y;
            }
        }
    }
    let tw = total_advance.max(1.0).ceil() as u32;

    // 底部对齐：把最低墨水（max_bottom）对齐到缓冲底（行 h）。
    let y0 = (h as f32) - max_bottom;

    // 渲染到临时缓冲（高 = h，宽 = 自然推进）
    let mut temp = vec![0u8; (h as usize) * (tw as usize)];
    let mut x_cursor = 0f32;
    for ch in text.chars() {
        let gid = font.glyph_id(ch);
        let glyph = gid.with_scale_and_position(scale, AbPoint::default());
        if let Some(outline) = font.outline_glyph(glyph) {
            let b = outline.px_bounds();
            outline.draw(|px, py, alpha| {
                // px/py 为相对字形包围盒左上角的渲染像素坐标（x 向右、y 向下，无翻转）。
                // px_bounds().min 为字形在基线坐标系下的原点偏移；加 y0 实现底部对齐。
                let x = (x_cursor + px as f32 + b.min.x).round() as i32;
                let y = (py as f32 + b.min.y + y0).round() as i32;
                if x >= 0 && x < tw as i32 && y >= 0 && y < h as i32 && alpha > 0.0 {
                    temp[y as usize * tw as usize + x as usize] = (alpha * 255.0) as u8;
                }
            });
        }
        x_cursor += scaled.h_advance(gid);
    }

    // 水平拉伸到 w（高度已一致），得到铺满框的位图
    let mut buf = vec![0u8; (h as usize) * (w as usize)];
    for (r, buf_row) in buf.chunks_mut(w as usize).enumerate() {
        let temp_base = r * tw as usize;
        for (c, buf_cell) in buf_row.iter_mut().enumerate() {
            let src_x = if tw == 0 {
                0
            } else {
                ((c as f64 * tw as f64 / w as f64) as u32).min(tw - 1)
            };
            *buf_cell = temp[temp_base + src_x as usize];
        }
    }
    Some((w, h, buf))
}

/// 将文字光栅化为占用网格（rows × cols，[row][col] = 是否有墨水）
///
/// 行 0 = 文字顶部。文字**底部对齐**到框底（共用基线），并按框高度填满、按框宽度拉伸。
/// 返回 `None` 表示无字体或文字为空。
pub(crate) fn rasterize_text(
    text: &str,
    cols: usize,
    rows: usize,
    family: &str,
) -> Option<Vec<Vec<bool>>> {
    let (w, _h, buf) = rasterize_glyph_alpha(text, cols, rows, family)?;

    // 由 SS×SS 子像素区域判定每个 (col,row) 是否占用
    let mut occ = vec![vec![false; cols]; rows];
    for (r, row) in occ.iter_mut().enumerate() {
        for (c, cell) in row.iter_mut().enumerate() {
            let mut ink = 0u32;
            let mut total = 0u32;
            for sr in (r as u32 * SS)..((r as u32 + 1) * SS) {
                for sc in (c as u32 * SS)..((c as u32 + 1) * SS) {
                    total += 1;
                    if buf[sr as usize * w as usize + sc as usize] > 0 {
                        ink += 1;
                    }
                }
            }
            *cell = (ink as f32 / total as f32) > COVERAGE_THRESHOLD;
        }
    }
    Some(occ)
}

/// 纯函数：将占用网格转换为音符列表（不依赖字体 / 画布）。
///
/// `merged = false`（正常采样）：每个有墨水的 (col,row) 生成一个音符，长度 = snap；
/// `merged = true`（key 范围合并）：每个 key 行内连续有墨水的列合并为一个音符，
/// 任意空隙断开；音符长度 = 连续列数 × snap。
///
/// `key_top` 为文字顶部对应的 key（行 0 映射到 `key_top`，向下递减）。
pub(crate) fn sample_to_notes(
    occupancy: &[Vec<bool>],
    tick_lo: f32,
    key_top: i32,
    snap: f32,
    merged: bool,
) -> Vec<(f32, u16, f32)> {
    let mut notes = Vec::new();
    if merged {
        for (r, row) in occupancy.iter().enumerate() {
            let mut run_start: Option<usize> = None;
            for (c, &cell) in row.iter().enumerate() {
                match (run_start, cell) {
                    (Some(_), true) => continue,
                    (Some(start), false) => {
                        let end = c.saturating_sub(1);
                        if end >= start {
                            let key = (key_top - r as i32).clamp(0, 255) as u16;
                            let tick = tick_lo + start as f32 * snap;
                            let len = (end - start + 1) as f32 * snap;
                            notes.push((tick, key, len));
                        }
                        run_start = None;
                    }
                    (None, true) => run_start = Some(c),
                    (None, false) => {}
                }
            }
            // 行尾收尾：仍有一段未闭合的连续墨水
            if let Some(start) = run_start {
                let end = row.len().saturating_sub(1);
                if end >= start {
                    let key = (key_top - r as i32).clamp(0, 255) as u16;
                    let tick = tick_lo + start as f32 * snap;
                    let len = (end - start + 1) as f32 * snap;
                    notes.push((tick, key, len));
                }
            }
        }
    } else {
        for (r, row) in occupancy.iter().enumerate() {
            for (c, &cell) in row.iter().enumerate() {
                if cell {
                    let key = (key_top - r as i32).clamp(0, 255) as u16;
                    let tick = tick_lo + c as f32 * snap;
                    notes.push((tick, key, snap));
                }
            }
        }
    }
    notes
}

impl Editor {
    /// 文字工具：设置输入框文字（来自画布覆盖层 TextInput 的 on_input）
    pub(crate) fn set_text_tool_text(&mut self, text: String) {
        if self.editor_state.text_tool.active {
            self.editor_state.text_tool.text = text;
        }
    }

    /// 文字工具：切换采样模式（正常 / key 范围合并）
    pub(crate) fn toggle_text_tool_mode(&mut self) {
        let m = &mut self.editor_state.text_tool.mode;
        *m = if m.is_merged() {
            TextToolMode::Normal
        } else {
            TextToolMode::KeyRangeMerged
        };
    }

    /// 文字工具：取消并清空文本框
    pub(crate) fn cancel_text_tool(&mut self) {
        self.editor_state.text_tool.reset();
    }

    /// 当前音轨是否允许使用文字工具
    ///
    /// Conductor 音轨（track 0）不可放置音符，整工具在 Conductor 轨上不可用
    /// （与铅笔等工具的 `current_track == 0` 守卫同源）。
    pub(crate) fn text_tool_allowed(&self) -> bool {
        self.editor_state.data.current_track != 0
    }

    /// 文字工具：按下处理
    pub(crate) fn handle_text_tool_pressed(&mut self, pos: Point, key: u16) {
        // Conductor 音轨（track 0）：整工具不可用，所有按下交互直接忽略
        if !self.text_tool_allowed() {
            return;
        }
        // 已拉出框：先判按钮命中
        if self.editor_state.text_tool.active {
            if let Some(btns) = button_rects(self) {
                if point_in_rect(pos, btns.confirm) {
                    self.confirm_text_tool();
                    return;
                }
                if point_in_rect(pos, btns.cancel) {
                    self.cancel_text_tool();
                    return;
                }
                if point_in_rect(pos, btns.mode) {
                    self.toggle_text_tool_mode();
                    return;
                }
            }
            // 框内点击（顶部输入条由 iced TextInput 覆盖，不会落到这里）：
            // 拖拽中间实心区域可整体移动文本框。
            if let Some((l, t, r, b)) = crate::grid::text_tool_box::box_rect_screen(self)
                && pos.x >= l
                && pos.x <= r
                && pos.y >= t
                && pos.y <= b
            {
                let grab_tick = self.pos_to_tick(pos);
                let grab_key = self.pos_to_key(pos) as f32;
                self.editor_state.text_tool.begin_move(grab_tick, grab_key);
                self.editor_state.text_tool.editing = true;
                return;
            }
            // 框外点击：取消当前框，开始拉新框
            self.cancel_text_tool();
        }

        // 新框：进入 Selecting 拖拽（Y 向吸附 key 行；X 向吸附音符精度，
        // 使拖框过程的拉伸变化按精度步进，与最终生成的音符列对齐）。
        let snap = self.editor_state.view.snap_precision.max(1.0);
        let tick = (self.pos_to_tick(pos) / snap).round() * snap;
        let view = &self.editor_state.view;
        let top_y = view.key_to_y(key);
        let bottom_y = top_y + view.zoom_y;
        self.editor_state.interaction.edit_state = EditState::Selecting {
            start_tick: tick,
            start_key: key,
            current_tick: tick,
            current_key: key,
            start_y: top_y,
            current_y: bottom_y,
        };
    }

    /// 文字工具：移动处理（拖拽中实时更新框的 current）
    ///
    /// X 向实时吸附到音符精度，使框的横向长度变化按精度步进（与最终生成一致）。
    pub(crate) fn handle_text_tool_moved(&mut self, pos: Point) {
        let snap = self.editor_state.view.snap_precision.max(1.0);
        let tick = (self.pos_to_tick(pos) / snap).round() * snap;
        let key = self.pos_to_key(pos);
        if let EditState::Selecting {
            current_tick,
            current_key,
            current_y,
            ..
        } = &mut self.editor_state.interaction.edit_state
        {
            *current_tick = tick;
            *current_key = key;
            let view = &self.editor_state.view;
            *current_y = view.key_to_y(key) + view.zoom_y;
        }
    }

    /// 文字工具：拖拽移动已放置的文本框（中间实心区域）
    ///
    /// 保持框尺寸，整体平移；X 向按音符精度、Y 向按 key 行吸附（与采样/生成一致）。
    pub(crate) fn handle_text_tool_box_move(&mut self, pos: Point) {
        let snap = self.editor_state.view.snap_precision;
        let cur_tick = self.pos_to_tick(pos);
        let cur_key = self.pos_to_key(pos) as f32;
        self.editor_state.text_tool.move_to(cur_tick, cur_key, snap);
    }

    /// 文字工具：确认生成音符（√ 按钮）
    ///
    /// 按字形占位采样：
    /// 正常模式：每个有墨水的 (col,row) 生成一个音符，长度 = 音符精度；
    /// key 范围合并模式：每个 key 行内连续有墨水的列合并为一个音符，任意空隙断开（不合并本应分开的笔画）。
    ///
    /// 成功后清空文本框与编辑历史，写入当前轨并进入撤销栈。返回是否生成了音符。
    pub(crate) fn confirm_text_tool(&mut self) -> bool {
        let tt = self.editor_state.text_tool.clone();
        if !tt.has_content() {
            return false;
        }
        // Conductor 音轨（track 0）禁止放置音符：与铅笔等工具（finish_drawing）一致，
        // 避免文字工具在不可编辑轨上创建音符。
        if self.editor_state.data.current_track == 0 {
            tracing::debug!("文字工具: Conductor 轨道禁止放置音符");
            return false;
        }
        let snap = self.editor_state.view.snap_precision;
        let (tick_lo, _) = tt.normalized_ticks();
        let (_key_lo, key_hi) = tt.normalized_keys();
        let cols = tt.cols(snap);
        let rows = tt.rows();
        if cols == 0 || rows == 0 {
            return false;
        }
        let occupancy = match rasterize_text(&tt.text, cols, rows, tt.font_family) {
            Some(o) => o,
            None => return false,
        };

        // 行 0 = 文字顶部 = 最高 key
        let key_top = key_hi as i32;
        let notes = sample_to_notes(&occupancy, tick_lo, key_top, snap, tt.mode.is_merged());

        if notes.is_empty() {
            return false;
        }

        let track = self.editor_state.data.current_track;
        let mut create_ops = Vec::with_capacity(notes.len());
        for (tick, key, len) in notes {
            let note = Note::new(tick, key, len);
            if self.editor_state.data.insert_note(track, note.clone()) {
                create_ops.push(CreateOp {
                    track_id: track as u32,
                    note: note_to_event(note),
                });
            }
        }
        if create_ops.is_empty() {
            return false;
        }

        self.editor_state.data.history.push_note_create(create_ops);
        self.editor_state.data.mark_current_track_changed();
        self.editor_state.text_tool.reset();
        self.mark_notes_changed();
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sample_to_notes_normal_single_cell() {
        let occ = vec![vec![true]];
        let notes = sample_to_notes(&occ, 0.0, 64, 1920.0, false);
        assert_eq!(notes, vec![(0.0, 64, 1920.0)]);
    }

    #[test]
    fn test_sample_to_notes_normal_grid() {
        // 2x2 全占用：正常模式生成 4 个独立音符，长度均为 snap
        let occ = vec![vec![true, true], vec![true, true]];
        let notes = sample_to_notes(&occ, 100.0, 70, 480.0, false);
        // 行 0 → key 70，行 1 → key 69
        assert_eq!(notes.len(), 4);
        assert!(notes.contains(&(100.0, 70, 480.0)));
        assert!(notes.contains(&(580.0, 70, 480.0)));
        assert!(notes.contains(&(100.0, 69, 480.0)));
        assert!(notes.contains(&(580.0, 69, 480.0)));
    }

    #[test]
    fn test_sample_to_notes_merged_runs() {
        // 一行 [T, F, T, T, F]：合并为两个音符（[0],[2,3]）
        let occ = vec![vec![true, false, true, true, false]];
        let notes = sample_to_notes(&occ, 0.0, 60, 1920.0, true);
        assert_eq!(notes, vec![(0.0, 60, 1920.0), (3840.0, 60, 3840.0)]);
    }

    #[test]
    fn test_sample_to_notes_merged_gap_breaks() {
        // 相邻但有空隙的行：空隙处必须断开（不合并本应分开的笔画）
        let occ = vec![vec![true, false, true]];
        let notes = sample_to_notes(&occ, 0.0, 60, 100.0, true);
        // [0..0] len 100, [2..2] len 100
        assert_eq!(notes, vec![(0.0, 60, 100.0), (200.0, 60, 100.0)]);
    }

    #[test]
    fn test_sample_to_notes_merged_multiline_keys() {
        // 两行：key_top=65 → 行 0 = 65，行 1 = 64
        let occ = vec![vec![true, true], vec![true, false]];
        let notes = sample_to_notes(&occ, 0.0, 65, 1920.0, true);
        assert!(notes.contains(&(0.0, 65, 3840.0))); // 行0 连续
        assert!(notes.contains(&(0.0, 64, 1920.0))); // 行1 单格
        assert!(!notes.iter().any(|&(_, k, _)| k == 66));
    }

    #[test]
    fn test_sample_to_notes_empty() {
        let occ: Vec<Vec<bool>> = vec![];
        assert!(sample_to_notes(&occ, 0.0, 60, 1920.0, true).is_empty());
    }

    #[test]
    fn test_rasterize_text_produces_ink() {
        // 仅在能加载到默认字体时验证（Windows 一般有 Microsoft YaHei；CI 缺失则跳过）
        if let Some(occ) = rasterize_text("A", 8, 8, "Microsoft YaHei") {
            assert!(occ.iter().flatten().any(|&b| b), "可识别字符应产生墨水");
        }
    }

    #[test]
    fn test_rasterize_text_empty() {
        assert!(rasterize_text("", 8, 8, "Microsoft YaHei").is_none());
    }

    #[test]
    fn test_rasterize_text_bottom_aligned() {
        // 文字应底部对齐到框底：下半区墨水应明显多于上半区，且顶部留白。
        // 本断言严格依赖 Microsoft YaHei 的字形度量（x-height/基线位置）。
        // `load_font` 缺失目标字体时会回退到首个可用字体（如 CI Linux 的 DejaVu），
        // 其 `c` 字形上下分布不同会导致 bottom>top 不成立（曾出现 50 vs 55 误报）。
        // 按本文件既有约定（“CI 缺失则跳过”）显式跳过，避免回退字体下的误报。
        let has_yahei = lumino_note_core::font_scanner::get_cached_fonts()
            .iter()
            .any(|f| {
                f.name == "Microsoft YaHei"
                    || f.name.eq_ignore_ascii_case("Microsoft YaHei")
                    || f.name.to_lowercase().contains("microsoft yahei")
            });
        if !has_yahei {
            eprintln!(
                "跳过 test_rasterize_text_bottom_aligned：缺失 Microsoft YaHei（回退字体不断言对齐）"
            );
            return;
        }
        if let Some(occ) = rasterize_text("c", 16, 16, "Microsoft YaHei") {
            let rows = occ.len();
            let half = rows / 2;
            let mut top_ink = 0u32;
            let mut bottom_ink = 0u32;
            for (r, row) in occ.iter().enumerate() {
                for &on in row.iter() {
                    if on {
                        if r < half {
                            top_ink += 1;
                        } else {
                            bottom_ink += 1;
                        }
                    }
                }
            }
            assert!(top_ink + bottom_ink > 0, "可识别字符应产生墨水");
            assert!(
                bottom_ink > top_ink,
                "文字应底部对齐：下半区墨水({bottom_ink})需多于上半区({top_ink})"
            );
        }
    }

    #[test]
    fn test_text_tool_drag_x_snaps_to_precision() {
        // 拉框过程中 X 向长度必须按音符精度步进（与最终生成一致），不能自由像素。
        let mut editor = Editor::new();
        // 文字工具交互测试必须落在「非 Conductor 的可编辑轨」上（默认 track 0 为 Conductor）
        use crate::tests::test_helpers::seed_notes;
        seed_notes(&mut editor, 2, 1, &[]);
        // 设定视图使 pos_to_tick 为恒等映射，并设置音符精度
        editor.editor_state.view.zoom_x = 1.0;
        editor.editor_state.view.scroll_x = 0.0;
        editor.editor_state.view.keyboard_width = 0.0;
        let snap = 480.0;
        editor.editor_state.view.snap_precision = snap;

        // 按下新框：起点 tick=100 → 吸附到 0（round(100/480)=0）
        editor.handle_text_tool_pressed(Point::new(100.0, 100.0), 60);
        // 拖到 tick=700 → 吸附到 round(700/480)*480 = 480
        editor.handle_text_tool_moved(Point::new(700.0, 100.0));

        match &editor.editor_state.interaction.edit_state {
            EditState::Selecting {
                start_tick,
                current_tick,
                ..
            } => {
                assert!(
                    (start_tick % snap).abs() < 1e-3,
                    "起点 tick 应吸附精度: {start_tick}"
                );
                assert!(
                    (current_tick % snap).abs() < 1e-3,
                    "当前 tick 应吸附精度: {current_tick}"
                );
                let expected = (700.0 / snap).round() * snap;
                assert_eq!(*current_tick, expected, "X 向应吸附到精度网格");
            }
            other => panic!("拉框过程应处于 Selecting 状态，实际 {other:?}"),
        }
    }

    #[test]
    fn test_text_tool_box_drag_move_wiring() {
        // 放置后的文本框：在中间实心区按下拖拽应整体移动，释放后清除拖拽态。
        use lumino_message::Tool;
        let mut editor = Editor::new();
        editor.editor_state.tool = Tool::Text;
        // 预置已放置框需落在非 Conductor 轨（默认 track 0 为 Conductor，文字工具不可用）
        use crate::tests::test_helpers::seed_notes;
        seed_notes(&mut editor, 2, 1, &[]);
        let view = &mut editor.editor_state.view;
        view.zoom_x = 1.0;
        view.scroll_x = 0.0;
        view.keyboard_width = 0.0;
        view.zoom_y = 1.0;
        view.scroll_y = 0.0;
        view.ruler_height = 0.0;
        view.visible_key_count = 256;
        view.snap_precision = 480.0;

        // 预置已放置框：tick [480,960], key [60,64]
        let tt = &mut editor.editor_state.text_tool;
        tt.set_drag(480.0, 960.0, 60, 64);
        tt.active = true;
        tt.editing = true;

        // 框内按下（x∈[480,960], y∈[191,195]）：应进入拖拽移动
        editor.handle_text_tool_pressed(Point::new(600.0, 193.0), 62);
        assert!(
            editor.editor_state.text_tool.is_dragging(),
            "框内按下应进入拖拽移动"
        );

        // 拖到 x=1100（右移一个精度单元），y 不变
        editor.handle_text_tool_box_move(Point::new(1100.0, 193.0));
        let tt = &editor.editor_state.text_tool;
        assert_eq!(tt.start_tick, 960.0, "框应整体右移一个精度单元");
        assert_eq!(tt.end_tick, 1440.0, "宽度保持不变");
        assert_eq!(tt.start_key, 60);
        assert_eq!(tt.end_key, 64);

        // 释放 → 退出拖拽态
        editor.handle_released();
        assert!(
            !editor.editor_state.text_tool.is_dragging(),
            "释放后应清除拖拽临时状态"
        );
    }

    #[test]
    fn test_text_tool_confirm_uses_incremental_path() {
        // 回归（Bug 2）：文字工具批量创建音符必须走「增量」路径（与铅笔/直线工具一致），
        // 绝不能用全量重建绕过。即确认后：note_delta_dirty 必须为 false（不触发
        // force_full_next），且 note_delta_events 携带正确的 InsertAt 增量事件，
        // 文档侧已写入音符。
        use crate::tests::test_helpers::seed_notes;
        use lumino_message::Tool;

        let mut editor = Editor::new();
        editor.editor_state.tool = Tool::Text;
        // 当前轨（非 Conductor 的普通轨 index=1）空文档：确认后的新音符从索引 0 起始
        seed_notes(&mut editor, 2, 1, &[]);

        // 构造一个能光栅化出音符的文字框（tick [0,480]，key [60,64]）
        let tt = &mut editor.editor_state.text_tool;
        tt.set_drag(0.0, 480.0, 60, 64);
        tt.active = true;
        tt.editing = true;
        tt.text = "A".to_string();
        tt.font_family = "Microsoft YaHei";

        assert!(
            editor.confirm_text_tool(),
            "确认应成功光栅化并创建音符（依赖系统字体回退）"
        );

        // 必须走增量：不触发全量重建（note_delta_dirty 保持 false）
        assert!(
            !editor.editor_state.data.note_delta_dirty,
            "文字工具确认必须走增量路径，不得触发全量重建（force_full_next）"
        );

        // 增量事件已记录：携带 InsertAt（供渲染线程段内插入）
        let inserts: Vec<_> = editor
            .editor_state
            .data
            .note_delta_events
            .iter()
            .filter(|e| matches!(e, lumino_editor_state::NoteDeltaEvent::InsertAt { .. }))
            .collect();
        assert!(!inserts.is_empty(), "确认后应产生 InsertAt 增量事件");

        // 文档侧：音符确实已写入当前轨（普通轨 index=1，非 Conductor）
        let created = editor.editor_state.data.track_notes(1).len();
        assert!(created > 0, "确认后当前轨应至少写入一个音符");
    }

    #[test]
    fn test_text_tool_confirm_rejected_on_conductor_track() {
        // 回归：文字工具不得在非可编辑的 Conductor 音轨（track 0）放置音符，
        // 与铅笔等工具（finish_drawing 的 `current_track == 0` 守卫）保持一致。
        use crate::tests::test_helpers::seed_notes;
        use lumino_message::Tool;

        let mut editor = Editor::new();
        editor.editor_state.tool = Tool::Text;
        // 选中 Conductor 音轨（track 0），且无任何音符
        seed_notes(&mut editor, 1, 0, &[]);

        let tt = &mut editor.editor_state.text_tool;
        tt.set_drag(0.0, 480.0, 60, 64);
        tt.active = true;
        tt.editing = true;
        tt.text = "A".to_string();
        tt.font_family = "Microsoft YaHei";

        // Conductor 音轨禁止放置：确认必须失败，且文档侧不得写入任何音符
        assert!(
            !editor.confirm_text_tool(),
            "Conductor 音轨（track 0）禁止放置音符，确认必须返回 false"
        );
        let created = editor.editor_state.data.track_notes(0).len();
        assert_eq!(created, 0, "Conductor 音轨不应写入任何音符");
    }

    #[test]
    fn test_text_tool_press_noop_on_conductor_track() {
        // 回归：文字工具在 Conductor 音轨（track 0）上「整个不可用」——
        // 即便完成「按下 → 释放」整段交互，也不得进入编辑态、不得生成任何音符。
        use crate::tests::test_helpers::seed_notes;
        use lumino_message::{EditorAction, Point2, Tool};

        let mut editor = Editor::new();
        editor.editor_state.tool = Tool::Text;
        // 选中 Conductor 音轨（track 0）
        seed_notes(&mut editor, 1, 0, &[]);

        // 模拟在 Conductor 轨上「按下 → 释放」整段交互
        editor.handle_action(EditorAction::Pressed {
            pos: Point2::new(100.0, 100.0),
            shift: false,
            ctrl: false,
        });
        editor.handle_action(EditorAction::Released);

        // 入口交互被拦截：文本框从未激活、从未进入编辑态
        assert!(!editor.text_tool_allowed(), "当前轨应为 Conductor");
        assert!(
            !editor.editor_state.text_tool.active,
            "Conductor 音轨上文字工具不得激活任何文本框"
        );
        assert!(!editor.editor_state.text_tool.editing);

        // released 处的 begin_editing 也被 Conductor 守卫拦下，不得置位激活状态
        assert!(!editor.editor_state.text_tool.active);

        // 文档侧：Conductor 轨不得写入任何音符
        assert_eq!(
            editor.editor_state.data.track_notes(0).len(),
            0,
            "Conductor 音轨不应写入任何音符"
        );
    }
}
