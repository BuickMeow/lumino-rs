//! 瀑布流 cull 路径与 legacy 窗口路径的像素等价性。
//!
//! 同一全集、两条管线、逐字节对比：legacy 侧消费 CPU 窗口（生产现状），
//! cull 侧消费常驻全集（`render_culled`：COUNT→FILL→内核→legacy 渲染）：
//! - `test_cull_render_matches_legacy`（严格）：并列同序 → 必须 bit-identical；
//! - `test_cull_render_dense_history`（严格）：300+ 死音符下的覆盖长音必须渲染
//!   （旧索引回溯在此漏检，cull 召回；集合层见 `global_bucket::cull_tests`）；
//! - `test_cull_tiebreak_deviation_is_bounded`（量化）：并列逆序（load 序 vs
//!   窗口序 tiebreak）→ 差异局限并列重叠区，总量封顶 2%。

use super::test_harness::*;
use crate::NoteInstance;

use crate::CullWindow;
use crate::waterfall_renderer::CullRenderOutcome;

/// 合成全集（load 序）：随机 3000 + 64 组并列（每组 3 个同 key 同 start）。
fn synthetic_full() -> Vec<NoteInstance> {
    let mut state = 0x9E37_79B9_7F4A_7C15u64;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    let mut notes = Vec::new();
    for i in 0..3000u32 {
        let key = (next() % 128) as u8;
        let start = (next() % 9000) as f32;
        let len = (50 + next() % 1500) as f32;
        let shade = (i % 7) as f32 / 7.0;
        notes.push(NoteInstance::new(
            start,
            key,
            len,
            [0.2 + shade * 0.5, 0.9 - shade * 0.4, 0.3 + shade * 0.3, 1.0],
            0,
        ));
    }
    for g in 0..64u32 {
        let key = (g * 37 % 128) as u8;
        let start = 4000.0 + (g % 8) as f32 * 100.0;
        for v in 0..3u32 {
            notes.push(NoteInstance::new(
                start,
                key,
                200.0 + v as f32 * 300.0,
                [0.2 + v as f32 * 0.3, 0.5, 0.9 - v as f32 * 0.2, 1.0],
                0,
            ));
        }
    }
    notes
}

/// 并列逆序：连续同 (key, start) 段整体反转（模拟 load 序 vs 窗口序 tiebreak 差异）。
fn reverse_ties(mut notes: Vec<NoteInstance>) -> Vec<NoteInstance> {
    let mut out = Vec::with_capacity(notes.len());
    let mut run: Vec<NoteInstance> = Vec::new();
    let key_of = |n: &NoteInstance| (n.key_color & 0xFF, n.start_length[0].max(0.0).to_bits());
    for n in notes.drain(..) {
        if run.last().is_some_and(|last| key_of(last) == key_of(&n)) {
            run.push(n);
        } else {
            run.iter().rev().for_each(|r| out.push(*r));
            run.clear();
            run.push(n);
        }
    }
    run.iter().rev().for_each(|r| out.push(*r));
    out
}

/// CPU 参考窗口（生产窗口语义：同谓词 + `(key, start)` 稳定排序）。
fn cpu_window(notes: &[NoteInstance], tick_start: u32, tick_end: u32) -> Vec<NoteInstance> {
    let mut out: Vec<NoteInstance> = notes
        .iter()
        .copied()
        .filter(|n| {
            let key = (n.key_color & 0xFF) as usize;
            let start = n.start_length[0].max(0.0) as u32;
            let end = start.saturating_add(n.start_length[1].max(1.0) as u32);
            key < 128 && end > tick_start && start < tick_end
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

/// legacy 分桶偏移派生（调用方须保证输入已按 key 聚簇）。
fn key_offsets_for(notes: &[NoteInstance], key_count: usize) -> Vec<u32> {
    let mut counts = vec![0u32; key_count];
    for n in notes {
        let key = (n.key_color & 0xFF) as usize;
        if key < key_count {
            counts[key] += 1;
        }
    }
    let mut offsets = vec![0u32; key_count + 1];
    for k in 0..key_count {
        offsets[k + 1] = offsets[k] + counts[k];
    }
    offsets
}

/// CPU 参考键色（生产 legacy 循环：窗口序 last-writer）。
fn cpu_active_colors(notes: &[NoteInstance], tick: u32) -> [u32; 128] {
    use crate::{pack_color, unpack_key_color};
    let mut colors = [0u32; 128];
    for n in notes.iter() {
        let (key, rgb) = unpack_key_color(n.key_color);
        let start = n.start_length[0].max(0.0) as u32;
        let end = start.saturating_add(n.start_length[1].max(1.0) as u32);
        if start <= tick && end > tick && (key as usize) < 128 {
            colors[key as usize] = pack_color([rgb[0], rgb[1], rgb[2], 1.0]) & 0xFFFF_FF00 | 153u32;
        }
    }
    colors
}

/// 双轨各渲染一帧并回读像素（legacy 窗口 vs cull 全集）。
fn render_both(window: &[NoteInstance], resident: &[NoteInstance]) -> (Vec<u8>, Vec<u8>) {
    let (device, queue) = test_device();
    let uniform = test_uniform();
    let (tick, tick_end) = test_window();
    let colors = cpu_active_colors(window, uniform.tick);

    // legacy：窗口集上传 + 派生分桶。
    let mut legacy_renderer = new_renderer(&device);
    let window_buf = upload_storage(&device, "waterfall_equiv_window", window);
    let key_offsets = key_offsets_for(window, 128);
    let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("waterfall_equiv_legacy"),
    });
    legacy_renderer.render(
        &device,
        &queue,
        &mut enc,
        &uniform,
        &window_buf,
        window.len(),
        &key_offsets,
        &colors,
    );
    let legacy_tex = legacy_renderer
        .output_texture()
        .expect("legacy 输出纹理应就绪");
    queue.submit(Some(enc.finish()));
    let legacy_pixels = readback_texture(&device, &queue, legacy_tex);

    // cull：常驻全集上传 + COUNT→FILL→内核→legacy 渲染。
    let mut cull_renderer = new_renderer(&device);
    let resident_buf = upload_storage(&device, "waterfall_equiv_resident", resident);
    cull_renderer.mark_resident_updated();
    let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("waterfall_equiv_cull"),
    });
    match cull_renderer.render_culled(
        &device,
        &queue,
        &mut enc,
        &uniform,
        &resident_buf,
        resident.len(),
        CullWindow {
            tick_start: tick,
            tick_end,
            key_count: 128,
        },
    ) {
        CullRenderOutcome::Culled { .. } => {}
        CullRenderOutcome::FallbackNeeded => panic!("cull 渲染应成功（合成数据无异常）"),
    }
    queue.submit(Some(enc.finish()));
    let cull_tex = cull_renderer.output_texture().expect("cull 输出纹理应就绪");
    let cull_pixels = readback_texture(&device, &queue, cull_tex);

    (legacy_pixels, cull_pixels)
}

fn diff_bytes(a: &[u8], b: &[u8]) -> usize {
    assert_eq!(a.len(), b.len(), "双轨输出字节数一致");
    a.iter().zip(b.iter()).filter(|(x, y)| x != y).count()
}

#[test]
fn test_cull_render_matches_legacy() {
    // 同序输入（并列同序）：cull 窗口与 CPU 窗口同集同序 → bit-identical。
    let full = synthetic_full();
    let (tick, tick_end) = test_window();
    let window = cpu_window(&full, tick, tick_end);
    let (legacy_pixels, cull_pixels) = render_both(&window, &full);
    let diffs = diff_bytes(&legacy_pixels, &cull_pixels);
    assert_eq!(
        diffs, 0,
        "同序输入下 cull 与 legacy 像素必须 bit-identical（差异字节 {diffs}）"
    );
}

#[test]
fn test_cull_render_dense_history() {
    // 密集死历史 + 覆盖长音：cull 召回长音，像素与 CPU 窗口一致。
    let (tick, tick_end) = test_window();
    let mut full = vec![NoteInstance::new(
        1000.0,
        60,
        9000.0,
        [1.0, 0.2, 0.2, 1.0],
        0,
    )];
    for i in 0..300u32 {
        full.push(NoteInstance::new(
            2000.0 + i as f32 * 10.0,
            60,
            5.0,
            [0.2, 0.5, 0.9, 1.0],
            0,
        ));
    }
    for i in 0..1500u32 {
        full.push(NoteInstance::new(
            (i * 17 % 9000) as f32,
            (i * 31 % 128) as u8,
            (50 + i * 13 % 800) as f32,
            [0.5, 0.5, 0.5, 1.0],
            0,
        ));
    }
    let window = cpu_window(&full, tick, tick_end);
    assert!(
        window
            .iter()
            .any(|n| (n.key_color & 0xFF) == 60 && n.start_length[0] == 1000.0),
        "CPU 参考窗口应含覆盖长音（测试前提）"
    );
    let (legacy_pixels, cull_pixels) = render_both(&window, &full);
    let diffs = diff_bytes(&legacy_pixels, &cull_pixels);
    assert_eq!(
        diffs, 0,
        "密集死历史下 cull 必须召回长音并与 legacy 逐位一致（差异字节 {diffs}）"
    );
}

#[test]
fn test_cull_tiebreak_deviation_is_bounded() {
    // 并列逆序（tiebreak 最大压力）：legacy 窗口取 load 序并列，常驻取逆序并列；
    // cull 输出并列倒置，last-writer 取色在重叠区不同，其余逐位一致。
    let full_ordered = synthetic_full();
    let full_reversed = reverse_ties(full_ordered.clone());
    let (tick, tick_end) = test_window();
    let window = cpu_window(&full_ordered, tick, tick_end);
    let (legacy_pixels, cull_pixels) = render_both(&window, &full_reversed);
    let diff_pixels = diff_bytes(&legacy_pixels, &cull_pixels).div_ceil(4);
    let total_pixels = (TEST_W * TEST_H) as usize;
    let ratio = diff_pixels as f64 / total_pixels as f64;
    // 验收线：并列逆序极端压力下差异像素 < 2%（差异仅来自同 (key, start) 并列的
    // load 序 vs 窗口序 last-writer 取色不同；超标说明影响超出并列语义）。
    assert!(
        ratio < 0.02,
        "tiebreak 差异像素占比超标：{diff_pixels}/{total_pixels} = {ratio:.4}"
    );
}
