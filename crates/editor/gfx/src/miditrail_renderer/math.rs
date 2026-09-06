//! Miditrail 3D 渲染器数学工具
//!
//! 列主序 4x4 矩阵与向量工具，避免引入额外依赖。

use super::types::{MiditrailCameraGpu, MiditrailViewMode};

/// 构建相机 Uniform（投影 * 视图），按视图模式分支。
pub fn build_camera_uniform(
    width: u32,
    height: u32,
    view_mode: MiditrailViewMode,
    z_far_distance: f32,
) -> MiditrailCameraGpu {
    match view_mode {
        MiditrailViewMode::Top => build_top_camera_uniform(width, height, z_far_distance),
        MiditrailViewMode::Normal => build_normal_camera_uniform(width, height),
    }
}

/// Normal 普通视图相机（原有 3D 斜视实现迁移而来，行为不变）。
fn build_normal_camera_uniform(width: u32, height: u32) -> MiditrailCameraGpu {
    let aspect = if height == 0 {
        1.0
    } else {
        width as f32 / height as f32
    };
    // 参考 Comet 默认 MIDITrail 设置：
    // cameraPos = (0.5, 0.466, 0.385)，pitch = 30.798°，FOV = 59.327°
    // 从斜上方俯视键盘，使键盘位于画面底部，音符向远方延伸。
    let eye = [0.5f32, 0.466, 0.385];
    let pitch = 30.798f32.to_radians();
    let forward = [0.0f32, -pitch.sin(), -pitch.cos()];
    let center = [
        eye[0] + forward[0],
        eye[1] + forward[1],
        eye[2] + forward[2],
    ];
    let up = [0.0f32, 1.0, 0.0];
    let fov_deg = 59.327f32;
    let fov = fov_deg.to_radians();
    let near = 0.1f32;
    let far = 100.0f32;
    let view = look_at_rh(eye, center, up);
    let proj = perspective_rh(fov, aspect, near, far);
    let view_proj = mat4_mul(proj, view);
    let light_dir = normalize([0.3, 0.8, -0.5]);
    MiditrailCameraGpu {
        view_proj,
        light_dir,
        ambient: 0.4,
    }
}

/// Top 顶部视图俯视相机（参考 Comet MIDITrail `Top Down Above` 预设）。
///
/// 预设原文：FOV 38.354，相机位置 (2.492, 8.105, -4.853)，
/// 旋转 (90°, -90°, 0°)（俯仰 90° 直视下方 + 偏航 -90°）。
/// 换算到本实现的 `look_at` 表达：
/// - 视线：正下方 `(0,-1,0)`；
/// - 屏幕上 = 世界 `-X`（低键在上，高键在下）；
/// - 屏幕右 = 世界 `-Z`（键盘在左竖条，音符向右延伸，与附图一致）。
///
/// Z 向取景框按实际显示距离居中；窄高比下按需抬高相机，
/// 保证键盘与显示距离末端整体入画（切换视图不丢状态）。
fn build_top_camera_uniform(width: u32, height: u32, z_far_distance: f32) -> MiditrailCameraGpu {
    const TOP_EYE_X: f32 = 2.492;
    const TOP_BASE_HEIGHT: f32 = 8.105;
    const TOP_FOV_DEG: f32 = 38.354;
    const TOP_KEY_NEAR_Z: f32 = 0.25;

    let aspect = if height == 0 {
        1.0
    } else {
        width as f32 / height as f32
    };
    let z_far = z_far_distance.max(0.1);
    let z_end = 0.012 - z_far;
    let z_center = (TOP_KEY_NEAR_Z + z_end) * 0.5;
    let z_half = ((TOP_KEY_NEAR_Z - z_end) * 0.5 + 0.6).max(0.5);
    let half_h_tan = (TOP_FOV_DEG.to_radians() * 0.5).tan().max(1e-3);
    // 水平半视野 = 高度 × half_h_tan × 高宽比 ≥ z_half → 反解最小高度。
    let eye_y = TOP_BASE_HEIGHT.max(z_half / (half_h_tan * aspect.max(0.1)));

    let eye = [TOP_EYE_X, eye_y, z_center];
    let center = [TOP_EYE_X, 0.0, z_center];
    let up = [-1.0f32, 0.0, 0.0];
    let view = look_at_rh(eye, center, up);
    let proj = perspective_rh(TOP_FOV_DEG.to_radians(), aspect, 0.1, 100.0);
    MiditrailCameraGpu {
        view_proj: mat4_mul(proj, view),
        light_dir: normalize([0.3, 0.8, -0.5]),
        // Top 走 flat 着色（见 miditrail_top.wgsl），环境光拉满避免压暗。
        ambient: 1.0,
    }
}

pub fn look_at_rh(eye: [f32; 3], center: [f32; 3], up: [f32; 3]) -> [[f32; 4]; 4] {
    let f = normalize([center[0] - eye[0], center[1] - eye[1], center[2] - eye[2]]);
    let s = normalize(cross(f, up));
    let u = cross(s, f);

    // 标准 OpenGL/WGPU 列主序 view 矩阵
    [
        [s[0], u[0], -f[0], 0.0],
        [s[1], u[1], -f[1], 0.0],
        [s[2], u[2], -f[2], 0.0],
        [-dot(s, eye), -dot(u, eye), dot(f, eye), 1.0],
    ]
}

pub fn perspective_rh(fov_y: f32, aspect: f32, near: f32, far: f32) -> [[f32; 4]; 4] {
    let f = 1.0 / (fov_y * 0.5).tan();
    let nf = 1.0 / (near - far);
    // wgpu / WebGPU 的 NDC z 范围是 [0, 1]，因此使用 Vulkan 风格投影矩阵
    [
        [f / aspect, 0.0, 0.0, 0.0],
        [0.0, f, 0.0, 0.0],
        [0.0, 0.0, far * nf, -1.0],
        [0.0, 0.0, far * near * nf, 0.0],
    ]
}

pub fn mat4_mul(a: [[f32; 4]; 4], b: [[f32; 4]; 4]) -> [[f32; 4]; 4] {
    let mut r = [[0.0f32; 4]; 4];
    for col in 0..4 {
        for row in 0..4 {
            let mut sum = 0.0f32;
            for k in 0..4 {
                sum += a[k][row] * b[col][k];
            }
            r[col][row] = sum;
        }
    }
    r
}

pub fn normalize(v: [f32; 3]) -> [f32; 3] {
    let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if len < 1e-6 {
        return [0.0, 1.0, 0.0];
    }
    [v[0] / len, v[1] / len, v[2] / len]
}

pub fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

pub fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_matrix_utilities() {
        let eye = [0.5f32, 0.5, 0.5];
        let center = [0.5f32, 0.0, -0.5];
        let up = [0.0f32, 1.0, 0.0];
        let view = look_at_rh(eye, center, up);
        let proj = perspective_rh(std::f32::consts::FRAC_PI_4, 16.0 / 9.0, 0.01, 100.0);
        let _vp = mat4_mul(proj, view);
        assert!((dot([1.0, 0.0, 0.0], [0.0, 1.0, 0.0]) - 0.0).abs() < 1e-6);
    }

    #[test]
    fn test_camera_uniform_size() {
        let cam = build_camera_uniform(1920, 1080, MiditrailViewMode::Normal, 7.5);
        assert!(cam.light_dir[0].is_finite());
    }

    /// 列主序 view_proj 变换世界点到 NDC（透视除法后）。
    fn project_to_ndc(view_proj: [[f32; 4]; 4], point: [f32; 3]) -> [f32; 3] {
        let v = [point[0], point[1], point[2], 1.0];
        let mut clip = [0.0f32; 4];
        for row in 0..4 {
            let mut sum = 0.0f32;
            for col in 0..4 {
                sum += view_proj[col][row] * v[col];
            }
            clip[row] = sum;
        }
        assert!(clip[3] > 1e-6, "投影后 w 应为正");
        [clip[0] / clip[3], clip[1] / clip[3], clip[2] / clip[3]]
    }

    #[test]
    fn test_top_camera_layout_matches_reference() {
        // Top 布局（与附图一致）：键盘在左竖条（NDC x < 0），音符向右延伸；
        // 低键在上（NDC y 大），高键在下。
        let cam = build_camera_uniform(1920, 1080, MiditrailViewMode::Top, 7.5);
        let key = project_to_ndc(cam.view_proj, [0.5, 0.006, 0.03]);
        let far_note = project_to_ndc(cam.view_proj, [0.5, 0.004, -5.0]);
        let low_key = project_to_ndc(cam.view_proj, [0.02, 0.006, 0.03]);
        let high_key = project_to_ndc(cam.view_proj, [0.98, 0.006, 0.03]);
        assert!(key[0] < 0.0, "键盘应在左半屏，实际 {}", key[0]);
        assert!(
            far_note[0] > key[0],
            "远端音符应在键盘右侧：{} > {}",
            far_note[0],
            key[0]
        );
        assert!(
            low_key[1] > high_key[1],
            "低键应在高键上方：{} > {}",
            low_key[1],
            high_key[1]
        );
    }

    #[test]
    fn test_top_camera_frames_keyboard_and_z_far() {
        // 横竖屏下键盘与显示距离末端均应入画（切换视图不丢状态）。
        for (w, h) in [(1920u32, 1080u32), (1080, 1080), (1080, 1920)] {
            let cam = build_camera_uniform(w, h, MiditrailViewMode::Top, 7.5);
            let key = project_to_ndc(cam.view_proj, [0.5, 0.006, 0.03]);
            let far = project_to_ndc(cam.view_proj, [0.5, 0.004, 0.012 - 7.5]);
            for (label, ndc) in [("键盘", key), ("远端", far)] {
                assert!(
                    (-1.0..=1.0).contains(&ndc[0]) && (-1.0..=1.0).contains(&ndc[1]),
                    "{label}在 {w}x{h} 下应入画，实际 {ndc:?}"
                );
            }
        }
    }

    #[test]
    fn test_top_and_normal_cameras_differ() {
        let normal = build_camera_uniform(1920, 1080, MiditrailViewMode::Normal, 7.5);
        let top = build_camera_uniform(1920, 1080, MiditrailViewMode::Top, 7.5);
        assert_ne!(normal.view_proj, top.view_proj, "两视图相机必须不同");
        assert!(
            (top.ambient - 1.0).abs() < 1e-6,
            "Top 走 flat 着色，环境光应拉满"
        );
    }
}
