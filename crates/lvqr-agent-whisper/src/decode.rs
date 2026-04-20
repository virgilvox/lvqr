//! AAC -> mono 16 kHz f32 PCM decode pipeline (whisper-input
//! shape) for [`crate::worker`].
//!
//! whisper.cpp eats mono 16 kHz f32 samples in `[-1.0, 1.0]`.
//! LVQR's audio fragments carry stereo or mono AAC-LC at the
//! source sample rate (typically 44.1 kHz or 48 kHz). This
//! module owns the conversion: symphonia AAC decode -> channel
//! downmix -> nearest-neighbour resample to 16 kHz.
//!
//! Resampling is intentionally crude (nearest neighbour with
//! integer ratio computed per-call) -- whisper's input
//! tolerance is wide enough that aliasing artifacts on speech
//! audio do not measurably degrade transcription quality, and
//! pulling in `rubato` adds another transitive closure for a
//! marginal gain. If session 99 C surfaces a quality
//! regression, swapping in a polyphase resampler is a
//! single-file change inside this module.

use std::sync::Arc;

use bytes::Bytes;
use symphonia::core::codecs::audio::well_known::CODEC_ID_AAC;
use symphonia::core::codecs::audio::{AudioCodecParameters, AudioDecoder, AudioDecoderOptions};
use symphonia::core::packet::Packet;
use symphonia::core::units::{Duration, Timestamp};
use symphonia::default::get_codecs;
use thiserror::Error;

/// whisper.cpp's required input sample rate.
pub(crate) const WHISPER_SAMPLE_RATE: u32 = 16_000;

#[derive(Debug, Error)]
pub(crate) enum DecodeError {
    #[error("symphonia codec init failed: {0}")]
    CodecInit(String),

    #[error("symphonia decode failed: {0}")]
    Decode(String),
}

/// Stateful AAC decoder + resampler. One instance per
/// `(broadcast, track)` agent. Holds the AAC decoder context
/// across calls so the AAC-LC predictive state stays warm.
pub(crate) struct AacToMonoF32 {
    decoder: Box<dyn AudioDecoder>,
    source_sample_rate: u32,
    /// Reusable interleaved-PCM buffer. Held across calls so we
    /// avoid one heap allocation per AAC frame on the hot path.
    interleaved: Vec<f32>,
}

impl AacToMonoF32 {
    /// Construct a fresh decoder configured against the
    /// supplied AAC `AudioSpecificConfig` (extracted from the
    /// fragment broadcaster's init segment).
    pub fn new(asc: Arc<Bytes>, source_sample_rate: u32) -> Result<Self, DecodeError> {
        let registry = get_codecs();
        let registered = registry
            .get_audio_decoder(CODEC_ID_AAC)
            .ok_or_else(|| DecodeError::CodecInit("AAC decoder not registered with symphonia".into()))?;

        let mut params = AudioCodecParameters::new();
        params
            .for_codec(CODEC_ID_AAC)
            .with_sample_rate(source_sample_rate)
            .with_extra_data(asc.to_vec().into_boxed_slice());

        let decoder = (registered.factory)(&params, &AudioDecoderOptions::default())
            .map_err(|e| DecodeError::CodecInit(format!("{e}")))?;

        Ok(Self {
            decoder,
            source_sample_rate,
            interleaved: Vec::with_capacity(2048),
        })
    }

    /// Decode one raw AAC frame and return mono 16 kHz f32
    /// samples ready to be appended to the whisper PCM ring.
    ///
    /// `dts` is the fragment's DTS in the source timescale; it
    /// is used as the symphonia packet timestamp so the decoder
    /// can do its own internal accounting.
    pub fn decode_frame(&mut self, dts: u64, aac: &Bytes) -> Result<Vec<f32>, DecodeError> {
        let pts = Timestamp::from(dts as i64);
        let dur = Duration::from(1024u64);
        let packet = Packet::new(0, pts, dur, aac.to_vec());
        let buf_ref = self
            .decoder
            .decode(&packet)
            .map_err(|e| DecodeError::Decode(format!("{e}")))?;

        let channels = buf_ref.spec().channels().count().max(1);
        self.interleaved.clear();
        buf_ref.copy_to_vec_interleaved::<f32>(&mut self.interleaved);

        // Downmix interleaved -> mono by averaging across channels.
        let mut mono = Vec::with_capacity(self.interleaved.len() / channels);
        for frame in self.interleaved.chunks(channels) {
            let sum: f32 = frame.iter().sum();
            mono.push(sum / channels as f32);
        }
        Ok(resample_nearest(&mono, self.source_sample_rate, WHISPER_SAMPLE_RATE))
    }
}

/// Nearest-neighbour resample. `mono` is in `src_rate` Hz;
/// returns samples at `dst_rate` Hz. Cheap; designed for
/// whisper's tolerant 16 kHz target.
fn resample_nearest(mono: &[f32], src_rate: u32, dst_rate: u32) -> Vec<f32> {
    if src_rate == dst_rate || mono.is_empty() {
        return mono.to_vec();
    }
    let out_count = (mono.len() as u64 * dst_rate as u64 / src_rate as u64) as usize;
    let mut out = Vec::with_capacity(out_count);
    let ratio = src_rate as f64 / dst_rate as f64;
    for i in 0..out_count {
        let src_idx = (i as f64 * ratio) as usize;
        if src_idx >= mono.len() {
            break;
        }
        out.push(mono[src_idx]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resample_identity_when_rates_match() {
        let pcm: Vec<f32> = (0..16).map(|i| i as f32).collect();
        let out = resample_nearest(&pcm, 16_000, 16_000);
        assert_eq!(out, pcm);
    }

    #[test]
    fn resample_downsample_44100_to_16000_is_correct_length() {
        let pcm = vec![0.5_f32; 44_100];
        let out = resample_nearest(&pcm, 44_100, 16_000);
        assert_eq!(out.len(), 16_000);
        assert!(out.iter().all(|&v| v == 0.5));
    }

    #[test]
    fn resample_empty_input_yields_empty_output() {
        let out = resample_nearest(&[], 48_000, 16_000);
        assert!(out.is_empty());
    }

    #[test]
    fn resample_upsample_8000_to_16000_doubles_length() {
        let pcm: Vec<f32> = (0..8).map(|i| i as f32).collect();
        let out = resample_nearest(&pcm, 8_000, 16_000);
        assert_eq!(out.len(), 16);
        assert_eq!(out[0], 0.0);
        assert_eq!(out[1], 0.0);
        assert_eq!(out[2], 1.0);
    }
}
