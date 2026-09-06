//! Top 顶部视图渲染管线（Comet flat 绘制风格）
//!
//! 与 `miditrail_3d.wgsl` 共用同一实例缓冲布局（translation/scale/color/
//! is_key/press_factor/press_depth），但片元着色极简：
//! - 俯视下可见面全是顶面，光照为常量 → 直接输出平面颜色，
//!   省掉逐片元 `normalize` + 按面分支（Comet `noteShaderFrag` 同理，
//!   激活提亮由 CPU 端 `boost_color_packed` +0.5 完成）；
//! - 琴键按下位移仍在顶点着色保留（与 Normal 共用实例数据，零第二份显存）。

struct Camera {
    view_proj: mat4x4<f32>,
    light_dir: vec3<f32>,
    ambient: f32,
}

@group(0) @binding(0) var<uniform> camera: Camera;

struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) translation: vec3<f32>,
    @location(3) scale: vec3<f32>,
    @location(4) color: u32,
    @location(5) is_key: u32,
    @location(6) press_factor: f32,
    @location(7) press_depth: f32,
}

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) color: vec3<f32>,
}

fn unpack_color(packed: u32) -> vec4<f32> {
    let r = f32((packed >> 24u) & 0xFFu) / 255.0;
    let g = f32((packed >> 16u) & 0xFFu) / 255.0;
    let b = f32((packed >> 8u) & 0xFFu) / 255.0;
    let a = f32(packed & 0xFFu) / 255.0;
    return vec4(r, g, b, a);
}

@vertex
fn vs_main(in: VertexInput) -> VertexOutput {
    var translation = in.translation;
    if (in.is_key == 1u) {
        translation.y -= in.press_factor * in.press_depth;
    }

    let model = mat4x4<f32>(
        vec4(in.scale.x, 0.0, 0.0, 0.0),
        vec4(0.0, in.scale.y, 0.0, 0.0),
        vec4(0.0, 0.0, in.scale.z, 0.0),
        vec4(translation, 1.0),
    );
    let world_pos = (model * vec4(in.position, 1.0)).xyz;

    var out: VertexOutput;
    out.clip_position = camera.view_proj * vec4(world_pos, 1.0);
    out.color = unpack_color(in.color).rgb;
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    // Flat 输出：无光照、无按面分支（Comet 风格平面色）。
    return vec4(in.color, 1.0);
}
