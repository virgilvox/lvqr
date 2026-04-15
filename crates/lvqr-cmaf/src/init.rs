//! Init segment writer built on top of [`mp4_atom`].
//!
//! Emits the `ftyp + moov` prelude that every CMAF consumer (MSE,
//! ffprobe, HLS, DASH) expects before the first media segment. The
//! hand-rolled writer at `lvqr-ingest::remux::fmp4` will eventually be
//! retired in favour of this path; for now it stays in place so the
//! existing `rtmp_ws_e2e` test does not regress.
//!
//! Session 5 landed AVC. Session 6 adds HEVC (`hev1` sample entry with
//! a `hvcC` filled from parsed SPS values) and AAC (`mp4a` sample entry
//! with an `esds` fed from the hardened [`lvqr_codec::aac::parse_asc`]).
//! AV1 lands alongside its first real producer.

use bytes::BytesMut;
use lvqr_codec::aac::{AAC_SAMPLE_FREQUENCIES, AudioSpecificConfig, parse_asc};
use lvqr_codec::hevc::HevcSps;
use mp4_atom::{
    Audio, Avc1, Avcc, Codec, Compressor, Dinf, Dref, Encode, Esds, FourCC, Ftyp, Hdlr, Hev1, HvcCArray, Hvcc, Mdhd,
    Mdia, Minf, Moov, Mp4a, Mvex, Mvhd, Smhd, Stbl, Stco, Stsc, Stsd, Stsz, Stts, Tkhd, Trak, Trex, Url, Visual, Vmhd,
    esds::{DecoderConfig, DecoderSpecific, EsDescriptor, SLConfig},
};

/// Errors produced by the init-segment writer.
#[derive(Debug, thiserror::Error)]
pub enum InitSegmentError {
    /// An mp4-atom encode call returned an error. In practice this only
    /// happens if a field is out of spec (e.g. zero length-size in
    /// `avcC`), which is a bug in the caller not a wire-level failure.
    #[error("mp4-atom encode error: {0}")]
    Encode(#[from] mp4_atom::Error),
    /// Raw AAC AudioSpecificConfig bytes failed to parse.
    #[error("invalid AAC AudioSpecificConfig: {0}")]
    InvalidAsc(#[from] lvqr_codec::CodecError),
    /// The caller supplied an AAC sample rate that does not map to one
    /// of the 13 indexable frequencies in ISO/IEC 14496-3 Table 1.16.
    /// mp4-atom's `DecoderSpecific` descriptor only encodes the 4-bit
    /// frequency index form, so the explicit 24-bit escape path is not
    /// reachable through this writer. Callers hitting this error have
    /// an unusual sample rate (e.g. 11468 Hz) and should extend the
    /// writer when that becomes a real constraint.
    #[error("AAC sample rate {0} Hz has no standard frequency index")]
    UnsupportedAacSampleRate(u32),
}

/// Parameters needed to build an AVC init segment.
///
/// Deliberately minimal: codec params come straight from the FLV /
/// RTMP AVC sequence header, and the video dimensions come from the
/// SPS via `lvqr-ingest::remux::extract_resolution` (or any other
/// producer that already parses resolution).
#[derive(Debug, Clone)]
pub struct VideoInitParams {
    /// Raw SPS NALU (without start code, first byte is `0x67` AVC NAL
    /// header). Passed straight to `Avcc::new`.
    pub sps: Vec<u8>,
    /// Raw PPS NALU (without start code, first byte is `0x68`).
    pub pps: Vec<u8>,
    /// Width in pixels as reported by the SPS.
    pub width: u16,
    /// Height in pixels as reported by the SPS.
    pub height: u16,
    /// Movie timescale. 90000 is the usual choice for video tracks.
    pub timescale: u32,
}

/// Write an AVC init segment (`ftyp + moov`) into `buf` using
/// `mp4-atom` for every box.
///
/// Returns the number of bytes written so callers that are streaming
/// to a socket can track progress without re-measuring the buffer.
pub fn write_avc_init_segment(buf: &mut BytesMut, params: &VideoInitParams) -> Result<usize, InitSegmentError> {
    let start = buf.len();

    // ftyp: `isom` major brand with CMAF-compatible brands matches the
    // output of the existing hand-rolled writer, which was validated
    // against MSE and ffprobe. Keeping the brand list identical means
    // the byte-level diff between old and new init segments stays
    // small when the Tier 2.3 migration lands.
    let ftyp = Ftyp {
        major_brand: FourCC::from(*b"isom"),
        minor_version: 0,
        compatible_brands: vec![
            FourCC::from(*b"isom"),
            FourCC::from(*b"iso6"),
            FourCC::from(*b"msdh"),
            FourCC::from(*b"msix"),
        ],
    };
    ftyp.encode(buf)?;

    let avcc = Avcc::new(&params.sps, &params.pps)?;

    let moov = Moov {
        mvhd: Mvhd {
            creation_time: 0,
            modification_time: 0,
            timescale: params.timescale,
            duration: 0,
            rate: 1.into(),
            volume: 1.into(),
            matrix: Default::default(),
            next_track_id: 2,
        },
        meta: None,
        mvex: Some(Mvex {
            mehd: None,
            trex: vec![Trex {
                track_id: 1,
                default_sample_description_index: 1,
                default_sample_duration: 0,
                default_sample_size: 0,
                default_sample_flags: 0,
            }],
        }),
        trak: vec![Trak {
            tkhd: Tkhd {
                creation_time: 0,
                modification_time: 0,
                track_id: 1,
                duration: 0,
                layer: 0,
                alternate_group: 0,
                enabled: true,
                volume: 0.into(),
                matrix: Default::default(),
                width: params.width.into(),
                height: params.height.into(),
            },
            edts: None,
            meta: None,
            mdia: Mdia {
                mdhd: Mdhd {
                    creation_time: 0,
                    modification_time: 0,
                    timescale: params.timescale,
                    duration: 0,
                    language: "und".into(),
                },
                hdlr: Hdlr {
                    handler: FourCC::from(*b"vide"),
                    name: "LVQR Video".to_string(),
                },
                minf: Minf {
                    vmhd: Some(Vmhd::default()),
                    dinf: Dinf {
                        dref: Dref {
                            urls: vec![Url::default()],
                        },
                    },
                    stbl: Stbl {
                        stsd: Stsd {
                            codecs: vec![Codec::Avc1(Avc1 {
                                visual: Visual {
                                    data_reference_index: 1,
                                    width: params.width,
                                    height: params.height,
                                    horizresolution: 0x48.into(),
                                    vertresolution: 0x48.into(),
                                    frame_count: 1,
                                    compressor: Compressor::default(),
                                    depth: 0x0018,
                                },
                                avcc,
                                btrt: None,
                                colr: None,
                                pasp: None,
                                taic: None,
                                fiel: None,
                            })],
                        },
                        stts: Stts::default(),
                        ctts: None,
                        stss: None,
                        stsc: Stsc::default(),
                        stsz: Stsz::default(),
                        stco: Some(Stco::default()),
                        co64: None,
                        sbgp: vec![],
                        sgpd: vec![],
                        subs: vec![],
                        saio: vec![],
                        saiz: vec![],
                        cslg: None,
                    },
                    ..Default::default()
                },
            },
            senc: None,
            udta: None,
        }],
        udta: None,
    };
    moov.encode(buf)?;

    Ok(buf.len() - start)
}

/// Parameters needed to build an HEVC init segment.
///
/// The three NAL unit byte blobs are written verbatim into the `hvcC`
/// sample-entry arrays (one array each for VPS / SPS / PPS). Every blob
/// must include the 2-byte HEVC NAL unit header; that is what real
/// decoders see in the bitstream and what ffprobe cross-checks against
/// the `hvcC` metadata.
///
/// `sps_info` is the parsed-SPS view used to populate the `hvcC` header
/// (profile / tier / level / chroma format / resolution). Callers that
/// already decoded the SPS via [`lvqr_codec::hevc::parse_sps`] pass the
/// result straight in; callers with a hand-built sample entry (e.g. for
/// a conformance test against a pre-captured encoder output) can fill
/// in the struct directly.
#[derive(Debug, Clone)]
pub struct HevcInitParams {
    /// Video Parameter Set NAL unit (nal_unit_type = 32).
    pub vps: Vec<u8>,
    /// Sequence Parameter Set NAL unit (nal_unit_type = 33).
    pub sps: Vec<u8>,
    /// Picture Parameter Set NAL unit (nal_unit_type = 34).
    pub pps: Vec<u8>,
    /// Decoded SPS used to fill `hvcC` profile / tier / level / chroma
    /// and `tkhd` / `visual` dimensions.
    pub sps_info: HevcSps,
    /// Movie timescale. 90000 is the usual choice for video tracks.
    pub timescale: u32,
}

/// Write an HEVC init segment (`ftyp + moov` with a `hev1` sample
/// entry) into `buf`.
pub fn write_hevc_init_segment(buf: &mut BytesMut, params: &HevcInitParams) -> Result<usize, InitSegmentError> {
    let start = buf.len();

    // Reuse the AVC writer's brand list. `hev1` is valid under `iso6`
    // (the baseline CMAF brand) so no extra brand is required.
    let ftyp = Ftyp {
        major_brand: FourCC::from(*b"isom"),
        minor_version: 0,
        compatible_brands: vec![
            FourCC::from(*b"isom"),
            FourCC::from(*b"iso6"),
            FourCC::from(*b"msdh"),
            FourCC::from(*b"msix"),
        ],
    };
    ftyp.encode(buf)?;

    let width = params.sps_info.pic_width_in_luma_samples as u16;
    let height = params.sps_info.pic_height_in_luma_samples as u16;

    let hvcc = Hvcc {
        configuration_version: 1,
        general_profile_space: params.sps_info.general_profile_space,
        general_tier_flag: params.sps_info.general_tier_flag,
        general_profile_idc: params.sps_info.general_profile_idc,
        general_profile_compatibility_flags: params.sps_info.general_profile_compatibility_flags.to_be_bytes(),
        // LVQR's parser does not extract the 48-bit constraint block
        // because no LVQR consumer has needed it yet. Zero is a safe
        // default for the 8-bit Main profile streams we support today;
        // when a consumer needs the real bits, extend `HevcSps` first.
        general_constraint_indicator_flags: [0; 6],
        general_level_idc: params.sps_info.general_level_idc,
        min_spatial_segmentation_idc: 0,
        parallelism_type: 0,
        chroma_format_idc: (params.sps_info.chroma_format_idc & 0b11) as u8,
        // Not extracted by the SPS parser yet; 8-bit is the only depth
        // LVQR ships support for and matches every Main-profile stream.
        bit_depth_luma_minus8: 0,
        bit_depth_chroma_minus8: 0,
        avg_frame_rate: 0,
        constant_frame_rate: 0,
        num_temporal_layers: 1,
        temporal_id_nested: true,
        // 4-byte NAL length prefix. Same as the AVC path.
        length_size_minus_one: 3,
        arrays: vec![
            HvcCArray {
                completeness: true,
                nal_unit_type: 32,
                nalus: vec![params.vps.clone()],
            },
            HvcCArray {
                completeness: true,
                nal_unit_type: 33,
                nalus: vec![params.sps.clone()],
            },
            HvcCArray {
                completeness: true,
                nal_unit_type: 34,
                nalus: vec![params.pps.clone()],
            },
        ],
    };

    let moov = Moov {
        mvhd: Mvhd {
            creation_time: 0,
            modification_time: 0,
            timescale: params.timescale,
            duration: 0,
            rate: 1.into(),
            volume: 1.into(),
            matrix: Default::default(),
            next_track_id: 2,
        },
        meta: None,
        mvex: Some(Mvex {
            mehd: None,
            trex: vec![Trex {
                track_id: 1,
                default_sample_description_index: 1,
                default_sample_duration: 0,
                default_sample_size: 0,
                default_sample_flags: 0,
            }],
        }),
        trak: vec![Trak {
            tkhd: Tkhd {
                creation_time: 0,
                modification_time: 0,
                track_id: 1,
                duration: 0,
                layer: 0,
                alternate_group: 0,
                enabled: true,
                volume: 0.into(),
                matrix: Default::default(),
                width: width.into(),
                height: height.into(),
            },
            edts: None,
            meta: None,
            mdia: Mdia {
                mdhd: Mdhd {
                    creation_time: 0,
                    modification_time: 0,
                    timescale: params.timescale,
                    duration: 0,
                    language: "und".into(),
                },
                hdlr: Hdlr {
                    handler: FourCC::from(*b"vide"),
                    name: "LVQR Video".to_string(),
                },
                minf: Minf {
                    vmhd: Some(Vmhd::default()),
                    dinf: Dinf {
                        dref: Dref {
                            urls: vec![Url::default()],
                        },
                    },
                    stbl: Stbl {
                        stsd: Stsd {
                            codecs: vec![Codec::Hev1(Hev1 {
                                visual: Visual {
                                    data_reference_index: 1,
                                    width,
                                    height,
                                    horizresolution: 0x48.into(),
                                    vertresolution: 0x48.into(),
                                    frame_count: 1,
                                    compressor: Compressor::default(),
                                    depth: 0x0018,
                                },
                                hvcc,
                                btrt: None,
                                colr: None,
                                pasp: None,
                                taic: None,
                                fiel: None,
                            })],
                        },
                        stts: Stts::default(),
                        ctts: None,
                        stss: None,
                        stsc: Stsc::default(),
                        stsz: Stsz::default(),
                        stco: Some(Stco::default()),
                        co64: None,
                        sbgp: vec![],
                        sgpd: vec![],
                        subs: vec![],
                        saio: vec![],
                        saiz: vec![],
                        cslg: None,
                    },
                    ..Default::default()
                },
            },
            senc: None,
            udta: None,
        }],
        udta: None,
    };
    moov.encode(buf)?;

    Ok(buf.len() - start)
}

/// Parameters needed to build an AAC init segment.
///
/// The ASC bytes are the raw `AudioSpecificConfig` payload as found in
/// an FLV `AAC_SEQUENCE_HEADER` tag or an MP4 `esds` box. The writer
/// runs them through [`lvqr_codec::aac::parse_asc`] so the `mp4a`
/// sample-entry fields (channel count, sample rate) and the `esds`
/// `DecoderSpecific` descriptor (profile, freq_index, chan_conf) come
/// from the same parse, not from a caller-supplied summary that could
/// drift from the bytes on the wire.
#[derive(Debug, Clone)]
pub struct AudioInitParams {
    /// Raw AudioSpecificConfig bytes.
    pub asc: Vec<u8>,
    /// Track timescale. Usually equal to the audio sample rate so one
    /// AAC frame (1024 samples) maps to a `sample_duration` of 1024.
    pub timescale: u32,
}

/// Write an AAC init segment (`ftyp + moov` with an `mp4a` sample
/// entry) into `buf`.
pub fn write_aac_init_segment(buf: &mut BytesMut, params: &AudioInitParams) -> Result<usize, InitSegmentError> {
    let start = buf.len();

    let asc = parse_asc(&params.asc)?;
    let mp4a = build_mp4a(&asc)?;

    let ftyp = Ftyp {
        major_brand: FourCC::from(*b"isom"),
        minor_version: 0,
        compatible_brands: vec![
            FourCC::from(*b"isom"),
            FourCC::from(*b"iso6"),
            FourCC::from(*b"msdh"),
            FourCC::from(*b"msix"),
        ],
    };
    ftyp.encode(buf)?;

    // Audio sample_rate in an ISO BMFF `AudioSampleEntry` is stored as
    // a 16.16 fixed point value; the low 16 bits are always zero and
    // the high 16 bits carry the integer Hz. mp4-atom's
    // `FixedPoint<u16>::from(u16)` encodes exactly that layout, but it
    // caps at 65535 Hz. Every rate in the AAC indexable frequency
    // table fits (max 96000 > 65535 — see below), except that 96 kHz
    // and 88.2 kHz overflow. Callers hitting that ceiling should use
    // the QuickTime version-1 sound entry; we reject here rather than
    // silently truncate.
    if asc.sample_rate > u16::MAX as u32 {
        return Err(InitSegmentError::UnsupportedAacSampleRate(asc.sample_rate));
    }

    let moov = Moov {
        mvhd: Mvhd {
            creation_time: 0,
            modification_time: 0,
            timescale: params.timescale,
            duration: 0,
            rate: 1.into(),
            volume: 1.into(),
            matrix: Default::default(),
            next_track_id: 2,
        },
        meta: None,
        mvex: Some(Mvex {
            mehd: None,
            trex: vec![Trex {
                track_id: 1,
                default_sample_description_index: 1,
                default_sample_duration: 0,
                default_sample_size: 0,
                default_sample_flags: 0,
            }],
        }),
        trak: vec![Trak {
            tkhd: Tkhd {
                creation_time: 0,
                modification_time: 0,
                track_id: 1,
                duration: 0,
                layer: 0,
                alternate_group: 0,
                enabled: true,
                volume: 1.into(),
                matrix: Default::default(),
                width: 0.into(),
                height: 0.into(),
            },
            edts: None,
            meta: None,
            mdia: Mdia {
                mdhd: Mdhd {
                    creation_time: 0,
                    modification_time: 0,
                    timescale: params.timescale,
                    duration: 0,
                    language: "und".into(),
                },
                hdlr: Hdlr {
                    handler: FourCC::from(*b"soun"),
                    name: "LVQR Audio".to_string(),
                },
                minf: Minf {
                    vmhd: None,
                    smhd: Some(Smhd::default()),
                    dinf: Dinf {
                        dref: Dref {
                            urls: vec![Url::default()],
                        },
                    },
                    stbl: Stbl {
                        stsd: Stsd {
                            codecs: vec![Codec::Mp4a(mp4a)],
                        },
                        stts: Stts::default(),
                        ctts: None,
                        stss: None,
                        stsc: Stsc::default(),
                        stsz: Stsz::default(),
                        stco: Some(Stco::default()),
                        co64: None,
                        sbgp: vec![],
                        sgpd: vec![],
                        subs: vec![],
                        saio: vec![],
                        saiz: vec![],
                        cslg: None,
                    },
                    ..Default::default()
                },
            },
            senc: None,
            udta: None,
        }],
        udta: None,
    };
    moov.encode(buf)?;

    Ok(buf.len() - start)
}

fn build_mp4a(asc: &AudioSpecificConfig) -> Result<Mp4a, InitSegmentError> {
    // Map the decoded Hz back to the 4-bit sampling_frequency_index.
    let freq_index = AAC_SAMPLE_FREQUENCIES
        .iter()
        .position(|&hz| hz == asc.sample_rate)
        .ok_or(InitSegmentError::UnsupportedAacSampleRate(asc.sample_rate))? as u8;

    // `DecoderSpecific.profile` is the 5-bit AOT as stored in the ASC.
    // mp4-atom's encoder shifts it left by 3 so anything in 0..=31 is
    // safe; escape-encoded object types (>=32) would need the two-byte
    // form which mp4-atom does not implement. LVQR only emits AAC-LC /
    // HE-AAC today so AOT 2 and 5 are the only cases in flight.
    if asc.object_type > 31 {
        return Err(InitSegmentError::InvalidAsc(lvqr_codec::CodecError::MalformedAsc(
            "AAC object type >= 32 (escape-encoded) cannot round-trip through mp4-atom esds",
        )));
    }

    Ok(Mp4a {
        audio: Audio {
            data_reference_index: 1,
            channel_count: asc.channel_config as u16,
            sample_size: 16,
            sample_rate: (asc.sample_rate as u16).into(),
        },
        esds: Esds {
            es_desc: EsDescriptor {
                es_id: 1,
                dec_config: DecoderConfig {
                    // MPEG-4 Audio (ISO/IEC 14496-3). 0x40 is the
                    // objectTypeIndication that maps to the AAC family
                    // under the MP4 Registration Authority.
                    object_type_indication: 0x40,
                    // AudioStream (0x05). The bitstream is an
                    // elementary audio stream, not scene description.
                    stream_type: 0x05,
                    up_stream: 0,
                    buffer_size_db: Default::default(),
                    // Conservative placeholders. A real muxer fills
                    // these from the VBV model; ffprobe accepts zero.
                    max_bitrate: 0,
                    avg_bitrate: 0,
                    dec_specific: DecoderSpecific {
                        profile: asc.object_type,
                        freq_index,
                        chan_conf: asc.channel_config,
                    },
                },
                sl_config: SLConfig::default(),
            },
        },
        btrt: None,
        taic: None,
    })
}

/// Decode an fMP4 init segment (ftyp + moov) and, from the first
/// video sample entry, produce the ISO BMFF codec string the
/// HLS/DASH master playlist needs to announce it.
///
/// Returns:
/// * `Some("avc1.PPCCLL")` for an `Avc1` entry, where `PP`,
///   `CC`, and `LL` are the 2-digit hex of
///   `avc_profile_indication`, `profile_compatibility`, and
///   `avc_level_indication` parsed out of the `avcC` box.
/// * `Some("hvc1.<profile_space_letter><profile_idc>.<compat_rev>.
///   L<level_idc>.B0")` for a `Hev1` entry, where the
///   compatibility flags are formatted as a reverse-bit-ordered
///   hex value (the format DASH/HLS clients expect).
/// * `None` if the input is not a parseable ftyp+moov, carries no
///   video trak, or carries a sample entry type we do not yet
///   stringify (AV1, VP9, codec-neutral, etc.).
///
/// The function is intended to be called once per broadcast at
/// the moment the init segment lands (e.g. from
/// `HlsServer::push_init`) and cached; it is cheap relative to
/// the per-fragment path but not free (decodes the full moov).
pub fn detect_video_codec_string(init: &[u8]) -> Option<String> {
    use mp4_atom::Decode;

    let mut cursor = std::io::Cursor::new(init);
    mp4_atom::Ftyp::decode(&mut cursor).ok()?;
    let moov = Moov::decode(&mut cursor).ok()?;
    for trak in &moov.trak {
        for codec in &trak.mdia.minf.stbl.stsd.codecs {
            if let Some(s) = codec_string_for_codec(codec) {
                return Some(s);
            }
        }
    }
    None
}

fn codec_string_for_codec(codec: &Codec) -> Option<String> {
    match codec {
        Codec::Avc1(avc1) => Some(format!(
            "avc1.{:02X}{:02X}{:02X}",
            avc1.avcc.avc_profile_indication, avc1.avcc.profile_compatibility, avc1.avcc.avc_level_indication,
        )),
        Codec::Hev1(hev1) => Some(hvc1_codec_string(&hev1.hvcc)),
        _ => None,
    }
}

fn hvc1_codec_string(hvcc: &mp4_atom::Hvcc) -> String {
    // Profile space letter: 0 -> "", 1 -> "A", 2 -> "B", 3 -> "C".
    let space = match hvcc.general_profile_space {
        1 => "A",
        2 => "B",
        3 => "C",
        _ => "",
    };
    // Compatibility flags are reverse-bit-ordered then hex-
    // encoded. ISO/IEC 14496-15 section E.3 pins this.
    let compat = u32::from_be_bytes(hvcc.general_profile_compatibility_flags);
    let compat_rev = compat.reverse_bits();
    let tier = if hvcc.general_tier_flag { 'H' } else { 'L' };
    // General constraint indicator flags: compress trailing-zero
    // bytes to a single "B0" for the common case; a precise
    // encoder would emit per-byte dot-separated values but every
    // real HLS / DASH parser accepts the "B0" abbreviation for
    // Main / Main 10 profiles without the constraint bits set.
    let constraints_all_zero = hvcc.general_constraint_indicator_flags.iter().all(|b| *b == 0);
    if constraints_all_zero {
        format!(
            "hvc1.{}{}.{:X}.{}{}.B0",
            space, hvcc.general_profile_idc, compat_rev, tier, hvcc.general_level_idc,
        )
    } else {
        let first_nonzero = hvcc
            .general_constraint_indicator_flags
            .iter()
            .find(|b| **b != 0)
            .copied()
            .unwrap_or(0);
        format!(
            "hvc1.{}{}.{:X}.{}{}.{:02X}",
            space, hvcc.general_profile_idc, compat_rev, tier, hvcc.general_level_idc, first_nonzero,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mp4_atom::Decode;

    /// Deterministic SPS+PPS from the lvqr-ingest golden fixtures so
    /// the produced init segment is reproducible across runs.
    const SPS: &[u8] = &[
        0x67, 0x42, 0x00, 0x1F, 0xD9, 0x40, 0x50, 0x04, 0xFB, 0x01, 0x10, 0x00, 0x00, 0x03, 0x00, 0x10, 0x00, 0x00,
        0x03, 0x03, 0xC0, 0xF1, 0x83, 0x2A,
    ];
    const PPS: &[u8] = &[0x68, 0xEB, 0xE3, 0xCB, 0x22, 0xC0];

    #[test]
    fn avc_init_segment_starts_with_ftyp_and_contains_moov() {
        let params = VideoInitParams {
            sps: SPS.to_vec(),
            pps: PPS.to_vec(),
            width: 1280,
            height: 720,
            timescale: 90_000,
        };
        let mut buf = BytesMut::new();
        let n = write_avc_init_segment(&mut buf, &params).expect("encode");
        assert_eq!(n, buf.len());
        // ftyp box starts at offset 4 with the FourCC "ftyp".
        assert_eq!(&buf[4..8], b"ftyp", "first box is ftyp");
        // moov follows immediately after ftyp. Skip the ftyp size + 4
        // bytes of size to land on the moov FourCC.
        let ftyp_size = u32::from_be_bytes([buf[0], buf[1], buf[2], buf[3]]) as usize;
        assert_eq!(&buf[ftyp_size + 4..ftyp_size + 8], b"moov", "second box is moov");
    }

    // Real x265 HEVC Main 3.0 NAL units captured from
    // `ffmpeg 8.1 -f lavfi -i testsrc2=320x240:rate=30 -t 1 -c:v libx265`.
    // The same capture drives the conformance test in
    // `tests/conformance_init.rs`; keeping the blobs here as well
    // lets the library unit tests run without the lvqr-conformance
    // dev-dep. If x265 is updated and these bytes drift, the
    // conformance test catches it first.
    const HEVC_VPS_X265: &[u8] = &[
        0x40, 0x01, 0x0c, 0x01, 0xff, 0xff, 0x01, 0x60, 0x00, 0x00, 0x03, 0x00, 0x90, 0x00, 0x00, 0x03, 0x00, 0x00,
        0x03, 0x00, 0x3c, 0x95, 0x94, 0x09,
    ];
    const HEVC_SPS_X265_NAL: &[u8] = &[
        0x42, 0x01, 0x01, 0x01, 0x60, 0x00, 0x00, 0x03, 0x00, 0x90, 0x00, 0x00, 0x03, 0x00, 0x00, 0x03, 0x00, 0x3c,
        0xa0, 0x0a, 0x08, 0x0f, 0x16, 0x59, 0x59, 0x52, 0x93, 0x0b, 0xc0, 0x5a, 0x02, 0x00, 0x00, 0x03, 0x00, 0x02,
        0x00, 0x00, 0x03, 0x00, 0x3c, 0x10,
    ];
    const HEVC_PPS_X265: &[u8] = &[0x44, 0x01, 0xc0, 0x73, 0xc1, 0x89];

    /// Parsed-SPS view matching [`HEVC_SPS_X265_NAL`]. Values pinned
    /// against the `lvqr-conformance` codec corpus for the existing
    /// 320x240 x265 capture, which was in turn verified against
    /// ffprobe output during the session 5 bootstrap.
    fn hevc_sps_x265_info() -> lvqr_codec::hevc::HevcSps {
        lvqr_codec::hevc::HevcSps {
            general_profile_space: 0,
            general_tier_flag: false,
            general_profile_idc: 1,
            general_profile_compatibility_flags: 0x60000000,
            general_level_idc: 60,
            chroma_format_idc: 1,
            pic_width_in_luma_samples: 320,
            pic_height_in_luma_samples: 240,
        }
    }

    #[test]
    fn hevc_init_segment_starts_with_ftyp_and_contains_moov() {
        let params = HevcInitParams {
            vps: HEVC_VPS_X265.to_vec(),
            sps: HEVC_SPS_X265_NAL.to_vec(),
            pps: HEVC_PPS_X265.to_vec(),
            sps_info: hevc_sps_x265_info(),
            timescale: 90_000,
        };
        let mut buf = BytesMut::new();
        let n = write_hevc_init_segment(&mut buf, &params).expect("encode");
        assert_eq!(n, buf.len());
        assert_eq!(&buf[4..8], b"ftyp");
        let ftyp_size = u32::from_be_bytes([buf[0], buf[1], buf[2], buf[3]]) as usize;
        assert_eq!(&buf[ftyp_size + 4..ftyp_size + 8], b"moov");
    }

    #[test]
    fn hevc_init_segment_round_trips_through_mp4_atom() {
        let params = HevcInitParams {
            vps: HEVC_VPS_X265.to_vec(),
            sps: HEVC_SPS_X265_NAL.to_vec(),
            pps: HEVC_PPS_X265.to_vec(),
            sps_info: hevc_sps_x265_info(),
            timescale: 90_000,
        };
        let mut buf = BytesMut::new();
        write_hevc_init_segment(&mut buf, &params).expect("encode");

        let mut cursor = std::io::Cursor::new(buf.as_ref());
        let _ftyp = mp4_atom::Ftyp::decode(&mut cursor).expect("decode ftyp");
        let moov = mp4_atom::Moov::decode(&mut cursor).expect("decode moov");
        assert_eq!(moov.trak.len(), 1);
        let codec = &moov.trak[0].mdia.minf.stbl.stsd.codecs[0];
        let hev1 = match codec {
            mp4_atom::Codec::Hev1(h) => h,
            other => panic!("expected Hev1, got {:?}", std::mem::discriminant(other)),
        };
        assert_eq!(hev1.visual.width, 320);
        assert_eq!(hev1.visual.height, 240);
        assert_eq!(hev1.hvcc.general_profile_idc, 1);
        assert_eq!(hev1.hvcc.general_level_idc, 60);
        assert_eq!(hev1.hvcc.chroma_format_idc, 1);
        // Three arrays: VPS, SPS, PPS. The SEI array that ffmpeg
        // emits alongside x265 output is intentionally dropped.
        assert_eq!(hev1.hvcc.arrays.len(), 3);
        assert_eq!(hev1.hvcc.arrays[0].nal_unit_type, 32);
        assert_eq!(hev1.hvcc.arrays[0].nalus[0], HEVC_VPS_X265);
        assert_eq!(hev1.hvcc.arrays[1].nal_unit_type, 33);
        assert_eq!(hev1.hvcc.arrays[1].nalus[0], HEVC_SPS_X265_NAL);
        assert_eq!(hev1.hvcc.arrays[2].nal_unit_type, 34);
        assert_eq!(hev1.hvcc.arrays[2].nalus[0], HEVC_PPS_X265);
    }

    #[test]
    fn aac_init_segment_round_trips_through_mp4_atom() {
        // The same 2-byte AAC-LC 44.1 kHz stereo ASC used by
        // lvqr-ingest's existing esds tests and the conformance
        // corpus. Decoding should yield a valid `mp4a` sample entry
        // that round-trips through mp4-atom.
        let params = AudioInitParams {
            asc: vec![0x12, 0x10],
            timescale: 44_100,
        };
        let mut buf = BytesMut::new();
        write_aac_init_segment(&mut buf, &params).expect("encode");

        let mut cursor = std::io::Cursor::new(buf.as_ref());
        let _ftyp = mp4_atom::Ftyp::decode(&mut cursor).expect("decode ftyp");
        let moov = mp4_atom::Moov::decode(&mut cursor).expect("decode moov");
        assert_eq!(moov.trak.len(), 1);
        let codec = &moov.trak[0].mdia.minf.stbl.stsd.codecs[0];
        let mp4a = match codec {
            mp4_atom::Codec::Mp4a(m) => m,
            other => panic!("expected Mp4a, got {:?}", std::mem::discriminant(other)),
        };
        assert_eq!(mp4a.audio.channel_count, 2);
        assert_eq!(mp4a.audio.sample_size, 16);
        let ds = &mp4a.esds.es_desc.dec_config.dec_specific;
        assert_eq!(ds.profile, 2, "AAC-LC");
        // freq_index 4 = 44100 Hz per ISO/IEC 14496-3 Table 1.16.
        assert_eq!(ds.freq_index, 4);
        assert_eq!(ds.chan_conf, 2);
    }

    #[test]
    fn detect_video_codec_string_reports_avc1_from_avc_init() {
        let params = VideoInitParams {
            sps: SPS.to_vec(),
            pps: PPS.to_vec(),
            width: 1280,
            height: 720,
            timescale: 90_000,
        };
        let mut buf = BytesMut::new();
        write_avc_init_segment(&mut buf, &params).expect("encode");
        let got = detect_video_codec_string(&buf).expect("avc codec string");
        // The fixture SPS reports profile_idc=0x42, compat=0x00,
        // level_idc=0x1F. `{:02X}` upper-case hex keeps the string
        // aligned with the "avc1.4200XX" convention real players
        // emit in their `canPlayType` probes.
        assert!(got.starts_with("avc1."), "got {got}");
        assert_eq!(got, "avc1.42001F");
    }

    #[test]
    fn detect_video_codec_string_reports_hvc1_from_hevc_init() {
        let params = HevcInitParams {
            vps: HEVC_VPS_X265.to_vec(),
            sps: HEVC_SPS_X265_NAL.to_vec(),
            pps: HEVC_PPS_X265.to_vec(),
            sps_info: hevc_sps_x265_info(),
            timescale: 90_000,
        };
        let mut buf = BytesMut::new();
        write_hevc_init_segment(&mut buf, &params).expect("encode");
        let got = detect_video_codec_string(&buf).expect("hevc codec string");
        // x265 fixture: profile_space=0 -> "", profile_idc=1,
        // compatibility_flags=0x60000000 reverse-bit-ordered ->
        // 0x00000006 -> "6", tier_flag=false -> 'L',
        // level_idc=60, constraints all zero -> "B0".
        assert_eq!(got, "hvc1.1.6.L60.B0");
    }

    #[test]
    fn detect_video_codec_string_returns_none_on_garbage() {
        assert!(detect_video_codec_string(&[]).is_none());
        assert!(detect_video_codec_string(&[0; 16]).is_none());
        assert!(detect_video_codec_string(b"not a real mp4 init segment").is_none());
    }

    #[test]
    fn aac_init_rejects_non_indexable_sample_rate() {
        // Explicit-frequency escape: pick a rate outside the
        // 13-entry indexable table so the writer hits the
        // `UnsupportedAacSampleRate` branch. 11468 Hz is not in
        // `AAC_SAMPLE_FREQUENCIES` and sits above the parser's
        // 7350 Hz plausibility floor, so parse_asc decodes it
        // cleanly and the writer is the one that refuses.
        //
        // ASC layout: AOT=2 (5 bits) | sfi=15 (4 bits) |
        // explicit_freq=0x002CCC (24 bits) | channel=2 (4 bits) |
        // pad (3 bits) = 40 bits = 5 bytes. The previous fixture
        // here (6 bytes) mispacked the bit positions and actually
        // decoded to 1433 Hz; that slipped past the writer only
        // because the lookup still failed regardless of the exact
        // rate.
        let params = AudioInitParams {
            asc: vec![0x17, 0x80, 0x16, 0x66, 0x10],
            timescale: 11468,
        };
        let mut buf = BytesMut::new();
        match write_aac_init_segment(&mut buf, &params) {
            Err(InitSegmentError::UnsupportedAacSampleRate(_)) => {}
            other => panic!("expected UnsupportedAacSampleRate, got {other:?}"),
        }
    }

    #[test]
    fn avc_init_segment_round_trips_through_mp4_atom() {
        // Proof that the bytes we emit are parseable by the same
        // library that wrote them. Not a conformance check (that is
        // `tests/conformance_init.rs`), but the cheapest possible
        // regression guard.
        let params = VideoInitParams {
            sps: SPS.to_vec(),
            pps: PPS.to_vec(),
            width: 640,
            height: 360,
            timescale: 90_000,
        };
        let mut buf = BytesMut::new();
        write_avc_init_segment(&mut buf, &params).expect("encode");

        let mut cursor = std::io::Cursor::new(buf.as_ref());
        let ftyp = mp4_atom::Ftyp::decode(&mut cursor).expect("decode ftyp");
        assert_eq!(ftyp.major_brand, FourCC::from(*b"isom"));
        let moov = mp4_atom::Moov::decode(&mut cursor).expect("decode moov");
        assert_eq!(moov.mvhd.timescale, 90_000);
        assert_eq!(moov.trak.len(), 1);
        assert_eq!(moov.trak[0].tkhd.track_id, 1);
    }
}
