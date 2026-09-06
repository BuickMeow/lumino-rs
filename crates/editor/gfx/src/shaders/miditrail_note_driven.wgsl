//! GPU-Driven 音符管线（Normal 视图）：位姿全部由 vertex shader 计算。
//!
//! CPU 每帧只上传窗口内 `NoteInstance` 原字节（16B/音符，零换算、零排序），
//! translation/scale/color 的推导（与 `instances.rs::build_note_instances`
//! 逐 op 对应，运算顺序刻意保持一致）搬进 `vs_main`。顺序由深度测试解决
//! （不透明管线，`depth_write=true`），CPU 画家排序彻底删除。
//! 琴键仍走旧管线最后绘制（depth compare Always，永远置顶，观感不变）。

struct Camera {
    view_proj: mat4x4<f32>,
    light_dir: vec3<f32>,
    ambient: f32,
}

@group(0) @binding(0) var<uniform> camera: Camera;

struct DrivenParams {
    tick: u32,
    viewport_tick_span: f32,
    scene_depth: f32,
    note_z_offset: f32,
    z_far: f32,
    note_height: f32,
    note_y: f32,
    key_count: u32,
    // [left, width]（z/w 保留），key 越界时 shader 取零向量（与 CPU `key >= len`
    // 跳过等价，零宽实例退化为零面积三角形被光栅器剔除）。
    // NOTE：uniform 地址空间数组步长须 16 对齐，故用 vec4（vec2 stride=8 非法）。
    key_table: array<vec4<f32>, 128>,
}

@group(1) @binding(0) var<uniform> params: DrivenParams;

struct DrivenInput {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    // 与 `NoteInstance` 字节布局一致：[start, length] + key_color + border。
    @location(2) start_length: vec2<f32>,
    @location(3) key_color: u32,
    @location(4) border: u32,
}

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) world_pos: vec3<f32>,
    @location(1) color: vec3<f32>,
    @location(2) normal: vec3<f32>,
}

@vertex
fn vs_main(in: DrivenInput) -> VertexOutput {
    // key_color：低 8 位 = key，高 24 位 = RGB（与 `pack_key_color` 一致）。
    let key = in.key_color & 0xFFu;
    let rgb = in.key_color >> 8u;
    var r = f32((rgb >> 16u) & 0xFFu) / 255.0;
    var g = f32((rgb >> 8u) & 0xFFu) / 255.0;
    var b = f32(rgb & 0xFFu) / 255.0;

    let tick_f = f32(params.tick);
    let start = in.start_length.x;
    // CPU `pack` 保证 length >= 1.0，此处再钳一次防脏数据（与 CPU
    // `n.start_length[1].max(1.0)` 同语义）。
    let end = start + max(in.start_length.y, 1.0);

    // 与 CPU `build_note_instances` 同式的可见区间与深度映射
    // （运算顺序刻意保持一致，减小 CPU/GPU 浮点差异）。
    let visible_start = max(start, tick_f);
    let z_start = params.note_z_offset
        - ((visible_start - tick_f) / params.viewport_tick_span * params.scene_depth);
    var z_end = params.note_z_offset
        - ((end - tick_f) / params.viewport_tick_span * params.scene_depth);
    z_end = max(z_end, params.z_far);

    var scale = vec3(0.0);
    var translation = vec3(0.0);
    // z_end >= z_start 的实例退化为零体积（光栅器自动剔除）；
    // collect 已保证 end > tick，正常帧此处恒为 false，仅防脏数据。
    if (z_end < z_start && key < 128u) {
        let kw = params.key_table[key];
        let width = kw.y;
        let z_center = (z_start + z_end) * 0.5;
        let z_length = z_start - z_end;
        scale = vec3(width * 0.92, params.note_height, z_length);
        translation = vec3(
            kw.x + width * 0.04,
            params.note_y,
            z_center - z_length * 0.5,
        );
        // 激活提亮（与 CPU `boost_color_packed(c, 0.5)` 同值；shader 直接输出
        // float，比 CPU 的 u8 中转精度更高，只会更准不会更差）。
        if (start <= tick_f && tick_f < end) {
            r = min(r + 0.5, 1.0);
            g = min(g + 0.5, 1.0);
            b = min(b + 0.5, 1.0);
        }
    }

    let model = mat4x4<f32>(
        vec4(scale.x, 0.0, 0.0, 0.0),
        vec4(0.0, scale.y, 0.0, 0.0),
        vec4(0.0, 0.0, scale.z, 0.0),
        vec4(translation, 1.0),
    );
    let world_pos = (model * vec4(in.position, 1.0)).xyz;
    let world_normal = (model * vec4(in.normal, 0.0)).xyz;

    var out: VertexOutput;
    out.clip_position = camera.view_proj * vec4(world_pos, 1.0);
    out.world_pos = world_pos;
    out.normal = world_normal;
    out.color = vec3(r, g, b);
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    // 音符保持平面颜色（与旧管线 is_key=0 路径一致）。
    return vec4(in.color, 1.0);
}
