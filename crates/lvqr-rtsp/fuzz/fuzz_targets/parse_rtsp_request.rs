#![no_main]
//! libfuzzer target for `lvqr_rtsp::proto::parse_request`.
//!
//! The RTSP request parser handles untrusted TCP bytes from any
//! client. It must never panic on attacker-shaped input -- truncated
//! headers, missing CRLF terminators, oversized Content-Length,
//! embedded NUL bytes, UTF-8 boundary conditions.

use libfuzzer_sys::fuzz_target;
use lvqr_rtsp::proto::parse_request;

fuzz_target!(|data: &[u8]| {
    let _ = parse_request(data);
});
