//! Miditrail 3D 渲染器实例构建

use super::MIDITRAIL_SCENE_DEPTH;
use super::types::{
    MiditrailAuraInstanceGpu, MiditrailDrivenParamsGpu, MiditrailInstanceGpu, MiditrailNoteGpu,
    MiditrailUniformGpu,
};
use crate::NoteInstance;
use crate::is_black_key;

const KEYBOARD_HEIGHT: f32 = 0.012;
const WHITE_KEY_DEPTH: f32 = 0.07;
const BLACK_KEY_DEPTH: f32 = 0.0448;
const NOTE_HEIGHT: f32 = 0.007;
const NOTE_Y: f32 = 0.0005;
const NOTE_Z_OFFSET: f32 = 0.012;
const BLACK_KEY_ELEVATION: f32 = 0.0;
const BLACK_KEY_HEIGHT: f32 = 0.024;
const BLACK_KEY_WIDTH_RATIO: f32 = 0.58;

// ── Aura 光晕环动画参数（参考 Zenith-MIDI MidiTrailRender/Render.cs）──

/// 光环半径 = 键宽 × 该系数 × 光晕系数。
///
/// Zenith 原式为 `circleRadius * 12 * auraSize`；按视觉反馈缩到其 2/3（12 → 8）：
/// 常态尺寸回到 4 倍键宽（与动画化之前一致），按下闪光峰值 = 8 × 2/3 ≈ 5.33 倍键宽。
const AURA_RING_SCALE: f32 = 8.0;
/// 按下闪光在起始后多少帧内二次衰减到 0（Zenith 硬编码 10）。
const AURA_FLASH_FRAMES: f32 = 10.0;
/// 闪光分量缩放系数：起始峰值 = 100 / 600 ≈ 0.167。
const AURA_FLASH_DIVISOR: f32 = 600.0;
/// 常态/收缩分量的时间基准（秒）：剩余时长超过该值时光环保持常态尺寸。
const AURA_TAIL_SECONDS: f32 = 1.0;
/// 收缩分量幂指数：`(剩余时长 / 音符时长) ^ 0.3`。
const AURA_TAIL_POWER: f32 = 0.3;
/// 长音符保持期（剩余时长 ≥ 1s）的光环系数。
const AURA_HELD_FACTOR: f32 = 0.5;

/// 当前 tick 下被按下的键信息（同一键多个音符时取最后一个音符颜色）。
#[derive(Debug, Clone, Copy)]
pub struct ActiveKeys {
    /// 每个键是否被按下。
    pub pressed: [bool; 128],
    /// 每个键的激活颜色。
    pub colors: [u32; 128],
}

/// 计算当前 tick 下每个键是否被按下及其对应颜色。
///
/// 同一键多个音符激活时，取 `notes` 中最后一个音符的颜色，与
/// `build_key_instances` / `build_aura_instances` 历史行为一致。
#[must_use]
pub fn compute_active_keys(tick: u32, notes: &[MiditrailNoteGpu]) -> ActiveKeys {
    let mut pressed = [false; 128];
    let mut colors = [0u32; 128];
    for note in notes {
        if note.is_active_at(tick) {
            let key = note.key as usize;
            if key < 128 {
                pressed[key] = true;
                colors[key] = note.color_packed;
            }
        }
    }
    ActiveKeys { pressed, colors }
}

/// 更新键位布局缓存。
pub fn update_key_positions(
    key_count: u32,
    last_key_count: &mut u32,
    key_positions: &mut Vec<f32>,
    key_widths: &mut Vec<f32>,
) {
    if key_count == 0 || key_count == *last_key_count {
        return;
    }
    *last_key_count = key_count;
    key_positions.resize(key_count as usize, 0.0);
    key_widths.resize(key_count as usize, 0.0);

    let count = key_count as usize;
    let num_white = (0..count)
        .filter(|key_idx| !is_black_key(*key_idx as isize))
        .count()
        .max(1);
    let white_width = 1.0 / num_white as f32;
    let black_width = white_width * BLACK_KEY_WIDTH_RATIO;

    let mut pos = 0.0f32;
    for key_idx in 0..count {
        if is_black_key(key_idx as isize) {
            key_positions[key_idx] = pos - black_width * 0.5;
            key_widths[key_idx] = black_width;
        } else {
            key_positions[key_idx] = pos;
            key_widths[key_idx] = white_width;
            pos += white_width;
        }
    }
}

/// 将 [r, g, b, a] 颜色打包为 `0xRRGGBBAA`。
#[must_use]
pub fn pack_color(color: [f32; 4]) -> u32 {
    let r = (color[0].clamp(0.0, 1.0) * 255.0) as u32;
    let g = (color[1].clamp(0.0, 1.0) * 255.0) as u32;
    let b = (color[2].clamp(0.0, 1.0) * 255.0) as u32;
    let a = (color[3].clamp(0.0, 1.0) * 255.0) as u32;
    (r << 24) | (g << 16) | (b << 8) | a
}

/// 将颜色每个通道提亮指定值并重新打包为 `0xRRGGBBAA`。
///
/// 参考 Comet MIDITrail：激活音符在最终颜色上直接 +0.5（clamp 到 1.0）。
#[must_use]
pub fn boost_color_packed(packed: u32, amount: f32) -> u32 {
    let a = packed & 0xFF;
    let r = (((packed >> 24) & 0xFF) as f32 / 255.0 + amount).clamp(0.0, 1.0);
    let g = (((packed >> 16) & 0xFF) as f32 / 255.0 + amount).clamp(0.0, 1.0);
    let b = (((packed >> 8) & 0xFF) as f32 / 255.0 + amount).clamp(0.0, 1.0);
    (((r * 255.0) as u32) << 24) | (((g * 255.0) as u32) << 16) | (((b * 255.0) as u32) << 8) | a
}

/// `build_note_instances` 跨帧复用的暂存集（调用方持有，零每帧分配）。
///
/// 四块缓冲各司其职：`order` 存 (排序键, 下标) 供 radix 排序；`gather`
/// 存按序 gather 后的实例（与 `out` swap）；`radix` 是基数排序 ping-pong
/// 副缓冲；`hist` 复用 65536 桶直方图。由渲染器/测试持有并跨帧复用。
#[derive(Debug, Default)]
pub struct NoteBuildScratch {
    /// (排序键, `out` 下标)，radix 排序输入/输出。
    pub order: Vec<(u64, u32)>,
    /// gather 暂存（与 `out` swap）。
    pub gather: Vec<MiditrailInstanceGpu>,
    /// radix ping-pong 副缓冲。
    pub radix: Vec<(u64, u32)>,
    /// 65536 桶直方图复用。
    pub hist: Vec<u32>,
}

/// 构建 GPU-Driven 管线参数（`MiditrailDrivenParamsGpu`）。
///
/// 视口/深度公式与 `build_note_instances` 头部逐 op 一致（ppq/speed 钳位、
/// `visible_measure_count` 取整、span 下限 1），键位表直拷调用方缓存。
/// Top 视图不走此路径（沿用 CPU `quantize_notes_for_top`＋legacy 构建）。
pub fn build_driven_params(
    uniform: &MiditrailUniformGpu,
    key_positions: &[f32],
    key_widths: &[f32],
) -> MiditrailDrivenParamsGpu {
    let ppq = uniform.ppq.max(1);
    let speed = uniform.speed.max(0.1);
    let ticks_per_measure = ppq * 4;
    let visible_measure_count = ((4.0 / speed).round()).max(1.0) as u32;
    let viewport_tick_span = (ticks_per_measure * visible_measure_count).max(1) as f32;
    let z_far = NOTE_Z_OFFSET - uniform.z_far_distance.max(0.1);
    let mut key_table = [[0.0f32; 4]; 128];
    let n = key_positions.len().min(key_widths.len()).min(128);
    for (i, slot) in key_table.iter_mut().enumerate().take(n) {
        *slot = [key_positions[i], key_widths[i], 0.0, 0.0];
    }
    MiditrailDrivenParamsGpu {
        tick: uniform.tick,
        viewport_tick_span,
        scene_depth: MIDITRAIL_SCENE_DEPTH,
        note_z_offset: NOTE_Z_OFFSET,
        z_far,
        note_height: NOTE_HEIGHT,
        note_y: NOTE_Y,
        key_count: uniform.key_count,
        key_table,
    }
}

/// 构建可见音符的实例数据。
///
/// 排序解耦为"索引排序 + gather"两步：`scratch.order` 存 (排序键, 下标) 16B
/// 元组并用 LSD 基数排序（稳定，与 `sort_by_key` 输出严格一致，见
/// sort_equivalence 回归测试），再按排好序的下标把 `out` 中的实例 gather
/// 到 `scratch.gather` 后 swap 回 `out`。
pub fn build_note_instances(
    uniform: &MiditrailUniformGpu,
    notes: &[MiditrailNoteGpu],
    key_positions: &[f32],
    key_widths: &[f32],
    out: &mut Vec<MiditrailInstanceGpu>,
    scratch: &mut NoteBuildScratch,
) {
    let tick = uniform.tick;
    let ppq = uniform.ppq.max(1);
    let speed = uniform.speed.max(0.1);
    let ticks_per_measure = ppq * 4;
    let visible_measure_count = ((4.0 / speed).round()).max(1.0) as u32;
    let viewport_tick_span = (ticks_per_measure * visible_measure_count).max(1) as f32;
    let scene_depth = MIDITRAIL_SCENE_DEPTH;
    let note_height = NOTE_HEIGHT;
    let note_y = NOTE_Y;
    let note_z_offset = NOTE_Z_OFFSET;
    let z_far_distance = uniform.z_far_distance.max(0.1);
    let z_far = note_z_offset - z_far_distance;

    // 打包排序键 + 下标：排序键 = is_black(1bit) | z 可排序位(32bit) | key(7bit)，
    // 详见下方 f32 位重排注释。只排序 16B (键, 下标) 元组（旧 56B (键, 实例)
    // 元组移动量的约 1/3），实例按输入序直接进 `out`，排序后 gather 重排。
    // scratch 均由调用方提供并跨帧复用，避免每帧大堆分配。
    out.clear();
    out.reserve(notes.len());
    let order = &mut scratch.order;
    order.clear();
    order.reserve(notes.len());
    let t_loop = std::time::Instant::now();
    for note in notes {
        if !note.is_visible_at(tick) {
            continue;
        }
        let key = note.key as usize;
        if key >= key_positions.len() {
            continue;
        }
        let left = key_positions[key];
        let width = key_widths[key];

        let visible_start = note.start_tick.max(tick);
        let visible_end = note.end_tick;
        let z_start = note_z_offset
            - ((visible_start.saturating_sub(tick)) as f32 / viewport_tick_span * scene_depth);
        let mut z_end = note_z_offset
            - ((visible_end.saturating_sub(tick)) as f32 / viewport_tick_span * scene_depth);
        z_end = z_end.max(z_far);
        if z_end >= z_start {
            continue;
        }
        let z_center = (z_start + z_end) * 0.5;
        let z_length = z_start - z_end;

        let scale = [width * 0.92, note_height, z_length];
        let translation = [left + width * 0.04, note_y, z_center - z_length * 0.5];

        let color = if note.is_active_at(tick) {
            boost_color_packed(note.color_packed, 0.5)
        } else {
            note.color_packed
        };

        // f32 位重排为可排序 u32（与 f32::total_cmp 全序严格等价）：
        // - 正数（含正 NaN）：翻转符号位 → 映射到 [0x8000_0000, 0xFFFF_FFFF]
        // - 负数（含负 NaN）：按位取反 → 映射到 [0x0000_0000, 0x7FFF_FFFF]
        // 这样排序结果与旧实现 `z_start.total_cmp` 完全一致（含 NaN 顺序）。
        let z_bits = z_start.to_bits();
        let z_sortable = if z_bits & 0x8000_0000 != 0 {
            !z_bits
        } else {
            z_bits ^ 0x8000_0000
        };
        // 位布局：is_black(bit 63) | z_sortable(bit 39..=7，32 位) | key(bit 6..=0，7 位)。
        // key 范围 0-127 只需 7 位；z 左移 7 位后最高到 bit 38，不与 bit 63 冲突。
        // 注意：z 不能左移 32 位——z_sortable ≥ 0x8000_0000（正数 z）时其 bit 31
        // 会落到 bit 63，与 is_black 位互相污染导致排序错乱（曾实测 85 处不一致）。
        let sort_key = ((is_black_key(note.key as isize) as u64) << 63)
            | ((z_sortable as u64) << 7)
            | (note.key as u64);

        out.push(MiditrailInstanceGpu::new(
            translation,
            scale,
            color,
            false,
            0.0,
            0.0,
        ));
        order.push((sort_key, out.len() as u32 - 1));
    }

    // 按 Comet MIDITrail 的音符绘制顺序排序：
    // 1. 白键音符先绘制，黑键音符后绘制（确保黑键音符覆盖白键音符）；
    // 2. 同颜色组内按前缘深度 far-to-near 排序，使靠近键盘的音符最后绘制。
    // 这样画家算法 + 音符不写深度，可消除重叠部分的颜色闪烁。
    // 用 `sort_by_key`（稳定）替代旧 `sort_by` 三键闭包：排序键 u64 已完整编码
    // (is_black, z, key) 全序，单键比较比闭包快（基准：10 万音符 6.37ms → 2.5ms，
    // 省约 60%）；稳定性保留旧语义——完全同键（同 key 同 start 的和弦叠音）
    // 按输入顺序绘制，与旧实现一致，避免同位置音符覆盖顺序不确定导致闪烁。
    let loop_us = t_loop.elapsed().as_micros() as u64;
    let t_sort = std::time::Instant::now();
    radix_sort_order(order, &mut scratch.radix, &mut scratch.hist);
    let sort_us = t_sort.elapsed().as_micros() as u64;
    let t_gather = std::time::Instant::now();
    // 按排好序的下标 gather：`scratch_gather` 复用跨帧容量，swap 后 `out`
    // 即为最终绘制序，旧内容留在 gather 缓冲供下一帧复用（零分配）。
    let gather = &mut scratch.gather;
    gather.clear();
    gather.reserve(out.len());
    for &(_, idx) in order.iter() {
        gather.push(out[idx as usize]);
    }
    std::mem::swap(out, gather);
    let gather_us = t_gather.elapsed().as_micros() as u64;
    diag_build_notes(loop_us, sort_us, gather_us, out.len());
}

/// build_note_instances 内部分段打点（首 3 帧 + 每 300 帧）：定位 loop/sort/gather 配比。
fn diag_build_notes(loop_us: u64, sort_us: u64, gather_us: u64, notes: usize) {
    static COUNT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    if n < 3 || n.is_multiple_of(300) {
        tracing::info!(
            "miditrail细分[{n}]: loop={loop_us} sort={sort_us} gather={gather_us} notes={notes}"
        );
    }
}

/// LSD 基数排序（16bit × 4 pass，稳定）：对 (排序键, 下标) 按键排序。
///
/// 与 `sort_by_key`（稳定）输出严格一致：LSD 从低位到高位逐 pass 稳定
/// 分桶，同键保持输入相对顺序；排序键是全序 u64，不存在"相等但比较器
/// 不一致"的暗坑。4 pass 为偶数，ping-pong 后结果落回 `order`，无需回拷。
/// 每 pass 流量 ≈ 键读 8B + 下标读写 8B，36 万音符约 23MB，远小于比较排序
/// O(n log n) 次 16B 元组搬运。直方图 65536 × u32 由调用方复用。
fn radix_sort_order(order: &mut Vec<(u64, u32)>, tmp: &mut Vec<(u64, u32)>, hist: &mut Vec<u32>) {
    const BITS: u32 = 16;
    const BUCKETS: usize = 1 << BITS;
    const PASSES: u32 = 64 / BITS;
    let n = order.len();
    if n < 2 {
        return;
    }
    tmp.clear();
    tmp.resize(n, (0, 0));
    hist.clear();
    hist.resize(BUCKETS, 0);
    let (mut src, mut dst) = (order.as_mut_slice(), tmp.as_mut_slice());
    for pass in 0..PASSES {
        let shift = pass * BITS;
        hist.fill(0);
        for &(key, _) in src.iter() {
            hist[((key >> shift) & 0xFFFF) as usize] += 1;
        }
        let mut sum = 0u32;
        for count in hist.iter_mut() {
            let c = *count;
            *count = sum;
            sum += c;
        }
        for &(key, idx) in src.iter() {
            let b = ((key >> shift) & 0xFFFF) as usize;
            let pos = hist[b] as usize;
            hist[b] = pos as u32 + 1;
            dst[pos] = (key, idx);
        }
        std::mem::swap(&mut src, &mut dst);
    }
    // PASSES = 4 为偶数：偶数次 swap 后 `src` 指回 `order` 的缓冲，结果已就位。
}

/// 构建琴键实例。
/// `active_keys` 由 `compute_active_keys` 预先计算，避免本函数再次扫描全部音符。
pub fn build_key_instances(
    uniform: &MiditrailUniformGpu,
    active_keys: &ActiveKeys,
    key_positions: &[f32],
    key_widths: &[f32],
    press_factors: &[f32],
    out: &mut Vec<MiditrailInstanceGpu>,
) {
    let key_count = uniform.key_count as usize;
    let key_count = key_count.min(key_positions.len());

    for key_idx in 0..key_count {
        let left = key_positions[key_idx];
        let width = key_widths[key_idx];
        let is_black = is_black_key(key_idx as isize);
        let (y, height) = if is_black {
            (BLACK_KEY_ELEVATION, BLACK_KEY_HEIGHT)
        } else {
            (0.0, KEYBOARD_HEIGHT)
        };
        let color = if active_keys.pressed[key_idx] {
            active_keys.colors[key_idx]
        } else if is_black {
            pack_color([0.2, 0.2, 0.2, 1.0])
        } else {
            pack_color([1.0, 1.0, 1.0, 1.0])
        };
        let depth = if is_black {
            BLACK_KEY_DEPTH
        } else {
            WHITE_KEY_DEPTH
        };
        let press_depth = if is_black {
            // 黑键按下深度最多为高出白键部分高度的 0.5，保证按下后仍可见
            (BLACK_KEY_HEIGHT - KEYBOARD_HEIGHT) * 0.5
        } else {
            KEYBOARD_HEIGHT * 0.5
        };
        let scale = [width, height, depth];
        let translation = [left, y, 0.0];
        let press = press_factors
            .get(key_idx)
            .copied()
            .unwrap_or(0.0)
            .clamp(0.0, 1.0);
        out.push(MiditrailInstanceGpu::new(
            translation,
            scale,
            color,
            true,
            press,
            press_depth,
        ));
    }
}

/// 构建 Aura 实例（音符光晕环放大动画）。
///
/// 参考 Zenith-MIDI `MidiTrailRender/Render.cs` 的 `auraSize` 累加逻辑，
/// 每个键的光晕尺寸由该键上**正在发声**的音符实时驱动：
/// - **按下闪光**：音符起始后 `AURA_FLASH_FRAMES` 帧内二次衰减的冲击分量
///   `(max(10 - 起始帧数, 0))² / 600`，按下瞬间光环放大到最大再回落到常态；
/// - **常态/收缩**：`(min(剩余时长, 1s) / min(音符时长, 1s))^0.3 / 2`，
///   长音符保持 0.5，临近结束时（最后 1 秒）光环收缩到 0，平滑消失；
/// - 光环半径 = 键宽 × `AURA_RING_SCALE` × 光晕系数（Zenith 的
///   `circleRadius * 12 * auraSize` 按视觉反馈缩到 2/3）。
///
/// 同键多个音符取光晕系数最大值；环颜色沿用 `active_keys` 的按下键颜色。
/// 每帧完全由 (tick, notes) 重算，不依赖跨帧状态，seek/变速均自洽。
pub fn build_aura_instances(
    uniform: &MiditrailUniformGpu,
    notes: &[MiditrailNoteGpu],
    active_keys: &ActiveKeys,
    key_positions: &[f32],
    key_widths: &[f32],
    out: &mut Vec<MiditrailAuraInstanceGpu>,
) {
    let key_count = (uniform.key_count as usize)
        .min(key_positions.len())
        .min(128);

    // 每键光晕系数：该键正在发声的音符贡献的最大值（Zenith `auraSize[k]`）
    let mut aura_sizes = [0.0f32; 128];
    for note in notes {
        let key = note.key as usize;
        if key >= key_count {
            continue;
        }
        let factor = aura_factor_for_note(uniform, note);
        if factor > aura_sizes[key] {
            aura_sizes[key] = factor;
        }
    }

    for key_idx in 0..key_count {
        // 颜色仅来自当前按下的键（与 compute_active_keys 一致）；
        // aura_sizes[key] > 0 等价于该键有正在发声的音符。
        if !active_keys.pressed[key_idx] {
            continue;
        }
        let aura = aura_sizes[key_idx];
        if aura <= 0.0 {
            continue;
        }
        let width = key_widths[key_idx];
        let center = key_positions[key_idx] + width * 0.5;
        let size = (width * AURA_RING_SCALE * aura).max(0.001);
        out.push(MiditrailAuraInstanceGpu {
            size,
            pos: center,
            color_packed: active_keys.colors[key_idx],
            _padding: 0,
        });
    }
}

/// 单个音符对所在键光晕系数的贡献（Zenith `factor + factor2`）。
///
/// 仅当音符当前正在发声（`start_tick <= tick < end_tick`）且已开始时有贡献；
/// 未开始的音符（Zenith `n.start < midiTime` 才累加）与已结束的音符直接返回 0。
fn aura_factor_for_note(uniform: &MiditrailUniformGpu, note: &MiditrailNoteGpu) -> f32 {
    aura_factor_raw(
        uniform.tick,
        uniform.ticks_per_second,
        uniform.fps,
        note.start_tick,
        note.end_tick,
    )
}

/// 光晕系数核心数学（`tick/tps/fps/start/end` 五元决定，与载体无关）。
///
/// Legacy `MiditrailNoteGpu` 路径与 GPU-Driven `NoteInstance` 路径共用，
/// 保证两条路径的光晕动画逐位一致。
fn aura_factor_raw(
    tick: u32,
    ticks_per_second: f32,
    fps: f32,
    start_tick: u32,
    end_tick: u32,
) -> f32 {
    if start_tick > tick || end_tick <= tick {
        return 0.0;
    }
    let tps = ticks_per_second.max(0.1);
    // Zenith `tempoFrameStep`：每帧 tick 数 = 每秒 tick 数 / fps
    let frame_ticks = (tps / fps.max(1.0)).max(0.001);

    // 按下闪光：起始后 AURA_FLASH_FRAMES 帧内二次衰减到 0
    let frames_since_start = (tick - start_tick) as f32 / frame_ticks;
    let flash = (AURA_FLASH_FRAMES - frames_since_start).max(0.0).powi(2) / AURA_FLASH_DIVISOR;

    // 常态/收缩：长音符保持 AURA_HELD_FACTOR，最后 AURA_TAIL_SECONDS 内收缩到 0。
    // Zenith `maxAuraLen = tempoFrameStep * fps` 即每秒 tick 数，作为收缩窗口。
    let aura_len = tps * AURA_TAIL_SECONDS;
    let length = (end_tick - start_tick).max(1) as f32;
    let remaining = (end_tick - tick) as f32;
    let offset = remaining.min(aura_len);
    let len = length.min(aura_len);
    let tail = if len > 0.0 {
        (offset / len).powf(AURA_TAIL_POWER) * AURA_HELD_FACTOR
    } else {
        0.0
    };

    tail + flash
}

/// GPU-Driven 路径：单次遍历 `NoteInstance` 同时产出按键状态与每键光晕系数。
///
/// 与 legacy 两次全量扫描（`compute_active_keys`＋`build_aura_instances`内循环）
/// 数学等价：`start/end` 解码与 `render_from_instances` 换算逐 op 一致
/// （`max(0)`钳位＋`max(1)`长度），同键多音符取最后颜色、光晕取最大。
/// 返回 `(ActiveKeys, aura_sizes[128])`，调用方用 `emit_aura_instances` 落盘。
pub fn compute_active_and_aura_for_compact(
    tick: u32,
    ticks_per_second: f32,
    fps: f32,
    notes: &[NoteInstance],
) -> (ActiveKeys, [f32; 128]) {
    let mut pressed = [false; 128];
    let mut colors = [0u32; 128];
    let mut aura_sizes = [0.0f32; 128];
    for n in notes {
        let key = (n.key_color & 0xFF) as usize;
        if key >= 128 {
            continue;
        }
        // 与 `render_from_instances` 换算一致：start 钳零，长度至少 1 tick。
        let start = n.start_length[0].max(0.0) as u32;
        let end = start.saturating_add(n.start_length[1].max(1.0) as u32);
        if start <= tick && tick < end {
            pressed[key] = true;
            // `key_color` 高 24 位即 RGB，低 8 key 字节清零后补 alpha=0xFF，
            // 与 legacy `unpack→pack_color` 逐字节一致。
            colors[key] = (n.key_color & 0xFFFF_FF00) | 0xFF;
            let factor = aura_factor_raw(tick, ticks_per_second, fps, start, end);
            if factor > aura_sizes[key] {
                aura_sizes[key] = factor;
            }
        }
    }
    (ActiveKeys { pressed, colors }, aura_sizes)
}

/// 由 `(ActiveKeys, aura_sizes)` 落盘 Aura 实例（GPU-Driven 路径）。
///
/// 与 `build_aura_instances` 后半段（`pressed` 门控＋环几何）逐 op 一致，
/// 只是输入已由 `compute_active_and_aura_for_compact` 预聚合，省一次全量扫描。
pub fn emit_aura_instances(
    active_keys: &ActiveKeys,
    aura_sizes: &[f32; 128],
    key_count: usize,
    key_positions: &[f32],
    key_widths: &[f32],
    out: &mut Vec<MiditrailAuraInstanceGpu>,
) {
    let key_count = key_count
        .min(key_positions.len())
        .min(key_widths.len())
        .min(128);
    for key_idx in 0..key_count {
        if !active_keys.pressed[key_idx] {
            continue;
        }
        let aura = aura_sizes[key_idx];
        if aura <= 0.0 {
            continue;
        }
        let width = key_widths[key_idx];
        let center = key_positions[key_idx] + width * 0.5;
        let size = (width * AURA_RING_SCALE * aura).max(0.001);
        out.push(MiditrailAuraInstanceGpu {
            size,
            pos: center,
            color_packed: active_keys.colors[key_idx],
            _padding: 0,
        });
    }
}

#[cfg(test)]
mod tests;
