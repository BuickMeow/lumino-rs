//! cull 两阶段提取（COUNT 自有提交 + FILL 追加调用方 encoder）。
//!
//! COUNT 与 FILL 同构（同一 shader，同谓词），中间经调用方前缀和衔接；
//! 两阶段划分保证输出 key 连续（legacy 桶内二分前提），无原子竞争。

use super::super::support::readback_u256;
use super::super::{BucketSource, GlobalBucketError, KEY_BUCKETS, ResidentCull};
use super::{CullExtract, CullParamsGpu, CullWindow, missing};

impl ResidentCull {
    /// 提取计数（COUNT：自有 encoder + 提交，回读 1KB 后返回）。
    ///
    /// 回读须在提交后 poll——调用方 encoder 提交时机不定，故 COUNT 自有提交；
    /// FILL 追加到调用方 encoder（与后续渲染同序，一次提交）。
    pub fn extract_count(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        resident: &wgpu::Buffer,
        count: usize,
        window: CullWindow,
    ) -> Result<CullExtract, GlobalBucketError> {
        let source = BucketSource {
            buffer: resident,
            count,
            epoch: self.seq,
        };
        let key_count = window.key_count.min(KEY_BUCKETS);
        let mut out = CullExtract {
            counts: [0u32; KEY_BUCKETS],
            total: 0,
            bucket_rebuilt: false,
        };
        if source.count == 0 || key_count == 0 {
            return Ok(out);
        }
        let total_u32 = u32::try_from(source.count)
            .map_err(|_| GlobalBucketError::CountOverflow(source.count))?;
        self.ensure_bucket(device, queue, &source)?;
        out.bucket_rebuilt = self.bucket_rebuilt_flag;
        self.bucket_rebuilt_flag = false;
        self.ensure_cull_resources(device, queue);
        // 游标护栏：tick 倒退（非单调调用）则清零重扫，否则游标位置相对
        // 新 tick 仍有效（死亡单调）。等值 tick 无需处理（非严格单调成立）。
        // 注意 `ensure_bucket` 重建时已清零，此处只处理"桶未变但 tick 回退"。
        if window.tick_start < self.last_tick_start {
            self.zero_cursors(queue);
        }
        self.last_tick_start = window.tick_start;

        let params = CullParamsGpu {
            tick_start: window.tick_start,
            tick_end: window.tick_end,
            key_count: key_count as u32,
            phase: 0,
            total_count: total_u32,
            _pad: [0; 3],
        };
        let params_buf = self
            .params_buffer
            .as_ref()
            .ok_or(missing("cull 参数缓冲"))?;
        queue.write_buffer(params_buf.inner(), 0, bytemuck::cast_slice(&[params]));
        self.rebuild_bind_group(device, source.buffer)?;
        {
            let mut count_enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("bucket_cull_count"),
            });
            {
                let mut pass = count_enc.begin_compute_pass(&wgpu::ComputePassDescriptor {
                    label: Some("bucket_cull_count_pass"),
                    timestamp_writes: None,
                });
                let pipeline = self.pipeline.as_ref().ok_or(missing("cull 管线"))?;
                pass.set_pipeline(pipeline);
                let bg = self.bind_group.as_ref().ok_or(missing("cull 绑定组"))?;
                pass.set_bind_group(0, bg, &[]);
                pass.dispatch_workgroups(key_count.div_ceil(64) as u32, 1, 1);
            }
            let counts_inner = self
                .counts_buffer
                .as_ref()
                .ok_or(missing("cull 计数缓冲"))?
                .inner();
            let staging_inner = self
                .counts_staging
                .as_ref()
                .ok_or(missing("cull 回读暂存"))?
                .inner();
            count_enc.copy_buffer_to_buffer(counts_inner, 0, staging_inner, 0, 1024);
            queue.submit(Some(count_enc.finish()));
        }
        let mut counts = readback_u256(
            device,
            self.counts_staging
                .as_ref()
                .ok_or(missing("cull 回读暂存"))?
                .inner(),
        )?;
        // 尾部清零：COUNT 内核只写 `[0, key_count)`，旧帧残留不得泄漏。
        for c in counts.iter_mut().skip(key_count) {
            *c = 0;
        }
        let total = counts
            .iter()
            .take(key_count)
            .fold(0usize, |acc, &c| acc.saturating_add(c as usize));
        out.counts = counts;
        out.total = total;
        Ok(out)
    }

    /// FILL：按调用方前缀和基址写 compact（追加到调用方 encoder，`submit` 后可见）。
    ///
    /// 调用方须保证 `total`/`bases` 与 `extract_count` 返回同源；compact 按 `total`
    /// 内部扩容，句柄变化时重建绑定组，调用方无须关心。
    /// 参数偏多是 GPU 调度打包（一次 dispatch 免每帧分配），与 `render()` 同策略。
    #[allow(clippy::too_many_arguments)]
    pub fn extract_fill(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        resident: &wgpu::Buffer,
        count: usize,
        window: CullWindow,
        total: usize,
        bases: &[u32; KEY_BUCKETS],
    ) -> Result<(), GlobalBucketError> {
        let source = BucketSource {
            buffer: resident,
            count,
            epoch: self.seq,
        };
        let key_count = window.key_count.min(KEY_BUCKETS);
        if total == 0 || key_count == 0 {
            return Ok(());
        }
        let total_u32 = u32::try_from(source.count)
            .map_err(|_| GlobalBucketError::CountOverflow(source.count))?;
        self.ensure_compact(device, total);
        self.rebuild_bind_group(device, source.buffer)?;
        let base_buf = self.base_buffer.as_ref().ok_or(missing("cull 基址缓冲"))?;
        queue.write_buffer(base_buf.inner(), 0, bytemuck::cast_slice(bases));
        let params = CullParamsGpu {
            tick_start: window.tick_start,
            tick_end: window.tick_end,
            key_count: key_count as u32,
            phase: 1,
            total_count: total_u32,
            _pad: [0; 3],
        };
        let params_buf = self
            .params_buffer
            .as_ref()
            .ok_or(missing("cull 参数缓冲"))?;
        queue.write_buffer(params_buf.inner(), 0, bytemuck::cast_slice(&[params]));
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("bucket_cull_fill_pass"),
                timestamp_writes: None,
            });
            let pipeline = self.pipeline.as_ref().ok_or(missing("cull 管线"))?;
            pass.set_pipeline(pipeline);
            let bg = self.bind_group.as_ref().ok_or(missing("cull 绑定组"))?;
            pass.set_bind_group(0, bg, &[]);
            pass.dispatch_workgroups(key_count.div_ceil(64) as u32, 1, 1);
        }
        Ok(())
    }
}
