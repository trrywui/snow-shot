//! Deliberately small admission policy shared by fuzz entry points.
use snow_image_core::{Budget, ResourcePolicy};
pub const MAX_INPUT: usize = 1024 * 1024;
pub fn budget() -> Budget {
    Budget::new(ResourcePolicy {
        max_input_bytes: MAX_INPUT as u64,
        max_metadata_bytes: 64 * 1024,
        max_animation_frames: 32,
        working_bytes_per_job: 8 * 1024 * 1024,
        total_memory_bytes: 8 * 1024 * 1024,
        max_raster_bytes: 4 * 1024 * 1024,
        max_prepared_bytes: 1024 * 1024,
        max_artifact_bytes: 1024 * 1024,
        max_temp_bytes: 8 * 1024 * 1024,
        max_codec_threads: 1,
        preview_max_dimension: 64,
        ..ResourcePolicy::default()
    })
}
