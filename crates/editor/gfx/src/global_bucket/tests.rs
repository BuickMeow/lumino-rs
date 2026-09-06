//! 全局桶单测（TDD）：纯函数断言 + 真机 GPU 构建验证。

use super::*;
use crate::NoteInstance;
use futures::executor::block_on;

fn test_device() -> (wgpu::Device, wgpu::Queue) {
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
    let adapter = block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
        .expect("测试需要可用的 wgpu 适配器");
    block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("global_bucket_test_device"),
        required_features: adapter.features() & wgpu::Features::default(),
        required_limits: wgpu::Limits::default(),
        memory_hints: wgpu::MemoryHints::default(),
        trace: wgpu::Trace::Off,
        experimental_features: wgpu::ExperimentalFeatures::disabled(),
    }))
    .expect("请求 wgpu 设备失败")
}

fn upload_notes(device: &wgpu::Device, notes: &[NoteInstance]) -> wgpu::Buffer {
    use wgpu::util::DeviceExt;
    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("global_bucket_test_notes"),
        contents: bytemuck::cast_slice(notes),
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
    })
}

/// 通用 GPU→CPU 同步回读（测试用；1KB 产物复用构建侧模式）。
fn readback_buffer(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    src: &wgpu::Buffer,
    size: u64,
) -> Vec<u8> {
    let staging = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("global_bucket_test_staging"),
        size,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("global_bucket_test_readback"),
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
            let out = data.to_vec();
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

fn readback_u32_vec(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    src: &wgpu::Buffer,
    count: usize,
) -> Vec<u32> {
    let bytes = readback_buffer(device, queue, src, (count * 4) as u64);
    bytemuck::cast_slice(&bytes).to_vec()
}

fn make_note(start: f32, key: u8, len: f32) -> NoteInstance {
    NoteInstance::new(start, key, len, [1.0, 0.0, 0.0, 1.0], 0)
}

#[test]
fn test_readback_helper_is_trustworthy() {
    // 前置验证：独立 encoder 拷贝 + map 回读是否如实返回数据。
    // 若此测试失败，说明回读模式本身有问题，后续所有 GPU 断言都不可信。
    let (device, queue) = test_device();
    let expected: Vec<u32> = (0..64u32).map(|i| i.wrapping_mul(2654435761)).collect();
    let src = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("global_bucket_test_known"),
        size: (expected.len() * 4) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    queue.write_buffer(&src, 0, bytemuck::cast_slice(&expected));
    let got = readback_u32_vec(&device, &queue, &src, expected.len());
    assert_eq!(got, expected, "回读必须如实返回写入数据");
}

#[test]
fn test_exclusive_prefix_basic() {
    let mut hist = [0u32; HIST_LEN];
    hist[0] = 3;
    hist[1] = 0;
    hist[2] = 5;
    hist[255] = 1;
    let pfx = exclusive_prefix(&hist);
    assert_eq!(pfx[0], 0, "首桶起始为 0");
    assert_eq!(pfx[1], 3, "空桶不推进");
    assert_eq!(pfx[2], 3, "前缀和连续");
    assert_eq!(pfx[3], 8, "前缀和连续");
    assert_eq!(pfx[255], 8, "末桶前缀正确");
}

#[test]
fn test_sort_passes_shape() {
    let passes = sort_passes();
    assert_eq!(passes.len(), 5, "4 个 start 字节 pass + 1 个 key pass");
    assert_eq!(
        passes[..4],
        [(0, false), (8, false), (16, false), (24, false)],
        "先排 start（低位在前，LSD）"
    );
    assert_eq!(passes[4], (0, true), "最后稳定分桶 key");
}

#[test]
fn test_build_empty_is_valid() {
    let (device, queue) = test_device();
    let notes: Vec<NoteInstance> = Vec::new();
    let buf = upload_notes(&device, &notes);
    let index = GlobalBucketIndex::build(&device, &queue, &buf, 0).expect("空集构建应成功");
    assert_eq!(index.note_count(), 0, "空集数量为 0");
    let offsets = readback_u32_vec(&device, &queue, index.key_offsets_buffer(), OFFSETS_LEN);
    assert!(
        offsets.iter().all(|o| *o == 0),
        "空集桶边界全零：{offsets:?}"
    );
}

#[test]
fn test_build_single_note() {
    let (device, queue) = test_device();
    let notes = vec![make_note(100.0, 60, 50.0)];
    let buf = upload_notes(&device, &notes);
    let index = GlobalBucketIndex::build(&device, &queue, &buf, 1).expect("单音符构建应成功");
    let order = readback_u32_vec(&device, &queue, index.sort_index_buffer(), 1);
    assert_eq!(order, vec![0], "单音符索引恒等");
    let offsets = readback_u32_vec(&device, &queue, index.key_offsets_buffer(), OFFSETS_LEN);
    assert_eq!(offsets[60], 0, "60 号桶起始为 0");
    assert_eq!(offsets[61], 1, "60 号桶含 1 个音符");
    assert_eq!(offsets[256], 1, "哨兵为总数");
}

#[test]
fn test_build_orders_by_key_then_start() {
    let (device, queue) = test_device();
    // 故意逆序 + 跨桶交错 + 同 (key, start) 并列（下标 3/4 并列，验证稳定性）。
    let input: Vec<(f32, u8)> = vec![
        (900.0, 72),
        (100.0, 60),
        (500.0, 60),
        (300.0, 64),
        (300.0, 64),
        (200.0, 60),
        (700.0, 0),
        (50.0, 255),
    ];
    let notes: Vec<NoteInstance> = input
        .iter()
        .map(|(start, key)| make_note(*start, *key, 10.0))
        .collect();
    let buf = upload_notes(&device, &notes);
    let index =
        GlobalBucketIndex::build(&device, &queue, &buf, notes.len()).expect("多音符构建应成功");
    let order = readback_u32_vec(&device, &queue, index.sort_index_buffer(), notes.len());
    let ordered: Vec<(u8, u32)> = order
        .iter()
        .map(|i| (input[*i as usize].1, input[*i as usize].0 as u32))
        .collect();
    let mut sorted = ordered.clone();
    sorted.sort();
    assert_eq!(ordered, sorted, "全局 (key, start) 有序：{ordered:?}");
    // 并列稳定性：输入下标 3/4 同 (64, 300)，有序输出中 3 必须在 4 之前。
    let pos3 = order.iter().position(|i| *i == 3).expect("下标 3 应存在");
    let pos4 = order.iter().position(|i| *i == 4).expect("下标 4 应存在");
    assert!(pos3 < pos4, "并列保持 load 顺序：pos3={pos3} pos4={pos4}");
}

#[test]
fn test_key_offsets_match_histogram() {
    let (device, queue) = test_device();
    // 2000 音符撒过 256 键（含空桶），key 由下标派生保证覆盖。
    let notes: Vec<NoteInstance> = (0..2000u32)
        .map(|i| make_note((i * 37 % 10000) as f32, (i * 131 % 256) as u8, 10.0))
        .collect();
    let mut expected = [0u32; KEY_BUCKETS];
    for i in 0..2000u32 {
        expected[(i * 131 % 256) as usize] += 1;
    }
    let buf = upload_notes(&device, &notes);
    let index =
        GlobalBucketIndex::build(&device, &queue, &buf, notes.len()).expect("直方图测试构建应成功");
    let offsets = readback_u32_vec(&device, &queue, index.key_offsets_buffer(), OFFSETS_LEN);
    assert_eq!(offsets.len(), OFFSETS_LEN, "桶边界长度 257");
    let mut acc = 0u32;
    for k in 0..KEY_BUCKETS {
        assert_eq!(offsets[k], acc, "{k} 号桶起始偏移");
        assert_eq!(
            offsets[k + 1] - offsets[k],
            expected[k],
            "{k} 号桶计数（期望 {}，含空桶零计数）",
            expected[k]
        );
        acc += expected[k];
    }
    assert_eq!(offsets[256], 2000, "哨兵为总数");
}

#[test]
fn test_build_medium_random_stays_ordered() {
    let (device, queue) = test_device();
    // 5 万随机音符：验证全局有序（输入 (key, start) 已知，无需回读 note 字节）。
    let input: Vec<(u8, u32)> = (0..50_000u32)
        .map(|i| {
            let key = (i.wrapping_mul(2654435761u32) >> 24) as u8;
            let start = i.wrapping_mul(40503) % 600_000;
            (key, start)
        })
        .collect();
    let notes: Vec<NoteInstance> = input
        .iter()
        .map(|(key, start)| make_note(*start as f32, *key, 10.0))
        .collect();
    let buf = upload_notes(&device, &notes);
    let index =
        GlobalBucketIndex::build(&device, &queue, &buf, notes.len()).expect("5 万构建应成功");
    let order = readback_u32_vec(&device, &queue, index.sort_index_buffer(), notes.len());
    assert_eq!(order.len(), notes.len(), "置换索引长度为 N");
    let mut seen = vec![false; notes.len()];
    let mut prev = (0u8, 0u32);
    for (p, src) in order.iter().enumerate() {
        assert!((*src as usize) < notes.len(), "索引越界：{src}");
        assert!(!seen[*src as usize], "索引重复：{src}");
        seen[*src as usize] = true;
        let cur = input[*src as usize];
        assert!(cur >= prev, "位置 {p} 逆序：前 {prev:?} 后 {cur:?}");
        prev = cur;
    }
}
