#![no_main]
//! libfuzzer target for the `lvqr-cmaf` codec-string detectors.
//!
//! `detect_video_codec_string` and `detect_audio_codec_string`
//! parse an fMP4 init segment produced by an RTMP or WHIP
//! publisher and extract an RFC 6381 codec string for the HLS
//! master playlist's `CODECS="..."` attribute and the DASH
//! Representation's `codecs` attribute. Both detectors are
//! publisher-reachable: the init-segment bytes flow from an
//! ingest path to these functions without any schema validation,
//! so a crash inside the parser is a publisher-triggered denial
//! of service against the egress surface.
//!
//! Session 33 added XML attribute escaping in `lvqr-dash`
//! (`mpd::esc`) so the *output* of these functions can no longer
//! tear the MPD root apart even if it contains adversarial
//! characters. This target covers the *input* side: it asserts
//! that neither detector panics on arbitrary byte buffers, and
//! that a returned `Some(String)` is nothing weirder than a
//! normal `String` (trivially true given the return type, but
//! the implicit invariant is "no unbounded allocation", which
//! libfuzzer enforces via its OOM ceiling).
//!
//! Invariants asserted:
//!
//! 1. `detect_video_codec_string(data)` never panics.
//! 2. `detect_audio_codec_string(data)` never panics.
//! 3. Neither function allocates so much memory that libfuzzer's
//!    OOM sentinel fires (implicit: the target simply calls the
//!    function and returns normally).

use libfuzzer_sys::fuzz_target;
use lvqr_cmaf::{detect_audio_codec_string, detect_video_codec_string};

fuzz_target!(|data: &[u8]| {
    // Both detectors take `&[u8]` directly and return
    // `Option<String>`. We do not care what they return; we
    // only care that they return at all.
    let _ = detect_video_codec_string(data);
    let _ = detect_audio_codec_string(data);
});
