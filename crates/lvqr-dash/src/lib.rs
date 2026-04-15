//! MPEG-DASH egress for LVQR.
//!
//! This is the Tier 2.6 crate in the roadmap: a typed Media
//! Presentation Description (MPD) generator that reuses the
//! `lvqr-cmaf` segmenter's CMAF fragments. The crate is deliberately
//! shaped as a sibling of `lvqr-hls`: both egresses observe the same
//! [`lvqr_ingest::FragmentObserver`] contract, both buffer segment
//! bytes under predictable URIs, and both project an axum router from
//! the buffered state. The difference is the on-the-wire manifest
//! format.
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
//! ## Intended flow (target state once the server lands)
//!
//! 1. A `DashFragmentBridge` implementing `FragmentObserver`
//!    subscribes to every RTMP + WHIP bridge. `on_init` stashes the
//!    init segment bytes under `init-<track>.m4s`. `on_fragment`
//!    pushes partial / segment bytes into a sliding window under
//!    `seg-<track>-<n>.m4s`.
//! 2. `DashServer::router()` serves three routes per broadcast:
//!    `/dash/{broadcast}/manifest.mpd`, `/dash/{broadcast}/init-*.m4s`,
//!    `/dash/{broadcast}/seg-*-$n$.m4s`. The MPD is re-rendered per
//!    request from the current in-memory state so the `minimumUpdatePeriod`
//!    can be kept short for low-latency DASH.
//! 3. A typical LL-DASH client polls the MPD every few hundred ms,
//!    follows the SegmentTemplate to the next segment URI, and
//!    fetches the segment via chunked-transfer HTTP/1.1.

pub mod bridge;
pub mod mpd;
pub mod server;

pub use bridge::DashFragmentBridge;
pub use mpd::{AdaptationSet, DashError, Mpd, MpdType, Period, Representation, SegmentTemplate, render_mpd};
pub use server::{DashConfig, DashServer, MultiDashServer};
