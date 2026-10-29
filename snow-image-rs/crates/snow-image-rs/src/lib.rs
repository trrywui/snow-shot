//! A UI-independent image service with bounded storage and supervised worker processes.
//!
//! Build and distribute `snow-image-worker` alongside the application, then construct
//! [`ImageService`] with its absolute path. No native codec is linked into this crate.

mod animation;
mod job;
mod platform;
mod prepared_cache;
mod runtime;

pub use animation::{AnimationFrame, AnimationInfo, AnimationSession};
pub use job::Job;
pub use runtime::*;
pub use snow_image_core::*;
