//! 视频导出事件共享类型。
//!
//! 事件层传输结构：与 `lumino-export::video::config` 的强类型枚举同构，
//! 但事件层不依赖导出实现层（依赖方向：export < event），故在此独立定义，
//! 由 runner 做 1:1 映射到导出层枚举。
//!
//! 基础枚举类型见 `types` 子模块，此处保留三个配置结构体并统一对外暴露 `*`。

mod types;

pub use types::*;

/// 计数器渲染配置（事件层传输结构）。
///
/// 参考 Zenith-MIDI NoteCountRender / fmr NoteCounter 的设置模型：
/// 文本模板 + 对齐 + 字号 + 千分位 + 补零 + CSV 导出 + 数字补零宽度。
#[derive(Debug, Clone)]
pub struct NoteCounterConfig {
    /// 文本模板（支持 `{nc}` `{nps}` `{bpm}` 等占位符，`\n` 换行）
    pub text: String,
    /// 文本对齐方式
    pub alignment: CounterAlignment,
    /// 字体大小（像素）
    pub font_size: u32,
    /// 字体来源（内置点阵 / 系统字体 / 自定义字体文件）
    pub font: CounterFont,
    /// 千分位分隔符
    pub separator: CounterSeparator,
    /// 数字补零（启用后按各 pad 宽度左补零）
    pub padding_zeroes: bool,
    /// BPM 整数部分补零宽度
    pub bpm_int_pad: u32,
    /// BPM 小数部分位数
    pub bpm_dec_pad: u32,
    /// 音符数补零宽度
    pub note_count_pad: u32,
    /// 复音数补零宽度
    pub polyphony_pad: u32,
    /// NPS 补零宽度
    pub nps_pad: u32,
    /// 时钟 tick 补零宽度
    pub ticks_pad: u32,
    /// 小节数补零宽度
    pub bars_pad: u32,
    /// 帧数补零宽度
    pub frames_pad: u32,
    /// 是否将每帧统计数据写入 CSV 文件
    pub save_csv: bool,
    /// CSV 输出路径
    pub csv_output: String,
    /// CSV 每行格式（支持与文本模板相同的占位符）
    pub csv_format: String,
}

impl Default for NoteCounterConfig {
    fn default() -> Self {
        Self {
            text: "Notes: {nc} / {tn}\nBPM: {bpm}\nNPS: {nps}\nPPQ: {ppq}\nPolyphony: {plph}\nTime: {currtime}".to_string(),
            alignment: CounterAlignment::TopLeft,
            font_size: 40,
            font: CounterFont::Bitmap,
            separator: CounterSeparator::Comma,
            padding_zeroes: false,
            bpm_int_pad: 3,
            bpm_dec_pad: 2,
            note_count_pad: 5,
            polyphony_pad: 3,
            nps_pad: 3,
            ticks_pad: 5,
            bars_pad: 3,
            frames_pad: 5,
            save_csv: false,
            csv_output: String::new(),
            csv_format: "{nps},{plph},{bpm},{nc}".to_string(),
        }
    }
}

/// 数据曲线渲染配置（事件层传输结构）。
///
/// 移植自 MIDIGraphRenderer（LÖVE2D）的 graph 设置模型：
/// 自动缩放折线 + 水平刻度网格 + 里程碑文字放大 + 数字缩写。
/// 数据源由内部统计状态按帧直传（见 [`DataCurveMetric`]），不走文件 IO。
/// 颜色均为 RGBA 顺序（UI 层 hex 字符串解析后传入）。
#[derive(Debug, Clone)]
pub struct DataCurveConfig {
    /// 数据来源指标
    pub metric: DataCurveMetric,
    /// 曲线窗口时长（秒，默认 2.0，原版 `graph_duration`）
    pub graph_duration: f32,
    /// 缩放动画平滑度（EMA 分母，越大越平滑，默认 8，原版 `zoom_smoothness`）
    pub zoom_smoothness: f32,
    /// 折线前向滑动平均窗口（0=关闭，默认 0，原版 `graph_smoothness`）
    pub graph_smoothness: u32,
    /// 纵轴缩放 padding 放大系数（默认 0.1，原版 `padding_mul`）
    pub padding_mul: f32,
    /// 背景颜色（RGBA）
    pub bg_color: [u8; 4],
    /// 折线颜色（RGBA）
    pub line_color: [u8; 4],
    /// 刻度文字颜色（RGBA）
    pub text_color: [u8; 4],
    /// 水平网格线颜色（RGBA）
    pub bar_color: [u8; 4],
    /// 折线宽度（像素，默认 3）
    pub line_thickness: u32,
    /// 水平网格线宽度（像素，默认 1）
    pub bar_thickness: u32,
    /// 刻度文字字号（像素，默认 24）
    pub font_size: u32,
    /// 刻度文字字体来源（复用计数器字体渲染器）
    pub font: CounterFont,
    /// 刻度文字 X 偏移（像素，默认 2）
    pub text_x_offset: u32,
    /// 刻度文字 Y 偏移（像素，默认 2）
    pub text_y_offset: u32,
    /// 里程碑文字（1k/10k/100k…）放大倍数（默认 1.5）
    pub milestone_scale_mul: f32,
    /// 刻度数字缩写（1,000 → 1K，默认关闭）
    pub abbreviate: bool,
    /// 缩写保留小数位数（默认 3）
    pub abbreviate_digits: u32,
    /// 是否显示刻度文字（默认 true）
    pub show_text: bool,
    /// 是否显示水平网格线（默认 true）
    pub show_bars: bool,
}

impl Default for DataCurveConfig {
    fn default() -> Self {
        Self {
            metric: DataCurveMetric::Nps,
            graph_duration: 2.0,
            zoom_smoothness: 8.0,
            graph_smoothness: 0,
            padding_mul: 0.1,
            bg_color: [0, 0, 0, 255],
            line_color: [0, 255, 255, 255],
            text_color: [255, 255, 255, 127],
            bar_color: [255, 255, 255, 127],
            line_thickness: 3,
            bar_thickness: 1,
            font_size: 24,
            font: CounterFont::Bitmap,
            text_x_offset: 2,
            text_y_offset: 2,
            milestone_scale_mul: 1.5,
            abbreviate: false,
            abbreviate_digits: 3,
            show_text: true,
            show_bars: true,
        }
    }
}

/// MidiConsole 渲染后端（GPU 加速 / CPU 栅格化）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MidiConsoleBackend {
    /// GPU 加速（wgpu）。无可用适配器时由渲染器自动回退 CPU。
    #[default]
    Gpu,
    /// CPU 栅格化（ab_glyph），兼容性最好（无 GPU 依赖）。
    Cpu,
}

impl std::fmt::Display for MidiConsoleBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MidiConsoleBackend::Gpu => write!(f, "GPU"),
            MidiConsoleBackend::Cpu => write!(f, "CPU"),
        }
    }
}

impl std::str::FromStr for MidiConsoleBackend {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim() {
            "GPU" | "gpu" => Ok(MidiConsoleBackend::Gpu),
            "CPU" | "cpu" => Ok(MidiConsoleBackend::Cpu),
            other => Err(format!("未知的 MidiConsole 渲染后端: {other}")),
        }
    }
}

/// MidiConsole 风格渲染配置（事件层传输结构）。
///
/// 复刻 MidiConsole（by Zacksony）的终端像素网格风格：逐通道半块键盘条 +
/// 控制面板（PC/VOL/EXP/PAN/P.BEND/P.RANGE/MOD/HOLD/CUT/RESO/ATT/DEC/REL）。
/// 风格高度固定（忠实原版配色），此处仅暴露少量可调开关，其余沿用原版常量。
#[derive(Debug, Clone)]
pub struct MidiConsoleConfig {
    /// 渲染后端（GPU 加速 / CPU 栅格化），默认 GPU。
    pub render_backend: MidiConsoleBackend,
    /// 是否绘制逐通道控制面板（PC/VOL/EXP/...）。关闭后仅保留键盘条与统计行。
    pub show_control_panel: bool,
    /// 按键按下后的淡出帧数（原版 MaxKeyboardFadeTime = 6）
    pub keyboard_fade_frames: u32,
    /// 控制值变化后的高亮淡出帧数（原版 MaxControlHighLightFadeTime = 40）
    pub control_fade_frames: u32,
    /// 按键按下时的暖色目标 RGB（原版 maxR/G/B = 234/234/208）
    pub warm_key_color: [u8; 3],
}

impl Default for MidiConsoleConfig {
    fn default() -> Self {
        Self {
            render_backend: MidiConsoleBackend::Gpu,
            show_control_panel: true,
            keyboard_fade_frames: 6,
            control_fade_frames: 40,
            warm_key_color: [234, 234, 208],
        }
    }
}

/// 视频导出配置（事件层传输结构）。
#[derive(Debug, Clone)]
pub struct VideoExportConfig {
    /// 输出文件路径
    pub output_path: String,
    /// MIDI 文件路径（流式读取模式使用）
    pub midi_path: String,
    /// 视频宽度（像素）
    pub width: u32,
    /// 视频高度（像素）
    pub height: u32,
    /// 帧率
    pub fps: u32,
    /// MIDI 分辨率（PPQ）
    pub ppq: u16,
    /// 可见键位数（128 或 256，用于 Y 向缩放）
    pub key_count: u16,
    /// 容器格式
    pub container: Container,
    /// 视频编码器
    pub codec: VideoCodec,
    /// 硬件加速后端
    pub backend: EncoderBackend,
    /// 质量预设
    pub quality: QualityPreset,
    /// 渲染模式（瀑布流/音符矩形/MIDITrail/计数器/数据曲线）
    pub render_mode: RenderMode,
    /// 瀑布流滚动速度（0.1~10.0，默认 1.0）
    pub waterfall_scroll_speed: f32,
    /// MIDITrail 视图模式（Normal 普通 / Top 顶部，见 VIEW-001）
    pub miditrail_view_mode: MiditrailViewMode,
    /// MIDITrail Normal 视图滚动速度（0.1~10.0，默认 1.0，由旧共享速度迁移而来）
    pub miditrail_normal_speed: f32,
    /// MIDITrail Top 视图滚动速度（0.1~10.0，默认 1.0，与 Normal 互相独立）
    pub miditrail_top_speed: f32,
    /// MIDITrail Z 方向显示距离（0.1~15.0，默认 7.5，精度 0.1）
    pub miditrail_z_far: f32,
    /// MIDITrail 3D 音符开关（默认 false = 平面）：true 还原盒子，
    /// false 只绘制朝向相机的单面（Normal 正面 / Top 顶面）；行为零改动。
    pub miditrail_3d_notes: bool,
    /// 计数器渲染配置（仅 `render_mode == NoteCounter` 时生效）
    pub note_counter: NoteCounterConfig,
    /// 数据曲线渲染配置（仅 `render_mode == DataCurve` 时生效）
    pub data_curve: DataCurveConfig,
    /// MidiConsole 风格渲染配置（仅 `render_mode == MidiConsole` 时生效）
    pub midi_console: MidiConsoleConfig,
}
