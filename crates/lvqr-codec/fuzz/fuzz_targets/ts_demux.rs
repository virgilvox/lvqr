#![no_main]
//! libfuzzer target for the MPEG-TS demuxer.
//!
//! The demuxer sits on the SRT ingest path: raw bytes from a
//! broadcast encoder flow through `TsDemuxer::feed` before any
//! codec parsing happens. A panic inside the demuxer is a
//! publisher-triggered denial of service against the entire
//! ingest surface.
//!
//! The proptest harness at `tests/proptest_ts.rs` exercises
//! 512 random inputs per invariant; this target runs
//! indefinitely under libfuzzer's coverage-guided mutation
//! and is more likely to discover corner cases in the PAT/PMT
//! section parsing, PES reassembly across packet boundaries,
//! and the sync-byte recovery path.
//!
//! Invariants asserted:
//!
//! 1. `feed` never panics on any byte sequence.
//! 2. Every yielded `PesPacket` has a non-empty payload.
//! 3. Two feeds of the same input in different chunk sizes
//!    produce the same number of PES packets (tested by
//!    feeding the full input, then feeding it in 188-byte
//!    aligned chunks).

use libfuzzer_sys::fuzz_target;
use lvqr_codec::ts::TsDemuxer;

fuzz_target!(|data: &[u8]| {
    // Single-shot feed.
    let mut demux = TsDemuxer::new();
    let packets = demux.feed(data);
    for pkt in &packets {
        assert!(!pkt.payload.is_empty());
    }

    // Chunked feed at TS packet alignment (188 bytes) to
    // exercise the cross-call buffering path.
    let mut demux2 = TsDemuxer::new();
    let mut count2 = 0usize;
    for chunk in data.chunks(188) {
        count2 += demux2.feed(chunk).len();
    }
    assert_eq!(packets.len(), count2);
});
