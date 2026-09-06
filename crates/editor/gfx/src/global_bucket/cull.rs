//! 全局桶窗口提取：常驻全量 → 每帧 GPU cull → 有序窗口。
//!
//! 背景：`waterfall_indexed.wgsl` 的逐像素桶内回溯（SEARCH_BUFFER=128）按窗口
//! 桶密度标定，全量历史入桶后死音符消耗预算、密集段漏检；因此导出改走
//! “cull 提取窗口 → legacy 精确渲染”（见 `shaders/bucket_cull.wgsl` 头注）。
//!
//! 两阶段 key 分区提取（输出 `(key, start)` 有序，与 `sort_visible_notes` 同序）：
//! - COUNT（`cull_extract.rs::extract_count`）：每 key 一线程计数 → 1KB 回读；
//! - 前缀和（`prefix_counts`，调用方复用派生 legacy `key_offsets`）；
//! - FILL（`cull_extract.rs::extract_fill`）：同构重扫写 compact（调用方 encoder
//!   追加，无原子、无竞争）。
//!
//! 常驻由调用方持有（瀑布流：导出共享缓冲；miditrail：自有全量缓冲），本结构只
//! 拥有桶 + cull 管线/暂存；`(字节数, 数量, 世代)` 任一变化即重建桶（一次性）。
//! 世代由调用方在上传后 `mark_resident_updated` 递增（导出缓冲内容只增不改，
//! 跨导出天然唯一，无需外部时钟）。
//!
//! 零长边界：谓词 `end` 按打包语义 `start + max(len, 1.0)`（与 legacy shader
//! 同式）；UI 窗口按原始 `end_tick`。零长音符恰落窗口下界时 cull 多收 1px，
//! 属退化输入分歧（legacy shader 本就会渲染 1px），harness 用非零长数据覆盖
//! 主路径，生产与现状逐位一致（等价测试断言）。

mod extract;
mod setup;

use super::{GlobalBucketError, KEY_BUCKETS};
use crate::gpu_resource_tracker::TrackedBuffer;

/// cull 窗口（tick 半开区间 + 有效 key 数；COUNT/FILL/cull 调用方共用一体）。
#[derive(Debug, Clone, Copy)]
pub struct CullWindow {
    /// 窗口下界（含；`end_tick > tick_start` 方可见，跨视口长音符在内）。
    pub tick_start: u32,
    /// 窗口上界（不含；`start_tick < tick_end`）。
    pub tick_end: u32,
    /// 有效 key 数（钳制 256；桶全局 257 项恒覆盖）。
    pub key_count: usize,
}

/// cull 参数 uniform（与 `bucket_cull.wgsl` `CullParams` 同布局，32B）。
#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub(super) struct CullParamsGpu {
    pub tick_start: u32,
    pub tick_end: u32,
    pub key_count: u32,
    pub phase: u32,
    pub total_count: u32,
    pub _pad: [u32; 3],
}

/// 窗口提取产物（compact 缓冲由本结构持有，调用方只读绑定/回读）。
#[derive(Debug, Clone)]
pub struct CullExtract {
    /// 每 key 窗口计数（256 项，`key_count` 之外恒为 0）。
    pub counts: [u32; KEY_BUCKETS],
    /// 窗口总数（`counts` 求和）。
    pub total: usize,
    /// 本次提取是否重建了桶（调用方据此失效依赖桶句柄的绑定组，如活跃键内核组）。
    pub bucket_rebuilt: bool,
}

/// 常驻全量窗口提取器：桶（一次构建）+ cull 管线/暂存（常驻复用）。
#[derive(Default)]
pub struct ResidentCull {
    pub(super) bucket: Option<super::GlobalBucketIndex>,
    pub(super) src_bytes: u64,
    pub(super) src_count: usize,
    pub(super) src_seq: u64,
    pub(super) seq: u64,
    pub(super) bucket_rebuilt_flag: bool,
    pub(super) pipeline: Option<wgpu::ComputePipeline>,
    pub(super) layout: Option<wgpu::BindGroupLayout>,
    pub(super) bind_group: Option<wgpu::BindGroup>,
    pub(super) params_buffer: Option<TrackedBuffer>,
    pub(super) counts_buffer: Option<TrackedBuffer>,
    pub(super) counts_staging: Option<TrackedBuffer>,
    pub(super) base_buffer: Option<TrackedBuffer>,
    pub(super) compact_buffer: Option<TrackedBuffer>,
    pub(super) compact_capacity: usize,
    /// 单调游标缓冲（256×u32，初值 0；见 `bucket_cull.wgsl` 头注）。
    pub(super) cursor_buffer: Option<TrackedBuffer>,
    /// 上次 COUNT 的 `tick_start`（倒退检测用；初值 0，到达过即单调记忆）。
    pub(super) last_tick_start: u32,
}

impl ResidentCull {
    /// 创建提取器（管线/暂存均懒初始化，首次提取时创建）。
    pub fn new() -> Self {
        Self::default()
    }

    /// 常驻上传后调用：世代递增，下次提取重建桶（一次性成本）。
    pub fn mark_resident_updated(&mut self) {
        self.seq = self.seq.wrapping_add(1);
    }

    /// 释放导出常驻全套 GPU 资源（桶 + cull 管线暂存 + compact + 游标）。
    ///
    /// 调用方（渲染器 `release_export_resources`）在导出完成/取消后调用；
    /// 常驻缓冲本身由调用方持有释放。下次 `seed` 会递增世代，`ensure_bucket`
    /// 见桶缺失即重建——释放后冷启动行为与首启一致，无残留句柄。
    /// 管线/布局（`pipeline`/`layout`，KB 级编译产物）有意保留：重建要重新
    /// 编译 shader，留着给下次导出复用；占用的只是小常量。
    pub fn release(&mut self) {
        self.bucket = None;
        self.src_bytes = 0;
        self.src_count = 0;
        self.src_seq = 0;
        self.bucket_rebuilt_flag = false;
        self.bind_group = None;
        self.params_buffer = None;
        self.counts_buffer = None;
        self.counts_staging = None;
        self.base_buffer = None;
        self.compact_buffer = None;
        self.compact_capacity = 0;
        self.cursor_buffer = None;
        self.last_tick_start = 0;
    }

    /// 只读访问 compact 缓冲（调用方绑定只读或回读；提取成功且总数 > 0 时有效）。
    pub fn compact_buffer(&self) -> Option<&wgpu::Buffer> {
        self.compact_buffer.as_ref().map(|b| b.inner())
    }

    /// 只读访问全局桶边界缓冲（活跃键内核绑定用；计数成功后有效）。
    pub fn bucket_key_offsets(&self) -> Option<&wgpu::Buffer> {
        self.bucket.as_ref().map(|b| b.key_offsets_buffer())
    }

    /// 只读访问置换索引缓冲（活跃键内核绑定用；计数成功后有效）。
    pub fn bucket_sort_index(&self) -> Option<&wgpu::Buffer> {
        self.bucket.as_ref().map(|b| b.sort_index_buffer())
    }
}

/// 由每 key 计数派生 legacy `key_offsets` 与 FILL 基址（CPU 侧，1KB 量级）。
///
/// - `offsets`（`key_count + 1` 项）：`offsets[k]` = key `k` 在 compact 中的起始，
///   与 `note_instances_to_key_offsets` 同语义（legacy 桶内二分前提）；
/// - `bases`（256 项全填，尾部补 0）：FILL 写地址；
/// - 返回总数 `total`。
pub fn prefix_counts(
    counts: &[u32; KEY_BUCKETS],
    key_count: usize,
) -> (Vec<u32>, [u32; KEY_BUCKETS], usize) {
    let key_count = key_count.min(KEY_BUCKETS);
    let mut offsets = vec![0u32; key_count + 1];
    let mut bases = [0u32; KEY_BUCKETS];
    let mut acc = 0u32;
    for ((o, b), &c) in offsets
        .iter_mut()
        .take(key_count)
        .zip(bases.iter_mut())
        .zip(counts.iter())
    {
        *o = acc;
        *b = acc;
        acc = acc.saturating_add(c);
    }
    offsets[key_count] = acc;
    (offsets, bases, acc as usize)
}

/// cull 内部资源缺失（管线/暂存未就绪，正常路径不应发生）。
pub(super) fn missing(what: &'static str) -> GlobalBucketError {
    GlobalBucketError::CullResource(what)
}
