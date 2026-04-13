//! Proptest harness for the HEVC parser.
//!
//! Invariant: every public entry point in `lvqr_codec::hevc` must return a
//! structured `Result` on arbitrary input -- no panics, no out-of-bounds
//! reads, no infinite loops. The fuzz target planned for Tier 2.2 uses
//! the same invariant as its oracle.

use lvqr_codec::hevc::{HevcNalType, parse_nal_header, parse_sps};
use proptest::prelude::*;

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn parse_nal_header_never_panics(bytes in prop::collection::vec(any::<u8>(), 0..=32)) {
        // Either returns a NalType or a structured error. Never panics.
        let _ = parse_nal_header(&bytes);
    }

    #[test]
    fn nal_type_from_any_u8_is_total(v in any::<u8>()) {
        // Every u8 maps to some HevcNalType variant.
        let _ = HevcNalType::from_u8(v);
    }

    #[test]
    fn parse_sps_never_panics(bytes in prop::collection::vec(any::<u8>(), 0..=256)) {
        // The SPS parser may reject arbitrary input for any number of
        // reasons (overflow, implausible dimensions, unsupported sub-layers)
        // but it must never panic.
        let _ = parse_sps(&bytes);
    }
}
