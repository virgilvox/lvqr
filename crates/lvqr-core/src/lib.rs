//! Core shared types for LVQR.
//!
//! After the Tier 2.1 fragment-model landing, the in-memory fanout types
//! that used to live here (`Registry`, `RingBuffer`, `GopCache`) are gone
//! -- their role has been taken over by `lvqr-moq` (MoQ routing and
//! fanout via `moq-lite::OriginProducer`) and `lvqr-fragment` (the
//! unified media interchange type). The internal audit at
//! `tracking/AUDIT-INTERNAL-2026-04-13.md` recommended deleting them in
//! the same PR that landed their replacement.
//!
//! What remains here:
//!
//! * [`Frame`] and [`TrackName`]: small value types kept as a stable
//!   cross-crate vocabulary for tests and simple in-memory scenarios.
//! * [`EventBus`] / [`RelayEvent`]: lifecycle bus used by the RTMP
//!   bridge, the WS ingest session, and the recorder to coordinate
//!   broadcast start/stop events without polling.
//! * [`CoreError`]: the shared error type for the above.
//! * [`now_unix_ms`]: UNIX wall-clock stamp used by every ingest +
//!   egress crate that records or consumes
//!   [`lvqr_fragment::Fragment::ingest_time_ms`]. Lives here so the
//!   ingest + HLS + DASH copies do not drift.

pub mod error;
pub mod events;
pub mod types;

pub use error::CoreError;
pub use events::{DEFAULT_EVENT_CAPACITY, EventBus, RelayEvent};
pub use types::*;

/// UNIX wall-clock milliseconds. Falls back to `0` when the system
/// clock is set before the UNIX epoch; callers should treat `0` as an
/// unset stamp (matches the `Fragment::ingest_time_ms == 0` "unset"
/// sentinel convention used by every ingest + egress path).
///
/// Consolidated in session 109 A follow-up; previously lived as a
/// private helper in `lvqr_ingest::dispatch`, `lvqr_cli::hls`, and
/// `lvqr_dash::bridge`. Any new ingest or egress surface that needs
/// the ingest wall clock should call this function rather than
/// copy-pasting the same four-line `SystemTime` dance.
pub fn now_unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod time_tests {
    use super::*;

    #[test]
    fn now_unix_ms_is_non_zero_at_reasonable_wall_clock() {
        // Every developer host + CI runner used to run LVQR has a
        // wall clock set after the UNIX epoch; the fallback-to-zero
        // branch is defensive only. Guard against a regression where
        // the helper silently returns 0 for a real clock.
        let ms = now_unix_ms();
        assert!(
            ms > 1_700_000_000_000,
            "now_unix_ms should be after 2023-11-15 UTC, got {ms}"
        );
    }

    #[test]
    fn now_unix_ms_is_monotonic_within_a_test() {
        // Spin a few calls back-to-back; on any realistic host the
        // later call is >= the earlier one. A strict-> would be
        // flaky on sub-millisecond hosts; stay >=.
        let a = now_unix_ms();
        let b = now_unix_ms();
        assert!(b >= a, "later now_unix_ms call ({b}) should not precede earlier ({a})");
    }
}
