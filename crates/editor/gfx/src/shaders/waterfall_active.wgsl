// waterfall_active.wgsl — 全局桶活跃键计算（128 线程，每键一线程）
//
// 背景：UI 生产侧跳过窗口构建后，渲染线程不再持有窗口集；活跃键色改由本内核
// 直接在常驻缓冲上推导（与 handle_waterfall_frame 旧 CPU 循环逐 op 一致）。
// CPU 语义（`video_export/waterfall.rs` 索引化前）：
//   start <= tick < end 且 key < 128 → colors[key] = pack(unpack(rgb)) & mask | 153，
//   同键多覆盖取窗口序最后一个（start 最大；并列取 load 序最后）。
// GPU 复刻：桶内二分定位 start <= tick 上界，自上而下回溯首个覆盖者即最大 start；
// 并列内自后向前，首个命中即 load 序最后——与 CPU last-writer 一致
// （跨轨同 key 同 start 并列且颜色不同时取 load 序最后，属已接受的 tiebreak 类，见全局桶文档）。
// 颜色换算复刻 `unpack_key_color` + `pack_color` 的 f32 来回（含截断语义，
// IEEE 单精度 CPU/GPU 同序同结果，单测覆盖 等价性）：
//   r/g/b = 通道值 / 255.0 → clamp → ×255.0 截断 → RGB + alpha 153。
// 回溯无上限（正确性优先：超长 pad 音后的 staccato 墓地也必须找到 pad；
// 代价上限为桶长，典型数步命中；legacy CPU 扫描全窗口，开销只多不少）。
// dispatch: (1, 1, 1)，单 workgroup 128 线程。

struct ActiveParams {
    tick: u32,
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
}

struct WaterfallNote {
    start_length: vec2<f32>,
    key_color: u32,
    border_width: u32,
}

@group(0) @binding(0) var<uniform> params: ActiveParams;
@group(0) @binding(1) var<storage, read> notes: array<WaterfallNote>;
@group(0) @binding(2) var<storage, read> key_offsets: array<u32>;
@group(0) @binding(3) var<storage, read> sort_index: array<u32>;
@group(0) @binding(4) var<storage, read_write> active_colors: array<u32>;

fn note_start(n: WaterfallNote) -> u32 {
    // 与 waterfall.wgsl note_start 逐字一致（含 max 钳负）。
    return u32(max(n.start_length.x, 0.0));
}

fn note_end(n: WaterfallNote) -> u32 {
    return note_start(n) + u32(max(n.start_length.y, 1.0));
}

// 复刻 unpack_key_color → pack_color → & 0xFFFFFF00 | 153（与 CPU 逐 op 一致，
/// f32 来回的截断语义由 IEEE 单精度保证 CPU/GPU 一致）。
fn active_color_for(key_color: u32) -> u32 {
    let r = f32((key_color >> 24u) & 0xFFu) / 255.0;
    let g = f32((key_color >> 16u) & 0xFFu) / 255.0;
    let b = f32((key_color >> 8u) & 0xFFu) / 255.0;
    let r2 = u32(clamp(r, 0.0, 1.0) * 255.0);
    let g2 = u32(clamp(g, 0.0, 1.0) * 255.0);
    let b2 = u32(clamp(b, 0.0, 1.0) * 255.0);
    return (r2 << 24u) | (g2 << 16u) | (b2 << 8u) | 153u;
}

@compute @workgroup_size(128)
fn main(@builtin(local_invocation_id) lid: vec3<u32>) {
    let key = lid.x;
    let tick = params.tick;
    let b0 = key_offsets[key];
    let b1 = key_offsets[key + 1u];

    // 上界：首个 start > tick 的位置（覆盖者必在其左侧）。
    var lo = b0;
    var hi = b1;
    while lo < hi {
        let mid = (lo + hi) / 2u;
        if note_start(notes[sort_index[mid]]) <= tick {
            lo = mid + 1u;
        } else {
            hi = mid;
        }
    }
    // 自上而下回溯：首个 end > tick 即最大 start 覆盖者（同 start 内 load 序最后）。
    var color = 0u;
    var i = hi;
    while i > b0 {
        i -= 1u;
        let n = notes[sort_index[i]];
        if note_end(n) > tick {
            color = active_color_for(n.key_color);
            break;
        }
    }
    active_colors[key] = color;
}
