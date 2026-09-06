//! 视频导出事件共享类型 —— 基础枚举与各自的 impl。
//!
//! 从 `video.rs` 拆分而来，由 `video.rs` 通过 `pub use types::*;` 对外统一暴露。

/// 生成 unit-variant 枚举的 `FromStr` 实现（收敛手写样板，别名表与错误消息保留）。
///
/// `$err_fmt` 为错误消息格式串，支持 `{input}` 占位（与原手写实现一致）。
macro_rules! impl_unit_enum_from_str {
    ($ty:ident, $err_fmt:literal, { $($variant:ident => [$($alias:literal),+ $(,)?]),+ $(,)? }) => {
        impl std::str::FromStr for $ty {
            type Err = String;
            fn from_str(input: &str) -> Result<Self, Self::Err> {
                match input {
                    $($($alias => Ok($ty::$variant),)+)*
                    _ => Err(format!($err_fmt, input = input)),
                }
            }
        }
    };
}

/// 输出容器格式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Container {
    /// MP4（默认）
    #[default]
    Mp4,
    /// QuickTime 电影格式
    Mov,
    /// Matroska 格式
    Mkv,
    /// AVI 格式
    Avi,
}

impl_unit_enum_from_str!(Container, "未知容器格式: {input}", {
    Mp4 => ["MP4"],
    Mov => ["MOV"],
    Mkv => ["MKV"],
    Avi => ["AVI"],
});

/// 视频编码器。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum VideoCodec {
    /// H.264（默认）
    #[default]
    H264,
    /// H.265 / HEVC
    H265,
    /// ProRes 编码器
    ProRes,
    /// VP9 编码器
    Vp9,
    /// AV1 编码器
    Av1,
}

impl_unit_enum_from_str!(VideoCodec, "未知视频编码器: {input}", {
    H264 => ["H.264"],
    H265 => ["H.265 / HEVC"],
    ProRes => ["ProRes"],
    Vp9 => ["VP9"],
    Av1 => ["AV1"],
});

/// 硬件加速后端。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EncoderBackend {
    /// 软件编码（CPU，默认）
    #[default]
    Software,
    /// VideoToolbox（macOS）
    VideoToolbox,
    /// NVENC（NVIDIA）
    Nvenc,
    /// AMF（AMD）
    Amf,
    /// QSV（Intel）
    Qsv,
    /// VAAPI（Linux）
    Vaapi,
}

impl_unit_enum_from_str!(EncoderBackend, "未知编码后端: {input}", {
    Software => ["Software (CPU)"],
    VideoToolbox => ["VideoToolbox (macOS)"],
    Nvenc => ["NVENC (NVIDIA)"],
    Amf => ["AMF (AMD)"],
    Qsv => ["QSV (Intel)"],
    Vaapi => ["VAAPI (Linux)"],
});

/// 质量预设。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum QualityPreset {
    /// 高质量
    High,
    /// 中等质量（默认）
    #[default]
    Medium,
    /// 低质量
    Low,
}

impl_unit_enum_from_str!(QualityPreset, "未知质量预设: {input}", {
    High => ["高"],
    Medium => ["中"],
    Low => ["低"],
});

/// 视频导出渲染模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RenderMode {
    /// Lumino瀑布流渲染（默认模式，音符随时间向下流动）
    #[default]
    Waterfall,
    /// 音符矩形渲染（传统钢琴卷帘样式）
    NoteRectangle,
    /// MIDITrail 风格（3D MIDI 轨迹可视化）
    MIDITrail,
    /// 计数器渲染（不绘制卷帘，仅在画面上显示变化的统计数据文本）
    NoteCounter,
    /// 数据曲线渲染（绘制统计数据随时间的折线图，参考 MIDIGraphRenderer 移植）
    DataCurve,
    /// MidiConsole 风格（复刻 MidiConsole 的终端像素网格：逐通道半块键盘条 + 控制面板，参考 MidiConsole by Zacksony）
    MidiConsole,
}

impl RenderMode {
    /// 导出到渲染线程用的规范字符串
    pub fn as_str(&self) -> &'static str {
        match self {
            RenderMode::Waterfall => "waterfall",
            RenderMode::NoteRectangle => "note_rectangle",
            RenderMode::MIDITrail => "miditrail",
            RenderMode::NoteCounter => "note_counter",
            RenderMode::DataCurve => "data_curve",
            RenderMode::MidiConsole => "midi_console",
        }
    }
}

impl std::fmt::Display for RenderMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RenderMode::Waterfall => f.write_str("Lumino瀑布流"),
            RenderMode::NoteRectangle => f.write_str("音符矩形"),
            RenderMode::MIDITrail => f.write_str("MIDITrail"),
            RenderMode::NoteCounter => f.write_str("计数器"),
            RenderMode::DataCurve => f.write_str("数据曲线"),
            RenderMode::MidiConsole => f.write_str("MidiConsole"),
        }
    }
}

impl_unit_enum_from_str!(RenderMode, "未知渲染模式: {input}", {
    Waterfall => ["waterfall", "瀑布流", "Lumino瀑布流"],
    NoteRectangle => ["note_rectangle", "音符矩形"],
    MIDITrail => ["miditrail", "MIDITrail"],
    NoteCounter => ["note_counter", "计数器", "NoteCounter"],
    DataCurve => ["data_curve", "数据曲线", "DataCurve"],
    MidiConsole => ["midi_console", "MidiConsole"],
});

/// MIDITrail 视图模式（Normal 普通 / Top 顶部，见 VIEW-001）。
///
/// 两种视图为平级模式：Normal 为现有 3D 斜视实现迁移而来；
/// Top 为俯视实现（参考 Comet MIDITrail `Top Down Above` 预设）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MiditrailViewMode {
    /// 普通视图（默认，现有行为）。
    #[default]
    Normal,
    /// 顶部视图（俯视，音符精度降级以换取 GPU 开销下降）。
    Top,
}

impl MiditrailViewMode {
    /// 导出到渲染线程用的规范字符串。
    pub fn as_str(&self) -> &'static str {
        match self {
            MiditrailViewMode::Normal => "normal",
            MiditrailViewMode::Top => "top",
        }
    }

    /// 是否为顶部视图。
    pub fn is_top(&self) -> bool {
        matches!(self, MiditrailViewMode::Top)
    }
}

impl std::fmt::Display for MiditrailViewMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MiditrailViewMode::Normal => f.write_str("Normal"),
            MiditrailViewMode::Top => f.write_str("Top"),
        }
    }
}

impl_unit_enum_from_str!(MiditrailViewMode, "未知 MIDITrail 视图模式: {input}", {
    Normal => ["normal", "Normal", "普通"],
    Top => ["top", "Top", "顶部"],
});

/// 数据曲线模式的数据来源指标。
///
/// 原版 MIDIGraphRenderer 从 CSV 文件读入任意列数据；
/// lumino 版直接由内部统计状态按帧传入，可选四种内置指标。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DataCurveMetric {
    /// 每秒新开始音符数（NPS，原版默认展示数据）
    #[default]
    Nps,
    /// 当前复音数（正在发声的音符数）
    Polyphony,
    /// 累计已开始音符数
    NoteCount,
    /// 当前速度（BPM）
    Bpm,
}

impl DataCurveMetric {
    /// 设置面板用的规范字符串（与 `FromStr` 对应）
    pub fn as_str(&self) -> &'static str {
        match self {
            DataCurveMetric::Nps => "nps",
            DataCurveMetric::Polyphony => "polyphony",
            DataCurveMetric::NoteCount => "note_count",
            DataCurveMetric::Bpm => "bpm",
        }
    }
}

impl std::fmt::Display for DataCurveMetric {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DataCurveMetric::Nps => f.write_str("NPS（每秒音符数）"),
            DataCurveMetric::Polyphony => f.write_str("复音数"),
            DataCurveMetric::NoteCount => f.write_str("累计音符数"),
            DataCurveMetric::Bpm => f.write_str("BPM（速度）"),
        }
    }
}

impl_unit_enum_from_str!(DataCurveMetric, "未知数据曲线指标: {input}", {
    Nps => ["nps", "NPS", "NPS（每秒音符数）"],
    Polyphony => ["polyphony", "复音数"],
    NoteCount => ["note_count", "累计音符数"],
    Bpm => ["bpm", "BPM", "BPM（速度）"],
});

/// 计数器文本对齐方式（参考 Zenith-MIDI NoteCountRender 的六种对齐）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CounterAlignment {
    /// 左上角
    #[default]
    TopLeft,
    /// 右上角
    TopRight,
    /// 左下角
    BottomLeft,
    /// 右下角
    BottomRight,
    /// 顶部垂直均匀分布
    TopSpread,
    /// 底部垂直均匀分布
    BottomSpread,
}

impl CounterAlignment {
    /// 设置面板用的规范字符串
    pub fn as_str(&self) -> &'static str {
        match self {
            CounterAlignment::TopLeft => "top_left",
            CounterAlignment::TopRight => "top_right",
            CounterAlignment::BottomLeft => "bottom_left",
            CounterAlignment::BottomRight => "bottom_right",
            CounterAlignment::TopSpread => "top_spread",
            CounterAlignment::BottomSpread => "bottom_spread",
        }
    }
}

impl std::fmt::Display for CounterAlignment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CounterAlignment::TopLeft => f.write_str("左上"),
            CounterAlignment::TopRight => f.write_str("右上"),
            CounterAlignment::BottomLeft => f.write_str("左下"),
            CounterAlignment::BottomRight => f.write_str("右下"),
            CounterAlignment::TopSpread => f.write_str("顶部分散"),
            CounterAlignment::BottomSpread => f.write_str("底部分散"),
        }
    }
}

impl_unit_enum_from_str!(CounterAlignment, "未知对齐方式: {input}", {
    TopLeft => ["top_left", "左上"],
    TopRight => ["top_right", "右上"],
    BottomLeft => ["bottom_left", "左下"],
    BottomRight => ["bottom_right", "右下"],
    TopSpread => ["top_spread", "顶部分散"],
    BottomSpread => ["bottom_spread", "底部分散"],
});

/// 千分位分隔符。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CounterSeparator {
    /// 逗号分隔（1,234,567）
    #[default]
    Comma,
    /// 无分隔符
    Nothing,
}

impl CounterSeparator {
    /// 返回设置面板用的规范字符串
    pub fn as_str(&self) -> &'static str {
        match self {
            CounterSeparator::Comma => "comma",
            CounterSeparator::Nothing => "nothing",
        }
    }
}

impl std::fmt::Display for CounterSeparator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CounterSeparator::Comma => f.write_str("逗号"),
            CounterSeparator::Nothing => f.write_str("无"),
        }
    }
}

impl_unit_enum_from_str!(CounterSeparator, "未知千分位分隔符: {input}", {
    Comma => ["comma", "逗号"],
    Nothing => ["nothing", "无"],
});

/// 计数器文本字体来源。
///
/// - [`CounterFont::Bitmap`]：内置 5x7 点阵字体（无外部依赖，仅支持 ASCII）
/// - [`CounterFont::System`]：操作系统自带字体（如微软雅黑，支持中文等 Unicode）
/// - [`CounterFont::File`]：用户指定的 TTF/OTF/TTC 字体文件
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum CounterFont {
    /// 内置 5x7 点阵字体（仅 ASCII，默认）
    #[default]
    Bitmap,
    /// 系统字体（按名称查系统字体路径表）
    System {
        /// 字体名称（如 "微软雅黑"）
        family: String,
    },
    /// 自定义字体文件（TTF/OTF/TTC）
    File {
        /// 字体文件路径
        path: String,
    },
}

impl CounterFont {
    /// 设置面板用的规范字符串（与 `FromStr` 对应）
    pub fn as_str(&self) -> &'static str {
        match self {
            CounterFont::Bitmap => "bitmap",
            CounterFont::System { .. } => "system",
            CounterFont::File { .. } => "file",
        }
    }
}

impl std::fmt::Display for CounterFont {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CounterFont::Bitmap => f.write_str("内置点阵（5x7）"),
            CounterFont::System { family } => write!(f, "系统字体：{family}"),
            CounterFont::File { path } => write!(f, "自定义字体：{path}"),
        }
    }
}

impl std::str::FromStr for CounterFont {
    type Err = String;
    fn from_str(input: &str) -> Result<Self, Self::Err> {
        match input {
            "bitmap" | "内置点阵" => Ok(CounterFont::Bitmap),
            "system" | "系统字体" => Ok(CounterFont::System {
                family: "微软雅黑".to_string(),
            }),
            "file" | "自定义字体" => Ok(CounterFont::File {
                path: String::new(),
            }),
            _ => Err(format!("未知字体来源: {input}")),
        }
    }
}
