//! 活跃键 GPU 内核与 CPU 循环的逐位一致性。
//!
//! 生产窗口序 last-writer（窗口集恒 `(key, start)` 稳定有序）等价于内核的
//! “桶内自上而下回溯首个覆盖者”（窗口含全部覆盖音符）：含并列与 f32 来回截断。

use super::test_harness::*;
use crate::NoteInstance;

/// CPU 参考循环（生产 legacy 语义）：稳定排序后窗口序 last-writer。
fn cpu_active_colors_sorted(notes: &[NoteInstance], tick: u32) -> [u32; 128] {
    use crate::{pack_color, unpack_key_color};
    let mut sorted = notes.to_vec();
    sorted.sort_by(|a, b| {
        let ka = a.key_color & 0xFF;
        let kb = b.key_color & 0xFF;
        ka.cmp(&kb).then_with(|| {
            let sa = a.start_length[0].max(0.0);
            let sb = b.start_length[0].max(0.0);
            sa.total_cmp(&sb)
        })
    });
    let mut colors = [0u32; 128];
    for n in sorted.iter() {
        let (key, rgb) = unpack_key_color(n.key_color);
        let start = n.start_length[0].max(0.0) as u32;
        let end = start.saturating_add(n.start_length[1].max(1.0) as u32);
        if start <= tick && end > tick && (key as usize) < 128 {
            colors[key as usize] = pack_color([rgb[0], rgb[1], rgb[2], 1.0]) & 0xFFFF_FF00 | 153u32;
        }
    }
    colors
}

#[test]
fn test_active_kernel_matches_cpu_loop() {
    use crate::GlobalBucketIndex;
    // 活跃/已结束/未开始/越界键/零长/并列全覆盖；通道值遍历 stress f32 来回截断。
    let tick = 5000u32;
    let mut notes = Vec::new();
    for i in 0..2000u32 {
        let key = (i * 131 % 160) as u8; // 含 >=128 越界键
        let start = ((i * 37) % 9000) as f32;
        let len = (1 + i * 13 % 2000) as f32;
        let r = ((i * 17) % 256) as f32 / 255.0;
        let g = ((i * 29) % 256) as f32 / 255.0;
        let b = ((i * 43) % 256) as f32 / 255.0;
        notes.push(NoteInstance::new(start, key, len, [r, g, b, 1.0], 0));
    }
    // 刻意并列：同 key 同 start 不同颜色（load 序后赢 vs CPU 循环后赢，必须一致）。
    for v in 0..3u32 {
        notes.push(NoteInstance::new(
            4900.0,
            60,
            500.0,
            [0.1 + v as f32 * 0.3, 0.5, 0.9 - v as f32 * 0.2, 1.0],
            0,
        ));
    }
    let expected = cpu_active_colors_sorted(&notes, tick);

    let (device, queue) = test_device();
    let resident = upload_storage(&device, "waterfall_active_test_notes", &notes);
    let bucket = GlobalBucketIndex::build(&device, &queue, &resident, notes.len())
        .expect("活跃键测试全局桶构建应成功");

    // 最小管线装配（与 `WaterfallRenderer::run_active_kernel` 同布局语义）。
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("waterfall_active_test"),
        source: wgpu::ShaderSource::Wgsl(include_str!("../shaders/waterfall_active.wgsl").into()),
    });
    let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("waterfall_active_test_bgl"),
        entries: &[
            uniform_entry_for_test(0),
            storage_ro_entry_for_test(1),
            storage_ro_entry_for_test(2),
            storage_ro_entry_for_test(3),
            storage_rw_entry_for_test(4),
        ],
    });
    let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("waterfall_active_test_pipe"),
        layout: Some(
            &device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("waterfall_active_test_layout"),
                bind_group_layouts: &[&layout],
                push_constant_ranges: &[],
            }),
        ),
        module: &module,
        entry_point: Some("main"),
        compilation_options: wgpu::PipelineCompilationOptions::default(),
        cache: None,
    });
    let params_buf = upload_storage_u32(&device, "waterfall_active_test_params", &[tick, 0, 0, 0]);
    let colors_buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("waterfall_active_test_colors"),
        size: 512,
        usage: wgpu::BufferUsages::STORAGE
            | wgpu::BufferUsages::COPY_DST
            | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("waterfall_active_test_bg"),
        layout: &layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: params_buf.as_entire_binding(),
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
                resource: colors_buf.as_entire_binding(),
            },
        ],
    });
    let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("waterfall_active_test_enc"),
    });
    {
        let mut cpass = enc.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("waterfall_active_test_pass"),
            timestamp_writes: None,
        });
        cpass.set_pipeline(&pipeline);
        cpass.set_bind_group(0, &bg, &[]);
        cpass.dispatch_workgroups(1, 1, 1);
    }
    queue.submit(Some(enc.finish()));
    let got = readback_u32_vec(&device, &queue, &colors_buf, 128);
    assert_eq!(
        got.as_slice(),
        &expected,
        "活跃键内核必须与 CPU 循环逐位一致（含并列与颜色取整）"
    );
}
