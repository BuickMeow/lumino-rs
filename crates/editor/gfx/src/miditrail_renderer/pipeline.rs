//! Miditrail 3D 渲染管线创建
//!
//! Normal 与 Top 视图共用同一实例缓冲布局（见 `miditrail_top.wgsl` 头注释），
//! 区别仅在于着色器模块（3D 光照 vs flat）与深度写入策略（两者一致：
//! 音符不写深度、琴键写深度，琴键最后绘制覆盖音符）。

use super::MiditrailInstanceGpu;
use super::types::{MiditrailAuraInstanceGpu, MiditrailCameraGpu};
use crate::gpu_resource_tracker::TrackedBuffer;
use crate::pipeline::RenderPipelineBuilder;

pub fn create_bind_group_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("miditrail_bind_group_layout"),
        entries: &[
            // binding 0: 相机 uniform
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            // binding 1: aura 纹理采样器
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
            // binding 2: aura 纹理
            wgpu::BindGroupLayoutEntry {
                binding: 2,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
        ],
    })
}

pub fn create_render_pipeline(
    device: &wgpu::Device,
    bind_group_layout: &wgpu::BindGroupLayout,
    shader: &wgpu::ShaderModule,
) -> wgpu::RenderPipeline {
    create_instanced_pipeline(
        device,
        bind_group_layout,
        shader,
        "miditrail_render_pipeline",
        true,
    )
}

/// 创建音符渲染管线（不写入深度缓冲，配合 Painter's algorithm 与琴键最后绘制）。
///
/// 参考 Comet MIDITrail：音符先绘制且不写深度，琴键后绘制使用深度测试，
/// 从而保证琴键始终覆盖在音符之上。
pub fn create_note_render_pipeline(
    device: &wgpu::Device,
    bind_group_layout: &wgpu::BindGroupLayout,
    shader: &wgpu::ShaderModule,
) -> wgpu::RenderPipeline {
    create_instanced_pipeline(
        device,
        bind_group_layout,
        shader,
        "miditrail_note_render_pipeline",
        false,
    )
}

/// 创建 Top 视图琴键渲染管线（flat 着色，写深度，琴键最后绘制覆盖音符）。
pub fn create_top_render_pipeline(
    device: &wgpu::Device,
    bind_group_layout: &wgpu::BindGroupLayout,
    shader: &wgpu::ShaderModule,
) -> wgpu::RenderPipeline {
    create_instanced_pipeline(
        device,
        bind_group_layout,
        shader,
        "miditrail_top_render_pipeline",
        true,
    )
}

/// 创建 Top 视图音符渲染管线（flat 着色，不写深度，配合画家算法）。
pub fn create_top_note_render_pipeline(
    device: &wgpu::Device,
    bind_group_layout: &wgpu::BindGroupLayout,
    shader: &wgpu::ShaderModule,
) -> wgpu::RenderPipeline {
    create_instanced_pipeline(
        device,
        bind_group_layout,
        shader,
        "miditrail_top_note_render_pipeline",
        false,
    )
}

/// 创建 GPU-Driven 音符渲染管线（Normal 视图终局路径）。
///
/// 与旧 `note_pipeline` 的两处关键差异：
/// - 实例布局 = `NoteInstance` 原字节（16B：start_length + key_color + border），
///   位姿由 `miditrail_note_driven.wgsl` 按实例实时推导；
/// - `depth_write=true`：不透明音符用深度测试解决遮挡，CPU 画家排序删除。
/// 琴键仍走旧管线最后绘制（compare 已改为 Always，永远置顶，观感不变）。
pub fn create_note_driven_pipeline(
    device: &wgpu::Device,
    bind_group_layout: &wgpu::BindGroupLayout,
    driven_group_layout: &wgpu::BindGroupLayout,
    shader: &wgpu::ShaderModule,
) -> wgpu::RenderPipeline {
    let pos_normal_layout = wgpu::VertexBufferLayout {
        array_stride: 24,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &[
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32x3,
                offset: 0,
                shader_location: 0,
            },
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32x3,
                offset: 12,
                shader_location: 1,
            },
        ],
    };
    // 紧凑实例：vec2(start, length) + u32(key_color) + u32(border)，stride 16。
    let compact_layout = wgpu::VertexBufferLayout {
        array_stride: 16,
        step_mode: wgpu::VertexStepMode::Instance,
        attributes: &[
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32x2,
                offset: 0,
                shader_location: 2,
            },
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Uint32,
                offset: 8,
                shader_location: 3,
            },
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Uint32,
                offset: 12,
                shader_location: 4,
            },
        ],
    };

    RenderPipelineBuilder::new(device, "miditrail_note_driven_pipeline", shader)
        .bind_group(bind_group_layout)
        .bind_group(driven_group_layout)
        .vertex_buffer(pos_normal_layout)
        .vertex_buffer(compact_layout)
        .opaque_target(wgpu::TextureFormat::Rgba8Unorm)
        .depth_stencil(Some(wgpu::DepthStencilState {
            format: wgpu::TextureFormat::Depth32Float,
            depth_write_enabled: true,
            depth_compare: wgpu::CompareFunction::LessEqual,
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        }))
        .build()
}

/// GPU-Driven 参数组布局（group1：位姿参数＋键位表 uniform，顶点着色器只读）。
pub fn create_driven_group_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("miditrail_driven_group_layout"),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::VERTEX,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        }],
    })
}

fn create_instanced_pipeline(
    device: &wgpu::Device,
    bind_group_layout: &wgpu::BindGroupLayout,
    shader: &wgpu::ShaderModule,
    label: &str,
    depth_write: bool,
) -> wgpu::RenderPipeline {
    // 顶点位置与法线
    let pos_normal_layout = wgpu::VertexBufferLayout {
        array_stride: 24,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &[
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32x3,
                offset: 0,
                shader_location: 0,
            },
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32x3,
                offset: 12,
                shader_location: 1,
            },
        ],
    };
    // 实例数据
    let instance_layout = wgpu::VertexBufferLayout {
        array_stride: std::mem::size_of::<MiditrailInstanceGpu>() as u64,
        step_mode: wgpu::VertexStepMode::Instance,
        attributes: &[
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32x3,
                offset: 0,
                shader_location: 2,
            },
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32x3,
                offset: 16,
                shader_location: 3,
            },
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Uint32,
                offset: 32,
                shader_location: 4,
            },
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Uint32,
                offset: 36,
                shader_location: 5,
            },
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32,
                offset: 40,
                shader_location: 6,
            },
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32,
                offset: 44,
                shader_location: 7,
            },
        ],
    };

    RenderPipelineBuilder::new(device, label, shader)
        .bind_group(bind_group_layout)
        .vertex_buffer(pos_normal_layout)
        .vertex_buffer(instance_layout)
        .opaque_target(wgpu::TextureFormat::Rgba8Unorm)
        .depth_stencil(Some(wgpu::DepthStencilState {
            format: wgpu::TextureFormat::Depth32Float,
            depth_write_enabled: depth_write,
            // 回退到 LessEqual（2026-09-05 driven 实验前的原始状态）：
            // UI 实测键盘顶层/前面层异常，先恢复最后已知良好状态再查根因。
            depth_compare: wgpu::CompareFunction::LessEqual,
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        }))
        .build()
}

pub fn create_aura_render_pipeline(
    device: &wgpu::Device,
    bind_group_layout: &wgpu::BindGroupLayout,
    shader: &wgpu::ShaderModule,
) -> wgpu::RenderPipeline {
    // 顶点位置与 UV
    let pos_uv_layout = wgpu::VertexBufferLayout {
        array_stride: 16,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &[
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32x2,
                offset: 0,
                shader_location: 0,
            },
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32x2,
                offset: 8,
                shader_location: 1,
            },
        ],
    };
    // Aura 实例数据
    let aura_instance_layout = wgpu::VertexBufferLayout {
        array_stride: std::mem::size_of::<MiditrailAuraInstanceGpu>() as u64,
        step_mode: wgpu::VertexStepMode::Instance,
        attributes: &[
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32,
                offset: 0,
                shader_location: 2,
            },
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32,
                offset: 4,
                shader_location: 3,
            },
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Uint32,
                offset: 8,
                shader_location: 4,
            },
        ],
    };

    RenderPipelineBuilder::new(device, "miditrail_aura_render_pipeline", shader)
        .bind_group(bind_group_layout)
        .vertex_buffer(pos_uv_layout)
        .vertex_buffer(aura_instance_layout)
        // 叠加混合（SrcAlpha, One）
        .color_target(wgpu::ColorTargetState {
            format: wgpu::TextureFormat::Rgba8Unorm,
            blend: Some(wgpu::BlendState {
                color: wgpu::BlendComponent {
                    src_factor: wgpu::BlendFactor::SrcAlpha,
                    dst_factor: wgpu::BlendFactor::One,
                    operation: wgpu::BlendOperation::Add,
                },
                alpha: wgpu::BlendComponent {
                    src_factor: wgpu::BlendFactor::SrcAlpha,
                    dst_factor: wgpu::BlendFactor::One,
                    operation: wgpu::BlendOperation::Add,
                },
            }),
            write_mask: wgpu::ColorWrites::ALL,
        })
        .depth_stencil(Some(wgpu::DepthStencilState {
            format: wgpu::TextureFormat::Depth32Float,
            depth_write_enabled: false,
            depth_compare: wgpu::CompareFunction::Always,
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        }))
        .build()
}

pub fn create_buffers(
    device: &wgpu::Device,
    vertices: &[f32],
    indices: &[u16],
) -> (TrackedBuffer, TrackedBuffer, TrackedBuffer) {
    let uniform_buffer = TrackedBuffer::new(
        device,
        &wgpu::BufferDescriptor {
            label: Some("miditrail_camera_uniform_buffer"),
            size: std::mem::size_of::<MiditrailCameraGpu>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        },
    );

    let vertex_buffer = TrackedBuffer::new_init(
        device,
        &wgpu::util::BufferInitDescriptor {
            label: Some("miditrail_cube_vertex_buffer"),
            contents: bytemuck::cast_slice(vertices),
            usage: wgpu::BufferUsages::VERTEX,
        },
    );

    let index_buffer = TrackedBuffer::new_init(
        device,
        &wgpu::util::BufferInitDescriptor {
            label: Some("miditrail_cube_index_buffer"),
            contents: bytemuck::cast_slice(indices),
            usage: wgpu::BufferUsages::INDEX,
        },
    );

    (uniform_buffer, vertex_buffer, index_buffer)
}

/// 音符平面索引缓冲（`QUAD_INDICES`，6×u16 常驻；仅音符 draw 绑定）。
pub fn create_quad_index_buffer(device: &wgpu::Device, indices: &[u16]) -> TrackedBuffer {
    TrackedBuffer::new_init(
        device,
        &wgpu::util::BufferInitDescriptor {
            label: Some("miditrail_quad_index_buffer"),
            contents: bytemuck::cast_slice(indices),
            usage: wgpu::BufferUsages::INDEX,
        },
    )
}
