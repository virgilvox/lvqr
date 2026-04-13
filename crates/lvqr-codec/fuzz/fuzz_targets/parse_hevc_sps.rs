#![no_main]
//! libfuzzer target for `lvqr_codec::hevc::parse_sps`.
//!
//! The SPS parser must never panic on attacker-controlled EBSP bytes.
//! The proptest harness at `tests/proptest_hevc.rs` covers the same
//! invariant on a ~200-case budget; this fuzz target runs much longer
//! and covers inputs proptest's strategy cannot reach (e.g. byte
//! sequences that happen to land on a valid exp-Golomb boundary only
//! after the emulation-prevention strip). Any crash is a regression.

use libfuzzer_sys::fuzz_target;
use lvqr_codec::hevc::parse_sps;

fuzz_target!(|data: &[u8]| {
    let _ = parse_sps(data);
});
