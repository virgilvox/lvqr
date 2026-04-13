#![no_main]
//! libfuzzer target for `lvqr_ingest::remux::parse_audio_tag`.
//!
//! Same invariant as parse_video_tag: never panic on arbitrary bytes.

use bytes::Bytes;
use libfuzzer_sys::fuzz_target;
use lvqr_ingest::remux::parse_audio_tag;

fuzz_target!(|data: &[u8]| {
    let _ = parse_audio_tag(&Bytes::copy_from_slice(data));
});
