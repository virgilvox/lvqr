//! Error type for the segment index.

use thiserror::Error;

/// Error type returned from all segment-index operations.
///
/// `redb` errors are collapsed into a single `Storage` variant so
/// callers do not have to depend on the `redb` crate to pattern
/// match. Lookup misses return `Ok(None)` or `Ok(Vec::new())` and
/// do not use an error variant.
#[derive(Debug, Error)]
pub enum ArchiveError {
    /// Path used on a `SegmentRef` was not valid UTF-8. See the
    /// crate-level doc for the rationale.
    #[error("segment path is not valid UTF-8")]
    NonUtf8Path,

    /// An on-disk record had an unexpected byte layout. Either the
    /// database was written by a newer version of this crate or
    /// corrupted. The caller should treat this as permanent.
    #[error("corrupt segment record: {0}")]
    Corrupt(String),

    /// redb returned an error during open, txn, insert, commit, or
    /// range scan. The inner string is the redb `Display` so
    /// callers do not need a redb dependency to log it.
    #[error("redb storage error: {0}")]
    Storage(String),
}
