// waterfall.wgsl — 瀑布流模式全 GPU 帧渲染计算着色器
//
// 每个线程处理一个像素，直接从权威音符缓冲读取数据，
// 写入 storage texture，实现零 CPU 参与的帧渲染。
//
// 音符存储是全管线唯一的 `NoteInstance` 常驻缓冲（与钢琴卷帘/走带同源同缓冲，
// 由调用方绑定，`binding(1)` 不再是瀑布流自有拷贝）：
//   - `key` 取自 `key_color` 低 8 位，`start/end` 由 `start_length` 还原，
//   - 颜色取自 `key_color` 高 24 位 RGB（alpha 输出固定 200，与旧行为一致）。
//
// dispatch: (ceil(w/16), ceil(h/16), 1)
// workgroup_size: (16, 16, 1)
//
// 性能设计（10W+ 密集音符优化）：
// 音符按 (key, start_tick) 升序排列，通过 key_offsets 分桶。
// 每个像素先 O(1) 定位所在 key 的桶 [offsets[key], offsets[key+1])，
// 再桶内二分定位 start_tick <= pixel_tick 的上界，最后从该位置
// 向前回溯（最大回溯 SEARCH_BUFFER），命中即 break。
// 复杂度：O(N×P) → O(P × (log(N/K) + SEARCH_BUFFER))，避免 GPU 内存带宽饥饿。

// 桶内二分后向前回溯的最大音符数。
// 高密集度段落（黑 MIDI / 密集和弦）中单 key 桶可能包含视口内海量音符，
// 若不加限制，间隙像素会回溯整个桶（O(桶大小)×像素），渲染速度断崖式下降。
// 限制回溯窗口后，密集段每像素从 O(桶大小) 降至 O(SEARCH_BUFFER)。
// 权衡：超长音符若被 SEARCH_BUFFER 个以上的同 key 音符遮挡，其露出尾部可能丢失，
// 但被遮挡部分视觉上本就不可见，影响可忽略。
const SEARCH_BUFFER: u32 = 128u;

// ── 数据结构 ──

struct WaterfallUniform {
    tick: u32,
    ppq: u32,
    key_count: u32,
    frame_width: u32,
    frame_height: u32,
    kb_height: u32,
    speed: f32,
    _padding: u32,
}

struct WaterfallNote {
    start_length: vec2<f32>,  // [start_tick, length_tick]（与 NoteInstance 同布局）
    key_color: u32,           // 低8位=key，高24位=RGB
    border_width: u32,        // 本 shader 忽略（钢琴卷帘矩形管线使用）
}

// ── Bindings ──

@group(0) @binding(0) var<uniform> params: WaterfallUniform;
@group(0) @binding(1) var<storage, read> notes: array<WaterfallNote>;
@group(0) @binding(2) var<storage, read> active_key_colors: array<u32>;
@group(0) @binding(3) var output_tex: texture_storage_2d<rgba8unorm, write>;
@group(0) @binding(4) var<storage, read> key_offsets: array<u32>;

// ── 工具函数 ──

fn is_black_key(key: u32) -> bool {
    let k = key % 12;
    return k == 1 || k == 3 || k == 6 || k == 8 || k == 10;
}

// 颜色解包与 miditrail_3d.wgsl 完全一致：`pack_color` 打包为 0xRRGGBBAA，
// 按 (r, g, b, a) 解包，保证瀑布流活跃键与 MIDITrail 使用相同的调色板颜色处理逻辑。
fn unpack_color(packed: u32) -> vec4<u32> {
    let r = (packed >> 24u) & 0xFFu;
    let g = (packed >> 16u) & 0xFFu;
    let b = (packed >> 8u) & 0xFFu;
    let a = packed & 0xFFu;
    return vec4<u32>(r, g, b, a);
}

// 权威 NoteInstance 解码（与 note.wgsl / cull.wgsl 的 key_color 语义一致）。
fn note_key(n: WaterfallNote) -> u32 {
    return n.key_color & 0xFFu;
}

fn note_start(n: WaterfallNote) -> u32 {
    return u32(max(n.start_length.x, 0.0));
}

fn note_end(n: WaterfallNote) -> u32 {
    return note_start(n) + u32(max(n.start_length.y, 1.0));
}

fn unpack_key_rgb(packed: u32) -> vec3<u32> {
    let rgb = packed >> 8u;
    return vec3<u32>((rgb >> 16u) & 0xFFu, (rgb >> 8u) & 0xFFu, rgb & 0xFFu);
}

// 通道顺序语义为 (r, g, b, a)，与 unpack_color 保持一致。
fn blend_key_color(base: vec4<u32>, overlay: vec4<u32>, alpha: u32) -> vec4<u32> {
    if overlay.a == 0u || alpha == 0u {
        return base;
    }
    let a = alpha;
    let r = (base.x * (255u - a) + overlay.x * a) / 255u;
    let g = (base.y * (255u - a) + overlay.y * a) / 255u;
    let b = (base.z * (255u - a) + overlay.z * a) / 255u;
    return vec4<u32>(r, g, b, 255u);
}

// 第一参数为 R 通道（低 8 位），对应 textureStore 输出的 R 分量。
fn pack_u32(r: u32, g: u32, b: u32, a: u32) -> u32 {
    return r | (g << 8u) | (b << 16u) | (a << 24u);
}

// ── 主函数 ──

@compute @workgroup_size(16, 16, 1)
fn main(@builtin(global_invocation_id) global_id: vec3<u32>) {
    let x = global_id.x;
    let y = global_id.y;
    let w = params.frame_width;
    let h = params.frame_height;

    if x >= w || y >= h {
        return;
    }

    let kb_h = params.kb_height;
    let note_area_h = h - min(h, kb_h);
    let ppq = params.ppq;
    let tick = params.tick;
    let speed = max(params.speed, 0.1);
    let key_count = params.key_count;

    // 计算瀑布流可见 tick 范围
    let ticks_per_measure = ppq * 4u;
    let visible_measure_count = max(u32(round(4.0 / speed)), 1u);
    let viewport_tick_span = max(ticks_per_measure * visible_measure_count, 1u);
    let tick_end = tick + viewport_tick_span;
    let zoom_x = f32(w) / f32(key_count);
    let zoom_y = f32(note_area_h) / f32(viewport_tick_span);
    // 预计算倒数，避免每像素除法（pixel_tick 换算用）
    let inv_zoom_y = 1.0 / max(zoom_y, 1e-6);

    // 默认黑色背景
    var pixel: u32 = pack_u32(0u, 0u, 0u, 255u);

    if y < note_area_h {
        // ── 音符区域 ──
        // 像素 → key 列（O(1) 分桶定位）。
        // 列 k 覆盖像素区间 [u32(k*z), u32(k*z)+u32(ceil(z)))，
        // 不能简单用 floor(x/z)（浮点截断在列边界会偏一个 key，
        // 如 z=21.8 时 x=43 属于列 2，但 43/21.8=1.97 截断为 1）。
        // 方案：主键 k0 = floor(x/z)，若 x 越过 k0 列右边界则取 k0+1。
        let z = zoom_x;
        let k0 = min(u32(f32(x) / z), key_count - 1u);
        let col_left = u32(f32(k0) * z);
        let col_right = col_left + u32(ceil(z));
        var key = k0;
        if x >= col_right && k0 + 1u < key_count {
            key = k0 + 1u;
        }
        let bucket_start = key_offsets[key];
        let bucket_end = key_offsets[key + 1u];
        let bucket_len = bucket_end - bucket_start;

        // 像素 y 对应的 tick 位置（桶内二分定位上界用）
        let pixel_tick = tick_end - u32(f32(y) * inv_zoom_y);

        if bucket_len > 0u {
            // 桶内二分：第一个 start_tick > pixel_tick 的位置（上界）。
            // 只有 start_tick <= pixel_tick 的音符才可能覆盖该像素，
            // 因此只需扫描 [bucket_start, hi) 区间。
            var lo = bucket_start;
            var hi = bucket_end;
            while lo < hi {
                let mid = (lo + hi) / 2u;
                if note_start(notes[mid]) <= pixel_tick {
                    lo = mid + 1u;
                } else {
                    hi = mid;
                }
            }
            // 回溯扫描 [bucket_start, hi)：候选音符按 start_tick 升序，
            // 命中即 break。与原实现一致的矩形判定（含 u32 边界保护），
            // 视觉结果完全一致；遍历范围从全量 N 缩小到桶内候选，
            // 并限制在 SEARCH_BUFFER 窗口内，防止密集段全桶回溯。
            var i = hi;
            var scanned: u32 = 0u;
            while i > bucket_start && scanned < SEARCH_BUFFER {
                i -= 1u;
                scanned += 1u;
                let n = notes[i];

                // 将音符的 tick 范围裁剪到视口内，避免 u32 下溢
                let visible_end = min(note_end(n), tick_end);
                let visible_start = max(note_start(n), tick);
                if visible_end <= visible_start {
                    continue;
                }

                let note_x = u32(f32(note_key(n)) * zoom_x);
                let note_w = u32(ceil(zoom_x));
                let note_top = u32(f32(tick_end - visible_end) * zoom_y);
                // 将 note_bottom 限制在 note_area_h 内，防止浮点精度导致音符被裁切
                let note_bottom = min(u32(f32(tick_end - visible_start) * zoom_y), note_area_h);
                let note_h = max(note_bottom - note_top, 1u);

                if x >= note_x && x < note_x + note_w && y >= note_top && y < note_top + note_h {
                    let c = unpack_key_rgb(n.key_color);
                    pixel = pack_u32(c.x, c.y, c.z, 200u);
                    break;
                }
            }
        }
    } else {
        // ── 键盘区域 ──
        // 采用 Nezha 风格的两阶段渲染：
        // 阶段1：确定像素属于哪个白键
        // 阶段2：若在键盘上半部分，检查是否被黑键覆盖
        // 这样黑键下方始终露出白键，符合真实钢琴键盘外观

        let kb_y = note_area_h;
        let local_y = y - kb_y;
        let black_kb_h = u32(f32(kb_h) * 0.6);

        // 先计算白键数量
        var total_white: u32 = 0u;
        for (var k: u32 = 0u; k < key_count; k++) {
            if !is_black_key(k) {
                total_white++;
            }
        }
        if total_white == 0u {
            total_white = 1u;
        }

        let white_w = f32(w) / f32(total_white);
        let black_w = white_w * 0.65;
        let black_w_offset = black_w * 0.5;

        // 阶段1：确定当前像素属于哪个白键
        var white_key_idx: u32 = 0u;
        var wc: u32 = 0u;
        var white_found: bool = false;
        for (var k: u32 = 0u; k < key_count && !white_found; k++) {
            if !is_black_key(k) {
                let kx_f = f32(wc) * white_w;
                let kw_f = white_w;
                if f32(x) >= kx_f && f32(x) < kx_f + kw_f {
                    white_key_idx = k;
                    white_found = true;
                }
                wc++;
            }
        }

        // 阶段2：若在键盘上半部分，检查是否在黑键区域内
        var in_black_area: bool = false;
        var black_key_idx: u32 = 0u;
        if local_y < black_kb_h {
            wc = 0u;
            for (var k: u32 = 0u; k < key_count && !in_black_area; k++) {
                if is_black_key(k) {
                    if wc > 0u {
                        let boundary_x = f32(wc) * white_w;
                        let kx_f = boundary_x - black_w_offset;
                        let kw_f = black_w;
                        if f32(x) >= kx_f && f32(x) < kx_f + kw_f {
                            black_key_idx = k;
                            in_black_area = true;
                        }
                    }
                } else {
                    wc++;
                }
            }
        }

        // 阶段3：渲染
        if white_found {
            if in_black_area {
                // 黑键覆盖区域：使用黑键颜色和黑键的活跃色
                let active_color_packed = active_key_colors[black_key_idx];
                let active_color = unpack_color(active_color_packed);
                let base = vec4<u32>(41u, 41u, 42u, 255u);
                let blended = blend_key_color(base, active_color, 153u);
                pixel = pack_u32(blended.x, blended.y, blended.z, 255u);
            } else {
                // 白键区域（含黑键下方露出的白键部分）
                let active_color_packed = active_key_colors[white_key_idx];
                let active_color = unpack_color(active_color_packed);
                let base = vec4<u32>(235u, 235u, 235u, 255u);
                let blended = blend_key_color(base, active_color, 153u);
                pixel = pack_u32(blended.x, blended.y, blended.z, 255u);
            }
        } else {
            // 未找到任何键（边界保护）：深色背景
            pixel = pack_u32(30u, 30u, 30u, 255u);
        }
    }

    textureStore(output_tex, vec2<i32>(i32(x), i32(y)), vec4<f32>(
        f32(pixel & 0xFFu) / 255.0,
        f32((pixel >> 8u) & 0xFFu) / 255.0,
        f32((pixel >> 16u) & 0xFFu) / 255.0,
        f32((pixel >> 24u) & 0xFFu) / 255.0,
    ));
}
