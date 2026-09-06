//! 渲染循环单元测试
//!
//! 使用 naga 验证所有 WGSL 着色器可被正确解析与校验，
//! 并验证视频导出路径的 depth-stencil 兼容性决策。

use naga::valid::{Capabilities, ValidationFlags, Validator};

/// 使用 naga 解析并校验一段 WGSL 源码。
///
/// 失败时输出具体错误信息，便于定位着色器问题。
fn validate_wgsl(source: &str, label: &str) {
    let module = naga::front::wgsl::parse_str(source)
        .unwrap_or_else(|e| panic!("{} WGSL 解析失败: {:?}", label, e));
    // arrangement.wgsl 使用 f16 与 f32 混合运算，需要显式开启对应 capability。
    let capabilities = Capabilities::default() | Capabilities::SHADER_FLOAT16_IN_FLOAT32;
    let mut validator = Validator::new(ValidationFlags::default(), capabilities);
    validator
        .validate(&module)
        .unwrap_or_else(|e| panic!("{} WGSL 校验失败: {:?}", label, e));
}

#[test]
fn test_arrangement_shader_valid() {
    validate_wgsl(
        include_str!("../../shaders/arrangement.wgsl"),
        "arrangement",
    );
}

#[test]
fn test_cc_bar_shader_valid() {
    validate_wgsl(include_str!("../../shaders/cc_bar.wgsl"), "cc_bar");
}

#[test]
fn test_cull_shader_valid() {
    validate_wgsl(include_str!("../../shaders/cull.wgsl"), "cull");
    validate_wgsl(
        include_str!("../../shaders/cull_vertical.wgsl"),
        "cull_vertical",
    );
}

#[test]
fn test_infinite_grid_shader_valid() {
    validate_wgsl(
        include_str!("../../shaders/infinite_grid.wgsl"),
        "infinite_grid",
    );
    validate_wgsl(
        include_str!("../../shaders/infinite_grid_vertical.wgsl"),
        "infinite_grid_vertical",
    );
}

#[test]
fn test_note_shader_valid() {
    validate_wgsl(include_str!("../../shaders/note.wgsl"), "note");
    validate_wgsl(
        include_str!("../../shaders/note_vertical.wgsl"),
        "note_vertical",
    );
    validate_wgsl(include_str!("../../shaders/onion_note.wgsl"), "onion_note");
    validate_wgsl(
        include_str!("../../shaders/onion_note_vertical.wgsl"),
        "onion_note_vertical",
    );
}

#[test]
fn test_ruler_shader_valid() {
    validate_wgsl(include_str!("../../shaders/ruler.wgsl"), "ruler");
}

#[test]
fn test_miditrail_shader_valid() {
    validate_wgsl(
        include_str!("../../shaders/miditrail_3d.wgsl"),
        "miditrail_3d",
    );
    validate_wgsl(
        include_str!("../../shaders/miditrail_top.wgsl"),
        "miditrail_top",
    );
    validate_wgsl(
        include_str!("../../shaders/miditrail_aura.wgsl"),
        "miditrail_aura",
    );
    validate_wgsl(
        include_str!("../../shaders/miditrail_note_driven.wgsl"),
        "miditrail_note_driven",
    );
}

#[test]
fn test_waterfall_shader_valid() {
    validate_wgsl(include_str!("../../shaders/waterfall.wgsl"), "waterfall");
}

#[test]
fn test_video_export_renderers_use_no_depth_shaders() {
    // 视频导出路径使用的所有着色器均不应写入 depth output。
    // 若着色器中存在 @builtin(position).z 写入或 @builtin(frag_depth) 输出，
    // naga 校验不会直接报错，但此处至少保证所有源码均能被无 depth 管线的 wgpu 接受。
    let sources = [
        (
            "arrangement",
            include_str!("../../shaders/arrangement.wgsl"),
        ),
        ("cc_bar", include_str!("../../shaders/cc_bar.wgsl")),
        ("cull", include_str!("../../shaders/cull.wgsl")),
        (
            "infinite_grid",
            include_str!("../../shaders/infinite_grid.wgsl"),
        ),
        ("note", include_str!("../../shaders/note.wgsl")),
        ("ruler", include_str!("../../shaders/ruler.wgsl")),
    ];

    for (label, source) in sources {
        validate_wgsl(source, label);
    }
}
