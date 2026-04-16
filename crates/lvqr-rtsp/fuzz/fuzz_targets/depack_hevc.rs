#![no_main]
//! libfuzzer target for `lvqr_rtsp::rtp::HevcDepacketizer`.
//!
//! HEVC RTP depacketization (RFC 7798) processes untrusted payloads
//! with single NAL units, Aggregation Packets (AP), and Fragmentation
//! Units (FU). The depacketizer must never panic on malformed input.

use libfuzzer_sys::fuzz_target;
use lvqr_rtsp::rtp::{HevcDepacketizer, RtpHeader};

fuzz_target!(|data: &[u8]| {
    let header = RtpHeader {
        payload_type: 96,
        sequence: 0,
        timestamp: 90000,
        ssrc: 0,
        marker: true,
        header_len: 12,
    };
    let mut depack = HevcDepacketizer::new();
    let _ = depack.depacketize(data, &header);
});
