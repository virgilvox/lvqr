/// Bridge between RTMP ingest and MoQ Origin.
///
/// When an RTMP publisher connects, this module creates a MoQ broadcast
/// with CMAF-formatted tracks. Video and audio data from RTMP is remuxed
/// from FLV to fMP4 (CMAF) segments, compatible with MSE browser playback
/// and the MoQ ecosystem (moq-js).
use bytes::Bytes;
use dashmap::DashMap;
use moq_lite::Track;
use std::sync::Arc;
use tracing::{debug, info, warn};

use crate::remux::{
    AudioConfig, FlvAudioTag, FlvVideoTag, VideoConfig, VideoSample, audio_init_segment, audio_segment,
    generate_catalog, parse_audio_tag, parse_video_tag, video_init_segment, video_segment,
};
use crate::rtmp::{MediaCallback, RtmpConfig, RtmpServer, StreamCallback};

/// State for a single active RTMP stream being bridged to MoQ.
struct ActiveStream {
    _broadcast: moq_lite::BroadcastProducer,
    video_track: moq_lite::TrackProducer,
    audio_track: moq_lite::TrackProducer,
    catalog_track: moq_lite::TrackProducer,
    video_group: Option<moq_lite::GroupProducer>,
    // Codec configuration (set when sequence headers arrive)
    video_config: Option<VideoConfig>,
    audio_config: Option<AudioConfig>,
    video_init: Option<Bytes>,
    audio_init: Option<Bytes>,
    catalog_written: bool,
    // Segment sequencing
    video_seq: u32,
    audio_seq: u32,
    // DTS tracking (90kHz for video, sample_rate for audio)
    last_video_ts: Option<u32>,
}

/// Bridges RTMP ingest to a MoQ OriginProducer.
///
/// Creates MoQ broadcasts for each RTMP stream and remuxes video/audio
/// data from FLV to CMAF/fMP4 segments.
pub struct RtmpMoqBridge {
    origin: moq_lite::OriginProducer,
    streams: Arc<DashMap<String, ActiveStream>>,
}

impl RtmpMoqBridge {
    pub fn new(origin: moq_lite::OriginProducer) -> Self {
        Self {
            origin,
            streams: Arc::new(DashMap::new()),
        }
    }

    /// Create an RTMP server wired to this bridge.
    pub fn create_rtmp_server(&self, config: RtmpConfig) -> RtmpServer {
        let origin = self.origin.clone();
        let streams_publish = self.streams.clone();
        let streams_unpublish = self.streams.clone();
        let streams_video = self.streams.clone();
        let streams_audio = self.streams.clone();

        let on_publish: StreamCallback = Arc::new(move |app: &str, key: &str| {
            let stream_name = format!("{app}/{key}");
            info!(stream = %stream_name, "creating MoQ broadcast for RTMP stream");

            let Some(mut broadcast) = origin.create_broadcast(&stream_name) else {
                warn!(stream = %stream_name, "broadcast not allowed by origin");
                return;
            };

            // Track names compatible with moq-js CMAF convention
            let video_track = match broadcast.create_track(Track::new("0.mp4")) {
                Ok(t) => t,
                Err(e) => {
                    warn!(stream = %stream_name, error = ?e, "failed to create video track");
                    return;
                }
            };

            let audio_track = match broadcast.create_track(Track::new("1.mp4")) {
                Ok(t) => t,
                Err(e) => {
                    warn!(stream = %stream_name, error = ?e, "failed to create audio track");
                    return;
                }
            };

            let catalog_track = match broadcast.create_track(Track::new(".catalog")) {
                Ok(t) => t,
                Err(e) => {
                    warn!(stream = %stream_name, error = ?e, "failed to create catalog track");
                    return;
                }
            };

            streams_publish.insert(
                stream_name,
                ActiveStream {
                    _broadcast: broadcast,
                    video_track,
                    audio_track,
                    catalog_track,
                    video_group: None,
                    video_config: None,
                    audio_config: None,
                    video_init: None,
                    audio_init: None,
                    catalog_written: false,
                    video_seq: 0,
                    audio_seq: 0,
                    last_video_ts: None,
                },
            );
        });

        let on_unpublish: StreamCallback = Arc::new(move |app: &str, key: &str| {
            let stream_name = format!("{app}/{key}");
            if let Some((_, mut stream)) = streams_unpublish.remove(&stream_name) {
                if let Some(mut group) = stream.video_group.take() {
                    let _ = group.finish();
                }
                info!(stream = %stream_name, "removed MoQ broadcast");
            }
        });

        let on_video: MediaCallback = Arc::new(move |app: &str, key: &str, data: Bytes, timestamp: u32| {
            let stream_name = format!("{app}/{key}");
            let Some(mut entry) = streams_video.get_mut(&stream_name) else {
                return;
            };
            let stream = entry.value_mut();

            match parse_video_tag(&data) {
                FlvVideoTag::SequenceHeader(config) => {
                    debug!(stream = %stream_name, codec = %config.codec_string(), "video sequence header");
                    let init = video_init_segment(&config);
                    stream.video_config = Some(config);
                    stream.video_init = Some(init);
                    maybe_write_catalog(stream, &stream_name);
                }
                FlvVideoTag::Nalu {
                    keyframe,
                    cts,
                    data: nalu_data,
                } => {
                    let Some(ref _config) = stream.video_config else {
                        return; // no sequence header yet
                    };
                    let Some(ref init) = stream.video_init else {
                        return;
                    };

                    // Compute duration from timestamp delta (default 33ms = ~30fps)
                    let duration_ms = match stream.last_video_ts {
                        Some(prev) => timestamp.saturating_sub(prev),
                        None => 33,
                    };
                    stream.last_video_ts = Some(timestamp);
                    let duration_ticks = duration_ms * 90; // 90kHz timescale
                    let base_dts = (timestamp as u64) * 90;

                    let sample = VideoSample {
                        data: nalu_data,
                        duration: duration_ticks,
                        cts_offset: cts * 90, // ms to 90kHz
                        keyframe,
                    };

                    if keyframe {
                        // Finish previous group
                        if let Some(mut group) = stream.video_group.take() {
                            let _ = group.finish();
                        }

                        stream.video_seq += 1;

                        // Start new group: init segment as frame 0, keyframe as frame 1
                        match stream.video_track.append_group() {
                            Ok(mut group) => {
                                if let Err(e) = group.write_frame(init.clone()) {
                                    debug!(error = ?e, "failed to write video init segment");
                                    return;
                                }
                                let seg = video_segment(stream.video_seq, base_dts, &[sample]);
                                if let Err(e) = group.write_frame(seg) {
                                    debug!(error = ?e, "failed to write video keyframe segment");
                                    return;
                                }
                                stream.video_group = Some(group);
                            }
                            Err(e) => {
                                debug!(error = ?e, "failed to append video group");
                            }
                        }
                    } else if let Some(ref mut group) = stream.video_group {
                        stream.video_seq += 1;
                        let seg = video_segment(stream.video_seq, base_dts, &[sample]);
                        if let Err(e) = group.write_frame(seg) {
                            debug!(error = ?e, "failed to write video delta segment");
                        }
                    }
                }
                FlvVideoTag::EndOfSequence => {
                    if let Some(mut group) = stream.video_group.take() {
                        let _ = group.finish();
                    }
                }
                FlvVideoTag::Unknown => {}
            }
        });

        let on_audio: MediaCallback = Arc::new(move |app: &str, key: &str, data: Bytes, timestamp: u32| {
            let stream_name = format!("{app}/{key}");
            let Some(mut entry) = streams_audio.get_mut(&stream_name) else {
                return;
            };
            let stream = entry.value_mut();

            match parse_audio_tag(&data) {
                FlvAudioTag::SequenceHeader(config) => {
                    debug!(stream = %stream_name, codec = %config.codec_string(), "audio sequence header");
                    let init = audio_init_segment(&config);
                    stream.audio_config = Some(config);
                    stream.audio_init = Some(init);
                    maybe_write_catalog(stream, &stream_name);
                }
                FlvAudioTag::RawAac(aac_data) => {
                    let Some(ref config) = stream.audio_config else {
                        return;
                    };
                    let Some(ref init) = stream.audio_init else {
                        return;
                    };

                    stream.audio_seq += 1;
                    // AAC-LC uses 1024 samples per frame at the audio sample rate
                    let duration = 1024;
                    let base_dts = (timestamp as u64) * (config.sample_rate as u64) / 1000;

                    match stream.audio_track.append_group() {
                        Ok(mut group) => {
                            if let Err(e) = group.write_frame(init.clone()) {
                                debug!(error = ?e, "failed to write audio init segment");
                                return;
                            }
                            let seg = audio_segment(stream.audio_seq, base_dts, duration, &aac_data);
                            if let Err(e) = group.write_frame(seg) {
                                debug!(error = ?e, "failed to write audio segment");
                            }
                            let _ = group.finish();
                        }
                        Err(e) => {
                            debug!(error = ?e, "failed to append audio group");
                        }
                    }
                }
                FlvAudioTag::Unknown => {}
            }
        });

        RtmpServer::from_callbacks(config, on_video, on_audio, on_publish, on_unpublish)
    }

    /// Number of active RTMP streams being bridged.
    pub fn active_stream_count(&self) -> usize {
        self.streams.len()
    }

    /// Names of active RTMP streams (e.g. "live/mystream").
    pub fn stream_names(&self) -> Vec<String> {
        self.streams.iter().map(|e| e.key().clone()).collect()
    }
}

/// Write the catalog track once both video and audio configs are available
/// (or when the first config arrives if the other won't come).
fn maybe_write_catalog(stream: &mut ActiveStream, stream_name: &str) {
    if stream.catalog_written {
        return;
    }
    // Write catalog once we have at least one config
    if stream.video_config.is_none() && stream.audio_config.is_none() {
        return;
    }

    let catalog_json = generate_catalog(stream.video_config.as_ref(), stream.audio_config.as_ref());

    match stream.catalog_track.append_group() {
        Ok(mut group) => {
            if let Err(e) = group.write_frame(Bytes::from(catalog_json)) {
                debug!(error = ?e, "failed to write catalog");
                return;
            }
            let _ = group.finish();
            stream.catalog_written = true;
            info!(stream = %stream_name, "catalog published");
        }
        Err(e) => {
            debug!(error = ?e, "failed to append catalog group");
        }
    }
}
