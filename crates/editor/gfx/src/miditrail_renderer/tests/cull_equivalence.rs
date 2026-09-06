//! Miditrail 导出 cull 等价性：`seed_resident` → `cull_window` 回读 ≡ CPU 窗口。
//!
//! 覆盖渲染器自有常驻路径（播种/世代/回读装配），谓词层已由
//! `global_bucket::cull_tests` 证明；此处断言经回读的 compact 与 CPU 参考
//!（同谓词 + 同序）逐字节一致——回读后 legacy 渲染像素随之逐位一致
//!（`build_note_instances` 内部稳定排序，输入同序则输出同序）。

use super::super::{MiditrailRenderer, types::miditrail_viewport_span};
use crate::{CullWindow, NoteInstance};
use futures::executor::block_on;

fn test_device() -> (wgpu::Device, wgpu::Queue) {
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
    let adapter = block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
        .expect("测试需要可用的 wgpu 适配器");
    block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("miditrail_cull_equiv_device"),
        required_features: adapter.features() & wgpu::Features::default(),
        required_limits: wgpu::Limits::default(),
        memory_hints: wgpu::MemoryHints::default(),
        trace: wgpu::Trace::Off,
        experimental_features: wgpu::ExperimentalFeatures::disabled(),
    }))
    .expect("请求 wgpu 设备失败")
}

/// CPU 参考窗口（UI 收集 + 排序语义；打包 end 语义，合成数据无零长音符）。
fn cpu_window(
    notes: &[NoteInstance],
    tick_start: u32,
    tick_end: u32,
    key_count: usize,
) -> Vec<NoteInstance> {
    let mut out: Vec<NoteInstance> = notes
        .iter()
        .copied()
        .filter(|n| {
            let key = (n.key_color & 0xFF) as usize;
            let start = n.start_length[0].max(0.0) as u32;
            let end = start.saturating_add(n.start_length[1].max(1.0) as u32);
            key < key_count && end > tick_start && start < tick_end
        })
        .collect();
    out.sort_by(|a, b| {
        let ka = a.key_color & 0xFF;
        let kb = b.key_color & 0xFF;
        ka.cmp(&kb).then_with(|| {
            a.start_length[0]
                .max(0.0)
                .total_cmp(&b.start_length[0].max(0.0))
        })
    });
    out
}

fn synthetic_full() -> Vec<NoteInstance> {
    let mut state = 0x51F1_5EED_1234_5678u64;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    let mut notes = Vec::new();
    for i in 0..2500u32 {
        let key = (next() % 128) as u8;
        let start = (next() % 12000) as f32;
        let len = (60 + next() % 2000) as f32;
        let shade = (i % 5) as f32 / 5.0;
        notes.push(NoteInstance::new(
            start,
            key,
            len,
            [0.3 + shade * 0.4, 0.8 - shade * 0.3, 0.4 + shade * 0.2, 1.0],
            0,
        ));
    }
    // 并列组（同 key 同 start 不同长度，cull 与 CPU 同为 load 序）。
    for g in 0..48u32 {
        let key = (g * 53 % 128) as u8;
        let start = 5000.0 + (g % 6) as f32 * 200.0;
        for v in 0..3u32 {
            notes.push(NoteInstance::new(
                start,
                key,
                300.0 + v as f32 * 400.0,
                [0.3 + v as f32 * 0.2, 0.6, 0.8 - v as f32 * 0.15, 1.0],
                0,
            ));
        }
    }
    notes
}

#[test]
fn test_miditrail_cull_window_matches_cpu() {
    const TICK: u32 = 5000;
    const KEY_COUNT: usize = 128;
    // 与生产同公式（ppq=480, speed=1.0, z_far=7.5=SCENE_DEPTH → 全跨度 7680）。
    let tick_end = TICK.saturating_add(miditrail_viewport_span(480, 1.0, 7.5));
    let notes = synthetic_full();
    let expected = cpu_window(&notes, TICK, tick_end, KEY_COUNT);
    assert!(!expected.is_empty(), "合成窗口非空（测试前提）");

    let (device, queue) = test_device();
    let mut renderer = MiditrailRenderer::new(&device);
    renderer.seed_resident(&device, &queue, &notes);
    let (window, _timing) = renderer
        .cull_window(
            &device,
            &queue,
            CullWindow {
                tick_start: TICK,
                tick_end,
                key_count: KEY_COUNT,
            },
        )
        .expect("cull 窗口提取应成功");
    assert_eq!(
        window.len(),
        expected.len(),
        "cull 窗口数量必须与 CPU 参考一致"
    );
    assert_eq!(
        bytemuck::cast_slice::<NoteInstance, u8>(&window),
        bytemuck::cast_slice::<NoteInstance, u8>(&expected),
        "cull 回读必须与 CPU 窗口逐字节一致（含序）"
    );
    renderer.restore_window(window);
}

#[test]
fn test_miditrail_cull_empty_window() {
    // 空窗口（tick 越过全部音符）：返回空集，不回读、不崩溃。
    let notes = synthetic_full();
    let (device, queue) = test_device();
    let mut renderer = MiditrailRenderer::new(&device);
    renderer.seed_resident(&device, &queue, &notes);
    let (window, _timing) = renderer
        .cull_window(
            &device,
            &queue,
            CullWindow {
                tick_start: 9_000_000,
                tick_end: 9_010_000,
                key_count: 128,
            },
        )
        .expect("空窗口 cull 应成功");
    assert!(window.is_empty(), "越过全曲的窗口必须为空");
    renderer.restore_window(window);
}
