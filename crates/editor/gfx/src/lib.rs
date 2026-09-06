//! lumino-gfx：图形渲染层（基于 wgpu）。
//!
//! 负责钢琴卷帘、工程走带、CC 柱状条、标尺、键盘、网格、贴图瀑布流
//! 与 Miditrail 3D 等视图的 GPU 渲染，包含独立的 WGPU 渲染线程、
//! 视频导出管线以及 GPU 音符缓冲区（支持增量更新）。

#![allow(deprecated)]
mod arrangement_instances;
mod arrangement_renderer;
/// 渲染缓存（双缓冲音符实例 + 视口哈希 + 深度纹理缓存）
pub mod cache;
mod cc_bar_renderer;
pub mod constants;
mod context;
mod global_bucket;
mod gpu_note_buffer;
mod gpu_resource_tracker;
pub mod grid;
mod grid_renderer;
mod note_renderer;
mod pipeline;
pub mod render_thread;
mod ruler_renderer;
mod vertical_grid_renderer;
// mod velocity_line_renderer; // 已弃用 — 改用 CcBarRenderer

pub mod automation;
pub mod midiconsole_renderer;
pub mod miditrail_renderer;
pub mod waterfall_renderer;

mod shader;
mod swappable_buffer;

pub use arrangement_instances::{
    ArrangementSceneParams, ArrangementViewColors, ArrangementViewport,
    build_arrangement_overlay_back, build_arrangement_overlay_front,
};
pub use arrangement_renderer::{
    ArrangementNoteInstance, ArrangementNoteUniform, ArrangementRenderer, ArrangementUniform,
    colors,
};
pub use cc_bar_renderer::{
    CcBarColors, CcBarData, CcBarInstance, CcBarRenderer, CcBarViewParams, CcBarViewportUniform,
    build_cc_bar_instances,
};
pub use context::{Context, ContextError, Result};
pub(crate) use global_bucket::readback_bytes_sync;
pub use global_bucket::{
    BucketSource, CullExtract, CullWindow, GlobalBucketError, GlobalBucketIndex, KEY_BUCKETS,
    ResidentCull, prefix_counts,
};
pub use gpu_note_buffer::{GpuNoteBuffer, NoteEvent, OnionSkinStreamMsg};
pub use grid::{generate_ruler_instances, is_black_key};
pub use grid_renderer::{GridLineInstance, GridPrepareParams, GridRenderer};
/// 贴图瀑布流音符类型（从 lumino-midiplayer 重导出）
pub use lumino_midiplayer::texture_waterfall::WaterfallNote;
/// 贴图瀑布流渲染器（从 lumino-midiplayer 重导出）
pub use lumino_midiplayer::texture_waterfall::{
    TextureWaterfallConfig, TextureWaterfallProgressCallback, TextureWaterfallRenderMode,
    TextureWaterfallRenderer, TextureWaterfallUniform, WATERFALL_TRACKS_PER_GROUP,
    WaterfallCacheMeta, WaterfallCommand, WaterfallGenContext, WaterfallGenerateError,
    WaterfallGpuCtx, WaterfallGroupTile, WaterfallMeta, WaterfallStreamMsg, WaterfallTileCoord,
    WaterfallTrackParams, WaterfallTrackTile, WaterfallViewportParams,
    compute_waterfall_cache_hash, generate_waterfall_tiles, generate_waterfall_tiles_streaming,
    generate_waterfall_track_tile, merge_waterfall_group_tiles, merge_waterfall_track_tile_into,
    read_waterfall_track_tile_cache,
};
pub use note_renderer::{
    CameraParams, CameraUniform, CullUniform, NoteInstance, NoteRenderer, PREVIEW_BORDER_SENTINEL,
    RenderUniform, ViewState, calculate_border_width, pack_key_color, unpack_key_color,
};
pub use ruler_renderer::{
    RulerPrepareParams, RulerRenderer, RulerTickInstance, RulerViewportUniform,
};
pub use swappable_buffer::{AtomicSwappableBuffer, MpscQueue, RenderData, SwappableBuffer};
pub use vertical_grid_renderer::VerticalGridRenderer;

pub use miditrail_renderer::{
    MiditrailNoteGpu, MiditrailRenderer, MiditrailUniformGpu, MiditrailViewMode, pack_color,
};
pub use render_thread::{RenderParams, WgpuRenderThread};
pub use waterfall_renderer::{WaterfallRenderer, WaterfallUniformGpu};
/// 重导出 wgpu 纹理格式，供 UI 层匹配视频导出像素格式
pub use wgpu::TextureFormat;

#[cfg(test)]
mod test_minimal;
