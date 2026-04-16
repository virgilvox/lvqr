//! Proptest harness for the MPEG-TS demuxer.
//!
//! Invariants:
//! 1. `TsDemuxer::feed` never panics on arbitrary input.
//! 2. Every `PesPacket` the demuxer yields has a non-empty payload.
//! 3. Every yielded PTS/DTS is a valid 33-bit value (< 2^33).
//! 4. Feeding the same input in different chunk sizes yields the
//!    same PES packets (deterministic reassembly).

use lvqr_codec::ts::TsDemuxer;
use proptest::prelude::*;

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn feed_never_panics(bytes in prop::collection::vec(any::<u8>(), 0..=1024)) {
        let mut demux = TsDemuxer::new();
        let _ = demux.feed(&bytes);
    }

    #[test]
    fn feed_never_panics_large(bytes in prop::collection::vec(any::<u8>(), 0..=4096)) {
        let mut demux = TsDemuxer::new();
        let _ = demux.feed(&bytes);
    }

    #[test]
    fn yielded_pes_has_nonempty_payload(bytes in prop::collection::vec(any::<u8>(), 0..=2048)) {
        let mut demux = TsDemuxer::new();
        for pkt in demux.feed(&bytes) {
            assert!(!pkt.payload.is_empty(), "PesPacket must have non-empty payload");
        }
    }

    #[test]
    fn yielded_pts_is_33_bit(bytes in prop::collection::vec(any::<u8>(), 0..=2048)) {
        let mut demux = TsDemuxer::new();
        for pkt in demux.feed(&bytes) {
            if let Some(pts) = pkt.pts {
                assert!(pts < (1u64 << 33), "PTS must be < 2^33, got {pts}");
            }
            if let Some(dts) = pkt.dts {
                assert!(dts < (1u64 << 33), "DTS must be < 2^33, got {dts}");
            }
        }
    }

    #[test]
    fn chunked_feed_is_deterministic(
        bytes in prop::collection::vec(any::<u8>(), 0..=1024),
        chunk_size in 1usize..=256,
    ) {
        // Single feed.
        let mut demux1 = TsDemuxer::new();
        let result1 = demux1.feed(&bytes);

        // Chunked feed.
        let mut demux2 = TsDemuxer::new();
        let mut result2 = Vec::new();
        for chunk in bytes.chunks(chunk_size) {
            result2.extend(demux2.feed(chunk));
        }

        assert_eq!(result1.len(), result2.len(), "chunk size must not affect PES count");
        for (a, b) in result1.iter().zip(result2.iter()) {
            assert_eq!(a.pid, b.pid);
            assert_eq!(a.stream_type, b.stream_type);
            assert_eq!(a.pts, b.pts);
            assert_eq!(a.dts, b.dts);
            assert_eq!(a.payload, b.payload);
        }
    }
}
