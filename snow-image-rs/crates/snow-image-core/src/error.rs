use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use crate::{Budget, Reservation, ResourceKind};

/// Maximum UTF-8 diagnostic length retained by an error, including remote failures.
pub const MAX_DIAGNOSTIC_BYTES: usize = 4096;

/// Bounded error text. Remote diagnostics retain their memory reservation until dropped.
#[derive(Debug)]
pub struct Diagnostic {
    text: String,
    _reservation: Option<Reservation>,
}

impl Diagnostic {
    /// Copies a bounded prefix only after admission to the caller's metadata budget.
    pub fn from_remote(message: &str, budget: &Budget) -> Result<Self> {
        let message = bounded_message(message);
        let mut reservation = budget.reserve(ResourceKind::Metadata, message.len() as u64)?;
        let mut text = String::new();
        text.try_reserve_exact(message.len())
            .map_err(|_| Error::AllocationFailed)?;
        reservation.resize(text.capacity() as u64)?;
        text.push_str(message);
        Ok(Self {
            text,
            _reservation: Some(reservation),
        })
    }

    /// Transfers the bounded text for serialization, releasing any local reservation.
    pub fn into_string(self) -> String {
        self.text
    }
}

fn bounded_message(message: &str) -> &str {
    let mut end = message.len().min(MAX_DIAGNOSTIC_BYTES);
    while !message.is_char_boundary(end) {
        end -= 1;
    }
    &message[..end]
}

impl From<String> for Diagnostic {
    fn from(mut text: String) -> Self {
        text.truncate(bounded_message(&text).len());
        // Local diagnostics have no job owner but cannot retain oversized capacities.
        text.shrink_to_fit();
        Self {
            text,
            _reservation: None,
        }
    }
}

impl From<&str> for Diagnostic {
    fn from(text: &str) -> Self {
        Self::from(bounded_message(text).to_owned())
    }
}

impl std::ops::Deref for Diagnostic {
    type Target = str;
    fn deref(&self) -> &str {
        &self.text
    }
}

impl AsRef<str> for Diagnostic {
    fn as_ref(&self) -> &str {
        &self.text
    }
}

impl std::fmt::Display for Diagnostic {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.text.fmt(formatter)
    }
}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid input: {0}")]
    InvalidInput(Diagnostic),
    #[error("unsupported operation: {0}")]
    Unsupported(Diagnostic),
    #[error("{resource:?} limit exceeded: requested {requested}, limit {limit}")]
    ResourceLimit {
        resource: ResourceKind,
        requested: u64,
        limit: u64,
    },
    #[error("native backend resource limit: {0}")]
    BackendResourceLimit(Diagnostic),
    #[error("memory allocation failed")]
    AllocationFailed,
    #[error("operation cancelled")]
    Cancelled,
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("codec error: {0}")]
    Codec(Diagnostic),
    #[error("worker error: {0}")]
    Worker(Diagnostic),
}

#[derive(Clone, Debug, Default)]
pub struct CancellationToken(Arc<AtomicBool>);

impl CancellationToken {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
    pub fn check(&self) -> Result<()> {
        if self.is_cancelled() {
            Err(Error::Cancelled)
        } else {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ResourcePolicy;

    #[test]
    fn remote_diagnostic_owns_its_reservation() {
        let budget = Budget::default();
        let diagnostic = Diagnostic::from_remote("codec failed", &budget).unwrap();
        assert_eq!(&*diagnostic, "codec failed");
        let error = Error::Codec(diagnostic);
        assert_eq!(budget.used(ResourceKind::Metadata), 12);
        assert_eq!(error.to_string(), "codec error: codec failed");
        drop(error);
        assert_eq!(budget.total_used(), 0);
    }

    #[test]
    fn diagnostics_bound_utf8_and_release_failed_admissions() {
        let budget = Budget::default();
        let text = "界".repeat(MAX_DIAGNOSTIC_BYTES);
        let diagnostic = Diagnostic::from_remote(&text, &budget).unwrap();
        assert_eq!(diagnostic.len(), 4095);
        assert_eq!(budget.used(ResourceKind::Metadata), 4095);
        drop(diagnostic);
        let local = Diagnostic::from(text);
        assert_eq!(local.len(), 4095);
        assert_eq!(local.text.capacity(), 4095);
        let constrained = Budget::new(ResourcePolicy {
            max_metadata_bytes: 3,
            ..ResourcePolicy::default()
        });
        assert!(matches!(
            Diagnostic::from_remote("failure", &constrained),
            Err(Error::ResourceLimit {
                resource: ResourceKind::Metadata,
                ..
            })
        ));
        assert_eq!(constrained.total_used(), 0);
        assert_eq!(budget.total_used(), 0);
    }
}
