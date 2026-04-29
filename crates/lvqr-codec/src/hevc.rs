//! HEVC (H.265) NAL unit classification and minimal SPS parsing.
//!
//! The goal of this module is NOT to fully decode an HEVC SPS -- that
//! would require scaling lists, VUI parameters, HRD parameters, and the
//! full sub-layer ladder, which only matter for pixel-accurate decoding.
//! LVQR never decodes HEVC; it only needs to:
//!
//! 1. Identify NAL unit types so the ingest layer can distinguish VPS,
//!    SPS, PPS, and slice NALUs.
//! 2. Extract `general_profile_idc`, `general_tier_flag`,
//!    `general_level_idc`, and `pic_width/height_in_luma_samples` from
//!    an SPS so the CMAF muxer can produce the correct `hvc1` or `hev1`
//!    sample entry and the catalog can emit a proper codec string.
//! 3. Never panic on arbitrary input. The proptest harness in
//!    `tests/proptest_hevc.rs` enforces this.
//!
//! Anything beyond those three goals is explicitly out of scope and will
//! return [`CodecError::Unsupported`] so callers know to fall back to a
//! more complete parser if one becomes necessary.

use crate::bit_reader::{BitReader, rbsp_from_ebsp};
use crate::error::CodecError;

/// HEVC NAL unit types LVQR cares about. The raw u8 is the value from
/// `nal_unit_type` (6 bits) in the NAL unit header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum HevcNalType {
    TrailN = 0,
    TrailR = 1,
    IdrWRadl = 19,
    IdrNLp = 20,
    CraNut = 21,
    Vps = 32,
    Sps = 33,
    Pps = 34,
    AudNut = 35,
    EosNut = 36,
    EobNut = 37,
    FdNut = 38,
    PrefixSeiNut = 39,
    SuffixSeiNut = 40,
    /// Any type not covered by the LVQR-relevant enum variants.
    Other(u8),
}

impl HevcNalType {
    pub fn from_u8(v: u8) -> Self {
        match v {
            0 => Self::TrailN,
            1 => Self::TrailR,
            19 => Self::IdrWRadl,
            20 => Self::IdrNLp,
            21 => Self::CraNut,
            32 => Self::Vps,
            33 => Self::Sps,
            34 => Self::Pps,
            35 => Self::AudNut,
            36 => Self::EosNut,
            37 => Self::EobNut,
            38 => Self::FdNut,
            39 => Self::PrefixSeiNut,
            40 => Self::SuffixSeiNut,
            other => Self::Other(other),
        }
    }

    pub fn is_keyframe(self) -> bool {
        matches!(self, Self::IdrWRadl | Self::IdrNLp | Self::CraNut)
    }
}

/// Parse a 2-byte HEVC NAL unit header and return the NAL type.
///
/// Layout (bit 0 = MSB of byte 0):
///
/// ```text
///  0 | forbidden_zero_bit
///  1..=6 | nal_unit_type
///  7..=12 | nuh_layer_id
///  13..=15 | nuh_temporal_id_plus1
/// ```
pub fn parse_nal_header(bytes: &[u8]) -> Result<HevcNalType, CodecError> {
    if bytes.len() < 2 {
        return Err(CodecError::EndOfStream {
            needed: 2,
            remaining: bytes.len(),
        });
    }
    let nal_type = (bytes[0] >> 1) & 0x3F;
    Ok(HevcNalType::from_u8(nal_type))
}

/// Minimal HEVC SPS information extracted by [`parse_sps`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HevcSps {
    pub general_profile_space: u8,
    pub general_tier_flag: bool,
    pub general_profile_idc: u8,
    pub general_profile_compatibility_flags: u32,
    pub general_level_idc: u8,
    pub chroma_format_idc: u32,
    pub pic_width_in_luma_samples: u32,
    pub pic_height_in_luma_samples: u32,
}

impl HevcSps {
    /// RFC 6381 codec string for the `hev1` / `hvc1` sample entry.
    ///
    /// Format (per ISO/IEC 14496-15 annex E and RFC 7798 §3.2):
    ///
    /// ```text
    ///   hev1.<profile>.<compat-reversed>.L<level>.<constraint>
    /// ```
    ///
    /// `general_profile_compatibility_flags` are emitted with bit
    /// order reversed (the spec's flag[0] becomes bit 0 of the
    /// encoded value) per ISO/IEC 14496-15 annex E. Trailing zero
    /// hex digits are stripped naturally by `format!("{:X}", n)`,
    /// matching Apple's HLS authoring spec examples like
    /// `"hev1.1.6.L93.B0"` for Main-compatible profiles. Without
    /// the reversal Shaka Player + dash.js reject the string with
    /// `CODEC_NOT_SUPPORTED`; iOS Safari is more forgiving because
    /// it leans on the `hvc1` sample-entry tag rather than parsing
    /// the codec string.
    ///
    /// LVQR always emits `hev1` (byte-stream compatible) rather than
    /// `hvc1` (length-prefixed parameter-set compatible). The constraint
    /// flags are not currently captured so the trailing `.B0` reflects
    /// "no constraint info known".
    pub fn codec_string(&self) -> String {
        format!(
            "hev1.{}{}.{:X}.L{}.B0",
            match self.general_profile_space {
                0 => String::new(),
                1 => "A".to_string(),
                2 => "B".to_string(),
                3 => "C".to_string(),
                _ => String::new(),
            },
            self.general_profile_idc,
            self.general_profile_compatibility_flags.reverse_bits(),
            self.general_level_idc,
        )
    }
}

/// Parse an HEVC SPS NAL unit payload (not including the 2-byte NAL header).
///
/// Caller supplies the EBSP bytes *after* the NAL unit header. This
/// function strips emulation-prevention bytes internally.
///
/// Scope: extracts profile / tier / level / chroma format / resolution.
/// Handles `sps_max_sub_layers_minus1` values in `0..=6` (the full HEVC
/// range), including the sub-layer profile/level present flag loop, the
/// reserved-zero-2-bits padding for layers `max_sub_layers_minus1..8`,
/// and the per-sub-layer PTL body skip. LVQR does not expose the
/// per-sub-layer PTL data because it only needs the general PTL to emit
/// a codec string; the sub-layer bits are parsed for completeness and
/// discarded.
pub fn parse_sps(payload: &[u8]) -> Result<HevcSps, CodecError> {
    let rbsp = rbsp_from_ebsp(payload);
    let mut r = BitReader::new(&rbsp);

    // sps_video_parameter_set_id u(4)
    let _vps_id = r.read_bits(4)?;
    // sps_max_sub_layers_minus1 u(3)
    let max_sub_layers_minus1 = r.read_bits(3)? as u8;
    if max_sub_layers_minus1 > 6 {
        return Err(CodecError::MalformedSps("sps_max_sub_layers_minus1 > 6"));
    }
    // sps_temporal_id_nesting_flag u(1)
    let _tid_nesting = r.read_bit()?;

    // profile_tier_level( 1, sps_max_sub_layers_minus1 )
    let sps = parse_ptl_general(&mut r)?;
    parse_ptl_sublayers(&mut r, max_sub_layers_minus1)?;

    // sps_seq_parameter_set_id ue(v)
    let _sps_id = r.read_ue_v()?;
    // chroma_format_idc ue(v)
    let chroma_format_idc = r.read_ue_v()?;
    if chroma_format_idc > 3 {
        return Err(CodecError::MalformedSps("chroma_format_idc > 3"));
    }
    if chroma_format_idc == 3 {
        // separate_colour_plane_flag u(1)
        let _ = r.read_bit()?;
    }
    // pic_width_in_luma_samples ue(v)
    let pic_width_in_luma_samples = r.read_ue_v()?;
    // pic_height_in_luma_samples ue(v)
    let pic_height_in_luma_samples = r.read_ue_v()?;
    // Sanity clamp. 16384 is the HEVC spec hard ceiling and anything
    // bigger is either garbage or way outside LVQR's target market.
    if pic_width_in_luma_samples == 0
        || pic_height_in_luma_samples == 0
        || pic_width_in_luma_samples > 16384
        || pic_height_in_luma_samples > 16384
    {
        return Err(CodecError::MalformedSps("implausible SPS dimensions"));
    }

    Ok(HevcSps {
        general_profile_space: sps.general_profile_space,
        general_tier_flag: sps.general_tier_flag,
        general_profile_idc: sps.general_profile_idc,
        general_profile_compatibility_flags: sps.general_profile_compatibility_flags,
        general_level_idc: sps.general_level_idc,
        chroma_format_idc,
        pic_width_in_luma_samples,
        pic_height_in_luma_samples,
    })
}

/// Intermediate: just the general portion of profile_tier_level.
#[derive(Debug, Clone)]
struct PtlGeneral {
    general_profile_space: u8,
    general_tier_flag: bool,
    general_profile_idc: u8,
    general_profile_compatibility_flags: u32,
    general_level_idc: u8,
}

/// Parse the general_profile_tier_level block. Total width is 96 bits:
///
/// ```text
///   general_profile_space            2
///   general_tier_flag                1
///   general_profile_idc              5
///   general_profile_compatibility    32
///   progressive/interlaced/non_packed/frame_only  4
///   constraint flags (incl inbld)    44
///   general_level_idc                8
///   = 96
/// ```
fn parse_ptl_general(r: &mut BitReader<'_>) -> Result<PtlGeneral, CodecError> {
    let general_profile_space = r.read_bits(2)? as u8;
    let general_tier_flag = r.read_bit()? == 1;
    let general_profile_idc = r.read_bits(5)? as u8;
    let general_profile_compatibility_flags = r.read_bits(32)?;
    // 4 source flags
    let _progressive = r.read_bit()?;
    let _interlaced = r.read_bit()?;
    let _non_packed = r.read_bit()?;
    let _frame_only = r.read_bit()?;
    // 43 constraint flags + 1 inbld. Skip wholesale.
    r.skip_bits(44)?;
    let general_level_idc = r.read_bits(8)? as u8;
    Ok(PtlGeneral {
        general_profile_space,
        general_tier_flag,
        general_profile_idc,
        general_profile_compatibility_flags,
        general_level_idc,
    })
}

/// Parse the sub-layer portion of profile_tier_level for the SPS.
///
/// Per HEVC spec (T-REC-H.265 §7.3.3) with `profilePresentFlag == 1`:
///
/// ```text
///   for( i = 0; i < maxNumSubLayersMinus1; i++ ) {
///     sub_layer_profile_present_flag[i] u(1)
///     sub_layer_level_present_flag[i]   u(1)
///   }
///   if( maxNumSubLayersMinus1 > 0 )
///     for( i = maxNumSubLayersMinus1; i < 8; i++ )
///       reserved_zero_2bits[i] u(2)
///   for( i = 0; i < maxNumSubLayersMinus1; i++ ) {
///     if( sub_layer_profile_present_flag[i] ) <88 bits of PTL body>
///     if( sub_layer_level_present_flag[i] )   u(8) level_idc
///   }
/// ```
///
/// The sub-layer PTL body has the same 88-bit layout as the general
/// PTL minus the trailing `general_level_idc` byte: profile_space(2) +
/// tier_flag(1) + profile_idc(5) + compat_flags(32) + source_flags(4) +
/// constraint_flags(43) + inbld(1) = 88. LVQR does not surface per-
/// sub-layer data; the bits are consumed so the bit cursor ends up in
/// the right place for the SPS fields that follow.
fn parse_ptl_sublayers(r: &mut BitReader<'_>, max_sub_layers_minus1: u8) -> Result<(), CodecError> {
    if max_sub_layers_minus1 == 0 {
        return Ok(());
    }
    let n = max_sub_layers_minus1 as usize;
    // Collect presence flags for the per-sub-layer pass below. Capacity
    // is at most 6 entries (max_sub_layers_minus1 is bounded to 6 by the
    // caller) so a small fixed array avoids any heap traffic.
    let mut profile_present = [false; 6];
    let mut level_present = [false; 6];
    for i in 0..n {
        profile_present[i] = r.read_bit()? == 1;
        level_present[i] = r.read_bit()? == 1;
    }
    // Reserved padding: layers [max_sub_layers_minus1, 8) each contribute
    // a reserved_zero_2bits field. Total padding width is
    // 2 * (8 - max_sub_layers_minus1) bits.
    let padding_bits = 2 * (8 - n);
    r.skip_bits(padding_bits)?;
    for i in 0..n {
        if profile_present[i] {
            // 88-bit sub-layer PTL body. Split into two skip calls to
            // stay within the 32-bit read_bits budget if we ever wanted
            // to inspect them; skip_bits takes a usize so a single call
            // is fine.
            r.skip_bits(88)?;
        }
        if level_present[i] {
            r.skip_bits(8)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal MSB-first bit writer used only by the synthetic-SPS
    /// fixture builder below. Kept inside the test module because it
    /// has no production consumers and should not grow one.
    struct BitWriter {
        bytes: Vec<u8>,
        bit_pos: usize,
    }

    impl BitWriter {
        fn new() -> Self {
            Self {
                bytes: Vec::new(),
                bit_pos: 0,
            }
        }

        fn write_bit(&mut self, b: u32) {
            if self.bit_pos % 8 == 0 {
                self.bytes.push(0);
            }
            let byte_idx = self.bit_pos / 8;
            let shift = 7 - (self.bit_pos % 8);
            self.bytes[byte_idx] |= ((b & 1) as u8) << shift;
            self.bit_pos += 1;
        }

        fn write_bits(&mut self, value: u32, n: u8) {
            for i in (0..n).rev() {
                self.write_bit((value >> i) & 1);
            }
        }

        fn write_ue(&mut self, value: u32) {
            // value v encoded as k zeros, 1 bit, k-bit suffix where
            //   (1 << k) - 1 + suffix = v. Equivalently, write (v+1) as
            //   a minimal-width binary code prefixed by enough leading
            //   zeros to pad it out.
            let code = (value as u64) + 1;
            let mut k = 0u8;
            while (1u64 << (k + 1)) <= code {
                k += 1;
            }
            for _ in 0..k {
                self.write_bit(0);
            }
            self.write_bits(code as u32, k + 1);
        }

        fn into_bytes(self) -> Vec<u8> {
            self.bytes
        }
    }

    /// Build a synthetic HEVC SPS with the given `max_sub_layers_minus1`
    /// and a full general PTL body. Used by the positive decode tests.
    fn build_synthetic_sps(max_sub_layers_minus1: u8) -> Vec<u8> {
        let mut w = BitWriter::new();
        // sps_video_parameter_set_id u(4)
        w.write_bits(0, 4);
        // sps_max_sub_layers_minus1 u(3)
        w.write_bits(max_sub_layers_minus1 as u32, 3);
        // sps_temporal_id_nesting_flag u(1)
        w.write_bit(1);
        // general PTL: 96 bits
        w.write_bits(0, 2); // profile_space
        w.write_bit(0); // tier_flag
        w.write_bits(1, 5); // profile_idc = 1 (Main)
        w.write_bits(0x60000000, 32); // compat flags
        // 4 source flags
        w.write_bit(0);
        w.write_bit(0);
        w.write_bit(0);
        w.write_bit(0);
        // 44 constraint + inbld bits
        for _ in 0..44 {
            w.write_bit(0);
        }
        w.write_bits(93, 8); // general_level_idc = 93
        // sub_layer_profile_present / level_present flags
        for _ in 0..max_sub_layers_minus1 {
            w.write_bit(1); // profile_present
            w.write_bit(1); // level_present
        }
        if max_sub_layers_minus1 > 0 {
            let padding = 2 * (8 - max_sub_layers_minus1 as usize);
            for _ in 0..padding {
                w.write_bit(0);
            }
        }
        // Per-sub-layer PTL body (88 bits) + level_idc (8 bits) since
        // both flags are set above.
        for _ in 0..max_sub_layers_minus1 {
            for _ in 0..88 {
                w.write_bit(0);
            }
            w.write_bits(60, 8); // sub_layer_level_idc = 60
        }
        // sps_seq_parameter_set_id ue(v) = 0
        w.write_ue(0);
        // chroma_format_idc ue(v) = 1 (4:2:0)
        w.write_ue(1);
        // pic_width_in_luma_samples ue(v) = 1920
        w.write_ue(1920);
        // pic_height_in_luma_samples ue(v) = 1080
        w.write_ue(1080);
        w.into_bytes()
    }

    #[test]
    fn nal_type_round_trip() {
        assert_eq!(HevcNalType::from_u8(33), HevcNalType::Sps);
        assert_eq!(HevcNalType::from_u8(19), HevcNalType::IdrWRadl);
        assert!(HevcNalType::from_u8(19).is_keyframe());
        assert!(HevcNalType::from_u8(21).is_keyframe());
        assert!(!HevcNalType::from_u8(1).is_keyframe());
        assert_eq!(HevcNalType::from_u8(63), HevcNalType::Other(63));
    }

    #[test]
    fn nal_header_rejects_short_input() {
        assert!(matches!(parse_nal_header(&[0x42]), Err(CodecError::EndOfStream { .. })));
    }

    #[test]
    fn nal_header_decodes_sps_byte() {
        // SPS = 33 in the top 6 bits of byte 0 (after the forbidden-zero
        // bit). Construct 0 << 7 | 33 << 1 = 0x42.
        assert_eq!(parse_nal_header(&[0x42, 0x01]).unwrap(), HevcNalType::Sps);
    }

    #[test]
    fn parse_sps_rejects_empty_payload() {
        assert!(parse_sps(&[]).is_err());
    }

    #[test]
    fn parse_sps_rejects_all_zeros() {
        // 64 bytes of zero: the parser advances through the fixed-width
        // PTL (all-zero => profile_idc=0, level_idc=0), then tries to
        // read `sps_seq_parameter_set_id` as an exp-Golomb code. An
        // unbounded run of zero bits triggers the Golomb overflow guard.
        // Any structured error is acceptable here; the invariant is "no
        // panic".
        let zeros = vec![0u8; 64];
        assert!(parse_sps(&zeros).is_err());
    }

    #[test]
    fn parse_sps_decodes_synthetic_single_sublayer() {
        let sps_bytes = build_synthetic_sps(0);
        let sps = parse_sps(&sps_bytes).expect("single-sublayer SPS should parse");
        assert_eq!(sps.general_profile_idc, 1);
        assert_eq!(sps.general_profile_compatibility_flags, 0x60000000);
        assert_eq!(sps.general_level_idc, 93);
        assert_eq!(sps.chroma_format_idc, 1);
        assert_eq!(sps.pic_width_in_luma_samples, 1920);
        assert_eq!(sps.pic_height_in_luma_samples, 1080);
        assert_eq!(sps.codec_string(), "hev1.1.6.L93.B0");
    }

    #[test]
    fn parse_sps_decodes_two_sublayer_stream() {
        // Exercises the sub-layer profile/level present flag loop, the
        // reserved-zero-2-bits padding, and the per-sub-layer PTL body
        // skip. A two-sublayer SPS is the common case for any HEVC
        // stream that ships temporal scalability; the single-sublayer
        // path above stays as the common-case regression guard.
        let sps_bytes = build_synthetic_sps(1);
        let sps = parse_sps(&sps_bytes).expect("two-sublayer SPS should parse");
        assert_eq!(sps.general_profile_idc, 1);
        assert_eq!(sps.general_level_idc, 93);
        assert_eq!(sps.pic_width_in_luma_samples, 1920);
        assert_eq!(sps.pic_height_in_luma_samples, 1080);
    }

    #[test]
    fn parse_sps_decodes_max_sublayer_stream() {
        // Boundary: max_sub_layers_minus1 = 6 is the spec ceiling (7 sub
        // layers). Padding shrinks to 2 * (8 - 6) = 4 bits. Every sub-
        // layer sets both presence flags, producing 6 * (88 + 8) = 576
        // bits of sub-layer PTL body to skip.
        let sps_bytes = build_synthetic_sps(6);
        let sps = parse_sps(&sps_bytes).expect("max-sublayer SPS should parse");
        assert_eq!(sps.pic_width_in_luma_samples, 1920);
        assert_eq!(sps.pic_height_in_luma_samples, 1080);
    }

    #[test]
    fn parse_sps_decodes_real_x265_single_sublayer() {
        // Real SPS captured from `ffmpeg 8.1 -c:v libx265 -preset ultrafast`
        // encoding a 320x240 testsrc2 clip. Level 60 = HEVC level 2.0
        // is what x265 picks as the smallest level that fits this
        // resolution + frame rate. Single sub-layer because x265's
        // default temporal-layers mode does not flip
        // sps_max_sub_layers_minus1; the multi-sub-layer positive
        // coverage lives in the synthetic tests above.
        //
        // The same byte blob lives in the conformance fixture corpus
        // at `lvqr-conformance/fixtures/codec/hevc-sps-x265-main-320x240.bin`
        // with a sidecar TOML naming the full expected decoded
        // values. `lvqr-codec/tests/conformance_codec.rs` asserts
        // every field of the parser output matches the sidecar;
        // this unit test is a smaller, faster redundant check that
        // fails loudly if the hand-rolled parser's general PTL path
        // regresses, without needing the conformance dev-dep.
        let hex = "0101600000030090000003000003003ca00a080f165ba4a4c2f0168080000003008000000f0400";
        let sps_bytes: Vec<u8> = (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
            .collect();
        let sps = parse_sps(&sps_bytes).expect("real x265 SPS should parse");
        assert_eq!(sps.general_profile_idc, 1); // Main
        assert_eq!(sps.general_level_idc, 60); // HEVC level 2.0
        assert_eq!(sps.chroma_format_idc, 1); // 4:2:0
        assert_eq!(sps.pic_width_in_luma_samples, 320);
        assert_eq!(sps.pic_height_in_luma_samples, 240);
        assert_eq!(sps.codec_string(), "hev1.1.6.L60.B0");
    }

    #[test]
    fn codec_string_format() {
        let sps = HevcSps {
            general_profile_space: 0,
            general_tier_flag: false,
            general_profile_idc: 1,
            general_profile_compatibility_flags: 0x60000000,
            general_level_idc: 93,
            chroma_format_idc: 1,
            pic_width_in_luma_samples: 1920,
            pic_height_in_luma_samples: 1080,
        };
        // profile 1 (Main), compat flags 0x60000000 (Main + Main10
        // compatible), level 93 (level 3.1). The reverse-bit-ordered
        // hex of 0x60000000 is 0x6, so the third field is "6".
        assert_eq!(sps.codec_string(), "hev1.1.6.L93.B0");
    }

    #[test]
    fn codec_string_reverses_compat_flag_bit_order() {
        // Lock the bit-reversal explicitly so a future refactor that
        // drops the `.reverse_bits()` call regresses with a named
        // expectation rather than via the indirect Main-compat
        // assertion. flag[0..=3] all set in the spec's MSB-first
        // ordering reads as 0xF0000000 from `read_bits(32)`; reversed
        // those flags land in bits 0..=3 of the encoded value, so
        // the codec string carries 0xF.
        let sps = HevcSps {
            general_profile_space: 0,
            general_tier_flag: false,
            general_profile_idc: 4,
            general_profile_compatibility_flags: 0xF000_0000,
            general_level_idc: 120,
            chroma_format_idc: 1,
            pic_width_in_luma_samples: 1920,
            pic_height_in_luma_samples: 1080,
        };
        assert_eq!(sps.codec_string(), "hev1.4.F.L120.B0");
    }
}
