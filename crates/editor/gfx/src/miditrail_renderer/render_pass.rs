//! Miditrail 渲染通道执行
//!
//! 负责把音符、Aura 与琴键实例绘制到同一个离屏 render pass 中。
//! 绘制顺序：音符（不写深度）→ Aura（附加混合，不写深度）→ 琴键（写深度）。
//! 参考 Comet MIDITrail：音符先绘制、琴键后绘制，确保琴键始终在最顶层。
//! Top 视图复用同一顺序，仅切换 flat 管线并跳过 Aura（俯视下零面积不可见）。

use super::{MiditrailAuraInstanceGpu, MiditrailInstanceGpu, MiditrailRenderer};

impl MiditrailRenderer {
    /// 执行完整的 Miditrail 渲染通道。
    pub(super) fn execute_render_pass(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        note_instances: &[MiditrailInstanceGpu],
        key_instances: &[MiditrailInstanceGpu],
        aura_instances: &[MiditrailAuraInstanceGpu],
        is_top: bool,
    ) {
        // 不变式：execute_render_pass 仅在 render() 中 ensure_* 之后调用
        let Some(color_view) = self.output_texture_view.as_ref() else {
            debug_assert!(
                false,
                "output_texture_view 应已初始化（render 前 ensure_output_texture 已调用）"
            );
            return;
        };
        let Some(depth_view) = self.depth_texture_view.as_ref() else {
            debug_assert!(
                false,
                "depth_texture_view 应已初始化（render 前 ensure_depth_texture 已调用）"
            );
            return;
        };
        let Some(bind_group) = self.bind_group.as_ref() else {
            debug_assert!(false, "bind_group 应已初始化（rebuild_bind_group 已执行）");
            return;
        };
        let Some(instance_buf) = self.instance_buffer.as_ref() else {
            debug_assert!(
                false,
                "instance_buffer 应已初始化（render 前 ensure_instance_buffer 已调用）"
            );
            return;
        };

        let note_count = note_instances.len() as u32;
        let key_count = key_instances.len() as u32;
        let key_offset = note_count;

        {
            let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("miditrail_render_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: color_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.0,
                            g: 0.0,
                            b: 0.0,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });

            // 1. 绘制音符（不写深度缓冲，参考 Comet MIDITrail 的 Painter's algorithm，
            //    琴键后绘制并覆盖音符）。Top 用 flat 管线（省逐片元光照）。
            //    平面模式（`3D音符` 开关关闭 = 默认）：音符只画顶面（y=1 的 X-Z 面），
            //    即只压 Y 高度、X 宽/Z 长保留；Normal/Top 双视图共用同一面
            //    （Normal 下盒子可见像素本就几乎全来自顶面）；
            //    实例/顺序/管线/shader 全不动，琴键与 Aura 仍走立方体缓冲。
            if is_top {
                render_pass.set_pipeline(&self.top_note_pipeline);
            } else {
                render_pass.set_pipeline(&self.note_pipeline);
            }
            render_pass.set_bind_group(0, bind_group, &[]);
            render_pass.set_vertex_buffer(0, self.vertex_buffer.inner().slice(..));
            render_pass.set_vertex_buffer(1, instance_buf.inner().slice(..));
            if self.flat_notes {
                render_pass.set_index_buffer(
                    self.quad_index_buffer.inner().slice(..),
                    wgpu::IndexFormat::Uint16,
                );
                render_pass.draw_indexed(Self::QUAD_RANGE, 0, 0..note_count);
            } else {
                render_pass.set_index_buffer(
                    self.index_buffer.inner().slice(..),
                    wgpu::IndexFormat::Uint16,
                );
                render_pass.draw_indexed(0..Self::CUBE_INDICES.len() as u32, 0, 0..note_count);
            }

            // 2. 绘制 Aura（附加混合，不写入深度）。Top 在俯视下零面积不可见，跳过。
            if !is_top {
                self.draw_aura(&mut render_pass, aura_instances);
            }

            // 3. 绘制琴键，覆盖音符与光环。Top 用 flat 管线。
            if is_top {
                render_pass.set_pipeline(&self.top_render_pipeline);
            } else {
                render_pass.set_pipeline(&self.render_pipeline);
            }
            render_pass.set_bind_group(0, bind_group, &[]);
            render_pass.set_vertex_buffer(0, self.vertex_buffer.inner().slice(..));
            render_pass.set_vertex_buffer(1, instance_buf.inner().slice(..));
            render_pass.set_index_buffer(
                self.index_buffer.inner().slice(..),
                wgpu::IndexFormat::Uint16,
            );
            render_pass.draw_indexed(
                0..Self::CUBE_INDICES.len() as u32,
                0,
                key_offset..key_offset + key_count,
            );
        }
    }
}
