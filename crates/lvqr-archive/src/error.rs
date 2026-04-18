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

    /// A filesystem operation in [`crate::writer`] failed. The inner
    /// string carries the affected path plus the underlying
    /// `io::Error` `Display`, so callers log it without taking a
    /// transitive dependency on `std::io::Error` at the API
    /// boundary. Session 88 session A (io_uring archive writes)
    /// uses this variant for both the `std::fs` fallback path and
    /// the future `tokio-uring` path.
    #[error("archive I/O error: {0}")]
    Io(String),

    /// C2PA provenance signing failed (Tier 4 item 4.3). Gated on
    /// the `c2pa` crate feature so downstream consumers not building
    /// with provenance support do not see a dead variant they cannot
    /// construct. Inner string carries the failure site + the
    /// `c2pa::Error` `Display`; callers log it without taking a
    /// transitive dep on `c2pa-rs` at the API boundary.
    #[cfg(feature = "c2pa")]
    #[error("c2pa error: {0}")]
    C2pa(String),
}
