//! 视频导出 BGRA 帧绘制辅助
//!
//! 位图字体（draw_digit / DIGIT_BITMAPS 等）集中于此，
//! 并通过私有 use 在 video_export.rs 的标尺小节号合成中使用。
//! 黑底填充（fill_bgra_black）供计数器 / 数据曲线帧复用。

/// 将 BGRA 帧数据填充为黑色背景（使用 bulk fill + alpha 修复）
pub(super) fn fill_bgra_black(frame: &mut [u8]) {
    frame.fill(0);
    for a in frame.iter_mut().skip(3).step_by(4) {
        *a = 255;
    }
}

/// 5x7 位图字体：数字 0-9
///
/// 每个数字 5 列宽、7 行高，每行用一个 u8 位掩码表示（LSB = 左端像素）。
const DIGIT_BITMAPS: [[u8; 7]; 10] = [
    // 0
    [
        0b01110, 0b10001, 0b10011, 0b10101, 0b11001, 0b10001, 0b01110,
    ],
    // 1
    [
        0b00100, 0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110,
    ],
    // 2
    [
        0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0b01000, 0b11111,
    ],
    // 3
    [
        0b01110, 0b10001, 0b00001, 0b00110, 0b00001, 0b10001, 0b01110,
    ],
    // 4
    [
        0b00010, 0b00110, 0b01010, 0b10010, 0b11111, 0b00010, 0b00010,
    ],
    // 5
    [
        0b11111, 0b10000, 0b11110, 0b00001, 0b00001, 0b10001, 0b01110,
    ],
    // 6
    [
        0b01110, 0b10000, 0b10000, 0b11110, 0b10001, 0b10001, 0b01110,
    ],
    // 7
    [
        0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b01000, 0b01000,
    ],
    // 8
    [
        0b01110, 0b10001, 0b10001, 0b01110, 0b10001, 0b10001, 0b01110,
    ],
    // 9
    [
        0b01110, 0b10001, 0b10001, 0b01111, 0b00001, 0b00001, 0b01110,
    ],
];

/// 位图字体原始尺寸（列数）
pub(super) const DIGIT_BITMAP_W: u32 = 5;
/// 位图字体原始尺寸（行数）
pub(super) const DIGIT_BITMAP_H: u32 = 7;
/// 缩放倍数
pub(super) const DIGIT_SCALE: u32 = 2;
/// 数字位图渲染后宽度（像素）
pub(super) const DIGIT_W: u32 = DIGIT_BITMAP_W * DIGIT_SCALE;
/// 数字位图渲染后高度（像素）
pub(super) const DIGIT_H: u32 = DIGIT_BITMAP_H * DIGIT_SCALE;
/// 数字间距（像素）
pub(super) const DIGIT_SPACING: u32 = DIGIT_SCALE;

/// 在 BGRA 帧数据上绘制一个数字字符（2x 缩放）
///
/// 每个位图像素渲染为 `DIGIT_SCALE × DIGIT_SCALE` 的方块。
/// `x`、`y` 为左上角像素坐标。
/// `color` 为 BGRA 颜色值（[B, G, R, A]）。
pub(super) fn draw_digit(
    frame: &mut [u8],
    frame_width: u32,
    digit: u8,
    x: u32,
    y: u32,
    color: [u8; 4],
) {
    let Some(bitmap) = DIGIT_BITMAPS.get(digit as usize) else {
        return;
    };
    let frame_w = frame_width as usize;
    let row_bytes = frame_w * 4;
    let color_bytes = color;

    for row in 0..DIGIT_BITMAP_H {
        let mask = bitmap[row as usize];
        if mask == 0 {
            continue;
        }
        let base_row_start = ((y + row * DIGIT_SCALE) as usize) * row_bytes;

        for col in 0..DIGIT_BITMAP_W {
            if mask & (1 << (DIGIT_BITMAP_W - 1 - col)) == 0 {
                continue;
            }
            let block_x_bytes = ((x + col * DIGIT_SCALE) as usize) * 4;

            for sy in 0..DIGIT_SCALE {
                let row_start = base_row_start + (sy as usize) * row_bytes + block_x_bytes;
                let row_end = row_start + (DIGIT_SCALE as usize) * 4;
                if row_end <= frame.len() {
                    for px_offset in (0..DIGIT_SCALE as usize * 4).step_by(4) {
                        let dst = row_start + px_offset;
                        frame[dst..dst + 4].copy_from_slice(&color_bytes);
                    }
                }
            }
        }
    }
}
