//! Miditrail 离屏纹理管理
//!
//! Normal 与 Top 视图共用同一套颜色/深度纹理（切换视图不重建，
//! 不丢状态；尺寸变化时才释放重建）。

use super::MiditrailRenderer;

impl MiditrailRenderer {
    pub(super) fn ensure_output_texture(&mut self, device: &wgpu::Device, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        if self.current_width == width
            && self.current_height == height
            && self.output_texture.is_some()
            && self.depth_texture.is_some()
        {
            return;
        }

        self.release_textures();

        let color_texture = crate::gpu_resource_tracker::TrackedTexture::new(
            device,
            &wgpu::TextureDescriptor {
                label: Some("miditrail_output_texture"),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            },
        );
        self.output_texture_view =
            Some(color_texture.create_view(&wgpu::TextureViewDescriptor::default()));
        self.output_texture = Some(color_texture);

        let depth_texture = crate::gpu_resource_tracker::TrackedTexture::new(
            device,
            &wgpu::TextureDescriptor {
                label: Some("miditrail_depth_texture"),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Depth32Float,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            },
        );
        self.depth_texture_view =
            Some(depth_texture.create_view(&wgpu::TextureViewDescriptor::default()));
        self.depth_texture = Some(depth_texture);

        self.current_width = width;
        self.current_height = height;
        self.bind_group = None;
    }

    /// 释放纹理资源（由 [`TrackedTexture`] Drop 自动注销内存计数）
    fn release_textures(&mut self) {
        self.output_texture.take();
        self.output_texture_view.take();
        self.depth_texture.take();
        self.depth_texture_view.take();
    }
}
