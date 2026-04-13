//! Init segment writer built on top of [`mp4_atom`].
//!
//! Emits the `ftyp + moov` prelude that every CMAF consumer (MSE,
//! ffprobe, HLS, DASH) expects before the first media segment. The
//! hand-rolled writer at `lvqr-ingest::remux::fmp4` will eventually be
//! retired in favour of this path; for now it stays in place so the
//! existing `rtmp_ws_e2e` test does not regress.
//!
//! Only AVC is wired in session 5 because it is the smallest test
//! surface that exercises the `mp4-atom` integration end-to-end. HEVC
//! and AV1 use the same `Moov` skeleton with `Codec::Hev1` /
//! `Codec::Av01` swapped in; those land alongside their first real
//! producer.

use bytes::BytesMut;
use mp4_atom::{
    Avc1, Avcc, Codec, Compressor, Dinf, Dref, Encode, FourCC, Ftyp, Hdlr, Mdhd, Mdia, Minf, Moov, Mvex, Mvhd, Stbl,
    Stco, Stsc, Stsd, Stsz, Stts, Tkhd, Trak, Trex, Url, Visual, Vmhd,
};

/// Errors produced by the init-segment writer.
#[derive(Debug, thiserror::Error)]
pub enum InitSegmentError {
    /// An mp4-atom encode call returned an error. In practice this only
    /// happens if a field is out of spec (e.g. zero length-size in
    /// `avcC`), which is a bug in the caller not a wire-level failure.
    #[error("mp4-atom encode error: {0}")]
    Encode(#[from] mp4_atom::Error),
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
