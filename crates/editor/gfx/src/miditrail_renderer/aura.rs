//! Miditrail Aura 光环渲染
//!
//! 负责在按下的琴键下方生成发光光环，并管理光环相关的纹理、管线、缓冲区和渲染调用。
//!
//! Top 视图下 Aura 四边形与视线垂直（零面积）天然不可见，
//! 因此 Top 分支直接跳过 Aura 实例构建与绘制（CPU + GPU 双省）。

use super::{AURA_TEXTURE_SIZE, MiditrailAuraInstanceGpu, MiditrailRenderer};
use crate::gpu_resource_tracker::{TrackedBuffer, TrackedTexture};

impl MiditrailRenderer {
    /// 确保 Aura 实例缓冲区足够大。
    pub(super) fn ensure_aura_instance_buffer(&mut self, device: &wgpu::Device, count: usize) {
        if count <= self.aura_instance_capacity {
            return;
        }
        let new_cap = count
            .next_power_of_two()
            .max(Self::INITIAL_INSTANCE_CAPACITY);
        let size = (new_cap * std::mem::size_of::<MiditrailAuraInstanceGpu>()) as u64;
        // 旧缓冲由 Option::take 触发 Drop 自动注销
        let buffer = crate::gpu_resource_tracker::TrackedBuffer::new(
            device,
            &wgpu::BufferDescriptor {
                label: Some("miditrail_aura_instance_buffer"),
                size,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            },
        );
        self.aura_instance_buffer = Some(buffer);
        self.aura_instance_capacity = new_cap;
    }

    /// 确保 Aura 纹理和视图已创建。
    pub(super) fn ensure_aura_resources(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        if self.aura_resources_ready {
            return;
        }

        let texture = create_aura_texture(device, queue, AURA_TEXTURE_SIZE, &self.aura_image_data);
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        self.aura_texture_view = Some(view);
        self.aura_texture = Some(texture);
        self.aura_resources_ready = true;

        // 纹理改变后 bind group 需要重建
        self.bind_group = None;
    }

    /// 在同一个 render pass 中绘制 Aura 实例。
    pub(super) fn draw_aura(
        &self,
        render_pass: &mut wgpu::RenderPass<'_>,
        aura_instances: &[MiditrailAuraInstanceGpu],
    ) {
        if aura_instances.is_empty() {
            return;
        }
        // 不变式：draw_aura 在 render() 中 rebuild_bind_group 之后调用
        let Some(bind_group) = self.bind_group.as_ref() else {
            debug_assert!(false, "bind_group 应已初始化（rebuild_bind_group 已执行）");
            return;
        };
        let Some(aura_instance_buf) = self.aura_instance_buffer.as_ref() else {
            debug_assert!(
                false,
                "aura_instance_buffer 应已初始化（render 前 ensure_aura_instance_buffer 已调用）"
            );
            return;
        };

        render_pass.set_pipeline(&self.aura_pipeline);
        render_pass.set_bind_group(0, bind_group, &[]);
        render_pass.set_vertex_buffer(0, self.aura_vertex_buffer.inner().slice(..));
        render_pass.set_vertex_buffer(1, aura_instance_buf.inner().slice(..));
        render_pass.set_index_buffer(
            self.aura_index_buffer.inner().slice(..),
            wgpu::IndexFormat::Uint16,
        );
        render_pass.draw_indexed(0..6, 0, 0..aura_instances.len() as u32);
    }
}

/// 创建 Aura 四边形顶点/索引缓冲。
pub(super) fn create_aura_buffers(device: &wgpu::Device) -> (TrackedBuffer, TrackedBuffer) {
    const AURA_VERTICES: [f32; 16] = [
        -1.0, -1.0, 0.0, 0.0, 1.0, -1.0, 1.0, 0.0, 1.0, 1.0, 1.0, 1.0, -1.0, 1.0, 0.0, 1.0,
    ];
    const AURA_INDICES: [u16; 6] = [0, 1, 2, 0, 2, 3];

    let vertex_buffer = TrackedBuffer::new_init(
        device,
        &wgpu::util::BufferInitDescriptor {
            label: Some("miditrail_aura_vertex_buffer"),
            contents: bytemuck::cast_slice(&AURA_VERTICES),
            usage: wgpu::BufferUsages::VERTEX,
        },
    );

    let index_buffer = TrackedBuffer::new_init(
        device,
        &wgpu::util::BufferInitDescriptor {
            label: Some("miditrail_aura_index_buffer"),
            contents: bytemuck::cast_slice(&AURA_INDICES),
            usage: wgpu::BufferUsages::INDEX,
        },
    );

    (vertex_buffer, index_buffer)
}

/// 创建 Aura 纹理采样器。
pub(super) fn create_aura_sampler(device: &wgpu::Device) -> wgpu::Sampler {
    device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("miditrail_aura_sampler"),
        address_mode_u: wgpu::AddressMode::ClampToEdge,
        address_mode_v: wgpu::AddressMode::ClampToEdge,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        ..Default::default()
    })
}

/// 创建 Aura 环形纹理并上传初始数据。
pub(super) fn create_aura_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    size: u32,
    data: &[u8],
) -> TrackedTexture {
    let texture = TrackedTexture::new(
        device,
        &wgpu::TextureDescriptor {
            label: Some("miditrail_aura_texture"),
            size: wgpu::Extent3d {
                width: size,
                height: size,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        },
    );
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: texture.inner(),
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        data,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(size * 4),
            rows_per_image: Some(size),
        },
        wgpu::Extent3d {
            width: size,
            height: size,
            depth_or_array_layers: 1,
        },
    );
    texture
}

/// 生成一个软环形 Aura 纹理数据（RGBA8，size x size）。
pub(super) fn generate_aura_ring_data(size: u32) -> Vec<u8> {
    let mut data = vec![0u8; (size * size * 4) as usize];
    let center = (size - 1) as f32 * 0.5;
    let radius = size as f32 * 0.5;
    let inner = radius * 0.35;
    let outer = radius * 0.85;

    for y in 0..size {
        for x in 0..size {
            let dx = x as f32 - center;
            let dy = y as f32 - center;
            let dist = (dx * dx + dy * dy).sqrt();
            let alpha = if dist < inner || dist > outer {
                0.0
            } else {
                let mid = (inner + outer) * 0.5;
                let half = (outer - inner) * 0.5;
                let t = 1.0 - ((dist - mid) / half).abs();
                t * t * (3.0 - 2.0 * t)
            };
            let idx = ((y * size + x) * 4) as usize;
            data[idx] = 255;
            data[idx + 1] = 255;
            data[idx + 2] = 255;
            data[idx + 3] = (alpha * 255.0) as u8;
        }
    }
    data
}
