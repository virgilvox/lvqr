//! Shared FLV tag builders for integration tests (session 130).
//!
//! Every RTMP-ingest integration test historically drives the session
//! with the same fixed H.264 High@L3.1 SPS+PPS pair plus AAC-LC audio
//! tags. Every test file reimplemented its own copy of these byte
//! builders. This module centralizes the primitives so adopters get:
//!
//! * One canonical AVCDecoderConfigurationRecord for 1920x1080 High@L3.1.
//! * Keyframe vs delta-frame NALU tag assembly in one helper.
//! * Parameterized AAC-LC AudioSpecificConfig byte math with a
//!   convenience wrapper for the common 44.1 kHz / stereo case.
//!
//! The wire shapes follow FLV 1.0 (video codec id 7 = AVC; audio codec
//! id 10 = AAC) and ISO/IEC 14496-3 5.3 (AudioSpecificConfig). See the
//! function doc comments for precise byte layouts.
//!
//! The session-114 WHEP audio bridge requires 48 kHz AAC input; its
//! test builds the 48 kHz AudioSpecificConfig inline because no other
//! caller uses that sample rate. Add a convenience wrapper here if a
//! second 48 kHz test ever appears.

use bytes::Bytes;

/// FLV video tag carrying an AVCDecoderConfigurationRecord (SPS + PPS).
/// Fixed H.264 High profile Level 3.1 shape that every integration
/// test uses unchanged. Must precede any NALU payload on the RTMP
/// session.
///
/// Wire layout: `frame_type=0x17` (keyframe|AVC), `packet_type=0x00`
/// (seq header), `composition_time=0`, then the AVCC record: version,
/// profile, compat, level, NALU length-size-minus-one, SPS count,
/// SPS length+bytes, PPS count, PPS length+bytes.
pub fn flv_video_seq_header() -> Bytes {
    let sps = [0x67, 0x64, 0x00, 0x1F, 0xAC, 0xD9];
    let pps = [0x68, 0xEE, 0x3C, 0x80];
    let mut tag = vec![0x17, 0x00, 0x00, 0x00, 0x00, 0x01, 0x64, 0x00, 0x1F, 0xFF, 0xE1];
    tag.extend_from_slice(&(sps.len() as u16).to_be_bytes());
    tag.extend_from_slice(&sps);
    tag.push(0x01);
    tag.extend_from_slice(&(pps.len() as u16).to_be_bytes());
    tag.extend_from_slice(&pps);
    Bytes::from(tag)
}

/// FLV video tag carrying an H.264 NALU. `keyframe = true` sets the
/// frame-type nibble to 1 (IDR / keyframe); `false` marks it as a
/// non-IDR slice. `cts` is the composition-time offset in
/// milliseconds (signed 24-bit big-endian). `nalu_data` is the AVCC-
/// formatted payload (4-byte length prefix + EBSP bytes, repeated
/// per NAL unit).
pub fn flv_video_nalu(keyframe: bool, cts: i32, nalu_data: &[u8]) -> Bytes {
    let frame_type = if keyframe { 0x17 } else { 0x27 };
    let mut tag = vec![frame_type, 0x01, (cts >> 16) as u8, (cts >> 8) as u8, cts as u8];
    tag.extend_from_slice(nalu_data);
    Bytes::from(tag)
}

/// FLV audio tag carrying an AAC-LC AudioSpecificConfig. `sample_freq_index`
/// is the ISO/IEC 14496-3 sampling_frequency_index (e.g. `3` for
/// 48 kHz, `4` for 44.1 kHz). `channels` is the
/// channel_configuration (e.g. `1` for mono, `2` for stereo).
///
/// Wire layout: `audio_tag_header=0xAF` (AAC|44.1 kHz|16-bit|stereo;
/// FLV ignores the header's rate/size/channel bits when codec_id is
/// AAC, the truth is in the ASC bytes), `packet_type=0x00` (seq
/// header), then the 16-bit ASC: `[obj:5][freq_idx:4][chan:4][pad:3]`.
pub fn flv_audio_aac_lc_seq_header(sample_freq_index: u8, channels: u8) -> Bytes {
    let obj_type: u8 = 2;
    let b0 = (obj_type << 3) | (sample_freq_index >> 1);
    let b1 = ((sample_freq_index & 0x01) << 7) | (channels << 3);
    Bytes::from(vec![0xAF, 0x00, b0, b1])
}

/// 44.1 kHz / stereo / AAC-LC convenience wrapper. Sampling-frequency
/// index 4 + channel-config 2; matches what every non-WHEP RTMP
/// integration test uses.
pub fn flv_audio_aac_lc_seq_header_44k_stereo() -> Bytes {
    flv_audio_aac_lc_seq_header(4, 2)
}

/// FLV audio tag carrying a raw AAC-LC access unit payload. Must be
/// preceded at least once by a seq header tag (see
/// [`flv_audio_aac_lc_seq_header`]) or the decoder has no
/// AudioSpecificConfig to parse.
pub fn flv_audio_raw(aac_data: &[u8]) -> Bytes {
    let mut tag = vec![0xAF, 0x01];
    tag.extend_from_slice(aac_data);
    Bytes::from(tag)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn video_seq_header_matches_historical_bytes() {
        let expected: &[u8] = &[
            0x17, 0x00, 0x00, 0x00, 0x00, 0x01, 0x64, 0x00, 0x1F, 0xFF, 0xE1, 0x00, 0x06, 0x67, 0x64, 0x00, 0x1F, 0xAC,
            0xD9, 0x01, 0x00, 0x04, 0x68, 0xEE, 0x3C, 0x80,
        ];
        assert_eq!(&flv_video_seq_header()[..], expected);
    }

    #[test]
    fn video_nalu_keyframe_flag_flips_frame_type_nibble() {
        let key = flv_video_nalu(true, 0, &[0x00]);
        let delta = flv_video_nalu(false, 0, &[0x00]);
        assert_eq!(key[0], 0x17);
        assert_eq!(delta[0], 0x27);
    }

    #[test]
    fn video_nalu_composition_time_encodes_as_signed_24bit_be() {
        let tag = flv_video_nalu(false, 0x123456, &[]);
        assert_eq!(&tag[2..5], &[0x12, 0x34, 0x56]);
    }

    #[test]
    fn video_nalu_appends_payload_verbatim() {
        let tag = flv_video_nalu(true, 0, &[0xAA, 0xBB, 0xCC]);
        assert_eq!(&tag[5..], &[0xAA, 0xBB, 0xCC]);
    }

    #[test]
    fn audio_seq_header_44k_stereo_matches_historical_bytes() {
        assert_eq!(&flv_audio_aac_lc_seq_header_44k_stereo()[..], &[0xAF, 0x00, 0x12, 0x10]);
    }

    #[test]
    fn audio_seq_header_48k_stereo_matches_session_114_bytes() {
        // Session 114's rtmp_whep_audio_e2e.rs uses 48 kHz / stereo
        // to match its WHEP audio bridge; confirm the parameterized
        // helper reproduces those exact bytes.
        assert_eq!(&flv_audio_aac_lc_seq_header(3, 2)[..], &[0xAF, 0x00, 0x11, 0x90]);
    }

    #[test]
    fn audio_raw_prepends_packet_type_tag() {
        let tag = flv_audio_raw(&[0xDE, 0xAD, 0xBE, 0xEF]);
        assert_eq!(&tag[..], &[0xAF, 0x01, 0xDE, 0xAD, 0xBE, 0xEF]);
    }
}
