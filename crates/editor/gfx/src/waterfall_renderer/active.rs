//! 活跃键 GPU 内核：常驻全量 → 每键覆盖音符颜色，无回读。
//!
//! 生产窗口序 last-writer（`handle_waterfall_frame` 旧 CPU 循环）在全集上有序
//! 等价于“桶内自上而下回溯首个覆盖者”（窗口含全部覆盖音符，见
//! `tests::test_active_kernel_matches_cpu_loop`）：内核每 key 一线程，二分上界 +
//! 回溯首个 `end > tick` 者，颜色复刻 `unpack/pack` 截断逐位一致。
//! 主渲染 pass 之前同 encoder 内执行，键色对主 pass 可见，零 CPU/零回读。
//!
//! 数据源为调用方常驻（cull 同源）+ cull 全局桶；绑定组每帧重建（legacy
//! `render()` 同策略，创建成本相对 dispatch 可忽略，省句柄失效跟踪）。

use super::{TrackedBuffer, WaterfallRenderer};

/// 活跃键内核参数（16 字节 uniform，仅 tick 有效）。
#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct ActiveParamsGpu {
    tick: u32,
    _pad: [u32; 3],
}

impl WaterfallRenderer {
    const ACTIVE_SHADER: &'static str = include_str!("../shaders/waterfall_active.wgsl");

    /// 运行活跃键内核（键色写入自有 128×u32 缓冲，主 pass 经同一缓冲只读）。
    ///
    /// # 参数
    /// - `resident` — 常驻全量缓冲（cull 同源）；
    /// - `key_offsets/sort_index` — cull 全局桶句柄（调用方由 `ResidentCull` 取出，
    ///   Buffer 为廉价克隆，避免跨方法借用冲突）。
    ///
    /// 返回 `false` 表示前置条件不足，调用方应回退 CPU 循环。
    /// 参数偏多是 GPU 调度打包（绑定一次免每帧分配），与 `render()` 同策略。
    #[allow(clippy::too_many_arguments)]
    pub fn run_active_kernel(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        tick: u32,
        resident: &wgpu::Buffer,
        key_offsets: &wgpu::Buffer,
        sort_index: &wgpu::Buffer,
    ) -> bool {
        self.ensure_active_resources(device);
        self.ensure_active_key_colors_buffer(device);
        let (Some(layout), Some(params_buf), Some(colors)) = (
            self.active_layout.as_ref(),
            self.active_params_buffer.as_ref(),
            self.active_key_colors_buffer.as_ref(),
        ) else {
            debug_assert!(false, "活跃键管线资源应已创建");
            return false;
        };
        queue.write_buffer(
            params_buf.inner(),
            0,
            bytemuck::cast_slice(&[ActiveParamsGpu { tick, _pad: [0; 3] }]),
        );
        let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("waterfall_active_bind_group"),
            layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: params_buf.inner().as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: resident.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: key_offsets.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: sort_index.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: colors.inner().as_entire_binding(),
                },
            ],
        });
        let pipeline = match self.active_pipeline.as_ref() {
            Some(p) => p,
            None => {
                debug_assert!(false, "活跃键管线应已创建");
                return false;
            }
        };
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("waterfall_active_compute_pass"),
                timestamp_writes: None,
            });
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, &bg, &[]);
            pass.dispatch_workgroups(1, 1, 1);
        }
        true
    }

    /// 确保活跃键管线/布局/参数缓冲已创建（once）。
    fn ensure_active_resources(&mut self, device: &wgpu::Device) {
        if self.active_pipeline.is_some() {
            return;
        }
        let shader = crate::shader::create_shader_module(
            device,
            "waterfall_active_shader",
            Self::ACTIVE_SHADER,
        );
        // binding 0 参数 uniform；1 音符只读；2 桶边界只读；3 置换索引只读；
        // 4 键色读写（内核输出，主 pass 只读——分属不同 pass 先后执行，无冲突）。
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("waterfall_active_bind_group_layout"),
            entries: &[
                uniform_entry(0),
                storage_ro_entry(1),
                storage_ro_entry(2),
                storage_ro_entry(3),
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
            ],
        });
        let pipeline = crate::pipeline::ComputePipelineBuilder::new(
            device,
            "waterfall_active_compute_pipeline",
            &shader,
        )
        .bind_group(&layout)
        .build();
        let params_buffer = TrackedBuffer::new(
            device,
            &wgpu::BufferDescriptor {
                label: Some("waterfall_active_params_buffer"),
                size: std::mem::size_of::<ActiveParamsGpu>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            },
        );
        self.active_layout = Some(layout);
        self.active_pipeline = Some(pipeline);
        self.active_params_buffer = Some(params_buffer);
    }
}

fn uniform_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

fn storage_ro_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only: true },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}
