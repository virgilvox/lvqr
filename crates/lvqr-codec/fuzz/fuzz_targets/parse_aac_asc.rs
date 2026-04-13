#![no_main]
//! libfuzzer target for `lvqr_codec::aac::parse_asc`.
//!
//! The AAC AudioSpecificConfig parser must never panic on
//! attacker-controlled bytes. Critical because every ASC eventually
//! flows through `lvqr-ingest::remux::fmp4::esds` and becomes the
//! DecoderSpecificInfo payload in the init segment; a panic here
//! would crash the ingest session before any downstream auth or
//! rate-limiting layer sees the request.

use libfuzzer_sys::fuzz_target;
use lvqr_codec::aac::parse_asc;

fuzz_target!(|data: &[u8]| {
    let _ = parse_asc(data);
});
