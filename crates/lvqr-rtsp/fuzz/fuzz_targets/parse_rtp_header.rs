#![no_main]
//! libfuzzer target for `lvqr_rtsp::rtp::parse_rtp_header` and
//! `parse_interleaved_frame`.
//!
//! Both parsers handle untrusted binary data from the TCP stream.
//! The interleaved frame parser must handle partial reads; the RTP
//! header parser must handle truncated packets, bogus CSRC counts,
//! and extension headers that claim more data than available.

use libfuzzer_sys::fuzz_target;
use lvqr_rtsp::rtp::{parse_interleaved_frame, parse_rtp_header};

fuzz_target!(|data: &[u8]| {
    let _ = parse_rtp_header(data);
    let _ = parse_interleaved_frame(data);
});
