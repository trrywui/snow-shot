//! Private OS-handle transport for disposable Snow Image workers.
//!
//! macOS pixel objects are sized once and are never published while a writer
//! remains alive. The caller owns resource reservations and process supervision.

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::{
    allocation_len, create_pixel_backing, receive_backing, receive_input, send_backing, send_input,
    validate_pixel_backing,
};
