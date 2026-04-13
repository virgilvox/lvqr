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
            self.general_profile_compatibility_flags,
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
/// Currently only supports `sps_max_sub_layers_minus1 == 0` (single
/// sub-layer streams, which is every consumer HEVC stream LVQR has seen
/// in practice). Multi-sub-layer streams return
/// [`CodecError::Unsupported`]; add support when a user reports it.
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

    // Skip sub-layer profile/level flags if any. Each sub-layer contributes
    // a profile_present_flag + level_present_flag (2 bits) plus reserved
    // padding and optional sub-layer PTL bodies. Full support is out of
    // scope for LVQR; bail early with Unsupported on any multi-layer
    // stream so callers know to plug in a more complete parser.
    if max_sub_layers_minus1 > 0 {
        return Err(CodecError::Unsupported("HEVC SPS with sps_max_sub_layers_minus1 > 0"));
    }

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

#[cfg(test)]
mod tests {
    use super::*;

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
        // profile 1 (Main), compat flags 0x60000000, level 93 (level 3.1).
        assert_eq!(sps.codec_string(), "hev1.1.60000000.L93.B0");
    }
}
