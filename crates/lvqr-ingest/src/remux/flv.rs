//! FLV tag parser for RTMP video/audio data.
//!
//! Parses the tag bodies that rml_rtmp delivers via VideoDataReceived/AudioDataReceived.
//! These are the bytes AFTER the FLV tag header (rml_rtmp strips the tag header itself).

use bytes::Bytes;

/// H.264 codec configuration extracted from an FLV AVC sequence header.
#[derive(Debug, Clone)]
pub struct VideoConfig {
    pub sps: Vec<u8>,
    pub pps: Vec<u8>,
    pub profile: u8,
    pub compat: u8,
    pub level: u8,
    /// AVCC NALU length prefix size in bytes (1, 2, or 4). Almost always 4.
    pub nalu_length_size: u8,
}

impl VideoConfig {
    /// Generate the codec string for MSE (e.g. "avc1.64001F").
    pub fn codec_string(&self) -> String {
        format!("avc1.{:02X}{:02X}{:02X}", self.profile, self.compat, self.level)
    }
}

/// AAC codec configuration extracted from an FLV AAC sequence header.
#[derive(Debug, Clone)]
pub struct AudioConfig {
    /// Raw AudioSpecificConfig bytes (typically 2 bytes for AAC-LC).
    pub asc: Vec<u8>,
    pub sample_rate: u32,
    pub channels: u8,
    pub object_type: u8,
}

impl AudioConfig {
    /// Generate the codec string for MSE (e.g. "mp4a.40.2").
    pub fn codec_string(&self) -> String {
        format!("mp4a.40.{}", self.object_type)
    }
}

/// Parsed FLV video tag.
#[derive(Debug)]
pub enum FlvVideoTag {
    /// AVC sequence header containing codec configuration.
    SequenceHeader(VideoConfig),
    /// AVC NALU data (one or more length-prefixed NALUs in AVCC format).
    Nalu {
        keyframe: bool,
        /// Composition time offset in milliseconds (signed).
        cts: i32,
        /// Raw AVCC NALU data (length-prefixed, NOT Annex B).
        data: Bytes,
    },
    /// End of sequence marker.
    EndOfSequence,
    /// Non-AVC codec or unrecognized data.
    Unknown,
}

/// Parsed FLV audio tag.
#[derive(Debug)]
pub enum FlvAudioTag {
    /// AAC sequence header containing codec configuration.
    SequenceHeader(AudioConfig),
    /// Raw AAC frame data.
    RawAac(Bytes),
    /// Non-AAC codec or unrecognized data.
    Unknown,
}

/// Parse an FLV video tag body.
///
/// FLV video tag format:
/// - byte 0: frame_type (upper nibble) | codec_id (lower nibble)
/// - byte 1: AVC packet type (0=sequence header, 1=NALU, 2=end of sequence)
/// - bytes 2-4: composition time offset (i24 big-endian, signed)
/// - bytes 5+: payload
pub fn parse_video_tag(data: &Bytes) -> FlvVideoTag {
    if data.len() < 2 {
        return FlvVideoTag::Unknown;
    }

    let codec_id = data[0] & 0x0F;
    if codec_id != 7 {
        // Not AVC/H.264
        return FlvVideoTag::Unknown;
    }

    let keyframe = (data[0] >> 4) == 1;
    let avc_packet_type = data[1];

    match avc_packet_type {
        0 => {
            // AVC sequence header (AVCDecoderConfigurationRecord)
            if data.len() < 10 {
                return FlvVideoTag::Unknown;
            }
            match parse_avcc_record(&data[5..]) {
                Some(config) => FlvVideoTag::SequenceHeader(config),
                None => FlvVideoTag::Unknown,
            }
        }
        1 => {
            // AVC NALU(s)
            if data.len() < 5 {
                return FlvVideoTag::Unknown;
            }
            let cts = ((data[2] as i32) << 16) | ((data[3] as i32) << 8) | (data[4] as i32);
            // Sign-extend from 24 bits
            let cts = if cts & 0x800000 != 0 { cts | !0xFFFFFF } else { cts };
            let nalu_data = data.slice(5..);
            FlvVideoTag::Nalu {
                keyframe,
                cts,
                data: nalu_data,
            }
        }
        2 => FlvVideoTag::EndOfSequence,
        _ => FlvVideoTag::Unknown,
    }
}

/// Parse an AVCDecoderConfigurationRecord to extract SPS, PPS, profile, level.
fn parse_avcc_record(data: &[u8]) -> Option<VideoConfig> {
    if data.len() < 6 {
        return None;
    }

    let _config_version = data[0]; // should be 1
    let profile = data[1];
    let compat = data[2];
    let level = data[3];
    let nalu_length_size = (data[4] & 0x03) + 1; // lower 2 bits + 1

    let num_sps = (data[5] & 0x1F) as usize; // lower 5 bits
    let mut offset = 6;

    let mut sps = Vec::new();
    for _ in 0..num_sps {
        if offset + 2 > data.len() {
            return None;
        }
        let sps_len = u16::from_be_bytes([data[offset], data[offset + 1]]) as usize;
        offset += 2;
        if offset + sps_len > data.len() {
            return None;
        }
        sps = data[offset..offset + sps_len].to_vec();
        offset += sps_len;
    }

    if offset >= data.len() {
        return None;
    }
    let num_pps = data[offset] as usize;
    offset += 1;

    let mut pps = Vec::new();
    for _ in 0..num_pps {
        if offset + 2 > data.len() {
            return None;
        }
        let pps_len = u16::from_be_bytes([data[offset], data[offset + 1]]) as usize;
        offset += 2;
        if offset + pps_len > data.len() {
            return None;
        }
        pps = data[offset..offset + pps_len].to_vec();
        offset += pps_len;
    }

    if sps.is_empty() || pps.is_empty() {
        return None;
    }

    Some(VideoConfig {
        sps,
        pps,
        profile,
        compat,
        level,
        nalu_length_size,
    })
}

/// Parse an FLV audio tag body.
///
/// FLV audio tag format:
/// - byte 0: format (upper nibble) | rate (bits 3-2) | size (bit 1) | type (bit 0)
/// - byte 1: AAC packet type (0=AudioSpecificConfig, 1=raw AAC frame)
/// - bytes 2+: payload
pub fn parse_audio_tag(data: &Bytes) -> FlvAudioTag {
    if data.len() < 2 {
        return FlvAudioTag::Unknown;
    }

    let format = data[0] >> 4;
    if format != 10 {
        // Not AAC
        return FlvAudioTag::Unknown;
    }

    let aac_packet_type = data[1];

    match aac_packet_type {
        0 => {
            // AudioSpecificConfig
            if data.len() < 4 {
                return FlvAudioTag::Unknown;
            }
            let asc = data[2..].to_vec();
            match parse_audio_specific_config(&asc) {
                Some(config) => FlvAudioTag::SequenceHeader(config),
                None => FlvAudioTag::Unknown,
            }
        }
        1 => {
            // Raw AAC frame data
            if data.len() <= 2 {
                return FlvAudioTag::Unknown;
            }
            FlvAudioTag::RawAac(data.slice(2..))
        }
        _ => FlvAudioTag::Unknown,
    }
}

/// Sampling frequency index table from ISO 14496-3.
const AAC_SAMPLE_RATES: [u32; 13] = [
    96000, 88200, 64000, 48000, 44100, 32000, 24000, 22050, 16000, 12000, 11025, 8000, 7350,
];

/// Parse an AudioSpecificConfig (ISO 14496-3 section 1.6.2.1).
fn parse_audio_specific_config(asc: &[u8]) -> Option<AudioConfig> {
    if asc.len() < 2 {
        return None;
    }

    // AudioSpecificConfig is bit-packed:
    // bits 0-4: audioObjectType (5 bits)
    // bits 5-8: samplingFrequencyIndex (4 bits)
    // bits 9-12: channelConfiguration (4 bits)
    let object_type = asc[0] >> 3;
    let freq_index = (((asc[0] & 0x07) << 1) | (asc[1] >> 7)) as usize;
    let channels = (asc[1] >> 3) & 0x0F;

    let sample_rate = if freq_index < AAC_SAMPLE_RATES.len() {
        AAC_SAMPLE_RATES[freq_index]
    } else {
        return None;
    };

    Some(AudioConfig {
        asc: asc.to_vec(),
        sample_rate,
        channels,
        object_type,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_avcc_record(profile: u8, compat: u8, level: u8, sps: &[u8], pps: &[u8]) -> Vec<u8> {
        let mut rec = vec![
            0x01,                     // configurationVersion
            profile,                  // AVCProfileIndication
            compat,                   // profile_compatibility
            level,                    // AVCLevelIndication
            0xFF,                     // lengthSizeMinusOne=3 | reserved
            0xE1,                     // numSPS=1 | reserved
            (sps.len() >> 8) as u8,   // spsLength high
            (sps.len() & 0xFF) as u8, // spsLength low
        ];
        rec.extend_from_slice(sps);
        rec.push(0x01); // numPPS=1
        rec.push((pps.len() >> 8) as u8);
        rec.push((pps.len() & 0xFF) as u8);
        rec.extend_from_slice(pps);
        rec
    }

    fn make_video_seq_header(profile: u8, compat: u8, level: u8, sps: &[u8], pps: &[u8]) -> Bytes {
        let mut tag = vec![
            0x17, // keyframe + AVC
            0x00, // AVC sequence header
            0x00, 0x00, 0x00, // CTS = 0
        ];
        tag.extend_from_slice(&make_avcc_record(profile, compat, level, sps, pps));
        Bytes::from(tag)
    }

    fn make_video_nalu(keyframe: bool, cts: i32, nalu_data: &[u8]) -> Bytes {
        let frame_type = if keyframe { 0x17 } else { 0x27 };
        let cts_bytes = [(cts >> 16) as u8, (cts >> 8) as u8, cts as u8];
        let mut tag = vec![frame_type, 0x01, cts_bytes[0], cts_bytes[1], cts_bytes[2]];
        tag.extend_from_slice(nalu_data);
        Bytes::from(tag)
    }

    fn make_audio_seq_header(object_type: u8, freq_index: u8, channels: u8) -> Bytes {
        // AudioSpecificConfig: 5 bits objectType + 4 bits freqIndex + 4 bits channels
        let b0 = (object_type << 3) | (freq_index >> 1);
        let b1 = (freq_index << 7) | (channels << 3);
        Bytes::from(vec![0xAF, 0x00, b0, b1])
    }

    fn make_audio_raw(aac_data: &[u8]) -> Bytes {
        let mut tag = vec![0xAF, 0x01];
        tag.extend_from_slice(aac_data);
        Bytes::from(tag)
    }

    #[test]
    fn parse_video_sequence_header() {
        let sps = vec![0x67, 0x64, 0x00, 0x1F, 0xAC, 0xD9];
        let pps = vec![0x68, 0xEE, 0x3C, 0x80];
        let data = make_video_seq_header(0x64, 0x00, 0x1F, &sps, &pps);

        match parse_video_tag(&data) {
            FlvVideoTag::SequenceHeader(config) => {
                assert_eq!(config.profile, 0x64);
                assert_eq!(config.compat, 0x00);
                assert_eq!(config.level, 0x1F);
                assert_eq!(config.nalu_length_size, 4);
                assert_eq!(config.sps, sps);
                assert_eq!(config.pps, pps);
                assert_eq!(config.codec_string(), "avc1.64001F");
            }
            other => panic!("expected SequenceHeader, got {other:?}"),
        }
    }

    #[test]
    fn parse_video_nalu_keyframe() {
        let nalu_data = vec![0x00, 0x00, 0x00, 0x04, 0x65, 0x88, 0x84, 0x00];
        let data = make_video_nalu(true, 0, &nalu_data);

        match parse_video_tag(&data) {
            FlvVideoTag::Nalu { keyframe, cts, data } => {
                assert!(keyframe);
                assert_eq!(cts, 0);
                assert_eq!(&data[..], &nalu_data);
            }
            other => panic!("expected Nalu, got {other:?}"),
        }
    }

    #[test]
    fn parse_video_nalu_delta_with_cts() {
        let nalu_data = vec![0x00, 0x00, 0x00, 0x03, 0x41, 0x9A, 0x00];
        let data = make_video_nalu(false, 66, &nalu_data);

        match parse_video_tag(&data) {
            FlvVideoTag::Nalu { keyframe, cts, data } => {
                assert!(!keyframe);
                assert_eq!(cts, 66);
                assert_eq!(&data[..], &nalu_data);
            }
            other => panic!("expected Nalu, got {other:?}"),
        }
    }

    #[test]
    fn parse_video_end_of_sequence() {
        let data = Bytes::from(vec![0x17, 0x02, 0x00, 0x00, 0x00]);
        assert!(matches!(parse_video_tag(&data), FlvVideoTag::EndOfSequence));
    }

    #[test]
    fn parse_video_non_avc_codec() {
        // VP6 codec (codec_id = 4)
        let data = Bytes::from(vec![0x14, 0x01, 0x00, 0x00, 0x00]);
        assert!(matches!(parse_video_tag(&data), FlvVideoTag::Unknown));
    }

    #[test]
    fn parse_video_truncated_data() {
        assert!(matches!(parse_video_tag(&Bytes::new()), FlvVideoTag::Unknown));
        assert!(matches!(
            parse_video_tag(&Bytes::from(vec![0x17])),
            FlvVideoTag::Unknown
        ));
    }

    #[test]
    fn parse_audio_sequence_header() {
        // AAC-LC (object_type=2), 44100 Hz (freq_index=4), stereo (channels=2)
        let data = make_audio_seq_header(2, 4, 2);

        match parse_audio_tag(&data) {
            FlvAudioTag::SequenceHeader(config) => {
                assert_eq!(config.object_type, 2);
                assert_eq!(config.sample_rate, 44100);
                assert_eq!(config.channels, 2);
                assert_eq!(config.codec_string(), "mp4a.40.2");
            }
            other => panic!("expected SequenceHeader, got {other:?}"),
        }
    }

    #[test]
    fn parse_audio_sequence_header_48khz() {
        // AAC-LC, 48000 Hz (freq_index=3), stereo
        let data = make_audio_seq_header(2, 3, 2);

        match parse_audio_tag(&data) {
            FlvAudioTag::SequenceHeader(config) => {
                assert_eq!(config.sample_rate, 48000);
            }
            other => panic!("expected SequenceHeader, got {other:?}"),
        }
    }

    #[test]
    fn parse_audio_raw_aac() {
        let aac_frame = vec![0x01, 0x02, 0x03, 0x04, 0x05];
        let data = make_audio_raw(&aac_frame);

        match parse_audio_tag(&data) {
            FlvAudioTag::RawAac(raw) => {
                assert_eq!(&raw[..], &aac_frame);
            }
            other => panic!("expected RawAac, got {other:?}"),
        }
    }

    #[test]
    fn parse_audio_non_aac() {
        // MP3 (format = 2)
        let data = Bytes::from(vec![0x2F, 0x01, 0x00]);
        assert!(matches!(parse_audio_tag(&data), FlvAudioTag::Unknown));
    }

    #[test]
    fn video_config_codec_string_baseline() {
        let sps = vec![0x67, 0x42, 0xC0, 0x1E];
        let pps = vec![0x68, 0xCE];
        let data = make_video_seq_header(0x42, 0xC0, 0x1E, &sps, &pps);

        match parse_video_tag(&data) {
            FlvVideoTag::SequenceHeader(config) => {
                assert_eq!(config.codec_string(), "avc1.42C01E");
            }
            other => panic!("expected SequenceHeader, got {other:?}"),
        }
    }

    #[test]
    fn parse_negative_cts() {
        // CTS = -33 (0xFFFFDF in 24-bit signed)
        let nalu_data = vec![0x00, 0x00, 0x00, 0x01, 0x41];
        let data = make_video_nalu(false, -33, &nalu_data);

        match parse_video_tag(&data) {
            FlvVideoTag::Nalu { cts, .. } => {
                assert_eq!(cts, -33);
            }
            other => panic!("expected Nalu, got {other:?}"),
        }
    }
}
