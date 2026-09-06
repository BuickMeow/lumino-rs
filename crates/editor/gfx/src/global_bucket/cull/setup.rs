//! cull 资源管理：桶重建判定、管线/暂存懒初始化、绑定组装配。
//!
//! 均为 `ResidentCull` 的固有方法（`pub(super)`，仅 cull 树内调用）：
//! 桶按 `(字节数, 数量, 世代)` 判定，任一变化即重建（一次性成本，首帧打点）；
//! 绑定组在桶/compact 句柄变化时失效重建，句柄稳定时跨帧复用。

use super::super::support::{new_storage_buffer, storage_entry};
use super::super::{BucketSource, GlobalBucketError, GlobalBucketIndex, KEY_BUCKETS};
use super::{CullParamsGpu, missing};
use crate::gpu_resource_tracker::TrackedBuffer;

use super::super::ResidentCull;

impl ResidentCull {
    /// 确保桶有效：`(字节数, 数量, 世代)` 任一变化即重建（一次性成本，首帧打点）。
    pub(super) fn ensure_bucket(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        source: &BucketSource<'_>,
    ) -> Result<(), GlobalBucketError> {
        let src_bytes = source.buffer.size();
        let stale = match self.bucket.as_ref() {
            Some(b) => {
                self.src_bytes != src_bytes
                    || self.src_count != source.count
                    || self.src_seq != source.epoch
                    || b.note_count() != source.count
            }
            None => true,
        };
        if !stale {
            return Ok(());
        }
        let t0 = std::time::Instant::now();
        let bucket = GlobalBucketIndex::build(device, queue, source.buffer, source.count)?;
        tracing::info!(
            notes = source.count,
            seq = source.epoch,
            build_ms = t0.elapsed().as_secs_f64() * 1000.0,
            "cull 全局桶构建完成（一次性）",
        );
        self.bucket = Some(bucket);
        self.src_bytes = src_bytes;
        self.src_count = source.count;
        self.src_seq = source.epoch;
        self.bind_group = None;
        self.bucket_rebuilt_flag = true;
        // 排序变化 → 游标位置失效，清零（缓冲若尚未创建由下方的
        // `ensure_cull_resources` 补建，零初值即有效游标）。
        self.zero_cursors(queue);
        Ok(())
    }

    /// 游标清零（桶重建 / tick 倒退后调用；缓冲缺失时跳过，创建即零初值）。
    pub(super) fn zero_cursors(&mut self, queue: &wgpu::Queue) {
        if let Some(ref cursor) = self.cursor_buffer {
            queue.write_buffer(cursor.inner(), 0, &[0u8; KEY_BUCKETS * 4]);
        }
        self.last_tick_start = 0;
    }

    /// 确保 cull 管线与暂存已创建（once；compact 除外，按需扩容见 `ensure_compact`）。
    ///
    /// 游标创建后显式清零：wgpu storage 缓冲零初始化不可依赖（见 gpu-synth
    /// 的防御性 clear），游标垃圾值会跳过活音符——正确性悬崖，必须写零。
    pub(super) fn ensure_cull_resources(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        if self.pipeline.is_some() {
            return;
        }
        let shader = crate::shader::create_shader_module(
            device,
            "bucket_cull_shader",
            include_str!("../../shaders/bucket_cull.wgsl"),
        );
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("bucket_cull_bind_group_layout"),
            entries: &[
                uniform_entry(0),
                storage_entry(1, true),
                storage_entry(2, true),
                storage_entry(3, true),
                storage_entry(4, false),
                storage_entry(5, false),
                storage_entry(6, true),
                // binding 7：单调游标（每 key 一 u32，读写；零初值即有效）。
                storage_entry(7, false),
            ],
        });
        let pipeline = crate::pipeline::ComputePipelineBuilder::new(
            device,
            "bucket_cull_compute_pipeline",
            &shader,
        )
        .bind_group(&layout)
        .build();
        self.layout = Some(layout);
        self.pipeline = Some(pipeline);
        // 参数缓冲：UNIFORM 用途（着色器 binding 0 为 uniform 绑定）。
        self.params_buffer = Some(TrackedBuffer::new(
            device,
            &wgpu::BufferDescriptor {
                label: Some("bucket_cull_params"),
                size: (std::mem::size_of::<CullParamsGpu>() as u64).max(16),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            },
        ));
        self.counts_buffer = Some(new_storage_buffer(device, "bucket_cull_counts", 1024));
        self.base_buffer = Some(new_storage_buffer(device, "bucket_cull_base", 1024));
        // 游标 1KB：创建后显式清零（零初值 = 全键从桶底起扫，有效游标）。
        self.cursor_buffer = Some(new_storage_buffer(
            device,
            "bucket_cull_cursor",
            (KEY_BUCKETS * 4) as u64,
        ));
        self.zero_cursors(queue);
        self.counts_staging = Some(TrackedBuffer::new(
            device,
            &wgpu::BufferDescriptor {
                label: Some("bucket_cull_counts_staging"),
                size: 1024,
                usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            },
        ));
        // compact 最小占位（COUNT 阶段绑定用；FILL 前按精确总数扩容）。
        self.compact_buffer = Some(new_storage_buffer(device, "bucket_cull_compact", 16));
        self.compact_capacity = 0;
        self.bind_group = None;
    }

    /// 确保 compact 容量 ≥ `total`（不足则按 1.2× 扩容，句柄变化，绑定组失效）。
    pub(super) fn ensure_compact(&mut self, device: &wgpu::Device, total: usize) {
        if total <= self.compact_capacity {
            return;
        }
        let new_cap = (total.saturating_mul(6) / 5).max(total + 1024);
        let size = (new_cap * 16) as u64;
        self.compact_buffer = Some(new_storage_buffer(device, "bucket_cull_compact", size));
        self.compact_capacity = new_cap;
        self.bind_group = None;
    }

    /// 重建 cull 绑定组（桶/compact 句柄变化后；句柄稳定时复用，省每帧重建）。
    pub(super) fn rebuild_bind_group(
        &mut self,
        device: &wgpu::Device,
        resident: &wgpu::Buffer,
    ) -> Result<(), GlobalBucketError> {
        if self.bind_group.is_some() {
            return Ok(());
        }
        let layout = self.layout.as_ref().ok_or(missing("cull 布局"))?;
        let bucket = self.bucket.as_ref().ok_or(missing("cull 全局桶"))?;
        let params = self
            .params_buffer
            .as_ref()
            .ok_or(missing("cull 参数缓冲"))?;
        let compact = self
            .compact_buffer
            .as_ref()
            .ok_or(missing("cull 紧凑缓冲"))?;
        let counts = self
            .counts_buffer
            .as_ref()
            .ok_or(missing("cull 计数缓冲"))?;
        let base = self.base_buffer.as_ref().ok_or(missing("cull 基址缓冲"))?;
        self.bind_group = Some(
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("bucket_cull_bind_group"),
                layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: params.inner().as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: resident.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: bucket.key_offsets_buffer().as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: bucket.sort_index_buffer().as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: compact.inner().as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 5,
                        resource: counts.inner().as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 6,
                        resource: base.inner().as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 7,
                        resource: self
                            .cursor_buffer
                            .as_ref()
                            .ok_or(missing("cull 游标缓冲"))?
                            .inner()
                            .as_entire_binding(),
                    },
                ],
            }),
        );
        Ok(())
    }
}

/// uniform 绑定布局项（cull 参数）。
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
