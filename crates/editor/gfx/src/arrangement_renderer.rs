//! 工程走带渲染器
//!
//! 使用 WGPU 实例化渲染 (Instance Rendering) 高效渲染工程走带视图，
//! 参考 yinhe 实现，包含音轨 lane 背景、小节网格线、音符实例和演奏指示线。
//!
//! 渲染方式：Vertex Buffer + Instance Rendering (TriangleList)
//! 每个实例包含 xywh (屏幕空间坐标) + packed 颜色/属性数据
//!
//! 音符层（走带分音轨）：直接复用钢琴卷帘常驻 GPU 音符缓冲
//! （`onion_skin.gpu_note_buffer`），不再维护第二份音符缓冲 / 每帧重建，
//! 仅按可见音轨分段 draw，泳道映射通过 `lane_index` 存储缓冲完成。

mod draw;
mod init;
mod prepare;
mod types;

use crate::gpu_resource_tracker::TrackedBuffer;

pub use types::{ArrangementNoteInstance, ArrangementNoteUniform, ArrangementUniform, colors};

/// 走带视图渲染器
pub struct ArrangementRenderer {
    /// 覆盖层渲染管线（背景/lane/网格/框选/指示线，每帧重建的屏幕空间实例）
    pipeline: wgpu::RenderPipeline,
    /// 覆盖层 Uniform 缓冲区
    uniform_buffer: TrackedBuffer,
    /// 覆盖层实例缓冲区
    overlay_buffer: TrackedBuffer,
    /// 覆盖层当前容量
    overlay_capacity: usize,
    /// 覆盖层当前实例数
    overlay_count: u32,
    /// 覆盖层中"背景层"实例数（背景/lane/网格），绘制时排在音符之下
    overlay_back_len: u32,
    /// 覆盖层 Bind group
    bind_group: wgpu::BindGroup,

    /// 音符渲染管线 —— 复用钢琴卷帘常驻 GPU 音符缓冲（零第二份显存）
    note_pipeline: wgpu::RenderPipeline,
    /// GPU 裁剪计算管线（逐音符判定可见性，输出源索引 + indirect 计数）
    note_cull_pipeline: wgpu::ComputePipeline,
    /// 音符着色器 Uniform（滚动/缩放/泳道高/画布偏移）
    note_uniform_buffer: TrackedBuffer,
    /// 文档音轨 → 走带泳道序号 映射（存储缓冲，按 doc track 索引）
    lane_index_buffer: TrackedBuffer,
    /// lane_index 容量（f32 元素数）
    lane_index_capacity: usize,
    /// 音符渲染 Bind group（uniform + lane_index 存储 + 全部实例存储）
    note_draw_bind_group: Option<wgpu::BindGroup>,
    /// 音符裁剪 Bind group（uniform + cull_info + 实例 + lane_index + 可见索引 + indirect）
    note_cull_bind_group: Option<wgpu::BindGroup>,
    /// 裁剪 Bind group layout（计算阶段）
    note_cull_bind_group_layout: wgpu::BindGroupLayout,
    /// 渲染 Bind group layout（绘制阶段）
    note_draw_bind_group_layout: wgpu::BindGroupLayout,
    /// cull 输出：可见实例的全局源索引（u32，作为绘制阶段顶点缓冲）
    note_visible_buffer: TrackedBuffer,
    /// 间接绘制参数（DrawIndirectArgs：vertex_count=4, instance_count=可见数）
    note_indirect_buffer: TrackedBuffer,
    /// 裁剪 uniform（instance_count / chunk_start / chunk_count）
    cull_info_buffer: TrackedBuffer,
    /// 共享的钢琴卷帘常驻音符缓冲（GPU，按 NoteInstance 布局分段）
    note_source: wgpu::Buffer,
    /// 当前共享缓冲中的实例总数（cull 循环上界）
    note_instance_count: u32,
}

/// 顶点着色器代码
const VERTEX_SHADER: &str = include_str!("shaders/arrangement.wgsl");

/// 音符 GPU 裁剪计算着色器
const NOTE_CULL_SHADER: &str = include_str!("shaders/arrangement_cull.wgsl");

/// 初始实例缓冲区大小
const INITIAL_CAPACITY: usize = 4096;

/// lane_index 初始容量（文档音轨数上限的保守值）
const INITIAL_LANE_CAPACITY: usize = 1024;

/// 可见索引缓冲初始容量（全局源索引 u32 个数）
const INITIAL_VISIBLE_CAPACITY: usize = 1 << 20;
