//! 瀑布流模式 GPU 渲染器
//!
//! 使用 compute shader 在 GPU 上直接渲染瀑布流帧，
//! 支持音符绘制、钢琴键盘（含活跃键高亮）、速度控制。
//!
//! 音符存储不归本渲染器所有：直接绑定权威 `NoteInstance` 常驻缓冲
//! （与钢琴卷帘 / 走带同源同缓冲，调用方传入），本渲染器只拥有
//! uniform、活跃键色、分桶偏移（派生索引，可忽略）与输出纹理。
//!
//! # 生命周期
//!
//! 1. `new()` — 创建渲染器，编译 compute shader
//! 2. `render()` — 每帧调用：上传 uniform/偏移/键色、dispatch compute shader、写入 storage texture
//! 3. `storage_texture()` — 获取输出纹理，供 export pipeline 读回

mod active;
#[cfg(test)]
mod active_tests;
mod bind;
mod init;
mod render;
#[cfg(test)]
mod test_harness;
#[cfg(test)]
mod tests;

use crate::ResidentCull;
use crate::gpu_resource_tracker::{TrackedBuffer, TrackedTexture};

pub use render::CullRenderOutcome;

/// Uniform 参数（与 waterfall.wgsl 中 WaterfallUniform 匹配）
#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct WaterfallUniformGpu {
    /// 当前播放 tick（决定瀑布流纵向位置）
    pub tick: u32,
    /// 分辨率（每四分音符 tick 数）
    pub ppq: u32,
    /// 琴键数量（决定渲染高度）
    pub key_count: u32,
    /// 帧宽度（像素）
    pub frame_width: u32,
    /// 帧高度（像素）
    pub frame_height: u32,
    /// 键盘分区高度（像素）
    pub kb_height: u32,
    /// 瀑布流下落速度（倍率）
    pub speed: f32,
    /// 对齐填充（保持 16 字节对齐）
    pub _padding: u32,
}

/// 瀑布流 GPU 渲染器
pub struct WaterfallRenderer {
    compute_pipeline: wgpu::ComputePipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    bind_group: Option<wgpu::BindGroup>,

    uniform_buffer: TrackedBuffer,
    active_key_colors_buffer: Option<TrackedBuffer>,
    key_offsets_buffer: Option<TrackedBuffer>,

    output_texture: Option<TrackedTexture>,
    output_texture_view: Option<wgpu::TextureView>,

    key_offsets_capacity: usize,
    current_width: u32,
    current_height: u32,

    /// 常驻全量窗口提取器（导出共享缓冲一次上传，桶一次构建，常驻复用）。
    resident_cull: ResidentCull,
    /// 活跃键内核管线/布局/参数（`waterfall_active.wgsl`；绑定组每帧重建，见 `active.rs`）。
    active_pipeline: Option<wgpu::ComputePipeline>,
    active_layout: Option<wgpu::BindGroupLayout>,
    active_params_buffer: Option<TrackedBuffer>,
}

/// 瀑布流可见 tick 跨度（与 shader `viewport_tick_span` 同公式，速度越高窗口越窄）。
///
/// UI 窗口收集（`collect_window_notes` 上界）与渲染侧 cull 窗口共用，保证谓词一致。
/// 注意 `.round()` 为 Rust 半远离零，WGSL `round` 为半偶数——`4/speed` 恰为
/// x.5 时差 1 tick（既有 UI/shader 缝隙，cull 从 UI 侧对齐，与现状逐位一致）。
pub fn waterfall_viewport_span(ppq: u32, speed: f32) -> u32 {
    let speed = speed.max(0.1);
    let ticks_per_measure = ppq * 4;
    let visible_measure_count = ((4.0 / speed).round()).max(1.0) as u32;
    (ticks_per_measure * visible_measure_count).max(1)
}
