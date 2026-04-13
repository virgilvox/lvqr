//! MoQ catalog track generator.
//!
//! Produces a JSON catalog compatible with the MoQ ecosystem (kixelated/moq-js).

use super::flv::{AudioConfig, VideoConfig};

/// Generate a MoQ catalog JSON string from video and audio configurations.
pub fn generate_catalog(video: Option<&VideoConfig>, audio: Option<&AudioConfig>) -> String {
    let mut tracks = Vec::new();

    if let Some(v) = video {
        tracks.push(format!(
            r#"{{"name":"0.mp4","packaging":"cmaf","renderGroup":0,"codec":"{}","mimeType":"video/mp4"}}"#,
            v.codec_string()
        ));
    }

    if let Some(a) = audio {
        tracks.push(format!(
            r#"{{"name":"1.mp4","packaging":"cmaf","renderGroup":1,"codec":"{}","mimeType":"audio/mp4","samplerate":{},"channelCount":{}}}"#,
            a.codec_string(),
            a.sample_rate,
            a.channels
        ));
    }

    format!(r#"{{"version":1,"tracks":[{}]}}"#, tracks.join(","))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_with_video_and_audio() {
        let video = VideoConfig {
            sps_list: vec![vec![0x67]],
            pps_list: vec![vec![0x68]],
            profile: 0x64,
            compat: 0x00,
            level: 0x1F,
            nalu_length_size: 4,
        };
        let audio = AudioConfig {
            asc: vec![0x12, 0x10],
            sample_rate: 44100,
            channels: 2,
            object_type: 2,
        };

        let catalog = generate_catalog(Some(&video), Some(&audio));
        assert!(catalog.contains(r#""codec":"avc1.64001F""#));
        assert!(catalog.contains(r#""codec":"mp4a.40.2""#));
        assert!(catalog.contains(r#""samplerate":44100"#));
        assert!(catalog.contains(r#""channelCount":2"#));
    }

    #[test]
    fn catalog_video_only() {
        let video = VideoConfig {
            sps_list: vec![vec![0x67]],
            pps_list: vec![vec![0x68]],
            profile: 0x42,
            compat: 0xC0,
            level: 0x1E,
            nalu_length_size: 4,
        };
        let catalog = generate_catalog(Some(&video), None);
        assert!(catalog.contains(r#""codec":"avc1.42C01E""#));
        assert!(!catalog.contains("mp4a"));
    }
}
