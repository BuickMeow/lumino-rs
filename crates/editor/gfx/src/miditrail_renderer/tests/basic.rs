//! Miditrail 渲染器基础烟雾测试

use super::super::*;
use futures::executor::block_on;
use wgpu::util::DeviceExt;

#[test]
fn test_cube_constants() {
    assert_eq!(MiditrailRenderer::CUBE_VERTICES.len(), 144);
    assert_eq!(MiditrailRenderer::CUBE_INDICES.len(), 36);
}

/// 平面索引语义（盒子→平面的唯一改动点，纯 CPU 验证）：
/// - 仅顶面 6（y=1 的 X-Z 面）= 立方体索引 [0..6) 逐字复用；
/// - Y 即高度轴（scale[1]=NOTE_HEIGHT），平面只压 Y，X/Z 保留；
/// - 角点 y 全为 1、x/z 取遍 {0,1}（Z 长不丢失），索引不越界。
#[test]
fn test_quad_indices_reuse_cube_faces() {
    assert_eq!(MiditrailRenderer::QUAD_INDICES.len(), 6);
    assert_eq!(MiditrailRenderer::QUAD_RANGE, 0..6);
    let cube = MiditrailRenderer::CUBE_INDICES;
    let quad = MiditrailRenderer::QUAD_INDICES;
    assert_eq!(&quad[..], &cube[0..6], "平面必须逐字复用顶面");
    // 角点坐标约束：位置 stride 6（pos3 + normal3），24 个顶点。
    let verts = MiditrailRenderer::CUBE_VERTICES;
    let (mut xs, mut zs) = (vec![], vec![]);
    for &i in quad.iter() {
        let base = (i as usize) * 6;
        assert!(base + 2 < verts.len(), "平面索引越界: {i}");
        assert_eq!(verts[base + 1], 1.0, "顶面角点 y 必须为 1: {i}");
        xs.push(verts[base]);
        zs.push(verts[base + 2]);
    }
    xs.sort_by(|a, b| a.partial_cmp(b).expect("角点 x 非法"));
    zs.sort_by(|a, b| a.partial_cmp(b).expect("角点 z 非法"));
    assert_eq!((xs[0], xs[3]), (0.0, 1.0), "X 宽必须保留");
    assert_eq!((zs[0], zs[3]), (0.0, 1.0), "Z 长必须保留");
}

/// 验证 wgpu 设备/提交/读回链路可用（与 Miditrail 无关的烟雾测试）。
#[test]
fn test_wgpu_basic_red_triangle() {
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
    let adapter = block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
        .expect("需要适配器");
    let (device, queue) = block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("basic_test_device"),
        required_features: adapter.features() & wgpu::Features::default(),
        required_limits: wgpu::Limits::default(),
        memory_hints: wgpu::MemoryHints::default(),
        trace: wgpu::Trace::Off,
        experimental_features: wgpu::ExperimentalFeatures::disabled(),
    }))
    .expect("请求设备失败");

    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("basic_test_output"),
        size: wgpu::Extent3d {
            width: 32,
            height: 32,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());

    let shader = crate::shader::create_shader_module(
        &device,
        "basic_test_shader",
        r#"
            @vertex
            fn vs_main(@builtin(vertex_index) idx: u32) -> @builtin(position) vec4<f32> {
                var pos = array<vec2<f32>, 3>(
                    vec2(-1.0, -1.0),
                    vec2(3.0, -1.0),
                    vec2(-1.0, 3.0),
                );
                return vec4(pos[idx], 0.0, 1.0);
            }
            @fragment
            fn fs_main() -> @location(0) vec4<f32> {
                return vec4(1.0, 0.0, 0.0, 1.0);
            }
        "#,
    );
    let pipeline =
        crate::pipeline::RenderPipelineBuilder::new(&device, "basic_test_pipeline", &shader)
            .opaque_target(wgpu::TextureFormat::Rgba8Unorm)
            .build();

    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("basic_test_encoder"),
    });
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("basic_test_pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        pass.set_pipeline(&pipeline);
        pass.draw(0..3, 0..1);
    }

    let bytes_per_row = (32u32 * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let staging = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("basic_test_staging"),
        contents: &vec![0u8; (bytes_per_row * 32) as usize],
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
    });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &staging,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(bytes_per_row),
                rows_per_image: Some(32),
            },
        },
        wgpu::Extent3d {
            width: 32,
            height: 32,
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
    let mut red_count = 0usize;
    for row in 0..32 {
        let row_start = (row * bytes_per_row) as usize;
        for col in 0..32 {
            let idx = row_start + col as usize * 4;
            if data[idx] == 255 && data[idx + 1] == 0 && data[idx + 2] == 0 {
                red_count += 1;
            }
        }
    }
    drop(data);
    staging.unmap();
    assert!(
        red_count > 100,
        "基础全屏红三角测试失败：红像素数 {red_count}"
    );
}

/// 渲染一帧并回读统计非黑像素（Normal/Top 视图切换测试共用）。
pub(super) fn render_and_count_non_black(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    renderer: &mut MiditrailRenderer,
    uniform: &MiditrailUniformGpu,
    notes: &[MiditrailNoteGpu],
) -> usize {
    let (width, height) = (uniform.frame_width, uniform.frame_height);
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("miditrail_switch_encoder"),
    });
    renderer.render(device, queue, &mut encoder, uniform, notes);
    let texture = renderer.output_texture().expect("渲染后应存在输出纹理");

    let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let padded_bytes_per_row = (width * 4).next_multiple_of(align);
    let buffer_size = (padded_bytes_per_row * height) as u64;
    let staging = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("miditrail_switch_staging"),
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
                rows_per_image: Some(height),
            },
        },
        wgpu::Extent3d {
            width,
            height,
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
    let mut non_black = 0usize;
    for row in 0..height {
        let row_start = (row * padded_bytes_per_row) as usize;
        for col in 0..width {
            let idx = row_start + (col * 4) as usize;
            if data[idx] != 0 || data[idx + 1] != 0 || data[idx + 2] != 0 {
                non_black += 1;
            }
        }
    }
    drop(data);
    staging.unmap();
    non_black
}

/// 等价性：`render_from_instances` 与 `render(手动换算)` 输出像素一致。
///
/// 换算逻辑从 `note_instances_to_miditrail` 逐行搬迁，覆盖边界：
/// 起始音符/跨视口长音符/右边界音符/越界 key（下游跳过）。
#[test]
fn test_render_from_instances_matches_manual_convert() {
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
    let adapter = block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
        .expect("测试需要可用的 wgpu 适配器");
    let (device, queue) = block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("miditrail_equiv_test_device"),
        required_features: adapter.features() & wgpu::Features::default(),
        required_limits: wgpu::Limits::default(),
        memory_hints: wgpu::MemoryHints::default(),
        trace: wgpu::Trace::Off,
        experimental_features: wgpu::ExperimentalFeatures::disabled(),
    }))
    .expect("请求 wgpu 设备失败");

    let uniform = MiditrailUniformGpu {
        tick: 480,
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
    // (start, length, key)：起始/长音/右边界/越界 key 全覆盖。
    let raw = [
        (0u32, 1920u32, 60u8),
        (480, 480, 64),
        (960, 240, 200),
        (480, 960, 67),
    ];
    let instances: Vec<crate::NoteInstance> = raw
        .iter()
        .map(|&(s, l, k)| crate::NoteInstance {
            start_length: [s as f32, (l as f32).max(1.0)],
            key_color: crate::pack_key_color(k, [1.0, 0.0, 0.0, 1.0]),
            border_width: 0,
        })
        .collect();
    // 旧路径参考换算（与已删除的 `note_instances_to_miditrail` 同公式）。
    let manual: Vec<MiditrailNoteGpu> = instances
        .iter()
        .map(|n| {
            let (key, rgb) = crate::unpack_key_color(n.key_color);
            let start = n.start_length[0].max(0.0) as u32;
            let end = start.saturating_add(n.start_length[1].max(1.0) as u32);
            MiditrailNoteGpu {
                key: key as u32,
                start_tick: start,
                end_tick: end,
                color_packed: pack_color([rgb[0], rgb[1], rgb[2], 1.0]),
                track_idx: 0,
                velocity: 100,
                channel: 0,
                _padding: 0,
            }
        })
        .collect();

    let mut renderer_a = MiditrailRenderer::new(&device);
    let via_render =
        render_and_count_non_black(&device, &queue, &mut renderer_a, &uniform, &manual);

    // 独立 renderer 跑快捷路径（动画初始状态与 renderer_a 一致才可比）。
    // 同一 encoder 内 render_from_instances＋拷贝读回，一次 submit。
    let mut renderer_b = MiditrailRenderer::new(&device);
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("miditrail_equiv_shortcut"),
    });
    renderer_b.render_from_instances(&device, &queue, &mut encoder, &uniform, &instances);
    let texture = renderer_b.output_texture().expect("渲染后应存在输出纹理");
    let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let padded = (320u32 * 4).next_multiple_of(align);
    let staging = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("miditrail_equiv_staging"),
        contents: &vec![0u8; (padded * 180) as usize],
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
                bytes_per_row: Some(padded),
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
    let mut shortcut_non_black = 0usize;
    for row in 0..180 {
        let row_start = (row * padded) as usize;
        for col in 0..320 {
            let idx = row_start + (col * 4) as usize;
            if data[idx] != 0 || data[idx + 1] != 0 || data[idx + 2] != 0 {
                shortcut_non_black += 1;
            }
        }
    }
    drop(data);
    staging.unmap();

    assert!(via_render > 0, "参考路径应渲染出可见内容");
    assert_eq!(
        shortcut_non_black, via_render,
        "render_from_instances 输出像素应与手动换算一致"
    );
}

/// A/B：平面模式是盒子模式的严格几何子集（行为零改动的可验证定义）。
///
/// 同实例/同顺序/同管线下，平面三角形逐个等于盒子的顶面三角形
/// （同顶点/同插值/不透明覆盖），故平面非黑像素集 ⊆ 盒子非黑像素集；
/// 琴键/Aura 两边完全一致（同缓冲同实例）。
/// 差异仅来自被删掉的侧面/底面/背面曾覆盖的像素——这正是开关要的外观变化。
/// 双视图循环：平面几何与视图无关（统一顶面），Normal/Top 逐个断言，
/// 防"分视图取面"类回归（曾误取正面压掉 Z 长）。
#[test]
fn test_flat_notes_is_pixel_subset_of_box() {
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
    let adapter = block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
        .expect("测试需要可用的 wgpu 适配器");
    let (device, queue) = block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("miditrail_flat_ab_test_device"),
        required_features: adapter.features() & wgpu::Features::default(),
        required_limits: wgpu::Limits::default(),
        memory_hints: wgpu::MemoryHints::default(),
        trace: wgpu::Trace::Off,
        experimental_features: wgpu::ExperimentalFeatures::disabled(),
    }))
    .expect("请求 wgpu 设备失败");

    // 两枚重叠红音符：覆盖画家排序 + 侧面交叠路径。
    let notes = [(480u32, 960u32, 60u8), (600u32, 480u32, 64u8)];
    let manual: Vec<MiditrailNoteGpu> = notes
        .iter()
        .map(|&(s, l, k)| MiditrailNoteGpu {
            key: u32::from(k),
            start_tick: s,
            end_tick: s + l,
            color_packed: pack_color([1.0, 0.0, 0.0, 1.0]),
            track_idx: 0,
            velocity: 100,
            channel: 0,
            _padding: 0,
        })
        .collect();

    for view_mode in [MiditrailViewMode::Normal, MiditrailViewMode::Top] {
        let uniform = MiditrailUniformGpu {
            tick: 480,
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
            view_mode,
            ticks_per_second: 960.0,
            _padding1: 0,
        };
        let mut renderer_box = MiditrailRenderer::new(&device);
        renderer_box.flat_notes = false;
        let box_pixels =
            render_and_count_non_black(&device, &queue, &mut renderer_box, &uniform, &manual);

        let mut renderer_quad = MiditrailRenderer::new(&device);
        renderer_quad.flat_notes = true;
        let quad_pixels =
            render_and_count_non_black(&device, &queue, &mut renderer_quad, &uniform, &manual);

        assert!(box_pixels > 0, "{view_mode:?} 盒子模式应渲染出可见内容");
        assert!(quad_pixels > 0, "{view_mode:?} 平面模式应渲染出可见内容");
        assert!(
            quad_pixels <= box_pixels,
            "{view_mode:?} 平面像素集应为盒子子集：quad={quad_pixels} box={box_pixels}"
        );
    }
}
