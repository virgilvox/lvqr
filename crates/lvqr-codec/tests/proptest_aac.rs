//! Proptest harness for the AAC `AudioSpecificConfig` parser.
//!
//! Invariant: `parse_asc` never panics on arbitrary input, and every
//! successful parse produces a plausible sample rate (>= 7350 Hz).

use lvqr_codec::aac::parse_asc;
use proptest::prelude::*;

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn parse_asc_never_panics(bytes in prop::collection::vec(any::<u8>(), 0..=32)) {
        let _ = parse_asc(&bytes);
    }

    #[test]
    fn successful_parse_has_plausible_sample_rate(bytes in prop::collection::vec(any::<u8>(), 0..=32)) {
        if let Ok(asc) = parse_asc(&bytes) {
            // The standard table bottoms out at 7350 Hz. Explicit
            // frequencies are clamped to non-zero by the parser. Any
            // successful parse must sit above this floor.
            prop_assert!(asc.sample_rate >= 7350, "implausible sample rate {}", asc.sample_rate);
            // Channel config is a 4-bit field so cannot exceed 15.
            prop_assert!(asc.channel_config <= 15);
        }
    }
}
