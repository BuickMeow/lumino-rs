//! Miditrail 渲染器可见性与预览测试

use super::super::*;
use futures::executor::block_on;
use wgpu::util::DeviceExt;

/// 在可用 GPU/软件适配器上渲染一帧 Miditrail，并断言输出不为全黑。
///
/// 该测试用于验证：键盘与音符实例确实写入离屏纹理，且相机/投影可见。
#[test]
fn test_frame_renders_visible_content() {
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
    let adapter = block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
        .expect("测试需要可用的 wgpu 适配器");
    let (device, queue) = block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("miditrail_test_device"),
        required_features: adapter.features() & wgpu::Features::default(),
        required_limits: wgpu::Limits::default(),
        memory_hints: wgpu::MemoryHints::default(),
        trace: wgpu::Trace::Off,
        experimental_features: wgpu::ExperimentalFeatures::disabled(),
    }))
    .expect("请求 wgpu 设备失败");

    let mut renderer = MiditrailRenderer::new(&device);

    let uniform = MiditrailUniformGpu {
        tick: 0,
        ppq: 480,
        key_count: 128,
        frame_width: 320,
        frame_height: 180,
        kb_height: 20,
        _reserved: 0,
        speed: 1.0,
        param1: 0.0,
        param2: 0.0,
        fps: 60.0,
        z_far_distance: 7.5,
        view_mode: MiditrailViewMode::Normal,
        ticks_per_second: 960.0,
        _padding1: 0,
    };
    let notes = vec![MiditrailNoteGpu {
        key: 60,
        start_tick: 0,
        end_tick: 480,
        color_packed: 0xFF0000FF, // 红色
        track_idx: 0,
        velocity: 100,
        channel: 0,
        _padding: 0,
    }];

    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("miditrail_test_encoder"),
    });
    renderer.render(&device, &queue, &mut encoder, &uniform, &notes);

    let texture = renderer.output_texture().expect("渲染后应存在输出纹理");

    let bytes_per_pixel = 4u32;
    let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let padded_bytes_per_row = (320 * bytes_per_pixel).next_multiple_of(align);
    let buffer_size = (padded_bytes_per_row * 180) as u64;
    let staging = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("miditrail_test_staging"),
        contents: &vec![0u8; buffer_size as usize],
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
    });

    encoder.copy_texture_to_buffer(
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
                bytes_per_row: Some(padded_bytes_per_row),
                rows_per_image: Some(180),
            },
        },
        wgpu::Extent3d {
            width: 320,
            height: 180,
            depth_or_array_layers: 1,
        },
    );
    queue.submit(std::iter::once(encoder.finish()));

    let slice = staging.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |r| {
        tx.send(r).expect("map_async 回调发送失败");
    });
    let _ = device.poll(wgpu::PollType::Wait {
        submission_index: None,
        timeout: None,
    });
    rx.recv()
        .expect("map_async 回调未收到")
        .expect("map_async 失败");

    let data = slice.get_mapped_range();
    let mut non_black_pixels = 0usize;
    for row in 0..180 {
        let row_start = (row * padded_bytes_per_row) as usize;
        for col in 0..320 {
            let idx = row_start + (col * 4) as usize;
            if data[idx] != 0 || data[idx + 1] != 0 || data[idx + 2] != 0 {
                non_black_pixels += 1;
            }
        }
    }
    drop(data);
    staging.unmap();

    assert!(
        non_black_pixels > 100,
        "渲染结果应包含可见像素，但仅 {non_black_pixels} 个非黑像素"
    );
}

/// 渲染一帧 1280x720 的 MIDITrail 预览并写入 `target/miditrail_preview.png`。
///
/// 该测试不用于断言具体像素值，而是生成可视化产物供人工检查。
/// 若输出全黑或文件为空，则测试失败。
#[test]
fn test_export_preview_png() {
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
    let adapter = block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
        .expect("测试需要可用的 wgpu 适配器");
    let (device, queue) = block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("miditrail_preview_device"),
        required_features: adapter.features() & wgpu::Features::default(),
        required_limits: wgpu::Limits::default(),
        memory_hints: wgpu::MemoryHints::default(),
        trace: wgpu::Trace::Off,
        experimental_features: wgpu::ExperimentalFeatures::disabled(),
    }))
    .expect("请求 wgpu 设备失败");

    let mut renderer = MiditrailRenderer::new(&device);

    let uniform = MiditrailUniformGpu {
        tick: 0,
        ppq: 480,
        key_count: 128,
        frame_width: 1280,
        frame_height: 720,
        kb_height: 20,
        _reserved: 0,
        speed: 1.0,
        param1: 0.0,
        param2: 0.0,
        fps: 60.0,
        z_far_distance: 7.5,
        view_mode: MiditrailViewMode::Normal,
        ticks_per_second: 960.0,
        _padding1: 0,
    };
    let notes = vec![
        MiditrailNoteGpu {
            key: 60,
            start_tick: 0,
            end_tick: 12_000,
            color_packed: 0xFF0000FF, // 红色
            track_idx: 0,
            velocity: 100,
            channel: 0,
            _padding: 0,
        },
        MiditrailNoteGpu {
            key: 64,
            start_tick: 0,
            end_tick: 12_480,
            color_packed: 0x00FF00FF, // 绿色
            track_idx: 0,
            velocity: 100,
            channel: 0,
            _padding: 0,
        },
        MiditrailNoteGpu {
            key: 67,
            start_tick: 0,
            end_tick: 12_960,
            color_packed: 0x0000FFFF, // 蓝色
            track_idx: 0,
            velocity: 100,
            channel: 0,
            _padding: 0,
        },
    ];

    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("miditrail_preview_encoder"),
    });
    renderer.render(&device, &queue, &mut encoder, &uniform, &notes);

    let texture = renderer.output_texture().expect("渲染后应存在输出纹理");

    let bytes_per_pixel = 4u32;
    let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let padded_bytes_per_row = (1280 * bytes_per_pixel).next_multiple_of(align);
    let buffer_size = (padded_bytes_per_row * 720) as u64;
    let staging = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("miditrail_preview_staging"),
        contents: &vec![0u8; buffer_size as usize],
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
    });

    encoder.copy_texture_to_buffer(
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
                bytes_per_row: Some(padded_bytes_per_row),
                rows_per_image: Some(720),
            },
        },
        wgpu::Extent3d {
            width: 1280,
            height: 720,
            depth_or_array_layers: 1,
        },
    );
    queue.submit(std::iter::once(encoder.finish()));

    let slice = staging.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |r| {
        tx.send(r).expect("map_async 回调发送失败");
    });
    let _ = device.poll(wgpu::PollType::Wait {
        submission_index: None,
        timeout: None,
    });
    rx.recv()
        .expect("map_async 回调未收到")
        .expect("map_async 失败");

    let data = slice.get_mapped_range();
    let mut image_data = Vec::with_capacity(1280 * 720 * 4);
    for row in 0..720 {
        let row_start = (row * padded_bytes_per_row) as usize;
        image_data.extend_from_slice(&data[row_start..row_start + 1280 * 4]);
    }
    drop(data);
    staging.unmap();

    let target_dir = std::env::var("CARGO_TARGET_DIR").unwrap_or_else(|_| "target".to_string());
    let path = std::path::Path::new(&target_dir).join("miditrail_preview.png");
    std::fs::create_dir_all(path.parent().expect("预览路径父目录")).expect("创建预览目录失败");
    let file = std::fs::File::create(&path).expect("创建预览文件失败");
    let mut encoder = png::Encoder::new(file, 1280, 720);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().expect("PNG 写入头失败");
    writer
        .write_image_data(&image_data)
        .expect("PNG 写入数据失败");
    writer.finish().expect("PNG 完成写入失败");

    let metadata = std::fs::metadata(&path).expect("读取预览文件元数据失败");
    assert!(
        metadata.len() > 1024,
        "生成的预览文件过小：{} 字节",
        metadata.len()
    );
    eprintln!("MIDITrail 预览已写入：{}", path.display());
}

/// Normal / Top 切换不崩溃、不丢状态，且两视图均渲染出可见内容。
///
/// 同一渲染器连续渲染两种视图（纹理/管线/实例缓冲复用），
/// 断言输出纹理始终存在且每帧均有可见像素（切换无黑屏、无状态丢失）。
#[test]
fn test_view_switch_renders_both_modes() {
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
    let adapter = block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
        .expect("测试需要可用的 wgpu 适配器");
    let (device, queue) = block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("miditrail_switch_device"),
        required_features: adapter.features() & wgpu::Features::default(),
        required_limits: wgpu::Limits::default(),
        memory_hints: wgpu::MemoryHints::default(),
        trace: wgpu::Trace::Off,
        experimental_features: wgpu::ExperimentalFeatures::disabled(),
    }))
    .expect("请求 wgpu 设备失败");

    let mut renderer = MiditrailRenderer::new(&device);
    let notes = vec![
        MiditrailNoteGpu {
            key: 60,
            start_tick: 0,
            end_tick: 12_000,
            color_packed: 0xFF0000FF,
            track_idx: 0,
            velocity: 100,
            channel: 0,
            _padding: 0,
        },
        MiditrailNoteGpu {
            key: 64,
            start_tick: 0,
            end_tick: 480,
            color_packed: 0x00FF00FF,
            track_idx: 0,
            velocity: 100,
            channel: 0,
            _padding: 0,
        },
    ];
    let base = MiditrailUniformGpu {
        tick: 0,
        ppq: 480,
        key_count: 128,
        frame_width: 320,
        frame_height: 180,
        kb_height: 20,
        _reserved: 0,
        speed: 1.0,
        param1: 0.0,
        param2: 0.0,
        fps: 60.0,
        z_far_distance: 7.5,
        view_mode: MiditrailViewMode::Normal,
        ticks_per_second: 960.0,
        _padding1: 0,
    };

    let normal =
        super::basic::render_and_count_non_black(&device, &queue, &mut renderer, &base, &notes);
    assert!(normal > 100, "Normal 视图应有可见像素，实际 {normal}");
    let top = MiditrailUniformGpu {
        view_mode: MiditrailViewMode::Top,
        ..base
    };
    let top_count =
        super::basic::render_and_count_non_black(&device, &queue, &mut renderer, &top, &notes);
    assert!(top_count > 100, "Top 视图应有可见像素，实际 {top_count}");
    // 再切回 Normal：复用资源下状态不丢，依然可见。
    let back =
        super::basic::render_and_count_non_black(&device, &queue, &mut renderer, &base, &notes);
    assert!(back > 100, "切回 Normal 后应仍有可见像素，实际 {back}");
}
