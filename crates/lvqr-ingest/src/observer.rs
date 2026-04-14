//! Fragment observer hook for the RTMP -> MoQ bridge.
//!
//! The bridge already feeds every outgoing media unit through a
//! [`lvqr_fragment::MoqTrackSink`] so MoQ subscribers see the data. A
//! second class of consumers (LL-HLS, future DASH, future archive index)
//! needs the same fragments without mutating the MoQ path. The hook
//! here is the smallest contract that lets `lvqr-cli` wire HLS into
//! the bridge without baking HLS-specific code into `lvqr-ingest`.
//!
//! Implementations must be cheap and non-blocking. The observer is
//! invoked from inside the `rml_rtmp` callback chain, which runs on
//! the RTMP server's tokio task; long work or blocking I/O there will
//! stall ingest. The HLS implementation in `lvqr-cli` spawns a task
//! per push for that reason.

use bytes::Bytes;
use lvqr_fragment::Fragment;
use std::sync::Arc;

/// Shared, dynamically-dispatched observer handle.
pub type SharedFragmentObserver = Arc<dyn FragmentObserver>;

/// Observer hook called by [`crate::RtmpMoqBridge`] for every fragment it
/// emits. Implementations stay HLS- / DASH- / archive-agnostic; the
/// bridge only knows it has a list of observers to notify.
pub trait FragmentObserver: Send + Sync {
    /// Called when an init segment becomes available for `(broadcast,
    /// track)`. Fired again on every codec re-config (e.g. mid-stream
    /// resolution change), so implementations should treat repeat
    /// calls as overwrites rather than errors.
    fn on_init(&self, broadcast: &str, track: &str, init: Bytes);

    /// Called for every video / audio [`Fragment`] the bridge emits,
    /// in DTS order per track.
    fn on_fragment(&self, broadcast: &str, track: &str, fragment: &Fragment);
}

/// Drop-in observer that does nothing. Useful as a default when the
/// caller does not want any side channel.
pub struct NoopFragmentObserver;

impl FragmentObserver for NoopFragmentObserver {
    fn on_init(&self, _broadcast: &str, _track: &str, _init: Bytes) {}
    fn on_fragment(&self, _broadcast: &str, _track: &str, _fragment: &Fragment) {}
}
