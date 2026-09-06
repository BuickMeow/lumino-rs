use super::chunk::MAX_CHUNKS;
use super::types::{CameraUniform, CullUniform};
use crate::note_renderer::NoteRenderer;

/// u32 溢出防御：实例数超过 u32::MAX 时截断并报错
fn clamp_count(count: usize) -> u32 {
    if count > u32::MAX as usize {
        tracing::error!(
            "NoteRenderer: instance count {} exceeds u32::MAX, culling truncated",
            count
        );
        u32::MAX
    } else {
        count as u32
    }
}

impl NoteRenderer {
    /// 从外部 slice 上传音符实例到 GPU（不含 compute pass）
    ///
    /// 用于分离渲染线程从双缓冲读取数据后直接上传，
    /// 相比 `prepare_notes` 不包含 compute cull（由后续的 `prepare_pass` 完成）
    pub fn upload_instances(
        &mut self,
        instances: &[crate::NoteInstance],
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) {
        self.external_bound = false;
        self.gpu_note_buffer.upload_all(instances);
        self.update_cull_info(device, queue);
    }

    /// 共享缓冲上传（瀑布流视频导出专用）：只做 `upload_all`，跳过 cull 管线状态更新。
    ///
    /// 瀑布流 compute shader 直接绑定共享缓冲 + 自有分桶偏移表，钢琴卷帘的
    /// cull/render bind group 与可见索引缓冲全程不用——旧路径每帧重建两套
    /// bind group + 写 cull uniforms 是纯死工作（26 万实例下约毫秒级/帧）。
    /// 钢琴模式仍走 `upload_instances`/`prepare_notes`（cull compute 需要）。
    pub fn upload_shared_instances(&mut self, instances: &[crate::NoteInstance]) {
        self.external_bound = false;
        self.gpu_note_buffer.upload_all(instances);
    }

    /// 流式上传：开始一次流式上传会话（用于 6 亿音符场景控制 CPU 峰值）
    ///
    /// 调用方应在 UI 线程分块构建 NoteInstance，逐块调用 `streaming_append`，
    /// 最后调用 `finish_streaming_upload`。避免 `upload_instances` 的 CPU 全量副本。
    ///
    /// 详见 `GpuNoteBuffer::begin_streaming_upload`。
    pub fn begin_streaming_upload(&mut self) {
        self.gpu_note_buffer.begin_streaming_upload();
    }

    /// 流式上传：追加一块音符实例到 GPU buffer
    ///
    /// 每块大小建议 ≤ 10 万实例（1.6 MB）控制单次 write_buffer 内存峰值。
    pub fn streaming_append(&mut self, chunk: &[crate::NoteInstance]) {
        self.gpu_note_buffer.streaming_append(chunk);
    }

    /// 流式上传：结束会话并更新 cull info
    ///
    /// 必须在所有 `streaming_append` 完成后调用，以更新 bind group / uniform。
    pub fn finish_streaming_upload(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        self.gpu_note_buffer.finish_streaming_upload();
        self.update_cull_info(device, queue);
    }

    // ── 洋葱皮事件级增量专用（流式模式安全，无 CPU 缓存） ────────────────────
    //
    // 薄封装 GpuNoteBuffer 的流式安全操作，供渲染线程的 OnionSegment 段表增量使用。
    // 流式模式下 update_note/update_notes 会索引已清空的 CPU 缓存而 panic，
    // 因此段替换必须走以下纯 GPU 方法。

    /// 当前 GPU 实例数（段表偏移计算用）
    pub fn gpu_instance_count(&self) -> usize {
        self.gpu_note_buffer.len()
    }

    /// 当前 GPU 容量（实例数）
    pub fn gpu_capacity(&self) -> usize {
        self.gpu_note_buffer.capacity()
    }

    /// 扩容（grow 会重建 buffer 并 GPU 内部复制现有数据）
    pub fn grow_gpu(&mut self, required: usize) -> bool {
        self.gpu_note_buffer.grow(required)
    }

    /// 段内写（不更新计数，不触碰 CPU 缓存）
    pub fn write_segment(&mut self, offset: usize, instances: &[crate::NoteInstance]) {
        self.gpu_note_buffer.write_segment(offset, instances);
    }

    /// 设置实例计数（变长段替换后更新）
    pub fn set_gpu_instance_count(&mut self, count: usize) {
        self.gpu_note_buffer.set_instance_count(count);
    }

    /// GPU 内部搬移（staging 分块，支持重叠）
    pub fn move_gpu_range(&mut self, src: usize, dst: usize, count: usize) {
        self.gpu_note_buffer.move_range(src, dst, count);
    }

    /// 段内保序更新（等长，不改变计数）：主音轨段内增量（index = notes 索引）
    pub fn update_notes(&mut self, start_index: usize, instances: &[crate::NoteInstance]) {
        self.gpu_note_buffer.update_notes(start_index, instances);
    }

    /// 段内保序删除（后续段 GPU 内部左移 + 计数联动）：主音轨段内增量
    pub fn remove_at(&mut self, index: usize, count: usize) {
        self.gpu_note_buffer.remove_at(index, count);
    }

    /// 段内保序插入（后续段 GPU 内部右移 + 计数联动）：主音轨段内增量
    pub fn insert_at(&mut self, index: usize, instances: &[crate::NoteInstance]) {
        self.gpu_note_buffer.insert_at(index, instances);
    }

    /// 上传音符实例并准备渲染（推荐的替代方案，替代 `prepare_old`）
    pub fn prepare_notes(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        instances: &[crate::NoteInstance],
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        camera: CameraUniform,
    ) {
        puffin::profile_function!();
        self.external_bound = false;
        self.gpu_note_buffer.upload_all(instances);
        self.update_cull_info(device, queue);
        self.prepare_pass(encoder, camera, queue);
    }

    /// 兼容方法：数据+camera一步准备好（内部仍拆分成两步）
    /// 请改用 `prepare_notes`。
    #[deprecated(since = "0.1.0", note = "请改用 prepare_notes()")]
    pub fn prepare_old(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        instances: &[crate::NoteInstance],
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        camera: CameraUniform,
    ) {
        self.prepare_notes(encoder, instances, device, queue, camera);
    }

    /// 仅在音符数据真正变化时调用：负责更新 uniform
    ///
    /// 优化：bind group 仅在容量变化时重建；数据量变化只重写
    /// cull uniform 的 chunk 条目（chunk 绑定切片基于 buffer 容量，不变）。
    pub fn update_cull_info(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        puffin::profile_function!();
        let current_count = self.gpu_note_buffer.len();
        let source = self.gpu_note_buffer.buffer().clone();
        self.update_cull_info_for(device, queue, &source, current_count);
    }

    /// 绑定外部权威音符缓冲（视频导出直绑主缓冲，零上传）。
    ///
    /// 与 `upload_instances` 等价，唯数据源为外部 buffer 句柄：可见索引缓冲按需扩容，
    /// cull/render bind group 重建指向外部缓冲，`last_upload_count` 同步为外部计数。
    /// 视口 uniforms 由后续 `prepare_pass` 按帧更新；外部缓冲的内容更新
    /// （同句柄原地写）自动生效，无需重绑。
    pub fn bind_external_source(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        source: &wgpu::Buffer,
        count: usize,
    ) {
        puffin::profile_function!();
        self.update_cull_info_for(device, queue, source, count);
        // 空源不闩定：允许后续帧重试直绑（空文档/流式未完成场景零成本）。
        self.external_bound = count > 0;
    }

    /// 当前是否已直绑外部缓冲（调用方据此在直绑与上传回退间互斥）。
    pub fn is_external_bound(&self) -> bool {
        self.external_bound
    }

    /// `update_cull_info` 内核：按给定源缓冲与计数重建 cull 相关状态。
    fn update_cull_info_for(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        source: &wgpu::Buffer,
        current_count: usize,
    ) {
        puffin::profile_function!();

        if current_count == 0 {
            self.last_upload_count = 0;
            return;
        }

        // 扩容可见索引 buffer（如需）
        if current_count > self.capacity {
            self.grow_visible_buffer(device, current_count);
        }

        // 重新创建 cull / render bind groups：
        // source buffer 句柄可能因 GpuNoteBuffer::grow 而改变，必须同步到 bind group。
        self.cull_bind_groups = Self::create_cull_bind_groups(
            device,
            &self.chunk_layout,
            source,
            self.visible_instance_buffer.inner(),
            self.indirect_buffer.inner(),
            self.cull_uniform_buffer.inner(),
            self.viewport_buffer.inner(),
            self.cull_uniform_buffer_size,
            &self.cull_bind_group_layout,
        );
        self.render_bind_groups = Self::create_render_bind_groups(
            device,
            &self.render_bind_group_layout,
            self.viewport_buffer.inner(),
            self.view_state_buffer.inner(),
            source,
            self.visible_instance_buffer.inner(),
            &self.chunk_layout,
        );

        // 每 chunk 写入 uniform 条目（chunk_start / chunk_count）
        self.write_cull_uniforms(queue, current_count);

        self.last_upload_count = clamp_count(current_count);
    }

    /// 写入每 chunk 的 cull uniform 条目到槽位 buffer
    fn write_cull_uniforms(&self, queue: &wgpu::Queue, current_count: usize) {
        let chunk_count = self.chunk_layout.chunk_count(current_count).min(MAX_CHUNKS);
        for idx in 0..chunk_count {
            let (chunk_start, chunk_len) = self.chunk_layout.chunk_range(current_count, idx);
            let uniform = CullUniform {
                instance_count: clamp_count(current_count),
                chunk_start: clamp_count(chunk_start),
                chunk_count: clamp_count(chunk_len),
                _padding: 0,
            };
            let slot_offset = self.chunk_layout.chunk_offset_bytes(idx);
            queue.write_buffer(
                self.cull_uniform_buffer.inner(),
                slot_offset,
                bytemuck::cast_slice(&[uniform]),
            );
        }
    }

    /// 滚动/缩放等视口变化时调用：只更新 camera 并重跑 compute cull
    pub fn prepare_pass(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        camera: CameraUniform,
        queue: &wgpu::Queue,
    ) {
        self.prepare_pass_with_pipeline(encoder, camera, queue, false);
    }

    /// 纵向卷帘准备：复用同缓冲，转置坐标的裁剪（瀑布流风格纵向流动）
    pub fn prepare_vertical_pass(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        camera: CameraUniform,
        queue: &wgpu::Queue,
    ) {
        self.prepare_pass_with_pipeline(encoder, camera, queue, true);
    }

    fn prepare_pass_with_pipeline(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        camera: CameraUniform,
        queue: &wgpu::Queue,
        is_vertical: bool,
    ) {
        puffin::profile_function!();
        // 重置 indirect buffer（预计算模板，每帧零分配）
        queue.write_buffer(self.indirect_buffer.inner(), 0, &self.indirect_template);

        if self.last_upload_count == 0 {
            return;
        }
        queue.write_buffer(
            self.viewport_buffer.inner(),
            0,
            bytemuck::cast_slice(&[camera]),
        );

        let count = self.last_upload_count as usize;
        let chunk_count = self.chunk_layout.chunk_count(count).min(MAX_CHUNKS);
        let label = if is_vertical {
            "note_cull_vertical_pass"
        } else {
            "note_cull_pass"
        };
        let mut compute_pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some(label),
            timestamp_writes: None,
        });
        let pipeline = if is_vertical {
            &self.vertical_cull_pipeline
        } else {
            &self.cull_pipeline
        };
        compute_pass.set_pipeline(pipeline);

        const WORKGROUP_SIZE: u32 = 256;
        const MAX_DISPATCH_X: u32 = 65535;
        for idx in 0..chunk_count {
            compute_pass.set_bind_group(0, &self.cull_bind_groups[idx], &[]);
            let (_, chunk_len) = self.chunk_layout.chunk_range(count, idx);
            let workgroup_count = clamp_count(chunk_len).div_ceil(WORKGROUP_SIZE);
            let dispatch_x = workgroup_count.min(MAX_DISPATCH_X);
            let dispatch_y = workgroup_count.div_ceil(MAX_DISPATCH_X);
            compute_pass.dispatch_workgroups(dispatch_x, dispatch_y, 1);
        }
        drop(compute_pass);
    }
}
