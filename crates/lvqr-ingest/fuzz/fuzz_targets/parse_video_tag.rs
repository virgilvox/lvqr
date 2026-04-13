#![no_main]
//! libfuzzer target for `lvqr_ingest::remux::parse_video_tag`.
//!
//! The parser must never panic on attacker-controlled bytes. Any crash
//! discovered here is a regression. Seeds live under corpus/parse_video_tag/
//! and grow from the lvqr-conformance fixture set plus past crash repros.

use bytes::Bytes;
use libfuzzer_sys::fuzz_target;
use lvqr_ingest::remux::parse_video_tag;

fuzz_target!(|data: &[u8]| {
    let _ = parse_video_tag(&Bytes::copy_from_slice(data));
});
