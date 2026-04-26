//! Proptest harness for the SCTE-35 splice_info_section parser.
//!
//! Invariant: `lvqr_codec::parse_splice_info_section` must return a
//! structured `Result` on arbitrary input -- no panics, no
//! out-of-bounds reads, no infinite loops. SCTE-35 sections arrive
//! from publishers across the wire (RTMP onCuePoint, SRT MPEG-TS PID
//! 0x86); the parser is the boundary between attacker-controlled
//! bytes and the rest of the relay, so this property is
//! load-bearing.
//!
//! The unit-test harness in `crates/lvqr-codec/src/scte35.rs` covers
//! spec-shaped inputs + a small set of corruption cases (CRC
//! mismatch, truncation, wrong table_id). This harness covers
//! adversarial inputs the unit tests cannot enumerate (random
//! splice_command_length values, mismatched section_length vs. wire
//! bytes, splice_insert command bodies that promise more bytes than
//! they carry, time_specified_flag toggling around buffer
//! boundaries, encrypted sections, etc.).
//!
//! The libfuzzer target at
//! `fuzz/fuzz_targets/parse_scte35.rs` enforces the same invariant
//! over a much longer budget; this proptest runs on stable rust in
//! CI on every push.

use lvqr_codec::parse_splice_info_section;
use proptest::prelude::*;

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    /// Arbitrary input -- bytes can be any length, any value. The
    /// parser may reject for any number of reasons (CRC mismatch,
    /// short section_length, truncated command body, malformed
    /// encrypted_packet flag) but it must never panic.
    #[test]
    fn parse_splice_info_section_never_panics(bytes in prop::collection::vec(any::<u8>(), 0..=4096)) {
        let _ = parse_splice_info_section(&bytes);
    }

    /// Targeted shape: bytes that LOOK like a splice_info_section
    /// (table_id 0xFC, plausible section_length) but with random
    /// content beyond. Covers the codepath where the CRC verifier +
    /// command-body parser actually run instead of bouncing on the
    /// header check.
    #[test]
    fn parse_with_plausible_header_never_panics(
        section_length in 0u16..=4093u16,
        body in prop::collection::vec(any::<u8>(), 0..=4096),
    ) {
        let mut bytes = Vec::with_capacity(3 + body.len());
        bytes.push(0xFC);
        bytes.push(0x30 | ((section_length >> 8) as u8 & 0x0F));
        bytes.push(section_length as u8);
        bytes.extend_from_slice(&body);
        let _ = parse_splice_info_section(&bytes);
    }

    /// Targeted shape: random splice_command_type byte at the
    /// expected position. Covers the codepath where the parser
    /// dispatches to splice_insert / time_signal / unknown command
    /// body parsers based on the wire byte. Each of those sub-
    /// parsers has its own boundary checks that this harness
    /// exercises with arbitrary post-byte content.
    #[test]
    fn parse_with_arbitrary_command_type_never_panics(
        command_type in any::<u8>(),
        rest in prop::collection::vec(any::<u8>(), 0..=512),
    ) {
        let mut bytes = vec![
            0xFC, // table_id
            0x30, // section_syntax + private + sap_type + section_length high
            0x40, // section_length low (0x040 = 64 bytes after section_length field)
            0x00, // protocol_version
            0x00, // encrypted + encryption_alg + pts_adj high bit
            0x00, 0x00, 0x00, 0x00, // pts_adjustment lower 32 bits
            0x00, // cw_index
            0xFF, // tier high 8
            0xF0, // tier low | scl high
            0x10, // scl low (16 byte command body)
            command_type,
        ];
        bytes.extend_from_slice(&rest);
        // Pad with zeros so total is at least 17 + section_length so
        // we hit the CRC verification step rather than bouncing on
        // the EndOfStream length check.
        while bytes.len() < 67 {
            bytes.push(0x00);
        }
        let _ = parse_splice_info_section(&bytes);
    }
}
