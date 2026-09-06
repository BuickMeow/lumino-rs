//! 全局桶：常驻音符缓冲的一次性 GPU 排序索引。
//!
//! 背景：洋葱皮常驻缓冲为轨追加序（`OnionSegment` 段表依赖原址，不可移动字节）；
//! 瀑布流 / miditrail 导出需要 `(key, start)` 有序输入，但约束是不动 CPU 排布、
//! 不二次上传音符字节、不做二次拷贝重排。
//!
//! 方案：load 后（或首次导出前）调用 [`GlobalBucketIndex::build`] 在 GPU 上执行一次
//! 基数排序（见 `shaders/bucket_sort.wgsl`），产物为
//! `sort_index[N]`（置换索引，有序位置 `p` 的源下标为 `sort_index[p]`）与
//! `key_offsets[257]`（全局桶边界，桶 `k` 区间为有序位置
//! `[key_offsets[k], key_offsets[k+1])`，桶内按 `start_tick` 升序）。
//! 常驻字节原地不动（洋葱皮 / 钢琴卷帘零影响），此后导出每帧只读绑定，零上传、零 CPU。
//!
//! 并列 tiebreak：全相等 `(key, start)` 保持 load 原有相对顺序；legacy CPU
//! `sort_visible_notes` tiebreak 为 track 降序——差异由像素等价 harness 量化验收。

mod build;
mod cull;
#[cfg(test)]
mod cull_tests;
mod support;
#[cfg(test)]
mod tests;

pub use cull::{CullExtract, CullWindow, ResidentCull, prefix_counts};
pub(crate) use support::readback_bytes_sync;

use crate::gpu_resource_tracker::TrackedBuffer;

/// 全局桶 key 数量（key 为 u8 全范围，与 CPU `sort_visible_notes` 的 256 桶一致）。
pub const KEY_BUCKETS: usize = 256;
/// 直方图长度（每 digit 8 位）。
pub const HIST_LEN: usize = 256;
/// `key_offsets` 长度（256 桶边界 + 哨兵）。
pub const OFFSETS_LEN: usize = KEY_BUCKETS + 1;
/// 1KB 回读块字节数（256 × u32，直方图/桶边界传输单元）。
pub(crate) const HIST_BYTES: u64 = (HIST_LEN * std::mem::size_of::<u32>()) as u64;

/// 全局桶构建错误。
#[derive(Debug, thiserror::Error)]
pub enum GlobalBucketError {
    /// 音符数量超出 u32 索引范围。
    #[error("音符数量 {0} 超出 u32 索引范围")]
    CountOverflow(usize),
    /// 直方图回读 map 失败。
    #[error("直方图回读 map 失败: {0}")]
    MapFailed(String),
    /// 直方图回读超时（5s 未就绪）。
    #[error("直方图回读超时")]
    MapTimeout,
    /// 回读数据长度异常。
    #[error("回读数据长度异常：期望 {expected}，实际 {got}")]
    BadLength { expected: usize, got: usize },
    /// cull 内部资源缺失（管线/暂存未就绪，正常路径不应发生）。
    #[error("cull 内部资源缺失: {0}")]
    CullResource(&'static str),
}

/// 全局桶索引：一次构建，常驻复用。
///
/// `sort_index` 为 `u32 × N` 置换索引（`STORAGE`，供瀑布流 / miditrail 着色器只读绑定），
/// `key_offsets` 为 `u32 × 257` 全局桶边界（`STORAGE`，与 `waterfall.wgsl` 的
/// `key_offsets` 语义一致，区别是此处为全曲全局、一次构建）。
/// 数据变更（MIDI 重载 / TrackDelta）后调用方重建即可（构建为一次性成本，
/// 数据不变时永不重建）。
pub struct GlobalBucketIndex {
    sort_index: TrackedBuffer,
    key_offsets: TrackedBuffer,
    note_count: usize,
}

/// 全局桶构建源（调用方传入，渲染器内部缓存判定用）。
///
/// 打包常驻缓冲三要素，避免 cull 提取类接口参数爆炸；
/// miditrail 集成时复用同一结构。
#[derive(Debug)]
pub struct BucketSource<'a> {
    /// 常驻音符缓冲（`NoteInstance` 16B，要求 `STORAGE` 用途，只读）。
    pub buffer: &'a wgpu::Buffer,
    /// 有效音符数。
    pub count: usize,
    /// 常驻数据世代（`Renderers::onion_epoch`，内容变更即递增）。
    pub epoch: u64,
}

impl GlobalBucketIndex {
    /// 只读访问置换索引缓冲（供渲染管线绑定）。
    pub fn sort_index_buffer(&self) -> &wgpu::Buffer {
        self.sort_index.inner()
    }

    /// 只读访问全局桶边界缓冲（供渲染管线绑定）。
    pub fn key_offsets_buffer(&self) -> &wgpu::Buffer {
        self.key_offsets.inner()
    }

    /// 构建时音符数量。
    pub fn note_count(&self) -> usize {
        self.note_count
    }
}

/// 256 项直方图的互斥前缀和（CPU 侧，1KB 量级）。
///
/// `hist[d]` 为 digit `d` 的计数；返回 `pfx[d]` 为 digit `d` 在有序输出中的起始位置。
pub(crate) fn exclusive_prefix(hist: &[u32; HIST_LEN]) -> [u32; HIST_LEN] {
    let mut pfx = [0u32; HIST_LEN];
    let mut acc = 0u32;
    for (d, p) in hist.iter().zip(pfx.iter_mut()) {
        *p = acc;
        acc = acc.saturating_add(*d);
    }
    pfx
}

/// LSD pass 序列：`(shift, use_key)`。
///
/// 先按 `start_tick` 4 个字节稳定排序，再按 `key` 稳定分桶 → 终态 key 主序、start 次序。
/// 纯 u32 运算（WGSL u64 后端支持不完备，刻意避开）。
pub(crate) fn sort_passes() -> [(u32, bool); 5] {
    [(0, false), (8, false), (16, false), (24, false), (0, true)]
}
