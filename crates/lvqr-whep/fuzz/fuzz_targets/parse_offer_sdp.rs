#![no_main]
//! libfuzzer target for `str0m::change::SdpOffer::from_sdp_string`.
//!
//! This is the first byte a WHEP server runs over an attacker-
//! controlled body: `POST /whep/{broadcast}` arrives with whatever
//! `application/sdp` payload the client chose to send, and
//! `Str0mAnswerer::create_session` feeds that payload (as utf8) into
//! `SdpOffer::from_sdp_string` before it does anything else. The
//! parser must never panic on arbitrary bytes, must never read out
//! of bounds, and must never infinite-loop on crafted inputs.
//!
//! The proptest harness at `tests/proptest_packetizer.rs` covers the
//! RTP-level never-panic property for the H.264 packetizer; this
//! target covers the never-panic property for the SDP parser the
//! answerer hands input to. Any crash found here is a real reachable
//! bug on the WHEP signaling surface, not a hypothetical concern.
//!
//! Seed the corpus from real browser offers (Chrome devtools,
//! Firefox about:webrtc) before running, and let the fuzzer mutate
//! around them.

use libfuzzer_sys::fuzz_target;
use str0m::change::SdpOffer;

fuzz_target!(|data: &[u8]| {
    // `from_sdp_string` requires utf8. Anything non-utf8 would never
    // have reached this parser in production because
    // `Str0mAnswerer::create_session` rejects non-utf8 bodies before
    // the parser runs, so we mirror that guard here rather than
    // asking the fuzzer to rediscover the utf8 gate.
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let _ = SdpOffer::from_sdp_string(text);
});
