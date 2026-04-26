#![no_main]
//! libfuzzer target for `lvqr_codec::parse_splice_info_section`.
//!
//! The SCTE-35 parser must never panic on attacker-controlled wire
//! bytes. The unit-test harness in `crates/lvqr-codec/src/scte35.rs`
//! covers spec-shaped happy paths + a small set of corruption cases
//! (CRC mismatch, truncation, wrong table_id); this fuzz target runs
//! much longer and exercises adversarial inputs the unit tests
//! cannot enumerate (random `splice_command_length` values,
//! mismatched `section_length` vs. wire bytes, splice_insert command
//! bodies that promise more bytes than they carry,
//! `time_specified_flag` toggling around buffer boundaries, and so
//! on).
//!
//! Any crash is a regression. Add the offending input to
//! `fuzz/corpus/parse_scte35/` so it stays covered after the fix.
//!
//! Run with:
//!
//! ```bash
//! cargo +nightly fuzz run parse_scte35
//! ```

use libfuzzer_sys::fuzz_target;
use lvqr_codec::parse_splice_info_section;

fuzz_target!(|data: &[u8]| {
    let _ = parse_splice_info_section(data);
});
