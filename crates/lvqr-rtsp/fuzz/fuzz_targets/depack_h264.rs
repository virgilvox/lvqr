#![no_main]
//! libfuzzer target for `lvqr_rtsp::rtp::H264Depacketizer`.
//!
//! H.264 RTP depacketization (RFC 6184) processes untrusted payloads
//! that may contain single NAL units, STAP-A aggregation packets, or
//! FU-A fragmentation units. The depacketizer must never panic on
//! malformed payloads -- truncated FU headers, STAP-A with invalid
//! length fields, interleaved start/end flags.

use libfuzzer_sys::fuzz_target;
use lvqr_rtsp::rtp::{H264Depacketizer, RtpHeader};

fuzz_target!(|data: &[u8]| {
    let header = RtpHeader {
        payload_type: 96,
        sequence: 0,
        timestamp: 90000,
        ssrc: 0,
        marker: true,
        header_len: 12,
    };
    let mut depack = H264Depacketizer::new();
    let _ = depack.depacketize(data, &header);
});
