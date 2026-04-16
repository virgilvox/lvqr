#![no_main]
//! libfuzzer target for `lvqr_rtsp::rtp::AacDepacketizer`.
//!
//! AAC RTP depacketization (RFC 3640 AAC-hbr) processes untrusted
//! payloads with AU-headers-length fields and concatenated AU data.
//! The depacketizer must never panic on malformed input -- truncated
//! AU headers, AU-size fields larger than the packet, zero-length
//! payloads.

use libfuzzer_sys::fuzz_target;
use lvqr_rtsp::rtp::{AacDepacketizer, RtpHeader};

fuzz_target!(|data: &[u8]| {
    let header = RtpHeader {
        payload_type: 97,
        sequence: 0,
        timestamp: 44100,
        ssrc: 0,
        marker: true,
        header_len: 12,
    };
    let depack = AacDepacketizer::new();
    let _ = depack.depacketize(data, &header);
});
