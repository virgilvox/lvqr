//! SCTE-35 splice_info_section construction helpers for tests.
//!
//! Hand-rolled CRC-valid splice_insert section builder per ANSI/SCTE
//! 35-2024 section 8.1. Originally inlined in
//! `crates/lvqr-cli/tests/scte35_hls_dash_e2e.rs` (session 152) as
//! `build_splice_insert_section`; session 155 lifts the helper here so
//! the existing e2e test, the new `scte35-rtmp-push` bin, and the new
//! `scte35_rtmp_push_smoke` integration test share a single source of
//! truth.
//!
//! The byte layout pinned by [`splice_insert_section_bytes`] is what
//! the workspace's relay-side parser at
//! `crates/lvqr-codec/src/scte35.rs` round-trips against, so a hex-pin
//! regression test below catches any drift in the layout.
//!
//! Splice command: `splice_insert` with `out_of_network=1`,
//! `program_splice_flag=1`, `duration_flag=1`, `splice_immediate=0`.
//! The egress side renders `SCTE35-OUT` for the resulting playlist
//! daterange.

use lvqr_codec::scte35::{CMD_SPLICE_INSERT, TABLE_ID};

/// Build a CRC-valid splice_insert splice_info_section with the
/// supplied fields.
///
/// * `event_id` -- splice_event_id, also surfaces as the relay's HLS
///   `#EXT-X-DATERANGE` ID prefix (`splice-<event_id>`) and the DASH
///   `<Event id="...">` value.
/// * `pts_90k` -- splice_time PTS at 90 kHz timescale.
/// * `duration_90k` -- break_duration at 90 kHz timescale (so 90_000
///   = 1 s, 2_700_000 = 30 s).
///
/// Splice fields are fixed for this helper:
/// `out_of_network_indicator=1`, `program_splice_flag=1`,
/// `duration_flag=1`, `splice_immediate_flag=0`.
pub fn splice_insert_section_bytes(event_id: u32, pts_90k: u64, duration_90k: u64) -> Vec<u8> {
    // 14-byte prefix per SCTE 35-2024 section 8.1: table_id,
    // section_length(2), protocol_version, encrypted/encryption_alg/pts_adj high
    // bit, pts_adj_lower(4), cw_index, tier_high(1), tier_low|scl_high,
    // scl_low, splice_command_type.
    let mut prefix = vec![
        TABLE_ID,
        0x00,
        0x00,
        0x00,
        0x00,
        0x00,
        0x00,
        0x00,
        0x00,
        0x00,
        0xFF,
        0xF0,
        0x00,
        CMD_SPLICE_INSERT,
    ];

    // splice_insert body fields per SCTE 35-2024 section 9.7.3:
    // event_id(4) + flags(1: cancel=0, reserved=7) + flags(1: out=1, program=1,
    // duration=1, immediate=0, reserved=4) + splice_time(5) + break_duration(5) +
    // unique_program_id(2) + avail_num(1) + avails_expected(1).
    let body = vec![
        (event_id >> 24) as u8,
        (event_id >> 16) as u8,
        (event_id >> 8) as u8,
        event_id as u8,
        0x7F, // cancel=0, reserved=0x7F
        0xEF, // out=1, program=1, duration=1, immediate=0, reserved=1111
        0xFE | ((pts_90k >> 32) as u8 & 0x01),
        (pts_90k >> 24) as u8,
        (pts_90k >> 16) as u8,
        (pts_90k >> 8) as u8,
        pts_90k as u8,
        0xFE | ((duration_90k >> 32) as u8 & 0x01),
        (duration_90k >> 24) as u8,
        (duration_90k >> 16) as u8,
        (duration_90k >> 8) as u8,
        duration_90k as u8,
        0x00,
        0x01, // unique_program_id
        0x00, // avail_num
        0x00, // avails_expected
    ];

    let total_minus_crc = prefix.len() + body.len() + 2;
    let total = total_minus_crc + 4;
    let section_length = total - 3;

    prefix[1] = 0x30 | ((section_length >> 8) as u8 & 0x0F);
    prefix[2] = section_length as u8;
    prefix[11] = (prefix[11] & 0xF0) | ((body.len() >> 8) as u8 & 0x0F);
    prefix[12] = body.len() as u8;

    let mut section = Vec::with_capacity(total);
    section.extend_from_slice(&prefix);
    section.extend_from_slice(&body);
    section.push(0x00); // descriptor_loop_length high
    section.push(0x00); // descriptor_loop_length low

    // CRC-32/MPEG-2 over [0..total-4].
    let crc = {
        let mut c: u32 = 0xFFFF_FFFF;
        for &b in &section {
            c ^= (b as u32) << 24;
            for _ in 0..8 {
                c = if c & 0x8000_0000 != 0 {
                    (c << 1) ^ 0x04C1_1DB7
                } else {
                    c << 1
                };
            }
        }
        c
    };
    section.push((crc >> 24) as u8);
    section.push((crc >> 16) as u8);
    section.push((crc >> 8) as u8);
    section.push(crc as u8);
    section
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Hex-pin regression test for the canonical fixture used across
    /// the workspace (event_id=0xCAFEBABE, pts=8_100_000,
    /// duration_90k=2_700_000). The bytes below are the verbatim
    /// output of `build_splice_insert_section(0xCAFEBABE, 8_100_000,
    /// 2_700_000)` captured at the session-155 helper extraction. Any
    /// drift in the layout (or the CRC computation) flips this test
    /// red before the existing `scte35_hls_dash_e2e.rs` integration
    /// test catches it through the full pipeline.
    #[test]
    fn splice_insert_section_bytes_hex_pin_for_canonical_fixture() {
        let bytes = splice_insert_section_bytes(0xCAFE_BABE, 8_100_000, 2_700_000);
        let hex: String = bytes.iter().map(|b| format!("{:02X}", b)).collect();
        assert_eq!(
            hex, "FC302500000000000000FFF01405CAFEBABE7FEFFE007B98A0FE002932E0000100000000896C5144",
            "splice_insert_section bytes drifted from the captured fixture; \
             the existing scte35_hls_dash_e2e.rs assertions on event_id 3405691582 + \
             DURATION=30.000 will catch this through the full pipeline. Update the hex \
             literal only after verifying the new bytes round-trip via \
             lvqr_codec::parse_splice_info_section."
        );
    }

    #[test]
    fn splice_insert_section_round_trips_via_codec_parser() {
        // Cross-check via the relay's own parser: the bytes the
        // helper produces must parse cleanly + report the same
        // event_id / pts / duration the caller passed in.
        let bytes = splice_insert_section_bytes(0xCAFE_BABE, 8_100_000, 2_700_000);
        let info = lvqr_codec::parse_splice_info_section(&bytes).expect("section parses");
        assert_eq!(info.event_id, Some(0xCAFE_BABE));
        assert_eq!(info.absolute_pts(), Some(8_100_000));
        assert_eq!(info.duration, Some(2_700_000));
    }
}
