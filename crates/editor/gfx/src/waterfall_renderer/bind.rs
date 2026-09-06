//! Bind group 重建

use super::WaterfallRenderer;

impl WaterfallRenderer {
    /// 重建 bind group（当共享音符缓冲句柄变化或自有资源变化时）。
    ///
    /// `note_buffer` 为调用方传入的权威 `NoteInstance` 常驻缓冲（只读绑定，
    /// 不获取所有权）。共享缓冲扩容会更换句柄，因此每次 render 都重建——
    /// 本渲染器仅用于离屏导出，该开销可忽略（钢琴卷帘导出同样每传必建）。
    pub(crate) fn rebuild_bind_group(&mut self, device: &wgpu::Device, note_buffer: &wgpu::Buffer) {
        // 不变式：rebuild 仅在 render() 中 ensure_* 之后调用，各自有资源已初始化
        let Some(key_colors_buf) = self.active_key_colors_buffer.as_ref() else {
            debug_assert!(
                false,
                "active_key_colors_buffer 未初始化（render 前 ensure_active_key_colors_buffer 已调用）"
            );
            return;
        };
        let Some(out_view) = self.output_texture_view.as_ref() else {
            debug_assert!(
                false,
                "output_texture_view 未初始化（render 前 ensure_output_texture 已调用）"
            );
            return;
        };
        let Some(key_offsets_buf) = self.key_offsets_buffer.as_ref() else {
            debug_assert!(
                false,
                "key_offsets_buffer 未初始化（render 前 ensure_key_offsets_buffer 已调用）"
            );
            return;
        };

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("waterfall_bind_group"),
            layout: &self.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.uniform_buffer.inner().as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: note_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: key_colors_buf.inner().as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(out_view),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: key_offsets_buf.inner().as_entire_binding(),
                },
            ],
        });
        self.bind_group = Some(bind_group);
    }
}
