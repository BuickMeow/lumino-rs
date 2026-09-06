//! 每帧渲染（上传 uniform/派生索引 + dispatch compute shader）
//!
//! 音符数据零上传：直接绑定调用方权威 `NoteInstance` 常驻缓冲。

use super::{WaterfallRenderer, WaterfallUniformGpu};
use crate::{CullWindow, KEY_BUCKETS, prefix_counts};

/// cull 渲染产物（调用方打点/回退判定用）。
pub enum CullRenderOutcome {
    /// cull 成功并已渲染（`visible` = 窗口音符数）。
    Culled { visible: usize },
    /// cull 不可用（常驻为空/构建失败），调用方走 legacy 回退。
    FallbackNeeded,
}

impl WaterfallRenderer {
    /// 常驻上传后调用：cull 世代递增，下次渲染重建桶（一次性成本）。
    pub fn mark_resident_updated(&mut self) {
        self.resident_cull.mark_resident_updated();
    }

    /// 释放导出 cull 全套 GPU 资源（完成/取消后由 `FinishVideoExport` 调用）。
    ///
    /// 只放自有部分（桶/sort_index/compact + cull 暂存）：权威常驻是调用方
    /// 共享缓冲（onion，UI 实时也在用），此处绝不碰。下次提取见桶缺失即
    /// 按当前世代重建，冷启动与首启一致。
    pub fn release_export_resources(&mut self) {
        self.resident_cull.release();
    }

    /// cull 窗口渲染：COUNT → 前缀和 → FILL → 活跃键内核 → legacy 精确渲染。
    ///
    /// 常驻由调用方持有（导出共享缓冲，一次上传）；窗口提取零回读（仅 1KB 计数），
    /// 输出 compact 与 UI 窗口同序，legacy shader 回溯预算语义不变（像素等价 harness 保证）。
    /// cull 任一步失败返回 `FallbackNeeded`（调用方用所带音符走 legacy 上传路径）。
    #[allow(clippy::too_many_arguments)]
    pub fn render_culled(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        params: &WaterfallUniformGpu,
        resident: &wgpu::Buffer,
        resident_count: usize,
        window: CullWindow,
    ) -> CullRenderOutcome {
        let key_count = window.key_count.min(KEY_BUCKETS);
        if key_count == 0 || resident_count == 0 {
            return CullRenderOutcome::FallbackNeeded;
        }
        let extract =
            match self
                .resident_cull
                .extract_count(device, queue, resident, resident_count, window)
            {
                Ok(e) => e,
                Err(e) => {
                    tracing::error!("瀑布流 cull 计数失败，回退 legacy 路径: {e}");
                    return CullRenderOutcome::FallbackNeeded;
                }
            };
        let (offsets, bases, total) = prefix_counts(&extract.counts, key_count);
        if let Err(e) = self.resident_cull.extract_fill(
            device,
            queue,
            encoder,
            resident,
            resident_count,
            window,
            total,
            &bases,
        ) {
            tracing::error!("瀑布流 cull 填充失败，回退 legacy 路径: {e}");
            return CullRenderOutcome::FallbackNeeded;
        }
        let (Some(compact), Some(key_offsets_buf), Some(sort_index_buf)) = (
            self.resident_cull.compact_buffer().cloned(),
            self.resident_cull.bucket_key_offsets().cloned(),
            self.resident_cull.bucket_sort_index().cloned(),
        ) else {
            tracing::error!("瀑布流 cull 产物缺失，回退 legacy 路径");
            return CullRenderOutcome::FallbackNeeded;
        };
        // 活跃键内核先行（同 encoder 有序，键色对主 pass 可见）；失败则 CPU 零键色。
        if self.run_active_kernel(
            device,
            queue,
            encoder,
            window.tick_start,
            resident,
            &key_offsets_buf,
            &sort_index_buf,
        ) {
            self.render_with_kernel_colors(
                device, queue, encoder, params, &compact, total, &offsets,
            );
        } else {
            self.render(
                device,
                queue,
                encoder,
                params,
                &compact,
                total,
                &offsets,
                &[0u32; 128],
            );
        }
        CullRenderOutcome::Culled { visible: total }
    }
}

impl WaterfallRenderer {
    /// 渲染瀑布流帧。
    ///
    /// # 参数
    /// - `device` — wgpu 设备
    /// - `queue` — wgpu 队列
    /// - `encoder` — 命令编码器（compute pass 将追加到此 encoder）
    /// - `params` — 瀑布流 uniform 参数
    /// - `note_buffer` — 权威 `NoteInstance` 常驻缓冲（调用方所有，`binding(1)` 只读绑定；
    ///   内容须按 (key, start_tick) 升序排列，与 `key_offsets` 一致）
    /// - `key_offsets` — 分桶偏移表（len = key_count + 1），桶 k 区间为
    ///   `[key_offsets[k], key_offsets[k+1])`。动态分桶：支持任意 key 数量。
    ///   为空时回退为单桶（全部音符），shader 仍可工作。
    /// - `active_key_colors` — 活跃键颜色数组（128 个 u32，packed RGBA `0xRRGGBBAA`，0 表示无高亮）
    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        params: &WaterfallUniformGpu,
        note_buffer: &wgpu::Buffer,
        note_count: usize,
        key_offsets: &[u32],
        active_key_colors: &[u32; 128],
    ) {
        self.render_inner(
            device,
            queue,
            encoder,
            params,
            note_buffer,
            note_count,
            key_offsets,
            Some(active_key_colors),
        );
    }

    /// 内核键色变体：活跃键色由 GPU 内核写入自有缓冲（`run_active_kernel` 先行），
    /// 此处直接绑定，省一次 CPU→GPU 上传与 CPU 循环。
    #[allow(clippy::too_many_arguments)]
    pub fn render_with_kernel_colors(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        params: &WaterfallUniformGpu,
        note_buffer: &wgpu::Buffer,
        note_count: usize,
        key_offsets: &[u32],
    ) {
        // 不变式：调用方须先 `run_active_kernel`（同 encoder 先行），否则键色为旧帧残留。
        self.ensure_active_key_colors_buffer(device);
        self.render_inner(
            device,
            queue,
            encoder,
            params,
            note_buffer,
            note_count,
            key_offsets,
            None,
        );
    }

    /// 渲染内核：`colors` 为 `Some` 时上传 CPU 键色，`None` 时直绑已有 GPU 键色。
    #[allow(clippy::too_many_arguments)]
    fn render_inner(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        params: &WaterfallUniformGpu,
        note_buffer: &wgpu::Buffer,
        note_count: usize,
        key_offsets: &[u32],
        active_key_colors: Option<&[u32; 128]>,
    ) {
        let width = params.frame_width;
        let height = params.frame_height;
        if width == 0 || height == 0 {
            return;
        }

        // 确保自有资源已创建（音符缓冲归调用方，此处不创建）
        self.ensure_output_texture(device, width, height);
        self.ensure_active_key_colors_buffer(device);
        self.ensure_key_offsets_buffer(device, key_offsets.len().saturating_sub(1));

        // 共享缓冲句柄可能因调用方扩容而变化，每次重建绑定（仅离屏导出路径使用）。
        self.bind_group = None;
        self.rebuild_bind_group(device, note_buffer);

        // 上传 uniform
        queue.write_buffer(
            self.uniform_buffer.inner(),
            0,
            bytemuck::cast_slice(&[*params]),
        );

        // 上传分桶偏移表（空时回退单桶：全部音符归入 key 0 桶。
        // 注意必须上传完整 key_count+1 长度，shader 会访问 key_offsets[key_count]）
        if let Some(ref buf) = self.key_offsets_buffer {
            if key_offsets.is_empty() {
                // 单桶回退：key 0 桶 = [0, len]，其余 key 桶为空
                let mut offsets = vec![note_count as u32; params.key_count as usize + 1];
                offsets[0] = 0;
                queue.write_buffer(buf.inner(), 0, bytemuck::cast_slice(&offsets));
            } else {
                queue.write_buffer(buf.inner(), 0, bytemuck::cast_slice(key_offsets));
            }
        }

        // 上传活跃键颜色（CPU 传入时上传；内核变体直绑已有 GPU 缓冲）。
        if let (Some(buf), Some(colors)) =
            (self.active_key_colors_buffer.as_ref(), active_key_colors)
        {
            queue.write_buffer(buf.inner(), 0, bytemuck::cast_slice(colors));
        }

        // 计算 dispatch 参数
        let workgroup_size: u32 = 16;
        let dispatch_x = width.div_ceil(workgroup_size);
        let dispatch_y = height.div_ceil(workgroup_size);

        // dispatch compute shader
        {
            let mut compute_pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("waterfall_compute_pass"),
                timestamp_writes: None,
            });
            compute_pass.set_pipeline(&self.compute_pipeline);
            if let Some(ref bg) = self.bind_group {
                compute_pass.set_bind_group(0, bg, &[]);
            }
            compute_pass.dispatch_workgroups(dispatch_x, dispatch_y, 1);
        }
    }

    /// 获取输出纹理的引用（用于 export pipeline 读回）。
    pub fn output_texture(&self) -> Option<&wgpu::Texture> {
        self.output_texture.as_ref().map(|t| t.inner())
    }
}
