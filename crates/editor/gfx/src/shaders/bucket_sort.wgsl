// bucket_sort.wgsl — 全局桶一次性 GPU 基数排序（LSD，分块稳定版，纯 u32）
//
// 背景：洋葱皮常驻缓冲为轨追加序；瀑布流 / miditrail 需要 (key, start) 有序。
// 约束：不动 CPU 排布、不二次上传音符字节、不移动常驻字节（洋葱皮段表依赖原址）。
// 方案：load 后（或首次导出前）执行一次本排序，产物为置换索引 sort_index[N] +
// 全局 key_offsets[257]，常驻复用；排序键 = (key, start)，与 waterfall.wgsl 的
// note_key / note_start 公式逐字一致。
//
// 两阶段 LSD（低位先排，稳定性逐 pass 传递）：
//   pass 0..3：按 start_tick 的 4 个字节（shift = 0/8/16/24）；
//   pass 4：按 key（8 位）→ 终态 key 主序、start 次序。
// 每个 pass = tile_hist → prefix_tiles → scatter_stable 三个 dispatch：
// - tile_hist：每 workgroup 处理 TILE 个元素，块内共享直方图 → tile_buf；
// - prefix_tiles：单 workgroup 对 tile 做互斥前缀和（就地，digit 间无交集）；
// - scatter_stable：同分块二次遍历，块内按槽位顺序确定性排名，全局落点唯一。
// 全程确定性（无跨线程原子竞速决定顺序）：全相等键保持 load 原有相对顺序。
// 注意 legacy CPU sort_visible_notes 并列 tiebreak 为 track 降序，
// 此处为 load 顺序——差异由像素等价 harness 量化（集成任务验收）。

struct NoteInstance {
    start_length: vec2<f32>,
    key_color: u32,
    border_width: u32,
};

struct SortParams {
    count: u32,
    shift: u32,
    use_key: u32,    // 0 = 取 start 字节，1 = 取 key
    first_pass: u32, // 1 = 索引输入恒等（idx=i），跳过 indices_in 读取
};

// 每 workgroup 处理元素数（256 线程 × 8 项）。
const TILE: u32 = 2048u;
const ITEMS_PER_THREAD: u32 = 8u;
// scatter 子块：256 槽位顺序处理，共 8 轮（确定性排名，见 scatter_stable）。
const SUB: u32 = 256u;
const SUBS: u32 = 8u;
// 无效槽位哨兵（digit 取值恒 < 256，哨兵永不参与计数）。
const INVALID_DIGIT: u32 = 256u;

@group(0) @binding(0) var<storage, read> notes: array<NoteInstance>;
@group(0) @binding(1) var<storage, read> indices_in: array<u32>;
@group(0) @binding(2) var<storage, read_write> indices_out: array<u32>;
// tile 直方图 / tile 基址（prefix_tiles 就地改为基址，digit 间无交集故无竞争）。
@group(0) @binding(3) var<storage, read_write> tile_buf: array<u32>;
@group(0) @binding(4) var<uniform> params: SortParams;

fn note_key(n: NoteInstance) -> u32 {
    return n.key_color & 0xFFu;
}

fn note_start(n: NoteInstance) -> u32 {
    // 与 waterfall.wgsl note_start 逐字一致（含 max 钳负）。
    return u32(max(n.start_length.x, 0.0));
}

fn digit_of(n: NoteInstance) -> u32 {
    if params.use_key == 1u {
        return note_key(n);
    }
    return (note_start(n) >> params.shift) & 0xFFu;
}

fn source_idx(g: u32) -> u32 {
    if params.first_pass == 1u {
        return g;
    }
    return indices_in[g];
}

fn num_tiles() -> u32 {
    return (params.count + TILE - 1u) / TILE;
}

// ── pass 内阶段 1：分块直方图 ──
var<workgroup> hist_shared: array<atomic<u32>, 256>;

@compute @workgroup_size(256)
fn tile_hist(
    @builtin(workgroup_id) wid: vec3<u32>,
    @builtin(local_invocation_id) lid: vec3<u32>,
) {
    atomicStore(&hist_shared[lid.x], 0u);
    workgroupBarrier();

    let tile = wid.x;
    let base = tile * TILE + lid.x * ITEMS_PER_THREAD;
    for (var k = 0u; k < ITEMS_PER_THREAD; k++) {
        let g = base + k;
        if g < params.count {
            let d = digit_of(notes[source_idx(g)]);
            atomicAdd(&hist_shared[d], 1u);
        }
    }
    workgroupBarrier();
    tile_buf[tile * 256u + lid.x] = atomicLoad(&hist_shared[lid.x]);
}

// ── pass 内阶段 2：tile 基址 + 全局 digit 基址 ──
//
// tile_buf 就地两步走（单 workgroup，digit 间无交集）：
// 阶段 A：tile 间互斥前缀和 → tile_buf[t][d] = tile 偏移（此前 tile 之前同 digit 计数）；
// 阶段 B：digit 总数互斥前缀和 → 全局 digit 基址，加到各 tile 偏移上。
// 终态 tile_buf[t][d] = digit 基址 + tile 偏移 = 该 tile 该 digit 在有序输出中的起始位置。
// digit 总数暂存 key_hist（排序 pass 中该缓冲闲置；尾声归约前会被清零重写，无冲突）。
@compute @workgroup_size(256)
fn prefix_tiles(@builtin(local_invocation_id) lid: vec3<u32>) {
    let d = lid.x;
    let n = num_tiles();
    var acc = 0u;
    for (var t = 0u; t < n; t++) {
        let v = tile_buf[t * 256u + d];
        tile_buf[t * 256u + d] = acc;
        acc += v;
    }
    atomicStore(&key_hist[d], acc);
    workgroupBarrier();
    var base = 0u;
    for (var i = 0u; i < d; i++) {
        base += atomicLoad(&key_hist[i]);
    }
    for (var t = 0u; t < n; t++) {
        tile_buf[t * 256u + d] += base;
    }
}

// ── pass 内阶段 3：确定性稳定散射 ──
//
// 同分块二次遍历：tile 切 8 个 256 槽子块顺序处理；子块内槽位 t 的同 digit 排名 =
// 其左侧槽位同 digit 计数（共享内存扫描，顺序固定）；全局落点 =
// tile 基址 + 轮次累积 + 块内排名。落点是 (tile, 槽位, digit) 的纯函数，
// 与线程调度无关 → 真稳定。
var<workgroup> sub_dig: array<u32, 256>;
var<workgroup> run_base: array<atomic<u32>, 256>;
var<workgroup> sub_cnt: array<atomic<u32>, 256>;

@compute @workgroup_size(256)
fn scatter_stable(
    @builtin(workgroup_id) wid: vec3<u32>,
    @builtin(local_invocation_id) lid: vec3<u32>,
) {
    let tile = wid.x;
    let t = lid.x;
    let tile_base = tile * TILE;
    atomicStore(&run_base[t], tile_buf[tile * 256u + t]);
    workgroupBarrier();

    for (var s = 0u; s < SUBS; s++) {
        atomicStore(&sub_cnt[t], 0u);
        workgroupBarrier();

        let slot = s * SUB + t;
        let g = tile_base + slot;
        var idx = 0u;
        var d = INVALID_DIGIT;
        var valid = false;
        if g < params.count {
            idx = source_idx(g);
            d = digit_of(notes[idx]);
            valid = true;
        }
        sub_dig[t] = d;
        if valid {
            atomicAdd(&sub_cnt[d], 1u);
        }
        workgroupBarrier();

        if valid {
            // 块内排名：左侧槽位同 digit 计数（扫描顺序固定 → 确定性）。
            var rank = 0u;
            for (var j = 0u; j < t; j++) {
                if sub_dig[j] == d {
                    rank++;
                }
            }
            let dest = atomicLoad(&run_base[d]) + rank;
            // 防御性越界保护（算法正确时恒不触发；触发则丢该项而非越界写，
            // 单测的置换完整性断言会捕获此类 bug）。
            if dest < params.count {
                indices_out[dest] = idx;
            }
        }
        workgroupBarrier();

        // 本轮计数并入轮次基址（每线程认领一个 digit，无冲突）。
        let add = atomicLoad(&sub_cnt[t]);
        if add > 0u {
            atomicAdd(&run_base[t], add);
        }
        workgroupBarrier();
    }
}

// ── 构建尾声：tile 直方图按 key 归约（256 线程各扫全部 tile）──
//
// 用法：对有序结果跑 tile_hist(use_key=1) 后接本入口，输出 256 项 key 计数到
// key_hist（CPU 回读 1KB → 前缀和 → 全局 key_offsets）。
// 注意：key_hist 独占 binding 5（binding 2 已是 indices_out，WGSL 同一 binding
// 只允许一处资源声明）。
@group(0) @binding(5) var<storage, read_write> key_hist: array<atomic<u32>, 256>;

@compute @workgroup_size(256)
fn reduce_tiles(@builtin(local_invocation_id) lid: vec3<u32>) {
    let d = lid.x;
    let n = num_tiles();
    var acc = 0u;
    for (var t = 0u; t < n; t++) {
        acc += tile_buf[t * 256u + d];
    }
    atomicStore(&key_hist[d], acc);
}
