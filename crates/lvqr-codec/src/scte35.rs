//! SCTE-35 splice_info_section parser for ad-marker passthrough.
//!
//! Parses the binary section format from ANSI/SCTE 35-2024 section 8.1.
//! The parser is intentionally minimum-viable: it extracts the timing
//! and command-type fields LVQR's HLS / DASH egress renderers need
//! (event_id, splice_time PTS, break_duration, command_type) and
//! preserves the entire section verbatim for re-emission downstream.
//! No semantic interpretation, no descriptor decoding beyond what the
//! egress wire shapes require.
//!
//! ## Wire shape
//!
//! The splice_info_section is an MPEG-2 private section with table_id
//! 0xFC. Its layout per SCTE 35-2024 section 8.1:
//!
//! ```text
//! table_id                        8 bits   (0xFC)
//! section_syntax_indicator        1 bit
//! private_indicator               1 bit
//! sap_type                        2 bits
//! section_length                  12 bits
//! protocol_version                8 bits
//! encrypted_packet                1 bit
//! encryption_algorithm            6 bits
//! pts_adjustment                  33 bits
//! cw_index                        8 bits
//! tier                            12 bits
//! splice_command_length           12 bits
//! splice_command_type             8 bits
//! splice_command()                variable
//! descriptor_loop_length          16 bits
//! splice_descriptor()*            variable
//! [if encrypted: alignment + E_CRC_32]
//! CRC_32                          32 bits
//! ```
//!
//! ## CRC verification
//!
//! Per spec the trailing 32-bit CRC is the MPEG-2 polynomial
//! (0x04C11DB7) with initial 0xFFFFFFFF, no input/output reflection,
//! no final XOR. The parser computes the CRC over every byte from
//! table_id through the byte before the trailing CRC and rejects
//! sections whose computed CRC does not match the wire value. Buggy
//! publishers that emit malformed sections are dropped at the parser
//! boundary; the integration layer counts the drops via
//! `lvqr_scte35_drops_total{reason="crc"}`.
//!
//! ## Out of scope (passthrough only)
//!
//! * No descriptor decoding (segmentation_descriptor, etc.). The raw
//!   descriptor bytes ride along inside the preserved section blob.
//! * No semantic interpretation of splice_insert / time_signal beyond
//!   surfacing the splice_time PTS that egress renderers need.
//! * No encryption support (encrypted_packet sections are accepted
//!   for passthrough but the encrypted payload is not decoded).
//! * No SCTE-104 (a different studio-side wire format).

use crate::error::CodecError;
use bytes::Bytes;

/// SCTE-35 splice_info_section table_id per spec.
pub const TABLE_ID: u8 = 0xFC;

/// splice_command_type values per SCTE 35-2024 table 7.
pub const CMD_SPLICE_NULL: u8 = 0x00;
pub const CMD_SPLICE_SCHEDULE: u8 = 0x04;
pub const CMD_SPLICE_INSERT: u8 = 0x05;
pub const CMD_TIME_SIGNAL: u8 = 0x06;
pub const CMD_BANDWIDTH_RESERVATION: u8 = 0x07;
pub const CMD_PRIVATE_COMMAND: u8 = 0xFF;

/// Parsed view of a splice_info_section, preserving the raw bytes for
/// downstream passthrough alongside the timing fields HLS / DASH
/// renderers need.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpliceInfo {
    /// `splice_command_type` field (one of `CMD_*`).
    pub command_type: u8,
    /// `pts_adjustment` field. Added to every splice_time PTS by the
    /// receiving decoder. Surfaced for completeness; the egress
    /// renderers usually want the absolute PTS via [`SpliceInfo::pts`].
    pub pts_adjustment: u64,
    /// 33-bit splice_time PTS extracted from the splice_command, when
    /// present and time_specified. None when:
    /// * the command type has no splice_time (splice_null, splice_schedule,
    ///   bandwidth_reservation, private_command),
    /// * the splice_insert is splice_immediate (no pre-roll PTS),
    /// * splice_insert.cancel_indicator is set,
    /// * splice_insert is per-component (no program splice_time),
    /// * splice_time.time_specified_flag is 0 ("immediate").
    pub pts: Option<u64>,
    /// `break_duration` from splice_insert when the duration_flag is
    /// set. None for time_signal and for splice_insert without duration.
    pub duration: Option<u64>,
    /// `splice_event_id` from splice_insert. None for command types
    /// other than splice_insert.
    pub event_id: Option<u32>,
    /// True when `splice_event_cancel_indicator` is set on a
    /// splice_insert. Egress renderers may emit a cancellation
    /// SCTE35-CMD entry.
    pub cancel: bool,
    /// True when `out_of_network_indicator` is set on a splice_insert
    /// (signals an ad break out -- "going to ad"). Drives the choice
    /// of HLS SCTE35-OUT vs SCTE35-IN attribute.
    pub out_of_network: bool,
    /// Raw splice_info_section bytes from table_id through CRC_32,
    /// preserved for passthrough into HLS DATERANGE SCTE35-* hex
    /// attributes and DASH EventStream / Event base64 bodies.
    pub raw: Bytes,
}

impl SpliceInfo {
    /// Absolute PTS of the splice = `pts_adjustment + splice_time.pts`,
    /// when both are available. Wraps modulo 2^33 per SCTE 35.
    pub fn absolute_pts(&self) -> Option<u64> {
        self.pts.map(|p| (p + self.pts_adjustment) & ((1u64 << 33) - 1))
    }
}

/// Parse a SCTE-35 splice_info_section from raw bytes.
///
/// Performs CRC_32 verification per SCTE 35-2024 section 11.1; sections
/// with a wrong trailing CRC return [`CodecError::Scte35BadCrc`].
/// Truncated or under-length sections return
/// [`CodecError::EndOfStream`].
pub fn parse_splice_info_section(bytes: &[u8]) -> Result<SpliceInfo, CodecError> {
    if bytes.len() < 17 {
        return Err(CodecError::EndOfStream {
            needed: 17,
            remaining: bytes.len(),
        });
    }
    if bytes[0] != TABLE_ID {
        return Err(CodecError::Scte35Malformed("table_id != 0xFC"));
    }

    // section_length is the number of bytes following the section_length
    // field (i.e. starting at bytes[3]) through the trailing CRC.
    let section_length = (((bytes[1] & 0x0F) as usize) << 8) | bytes[2] as usize;
    let total_len = 3 + section_length;
    if total_len > bytes.len() {
        return Err(CodecError::EndOfStream {
            needed: total_len,
            remaining: bytes.len(),
        });
    }
    if section_length < 15 {
        // Bare minimum: protocol_version(1) + flags+pts_adj(5) + cw_index(1)
        // + tier+splice_command_length(3) + splice_command_type(1) +
        // descriptor_loop_length(2) + CRC_32(4) -- before any command body.
        return Err(CodecError::Scte35Malformed("section_length too short"));
    }

    // Verify CRC_32 over [0..total_len-4].
    let crc_offset = total_len - 4;
    let computed = crc32_mpeg2(&bytes[..crc_offset]);
    let wire = ((bytes[crc_offset] as u32) << 24)
        | ((bytes[crc_offset + 1] as u32) << 16)
        | ((bytes[crc_offset + 2] as u32) << 8)
        | (bytes[crc_offset + 3] as u32);
    if computed != wire {
        return Err(CodecError::Scte35BadCrc { computed, wire });
    }

    let encrypted = bytes[4] & 0x80 != 0;
    // pts_adjustment: 1 bit at bytes[4] LSB then bytes[5..=8].
    let pts_adjustment = (((bytes[4] & 0x01) as u64) << 32)
        | ((bytes[5] as u64) << 24)
        | ((bytes[6] as u64) << 16)
        | ((bytes[7] as u64) << 8)
        | (bytes[8] as u64);

    // Layout from byte 10: tier(12) | splice_command_length(12) | splice_command_type(8).
    // Byte 10 = tier[11..4], byte 11 = tier[3..0] | scl[11..8],
    // byte 12 = scl[7..0], byte 13 = splice_command_type.
    let splice_command_length = (((bytes[11] & 0x0F) as usize) << 8) | bytes[12] as usize;
    let splice_command_type = bytes[13];

    let cmd_start = 14;
    // The 12-bit splice_command_length value 0xFFF means "the splice
    // command extends to the end of the section minus descriptor_loop"
    // (per SCTE 35 spec note); but for passthrough we never re-walk the
    // command, so we only use it as a sanity bound.
    let cmd_end = if splice_command_length == 0xFFF {
        // Heuristic: scan forward to find descriptor_loop_length such
        // that everything fits. For simplicity we delegate to the
        // command-specific parser to know its own length.
        cmd_start
    } else {
        cmd_start + splice_command_length
    };
    if cmd_end > crc_offset {
        return Err(CodecError::Scte35Malformed("splice_command extends past section"));
    }

    let mut event_id = None;
    let mut cancel = false;
    let mut out_of_network = false;
    let mut pts = None;
    let mut duration = None;

    if !encrypted {
        match splice_command_type {
            CMD_SPLICE_INSERT => {
                let parsed = parse_splice_insert(&bytes[cmd_start..crc_offset])?;
                event_id = Some(parsed.event_id);
                cancel = parsed.cancel;
                out_of_network = parsed.out_of_network;
                pts = parsed.pts;
                duration = parsed.duration;
            }
            CMD_TIME_SIGNAL => {
                pts = parse_splice_time(&bytes[cmd_start..crc_offset])?;
            }
            // splice_null, splice_schedule, bandwidth_reservation,
            // private_command: no per-event timing surfaced for v1
            // passthrough; the raw section carries everything.
            _ => {}
        }
    }

    let raw = Bytes::copy_from_slice(&bytes[..total_len]);

    Ok(SpliceInfo {
        command_type: splice_command_type,
        pts_adjustment,
        pts,
        duration,
        event_id,
        cancel,
        out_of_network,
        raw,
    })
}

/// Helper struct: parsed splice_insert command body fields.
struct ParsedSpliceInsert {
    event_id: u32,
    cancel: bool,
    out_of_network: bool,
    pts: Option<u64>,
    duration: Option<u64>,
}

/// Parse a splice_insert() command body per SCTE 35-2024 section 9.7.3.
///
/// Returns the event_id and the timing/flag fields LVQR egress needs.
/// Returns [`CodecError::EndOfStream`] when the command body is short
/// of the bytes the field layout requires.
fn parse_splice_insert(body: &[u8]) -> Result<ParsedSpliceInsert, CodecError> {
    if body.len() < 5 {
        return Err(CodecError::EndOfStream {
            needed: 5,
            remaining: body.len(),
        });
    }
    let event_id = ((body[0] as u32) << 24) | ((body[1] as u32) << 16) | ((body[2] as u32) << 8) | (body[3] as u32);
    let cancel = body[4] & 0x80 != 0;

    let mut pts = None;
    let mut duration = None;
    let mut out_of_network = false;

    if !cancel {
        if body.len() < 6 {
            return Err(CodecError::EndOfStream {
                needed: 6,
                remaining: body.len(),
            });
        }
        let flags = body[5];
        out_of_network = flags & 0x80 != 0;
        let program_splice = flags & 0x40 != 0;
        let duration_flag = flags & 0x20 != 0;
        let splice_immediate = flags & 0x10 != 0;

        let mut cursor = 6;
        if program_splice && !splice_immediate {
            let (parsed_pts, consumed) = parse_splice_time_inline(&body[cursor..])?;
            pts = parsed_pts;
            cursor += consumed;
        }
        if !program_splice {
            // Per-component splice. Skip the component loop; we do not
            // surface per-component PTS for v1 passthrough.
            if cursor >= body.len() {
                return Err(CodecError::EndOfStream {
                    needed: cursor + 1,
                    remaining: body.len(),
                });
            }
            let component_count = body[cursor] as usize;
            cursor += 1;
            for _ in 0..component_count {
                if cursor >= body.len() {
                    return Err(CodecError::EndOfStream {
                        needed: cursor + 1,
                        remaining: body.len(),
                    });
                }
                cursor += 1; // component_tag
                if !splice_immediate {
                    let (_pts, consumed) = parse_splice_time_inline(&body[cursor..])?;
                    cursor += consumed;
                }
            }
        }
        if duration_flag {
            if body.len() < cursor + 5 {
                return Err(CodecError::EndOfStream {
                    needed: cursor + 5,
                    remaining: body.len(),
                });
            }
            // break_duration: 1 bit auto_return, 6 bits reserved, 33 bits duration.
            let dur = (((body[cursor] & 0x01) as u64) << 32)
                | ((body[cursor + 1] as u64) << 24)
                | ((body[cursor + 2] as u64) << 16)
                | ((body[cursor + 3] as u64) << 8)
                | (body[cursor + 4] as u64);
            duration = Some(dur);
        }
    }

    Ok(ParsedSpliceInsert {
        event_id,
        cancel,
        out_of_network,
        pts,
        duration,
    })
}

/// Parse a splice_time() field per SCTE 35-2024 section 9.4.1.
///
/// Returns the absolute splice_time PTS when time_specified_flag is 1,
/// or None when 0 ("immediate"). Used for time_signal commands where
/// the entire body is one splice_time.
fn parse_splice_time(body: &[u8]) -> Result<Option<u64>, CodecError> {
    Ok(parse_splice_time_inline(body)?.0)
}

/// Inline version of [`parse_splice_time`] that returns both the value
/// and the byte count consumed (1 byte for time_specified_flag=0,
/// 5 bytes for time_specified_flag=1).
fn parse_splice_time_inline(body: &[u8]) -> Result<(Option<u64>, usize), CodecError> {
    if body.is_empty() {
        return Err(CodecError::EndOfStream {
            needed: 1,
            remaining: 0,
        });
    }
    let time_specified = body[0] & 0x80 != 0;
    if !time_specified {
        return Ok((None, 1));
    }
    if body.len() < 5 {
        return Err(CodecError::EndOfStream {
            needed: 5,
            remaining: body.len(),
        });
    }
    let pts = (((body[0] & 0x01) as u64) << 32)
        | ((body[1] as u64) << 24)
        | ((body[2] as u64) << 16)
        | ((body[3] as u64) << 8)
        | (body[4] as u64);
    Ok((Some(pts), 5))
}

/// CRC-32/MPEG-2: polynomial 0x04C11DB7, initial 0xFFFFFFFF, no input
/// or output reflection, no final XOR. Used by SCTE-35 sections and by
/// ISO/IEC 13818-1 PSI tables.
fn crc32_mpeg2(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &byte in data {
        crc ^= (byte as u32) << 24;
        for _ in 0..8 {
            crc = if crc & 0x8000_0000 != 0 {
                (crc << 1) ^ 0x04C1_1DB7
            } else {
                crc << 1
            };
        }
    }
    crc
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a splice_info_section programmatically and append a valid
    /// CRC_32 trailer. Used by the per-command-type test cases below.
    fn build_section(prefix: &[u8], command_body: &[u8], descriptors: &[u8]) -> Vec<u8> {
        // Total length excluding CRC = prefix(13) + command + desc_loop_len(2) + descs.
        let total_minus_crc = prefix.len() + command_body.len() + 2 + descriptors.len();
        let total = total_minus_crc + 4;
        let section_length = total - 3;
        let mut out = Vec::with_capacity(total);
        out.push(TABLE_ID);
        // section_syntax=0, private=0, sap_type=11, section_length high 4.
        out.push(0x30 | ((section_length >> 8) as u8 & 0x0F));
        out.push(section_length as u8);
        out.extend_from_slice(&prefix[3..]);
        // After the prefix slice we have the splice_command_type at the
        // last byte of the 13-byte prefix; we need to update
        // splice_command_length in [10..12] to match command_body.len().
        let cmd_len = command_body.len();
        out[11] = (out[11] & 0xF0) | ((cmd_len >> 8) as u8 & 0x0F);
        out[12] = cmd_len as u8;
        out.extend_from_slice(command_body);
        out.push((descriptors.len() >> 8) as u8);
        out.push(descriptors.len() as u8);
        out.extend_from_slice(descriptors);
        let crc = crc32_mpeg2(&out);
        out.push((crc >> 24) as u8);
        out.push((crc >> 16) as u8);
        out.push((crc >> 8) as u8);
        out.push(crc as u8);
        out
    }

    /// Default 13-byte prefix: protocol_version=0, no encryption,
    /// pts_adjustment=0, cw_index=0, tier=0xFFF, splice_command_length=0
    /// (overridden by build_section), splice_command_type set by caller.
    fn default_prefix(command_type: u8) -> Vec<u8> {
        vec![
            TABLE_ID,
            0x00, // section_length high (placeholder)
            0x00, // section_length low (placeholder)
            0x00, // protocol_version
            0x00, // encrypted=0, encryption_alg=0, pts_adj high bit
            0x00,
            0x00,
            0x00,
            0x00, // pts_adjustment lower 32 bits
            0x00, // cw_index
            0xFF, // tier high 8 bits
            0xF0, // tier low 4 bits | splice_command_length high 4 (placeholder)
            0x00, // splice_command_length low 8 (placeholder)
            command_type,
        ]
    }

    #[test]
    fn parses_splice_null() {
        let prefix = default_prefix(CMD_SPLICE_NULL);
        let bytes = build_section(&prefix, &[], &[]);
        let info = parse_splice_info_section(&bytes).expect("splice_null parses");
        assert_eq!(info.command_type, CMD_SPLICE_NULL);
        assert!(info.pts.is_none());
        assert!(info.duration.is_none());
        assert!(info.event_id.is_none());
        assert!(!info.cancel);
        assert_eq!(&info.raw[..], &bytes[..]);
    }

    #[test]
    fn parses_time_signal_with_pts() {
        let prefix = default_prefix(CMD_TIME_SIGNAL);
        // splice_time: time_specified_flag=1, reserved=63, pts_time=0x12345678.
        let pts: u64 = 0x1_2345_6789;
        // splice_time(): time_specified=1 | reserved | pts high bit, then
        // 32 lower PTS bits.
        let command_body = vec![
            0xFE | ((pts >> 32) as u8 & 0x01),
            (pts >> 24) as u8,
            (pts >> 16) as u8,
            (pts >> 8) as u8,
            pts as u8,
        ];
        let bytes = build_section(&prefix, &command_body, &[]);
        let info = parse_splice_info_section(&bytes).expect("time_signal parses");
        assert_eq!(info.command_type, CMD_TIME_SIGNAL);
        assert_eq!(info.pts, Some(pts));
    }

    #[test]
    fn parses_time_signal_immediate() {
        let prefix = default_prefix(CMD_TIME_SIGNAL);
        // splice_time: time_specified_flag=0 -> single byte with reserved bits.
        let command_body = vec![0x7F];
        let bytes = build_section(&prefix, &command_body, &[]);
        let info = parse_splice_info_section(&bytes).expect("time_signal immediate parses");
        assert!(info.pts.is_none());
    }

    #[test]
    fn parses_splice_insert_with_duration_and_pts() {
        let prefix = default_prefix(CMD_SPLICE_INSERT);
        let event_id: u32 = 0xDEAD_BEEF;
        let pts: u64 = 0x0_FFFF_FFFF;
        let duration: u64 = 0x1_0000_0000;
        // splice_insert body fields per SCTE 35-2024 section 9.7.3:
        // event_id(4) + flags(1: cancel=0, reserved=7) + flags(1: out=1,
        // program=1, duration=1, immediate=0, reserved=4) +
        // splice_time(5) + break_duration(5) + unique_program_id(2) +
        // avail_num(1) + avails_expected(1).
        let command_body = vec![
            (event_id >> 24) as u8,
            (event_id >> 16) as u8,
            (event_id >> 8) as u8,
            event_id as u8,
            0x7F, // cancel=0, reserved=0x7F (all reserved bits set per spec)
            0xEF, // out=1, program=1, duration=1, immediate=0, reserved=1111
            0xFE | ((pts >> 32) as u8 & 0x01),
            (pts >> 24) as u8,
            (pts >> 16) as u8,
            (pts >> 8) as u8,
            pts as u8,
            0xFE | ((duration >> 32) as u8 & 0x01),
            (duration >> 24) as u8,
            (duration >> 16) as u8,
            (duration >> 8) as u8,
            duration as u8,
            0x00,
            0x01, // unique_program_id
            0x00, // avail_num
            0x00, // avails_expected
        ];
        let bytes = build_section(&prefix, &command_body, &[]);
        let info = parse_splice_info_section(&bytes).expect("splice_insert parses");
        assert_eq!(info.command_type, CMD_SPLICE_INSERT);
        assert_eq!(info.event_id, Some(event_id));
        assert!(!info.cancel);
        assert!(info.out_of_network);
        assert_eq!(info.pts, Some(pts));
        assert_eq!(info.duration, Some(duration));
    }

    #[test]
    fn parses_splice_insert_cancel_no_body() {
        let prefix = default_prefix(CMD_SPLICE_INSERT);
        let event_id: u32 = 0x1234_5678;
        // splice_insert body with cancel_indicator=1 (no further fields).
        let command_body = vec![
            (event_id >> 24) as u8,
            (event_id >> 16) as u8,
            (event_id >> 8) as u8,
            event_id as u8,
            0xFF, // cancel=1 | reserved=0x7F
        ];
        let bytes = build_section(&prefix, &command_body, &[]);
        let info = parse_splice_info_section(&bytes).expect("splice_insert cancel parses");
        assert_eq!(info.event_id, Some(event_id));
        assert!(info.cancel);
        assert!(info.pts.is_none());
        assert!(info.duration.is_none());
    }

    #[test]
    fn rejects_bad_crc() {
        let prefix = default_prefix(CMD_SPLICE_NULL);
        let mut bytes = build_section(&prefix, &[], &[]);
        // Flip a bit in the section body so the CRC stops matching.
        bytes[3] ^= 0x01;
        let err = parse_splice_info_section(&bytes).expect_err("bad CRC must reject");
        assert!(matches!(err, CodecError::Scte35BadCrc { .. }), "{err:?}");
    }

    #[test]
    fn rejects_truncated() {
        let prefix = default_prefix(CMD_SPLICE_NULL);
        let bytes = build_section(&prefix, &[], &[]);
        let err = parse_splice_info_section(&bytes[..10]).expect_err("truncated must reject");
        assert!(matches!(err, CodecError::EndOfStream { .. }), "{err:?}");
    }

    #[test]
    fn rejects_wrong_table_id() {
        let prefix = default_prefix(CMD_SPLICE_NULL);
        let mut bytes = build_section(&prefix, &[], &[]);
        bytes[0] = 0x00;
        let err = parse_splice_info_section(&bytes).expect_err("wrong table_id");
        assert!(matches!(err, CodecError::Scte35Malformed(_)), "{err:?}");
    }

    #[test]
    fn pts_adjustment_round_trips() {
        // Construct a splice_null with a known pts_adjustment, then
        // verify the parsed value matches.
        let mut prefix = default_prefix(CMD_SPLICE_NULL);
        let pts_adj: u64 = 0x1_FFFF_FFFE;
        prefix[4] = (prefix[4] & 0xFE) | ((pts_adj >> 32) as u8 & 0x01);
        prefix[5] = (pts_adj >> 24) as u8;
        prefix[6] = (pts_adj >> 16) as u8;
        prefix[7] = (pts_adj >> 8) as u8;
        prefix[8] = pts_adj as u8;
        let bytes = build_section(&prefix, &[], &[]);
        let info = parse_splice_info_section(&bytes).expect("parses");
        assert_eq!(info.pts_adjustment, pts_adj);
    }

    #[test]
    fn absolute_pts_wraps_at_33_bits() {
        let info = SpliceInfo {
            command_type: CMD_TIME_SIGNAL,
            pts_adjustment: 1,
            pts: Some((1u64 << 33) - 1),
            duration: None,
            event_id: None,
            cancel: false,
            out_of_network: false,
            raw: Bytes::new(),
        };
        assert_eq!(info.absolute_pts(), Some(0));
    }

    #[test]
    fn crc32_mpeg2_known_vector() {
        // Standard test vector for MPEG-2 CRC: input "123456789" yields
        // 0x0376E6E7.
        assert_eq!(crc32_mpeg2(b"123456789"), 0x0376E6E7);
    }

    /// Adversarial proptest: drive the parser with arbitrary single-
    /// byte mutations on a valid splice_info_section and assert the
    /// outcome is one of (a) accepts the mutation if the mutated byte
    /// happened to land somewhere CRC-recoverable AND the mutation
    /// happened to keep the section's structural fields valid (rare),
    /// (b) rejects with `Scte35BadCrc` (most common -- the CRC stops
    /// matching), (c) rejects with another structural error
    /// (`Scte35Malformed`, `EndOfStream`, etc.). The contract under
    /// test is that the parser NEVER panics on adversarial input and
    /// NEVER silently accepts a mutated section without re-deriving
    /// the CRC from the mutated bytes.
    ///
    /// Closes the audit gap that the existing `rejects_bad_crc` test
    /// only ever flips one specific bit in a single section shape.
    use proptest::prelude::*;

    proptest! {
        #![proptest_config(ProptestConfig {
            cases: 256,
            ..ProptestConfig::default()
        })]
        #[test]
        fn parse_handles_arbitrary_byte_mutations_without_panic(
            byte_index in 0usize..64,
            xor_mask in 1u8..=0xFFu8,
        ) {
            // Build a valid splice_null section (the smallest variant
            // we have). 17 bytes total: 13 prefix + 0 command body +
            // 2 desc-loop-length + 0 descriptors + 4 CRC.
            let prefix = default_prefix(CMD_SPLICE_NULL);
            let original = build_section(&prefix, &[], &[]);

            // Sanity: the unmutated section must parse cleanly. If
            // this ever fails the harness is wrong, not the parser.
            assert!(
                parse_splice_info_section(&original).is_ok(),
                "harness baseline: unmutated section must parse",
            );

            // Mutate one byte at a deterministic index. byte_index is
            // clamped into the section length; xor_mask=0 would be a
            // no-op so the strategy excludes it.
            let idx = byte_index % original.len();
            let mut mutated = original.clone();
            mutated[idx] ^= xor_mask;

            // The mutated section either parses (CRC happens to still
            // match for this specific bit pattern + the mutation didn't
            // break a structural field), or fails with a documented
            // error variant. Panic-freedom is the load-bearing
            // contract: a SCTE-35 wire from an adversarial publisher
            // must never crash the parser.
            match parse_splice_info_section(&mutated) {
                Ok(_info) => {
                    // If the parser accepted, the wire must literally
                    // produce a matching CRC under the same algorithm
                    // we use for emit. This catches a regression where
                    // CRC verification is silently disabled: in that
                    // failure mode, the parser would always Ok() under
                    // mutation and the CRC check below would catch it.
                    let body = &mutated[..mutated.len() - 4];
                    let computed = crc32_mpeg2(body);
                    let wire = u32::from_be_bytes([
                        mutated[mutated.len() - 4],
                        mutated[mutated.len() - 3],
                        mutated[mutated.len() - 2],
                        mutated[mutated.len() - 1],
                    ]);
                    assert_eq!(
                        computed, wire,
                        "if parse accepted, CRC must match: idx={idx} mask={xor_mask:#x}",
                    );
                }
                Err(CodecError::Scte35BadCrc { .. })
                | Err(CodecError::Scte35Malformed(_))
                | Err(CodecError::EndOfStream { .. }) => {
                    // All documented rejection paths.
                }
                Err(other) => panic!(
                    "unexpected error variant for mutated section: {other:?} \
                     (idx={idx} mask={xor_mask:#x})",
                ),
            }
        }
    }
}
