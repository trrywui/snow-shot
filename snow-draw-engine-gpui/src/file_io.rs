use crate::{CanvasDocument, CanvasError};
use std::{
    fs,
    io::{Read, Write},
    path::Path,
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT_SAVE: AtomicU64 = AtomicU64::new(1);

/// Reads one bounded drawing file without altering the current document.
pub fn read_document(path: &Path) -> Result<CanvasDocument, CanvasError> {
    CanvasDocument::deserialize(&read_bounded(path)?)
}

/// Reads at most 512 MiB even if another process grows the file during the read.
pub fn read_bounded(path: &Path) -> Result<Vec<u8>, CanvasError> {
    let file = fs::File::open(path).map_err(io_error)?;
    if file.metadata().map_err(io_error)?.len() > 512 * 1024 * 1024 {
        return Err(CanvasError::ResourceLimit);
    }
    let mut bytes = Vec::new();
    file.take(512 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(io_error)?;
    if bytes.len() > 512 * 1024 * 1024 {
        return Err(CanvasError::ResourceLimit);
    }
    Ok(bytes)
}

/// Writes a sibling temporary file and atomically renames it only after a complete write.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), CanvasError> {
    write_atomic_with(path, |writer| writer.write_all(bytes).map_err(io_error))
}

/// Streams into a sibling file; cancellation/failure preserves the old target.
pub fn write_atomic_with(
    path: &Path,
    write: impl FnOnce(&mut fs::File) -> Result<(), CanvasError>,
) -> Result<(), CanvasError> {
    let parent = path
        .parent()
        .ok_or_else(|| CanvasError::InvalidFile("A save directory is required".into()))?;
    let name = path
        .file_name()
        .ok_or_else(|| CanvasError::InvalidFile("A file name is required".into()))?
        .to_string_lossy();
    let temporary = parent.join(format!(
        ".{name}.{}.{}.tmp",
        std::process::id(),
        NEXT_SAVE.fetch_add(1, Ordering::Relaxed)
    ));
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .map_err(io_error)?;
    let result = (|| {
        write(&mut file)?;
        file.sync_all().map_err(io_error)?;
        drop(file);
        fs::rename(&temporary, path).map_err(io_error)
    })();
    if let Err(error) = result {
        let _ = fs::remove_file(&temporary);
        return Err(error);
    }
    Ok(())
}
fn io_error(error: std::io::Error) -> CanvasError {
    CanvasError::InvalidFile(error.to_string())
}
