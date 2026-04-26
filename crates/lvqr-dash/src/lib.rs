//! MPEG-DASH egress for LVQR.
//!
//! This is the Tier 2.6 crate in the roadmap: a typed Media
//! Presentation Description (MPD) generator that reuses the
//! `lvqr-cmaf` segmenter's CMAF fragments. The crate is deliberately
//! shaped as a sibling of `lvqr-hls`: both egresses subscribe to the
//! same [`lvqr_fragment::FragmentBroadcasterRegistry`] surface the
//! Tier 2.1 ingest migration landed, both buffer segment bytes under
//! predictable URIs, and both project an axum router from the
//! buffered state. The difference is the on-the-wire manifest format.
//!
//! Session 31 lands the MPD renderer and its unit tests. The HTTP
//! surface (`DashServer`, `MultiDashServer`, axum router, segment
//! cache) lands in a follow-up session so this skeleton can be
//! reviewed and tested in isolation.
//!
//! ## Scope choices
//!
//! * **Live profile only** for now. `type="dynamic"` with a
//!   `SegmentTemplate` / `$Number$` addressing mode is the default
//!   for live streaming and directly matches the fixed-duration
//!   segments the `lvqr-cmaf` policy state machine emits. Static
//!   (VOD) profile and `SegmentTimeline` addressing are
//!   future-session work when the DVR scrub story lands alongside
//!   LL-HLS VOD windows.
//! * **Broadcaster-native fragment consumer** (session 60). Ingest
//!   crates publish fragments into a shared
//!   [`lvqr_fragment::FragmentBroadcasterRegistry`]; the
//!   [`bridge::BroadcasterDashBridge`] here installs an
//!   `on_entry_created` callback and spawns one drain task per
//!   `(broadcast, track)` that pumps fragments into a
//!   [`server::MultiDashServer`]. The rendered MPD + segment cache
//!   surface is unchanged.
//! * **Hand-written XML** rather than a SerDe-backed `quick-xml`
//!   serializer. The MPD surface is small (a Period, one to two
//!   AdaptationSets, a Representation per track, a SegmentTemplate)
//!   and hand-writing keeps the crate dependency-light and the
//!   output exactly byte-stable for golden tests. If the MPD grows
//!   enough attributes that a structured serializer helps, it can
//!   be added without changing the public surface.
//! * **Codec strings come from the existing `lvqr-cmaf` helpers**
//!   (`detect_video_codec_string` / `detect_audio_codec_string`) so
//!   H.264 / HEVC / AAC / Opus publishers all surface the right
//!   `codecs` attribute without any DASH-specific codec detection.
//!   That is the exact same contract the LL-HLS master-playlist
//!   renderer uses, which means the two egresses pick up new codec
//!   support the moment the `lvqr-cmaf` helpers learn it.
//!
//! ## Flow
//!
//! 1. [`bridge::BroadcasterDashBridge::install`] wires an
//!    `on_entry_created` callback onto the shared
//!    [`lvqr_fragment::FragmentBroadcasterRegistry`]. For every new
//!    `(broadcast, track)` pair, the callback spawns a drain task
//!    that reads the init segment off the broadcaster meta and pushes
//!    each subsequent fragment into the per-broadcast
//!    [`server::DashServer`] under a monotonic `$Number$` counter.
//! 2. [`server::DashServer::router`] serves three routes per
//!    broadcast: `/dash/{broadcast}/manifest.mpd`,
//!    `/dash/{broadcast}/init-*.m4s`, and
//!    `/dash/{broadcast}/seg-*-$n$.m4s`. The MPD is re-rendered per
//!    request from the current in-memory state so
//!    `minimumUpdatePeriod` can be kept short for low-latency DASH.
//! 3. A typical LL-DASH client polls the MPD every few hundred ms,
//!    follows the SegmentTemplate to the next segment URI, and
//!    fetches the segment via chunked-transfer HTTP/1.1.

pub mod bridge;
pub mod mpd;
pub mod server;

pub use bridge::BroadcasterDashBridge;
pub use mpd::{
    AdaptationSet, DashError, DashEvent, EventStream, Mpd, MpdType, Period, Representation, SCTE35_SCHEME_ID,
    SCTE35_SIGNAL_NS, SegmentTemplate, render_mpd,
};
pub use server::{DashConfig, DashServer, MultiDashServer, OwnerResolver, RedirectFuture};
