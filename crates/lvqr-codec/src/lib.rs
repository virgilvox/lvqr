//! Codec parsers for LVQR.
//!
//! This crate is Tier 2.2 of the roadmap (`tracking/ROADMAP.md`). It owns
//! the "given raw codec-private bytes, produce enough information to build
//! an init segment and a FragmentMeta" surface. Every protocol crate that
//! terminates an ingest source (WHIP, SRT, RTSP, RTMP) eventually calls
//! into one of the modules here.
//!
//! ## Scope
//!
//! * [`hevc`]: HEVC (H.265) SPS parsing. Extracts profile / tier / level /
//!   resolution from a raw NAL unit. This is the bare minimum needed for
//!   a codec string (`hev1.<profile>.<compat>.L<level>.<constraint>`) and
//!   for an fMP4 `hvc1` / `hev1` sample entry. Full scaling-list parsing
//!   is intentionally not implemented; that is only needed for pixel-
//!   accurate decoding, which LVQR never does.
//! * [`aac`]: Hardened AAC `AudioSpecificConfig` parser that correctly
//!   decodes the 5-bit + 6-bit escape object-type encoding, the 15-index
//!   explicit-frequency encoding, and HE-AAC / HE-AAC v2 SBR/PS signals.
//!   Replaces the 2-byte ASC assumption baked into
//!   `lvqr-ingest::remux::fmp4::esds`.
//! * [`bit_reader`]: a forward-only MSB-first bit reader with exp-Golomb
//!   decoders, shared across codec modules.
//! * [`error`]: one error type, [`CodecError`].
//!
//! Future modules (VP9, AV1, Opus) land in this crate when the egress
//! protocol crates need them.
//!
//! ## Testing
//!
//! Every parser ships a proptest harness (never-panic on arbitrary input)
//! in `tests/proptest_*.rs`. Integration tests with real encoder output
//! live in `tests/integration_*.rs`. The crate is in scope for the
//! 5-artifact test contract at `tests/CONTRACT.md`.

pub mod aac;
pub mod bit_reader;
pub mod error;
pub mod hevc;
pub mod scte35;
pub mod ts;

pub use error::CodecError;
pub use scte35::{SpliceInfo, parse_splice_info_section};
pub use ts::{PesPacket, StreamType, TsDemuxer};
