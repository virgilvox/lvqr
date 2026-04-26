//! Minimal synthetic H.264 + FLV helpers for the `scte35-rtmp-push`
//! test bin (session 155).
//!
//! The `scte35-rtmp-push` bin opens a real RTMP publisher session and
//! sends a few seconds of synthetic H.264 NALUs through the relay's
//! RTMP -> HLS bridge so a `#EXT-X-DATERANGE` line shows up on the
//! served playlist after the bin injects an `onCuePoint scte35-bin64`
//! AMF0 Data message. The bridge at `crates/lvqr-ingest/src/bridge.rs`
//! drops video tags until an AVC sequence header populates
//! `stream.video_config + stream.video_init`, so the bin must send a
//! valid sequence header BEFORE any IDR / P-slice tag.
//!
//! The relay does NOT decode video; it just packages length-prefixed
//! NALUs into fMP4 mdat boxes and slices segments at IDR boundaries
//! (driven by the FLV tag's `frame_type=1` bit, not by decoding the
//! NALU itself). So the SPS / PPS bytes below need to PARSE (so the
//! `h264_reader` SPS bit-reader can extract dimensions for the
//! `tkhd`/`mdhd` boxes) but do not need to be a working encode -- the
//! macroblock data inside the IDR / P-slice NALUs is opaque to the
//! relay and is never decoded.
//!
//! [`SPS_HIGH_64X64`] and [`PPS_HIGH`] are lifted from the
//! `crates/lvqr-ingest/src/remux/flv.rs::tests::extract_resolution_from_known_sps`
//! fixture (which itself comes from the `h264-reader` crate's test
//! suite). Parseable, in-tree pinned, no codec drift risk.

use bytes::Bytes;

/// Known-parseable SPS NALU bytes (NAL type 7, High profile,
/// 64x64). Lifted verbatim from the `h264-reader` test suite via
/// `crates/lvqr-ingest/src/remux/flv.rs::extract_resolution_from_known_sps`.
/// 320x180 would have been more thematic but the in-tree pin
/// removes a fragile codec-version dependency; the relay's
/// `extract_resolution` falls back gracefully and the marker test
/// asserts on playlist content, not on rendered frame dimensions.
pub const SPS_HIGH_64X64: &[u8] = &[
    0x67, 0x64, 0x00, 0x0A, 0xAC, 0x72, 0x84, 0x44, 0x26, 0x84, 0x00, 0x00, 0x00, 0x04, 0x00, 0x00, 0x00, 0xCA, 0x3C,
    0x48, 0x96, 0x11, 0x80,
];

/// Known-parseable PPS NALU bytes (NAL type 8). Lifted from
/// `crates/lvqr-ingest/src/remux/flv.rs::tests::parse_video_sequence_header`.
pub const PPS_HIGH: &[u8] = &[0x68, 0xEE, 0x3C, 0x80];

/// Build the AVCDecoderConfigurationRecord (avcC) bytes for a single
/// SPS + single PPS. Mirrors `make_avcc_record` in
/// `crates/lvqr-ingest/src/remux/flv.rs` so the relay's `parse_avcc_record`
/// consumer round-trips byte-for-byte.
pub fn avcc_record(sps: &[u8], pps: &[u8]) -> Vec<u8> {
    // SPS NAL header byte 0 carries `nal_ref_idc | nal_unit_type` but
    // the avcC profile / compat / level fields come from SPS payload
    // bytes 1..4 (profile_idc, constraint_flags, level_idc). Index 0 is
    // the NAL header.
    assert!(sps.len() >= 4, "SPS too short");
    let profile = sps[1];
    let compat = sps[2];
    let level = sps[3];

    let mut rec = Vec::with_capacity(11 + sps.len() + pps.len());
    rec.push(0x01); // configurationVersion
    rec.push(profile);
    rec.push(compat);
    rec.push(level);
    rec.push(0xFF); // lengthSizeMinusOne=3 (4-byte NALU length prefix) | reserved
    rec.push(0xE1); // numSPS=1 | reserved
    rec.extend_from_slice(&(sps.len() as u16).to_be_bytes());
    rec.extend_from_slice(sps);
    rec.push(0x01); // numPPS=1
    rec.extend_from_slice(&(pps.len() as u16).to_be_bytes());
    rec.extend_from_slice(pps);
    rec
}

/// Build the FLV video tag body for an AVC sequence header carrying
/// the supplied SPS + PPS. Format:
///
/// * byte 0: `0x17` (frame_type=1 keyframe | codec_id=7 AVC)
/// * byte 1: `0x00` (AVCPacketType=0 sequence header)
/// * bytes 2-4: `0x00 0x00 0x00` (composition time = 0)
/// * bytes 5+: AVCDecoderConfigurationRecord
pub fn flv_avc_sequence_header(sps: &[u8], pps: &[u8]) -> Bytes {
    let mut tag = vec![0x17, 0x00, 0x00, 0x00, 0x00];
    tag.extend_from_slice(&avcc_record(sps, pps));
    Bytes::from(tag)
}

/// Build an FLV video tag body for an AVC NALU. The NALU body is
/// length-prefixed (4-byte big-endian length followed by NALU bytes)
/// per the AVCC `lengthSizeMinusOne=3` convention the bridge expects.
///
/// `keyframe=true` yields frame_type=1 (relay opens a new HLS segment
/// at this boundary); `false` yields frame_type=2 (P-slice).
pub fn flv_avc_nalu(keyframe: bool, cts_ms: i32, nalu_payload: &[u8]) -> Bytes {
    let frame_type_byte = if keyframe { 0x17 } else { 0x27 };
    let mut tag = Vec::with_capacity(5 + 4 + nalu_payload.len());
    tag.push(frame_type_byte);
    tag.push(0x01); // AVCPacketType = 1 (NALU)
    let cts = cts_ms & 0x00FF_FFFF;
    tag.push((cts >> 16) as u8);
    tag.push((cts >> 8) as u8);
    tag.push(cts as u8);
    // 4-byte big-endian NALU length prefix.
    let len = nalu_payload.len() as u32;
    tag.extend_from_slice(&len.to_be_bytes());
    tag.extend_from_slice(nalu_payload);
    Bytes::from(tag)
}

/// Synthetic IDR slice NALU payload. NAL header byte `0x65`
/// (forbidden_zero_bit=0, nal_ref_idc=3, nal_unit_type=5 = IDR slice)
/// followed by a small fixed slice-data pattern. The relay does not
/// decode the slice, so the macroblock data only needs to be present.
/// Lifted from `crates/lvqr-ingest/src/remux/flv.rs::tests::parse_video_nalu_keyframe`.
pub fn synthetic_idr_nal() -> Bytes {
    Bytes::from_static(&[0x65, 0x88, 0x84, 0x00])
}

/// Synthetic non-IDR (P-slice) NALU payload. NAL header byte `0x41`
/// (nal_ref_idc=2, nal_unit_type=1 = non-IDR slice) followed by a
/// small fixed pattern. Lifted from
/// `crates/lvqr-ingest/src/remux/flv.rs::tests::parse_video_nalu_delta_with_cts`.
pub fn synthetic_p_slice_nal() -> Bytes {
    Bytes::from_static(&[0x41, 0x9A, 0x00])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flv_avc_sequence_header_uses_keyframe_avc_packet_type_zero() {
        let body = flv_avc_sequence_header(SPS_HIGH_64X64, PPS_HIGH);
        // FLV tag header: 0x17 = keyframe + AVC, 0x00 = AVCPacketType seq header.
        assert_eq!(body[0], 0x17);
        assert_eq!(body[1], 0x00);
        // AVCDecoderConfigurationRecord starts at offset 5.
        assert_eq!(body[5], 0x01, "configurationVersion");
        assert_eq!(body[6], SPS_HIGH_64X64[1], "profile_idc carried from SPS[1]");
        assert_eq!(body[7], SPS_HIGH_64X64[2], "constraint_flags carried from SPS[2]");
        assert_eq!(body[8], SPS_HIGH_64X64[3], "level_idc carried from SPS[3]");
        assert_eq!(body[9], 0xFF, "lengthSizeMinusOne=3");
        assert_eq!(body[10] & 0x1F, 0x01, "numSPS=1");
    }

    #[test]
    fn flv_avc_nalu_keyframe_byte_is_set() {
        let nalu = synthetic_idr_nal();
        let tag = flv_avc_nalu(true, 0, &nalu);
        assert_eq!(tag[0], 0x17, "frame_type=1 + codec=7");
        assert_eq!(tag[1], 0x01, "AVCPacketType=1 NALU");
        // 4-byte NALU length prefix.
        let len = u32::from_be_bytes([tag[5], tag[6], tag[7], tag[8]]);
        assert_eq!(len as usize, nalu.len());
        assert_eq!(&tag[9..], &nalu[..]);
    }

    #[test]
    fn flv_avc_nalu_delta_uses_inter_frame_type() {
        let nalu = synthetic_p_slice_nal();
        let tag = flv_avc_nalu(false, 33, &nalu);
        assert_eq!(tag[0], 0x27, "frame_type=2 (inter) + codec=7");
        assert_eq!(tag[1], 0x01);
        // CTS field = 33 (24-bit big-endian).
        assert_eq!(tag[2], 0x00);
        assert_eq!(tag[3], 0x00);
        assert_eq!(tag[4], 0x21);
    }
}
