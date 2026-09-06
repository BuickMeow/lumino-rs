//! GPU-Driven 与 legacy 路径等价性：同一输入、两条管线、像素级对比。
//!
//! 背景：真机 dense 帧发现键盘区亮度差异（f0/f100 一致，f300 有 wash 缺失），
//! 本模块把差分收敛为可复现的单元测试：CPU 融合扫描逐位对比＋GPU 像素对比。

use super::super::instances::{
    build_aura_instances, compute_active_and_aura_for_compact, compute_active_keys,
    emit_aura_instances, update_key_positions,
};
use super::super::*;
use crate::NoteInstance;
use futures::executor::block_on;
use wgpu::util::DeviceExt;

/// 高密度合成场景：128 键全覆盖、起始交错（active/未开始/已结束混合）、
///
/// 同键叠音（稳定性）与黑白键重叠（覆盖序），复刻真机 dense 帧的特征。
fn dense_scene() -> Vec<NoteInstance> {
    let mut notes = Vec::new();
    let mut state = 0x9E37_79B9_7F4A_7C15u64;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    // tick=5000 处约 1/3 active、1/3 未开始、1/3 已结束（collect 语义只留 end>tick，
    // 此处故意混入已结束音符验证 legacy/Driven 过滤一致性——Driven 由 shader 剔除）。
    for i in 0..6000u32 {
        let key = (next() % 128) as u8;
        let start = (next() % 9000) as f32;
        let len = (50 + next() % 1500) as f32;
        let shade = (i % 5) as f32 * 0.2;
        notes.push(NoteInstance::new(
            start,
            key,
            len,
            [0.2 + shade, 0.9 - shade * 0.5, 0.3, 1.0],
            0,
        ));
    }
    notes
}

fn test_uniform() -> MiditrailUniformGpu {
    MiditrailUniformGpu {
        tick: 5000,
        ppq: 480,
        key_count: 128,
        frame_width: 640,
        frame_height: 360,
        kb_height: 43,
        _reserved: 0,
        speed: 1.0,
        param1: 0.0,
        param2: 0.0,
        fps: 60.0,
        z_far_distance: 7.5,
        view_mode: MiditrailViewMode::Normal,
        ticks_per_second: 960.0,
        _padding1: 0,
    }
}

fn test_device() -> (wgpu::Instance, wgpu::Device, wgpu::Queue) {
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
    let adapter = block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
        .expect("测试需要可用的 wgpu 适配器");
    let (device, queue) = block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("miditrail_driven_equiv_device"),
        required_features: adapter.features() & wgpu::Features::default(),
        required_limits: wgpu::Limits::default(),
        memory_hints: wgpu::MemoryHints::default(),
        trace: wgpu::Trace::Off,
        experimental_features: wgpu::ExperimentalFeatures::disabled(),
    }))
    .expect("请求 wgpu 设备失败");
    (instance, device, queue)
}

/// CPU 级：融合扫描（active＋aura）与 legacy 两次扫描逐位一致。
#[test]
fn test_fused_scan_matches_legacy() {
    let notes = dense_scene();
    let uniform = test_uniform();

    // legacy 输入：与 `render_from_instances` 逐 op 一致的换算。
    let derived: Vec<MiditrailNoteGpu> = notes
        .iter()
        .map(|n| {
            let (key, rgb) = crate::unpack_key_color(n.key_color);
            let start = n.start_length[0].max(0.0) as u32;
            let end = start.saturating_add(n.start_length[1].max(1.0) as u32);
            MiditrailNoteGpu {
                key: key as u32,
                start_tick: start,
                end_tick: end,
                color_packed: crate::miditrail_renderer::pack_color([rgb[0], rgb[1], rgb[2], 1.0]),
                track_idx: 0,
                velocity: 100,
                channel: 0,
                _padding: 0,
            }
        })
        .collect();

    let expected_active = compute_active_keys(uniform.tick, &derived);
    let (actual_active, aura_sizes) = compute_active_and_aura_for_compact(
        uniform.tick,
        uniform.ticks_per_second,
        uniform.fps,
        &notes,
    );
    assert_eq!(
        expected_active.pressed, actual_active.pressed,
        "pressed 必须逐键一致"
    );
    assert_eq!(
        expected_active.colors, actual_active.colors,
        "激活颜色必须逐键一致"
    );

    // aura 实例逐位对比（legacy 全量扫描 vs 融合预聚合＋emit）。
    let mut positions = Vec::new();
    let mut widths = Vec::new();
    let mut last = 0u32;
    update_key_positions(128, &mut last, &mut positions, &mut widths);
    let mut expected_auras = Vec::new();
    build_aura_instances(
        &uniform,
        &derived,
        &expected_active,
        &positions,
        &widths,
        &mut expected_auras,
    );
    let mut actual_auras = Vec::new();
    emit_aura_instances(
        &actual_active,
        &aura_sizes,
        uniform.key_count as usize,
        &positions,
        &widths,
        &mut actual_auras,
    );
    assert_eq!(
        expected_auras.len(),
        actual_auras.len(),
        "aura 实例数必须一致"
    );
    for (i, (a, b)) in expected_auras.iter().zip(actual_auras.iter()).enumerate() {
        assert_eq!(a.size, b.size, "第 {i} 个 aura 尺寸不一致");
        assert_eq!(a.pos, b.pos, "第 {i} 个 aura 位置不一致");
        assert_eq!(a.color_packed, b.color_packed, "第 {i} 个 aura 颜色不一致");
    }
}

/// 回读一帧 RGBA（去 row padding）。
fn readback_pixels(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    encoder: wgpu::CommandEncoder,
    texture: &wgpu::Texture,
    width: u32,
    height: u32,
) -> Vec<u8> {
    let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let padded = (width * 4).next_multiple_of(align);
    let staging = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("miditrail_driven_equiv_staging"),
        contents: &vec![0u8; (padded * height) as usize],
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
    });
    let mut encoder = encoder;
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
    let mut out = Vec::with_capacity((width * height * 4) as usize);
    for row in 0..height {
        let s = (row * padded) as usize;
        out.extend_from_slice(&data[s..s + (width * 4) as usize]);
    }
    drop(data);
    staging.unmap();
    out
}

/// 单音符精确对照：active 提亮＋几何必须逐像素一致（排除排序干扰）。
///
/// 一个 active 音符（start<=tick<end）＋一个未开始音符，分别走两条路径，
/// 差异通道必须为 0（允许 ±1 LSB 量化，共 4 个通道以内差异且差值 ≤1）。
#[test]
fn test_driven_single_active_note_matches() {
    let (_instance, device, queue) = test_device();
    let mut uniform = test_uniform();
    uniform.frame_width = 320;
    uniform.frame_height = 180;
    uniform.tick = 1000;
    // 红色 active 音符（boost 后应为粉白）＋绿色未开始音符。
    let notes = vec![
        NoteInstance::new(900.0, 60, 500.0, [1.0, 0.0, 0.0, 1.0], 0),
        NoteInstance::new(3000.0, 64, 500.0, [0.0, 1.0, 0.0, 1.0], 0),
    ];
    let (w, h) = (uniform.frame_width, uniform.frame_height);

    let mut legacy_renderer = MiditrailRenderer::new(&device);
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("miditrail_single_legacy"),
    });
    legacy_renderer.render_from_instances(&device, &queue, &mut encoder, &uniform, &notes);
    queue.submit(std::iter::once(encoder.finish()));
    let legacy_tex = legacy_renderer.output_texture().expect("legacy 应有输出");

    let mut driven_renderer = MiditrailRenderer::new(&device);
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("miditrail_single_driven"),
    });
    driven_renderer.render_gpu_driven(&device, &queue, &mut encoder, &uniform, &notes);
    queue.submit(std::iter::once(encoder.finish()));
    let driven_tex = driven_renderer.output_texture().expect("driven 应有输出");

    let encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("miditrail_single_rb0"),
    });
    let legacy_px = readback_pixels(&device, &queue, encoder, legacy_tex, w, h);
    let encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("miditrail_single_rb1"),
    });
    let driven_px = readback_pixels(&device, &queue, encoder, driven_tex, w, h);

    let mut over_one_lsb = 0usize;
    let mut max_diff = 0u8;
    for (a, b) in legacy_px.iter().zip(driven_px.iter()) {
        let d = a.abs_diff(*b);
        max_diff = max_diff.max(d);
        if d > 1 {
            over_one_lsb += 1;
        }
    }
    assert_eq!(
        over_one_lsb, 0,
        "单音符两条路径差异超 ±1LSB：{over_one_lsb} 通道，最大差 {max_diff}"
    );
}

/// GPU 级：同输入下 legacy 与 Driven 像素差异（已知：画家 vs 深度顺序语义差）。
///
/// 2026-09-05 结论：稀疏帧一致，密集重叠带存在系统性 winners 差异（YAVG~9@f300）。
/// UI 侧验收未通过，driven 已回退；本测试 ignore 保留，待观感方案确定后重启用。
/// 允许项：画家排序 vs 深度测试在重叠边界的离散差异、float±1LSB。
/// 不允许：系统性亮度/颜色/缺失差异（如 dense 帧 wash 缺失会直接超标）。
#[test]
#[ignore = "driven 已回退：画家vs深度观感差异待产品决策"]
fn test_driven_pixels_match_legacy() {
    let (_instance, device, queue) = test_device();
    let uniform = test_uniform();
    let notes = dense_scene();
    let (w, h) = (uniform.frame_width, uniform.frame_height);

    let mut legacy_renderer = MiditrailRenderer::new(&device);
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("miditrail_equiv_legacy"),
    });
    legacy_renderer.render_from_instances(&device, &queue, &mut encoder, &uniform, &notes);
    queue.submit(std::iter::once(encoder.finish()));
    let legacy_tex = legacy_renderer
        .output_texture()
        .expect("legacy 渲染后应存在输出纹理");

    let mut driven_renderer = MiditrailRenderer::new(&device);
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("miditrail_equiv_driven"),
    });
    driven_renderer.render_gpu_driven(&device, &queue, &mut encoder, &uniform, &notes);
    queue.submit(std::iter::once(encoder.finish()));
    let driven_tex = driven_renderer
        .output_texture()
        .expect("driven 渲染后应存在输出纹理");

    // 两次回读需各自的 encoder（texture borrow 结束后续借）：重建 encoder。
    let encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("miditrail_equiv_readback_legacy"),
    });
    let legacy_px = readback_pixels(&device, &queue, encoder, legacy_tex, w, h);
    let encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("miditrail_equiv_readback_driven"),
    });
    let driven_px = readback_pixels(&device, &queue, encoder, driven_tex, w, h);

    assert_eq!(legacy_px.len(), driven_px.len());
    let mut diff_pixels = 0usize;
    let mut max_diff = 0u8;
    // 键盘区（底部 25%）与音符区分开统计，定位差异来源。
    let mut diff_top = 0usize;
    let mut diff_bottom = 0usize;
    let split_row = h * 3 / 4;
    for row in 0..h {
        for col in 0..w {
            let idx = ((row * w + col) * 4) as usize;
            let mut row_diff = false;
            for c in 0..4 {
                let d = legacy_px[idx + c].abs_diff(driven_px[idx + c]);
                max_diff = max_diff.max(d);
                if d > 8 {
                    diff_pixels += 1;
                    row_diff = true;
                }
            }
            if row_diff {
                if row < split_row {
                    diff_top += 1;
                } else {
                    diff_bottom += 1;
                }
            }
        }
    }
    // 按"差异通道数/总通道数"计：重叠边界离散点应远低于千分之五。
    let total = legacy_px.len();
    let ratio = diff_pixels as f64 / total as f64;
    assert!(
        ratio < 0.005,
        "像素差异率超标：{ratio:.5}（差异通道 {diff_pixels}/{total}，最大差 {max_diff}，音符区差异像素 {diff_top}，键盘区差异像素 {diff_bottom}）"
    );
}
