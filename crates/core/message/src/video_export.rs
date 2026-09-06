//! 视频导出动作 — UI 控件交互与 Runner 回调
//!
//! 配置值用 String 传递（UI pick_list 原生支持），
//! Runner 端解析回强类型（与音频导出的 threading/interpolation 解析方式一致）。

/// 视频导出动作
#[derive(Debug, Clone)]
pub enum VideoExportAction {
    // ── 面板控制 ──
    /// 打开视频导出面板（侧边栏视频渲染子按钮）
    OpenPanel,
    /// 关闭视频导出面板
    ClosePanel,

    // ── 配置变更 ──
    /// 容器格式变更（"MP4"/"MOV"/"MKV"/"AVI"）
    ContainerChanged(String),
    /// 视频编码器变更（"H.264"/"H.265 / HEVC"/"ProRes"/"VP9"/"AV1"）
    CodecChanged(String),
    /// 硬件加速后端变更（"Software (CPU)"/"NVENC (NVIDIA)" 等）
    BackendChanged(String),
    /// 质量预设变更（"高"/"中"/"低"）
    QualityChanged(String),
    /// 分辨率宽度变更
    WidthChanged(String),
    /// 分辨率高度变更
    HeightChanged(String),
    /// 帧率变更
    FpsChanged(u32),
    /// 输出路径变更
    OutputPathChanged(String),
    /// 浏览输出路径
    BrowseOutput,
    /// MIDI 路径变更
    MidiPathChanged(String),
    /// 浏览 MIDI 路径
    BrowseMidi,
    /// 渲染模式变更（"Lumino瀑布流"/"音符矩形"/"MIDITrail"/"计数器"）
    RenderModeChanged(String),
    /// MidiConsole 渲染后端变更（"GPU"/"CPU"）
    MidiConsoleBackendChanged(String),
    /// 瀑布流滚动速度变更（0.1~10.0，精度 0.1）
    WaterfallSpeedChanged(f32),
    /// MIDITrail 视图模式变更（"Normal"/"Top"）
    MiditrailViewModeChanged(String),
    /// MIDITrail Normal 视图滚动速度变更（0.1~10.0，精度 0.1）
    MiditrailNormalSpeedChanged(f32),
    /// MIDITrail Top 视图滚动速度变更（0.1~10.0，精度 0.1）
    MiditrailTopSpeedChanged(f32),
    /// MIDITrail Z 方向显示距离变更（0.1~15.0，精度 0.1）
    MiditrailZFarChanged(f32),
    /// MIDITrail 3D 音符开关变更（true=盒子，false=平面默认）
    Miditrail3DNotesChanged(bool),

    // ── 计数器设置变更（参考 Zenith-MIDI NoteCountRender 设置面板） ──
    /// 计数器文本模板编辑（iced text_editor 动作，Handler 内 perform 并同步文本）
    CounterTextAction(iced_widget::text_editor::Action),
    /// 加载预设文本模板（"default"/"full"），整体替换编辑器内容
    CounterLoadTemplate(String),
    /// 计数器对齐方式变更（"左上"/"右上"/"左下"/"右下"/"顶部分散"/"底部分散"）
    CounterAlignmentChanged(String),
    /// 计数器字号变更
    CounterFontSizeChanged(u32),
    /// 计数器字体来源变更（"bitmap"/"system"/"file"）
    CounterFontModeChanged(String),
    /// 计数器系统字体选择变更（如 "微软雅黑"）
    CounterFontFamilyChanged(String),
    /// 计数器自定义字体文件路径变更
    CounterFontPathChanged(String),
    /// 浏览计数器自定义字体文件（rfd 文件对话框）
    CounterBrowseFont,
    /// 计数器千分位变更（true=逗号，false=无）
    CounterUseCommasChanged(bool),
    /// 计数器补零开关变更
    CounterPaddingZeroesChanged(bool),
    /// 计数器 CSV 导出开关变更
    CounterSaveCsvChanged(bool),
    /// 计数器 CSV 输出路径变更
    CounterCsvPathChanged(String),
    /// 计数器 CSV 行格式变更
    CounterCsvFormatChanged(String),
    /// 计数器补零宽度变更（field 为 pad 标识，value 为宽度）
    CounterPadChanged {
        /// pad 标识（"bpm_int"/"bpm_dec"/"nc"/"plph"/"nps"/"ticks"/"bars"/"frames"）
        field: String,
        /// 宽度值
        value: u32,
    },
    /// 恢复计数器默认补零宽度
    CounterResetPadding,
    /// 浏览计数器 CSV 输出路径
    CounterBrowseCsv,
    /// 恢复计数器默认文本模板
    CounterResetText,

    // ── 数据曲线设置变更（参考 MIDIGraphRenderer graph 设置面板） ──
    /// 数据曲线数值型设置变更（field 见下方注释，value 为解析后的数值）
    DataCurveNumberChanged {
        /// field：graph_duration / zoom_smoothness / graph_smoothness / padding_mul /
        /// line_thickness / bar_thickness / text_x_offset / text_y_offset /
        /// milestone_scale_mul / abbreviate_digits
        field: String,
        /// 数值（整数类字段自动取整）
        value: f32,
    },
    /// 数据曲线开关型设置变更
    DataCurveBoolChanged {
        /// field：abbreviate / show_text / show_bars
        field: String,
        /// 开关值
        value: bool,
    },
    /// 数据曲线文本型设置变更（field 见下方注释）
    DataCurveTextChanged {
        /// field：metric（数据来源指标）/ bg_color / line_color / text_color / bar_color /
        /// font_family / font_path / abbreviate_digits（以文本输入）
        field: String,
        /// 文本值
        value: String,
    },
    /// 数据曲线刻度文字字号变更
    DataCurveFontSizeChanged(u32),
    /// 数据曲线字体来源变更（"内置点阵"/"系统字体"/"自定义字体"）
    DataCurveFontModeChanged(String),
    /// 浏览数据曲线自定义字体文件（rfd 文件对话框）
    DataCurveBrowseFont,

    // ── 导出控制 ──
    /// 开始导出（点击「开始导出」按钮，由 handler 发射 StartVideoExport 事件）
    StartExport,
    /// 取消导出（Exporting 状态的「取消」按钮）
    CancelExport,
    /// 强制完成（Finalizing 状态的「强制完成」按钮，跳过等待 ffmpeg 封装）
    ForceFinish,
    /// 关闭覆盖层（Completed/Error 状态的「确定」按钮）
    DismissOverlay,

    // ── Runner 回调 ──
    /// 更新导出进度
    UpdateProgress {
        /// 状态消息
        message: String,
        /// 进度 0.0-1.0
        progress: f64,
        /// 当前帧
        current_frame: u64,
        /// 总帧数
        total_frames: u64,
        /// 渲染速度（fps）
        fps: f64,
    },
    /// 更新预览帧（RGBA 像素数据，含宽高）
    UpdatePreviewFrame {
        /// RGBA 像素数据
        data: Vec<u8>,
        /// 图像宽度
        width: u32,
        /// 图像高度
        height: u32,
    },
    /// 导出完成
    ExportCompleted,
    /// 导出失败
    ExportFailed(String),
}
