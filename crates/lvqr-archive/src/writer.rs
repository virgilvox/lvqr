//! Canonical on-disk layout + synchronous segment writer for the
//! DVR archive.
//!
//! **Tier 4 item 4.1 session A1.** This module is the result of a
//! session-88 refactor that lifted the inline `std::fs` calls out
//! of `lvqr_cli::archive::BroadcasterArchiveIndexer::drain` (see
//! `crates/lvqr-cli/src/archive.rs`) into the `lvqr-archive`
//! crate where the on-disk layout convention already lives
//! alongside the redb index. Behavior is unchanged vs. session
//! 87: [`write_segment`] is a synchronous wrapper around
//! `std::fs::create_dir_all` + `std::fs::write`, intended to run
//! inside a [`tokio::task::spawn_blocking`] closure exactly the
//! way the previous in-place code did.
//!
//! # Why a module in `lvqr-archive`
//!
//! The archive crate already owns the `(broadcast, track, seq)
//! -> on-disk path` convention via the stored `SegmentRef::path`
//! field. Keeping the writer in a different crate meant two
//! places had to agree on the same layout string, which is
//! exactly the staleness pattern that the session 59-60 refactor
//! created (the comment in `crate::lib` previously claimed "Not
//! a segment writer. That is in `lvqr-record`" -- neither still
//! true). This module makes the archive crate the single source
//! of truth for the on-disk layout.
//!
//! # Future sessions
//!
//! Session 88 B will add a `io-uring` feature that swaps the
//! body of [`write_segment`] for a `tokio-uring::fs` variant on
//! Linux. The `write_segment` / [`segment_path`] signatures are
//! frozen here so that swap does not cross the crate boundary.
//!
//! # Why synchronous
//!
//! The caller ([`BroadcasterArchiveIndexer::drain`]) already
//! runs each write inside `tokio::task::spawn_blocking`, so
//! keeping this API sync avoids an unnecessary async adapter.
//! The io_uring-enabled variant in session 88 B will be
//! feature-gated but will share the same sync signature, with
//! the async runtime spun up inside the function body when
//! needed (the usual `tokio_uring::start` pattern); callers do
//! not change.
//!
//! [`BroadcasterArchiveIndexer::drain`]: #
//! [`tokio::task::spawn_blocking`]: #

use std::path::{Path, PathBuf};

use crate::ArchiveError;

/// Filename format for one segment on disk. 8-digit zero-padded
/// sequence number matches the convention established by the
/// original [`BroadcasterArchiveIndexer`] implementation in
/// `lvqr-cli`; changing it would invalidate the `path` field on
/// every pre-existing `SegmentRef` row in the redb index, so the
/// refactor keeps the format byte-for-byte.
const SEGMENT_FILENAME_FMT_WIDTH: usize = 8;

/// Canonical on-disk path for a segment:
/// `<archive_dir>/<broadcast>/<track>/<seq:08>.m4s`.
///
/// `broadcast` may contain `/` characters (e.g. `live/dvr`);
/// those become nested subdirectories under `archive_dir`. The
/// caller is responsible for ensuring the archive dir is a real
/// directory on disk before write time; [`write_segment`]
/// creates the broadcast + track subdirectories on demand.
pub fn segment_path(archive_dir: &Path, broadcast: &str, track: &str, seq: u64) -> PathBuf {
    archive_dir.join(broadcast).join(track).join(format!(
        "{seq:0width$}.m4s",
        seq = seq,
        width = SEGMENT_FILENAME_FMT_WIDTH
    ))
}

/// Write one segment payload to its canonical path under
/// `archive_dir` and return the absolute-or-relative-as-passed
/// path on success. Creates any missing parent directories.
///
/// This function is synchronous. Callers running on a tokio
/// runtime should wrap the call in [`tokio::task::spawn_blocking`]
/// so the thread blocked on `std::fs::write` does not stall the
/// reactor.
///
/// The returned path is the value the caller should record on
/// the matching [`SegmentRef::path`]; it is computed identically
/// to [`segment_path`] above.
///
/// # Errors
///
/// * [`ArchiveError::Io`] if any filesystem step fails. The
///   inner message carries the affected path + the underlying
///   `io::Error` `Display`.
///
/// [`SegmentRef::path`]: crate::SegmentRef::path
pub fn write_segment(
    archive_dir: &Path,
    broadcast: &str,
    track: &str,
    seq: u64,
    payload: &[u8],
) -> Result<PathBuf, ArchiveError> {
    let path = segment_path(archive_dir, broadcast, track, seq);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| ArchiveError::Io(format!("create_dir_all {}: {e}", parent.display())))?;
    }
    std::fs::write(&path, payload).map_err(|e| ArchiveError::Io(format!("write {}: {e}", path.display())))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn segment_path_follows_broadcast_track_seq_layout() {
        let dir = TempDir::new().unwrap();
        let p = segment_path(dir.path(), "live/dvr", "0.mp4", 1);
        assert_eq!(
            p,
            dir.path().join("live").join("dvr").join("0.mp4").join("00000001.m4s")
        );
    }

    #[test]
    fn segment_path_pads_seq_to_eight_digits() {
        let dir = TempDir::new().unwrap();
        let p = segment_path(dir.path(), "live", "0.mp4", 42);
        assert!(p.to_string_lossy().ends_with("00000042.m4s"));
    }

    #[test]
    fn segment_path_does_not_truncate_seq_past_eight_digits() {
        let dir = TempDir::new().unwrap();
        let p = segment_path(dir.path(), "live", "0.mp4", 1_234_567_890);
        assert!(p.to_string_lossy().ends_with("1234567890.m4s"));
    }

    #[test]
    fn write_segment_creates_missing_parent_dirs_and_writes_bytes() {
        let dir = TempDir::new().unwrap();
        let bytes = b"moof+mdat";
        let path = write_segment(dir.path(), "live/dvr", "0.mp4", 7, bytes).unwrap();
        assert!(path.exists());
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        assert_eq!(path, segment_path(dir.path(), "live/dvr", "0.mp4", 7));
    }

    #[test]
    fn write_segment_is_idempotent_overwrites_existing_file() {
        let dir = TempDir::new().unwrap();
        write_segment(dir.path(), "live", "0.mp4", 1, b"first").unwrap();
        write_segment(dir.path(), "live", "0.mp4", 1, b"second").unwrap();
        let p = segment_path(dir.path(), "live", "0.mp4", 1);
        assert_eq!(std::fs::read(&p).unwrap(), b"second");
    }

    #[test]
    fn write_segment_returns_io_error_when_archive_dir_is_a_file() {
        let dir = TempDir::new().unwrap();
        let bogus_root = dir.path().join("not-a-directory");
        std::fs::write(&bogus_root, b"regular file").unwrap();
        let err = write_segment(&bogus_root, "live", "0.mp4", 1, b"x").unwrap_err();
        match err {
            ArchiveError::Io(_) => {}
            other => panic!("expected ArchiveError::Io, got {other:?}"),
        }
    }
}
