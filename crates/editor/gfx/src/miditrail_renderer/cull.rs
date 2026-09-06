//! Miditrail 导出 cull：首帧全量常驻 + 每帧 GPU 窗口提取 + 回读 legacy 渲染。
//!
//! 背景：24M 级文档下 UI 每帧 collect/sort/pack（~20ms）是导出主瓶颈；全量常驻
//! 曾因“390MB 显存＋390MB 镜像＋每帧两次全扫 100ms”被否决。本路径只保留必要
//! 部分：全量 GPU 常驻一次上传（310MB，与钢琴模式首帧全量同量级，无 CPU 镜像），
//! 每帧 cull 内核按桶分区提取有序窗口（`bucket_cull.wgsl`，与 UI 同谓词），
//! 回读 compact（V×16B）后走未经修改的 legacy `render_from_instances`——
//! 像素与现状逐位一致（集合等价 harness 保证），视觉 veto 不触发。
//!
//! 两次提交：COUNT 自有提交（含 1KB 回读，`ResidentCull` 内部）；FILL + compact
//! 回读自有提交（legacy 渲染需 CPU 切片，读回后新 encoder 渲染）。回读量 V×16B
//!（36 万可见约 6MB），相对省掉的 UI 排序可忽略，打点量化。

use super::MiditrailRenderer;
use crate::gpu_resource_tracker::TrackedBuffer;
use crate::readback_bytes_sync;
use crate::{CullWindow, KEY_BUCKETS, NoteInstance, prefix_counts};

/// cull 窗口分段耗时（随 `cull_window` 返回；打点拆分用，work 黑盒不再盲人摸象）。
#[derive(Debug, Default, Clone, Copy)]
pub struct CullTiming {
    /// COUNT 内核 + 提交 + 1KB 回读同步。
    pub count_us: u64,
    /// FILL + compact 回读提交 + V×16B 按需映射拷贝。
    pub fill_readback_us: u64,
}

impl MiditrailRenderer {
    /// 首帧全量播种：常驻缓冲一次上传 + cull 世代递增（桶下次提取时构建）。    ///
    /// 调用方（导出 handler）在 `params.note_instances` 非空时调用；后续空帧跳过。
    pub fn seed_resident(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        notes: &[NoteInstance],
    ) {
        let need = notes.len().max(1);
        let cap = self.resident_capacity;
        if self.resident_buffer.is_none() || need > cap {
            let new_cap = (need.saturating_mul(6) / 5).max(need + 1024);
            let size = (new_cap * 16) as u64;
            self.resident_buffer = Some(TrackedBuffer::new(
                device,
                &wgpu::BufferDescriptor {
                    label: Some("miditrail_export_resident"),
                    size,
                    usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                },
            ));
            self.resident_capacity = new_cap;
        }
        if let Some(ref buf) = self.resident_buffer {
            queue.write_buffer(buf.inner(), 0, bytemuck::cast_slice(notes));
        }
        self.resident_count = notes.len();
        self.resident_cull.mark_resident_updated();
    }

    /// cull 窗口提取并回读（返回 CPU 切片的所有权 + 分段耗时，调用方渲染后
    /// `restore_window` 归还，跨帧复用零分配；take/restore 为指针移动，无拷贝）。
    ///
    /// 失败（空常驻/构建失败/回读失败）返回 `Err`，调用方回退所带音符。
    pub fn cull_window(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        window: CullWindow,
    ) -> Result<(Vec<NoteInstance>, CullTiming), crate::GlobalBucketError> {
        let key_count = window.key_count.min(KEY_BUCKETS);
        let resident = self
            .resident_buffer
            .as_ref()
            .ok_or(crate::GlobalBucketError::CullResource("miditrail 常驻缓冲"))?;
        let resident_inner = resident.inner().clone();
        let resident_count = self.resident_count;
        let t_count = std::time::Instant::now();
        let extract = self.resident_cull.extract_count(
            device,
            queue,
            &resident_inner,
            resident_count,
            window,
        )?;
        let count_us = t_count.elapsed().as_micros() as u64;
        let (_offsets, bases, total) = prefix_counts(&extract.counts, key_count);
        if total == 0 {
            return Ok((std::mem::take(&mut self.cull_cpu), CullTiming::default()));
        }
        // FILL + compact 回读：自有 encoder + 提交（legacy 需 CPU 切片，此处同步等待）。
        let t_fill = std::time::Instant::now();
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("miditrail_cull_fill_readback"),
        });
        self.resident_cull.extract_fill(
            device,
            queue,
            &mut encoder,
            &resident_inner,
            resident_count,
            window,
            total,
            &bases,
        )?;
        let compact = self
            .resident_cull
            .compact_buffer()
            .ok_or(crate::GlobalBucketError::CullResource("miditrail 紧凑缓冲"))?
            .clone();
        let need_bytes = total * 16;
        self.ensure_cull_staging(device, need_bytes);
        let staging = self
            .cull_staging
            .as_ref()
            .ok_or(crate::GlobalBucketError::CullResource("miditrail 回读暂存"))?;
        encoder.copy_buffer_to_buffer(&compact, 0, staging.inner(), 0, need_bytes as u64);
        queue.submit(Some(encoder.finish()));
        let bytes = readback_bytes_sync(
            device,
            self.cull_staging
                .as_ref()
                .ok_or(crate::GlobalBucketError::CullResource("miditrail 回读暂存"))?
                .inner(),
            need_bytes,
        )?;
        let fill_readback_us = t_fill.elapsed().as_micros() as u64;
        // 按需映射已保证长度（`need_bytes` 内对齐截断）；尾部零填充不是音符。
        let bytes = &bytes[..need_bytes.min(bytes.len())];
        let count = bytes.len() / 16;
        let mut out = std::mem::take(&mut self.cull_cpu);
        out.clear();
        out.reserve(count);
        out.extend(
            bytemuck::cast_slice::<u8, NoteInstance>(bytes)
                .iter()
                .copied(),
        );
        let timing = CullTiming {
            count_us,
            fill_readback_us,
        };
        Ok((out, timing))
    }

    /// 归还 cull 窗口缓冲（`cull_window` 返回值的去向；跨帧复用）。
    pub fn restore_window(&mut self, window: Vec<NoteInstance>) {
        self.cull_cpu = window;
    }

    /// 释放导出全套 GPU/CPU 常驻（完成/取消后由 `FinishVideoExport` 调用）。
    ///
    /// 释放：全量常驻缓冲（`seed_resident` 一次上传，24M 文档约 370MB）、
    /// cull 提取器全套（桶/sort_index/compact，约 300MB）、回读暂存、
    /// 实例缓冲 + Aura 实例缓冲（历史峰值，`next_power_of_two` 只增不减）、
    /// 导出专用 CPU 切片（`cull_cpu`/`scratch_derived`）。
    /// 保留：管线/布局/纹理/采样器（编译产物或小常量，下次导出复用）与
    /// 通用构建暂存（`scratch_notes/keys/auras`，`clear+reserve` 语义，
    /// 下次导出首帧即复用，无需重分配）。
    /// 下次 `seed_resident` 按需重建 + 世代递增 → 桶重建，冷启动与首启一致。
    pub fn release_export_resources(&mut self) {
        self.resident_buffer = None;
        self.resident_capacity = 0;
        self.resident_count = 0;
        self.resident_cull.release();
        self.cull_staging = None;
        self.cull_cpu = Vec::new();
        self.scratch_derived = Vec::new();
        self.instance_buffer = None;
        self.instance_capacity = 0;
        self.aura_instance_buffer = None;
        self.aura_instance_capacity = 0;
    }

    /// 确保回读暂存容量（按需扩容 + 滞后收缩；`MAP_READ | COPY_DST`）。
    ///
    /// 收缩是按需映射的搭档：只增不减会让密集段后的稀疏帧常驻数十 MB
    ///（24M 文档实测密集段后稀疏帧被钉死 ~75ms）。`need < cap/4` 时缩到
    /// 1.2×need；staging 是纯 scratch（每帧全量覆写），重建近乎免费，
    /// 滞后带宽避免在边界反复抖动。
    fn ensure_cull_staging(&mut self, device: &wgpu::Device, need_bytes: usize) {
        let cap = self
            .cull_staging
            .as_ref()
            .map(|b| b.inner().size() as usize)
            .unwrap_or(0);
        if cap >= need_bytes && (need_bytes == 0 || cap < need_bytes.saturating_mul(4)) {
            return;
        }
        let size = cull_staging_size(need_bytes) as u64;
        self.cull_staging = Some(TrackedBuffer::new(
            device,
            &wgpu::BufferDescriptor {
                label: Some("miditrail_cull_staging"),
                size: size.max(16),
                usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            },
        ));
    }
}

/// 回读暂存容量：1.2×need 保底 +4KB，按 16（NoteInstance 步长）对齐
///（map_async range 对齐要求；调用方 `total * 16` 天然对齐）。
fn cull_staging_size(need_bytes: usize) -> usize {
    (need_bytes.saturating_mul(6) / 5)
        .max(need_bytes + 4096)
        .div_ceil(16)
        * 16
}
