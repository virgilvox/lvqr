//! Conformance check for the `mp4-atom`-backed init-segment writer.
//!
//! Feeds the output of [`lvqr_cmaf::write_avc_init_segment`] to
//! ffprobe via `lvqr_test_utils::ffprobe_bytes`. The test passes if
//! ffprobe exits zero (accepted) or if ffprobe is unavailable on
//! PATH (soft-skip). A "parsed-but-rejected" outcome fails loudly.
//!
//! This is the "conformance" slot of the 5-artifact contract for
//! `lvqr-cmaf`. The AVC init segment is the first real handshake
//! between the `mp4-atom` box writer and an external validator; a
//! regression here means the library integration is broken before
//! any downstream egress crate starts consuming chunks.
//!
//! Note: ffprobe wants to see at least one media segment before it
//! will decode anything, but an init segment alone produces
//! structural warnings on stderr without failing. The helper already
//! treats stderr warnings as diagnostics after the session-4 fix and
//! trusts the exit code as authoritative.

use bytes::BytesMut;
use lvqr_cmaf::{VideoInitParams, write_avc_init_segment};
use lvqr_test_utils::ffprobe_bytes;

#[test]
fn ffprobe_accepts_avc_init_segment() {
    let params = VideoInitParams {
        sps: vec![
            0x67, 0x42, 0x00, 0x1F, 0xD9, 0x40, 0x50, 0x04, 0xFB, 0x01, 0x10, 0x00, 0x00, 0x03, 0x00, 0x10, 0x00, 0x00,
            0x03, 0x03, 0xC0, 0xF1, 0x83, 0x2A,
        ],
        pps: vec![0x68, 0xEB, 0xE3, 0xCB, 0x22, 0xC0],
        width: 1280,
        height: 720,
        timescale: 90_000,
    };
    let mut buf = BytesMut::new();
    write_avc_init_segment(&mut buf, &params).expect("encode");
    ffprobe_bytes(&buf).assert_accepted();
}
