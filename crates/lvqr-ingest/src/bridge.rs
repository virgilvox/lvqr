/// Bridge between RTMP ingest and MoQ Origin.
///
/// When an RTMP publisher connects, this module creates a MoQ broadcast
/// and tracks on the relay's shared OriginProducer. Video and audio data
/// from RTMP is written as MoQ groups and frames.
use bytes::Bytes;
use dashmap::DashMap;
use moq_lite::Track;
use std::sync::Arc;
use tracing::{debug, info, warn};

use crate::rtmp::{MediaCallback, RtmpConfig, RtmpServer, StreamCallback};

/// State for a single active RTMP stream being bridged to MoQ.
struct ActiveStream {
    /// The MoQ broadcast producer. Must be kept alive for subscribers to see data.
    _broadcast: moq_lite::BroadcastProducer,
    /// Video track producer. Writes video groups/frames.
    video_track: moq_lite::TrackProducer,
    /// Audio track producer. Writes audio groups/frames.
    audio_track: moq_lite::TrackProducer,
    /// Current video group writer (if a group is open).
    video_group: Option<moq_lite::GroupProducer>,
}

/// Bridges RTMP ingest to a MoQ OriginProducer.
///
/// Creates MoQ broadcasts for each RTMP stream and writes video/audio
/// data as MoQ groups and frames.
pub struct RtmpMoqBridge {
    origin: moq_lite::OriginProducer,
    /// Active streams keyed by "app/streamkey".
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

            let video_track = match broadcast.create_track(Track::new("video")) {
                Ok(t) => t,
                Err(e) => {
                    warn!(stream = %stream_name, error = ?e, "failed to create video track");
                    return;
                }
            };

            let audio_track = match broadcast.create_track(Track::new("audio")) {
                Ok(t) => t,
                Err(e) => {
                    warn!(stream = %stream_name, error = ?e, "failed to create audio track");
                    return;
                }
            };

            streams_publish.insert(
                stream_name,
                ActiveStream {
                    _broadcast: broadcast,
                    video_track,
                    audio_track,
                    video_group: None,
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

        let on_video: MediaCallback = Arc::new(move |app: &str, key: &str, data: Bytes, _timestamp: u32| {
            let stream_name = format!("{app}/{key}");
            let Some(mut entry) = streams_video.get_mut(&stream_name) else {
                return;
            };
            let stream = entry.value_mut();
            let is_keyframe = crate::rtmp::is_keyframe(&data);

            if is_keyframe {
                // Finish the previous group if open
                if let Some(mut group) = stream.video_group.take() {
                    let _ = group.finish();
                }
                // Start a new group for this keyframe
                match stream.video_track.append_group() {
                    Ok(mut group) => {
                        if let Err(e) = group.write_frame(data) {
                            debug!(error = ?e, "failed to write keyframe");
                            return;
                        }
                        stream.video_group = Some(group);
                    }
                    Err(e) => {
                        debug!(error = ?e, "failed to append video group");
                    }
                }
            } else if let Some(ref mut group) = stream.video_group {
                if let Err(e) = group.write_frame(data) {
                    debug!(error = ?e, "failed to write delta frame");
                }
            }
        });

        let on_audio: MediaCallback = Arc::new(move |app: &str, key: &str, data: Bytes, _timestamp: u32| {
            let stream_name = format!("{app}/{key}");
            let Some(mut entry) = streams_audio.get_mut(&stream_name) else {
                return;
            };
            let stream = entry.value_mut();
            match stream.audio_track.append_group() {
                Ok(mut group) => {
                    if let Err(e) = group.write_frame(data) {
                        debug!(error = ?e, "failed to write audio frame");
                    }
                    let _ = group.finish();
                }
                Err(e) => {
                    debug!(error = ?e, "failed to append audio group");
                }
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
