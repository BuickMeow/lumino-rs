use super::types::CameraUniform;
use super::types::DrawIndirectArgs;
use crate::gpu_resource_tracker::TrackedBuffer;
use crate::note_renderer::NoteRenderer;

impl NoteRenderer {
    /// 创建新的音符渲染器（默认带 depth attachment）
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, format: wgpu::TextureFormat) -> Self {
        Self::new_with_depth(device, queue, format, true)
    }

    /// 创建不带 depth attachment 的音符渲染器（用于视频导出等纯 2D 路径）
    pub fn new_without_depth(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        format: wgpu::TextureFormat,
    ) -> Self {
        Self::new_with_depth(device, queue, format, false)
    }

    /// 创建洋葱皮音符渲染器
    ///
    /// 与主音轨 NoteRenderer 的差异：
    /// - 使用 `onion_note.wgsl`（不透明 alpha=1.0，2 像素同色加深描边）
    /// - 复用 cull.wgsl（GPU culling + indirect draw 零修改）
    ///
    /// depth 配置：与主音轨一致（needs_depth=true）
    /// 原因：encoder.rs 的 render_pass 带 depth_stencil_attachment，根据
    /// `constants::is_depth_stencil_compatible`，pipeline 必须携带匹配的
    /// depth-stencil 状态，否则 wgpu 验证层拒绝 draw call（洋葱皮不显示）。
    /// 洋葱皮先于主音轨绘制，depth=0.0 通过 LessEqual < 1.0（clear），
    /// 写入 depth=0.0 后主音轨 LessEqual 0.0<=0.0 通过并覆盖，视觉正确。
    ///
    /// 性能范式（照搬 wasabi 精神 + lumino 现有基础设施）：
    /// - 全量上传一次（MIDI 加载时，非每帧重写）—— 比 wasabi 每帧重写更优
    /// - GPU culling 每帧（复用 cull.wgsl 的 workgroup 批量原子 + LOD 剔除）
    /// - Indirect draw（CPU 零参与绘制提交）
    pub fn new_onion_skin(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        format: wgpu::TextureFormat,
    ) -> Self {
        Self::new_with_shader(
            device,
            queue,
            format,
            true,
            Self::ONION_SHADER,
            Self::CULL_SHADER,
        )
    }

    fn new_with_depth(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        format: wgpu::TextureFormat,
        needs_depth: bool,
    ) -> Self {
        Self::new_with_shader(
            device,
            queue,
            format,
            needs_depth,
            Self::VERTEX_SHADER,
            Self::CULL_SHADER,
        )
    }

    fn new_with_shader(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        format: wgpu::TextureFormat,
        needs_depth: bool,
        vertex_shader: &'static str,
        cull_shader: &'static str,
    ) -> Self {
        let shader = crate::shader::create_shader_module(device, "note_shader", vertex_shader);
        let cull_shader = crate::shader::create_shader_module(device, "cull_shader", cull_shader);

        // 纵向转置版管线（复用同缓冲，瀑布流风格的纵向流动）
        // 洋葱皮纵向需主轨特别显示（ViewState），与普通预览音符的 note_vertical 不同
        let is_onion = vertex_shader == Self::ONION_SHADER;
        let vertical_shader_source = if is_onion {
            Self::ONION_SHADER_VERTICAL
        } else {
            Self::VERTEX_SHADER_VERTICAL
        };
        let vertical_shader_label = if is_onion {
            "onion_note_vertical_shader"
        } else {
            "note_vertical_shader"
        };
        let vertical_shader = crate::shader::create_shader_module(
            device,
            vertical_shader_label,
            vertical_shader_source,
        );
        let vertical_cull_shader = crate::shader::create_shader_module(
            device,
            "cull_vertical_shader",
            Self::CULL_SHADER_VERTICAL,
        );

        // 创建渲染 bind group layout
        let render_bind_group_layout = Self::create_render_bind_group_layout(device);

        // 创建计算 bind group layout
        let cull_bind_group_layout = Self::create_cull_bind_group_layout(device);

        // 创建渲染管线
        let pipeline = Self::create_render_pipeline(
            device,
            &shader,
            &render_bind_group_layout,
            format,
            needs_depth,
        );
        let vertical_pipeline = Self::create_render_pipeline(
            device,
            &vertical_shader,
            &render_bind_group_layout,
            format,
            needs_depth,
        );

        // 创建计算管线
        let cull_pipeline =
            Self::create_cull_pipeline(device, &cull_shader, &cull_bind_group_layout);
        let vertical_cull_pipeline =
            Self::create_cull_pipeline(device, &vertical_cull_shader, &cull_bind_group_layout);

        // 创建缓冲区
        let max_capacity = (device.limits().max_storage_buffer_binding_size as usize)
            / std::mem::size_of::<crate::NoteInstance>();
        let max_capacity = max_capacity.min(
            (device.limits().max_buffer_size as usize) / std::mem::size_of::<crate::NoteInstance>(),
        );

        // storage binding 分块布局（2GB 上限规避，详见 chunk.rs）
        let chunk_layout = super::chunk::ChunkLayout::from_limits(&device.limits());
        let slot_align = chunk_layout.slot_align;

        // 间接绘制初始化模板：每槽位一个默认 DrawIndirectArgs，逐帧复用写回
        //（原每帧 `vec![0u8; 64 × slot_align]` 堆分配，约 16KB/帧）。
        let indirect_template = Self::build_indirect_template(slot_align);

        let (
            gpu_note_buffer,
            visible_instance_buffer,
            indirect_buffer,
            viewport_buffer,
            cull_uniform_buffer,
            cull_uniform_buffer_size,
        ) = Self::create_renderer_buffers(device, queue, slot_align);

        // 视图状态 uniform buffer（当前音轨 + 静音位图，切轨/静音零重传）
        let view_state_buffer = TrackedBuffer::new_init(
            device,
            &wgpu::util::BufferInitDescriptor {
                label: Some("note_view_state_uniform"),
                contents: bytemuck::cast_slice(&[crate::note_renderer::types::ViewState::new()]),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            },
        );

        // 创建渲染 bind groups（按 chunk 分块，避免大 source buffer 整体绑定超限）
        let render_bind_groups = Self::create_render_bind_groups(
            device,
            &render_bind_group_layout,
            viewport_buffer.inner(),
            view_state_buffer.inner(),
            gpu_note_buffer.buffer(),
            visible_instance_buffer.inner(),
            &chunk_layout,
        );

        // 创建计算 bind groups（初始无实例数据，绑定切片按 buffer 容量划分）
        let cull_bind_groups = Self::build_cull_bind_groups(
            device,
            &chunk_layout,
            &gpu_note_buffer,
            visible_instance_buffer.inner(),
            indirect_buffer.inner(),
            cull_uniform_buffer.inner(),
            &cull_uniform_buffer_size,
            viewport_buffer.inner(),
            &cull_bind_group_layout,
        );

        Self {
            pipeline,
            vertical_pipeline,
            cull_pipeline,
            vertical_cull_pipeline,
            gpu_note_buffer,
            visible_instance_buffer,
            indirect_buffer,
            indirect_template,
            external_bound: false,
            capacity: Self::INITIAL_CAPACITY,
            max_capacity,
            last_upload_count: 0,
            viewport_buffer,
            view_state_buffer,
            cull_uniform_buffer,
            cull_uniform_buffer_size,
            render_bind_groups,
            cull_bind_groups,
            render_bind_group_layout,
            cull_bind_group_layout,
            chunk_layout,
        }
    }

    /// 构建间接绘制初始化模板（MAX_CHUNKS 槽位 × 默认 DrawIndirectArgs）。
    ///
    /// `prepare_pass` 每帧将其写回 indirect buffer 做重置；预计算后每帧零分配。
    fn build_indirect_template(slot_align: u64) -> Vec<u8> {
        let slot_count = super::chunk::MAX_CHUNKS;
        let mut template = vec![0u8; (slot_count as u64 * slot_align) as usize];
        let default_args = DrawIndirectArgs::default();
        let default_bytes = bytemuck::bytes_of(&default_args);
        for idx in 0..slot_count {
            let offset = (idx as u64) * slot_align;
            template[offset as usize..offset as usize + default_bytes.len()]
                .copy_from_slice(default_bytes);
        }
        template
    }

    /// 创建所有 GPU 缓冲区（音符实例缓冲、可见索引缓冲、间接绘制缓冲、
    /// 视口/剔除 uniform 缓冲）。
    ///
    /// 2026-08-07：可见缓冲从完整 NoteInstance 改为 u32 源索引，
    /// 渲染时顶点着色器通过 storage binding 读取原数据。
    ///
    /// 间接缓冲与 cull uniform 缓冲按 `MAX_CHUNKS × slot_align` 固定槽位分配：
    /// 每 chunk 一个 `DrawIndirectArgs` / `CullUniform` 条目，chunk 数变化
    /// 无需重建 buffer（仅绑定 offset 不同）。
    fn create_renderer_buffers(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        slot_align: u64,
    ) -> (
        crate::gpu_note_buffer::GpuNoteBuffer,
        TrackedBuffer,
        TrackedBuffer,
        TrackedBuffer,
        TrackedBuffer,
        u64,
    ) {
        let gpu_note_buffer = crate::gpu_note_buffer::GpuNoteBuffer::new(device, queue);
        let visible_instance_buffer =
            Self::create_visible_index_buffer(device, Self::INITIAL_CAPACITY);

        let slot_count = super::chunk::MAX_CHUNKS as u64;
        let slot_bytes = slot_count * slot_align;
        let zeros = vec![0u8; slot_bytes as usize];
        let indirect_buffer = TrackedBuffer::new_init(
            device,
            &wgpu::util::BufferInitDescriptor {
                label: Some("note_indirect_buffer"),
                contents: &zeros,
                usage: wgpu::BufferUsages::INDIRECT
                    | wgpu::BufferUsages::STORAGE
                    | wgpu::BufferUsages::COPY_DST
                    | wgpu::BufferUsages::COPY_SRC,
            },
        );

        let viewport_buffer = TrackedBuffer::new_init(
            device,
            &wgpu::util::BufferInitDescriptor {
                label: Some("viewport_uniform"),
                contents: bytemuck::cast_slice(&[CameraUniform::default()]),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            },
        );

        let cull_uniform_buffer = TrackedBuffer::new_init(
            device,
            &wgpu::util::BufferInitDescriptor {
                label: Some("cull_uniform"),
                contents: &zeros,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            },
        );

        (
            gpu_note_buffer,
            visible_instance_buffer,
            indirect_buffer,
            viewport_buffer,
            cull_uniform_buffer,
            slot_bytes,
        )
    }

    /// 创建计算 bind groups（分块切片）。
    #[allow(clippy::too_many_arguments)]
    fn build_cull_bind_groups(
        device: &wgpu::Device,
        chunk_layout: &super::chunk::ChunkLayout,
        gpu_note_buffer: &crate::gpu_note_buffer::GpuNoteBuffer,
        visible_instance_buffer: &wgpu::Buffer,
        indirect_buffer: &wgpu::Buffer,
        cull_uniform_buffer: &wgpu::Buffer,
        cull_uniform_buffer_size: &u64,
        viewport_buffer: &wgpu::Buffer,
        cull_bind_group_layout: &wgpu::BindGroupLayout,
    ) -> Vec<wgpu::BindGroup> {
        Self::create_cull_bind_groups(
            device,
            chunk_layout,
            gpu_note_buffer.buffer(),
            visible_instance_buffer,
            indirect_buffer,
            cull_uniform_buffer,
            viewport_buffer,
            *cull_uniform_buffer_size,
            cull_bind_group_layout,
        )
    }
}
