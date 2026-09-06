//! 文件导出模块
//!
//! 提供 MIDI、工程文件等多种格式的导出能力。
//! 所有导出函数均为同步阻塞调用，适合在后台线程中执行。
//!
//! # 主要入口
//!
//! | 格式 | 函数 | 说明 |
//! |------|------|------|
//! | MIDI | [`export_midi`] / [`export_midi_to_bytes`] | 导出标准 MIDI 文件 |
//! | 工程 | [`save_to_archive`] / [`save_to_folder`] | 保存 Lumino 工程文件 |
//!
//! # 转换器
//!
//! [`converter`] 模块提供格式间同步转换的便捷函数，
//! 如 [`export_midi_from_parsed_midi_sync`] 等。

pub mod audio;
pub mod converter;
/// 导出错误类型
pub mod error;
/// `.lmpj` 二进制编码 / 解码
pub mod format;
pub mod material;
pub mod midi;
pub mod project;
pub mod video;
pub mod waterfall_export;

// ── 音频渲染 ──

/// 音频编码器类型
pub use audio::codec::AudioCodec;
/// 进度回调类型
pub use audio::config::ProgressCallback;
/// 音频渲染配置
pub use audio::config::{AudioBackendKind, AudioRenderConfig};
/// GPU 后端检测
pub use audio::gpu_backend::is_gpu_available;
/// 音频渲染（基于 xsynth / GPU）—— 流式模式
pub use audio::render_audio;
/// 音频渲染（基于 xsynth / GPU）—— 内存模式
pub use audio::render_audio_from_document;

// ── 视频导出 ──

/// 视频导出模块（基于 FFmpeg）
pub use video::{FfmpegEncoder, VideoExportConfig, VideoExportError};

// ── 格式转换 ──

/// 格式间同步转换工具函数
pub use converter::{copy_file_sync, export_midi_from_parsed_midi_sync};

// ── 错误类型 ──

/// 导出错误类型与 Result 别名
pub use error::{ExportError, ExportResult};

// ── MIDI 导出 ──

/// MIDI 文件导出（写入磁盘）
pub use midi::export_midi;
/// MIDI 文件导出到内存字节流
pub use midi::export_midi_to_bytes;

/// 贴图瀑布流导出
pub use waterfall_export::export_waterfall_tiles;

// ── 工程文件 ──

/// 素材文件（.lmmaterial）保存 / 路径判断
pub use material::{is_material_path, save_material};
/// 文件夹工程入口文件
pub use project::LuminoEntryFile;
/// 读取 `.lmpj` 入口文件对应的贴图瀑布流元数据
pub use project::load_project_image_metadata;
/// 保存工程为文件夹并生成 `.lmpj` 入口文件
pub use project::save_project_to_folder_with_entry;
/// 工程文件核心类型
pub use project::{
    LoadedFileEntry, LoadedFormat, LuminoProject, TrackSlot, TrackVisibilitySer,
    data_formats::{LmctlData, LmnamesData, LmsigData, LmtempData},
    metadata::ProjectMetadata,
    track::{LmtrackData, TrackMeta},
};
/// 从磁盘加载 Lumino 工程文件（兼容旧版 LMPJ）
pub use project::{load_project, project_to_parsed_midi};
/// 保存工程为压缩包/文件夹
pub use project::{save_to_archive, save_to_folder};
