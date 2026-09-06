use super::{
    ArrangementNoteUniform, ArrangementRenderer, ArrangementUniform, INITIAL_CAPACITY,
    INITIAL_LANE_CAPACITY, INITIAL_VISIBLE_CAPACITY, NOTE_CULL_SHADER, VERTEX_SHADER,
};
use crate::gpu_resource_tracker::{self, TrackedBuffer};
use crate::pipeline::{ComputePipelineBuilder, RenderPipelineBuilder};
use crate::shader::create_shader_module;

/// 走带音符着色器（直接复用钢琴卷帘常驻 GPU 音符缓冲）
const NOTE_VERTEX_SHADER: &str = include_str!("../shaders/arrangement_note.wgsl");

impl ArrangementRenderer {
    /// 创建新的走带渲染器（默认带 depth attachment）
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        Self::new_with_depth(device, format, true)
    }

    /// 创建不带 depth attachment 的走带渲染器（用于视频导出等纯 2D 路径）
    pub fn new_without_depth(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        Self::new_with_depth(device, format, false)
    }

    fn new_with_depth(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        needs_depth: bool,
    ) -> Self {
        // ── 覆盖层管线（屏幕空间实例，每帧重建）──
        let shader = create_shader_module(device, "arrangement_shader", VERTEX_SHADER);

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("arrangement_bind_group_layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });

        let pipeline = RenderPipelineBuilder::new(device, "arrangement_pipeline", &shader)
            .bind_group(&bind_group_layout)
            .vertex_buffer(wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<super::ArrangementNoteInstance>() as u64,
                step_mode: wgpu::VertexStepMode::Instance,
                attributes: &[
                    wgpu::VertexAttribute {
                        format: wgpu::VertexFormat::Float32x4,
                        offset: 0,
                        shader_location: 0,
                    },
                    wgpu::VertexAttribute {
                        format: wgpu::VertexFormat::Uint32x4,
                        offset: 16,
                        shader_location: 1,
                    },
                ],
            })
            .alpha_blended_target(format)
            .depth_stencil(crate::constants::rendering::depth_stencil_state_for(
                needs_depth,
            ))
            .build();

        let uniform_buffer = TrackedBuffer::new_init(
            device,
            &wgpu::util::BufferInitDescriptor {
                label: Some("arrangement_uniform"),
                contents: bytemuck::cast_slice(&[ArrangementUniform::default()]),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            },
        );

        let overlay_buffer = gpu_resource_tracker::create_instance_buffer::<
            super::ArrangementNoteInstance,
        >(device, "arrangement_overlay_buffer", INITIAL_CAPACITY);

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("arrangement_bind_group"),
            layout: &bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buffer.inner().as_entire_binding(),
            }],
        });

        // ── 音符管线：复用钢琴卷帘常驻 GPU 音符缓冲（零第二份显存）──
        // 顶点输入改为 cull 阶段输出的 u32 源索引（instance-step），
        // VS 从 all_instances storage buffer 回查原实例。
        let note_shader =
            create_shader_module(device, "arrangement_note_shader", NOTE_VERTEX_SHADER);

        // 绘制阶段 bind group layout：uniform + lane_index 存储 + 全部实例存储
        let note_draw_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("arrangement_note_draw_bind_group_layout"),
                entries: &[
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
                    // lane_index[track]：文档音轨 → 走带泳道序号（只读存储，顶点阶段读取）
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::VERTEX,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Storage { read_only: true },
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                    // all_instances：全部音符实例（只读存储，cull/draw 共用同一份）
                    wgpu::BindGroupLayoutEntry {
                        binding: 2,
                        visibility: wgpu::ShaderStages::VERTEX,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Storage { read_only: true },
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                ],
            });

        let note_pipeline =
            RenderPipelineBuilder::new(device, "arrangement_note_pipeline", &note_shader)
                .bind_group(&note_draw_bind_group_layout)
                // 顶点输入：u32 源索引（每 4 顶点一个实例，instance-step）
                .vertex_buffer(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<u32>() as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &[wgpu::VertexAttribute {
                        format: wgpu::VertexFormat::Uint32,
                        offset: 0,
                        shader_location: 0,
                    }],
                })
                .triangle_strip()
                .alpha_blended_target(format)
                .depth_stencil(crate::constants::rendering::depth_stencil_state_for(
                    needs_depth,
                ))
                .build();

        // 裁剪阶段 bind group layout（计算着色器）
        let note_cull_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("arrangement_note_cull_bind_group_layout"),
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::COMPUTE,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Uniform,
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::COMPUTE,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Uniform,
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 2,
                        visibility: wgpu::ShaderStages::COMPUTE,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Storage { read_only: true },
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 3,
                        visibility: wgpu::ShaderStages::COMPUTE,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Storage { read_only: true },
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 4,
                        visibility: wgpu::ShaderStages::COMPUTE,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Storage { read_only: false },
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 5,
                        visibility: wgpu::ShaderStages::COMPUTE,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Storage { read_only: false },
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                ],
            });

        let note_cull_pipeline = ComputePipelineBuilder::new(
            device,
            "arrangement_note_cull_pipeline",
            &create_shader_module(device, "arrangement_note_cull_shader", NOTE_CULL_SHADER),
        )
        .bind_group(&note_cull_bind_group_layout)
        .build();

        let note_uniform_buffer = TrackedBuffer::new_init(
            device,
            &wgpu::util::BufferInitDescriptor {
                label: Some("arrangement_note_uniform"),
                contents: bytemuck::cast_slice(&[ArrangementNoteUniform::default()]),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            },
        );

        let lane_index_buffer = TrackedBuffer::new(
            device,
            &wgpu::BufferDescriptor {
                label: Some("arrangement_lane_index_buffer"),
                size: (INITIAL_LANE_CAPACITY * std::mem::size_of::<f32>()) as wgpu::BufferAddress,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            },
        );

        // cull 输出：可见实例的全局源索引（u32，作为绘制阶段顶点缓冲）
        let note_visible_buffer = TrackedBuffer::new(
            device,
            &wgpu::BufferDescriptor {
                label: Some("arrangement_note_visible_buffer"),
                size: (INITIAL_VISIBLE_CAPACITY * std::mem::size_of::<u32>())
                    as wgpu::BufferAddress,
                usage: wgpu::BufferUsages::STORAGE
                    | wgpu::BufferUsages::VERTEX
                    | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            },
        );

        // 间接绘制参数（DrawIndirectArgs）：vertex_count=4, instance_count=可见数
        let note_indirect_buffer = TrackedBuffer::new(
            device,
            &wgpu::BufferDescriptor {
                label: Some("arrangement_note_indirect_buffer"),
                size: 256,
                usage: wgpu::BufferUsages::INDIRECT
                    | wgpu::BufferUsages::STORAGE
                    | wgpu::BufferUsages::COPY_DST
                    | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            },
        );

        // 裁剪 uniform（instance_count / chunk_start / chunk_count）
        let cull_info_buffer = TrackedBuffer::new_init(
            device,
            &wgpu::util::BufferInitDescriptor {
                label: Some("arrangement_cull_info_uniform"),
                contents: bytemuck::cast_slice(&[crate::note_renderer::types::CullUniform {
                    instance_count: 0,
                    chunk_start: 0,
                    chunk_count: 0,
                    _padding: 0,
                }]),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            },
        );

        let placeholder_source = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("arrangement_note_source_placeholder"),
            size: 16,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::VERTEX,
            mapped_at_creation: false,
        });

        Self {
            pipeline,
            uniform_buffer,
            overlay_buffer,
            overlay_capacity: INITIAL_CAPACITY,
            overlay_count: 0,
            overlay_back_len: 0,
            bind_group,
            note_pipeline,
            note_cull_pipeline,
            note_uniform_buffer,
            lane_index_buffer,
            lane_index_capacity: INITIAL_LANE_CAPACITY,
            note_draw_bind_group: None,
            note_cull_bind_group: None,
            note_cull_bind_group_layout,
            note_draw_bind_group_layout,
            note_visible_buffer,
            note_indirect_buffer,
            cull_info_buffer,
            note_source: placeholder_source,
            note_instance_count: 0,
        }
    }

    /// 确保 instance buffer 容量足够（旧缓冲由 [`TrackedBuffer`] Drop 自动注销）
    pub(super) fn ensure_capacity(
        instance_buffer: &mut TrackedBuffer,
        capacity: &mut usize,
        device: &wgpu::Device,
        instance_count: usize,
    ) {
        let needed = instance_count.next_power_of_two().max(INITIAL_CAPACITY);
        if needed > *capacity {
            *capacity = needed;
            *instance_buffer = gpu_resource_tracker::create_instance_buffer::<
                super::ArrangementNoteInstance,
            >(device, "arrangement_instance_buffer", needed);
        }
    }

    /// 确保 lane_index 存储缓冲容量足够（按 f32 元素数计）
    pub(super) fn ensure_lane_capacity(
        lane_index_buffer: &mut TrackedBuffer,
        lane_index_capacity: &mut usize,
        device: &wgpu::Device,
        track_count: usize,
    ) {
        let needed = track_count.next_power_of_two().max(INITIAL_LANE_CAPACITY);
        if needed > *lane_index_capacity {
            *lane_index_capacity = needed;
            *lane_index_buffer = TrackedBuffer::new(
                device,
                &wgpu::BufferDescriptor {
                    label: Some("arrangement_lane_index_buffer"),
                    size: (needed * std::mem::size_of::<f32>()) as wgpu::BufferAddress,
                    usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                },
            );
        }
    }

    /// 确保可见索引缓冲容量足够（cull 输出 u32 源索引个数 = 全局实例数上限）
    pub(super) fn ensure_visible_capacity(
        visible_buffer: &mut TrackedBuffer,
        device: &wgpu::Device,
        required: u64,
    ) {
        let current = visible_buffer.size();
        if required <= current {
            return;
        }
        // 增长至需求量的下一个 2 的幂（限制余量，避免单次翻倍过大）
        let mut new_size = current.max(1).next_power_of_two().max(required);
        let extra = new_size - required;
        const MAX_EXTRA: u64 = 4 * 1024 * 1024; // 16MB 余量（4M 个 u32 索引）
        if extra > MAX_EXTRA {
            new_size = required + MAX_EXTRA;
        }
        *visible_buffer = TrackedBuffer::new(
            device,
            &wgpu::BufferDescriptor {
                label: Some("arrangement_note_visible_buffer"),
                size: new_size,
                usage: wgpu::BufferUsages::STORAGE
                    | wgpu::BufferUsages::VERTEX
                    | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            },
        );
    }
}
