//! Property tests for the Annex B -> AVCC depacketizer.
//!
//! Slot 1 of the 5-artifact contract. The converter is called
//! with attacker-shaped bytes straight from a WebRTC peer (via
//! str0m's depacketizer), so the load-bearing property is "never
//! panics on arbitrary input". A secondary property asserts that
//! well-formed Annex B round-trips through the AVCC encoder into
//! a byte sequence whose NAL bodies match the inputs.

use lvqr_whip::{annex_b_to_avcc, split_annex_b};
use proptest::prelude::*;

proptest! {
    /// The walker must not panic on any byte string. This is the
    /// single most important property: str0m hands the buffer
    /// straight through from the wire, and any panic here is a
    /// DoS vector.
    #[test]
    fn split_annex_b_never_panics(bytes in proptest::collection::vec(any::<u8>(), 0..512)) {
        let _ = split_annex_b(&bytes);
    }

    #[test]
    fn annex_b_to_avcc_never_panics(bytes in proptest::collection::vec(any::<u8>(), 0..512)) {
        let _ = annex_b_to_avcc(&bytes);
    }

    /// For a well-formed Annex B buffer containing a sequence of
    /// NAL bodies between four-byte start codes, the AVCC output
    /// must be parseable as a sequence of length-prefixed NALs
    /// whose bodies match the inputs exactly.
    #[test]
    fn well_formed_round_trip_preserves_bodies(
        nals in proptest::collection::vec(proptest::collection::vec(any::<u8>(), 1..40), 1..8)
    ) {
        let mut annex_b = Vec::new();
        for nal in &nals {
            annex_b.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
            annex_b.extend_from_slice(nal);
        }

        let avcc = annex_b_to_avcc(&annex_b);
        let recovered = parse_avcc(&avcc);

        prop_assert_eq!(recovered.len(), nals.len());
        for (a, b) in recovered.iter().zip(nals.iter()) {
            prop_assert_eq!(a, b);
        }
    }
}

/// Minimal AVCC walker used only by the proptest round-trip
/// property. Stops at the first malformed entry rather than
/// trying to resync.
fn parse_avcc(buf: &[u8]) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    let mut i = 0;
    while i + 4 <= buf.len() {
        let len = u32::from_be_bytes([buf[i], buf[i + 1], buf[i + 2], buf[i + 3]]) as usize;
        i += 4;
        if len == 0 || i + len > buf.len() {
            break;
        }
        out.push(buf[i..i + len].to_vec());
        i += len;
    }
    out
}
