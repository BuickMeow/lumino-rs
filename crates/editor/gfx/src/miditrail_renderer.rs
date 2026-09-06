//! Miditrail 3D 视频导出渲染器（wgpu 渲染管线实现）
//!
//! 该渲染器以 3D 透视方式渲染 MIDI 键盘与音符轨迹，结果写入离屏纹理，
//! 再由导出管线读回 CPU 并编码为视频帧。

mod aura;
mod cull;
mod instances;
mod key_press;
mod math;
mod pipeline;
mod quantize;
mod render_pass;
mod textures;
mod types;

/// 重导出颜色打包工具（供视频导出 waterfall/miditrail 模式使用）
pub use instances::pack_color;
pub use types::{
    MiditrailAuraInstanceGpu, MiditrailCameraGpu, MiditrailDrivenParamsGpu, MiditrailInstanceGpu,
    MiditrailNoteGpu, MiditrailUniformGpu, MiditrailViewMode, miditrail_viewport_span,
};

use crate::NoteInstance;
use aura::{create_aura_buffers, create_aura_sampler, generate_aura_ring_data};
use instances::{
    ActiveKeys, NoteBuildScratch, build_aura_instances, build_key_instances, build_note_instances,
    compute_active_and_aura_for_compact, compute_active_keys, emit_aura_instances,
    update_key_positions,
};
use math::build_camera_uniform;
use pipeline::{
    create_aura_render_pipeline, create_bind_group_layout, create_buffers,
    create_driven_group_layout, create_note_driven_pipeline, create_note_render_pipeline,
    create_quad_index_buffer, create_render_pipeline, create_top_note_render_pipeline,
    create_top_render_pipeline,
};
use quantize::quantize_notes_for_top;

const KEY_PRESS_SPEED_DOWN: f32 = 15.0;
const KEY_PRESS_SPEED_UP: f32 = 10.0;
const AURA_TEXTURE_SIZE: u32 = 128;

/// 3D 场景深度（tick 到 Z 坐标的映射比例）。
pub const MIDITRAIL_SCENE_DEPTH: f32 = 7.5;
/// Z 方向显示距离默认值（与场景深度相同）。
pub const MIDITRAIL_DEFAULT_Z_FAR_DISTANCE: f32 = 7.5;
/// Z 方向显示距离最大值（也是音符收集范围的最大倍数）。
pub const MIDITRAIL_MAX_Z_FAR_DISTANCE: f32 = 15.0;

/// Top 键盘去按压用的全零系数（俯视只留颜色反馈；切回 Normal 时内部
/// `key_press_factors` 仍在更新，按压动画无缝衔接）。
static ZERO_PRESS_FACTORS: [f32; 128] = [0.0; 128];

/// 3D MIDITrail 渲染器
///
/// 使用实例化立方体渲染键盘与音符，结果写入 `Rgba8Unorm` 离屏纹理。
/// Normal 与 Top 视图共用实例缓冲/纹理/深度（零第二份显存），
/// 区别仅在于相机、音符精度（Top 逐音量化对齐、永不合并）、键盘反馈
/// （Top 无按压位移、只变色）与着色器（Top flat）。
pub struct MiditrailRenderer {
    render_pipeline: wgpu::RenderPipeline,
    note_pipeline: wgpu::RenderPipeline,
    top_render_pipeline: wgpu::RenderPipeline,
    top_note_pipeline: wgpu::RenderPipeline,
    /// GPU-Driven 音符管线（Normal 终局路径：compact 直传＋vertex 推导＋深度排序）。
    driven_note_pipeline: wgpu::RenderPipeline,
    /// Driven 参数组布局（group1：位姿参数＋键位表 uniform）。
    driven_group_layout: wgpu::BindGroupLayout,
    /// Driven 参数绑定（params 缓冲创建后初始化一次，缓冲句柄稳定）。
    driven_bind_group: Option<wgpu::BindGroup>,
    /// Driven 参数上传缓冲（约 1KB，每帧重写）。
    driven_params_buffer: crate::gpu_resource_tracker::TrackedBuffer,
    /// 紧凑音符上传缓冲（`NoteInstance` 原字节直传，16B/音符）。
    compact_buffer: Option<crate::gpu_resource_tracker::TrackedBuffer>,
    compact_capacity: usize,
    bind_group_layout: wgpu::BindGroupLayout,
    bind_group: Option<wgpu::BindGroup>,

    uniform_buffer: crate::gpu_resource_tracker::TrackedBuffer,
    vertex_buffer: crate::gpu_resource_tracker::TrackedBuffer,
    index_buffer: crate::gpu_resource_tracker::TrackedBuffer,
    /// 音符平面索引缓冲（`QUAD_INDICES`，仅音符 draw 绑定；琴键/Aura 不动）。
    quad_index_buffer: crate::gpu_resource_tracker::TrackedBuffer,
    /// 音符平面模式（`3D音符` 开关关闭 = 平面）：盒子→单面，12→2 三角形/音符。
    /// 只切换音符 draw 的索引缓冲与绘制段；实例/顺序/变换/颜色/其他物体零改动。
    /// 构造默认 false（盒子，现状零行为变化）；导出 handler 每帧按参数显式设置。
    pub flat_notes: bool,
    instance_buffer: Option<crate::gpu_resource_tracker::TrackedBuffer>,

    output_texture: Option<crate::gpu_resource_tracker::TrackedTexture>,
    output_texture_view: Option<wgpu::TextureView>,
    depth_texture: Option<crate::gpu_resource_tracker::TrackedTexture>,
    depth_texture_view: Option<wgpu::TextureView>,

    instance_capacity: usize,
    current_width: u32,
    current_height: u32,

    key_positions: Vec<f32>,
    key_widths: Vec<f32>,
    last_key_count: u32,
    key_press_factors: [f32; 128],
    /// 实例构建暂存集（跨帧复用，避免每帧大堆分配；渲染循环单线程独占）。
    ///
    /// 高密度导出（70 万可见音符）下实例缓冲约 33MB × 2、排序索引约 11MB × 2，
    /// 每帧新建是毫秒级开销；复用后仅首次分配。
    scratch_build: NoteBuildScratch,
    scratch_notes: Vec<MiditrailInstanceGpu>,
    scratch_keys: Vec<MiditrailInstanceGpu>,
    scratch_auras: Vec<MiditrailAuraInstanceGpu>,
    /// `NoteInstance` → `MiditrailNoteGpu` 换算暂存（跨帧复用；36 万可见时约
    /// 11.7MB，每帧新建是毫秒级分配器开销）。仅视频导出 `render_from_instances` 用。
    scratch_derived: Vec<MiditrailNoteGpu>,
    /// 导出全量常驻缓冲（首帧一次上传，`seed_resident`；无 CPU 镜像）。
    resident_buffer: Option<crate::gpu_resource_tracker::TrackedBuffer>,
    resident_capacity: usize,
    resident_count: usize,
    /// 常驻窗口提取器（桶一次构建，常驻复用；见 `cull.rs`）。
    resident_cull: crate::ResidentCull,
    /// cull compact 回读暂存（V×16B，按需扩容）与 CPU 切片（跨帧复用）。
    cull_staging: Option<crate::gpu_resource_tracker::TrackedBuffer>,
    cull_cpu: Vec<crate::NoteInstance>,

    // Aura 相关资源
    aura_pipeline: wgpu::RenderPipeline,
    aura_vertex_buffer: crate::gpu_resource_tracker::TrackedBuffer,
    aura_index_buffer: crate::gpu_resource_tracker::TrackedBuffer,
    aura_instance_buffer: Option<crate::gpu_resource_tracker::TrackedBuffer>,
    aura_instance_capacity: usize,
    aura_sampler: wgpu::Sampler,
    aura_texture: Option<crate::gpu_resource_tracker::TrackedTexture>,
    aura_texture_view: Option<wgpu::TextureView>,
    aura_image_data: Vec<u8>,
    aura_resources_ready: bool,
}

impl MiditrailRenderer {
    const SHADER: &'static str = include_str!("shaders/miditrail_3d.wgsl");
    const TOP_SHADER: &'static str = include_str!("shaders/miditrail_top.wgsl");
    const AURA_SHADER: &'static str = include_str!("shaders/miditrail_aura.wgsl");
    const DRIVEN_SHADER: &'static str = include_str!("shaders/miditrail_note_driven.wgsl");
    // 单位立方体，每面 4 个顶点，含法线（位置 + 法线 = 6 个 f32）
    const CUBE_VERTICES: [f32; 144] = [
        // 顶面 y=1, normal (0,1,0)
        0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 1.0, 1.0, 0.0, 0.0, 1.0, 0.0, 1.0, 1.0, 1.0, 0.0, 1.0, 0.0,
        0.0, 1.0, 1.0, 0.0, 1.0, 0.0, // 底面 y=0, normal (0,-1,0)
        0.0, 0.0, 0.0, 0.0, -1.0, 0.0, 1.0, 0.0, 0.0, 0.0, -1.0, 0.0, 1.0, 0.0, 1.0, 0.0, -1.0,
        0.0, 0.0, 0.0, 1.0, 0.0, -1.0, 0.0, // 正面 z=1, normal (0,0,1)
        0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 1.0, 0.0, 1.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0, 0.0, 0.0, 1.0,
        0.0, 1.0, 1.0, 0.0, 0.0, 1.0, // 背面 z=0, normal (0,0,-1)
        0.0, 0.0, 0.0, 0.0, 0.0, -1.0, 1.0, 0.0, 0.0, 0.0, 0.0, -1.0, 1.0, 1.0, 0.0, 0.0, 0.0,
        -1.0, 0.0, 1.0, 0.0, 0.0, 0.0, -1.0, // 左面 x=0, normal (-1,0,0)
        0.0, 0.0, 0.0, -1.0, 0.0, 0.0, 0.0, 0.0, 1.0, -1.0, 0.0, 0.0, 0.0, 1.0, 1.0, -1.0, 0.0,
        0.0, 0.0, 1.0, 0.0, -1.0, 0.0, 0.0, // 右面 x=1, normal (1,0,0)
        1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 1.0, 1.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0, 0.0, 0.0,
        1.0, 1.0, 0.0, 1.0, 0.0, 0.0,
    ];
    const CUBE_INDICES: [u16; 36] = [
        // 顶面
        0, 1, 2, 0, 2, 3, // 底面
        4, 6, 5, 4, 7, 6, // 正面
        8, 9, 10, 8, 10, 11, // 背面
        12, 14, 13, 12, 15, 14, // 左面
        16, 17, 18, 16, 18, 19, // 右面
        20, 21, 22, 20, 22, 23,
    ];
    /// 音符平面索引（盒子→平面的唯一改动点）：仅顶面 6（y=1 的 X-Z 面）。
    /// 音符实例语义（`instances.rs`）：scale=[宽, 高 NOTE_HEIGHT, Z 长]，
    /// Y 即高度轴；平面 = 只压 Y，X（音高宽）/Z（时值长）原样保留。
    /// 索引逐字复用 `CUBE_INDICES[0..6]` 绕序——同顶点/同插值/同 shader，
    /// 顶面片元与盒子顶面逐位一致；Normal/Top 双视图统一画本面
    /// （Normal 下盒子可见像素本就几乎全来自顶面：高仅 0.007，
    /// 正面只是 z=1 处一条细边；误取正面=把 Z 长压没了）。
    /// 琴键/Aura 继续走立方体缓冲，本缓冲仅音符 draw 绑定。
    const QUAD_INDICES: [u16; 6] = [
        // 顶面 y=1（X-Z 面：X 宽保留，Z 长保留，Y 压平）
        0, 1, 2, 0, 2, 3,
    ];
    /// 平面索引在 `QUAD_INDICES` 中的位置（Normal/Top 双视图共用）。
    const QUAD_RANGE: std::ops::Range<u32> = 0..6;

    const INITIAL_INSTANCE_CAPACITY: usize = 4096;

    /// 创建 Miditrail 渲染器。
    pub fn new(device: &wgpu::Device) -> Self {
        let shader = crate::shader::create_shader_module(device, "miditrail_shader", Self::SHADER);
        let top_shader =
            crate::shader::create_shader_module(device, "miditrail_top_shader", Self::TOP_SHADER);
        let aura_shader =
            crate::shader::create_shader_module(device, "miditrail_aura_shader", Self::AURA_SHADER);
        let driven_shader = crate::shader::create_shader_module(
            device,
            "miditrail_driven_shader",
            Self::DRIVEN_SHADER,
        );
        let bind_group_layout = create_bind_group_layout(device);
        let driven_group_layout = create_driven_group_layout(device);
        let render_pipeline = create_render_pipeline(device, &bind_group_layout, &shader);
        let note_pipeline = create_note_render_pipeline(device, &bind_group_layout, &shader);
        let top_render_pipeline =
            create_top_render_pipeline(device, &bind_group_layout, &top_shader);
        let top_note_pipeline =
            create_top_note_render_pipeline(device, &bind_group_layout, &top_shader);
        let driven_note_pipeline = create_note_driven_pipeline(
            device,
            &bind_group_layout,
            &driven_group_layout,
            &driven_shader,
        );
        let driven_params_buffer = crate::gpu_resource_tracker::TrackedBuffer::new(
            device,
            &wgpu::BufferDescriptor {
                label: Some("miditrail_driven_params_buffer"),
                size: std::mem::size_of::<MiditrailDrivenParamsGpu>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            },
        );
        let aura_pipeline = create_aura_render_pipeline(device, &bind_group_layout, &aura_shader);
        let (uniform_buffer, vertex_buffer, index_buffer) =
            create_buffers(device, &Self::CUBE_VERTICES, &Self::CUBE_INDICES);
        let quad_index_buffer = create_quad_index_buffer(device, &Self::QUAD_INDICES);
        let (aura_vertex_buffer, aura_index_buffer) = create_aura_buffers(device);
        let aura_sampler = create_aura_sampler(device);
        let aura_image_data = generate_aura_ring_data(AURA_TEXTURE_SIZE);

        Self {
            render_pipeline,
            note_pipeline,
            top_render_pipeline,
            top_note_pipeline,
            driven_note_pipeline,
            driven_group_layout,
            driven_bind_group: None,
            driven_params_buffer,
            compact_buffer: None,
            compact_capacity: 0,
            bind_group_layout,
            bind_group: None,
            uniform_buffer,
            vertex_buffer,
            index_buffer,
            quad_index_buffer,
            flat_notes: false,
            instance_buffer: None,
            output_texture: None,
            output_texture_view: None,
            depth_texture: None,
            depth_texture_view: None,
            instance_capacity: 0,
            current_width: 0,
            current_height: 0,
            key_positions: Vec::new(),
            key_widths: Vec::new(),
            last_key_count: 0,
            key_press_factors: [0.0; 128],
            scratch_build: NoteBuildScratch::default(),
            scratch_notes: Vec::new(),
            scratch_keys: Vec::new(),
            scratch_auras: Vec::new(),
            scratch_derived: Vec::new(),
            resident_buffer: None,
            resident_capacity: 0,
            resident_count: 0,
            resident_cull: crate::ResidentCull::new(),
            cull_staging: None,
            cull_cpu: Vec::new(),
            aura_pipeline,
            aura_vertex_buffer,
            aura_index_buffer,
            aura_instance_buffer: None,
            aura_instance_capacity: 0,
            aura_sampler,
            aura_texture: None,
            aura_texture_view: None,
            aura_image_data,
            aura_resources_ready: false,
        }
    }

    /// 渲染一帧到内部离屏纹理。
    ///
    /// # 参数
    /// - `device` — wgpu 设备
    /// - `queue` — wgpu 队列
    /// - `encoder` — 命令编码器（render pass 将追加到此 encoder）
    /// - `uniform` — 渲染参数（tick、尺寸、速度、视图模式等）
    /// - `notes` — 可见音符数据切片
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        uniform: &MiditrailUniformGpu,
        notes: &[MiditrailNoteGpu],
    ) {
        self.render_inner(device, queue, encoder, uniform, notes);
    }

    /// 直接消费统一 `NoteInstance` 的视频导出快捷路径（与 `render` 等价）。
    ///
    /// `NoteInstance` → `MiditrailNoteGpu` 换算写入跨帧复用的 `scratch_derived`，
    /// 消每帧 V×32B 整块分配（36 万可见时约 11.7MB/帧）。只读 key/start/end/color，
    /// 与旧 `note_instances_to_miditrail` 输出逐元素一致。
    pub fn render_from_instances(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        uniform: &MiditrailUniformGpu,
        note_instances: &[crate::NoteInstance],
    ) {
        // take/restore：fill 期间释放对 self 的借用，render_inner 才能拿 &mut self。
        let mut derived = std::mem::take(&mut self.scratch_derived);
        derived.clear();
        derived.reserve(note_instances.len());
        let t_convert = std::time::Instant::now();
        for n in note_instances {
            let (key, rgb) = crate::unpack_key_color(n.key_color);
            let start = n.start_length[0].max(0.0) as u32;
            let end = start.saturating_add(n.start_length[1].max(1.0) as u32);
            derived.push(MiditrailNoteGpu {
                key: key as u32,
                start_tick: start,
                end_tick: end,
                color_packed: pack_color([rgb[0], rgb[1], rgb[2], 1.0]),
                track_idx: 0,
                velocity: 100,
                channel: 0,
                _padding: 0,
            });
        }
        let convert_us = t_convert.elapsed().as_micros() as u64;
        self.render_inner(device, queue, encoder, uniform, &derived);
        Self::diag_convert(convert_us, derived.len());
        self.scratch_derived = derived;
    }

    /// 换算段打点（首 3 帧 + 每 300 帧）：`render_inner` 内部打点见 `diag_stages`。
    fn diag_convert(convert_us: u64, notes: usize) {
        static COUNT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        if n < 3 || n.is_multiple_of(300) {
            tracing::info!("miditrail换算打点[{n}]: convert={convert_us}us notes={notes}");
        }
    }

    /// GPU-Driven 终局路径（Normal 视图）：`NoteInstance` 原字节直传 GPU。
    ///
    /// CPU 每帧只做：单次融合扫描（按键＋光晕系数）＋128 键构建＋两次上传
    /// （compact 音符原字节＋1KB 参数）＋提交。换算/实例构建/排序/gather
    /// 全部删除（位姿由 vertex shader 推导，顺序由深度测试解决）。
    /// Top 视图与实时预览仍走 legacy `render`/`render_from_instances`。
    pub fn render_gpu_driven(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        uniform: &MiditrailUniformGpu,
        notes: &[NoteInstance],
    ) {
        let width = uniform.frame_width;
        let height = uniform.frame_height;
        if width == 0 || height == 0 {
            return;
        }
        self.ensure_output_texture(device, width, height);
        update_key_positions(
            uniform.key_count,
            &mut self.last_key_count,
            &mut self.key_positions,
            &mut self.key_widths,
        );

        let t_scan = std::time::Instant::now();
        let (active_keys, aura_sizes) = compute_active_and_aura_for_compact(
            uniform.tick,
            uniform.ticks_per_second,
            uniform.fps,
            notes,
        );
        self.update_key_press_factors(&active_keys, uniform.fps);
        let scan_us = t_scan.elapsed().as_micros() as u64;

        let mut key_instances = std::mem::take(&mut self.scratch_keys);
        key_instances.clear();
        build_key_instances(
            uniform,
            &active_keys,
            &self.key_positions,
            &self.key_widths,
            &self.key_press_factors,
            &mut key_instances,
        );

        let t_upload = std::time::Instant::now();
        self.ensure_compact_buffer(device, notes.len());
        if let Some(ref buf) = self.compact_buffer {
            queue.write_buffer(buf.inner(), 0, bytemuck::cast_slice(notes));
        }
        let params = instances::build_driven_params(uniform, &self.key_positions, &self.key_widths);
        queue.write_buffer(
            self.driven_params_buffer.inner(),
            0,
            bytemuck::cast_slice(&[params]),
        );
        if self.driven_bind_group.is_none() {
            self.driven_bind_group = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("miditrail_driven_bind_group"),
                layout: &self.driven_group_layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.driven_params_buffer.inner().as_entire_binding(),
                }],
            }));
        }
        // 琴键实例复用 legacy 实例缓冲（本路径只存键，offset 恒为 0）。
        self.ensure_instance_buffer(device, key_instances.len());
        if let Some(ref buf) = self.instance_buffer {
            queue.write_buffer(buf.inner(), 0, bytemuck::cast_slice(&key_instances));
        }
        let upload_us = t_upload.elapsed().as_micros() as u64;

        let mut aura_instances = std::mem::take(&mut self.scratch_auras);
        aura_instances.clear();
        let t_aura = std::time::Instant::now();
        emit_aura_instances(
            &active_keys,
            &aura_sizes,
            uniform.key_count as usize,
            &self.key_positions,
            &self.key_widths,
            &mut aura_instances,
        );
        self.ensure_aura_instance_buffer(device, aura_instances.len());
        if let Some(ref buf) = self.aura_instance_buffer {
            queue.write_buffer(buf.inner(), 0, bytemuck::cast_slice(&aura_instances));
        }
        let aura_us = t_aura.elapsed().as_micros() as u64;

        self.ensure_aura_resources(device, queue);

        let t_submit = std::time::Instant::now();
        let camera = build_camera_uniform(width, height, uniform.view_mode, uniform.z_far_distance);
        queue.write_buffer(
            self.uniform_buffer.inner(),
            0,
            bytemuck::cast_slice(&[camera]),
        );
        if self.bind_group.is_none() {
            self.rebuild_bind_group(device);
        }
        self.execute_driven_pass(encoder, notes.len(), &key_instances, &aura_instances);
        let submit_us = t_submit.elapsed().as_micros() as u64;
        Self::diag_driven(scan_us, upload_us, aura_us, submit_us, notes.len());
        self.scratch_keys = key_instances;
        self.scratch_auras = aura_instances;
    }

    /// GPU-Driven 分段打点（首 3 帧 + 每 300 帧）：与 legacy `diag_stages` 对齐口径。
    fn diag_driven(scan_us: u64, upload_us: u64, aura_us: u64, submit_us: u64, notes: usize) {
        static COUNT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        if n < 3 || n.is_multiple_of(300) {
            tracing::info!(
                "miditrail驱动[{n}]: scan={scan_us} upload={upload_us} aura={aura_us} submit={submit_us} notes={notes}"
            );
        }
    }

    /// 紧凑音符上传缓冲（按字节扩容，`NoteInstance` 16B/音符）。
    fn ensure_compact_buffer(&mut self, device: &wgpu::Device, count: usize) {
        if count <= self.compact_capacity {
            return;
        }
        let new_cap = count
            .next_power_of_two()
            .max(Self::INITIAL_INSTANCE_CAPACITY);
        let size = (new_cap * std::mem::size_of::<NoteInstance>()) as u64;
        let buffer = crate::gpu_resource_tracker::TrackedBuffer::new(
            device,
            &wgpu::BufferDescriptor {
                label: Some("miditrail_compact_buffer"),
                size,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            },
        );
        self.compact_buffer = Some(buffer);
        self.compact_capacity = new_cap;
    }

    /// GPU-Driven 提交：音符（driven 管线＋compact 缓冲）→ Aura → 琴键（legacy 管线）。
    ///
    /// 琴键管线 depth compare 已为 Always，绘制顺序即覆盖顺序，与 legacy
    /// "琴键永远置顶"观感一致；音符之间由深度测试解决（不透明，无需排序）。
    fn execute_driven_pass(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        note_count: usize,
        key_instances: &[MiditrailInstanceGpu],
        aura_instances: &[MiditrailAuraInstanceGpu],
    ) {
        let Some(color_view) = self.output_texture_view.as_ref() else {
            debug_assert!(false, "output_texture_view 应已初始化");
            return;
        };
        let Some(depth_view) = self.depth_texture_view.as_ref() else {
            debug_assert!(false, "depth_texture_view 应已初始化");
            return;
        };
        let Some(bind_group) = self.bind_group.as_ref() else {
            debug_assert!(false, "bind_group 应已初始化");
            return;
        };
        let Some(driven_group) = self.driven_bind_group.as_ref() else {
            debug_assert!(false, "driven_bind_group 应已初始化");
            return;
        };
        let Some(compact_buf) = self.compact_buffer.as_ref() else {
            debug_assert!(false, "compact_buffer 应已初始化");
            return;
        };
        let Some(instance_buf) = self.instance_buffer.as_ref() else {
            debug_assert!(false, "instance_buffer 应已初始化（琴键用）");
            return;
        };

        let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("miditrail_driven_pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: color_view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: 0.0,
                        g: 0.0,
                        b: 0.0,
                        a: 1.0,
                    }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: depth_view,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.0),
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
        });

        // 1. 音符：driven 管线，compact 实例直读。
        render_pass.set_pipeline(&self.driven_note_pipeline);
        render_pass.set_bind_group(0, bind_group, &[]);
        render_pass.set_bind_group(1, driven_group, &[]);
        render_pass.set_vertex_buffer(0, self.vertex_buffer.inner().slice(..));
        render_pass.set_vertex_buffer(1, compact_buf.inner().slice(..));
        render_pass.set_index_buffer(
            self.index_buffer.inner().slice(..),
            wgpu::IndexFormat::Uint16,
        );
        render_pass.draw_indexed(0..Self::CUBE_INDICES.len() as u32, 0, 0..note_count as u32);

        // 2. Aura（加色混合，与 legacy 同）。
        self.draw_aura(&mut render_pass, aura_instances);

        // 3. 琴键（legacy 管线＋实例缓冲，compare Always 置顶）。
        render_pass.set_pipeline(&self.render_pipeline);
        render_pass.set_bind_group(0, bind_group, &[]);
        render_pass.set_vertex_buffer(0, self.vertex_buffer.inner().slice(..));
        render_pass.set_vertex_buffer(1, instance_buf.inner().slice(..));
        render_pass.set_index_buffer(
            self.index_buffer.inner().slice(..),
            wgpu::IndexFormat::Uint16,
        );
        render_pass.draw_indexed(
            0..Self::CUBE_INDICES.len() as u32,
            0,
            0..key_instances.len() as u32,
        );
    }

    /// 渲染内阶段打点（首 3 帧 + 每 300 帧）：拆 render 42ms 黑盒，下一刀的靶子。
    fn diag_stages(
        active_us: u64,
        build_notes_us: u64,
        build_keys_us: u64,
        upload_notes_us: u64,
        aura_us: u64,
        submit_us: u64,
        notes: usize,
    ) {
        static COUNT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        if n < 3 || n.is_multiple_of(300) {
            tracing::info!(
                "miditrail阶段A[{n}]: active={active_us} build_notes={build_notes_us} build_keys={build_keys_us}"
            );
            tracing::info!(
                "miditrail阶段B[{n}]: upload={upload_notes_us} aura={aura_us} submit={submit_us} notes={notes}"
            );
        }
    }

    fn render_inner(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        uniform: &MiditrailUniformGpu,
        notes: &[MiditrailNoteGpu],
    ) {
        let width = uniform.frame_width;
        let height = uniform.frame_height;
        if width == 0 || height == 0 {
            return;
        }

        self.ensure_output_texture(device, width, height);
        update_key_positions(
            uniform.key_count,
            &mut self.last_key_count,
            &mut self.key_positions,
            &mut self.key_widths,
        );
        let t_active = std::time::Instant::now();
        let active_keys = compute_active_keys(uniform.tick, notes);
        self.update_key_press_factors(&active_keys, uniform.fps);
        let active_us = t_active.elapsed().as_micros() as u64;

        let is_top = uniform.view_mode.is_top();
        // Top 先做逐音时间量化对齐（永不合并；Normal 路径零改动，防污染）。
        let top_notes;
        let notes: &[MiditrailNoteGpu] = if is_top {
            top_notes = quantize_notes_for_top(uniform, notes);
            &top_notes
        } else {
            notes
        };

        let mut note_instances = std::mem::take(&mut self.scratch_notes);
        note_instances.clear();
        let t_build_notes = std::time::Instant::now();
        build_note_instances(
            uniform,
            notes,
            &self.key_positions,
            &self.key_widths,
            &mut note_instances,
            &mut self.scratch_build,
        );
        let build_notes_us = t_build_notes.elapsed().as_micros() as u64;
        let mut key_instances = std::mem::take(&mut self.scratch_keys);
        key_instances.clear();
        // Top 键盘不要按下位移（俯视下位移丑且无意义），只保留颜色反馈：
        // 传全零 press 数组，`update_key_press_factors` 照常更新内部状态，
        // 切回 Normal 时按压动画无缝衔接。
        let press_factors: &[f32] = if is_top {
            &ZERO_PRESS_FACTORS
        } else {
            &self.key_press_factors
        };
        let t_build_keys = std::time::Instant::now();
        build_key_instances(
            uniform,
            &active_keys,
            &self.key_positions,
            &self.key_widths,
            press_factors,
            &mut key_instances,
        );
        let build_keys_us = t_build_keys.elapsed().as_micros() as u64;

        let total_instances = note_instances.len() + key_instances.len();
        self.ensure_instance_buffer(device, total_instances);
        let t_upload = std::time::Instant::now();
        let note_bytes =
            (note_instances.len() * std::mem::size_of::<MiditrailInstanceGpu>()) as u64;
        if let Some(ref buf) = self.instance_buffer {
            queue.write_buffer(buf.inner(), 0, bytemuck::cast_slice(&note_instances));
            if !key_instances.is_empty() {
                queue.write_buffer(
                    buf.inner(),
                    note_bytes,
                    bytemuck::cast_slice(&key_instances),
                );
            }
        }
        let upload_notes_us = t_upload.elapsed().as_micros() as u64;

        let mut aura_instances = std::mem::take(&mut self.scratch_auras);
        aura_instances.clear();
        let t_aura = std::time::Instant::now();
        if !is_top {
            // Aura 四边形在俯视下与视线垂直（零面积）天然不可见，
            // Top 直接跳过实例构建与绘制（CPU + GPU 双省）。
            build_aura_instances(
                uniform,
                notes,
                &active_keys,
                &self.key_positions,
                &self.key_widths,
                &mut aura_instances,
            );
            self.ensure_aura_instance_buffer(device, aura_instances.len());
            if let Some(ref buf) = self.aura_instance_buffer {
                queue.write_buffer(buf.inner(), 0, bytemuck::cast_slice(&aura_instances));
            }
        }
        let aura_us = t_aura.elapsed().as_micros() as u64;

        self.ensure_aura_resources(device, queue);

        let t_submit = std::time::Instant::now();
        let camera = build_camera_uniform(width, height, uniform.view_mode, uniform.z_far_distance);
        queue.write_buffer(
            self.uniform_buffer.inner(),
            0,
            bytemuck::cast_slice(&[camera]),
        );

        if self.bind_group.is_none() {
            self.rebuild_bind_group(device);
        }

        self.execute_render_pass(
            encoder,
            &note_instances,
            &key_instances,
            &aura_instances,
            is_top,
        );
        let submit_us = t_submit.elapsed().as_micros() as u64;
        Self::diag_stages(
            active_us,
            build_notes_us,
            build_keys_us,
            upload_notes_us,
            aura_us,
            submit_us,
            notes.len(),
        );
        // 暂存 Vec 归还（保留容量，下一帧零分配复用）。
        self.scratch_notes = note_instances;
        self.scratch_keys = key_instances;
        self.scratch_auras = aura_instances;
    }

    /// 获取输出纹理引用。
    pub fn output_texture(&self) -> Option<&wgpu::Texture> {
        self.output_texture.as_ref().map(|t| t.inner())
    }

    fn ensure_instance_buffer(&mut self, device: &wgpu::Device, count: usize) {
        if count <= self.instance_capacity {
            return;
        }
        let new_cap = count
            .next_power_of_two()
            .max(Self::INITIAL_INSTANCE_CAPACITY);
        let size = (new_cap * std::mem::size_of::<MiditrailInstanceGpu>()) as u64;
        // 旧缓冲由 Option::take 触发 Drop 自动注销
        let buffer = crate::gpu_resource_tracker::TrackedBuffer::new(
            device,
            &wgpu::BufferDescriptor {
                label: Some("miditrail_instance_buffer"),
                size,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            },
        );
        self.instance_buffer = Some(buffer);
        self.instance_capacity = new_cap;
    }

    fn rebuild_bind_group(&mut self, device: &wgpu::Device) {
        // 不变式：rebuild 仅在 aura 纹理已初始化后调用（set_aura_texture 先于 render）
        let Some(view) = self.aura_texture_view.as_ref() else {
            debug_assert!(
                false,
                "aura 纹理应在创建 bind group 前初始化（set_aura_texture 已调用）"
            );
            return;
        };
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("miditrail_bind_group"),
            layout: &self.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.uniform_buffer.inner().as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.aura_sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(view),
                },
            ],
        });
        self.bind_group = Some(bind_group);
    }
}

#[cfg(test)]
mod tests;
