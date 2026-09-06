//! 视频导出面板状态

/// MIDITrail Z 方向显示距离默认值。
pub const MIDITRAIL_Z_FAR_DEFAULT: f32 = 7.5;
/// MIDITrail Z 方向显示距离最大值（也是滑杆上限）。
pub const MIDITRAIL_Z_FAR_MAX: f32 = 15.0;
/// MIDITrail 视图模式默认值（Normal 普通视图）。
pub const MIDITRAIL_VIEW_MODE_DEFAULT: &str = "Normal";
/// MIDITrail Normal/Top 视图滚动速度默认值（与瀑布流速度默认值一致）。
pub const MIDITRAIL_SPEED_DEFAULT: f32 = 1.0;

/// 计数器默认文本模板（参考 Zenith-MIDI NoteCountRender 的 default 模板）。
pub const COUNTER_DEFAULT_TEXT: &str =
    "Notes: {nc} / {tn}\nBPM: {bpm}\nNPS: {nps}\nPPQ: {ppq}\nPolyphony: {plph}\nTime: {currtime}";
/// 计数器完整文本模板（全部占位符演示）。
pub const COUNTER_FULL_TEXT: &str = "Notes: {nc} / {tn} / {nr}\nBPM: {bpm}\nNPS: {nps} (Max: {mnps})\nPolyphony: {plph} (Max: {mplph})\nSeconds: {currsec} / {totalsec} / {remsec}\nTime: {currtime} / {totaltime} / {remtime}\nTicks: {currticks} / {totalticks} / {remticks}\nBars: {currbars} / {totalbars} / {rembars}\nFrames: {currframes} / {totalframes} / {remframes}\nPPQ: {ppq}\nTime Signature: {tsn}/{tsd}\nAverage NPS: {avgnps}\n\n-----Progress-----\nNotes: {notep}%\nTicks: {tickp}%\nTime: {timep}%";
/// 计数器默认 CSV 行格式。
pub const COUNTER_DEFAULT_CSV_FORMAT: &str = "{nps},{plph},{bpm},{nc}";

/// 视频导出覆盖层状态（参照 nezha ExportState）
#[derive(Debug, Clone, Default)]
pub enum VideoExportOverlayState {
    /// 空闲（无覆盖层）
    #[default]
    None,
    /// 导出中（渲染+写帧）
    Exporting,
    /// 编码收尾（等待 ffmpeg 封装）
    Finalizing,
    /// 完成
    Completed {
        /// 总帧数
        total_frames: u64,
        /// 总用时（秒）
        elapsed_secs: f64,
        /// 平均渲染速度
        avg_fps: f64,
    },
    /// 错误
    Error(String),
}

/// 视频导出面板状态（主界面侧边栏面板）
///
/// 纯 UI 状态，保存控件值与导出进度。
/// 配置值用 String 存储（UI pick_list 原生支持），Runner 端解析回强类型。
#[derive(Debug, Clone)]
pub struct VideoExportDialogState {
    /// 容器格式（"MP4"/"MOV"/"MKV"/"AVI"）
    pub container: String,
    /// 视频编码器（"H.264"/"H.265 / HEVC"/"ProRes"/"VP9"/"AV1"）
    pub codec: String,
    /// 硬件加速后端（"Software (CPU)"/"NVENC (NVIDIA)" 等）
    pub backend: String,
    /// 质量预设（"高"/"中"/"低"）
    pub quality: String,
    /// 渲染模式（"Lumino瀑布流"/"音符矩形"/"MIDITrail"/"计数器"）
    pub render_mode: String,
    /// MidiConsole 渲染后端（"GPU"/"CPU"，默认 GPU）
    pub midi_console_backend: String,
    /// 瀑布流滚动速度（默认 1.0）
    pub waterfall_speed: f32,
    /// MIDITrail 视图模式（"Normal"/"Top"，默认 Normal；视图切换入口）
    pub miditrail_view_mode: String,
    /// MIDITrail Normal 视图滚动速度（默认 1.0，由旧共享速度迁移而来）
    pub miditrail_normal_speed: f32,
    /// MIDITrail Top 视图滚动速度（默认 1.0，与 Normal 互相独立）
    pub miditrail_top_speed: f32,
    /// MIDITrail Z 方向显示距离（默认 7.5，精度 0.1）
    pub miditrail_z_far: f32,
    /// MIDITrail 3D 音符开关（默认 false = 平面；true 还原盒子）
    pub miditrail_3d_notes: bool,
    // ── 计数器设置（参考 Zenith-MIDI NoteCountRender 设置面板） ──
    /// 计数器文本模板
    pub counter_text: String,
    /// 计数器文本模板多行编辑器内容（iced text_editor 状态，绑定 wgpu 渲染器）
    pub counter_editor: iced_widget::text_editor::Content<iced_wgpu::Renderer>,
    /// 计数器对齐方式（"左上"/"右上"/"左下"/"右下"/"顶部分散"/"底部分散"）
    pub counter_alignment: String,
    /// 计数器字号（像素）
    pub counter_font_size: u32,
    /// 计数器字体来源（"bitmap"/"system"/"file"）
    pub counter_font_mode: String,
    /// 计数器系统字体名称（如 "微软雅黑"）
    pub counter_font_family: String,
    /// 计数器自定义字体文件路径
    pub counter_font_path: String,
    /// 计数器千分位（true=逗号，false=无）
    pub counter_use_commas: bool,
    /// 计数器数字补零
    pub counter_padding_zeroes: bool,
    /// 计数器 CSV 导出开关
    pub counter_save_csv: bool,
    /// 计数器 CSV 输出路径
    pub counter_csv_output: String,
    /// 计数器 CSV 行格式
    pub counter_csv_format: String,
    // ── 数据曲线设置（参考 MIDIGraphRenderer graph 设置面板） ──
    /// 数据来源指标（"NPS（每秒音符数）"/"复音数"/"累计音符数"/"BPM（速度）"）
    pub dc_metric: String,
    /// 曲线窗口时长（秒）
    pub dc_graph_duration: String,
    /// 缩放动画平滑度
    pub dc_zoom_smoothness: String,
    /// 折线平滑窗口（0=关闭）
    pub dc_graph_smoothness: String,
    /// 纵轴缩放 padding 放大系数
    pub dc_padding_mul: String,
    /// 背景颜色（hex 字符串，如 "#000000"，支持 8 位 hex 带 alpha）
    pub dc_bg_color: String,
    /// 折线颜色（hex 字符串）
    pub dc_line_color: String,
    /// 刻度文字颜色（hex 字符串）
    pub dc_text_color: String,
    /// 水平网格线颜色（hex 字符串）
    pub dc_bar_color: String,
    /// 折线宽度（像素）
    pub dc_line_thickness: String,
    /// 水平网格线宽度（像素）
    pub dc_bar_thickness: String,
    /// 刻度文字字号（像素）
    pub dc_font_size: u32,
    /// 刻度文字字体来源（"内置点阵"/"系统字体"/"自定义字体"）
    pub dc_font_mode: String,
    /// 系统字体名称
    pub dc_font_family: String,
    /// 自定义字体文件路径
    pub dc_font_path: String,
    /// 刻度文字 X 偏移（像素）
    pub dc_text_x_offset: String,
    /// 刻度文字 Y 偏移（像素）
    pub dc_text_y_offset: String,
    /// 里程碑文字放大倍数
    pub dc_milestone_scale_mul: String,
    /// 刻度数字缩写（1,000 → 1K）
    pub dc_abbreviate: bool,
    /// 缩写保留小数位数
    pub dc_abbreviate_digits: String,
    /// 显示刻度文字
    pub dc_show_text: bool,
    /// 显示水平网格线
    pub dc_show_bars: bool,
    /// BPM 整数部分补零宽度
    pub counter_bpm_int_pad: u32,
    /// BPM 小数部分位数
    pub counter_bpm_dec_pad: u32,
    /// 音符数补零宽度
    pub counter_note_count_pad: u32,
    /// 复音数补零宽度
    pub counter_polyphony_pad: u32,
    /// NPS 补零宽度
    pub counter_nps_pad: u32,
    /// 时钟 tick 补零宽度
    pub counter_ticks_pad: u32,
    /// 小节数补零宽度
    pub counter_bars_pad: u32,
    /// 帧数补零宽度
    pub counter_frames_pad: u32,
    /// 分辨率宽度
    pub width: u32,
    /// 分辨率高度
    pub height: u32,
    /// 帧率
    pub fps: u32,
    /// MIDI 文件路径（流式读取模式使用；内存模式优先使用已加载的 MidiDocument）
    pub midi_path: String,
    /// 输出路径
    pub output_path: String,
    /// 覆盖层状态（None=空闲，其余=显示模态覆盖层）
    pub overlay: VideoExportOverlayState,
    /// 进度 (0.0 - 1.0)
    pub progress: f64,
    /// 状态消息
    pub status_message: String,
    /// 当前已渲染帧
    pub current_frame: u64,
    /// 总帧数
    pub total_frames: u64,
    /// 渲染速度（fps，EMA 平滑）
    pub render_fps: f64,
    /// 已用时间（秒，墙钟真实时间，由导出线程测量并通过进度通道传入）
    pub elapsed_secs: f64,
    /// 预览帧数据（RGBA 格式，压缩后用于 dialog 内显示预览图像）
    pub preview_frame: Option<Vec<u8>>,
    /// 预览帧宽度
    pub preview_width: u32,
    /// 预览帧高度
    pub preview_height: u32,
    /// 缓存的 iced image handle（避免每帧创建唯一 ID 导致 GPU 纹理缓存失效）
    ///
    /// `Handle::from_rgba` 每次调用生成唯一 ID，iced_wgpu 对大图（>2MB）走异步上传，
    /// 每个新 ID 都被视为全新图像重新上传。缓存 handle 后，相同数据复用已上传的纹理。
    pub cached_image_handle: Option<iced_core::image::Handle>,
}

impl Default for VideoExportDialogState {
    fn default() -> Self {
        Self::new()
    }
}

impl VideoExportDialogState {
    /// 创建一个默认的视频导出对话框状态
    pub fn new() -> Self {
        Self {
            container: "MP4".to_string(),
            codec: "H.264".to_string(),
            backend: "Software (CPU)".to_string(),
            quality: "中".to_string(),
            render_mode: "Lumino瀑布流".to_string(),
            midi_console_backend: "GPU".to_string(),
            waterfall_speed: 1.0,
            miditrail_view_mode: MIDITRAIL_VIEW_MODE_DEFAULT.to_string(),
            miditrail_normal_speed: MIDITRAIL_SPEED_DEFAULT,
            miditrail_top_speed: MIDITRAIL_SPEED_DEFAULT,
            miditrail_z_far: MIDITRAIL_Z_FAR_DEFAULT,
            miditrail_3d_notes: false,
            counter_text: COUNTER_DEFAULT_TEXT.to_string(),
            counter_editor: iced_widget::text_editor::Content::<iced_wgpu::Renderer>::default(),
            counter_alignment: "左上".to_string(),
            counter_font_size: 40,
            counter_font_mode: "内置点阵".to_string(),
            counter_font_family: "微软雅黑".to_string(),
            counter_font_path: String::new(),
            counter_use_commas: true,
            counter_padding_zeroes: false,
            counter_save_csv: false,
            counter_csv_output: String::new(),
            counter_csv_format: COUNTER_DEFAULT_CSV_FORMAT.to_string(),
            dc_metric: "NPS（每秒音符数）".to_string(),
            dc_graph_duration: "2.0".to_string(),
            dc_zoom_smoothness: "8.0".to_string(),
            dc_graph_smoothness: "0".to_string(),
            dc_padding_mul: "0.1".to_string(),
            dc_bg_color: "#000000".to_string(),
            dc_line_color: "#00FFFF".to_string(),
            dc_text_color: "#FFFFFF7F".to_string(),
            dc_bar_color: "#FFFFFF7F".to_string(),
            dc_line_thickness: "3".to_string(),
            dc_bar_thickness: "1".to_string(),
            dc_font_size: 24,
            dc_font_mode: "内置点阵".to_string(),
            dc_font_family: "微软雅黑".to_string(),
            dc_font_path: String::new(),
            dc_text_x_offset: "2".to_string(),
            dc_text_y_offset: "2".to_string(),
            dc_milestone_scale_mul: "1.5".to_string(),
            dc_abbreviate: false,
            dc_abbreviate_digits: "3".to_string(),
            dc_show_text: true,
            dc_show_bars: true,
            counter_bpm_int_pad: 3,
            counter_bpm_dec_pad: 2,
            counter_note_count_pad: 5,
            counter_polyphony_pad: 3,
            counter_nps_pad: 3,
            counter_ticks_pad: 5,
            counter_bars_pad: 3,
            counter_frames_pad: 5,
            width: 1920,
            height: 1080,
            fps: 60,
            midi_path: String::new(),
            output_path: String::new(),
            overlay: VideoExportOverlayState::None,
            progress: 0.0,
            status_message: String::new(),
            current_frame: 0,
            total_frames: 0,
            render_fps: 0.0,
            elapsed_secs: 0.0,
            preview_frame: None,
            preview_width: 0,
            preview_height: 0,
            cached_image_handle: None,
        }
    }

    /// 是否正在导出（覆盖层可见）
    pub fn is_exporting(&self) -> bool {
        !matches!(self.overlay, VideoExportOverlayState::None)
    }
}
