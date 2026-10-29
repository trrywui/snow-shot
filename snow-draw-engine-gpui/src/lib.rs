//! A retained native GPUI adapter for the UI-independent Snow Draw Engine.
//!
//! Shared Rust crates own document and interaction logic; this adapter owns GPU presentation.
mod cleanup;
mod document;
mod export;
mod file_io;
mod font_cache;
#[cfg(target_os = "windows")]
mod gpu_color_glyph;
#[cfg(target_os = "windows")]
mod gpu_export;
mod gpu_geometry;
mod gpu_memory;
#[cfg(target_os = "windows")]
mod gpu_pipeline;
#[cfg(target_os = "windows")]
mod gpu_renderer;
#[cfg(target_os = "windows")]
mod gpu_smart_erase;
#[cfg(all(test, target_os = "windows"))]
mod gpu_tests;
#[cfg(target_os = "windows")]
mod gpu_wake;
mod host;
mod persistence;
mod retained_scene;
mod sample;
mod smart_erase;
mod smart_erase_service;
mod snapshot;
mod source_geometry;
mod text_buffer;
mod text_layout;
mod text_shaping;
mod vector;
mod vector_export;
mod view;

pub use cleanup::{CanvasCleanupDiagnostics, canvas_cleanup_diagnostics, retire_canvas_document};
pub use document::{CanvasDocument, CanvasError, CanvasSession, SourceImage};
pub use file_io::{read_bounded, read_document, write_atomic, write_atomic_with};
#[cfg(target_os = "windows")]
pub use gpu_export::{GpuExportFormat, GpuExportJob, GpuSavePreparation};
#[cfg(target_os = "windows")]
pub use gpu_renderer::{
    DEFAULT_GPU_MEMORY_BUDGET, GPU_TILE_SIZE, GpuCanvas, GpuCanvasFrame, GpuCanvasTile,
    GpuRenderProfile,
};
#[cfg(target_os = "windows")]
pub use gpu_smart_erase::donors::{
    GpuDonorAssembler, GpuDonorInput, GpuDonorJob, GpuDonorOutput, GpuDonorProfile,
};
#[cfg(target_os = "windows")]
pub use gpu_smart_erase::source_prepare::{
    GpuSourceMask, GpuSourcePrepareJob, GpuSourcePrepareProfile, GpuSourcePrepared,
    GpuSourceSolveJob,
};
#[cfg(target_os = "windows")]
pub use gpu_smart_erase::{
    GpuSmartErase, GpuSmartEraseInput, GpuSmartEraseJob, GpuSmartEraseOutput, GpuSmartEraseProfile,
};
#[cfg(target_os = "windows")]
pub use gpu_wake::GpuCompletionWake;
pub use host::{
    CanvasPresentation, CanvasRect, CanvasViewOptions, ExportTile, ImageSourceLayer,
    SmartEraseExecution,
};
pub use persistence::DrawingSnapshot;
pub use retained_scene::RetainedScene;
pub use sample::showcase_document;
pub use smart_erase::Cancellation as CanvasCancellation;
pub use smart_erase::profile::{
    SmartEraseLevelProfile, SmartEraseProfile, SmartEraseSolver, SmartEraseStageTimings,
};
pub use smart_erase_service::{
    SmartEraseJob, SmartEraseJobOutput, SmartErasePatch, SmartEraseService, SmartEraseSnapshot,
};
pub use snapshot::CanvasSnapshot;
pub use snapshot::prepare_text_metrics;
pub use snow_draw_engine as engine;
pub use view::{
    CanvasCommandKeyResolver, CanvasCrosshair, CanvasCrosshairFactory, CanvasCursorLayer,
    CanvasEvent, CanvasView,
};
