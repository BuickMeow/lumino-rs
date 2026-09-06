//! 渲染器创建与 GPU 资源懒初始化

use super::{TrackedBuffer, TrackedTexture, WaterfallRenderer, WaterfallUniformGpu};

impl WaterfallRenderer {
    const SHADER: &'static str = include_str!("../shaders/waterfall.wgsl");

    /// 创建瀑布流渲染器。
    pub fn new(device: &wgpu::Device) -> Self {
        let shader = crate::shader::create_shader_module(device, "waterfall_shader", Self::SHADER);

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("waterfall_bind_group_layout"),
            entries: &[
                // binding 0: uniform
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
                // binding 1: notes storage buffer (read-only)
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                // binding 2: active_key_colors storage buffer (read-only)
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
                // binding 3: output storage texture
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::StorageTexture {
                        access: wgpu::StorageTextureAccess::WriteOnly,
                        format: wgpu::TextureFormat::Rgba8Unorm,
                        view_dimension: wgpu::TextureViewDimension::D2,
                    },
                    count: None,
                },
                // binding 4: key_offsets storage buffer (read-only)
                // 音符分桶偏移表：`[offsets[key], offsets[key+1])` 为 key 桶区间，
                // 长度 = key_count + 1（动态分桶，支持任意 key 数量）
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });

        let compute_pipeline = crate::pipeline::ComputePipelineBuilder::new(
            device,
            "waterfall_compute_pipeline",
            &shader,
        )
        .bind_group(&bind_group_layout)
        .build();

        let uniform_buffer = TrackedBuffer::new(
            device,
            &wgpu::BufferDescriptor {
                label: Some("waterfall_uniform_buffer"),
                size: std::mem::size_of::<WaterfallUniformGpu>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            },
        );

        Self {
            compute_pipeline,
            bind_group_layout,
            bind_group: None,
            uniform_buffer,
            active_key_colors_buffer: None,
            key_offsets_buffer: None,
            output_texture: None,
            output_texture_view: None,
            key_offsets_capacity: 0,
            current_width: 0,
            current_height: 0,
            resident_cull: crate::ResidentCull::new(),
            active_pipeline: None,
            active_layout: None,
            active_params_buffer: None,
        }
    }

    /// 确保输出纹理已创建（尺寸变化时重建）。
    pub(crate) fn ensure_output_texture(&mut self, device: &wgpu::Device, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        if self.current_width == width
            && self.current_height == height
            && self.output_texture.is_some()
        {
            return;
        }
        // 释放旧纹理（Option::take 触发 Drop 自动注销）
        self.output_texture.take();
        self.output_texture_view.take();

        let texture = TrackedTexture::new(
            device,
            &wgpu::TextureDescriptor {
                label: Some("waterfall_output_texture"),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            },
        );

        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());

        self.output_texture = Some(texture);
        self.output_texture_view = Some(view);
        self.current_width = width;
        self.current_height = height;
        // 尺寸变化 → legacy 绑定组重建（活跃键内核组每帧重建，无须处理）
        self.bind_group = None;
    }

    /// 确保 active_key_colors buffer 存在（128 个 u32）。
    pub(crate) fn ensure_active_key_colors_buffer(&mut self, device: &wgpu::Device) {
        if self.active_key_colors_buffer.is_some() {
            return;
        }
        let buffer = TrackedBuffer::new(
            device,
            &wgpu::BufferDescriptor {
                label: Some("waterfall_active_key_colors_buffer"),
                size: (128 * 4) as u64, // 128 u32
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            },
        );
        self.active_key_colors_buffer = Some(buffer);
        self.bind_group = None;
    }

    /// 确保 key_offsets buffer 有足够容量（动态分桶：key_count + 1 个 u32）
    pub(crate) fn ensure_key_offsets_buffer(&mut self, device: &wgpu::Device, key_count: usize) {
        let needed = key_count + 1;
        if needed <= self.key_offsets_capacity {
            return;
        }
        let new_cap = needed.next_power_of_two().max(65); // 至少 64 键 + 1 哨兵
        let size = (new_cap * std::mem::size_of::<u32>()) as u64;
        // 旧缓冲由 Option::take 触发 Drop 自动注销
        let buffer = TrackedBuffer::new(
            device,
            &wgpu::BufferDescriptor {
                label: Some("waterfall_key_offsets_buffer"),
                size,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            },
        );
        self.key_offsets_buffer = Some(buffer);
        self.key_offsets_capacity = new_cap;
        self.bind_group = None;
    }
}
