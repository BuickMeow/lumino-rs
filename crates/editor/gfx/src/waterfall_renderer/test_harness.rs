//! 瀑布流测试共享脚手架（设备/uniform/上传/回读/绑定布局）。
//!
//! `tests`（cull 像素等价）与 `active_tests`（活跃键内核）共用，避免两份重复。

use super::{WaterfallRenderer, WaterfallUniformGpu};
use crate::NoteInstance;
use futures::executor::block_on;

pub const TEST_W: u32 = 256;
pub const TEST_H: u32 = 144;

pub fn test_device() -> (wgpu::Device, wgpu::Queue) {
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
    let adapter = block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
        .expect("测试需要可用的 wgpu 适配器");
    block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("waterfall_equiv_device"),
        required_features: adapter.features() & wgpu::Features::default(),
        required_limits: wgpu::Limits::default(),
        memory_hints: wgpu::MemoryHints::default(),
        trace: wgpu::Trace::Off,
        experimental_features: wgpu::ExperimentalFeatures::disabled(),
    }))
    .expect("请求 wgpu 设备失败")
}

pub fn test_uniform() -> WaterfallUniformGpu {
    WaterfallUniformGpu {
        tick: 5000,
        ppq: 480,
        key_count: 128,
        frame_width: TEST_W,
        frame_height: TEST_H,
        kb_height: 17,
        speed: 1.0,
        _padding: 0,
    }
}

/// cull 测试窗口（与生产同公式；`speed=1.0, ppq=480` 下 span=7680）。
pub fn test_window() -> (u32, u32) {
    let tick = 5000u32;
    let tick_end = tick.saturating_add(super::waterfall_viewport_span(480, 1.0));
    (tick, tick_end)
}

pub fn upload_storage(
    device: &wgpu::Device,
    label: &'static str,
    notes: &[NoteInstance],
) -> wgpu::Buffer {
    use wgpu::util::DeviceExt;
    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some(label),
        contents: bytemuck::cast_slice(notes),
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
    })
}

pub fn upload_storage_u32(
    device: &wgpu::Device,
    label: &'static str,
    words: &[u32],
) -> wgpu::Buffer {
    use wgpu::util::DeviceExt;
    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some(label),
        contents: bytemuck::cast_slice(words),
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
    })
}

fn buffer_entry_for_test(binding: u32, read_only: Option<bool>) -> wgpu::BindGroupLayoutEntry {
    // None = uniform；Some(ro) = storage。
    let ty = match read_only {
        None => wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        Some(ro) => wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only: ro },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
    };
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty,
        count: None,
    }
}

pub fn uniform_entry_for_test(binding: u32) -> wgpu::BindGroupLayoutEntry {
    buffer_entry_for_test(binding, None)
}

pub fn storage_ro_entry_for_test(binding: u32) -> wgpu::BindGroupLayoutEntry {
    buffer_entry_for_test(binding, Some(true))
}

pub fn storage_rw_entry_for_test(binding: u32) -> wgpu::BindGroupLayoutEntry {
    buffer_entry_for_test(binding, Some(false))
}

/// u32 向量同步回读（测试用；与 global_bucket 侧同模式）。
pub fn readback_u32_vec(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    src: &wgpu::Buffer,
    count: usize,
) -> Vec<u32> {
    let size = (count * 4) as u64;
    let staging = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("waterfall_equiv_staging_u32"),
        size,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("waterfall_equiv_readback_u32"),
    });
    enc.copy_buffer_to_buffer(src, 0, &staging, 0, size);
    queue.submit(Some(enc.finish()));
    let (tx, rx) = std::sync::mpsc::channel();
    staging
        .slice(..)
        .map_async(wgpu::MapMode::Read, move |result| {
            let _ = tx.send(result);
        });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        if let Ok(map_result) = rx.try_recv() {
            map_result.expect("测试回读 map 失败");
            let data = staging.slice(..).get_mapped_range();
            let out: Vec<u32> = bytemuck::cast_slice(&data).to_vec();
            drop(data);
            staging.unmap();
            return out;
        }
        assert!(std::time::Instant::now() < deadline, "测试回读超时（10s）");
        let _ = device.poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: None,
        });
    }
}

/// 输出纹理同步回读（256×144 RGBA8，行距 1024 天然对齐，无 padding）。
pub fn readback_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
) -> Vec<u8> {
    let bytes_per_row = TEST_W * 4;
    let size = (bytes_per_row * TEST_H) as u64;
    let staging = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("waterfall_equiv_staging"),
        size,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("waterfall_equiv_readback"),
    });
    enc.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &staging,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(bytes_per_row),
                rows_per_image: Some(TEST_H),
            },
        },
        wgpu::Extent3d {
            width: TEST_W,
            height: TEST_H,
            depth_or_array_layers: 1,
        },
    );
    queue.submit(Some(enc.finish()));
    let (tx, rx) = std::sync::mpsc::channel();
    staging
        .slice(..)
        .map_async(wgpu::MapMode::Read, move |result| {
            let _ = tx.send(result);
        });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        if let Ok(map_result) = rx.try_recv() {
            map_result.expect("等价测试回读 map 失败");
            let data = staging.slice(..).get_mapped_range();
            let out = data.to_vec();
            drop(data);
            staging.unmap();
            return out;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "等价测试回读超时（10s）"
        );
        let _ = device.poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: None,
        });
    }
}

/// legacy 渲染器构造（测试用；`WaterfallRenderer::new` 公开构造）。
pub fn new_renderer(device: &wgpu::Device) -> WaterfallRenderer {
    WaterfallRenderer::new(device)
}
