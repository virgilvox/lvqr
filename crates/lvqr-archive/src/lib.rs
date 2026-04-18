//! `lvqr-archive` -- segment index for DVR scrub and time-range
//! playback.
//!
//! This crate owns the map from `(broadcast, track, dts)` to the
//! on-disk location of a CMAF segment that contains that decode
//! timestamp. It does not own the segment bytes themselves: the
//! writer is still [`lvqr-record`] (or any future writer that
//! shares the same filesystem layout). The crate is deliberately
//! tiny and load-bearing rather than general-purpose: every public
//! type exists to answer one of these three questions:
//!
//! * **Record**: the writer just finished a segment; remember where
//!   it landed on disk so a future scrub query can find it.
//! * **Find range**: "give me every segment for broadcast X, track
//!   T, whose [start_dts, end_dts) overlaps [query_start,
//!   query_end)". This is the DVR scrub primitive.
//! * **Latest**: "give me the most recent segment for broadcast X,
//!   track T". This is the live rewind starting point for a
//!   subscriber who wants to jump in at "now minus 10 seconds".
//!
//! Everything else (archive rotation, compaction, S3 upload, cross-
//! node replication) builds on top of these three operations. Doing
//! less here means every future consumer gets the same semantics,
//! and changing the on-disk schema is a single-file diff instead of
//! a cross-crate refactor.
//!
//! ## Design decisions
//!
//! 1. **redb**, not sqlite. Single-file pure-Rust B-tree store with
//!    a copy-on-write MVCC log, optimized for append-mostly
//!    workloads. Avoids a C dependency. Roadmap decision 9 names
//!    redb specifically.
//!
//! 2. **Time in track-native units**. `start_dts` and `end_dts`
//!    are stored in the track's own timescale (90 kHz for LVQR
//!    video, 44.1 / 48 kHz for audio). The index does not know
//!    about wallclock; callers that need "10 seconds ago" convert
//!    against the track timescale themselves. A `timescale` field
//!    rides on every [`SegmentRef`] so readers have the information
//!    without a side lookup.
//!
//! 3. **Byte-encoded compound key**. Keys are `[broadcast_len
//!    u16_be][broadcast][track_len u16_be][track][start_dts
//!    u64_be]` for two reasons: (a) keeping the (broadcast, track)
//!    prefix identical across all rows for one stream means redb's
//!    `range(..)` scan hits them in one contiguous sweep; (b)
//!    big-endian `start_dts` makes byte order equal to numeric
//!    order, which is what the DVR scrub path needs.
//!
//! 4. **Path stored as UTF-8**. The filesystem path of the segment
//!    file is stored as a `String`, not an `OsString`. Non-UTF-8
//!    paths are rejected at insert. Cross-platform stability of
//!    the on-disk schema is worth the small ergonomics cost; if a
//!    deployment insists on a non-UTF-8 archive root, the layout
//!    works against that constraint at the filesystem level, not
//!    the index level.
//!
//! 5. **No async**. The index API is synchronous because redb is
//!    synchronous and the DVR scrub path is latency-sensitive but
//!    not long-running. Callers that want to run an index query
//!    off the tokio runtime can wrap with `spawn_blocking`.
//!
//! ## What this crate OWNS
//!
//! * The segment index (redb B-tree keyed by `(broadcast,
//!   track, start_dts)`) via [`RedbSegmentIndex`].
//! * The canonical on-disk layout `<archive_dir>/<broadcast>/
//!   <track>/<seq:08>.m4s` + the synchronous
//!   `write_segment` helper via [`writer`]. Session 88 session
//!   A1 lifted this writer out of `lvqr-cli` so the layout and
//!   the index live in the same crate; session 88 B will add a
//!   feature-gated `io-uring` variant behind the same
//!   signature.
//!
//! ## What this crate is NOT
//!
//! * Not an HTTP playback endpoint -- that lives in
//!   `lvqr-cli/src/archive.rs` alongside the subscribe-token
//!   check, because the endpoint needs `lvqr-auth`.
//! * Not a transcoder or ABR ladder generator.
//! * Not responsible for disk quota, rotation, or cleanup. A
//!   `delete(broadcast, track, before_dts)` API will land when a
//!   writer integration forces the shape.

mod error;
mod index;
mod segment;
pub mod writer;

pub use error::ArchiveError;
pub use index::{RedbSegmentIndex, SegmentIndex};
pub use segment::SegmentRef;
