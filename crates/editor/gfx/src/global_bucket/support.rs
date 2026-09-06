//! 全局桶构建支撑：缓冲/绑定组装配与 1KB 同步回读。

use std::sync::mpsc;
use std::time::{Duration, Instant};

use super::{GlobalBucketError, HIST_BYTES, HIST_LEN};
use crate::gpu_resource_tracker::TrackedBuffer;

/// 创建 `STORAGE | COPY_DST | COPY_SRC` 通用缓冲（COPY_SRC 供单测回读验证）。
pub(crate) fn new_storage_buffer(
    device: &wgpu::Device,
    label: &'static str,
    size: u64,
) -> TrackedBuffer {
    TrackedBuffer::new(
        device,
        &wgpu::BufferDescriptor {
            label: Some(label),
            size: size.max(16),
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_DST
                | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        },
    )
}

/// storage 绑定布局项。
pub(crate) fn storage_entry(binding: u32, read_only: bool) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

/// 组装排序绑定组（binding 0~5：音符/输入索引/输出索引/tile/参数/key 直方图）。
#[allow(clippy::too_many_arguments)]
pub(crate) fn make_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    notes: &wgpu::Buffer,
    indices_in: &wgpu::Buffer,
    indices_out: &wgpu::Buffer,
    tile_buf: &wgpu::Buffer,
    params: &wgpu::Buffer,
    key_hist: &wgpu::Buffer,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("global_bucket_sort_bg"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: notes.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: indices_in.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: indices_out.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: tile_buf.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 4,
                resource: params.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 5,
                resource: key_hist.as_entire_binding(),
            },
        ],
    })
}

/// 变长同步回读（staging 由调用方按需扩容；cull compact V×16B 传输用）。
///
/// 只映射并拷贝前 `need_bytes` 字节——staging 经 1.2× 扩容只增不减（密集段可达
/// 数十 MB），全量 `slice(..)` 会让稀疏帧也付全缓冲 map + memcpy 税（实测 24M
/// 文档密集段后稀疏帧被钉死 ~75ms，recv≈work）。`need_bytes` 须为 MAP 对齐
/// 长度（调用方 `total * 16` 天然 16 对齐）；超长按缓冲实际长度截断。
/// 读回模式与 `readback_u256` 一致：`map_async` + `device.poll(Wait)` + 5s 兜底。
pub(crate) fn readback_bytes_sync(
    device: &wgpu::Device,
    staging: &wgpu::Buffer,
    need_bytes: usize,
) -> Result<Vec<u8>, GlobalBucketError> {
    let cap = staging.size() as usize;
    let mut len = need_bytes.min(cap);
    len -= len % wgpu::MAP_ALIGNMENT as usize;
    let (tx, rx) = mpsc::channel();
    staging
        .slice(..len as u64)
        .map_async(wgpu::MapMode::Read, move |result| {
            let _ = tx.send(result);
        });
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Ok(map_result) = rx.try_recv() {
            map_result.map_err(|e| GlobalBucketError::MapFailed(format!("{e:?}")))?;
            let data = staging.slice(..len as u64).get_mapped_range();
            let out = data.to_vec();
            drop(data);
            staging.unmap();
            return Ok(out);
        }
        if Instant::now() >= deadline {
            return Err(GlobalBucketError::MapTimeout);
        }
        let _ = device.poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: None,
        });
    }
}
///
/// 1KB 直方图同步回读（staging 由调用方复用；一次性构建成本）。
///
/// 读回模式沿用 `export_pipeline/staging.rs`：`MAP_READ | COPY_DST` staging +
/// `map_async` + `device.poll(Wait)` + 5s 兜底。
pub(crate) fn readback_u256(
    device: &wgpu::Device,
    staging: &wgpu::Buffer,
) -> Result<[u32; HIST_LEN], GlobalBucketError> {
    let (tx, rx) = mpsc::channel();
    staging
        .slice(..)
        .map_async(wgpu::MapMode::Read, move |result| {
            let _ = tx.send(result);
        });
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Ok(map_result) = rx.try_recv() {
            map_result.map_err(|e| GlobalBucketError::MapFailed(format!("{e:?}")))?;
            let data = staging.slice(..).get_mapped_range();
            if data.len() != HIST_BYTES as usize {
                return Err(GlobalBucketError::BadLength {
                    expected: HIST_BYTES as usize,
                    got: data.len(),
                });
            }
            let mut out = [0u32; HIST_LEN];
            out.copy_from_slice(bytemuck::cast_slice(&data));
            drop(data);
            staging.unmap();
            return Ok(out);
        }
        if Instant::now() >= deadline {
            return Err(GlobalBucketError::MapTimeout);
        }
        let _ = device.poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: None,
        });
    }
}
