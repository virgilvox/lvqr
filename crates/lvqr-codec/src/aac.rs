//! AAC `AudioSpecificConfig` (ASC) parser.
//!
//! The existing `lvqr-ingest::remux::fmp4::esds` writer assumes a 2-byte
//! ASC and hand-rolls MPEG-4 descriptor lengths with a single-byte prefix.
//! That works for AAC-LC but breaks on HE-AAC, HE-AAC v2, xHE-AAC, and
//! any config that uses the `sampling_frequency_index == 15` explicit
//! frequency escape. This module is the hardened parser that future
//! `lvqr-codec`-backed muxers will call to produce correct sample entries.
//!
//! Reference: ISO/IEC 14496-3 §1.6.2 (`AudioSpecificConfig`).
//!
//! Scope:
//!
//! * Object type decoding with the 5-bit base + 6-bit escape (object
//!   types 32..=63).
//! * Explicit sampling frequency when `samplingFrequencyIndex == 15`.
//! * Channel configuration decoding.
//! * Extension object type signaling for HE-AAC (SBR) and HE-AAC v2 (PS).
//!
//! Out of scope:
//!
//! * The full `GASpecificConfig` decoder. We only need to know enough to
//!   build an fMP4 sample entry; the ASC bytes themselves are written
//!   verbatim into the `esds` box. Scalable, CELP, HVXC, TwinVQ, and
//!   structured-audio payloads are parsed only up to the object-type and
//!   sample-rate fields.

use crate::bit_reader::BitReader;
use crate::error::CodecError;

/// Sampling frequency table indexed by `samplingFrequencyIndex`
/// (ISO/IEC 14496-3 Table 1.16). Index 15 is a sentinel meaning
/// "frequency follows explicitly as a 24-bit value".
pub const AAC_SAMPLE_FREQUENCIES: [u32; 13] = [
    96000, 88200, 64000, 48000, 44100, 32000, 24000, 22050, 16000, 12000, 11025, 8000, 7350,
];

/// Decoded AAC AudioSpecificConfig.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioSpecificConfig {
    /// MPEG-4 audio object type (AOT). 2 = AAC-LC, 5 = HE-AAC (SBR),
    /// 29 = HE-AAC v2 (PS), 42 = xHE-AAC.
    pub object_type: u8,
    /// Sampling rate in Hz.
    pub sample_rate: u32,
    /// Channel configuration (0 = PCE follows; 1..=7 = mapped layout).
    pub channel_config: u8,
    /// True if the config explicitly signals SBR (HE-AAC).
    pub sbr_present: bool,
    /// True if the config explicitly signals PS (HE-AAC v2).
    pub ps_present: bool,
}

impl AudioSpecificConfig {
    /// RFC 6381 codec string for the `mp4a` sample entry.
    /// Format: `mp4a.40.<object_type>`.
    pub fn codec_string(&self) -> String {
        format!("mp4a.40.{}", self.object_type)
    }
}

/// Parse an ASC from raw bytes. Returns a structured error on any
/// malformed input; never panics.
pub fn parse_asc(bytes: &[u8]) -> Result<AudioSpecificConfig, CodecError> {
    if bytes.is_empty() {
        return Err(CodecError::EndOfStream {
            needed: 1,
            remaining: 0,
        });
    }
    let mut r = BitReader::new(bytes);
    let object_type = read_object_type(&mut r)?;
    let sample_rate = read_sample_rate(&mut r)?;
    let channel_config = r.read_bits(4)? as u8;

    // Detect SBR/PS extension. Two forms:
    //
    // 1. Explicit hierarchical signalling: object_type == 5 (SBR) or 29
    //    (PS) means the config describes an extension over an AAC-LC
    //    payload. In both cases the extensionSamplingFrequencyIndex
    //    follows immediately and then the actual audioObjectType of the
    //    downstream config is read.
    // 2. Implicit (legacy): any object type may be followed by trailing
    //    bits that signal SBR; we do not try to detect that here because
    //    it requires scanning the GASpecificConfig payload.
    let (sbr_present, ps_present, base_object_type) = match object_type {
        5 | 29 => {
            // extensionSamplingFrequencyIndex u(4) [ + explicit u(24) ]
            let ext_sfi = r.read_bits(4)? as u8;
            if ext_sfi == 15 {
                // extensionSamplingFrequency u(24)
                let _ = r.read_bits(24)?;
            }
            // The downstream audioObjectType (typically 2 = AAC-LC)
            let downstream = read_object_type(&mut r)?;
            let ps = object_type == 29;
            (true, ps, downstream)
        }
        _ => (false, false, object_type),
    };

    Ok(AudioSpecificConfig {
        object_type: if sbr_present { object_type } else { base_object_type },
        sample_rate,
        channel_config,
        sbr_present,
        ps_present,
    })
}

/// Read a 5-bit audio object type with the 6-bit escape.
///
/// The wire encoding is:
///
/// ```text
///   audioObjectType u(5)
///   if audioObjectType == 31:
///       audioObjectType = 32 + audioObjectTypeExt u(6)
/// ```
fn read_object_type(r: &mut BitReader<'_>) -> Result<u8, CodecError> {
    let base = r.read_bits(5)? as u8;
    if base == 31 {
        let ext = r.read_bits(6)? as u8;
        Ok(32 + ext)
    } else {
        Ok(base)
    }
}

/// Read the sampling frequency: a 4-bit index into the standard table,
/// or index 15 which means "explicit 24-bit frequency follows".
fn read_sample_rate(r: &mut BitReader<'_>) -> Result<u32, CodecError> {
    let sfi = r.read_bits(4)? as u8;
    if sfi == 15 {
        let freq = r.read_bits(24)?;
        // Reject implausibly low explicit rates. The standard table
        // bottoms out at 7350 Hz (`AAC_SAMPLE_FREQUENCIES[12]`), and
        // no real-world AAC encoder produces anything below that;
        // accepting rate=1 Hz just because the 24-bit field happened
        // to decode that way lets attacker-shaped input through the
        // codec parser and produces nonsense downstream (init
        // segment timescale, LL-HLS partial duration reporting).
        const MIN_PLAUSIBLE_HZ: u32 = 7350;
        if freq < MIN_PLAUSIBLE_HZ {
            return Err(CodecError::MalformedAsc("explicit sample rate below 7350 Hz"));
        }
        Ok(freq)
    } else {
        AAC_SAMPLE_FREQUENCIES
            .get(sfi as usize)
            .copied()
            .ok_or(CodecError::MalformedAsc("sampling_frequency_index out of range"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_aac_lc_stereo_48khz() {
        // AOT=2 (5 bits = 00010), sfi=3 (4 bits = 0011), channel=2 (4 bits = 0010), pad 3 bits
        // concatenated bitstream: 00010 0011 0010 000
        // as bytes: 00010001 10010000 = 0x11 0x90
        let asc = parse_asc(&[0x11, 0x90]).unwrap();
        assert_eq!(asc.object_type, 2);
        assert_eq!(asc.sample_rate, 48000);
        assert_eq!(asc.channel_config, 2);
        assert!(!asc.sbr_present);
        assert!(!asc.ps_present);
        assert_eq!(asc.codec_string(), "mp4a.40.2");
    }

    #[test]
    fn parse_legacy_lvqr_aac_lc_stereo_44k() {
        // The 2-byte ASC that lvqr-ingest's existing esds writer hard-codes
        // (see HANDOFF.md session 3 notes): [0x12, 0x10]. Decode:
        //   0001 0010 0001 0000
        //   AOT  = 00010 = 2     (AAC-LC)
        //   sfi  = 0100  = 4     -> 44100 Hz
        //   chan = 0010  = 2     (stereo)
        //   pad  = 000
        // This test pins the interpretation of that magic pair so future
        // refactors of lvqr-ingest cannot silently drift from it.
        let asc = parse_asc(&[0x12, 0x10]).unwrap();
        assert_eq!(asc.object_type, 2);
        assert_eq!(asc.sample_rate, 44100);
        assert_eq!(asc.channel_config, 2);
    }

    #[test]
    fn parse_he_aac_signals_sbr() {
        // AOT=5 (SBR), ext sfi=3 (48kHz), downstream AOT=2 (AAC-LC),
        // sfi=3 (48kHz), channel=2
        //
        //   00101 0011 00010 0011 0010 0
        //   = 0010 1001 1000 1000 1100 1000
        //   = 0x29 0x88 0xC8 (last byte has 3 significant bits)
        let asc = parse_asc(&[0x29, 0x88, 0xC8]).unwrap();
        assert!(asc.sbr_present);
        assert!(!asc.ps_present);
        assert_eq!(asc.object_type, 5);
    }

    #[test]
    fn parse_rejects_empty_bytes() {
        assert!(matches!(parse_asc(&[]), Err(CodecError::EndOfStream { .. })));
    }

    #[test]
    fn parse_escape_object_type() {
        // AOT = 42 (xHE-AAC USAC) -> base = 31, ext = 10
        // bits: 11111 001010 (object type) 0011 (sfi=48k) 0010 (channel=2) pad
        // = 11111 001010 0011 0010 0
        // = 1111 1001 0100 0110 0100 (need 19 bits, pad to 24)
        // = 1111 1001 0100 0110 0100 0000 = 0xF9 0x46 0x40
        let asc = parse_asc(&[0xF9, 0x46, 0x40]).unwrap();
        assert_eq!(asc.object_type, 42);
        assert_eq!(asc.sample_rate, 48000);
        assert_eq!(asc.channel_config, 2);
    }

    #[test]
    fn parse_explicit_frequency() {
        // AOT=2, sfi=15 (escape), explicit freq=96000 (0x017700),
        // channel=2. Layout:
        //   AOT(5)     = 00010
        //   sfi(4)     = 1111
        //   freq(24)   = 000000010111011100000000  (0x017700)
        //   channel(4) = 0010
        //   pad(3)     = 000
        // Full 40-bit stream concatenated:
        //   00010 1111 00000001 01110111 00000000 0010 000
        //
        // Regrouped into 8-bit bytes:
        //   0001 0111 = 0x17
        //   1000 0000 = 0x80
        //   1011 1011 = 0xBB
        //   1000 0000 = 0x80
        //   0001 0000 = 0x10
        let asc = parse_asc(&[0x17, 0x80, 0xBB, 0x80, 0x10]).unwrap();
        assert_eq!(asc.object_type, 2);
        assert_eq!(asc.sample_rate, 96000);
        assert_eq!(asc.channel_config, 2);
    }
}
