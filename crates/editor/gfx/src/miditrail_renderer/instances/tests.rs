//! Miditrail 实例构建器单元测试
//!
//! 拆分说明（2026-08-18）：原文件 968 行超 clippy `too-many-lines-threshold`（400），
//! 按测试主题拆分为子模块：
//! - `keyboard`：琴键布局与实例构建（test_build_instances / test_black_key_press_depth_limited）
//! - `aura`：Aura 光晕环动画（按下闪光衰减 / 临近结束收缩 / 同键取最大 / 跳过未开始与已结束）
//! - `notes`：音符实例（深度排序 / 黑白键分组 / Z 远平面裁剪 / 窗口缩放等价性）
//! - `sort_equivalence`：u64 打包排序键与旧三键闭包排序等价性回归
//! - `top_view`：Top 视图实例数单调不增（精度降级收益）与键覆盖一致性
//! - `bench`：10 万音符实例构建性能基准

use super::*;

mod aura;
mod bench;
mod keyboard;
mod notes;
mod sort_equivalence;
mod top_view;

#[test]
fn test_black_keys() {
    assert!(is_black_key(1));
    assert!(is_black_key(61)); // C#4 + 5 octaves
    assert!(!is_black_key(0));
    assert!(!is_black_key(60));
}

#[test]
fn test_key_positions() {
    let mut positions = Vec::new();
    let mut widths = Vec::new();
    let mut last = 0u32;
    update_key_positions(128, &mut last, &mut positions, &mut widths);
    assert_eq!(positions.len(), 128);
    assert_eq!(widths.len(), 128);
    // 白键总宽度应约为 1.0
    let white_total: f32 = positions
        .iter()
        .enumerate()
        .filter(|(i, _)| !is_black_key(*i as isize))
        .map(|(i, _)| widths[i])
        .sum();
    assert!((white_total - 1.0).abs() < 1e-5);
    // 黑键应比相邻白键窄
    assert!(widths[1] < widths[0]);
}

#[test]
fn test_boost_color_packed_clamps_and_preserves_alpha() {
    // 0xRRGGBBAA 格式：红色 0.5 + 0.5 = 1.0，绿色 0 变 0.5，蓝色 0 变 0.5，alpha 保持不变
    let boosted = boost_color_packed(0x800000FF, 0.5);
    assert_eq!((boosted >> 24) & 0xFF, 0xFF);
    assert_eq!((boosted >> 16) & 0xFF, 0x7F);
    assert_eq!((boosted >> 8) & 0xFF, 0x7F);
    assert_eq!(boosted & 0xFF, 0xFF);
}
