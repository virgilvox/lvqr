//! Canonical on-disk layout + synchronous segment writer for the
//! DVR archive.
//!
//! **Tier 4 item 4.1, sessions A1 (session 88) + A2 (session 89).**
//! Session 88 lifted the inline `std::fs` calls out of
//! `lvqr_cli::archive::BroadcasterArchiveIndexer::drain` (see
//! `crates/lvqr-cli/src/archive.rs`) into this module so the
//! on-disk layout convention lives alongside the redb index.
//! Session 89 added the io-uring variant behind a compile-time
//! feature + target-OS gate; the outer [`write_segment`] signature
//! is frozen so callers do not change.
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
//! # Why synchronous
//!
//! The caller ([`BroadcasterArchiveIndexer::drain`]) already runs
//! each write inside `tokio::task::spawn_blocking`, so keeping
//! this API sync avoids an unnecessary async adapter. The
//! io_uring variant spins its own current-thread runtime inside
//! the function body via `tokio_uring::start`, matching the same
//! sync signature; the caller-side `spawn_blocking` contract is
//! unchanged.
//!
//! # io-uring variant (session 89 A2)
//!
//! When the crate is built with `--features io-uring` on a Linux
//! target, [`write_segment`] routes the file-create + payload
//! write + fsync phase through `tokio_uring::fs::File` inside a
//! per-call `tokio_uring::start` block. `create_dir_all` stays on
//! `std::fs` because `tokio-uring` 0.5 exposes no mkdir
//! primitive; the tree is created once per broadcast + track and
//! then amortised across thousands of segments, so the extra
//! syscall is noise vs. the payload write.
//!
//! The runtime probe is fallible: old kernels (< 5.6) and
//! container sandboxes without the `io_uring_*` syscalls trigger
//! a panic inside `tokio_uring::start` (the upstream
//! `Runtime::new` unwrap). A process-global `OnceLock<bool>`
//! latch catches the first such failure via `catch_unwind`, logs
//! a single `tracing::warn!`, and pins the fallback to
//! `std::fs::write` for this call and every subsequent call for
//! the rest of the process lifetime. On-path `io::Error`s (create
//! / write / sync / close after the runtime is up) are NOT latched
//! -- those surface as [`ArchiveError::Io`] so the caller's
//! existing warn log fires and the next segment retries.
//!
//! Off-feature or non-Linux: the io-uring path is gated out at
//! compile time and [`write_segment`] is the same std::fs body as
//! session 88's A1 landing.
//!
//! # Future sessions
//!
//! Session 90 B will compare the io-uring variant to the std path
//! under criterion on Linux and decide whether to promote option
//! (a) (per-segment `tokio_uring::start`, shipped here in A2) to
//! option (b) (persistent current-thread runtime pinned to a
//! dedicated writer thread). The outer signature survives either
//! choice.
//!
//! [`BroadcasterArchiveIndexer::drain`]: #
//! [`tokio::task::spawn_blocking`]: #

use std::path::{Path, PathBuf};
#[cfg(all(target_os = "linux", feature = "io-uring"))]
use std::sync::OnceLock;

use crate::ArchiveError;

/// Process-global latch for the io-uring write path. `None` means
/// the probe has not yet been attempted (or has succeeded); `Some(false)`
/// means `tokio_uring::start` has already failed once and the rest of
/// this process must fall back to `std::fs`. The latch is never set
/// to `true`: successful io-uring writes leave the latch at `None` so
/// a later per-call setup failure (unlikely but possible if the kernel
/// is mid-reload, etc.) can still trip the fallback cleanly.
#[cfg(all(target_os = "linux", feature = "io-uring"))]
static IO_URING_AVAILABLE: OnceLock<bool> = OnceLock::new();

/// Filename format for one segment on disk. 8-digit zero-padded
/// sequence number matches the convention established by the
/// original [`BroadcasterArchiveIndexer`] implementation in
/// `lvqr-cli`; changing it would invalidate the `path` field on
/// every pre-existing `SegmentRef` row in the redb index, so the
/// refactor keeps the format byte-for-byte.
const SEGMENT_FILENAME_FMT_WIDTH: usize = 8;

/// Canonical on-disk filename for the persisted init segment.
/// `<archive_dir>/<broadcast>/<track>/init.mp4` -- flat sibling of the
/// `<seq:08>.m4s` segment files. Session 94 B3 introduced the layout
/// for Tier 4 item 4.3's drain-terminated C2PA finalize path so the
/// init bytes are a first-class on-disk artefact rather than living
/// only in `FragmentBroadcaster::meta()` memory.
///
/// Flat `init.mp4` was picked over a `metadata.json` sidecar because
/// (i) it parallels the `<seq>.m4s` segment layout so non-c2pa
/// consumers (future `--export` tooling, operator-driven ffprobe
/// inspection) can reach it with no schema knowledge, (ii) the bytes
/// are already MP4 (moov + ftyp boxes) so concatenation with the
/// segment files for a finalize pass is literal byte concat, and
/// (iii) no JSON codec/timescale/codec_string surface is needed
/// today -- those values live on the `SegmentRef` rows and the
/// `FragmentMeta` wire format. If per-track metadata needs ever
/// grow beyond init bytes, a `metadata.json` sidecar lands alongside
/// this file without breaking the `init.mp4` contract.
pub const INIT_SEGMENT_FILENAME: &str = "init.mp4";

/// Canonical on-disk path for the init segment:
/// `<archive_dir>/<broadcast>/<track>/init.mp4`. See
/// [`INIT_SEGMENT_FILENAME`].
pub fn init_segment_path(archive_dir: &Path, broadcast: &str, track: &str) -> PathBuf {
    archive_dir.join(broadcast).join(track).join(INIT_SEGMENT_FILENAME)
}

/// Persist the init-segment bytes for one `(broadcast, track)` to
/// its canonical path, creating any missing parent directories.
/// Overwrites on repeat calls (the caller typically invokes this
/// once the first time it observes `FragmentBroadcaster::meta()`
/// carrying a non-empty `init_segment`; calling a second time with
/// identical bytes is a cheap idempotent no-op from the
/// caller's perspective).
///
/// Synchronous; callers on a tokio runtime should wrap in
/// [`tokio::task::spawn_blocking`] the same way [`write_segment`]
/// does. The bytes are typically small (ftyp + moov, single-digit
/// KiB for CMAF init segments) so the std::fs path is fine --
/// the io-uring route reserved for high-throughput segment writes
/// is not wired here.
///
/// Session 94 B3 uses this to persist the init bytes discovered on
/// the first fragment received in `BroadcasterArchiveIndexer::drain`
/// so the C2PA finalize orchestrator can concat
/// `init.mp4 + <seq:08>.m4s...` into the bytes-to-sign buffer when
/// the drain task terminates.
pub fn write_init(
    archive_dir: &Path,
    broadcast: &str,
    track: &str,
    init_bytes: &[u8],
) -> Result<PathBuf, ArchiveError> {
    let path = init_segment_path(archive_dir, broadcast, track);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| ArchiveError::Io(format!("create_dir_all {}: {e}", parent.display())))?;
    }
    std::fs::write(&path, init_bytes).map_err(|e| ArchiveError::Io(format!("write init {}: {e}", path.display())))?;
    Ok(path)
}

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
    write_payload(&path, payload)?;
    Ok(path)
}

/// std::fs write path. Always compiled; used as the baseline on every
/// target and as the fallback when the io-uring path is gated out or
/// has tripped the [`IO_URING_AVAILABLE`] latch.
fn write_payload_std(path: &Path, payload: &[u8]) -> Result<(), ArchiveError> {
    std::fs::write(path, payload).map_err(|e| ArchiveError::Io(format!("write {}: {e}", path.display())))
}

#[cfg(not(all(target_os = "linux", feature = "io-uring")))]
fn write_payload(path: &Path, payload: &[u8]) -> Result<(), ArchiveError> {
    write_payload_std(path, payload)
}

#[cfg(all(target_os = "linux", feature = "io-uring"))]
fn write_payload(path: &Path, payload: &[u8]) -> Result<(), ArchiveError> {
    if matches!(IO_URING_AVAILABLE.get(), Some(false)) {
        return write_payload_std(path, payload);
    }
    match write_payload_io_uring(path, payload) {
        Ok(()) => Ok(()),
        Err(IoUringWriteErr::SetupFailed) => {
            // First failure pins the latch; subsequent calls skip the probe
            // and go straight to write_payload_std. The warn is emitted once
            // per process; `OnceLock::set` returns `Err` on later attempts.
            if IO_URING_AVAILABLE.set(false).is_ok() {
                tracing::warn!(
                    path = %path.display(),
                    "tokio_uring::start failed (kernel < 5.6 or sandbox without io_uring syscalls); \
                     falling back to std::fs for archive writes for the rest of this process"
                );
            }
            write_payload_std(path, payload)
        }
        Err(IoUringWriteErr::Io(msg)) => Err(ArchiveError::Io(msg)),
    }
}

/// Split error type for [`write_payload_io_uring`]: `SetupFailed`
/// indicates the `tokio_uring::start` runtime probe panicked (kernel
/// does not support io_uring or the process is sandboxed) and the
/// caller should latch the fallback; `Io` indicates a per-call
/// filesystem error after the runtime came up successfully, which
/// should surface as [`ArchiveError::Io`] without disabling io_uring.
#[cfg(all(target_os = "linux", feature = "io-uring"))]
enum IoUringWriteErr {
    SetupFailed,
    Io(String),
}

/// Route one payload through `tokio_uring::fs`. We wrap
/// `tokio_uring::start` in `catch_unwind` because the upstream API
/// (v0.5) unwraps the `Runtime::new` result internally, so there is
/// no fallible variant to call; `catch_unwind` is the only way to
/// observe a kernel-side setup failure without aborting the process.
#[cfg(all(target_os = "linux", feature = "io-uring"))]
fn write_payload_io_uring(path: &Path, payload: &[u8]) -> Result<(), IoUringWriteErr> {
    use std::panic::{AssertUnwindSafe, catch_unwind};

    let path_buf = path.to_path_buf();
    let payload_vec = payload.to_vec();

    let attempt: std::thread::Result<Result<(), String>> = catch_unwind(AssertUnwindSafe(move || {
        tokio_uring::start(async move {
            let file = tokio_uring::fs::File::create(&path_buf)
                .await
                .map_err(|e| format!("create {}: {e}", path_buf.display()))?;
            let (res, _buf) = file.write_all_at(payload_vec, 0).await;
            res.map_err(|e| format!("write_all_at {}: {e}", path_buf.display()))?;
            file.sync_all()
                .await
                .map_err(|e| format!("sync_all {}: {e}", path_buf.display()))?;
            file.close()
                .await
                .map_err(|e| format!("close {}: {e}", path_buf.display()))?;
            Ok::<(), String>(())
        })
    }));

    match attempt {
        Ok(Ok(())) => Ok(()),
        Ok(Err(msg)) => Err(IoUringWriteErr::Io(msg)),
        Err(_) => Err(IoUringWriteErr::SetupFailed),
    }
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

    #[test]
    fn init_segment_path_follows_broadcast_track_layout() {
        let dir = TempDir::new().unwrap();
        let p = init_segment_path(dir.path(), "live/dvr", "0.mp4");
        assert_eq!(p, dir.path().join("live").join("dvr").join("0.mp4").join("init.mp4"));
    }

    #[test]
    fn write_init_creates_missing_parent_dirs_and_writes_bytes() {
        let dir = TempDir::new().unwrap();
        let bytes = b"ftyp+moov";
        let path = write_init(dir.path(), "live/dvr", "0.mp4", bytes).unwrap();
        assert!(path.exists());
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        assert_eq!(path, init_segment_path(dir.path(), "live/dvr", "0.mp4"));
    }

    #[test]
    fn write_init_is_idempotent_overwrites_existing_file() {
        let dir = TempDir::new().unwrap();
        write_init(dir.path(), "live", "0.mp4", b"first-init").unwrap();
        write_init(dir.path(), "live", "0.mp4", b"second-init").unwrap();
        let p = init_segment_path(dir.path(), "live", "0.mp4");
        assert_eq!(std::fs::read(&p).unwrap(), b"second-init");
    }

    #[test]
    fn write_init_returns_io_error_when_archive_dir_is_a_file() {
        let dir = TempDir::new().unwrap();
        let bogus_root = dir.path().join("not-a-directory");
        std::fs::write(&bogus_root, b"regular file").unwrap();
        let err = write_init(&bogus_root, "live", "0.mp4", b"x").unwrap_err();
        match err {
            ArchiveError::Io(_) => {}
            other => panic!("expected ArchiveError::Io, got {other:?}"),
        }
    }

    /// Exercises the io-uring write path (session 89 A2). Gated on
    /// `target_os = "linux"` + `feature = "io-uring"` because
    /// `tokio-uring` only builds on Linux and the feature is off by
    /// default. The assertion is byte-identity with the std::fs path:
    /// the on-disk contents of a segment written via
    /// `tokio_uring::fs::File::write_all_at` must match the payload
    /// the caller passed. The test also fails loudly if the latch
    /// has tripped into the fallback state, which on a real Linux
    /// runner (kernel >= 5.6) would signal an environmental problem
    /// (seccomp sandbox, missing CAP, etc.) rather than a code bug.
    #[cfg(all(target_os = "linux", feature = "io-uring"))]
    #[test]
    fn write_segment_io_uring_matches_std_bytes() {
        let dir = TempDir::new().unwrap();
        let payload: Vec<u8> = (0..4096u32).map(|i| (i & 0xff) as u8).collect();
        let path = write_segment(dir.path(), "live/dvr", "0.mp4", 17, &payload).unwrap();
        assert!(path.exists());
        assert_eq!(std::fs::read(&path).unwrap(), payload);
        assert_eq!(path, segment_path(dir.path(), "live/dvr", "0.mp4", 17));
        assert_ne!(
            IO_URING_AVAILABLE.get(),
            Some(&false),
            "io_uring fallback latch tripped; expected io_uring path to succeed on this runner"
        );
    }
}
