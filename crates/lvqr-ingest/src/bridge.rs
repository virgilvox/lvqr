/// Bridge between RTMP ingest and MoQ Origin.
///
/// When an RTMP publisher connects, this module creates a MoQ broadcast
/// with CMAF-formatted tracks. Video and audio data from RTMP is remuxed
/// from FLV to fMP4 (CMAF) segments, compatible with MSE browser playback
/// and the MoQ ecosystem (moq-js).
use bytes::Bytes;
use dashmap::DashMap;
use lvqr_auth::{AuthContext, NoopAuthProvider, SharedAuth};
use lvqr_core::{EventBus, RelayEvent};
use lvqr_fragment::{Fragment, FragmentFlags, FragmentMeta, MoqTrackSink};
use lvqr_moq::Track;
use std::sync::Arc;
use tracing::{debug, info, warn};

use crate::observer::{SharedFragmentObserver, SharedRawSampleObserver};
use crate::remux::{
    AudioConfig, FlvAudioTag, FlvVideoTag, VideoConfig, audio_init_segment, audio_segment, extract_resolution,
    generate_catalog, parse_audio_tag, parse_video_tag, video_init_segment_with_size,
};
use crate::rtmp::{AuthCallback, MediaCallback, RtmpConfig, RtmpServer, StreamCallback};

/// State for a single active RTMP stream being bridged to MoQ.
///
/// The track writes go through [`MoqTrackSink`] so this module is a
/// `Fragment`-shaped producer: every branch below constructs a `Fragment`
/// and calls `sink.push(..)`. This is the Tier 2.1 migration of the RTMP
/// bridge to the Unified Fragment Model.
struct ActiveStream {
    _broadcast: lvqr_moq::BroadcastProducer,
    video_sink: MoqTrackSink,
    audio_sink: MoqTrackSink,
    catalog_track: lvqr_moq::TrackProducer,
    // Codec configuration (set when sequence headers arrive)
    video_config: Option<VideoConfig>,
    audio_config: Option<AudioConfig>,
    video_init: Option<Bytes>,
    audio_init: Option<Bytes>,
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
    origin: lvqr_moq::OriginProducer,
    streams: Arc<DashMap<String, ActiveStream>>,
    auth: SharedAuth,
    events: Option<EventBus>,
    observer: Option<SharedFragmentObserver>,
    raw_observer: Option<SharedRawSampleObserver>,
}

impl RtmpMoqBridge {
    pub fn new(origin: lvqr_moq::OriginProducer) -> Self {
        Self {
            origin,
            streams: Arc::new(DashMap::new()),
            auth: Arc::new(NoopAuthProvider),
            events: None,
            observer: None,
            raw_observer: None,
        }
    }

    /// Construct with a specific auth provider.
    pub fn with_auth(origin: lvqr_moq::OriginProducer, auth: SharedAuth) -> Self {
        Self {
            origin,
            streams: Arc::new(DashMap::new()),
            auth,
            events: None,
            observer: None,
            raw_observer: None,
        }
    }

    /// Replace the auth provider after construction.
    pub fn set_auth(&mut self, auth: SharedAuth) {
        self.auth = auth;
    }

    /// Attach an `EventBus` so the bridge emits `BroadcastStarted` and
    /// `BroadcastStopped` events whenever an RTMP publisher connects or
    /// disconnects. Subscribers on the bus (e.g. the recorder) can react
    /// without polling `stream_names()`.
    pub fn with_events(mut self, events: EventBus) -> Self {
        self.events = Some(events);
        self
    }

    /// Attach or replace the event bus after construction.
    pub fn set_event_bus(&mut self, events: EventBus) {
        self.events = Some(events);
    }

    /// Attach a [`crate::FragmentObserver`] so the bridge fans every
    /// emitted [`Fragment`] (and the corresponding init segment) out
    /// to a non-MoQ consumer such as the LL-HLS server in `lvqr-cli`.
    /// The observer is invoked synchronously from inside the RTMP
    /// callback path, so implementations must be cheap.
    pub fn with_observer(mut self, observer: SharedFragmentObserver) -> Self {
        self.observer = Some(observer);
        self
    }

    /// Attach or replace the [`crate::FragmentObserver`] after
    /// construction.
    pub fn set_observer(&mut self, observer: SharedFragmentObserver) {
        self.observer = Some(observer);
    }

    /// Attach a [`crate::RawSampleObserver`] so the bridge fans every
    /// per-NAL video sample and every raw AAC audio access unit out
    /// to a consumer that needs pre-mux access (notably the future
    /// WHEP RTP packetizer). Read-only tap; does not alter the
    /// fragment / MoQ / HLS paths.
    pub fn with_raw_sample_observer(mut self, observer: SharedRawSampleObserver) -> Self {
        self.raw_observer = Some(observer);
        self
    }

    /// Attach or replace the [`crate::RawSampleObserver`] after
    /// construction.
    pub fn set_raw_sample_observer(&mut self, observer: SharedRawSampleObserver) {
        self.raw_observer = Some(observer);
    }

    /// Create an RTMP server wired to this bridge.
    pub fn create_rtmp_server(&self, config: RtmpConfig) -> RtmpServer {
        let origin = self.origin.clone();
        let streams_publish = self.streams.clone();
        let streams_unpublish = self.streams.clone();
        let streams_video = self.streams.clone();
        let streams_audio = self.streams.clone();
        let events_publish = self.events.clone();
        let events_unpublish = self.events.clone();

        let observer_video = self.observer.clone();
        let observer_audio = self.observer.clone();
        let raw_observer_video = self.raw_observer.clone();
        let raw_observer_audio = self.raw_observer.clone();

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

            metrics::gauge!("lvqr_active_streams").increment(1.0);
            if let Some(bus) = &events_publish {
                bus.emit(RelayEvent::BroadcastStarted {
                    name: stream_name.clone(),
                });
            }
            // Build Fragment sinks around the freshly-created TrackProducers.
            // Init segments are not yet known (they arrive with the FLV
            // sequence headers); set_init_segment is called when they do.
            let video_sink = MoqTrackSink::new(video_track, FragmentMeta::new("avc1", 90000));
            let audio_sink = MoqTrackSink::new(audio_track, FragmentMeta::new("mp4a", 0));
            streams_publish.insert(
                stream_name,
                ActiveStream {
                    _broadcast: broadcast,
                    video_sink,
                    audio_sink,
                    catalog_track,
                    video_config: None,
                    audio_config: None,
                    video_init: None,
                    audio_init: None,
                    video_seq: 0,
                    audio_seq: 0,
                    last_video_ts: None,
                },
            );
        });

        let on_unpublish: StreamCallback = Arc::new(move |app: &str, key: &str| {
            let stream_name = format!("{app}/{key}");
            if let Some((_, mut stream)) = streams_unpublish.remove(&stream_name) {
                // Explicitly close any open video group. Dropping the sink
                // would also do this, but being explicit makes the unpublish
                // path obvious to readers.
                stream.video_sink.finish_current_group();
                metrics::gauge!("lvqr_active_streams").decrement(1.0);
                if let Some(bus) = &events_unpublish {
                    bus.emit(RelayEvent::BroadcastStopped {
                        name: stream_name.clone(),
                    });
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
                    let (width, height) = config
                        .sps_list
                        .first()
                        .and_then(|sps| extract_resolution(sps))
                        .unwrap_or((0, 0));
                    debug!(
                        stream = %stream_name,
                        codec = %config.codec_string(),
                        width, height,
                        "video sequence header"
                    );
                    let init = video_init_segment_with_size(&config, width as u16, height as u16);
                    // Hand the init segment to the sink so every new MoQ
                    // group starts with it.
                    stream.video_sink.set_init_segment(init.clone());
                    stream.video_config = Some(config);
                    stream.video_init = Some(init.clone());
                    if let Some(obs) = observer_video.as_ref() {
                        // Video init writer is hardcoded to 90 kHz
                        // (see `video_init_segment_with_size` ->
                        // `mvhd.timescale = 90000`), so the bridge
                        // reports the same value here.
                        obs.on_init(&stream_name, "0.mp4", 90_000, init);
                    }
                    maybe_write_catalog(stream, &stream_name);
                }
                FlvVideoTag::Nalu {
                    keyframe,
                    cts,
                    data: nalu_data,
                } => {
                    if stream.video_config.is_none() || stream.video_init.is_none() {
                        return; // no sequence header yet
                    };

                    metrics::counter!("lvqr_frames_published_total", "type" => "video").increment(1);
                    metrics::counter!("lvqr_bytes_ingested_total", "type" => "video").increment(nalu_data.len() as u64);

                    // Compute duration from timestamp delta (default 33ms = ~30fps)
                    let duration_ms = match stream.last_video_ts {
                        Some(prev) => timestamp.saturating_sub(prev),
                        None => 33,
                    };
                    stream.last_video_ts = Some(timestamp);
                    let duration_ticks = duration_ms * 90; // 90kHz timescale
                    let base_dts = (timestamp as u64) * 90;

                    let sample = lvqr_cmaf::RawSample {
                        track_id: 1,
                        dts: base_dts,
                        cts_offset: cts * 90, // ms to 90kHz
                        duration: duration_ticks,
                        payload: nalu_data,
                        keyframe,
                    };

                    if let Some(obs) = raw_observer_video.as_ref() {
                        // RTMP / FLV ingest is AVC-only in LVQR today;
                        // HEVC-over-RTMP (enhanced-RTMP) lives in a
                        // later session. The codec tag is therefore
                        // constant here, unlike the WHIP bridge.
                        obs.on_raw_sample(&stream_name, "0.mp4", crate::VideoCodec::H264, &sample);
                    }

                    stream.video_seq += 1;
                    let seg = lvqr_cmaf::build_moof_mdat(stream.video_seq, 1, base_dts, &[sample]);

                    // Build a Fragment and push it through the sink. On a
                    // keyframe the sink closes the previous group, opens a
                    // new one, and prepends the init segment from
                    // FragmentMeta. On a delta the sink writes into the
                    // open group.
                    let flags = if keyframe {
                        FragmentFlags::KEYFRAME
                    } else {
                        FragmentFlags::DELTA
                    };
                    let frag = Fragment::new(
                        "0.mp4",
                        stream.video_seq as u64,
                        0,
                        0,
                        base_dts,
                        base_dts.saturating_add((cts as u64) * 90),
                        duration_ticks as u64,
                        flags,
                        seg,
                    );
                    if let Err(e) = stream.video_sink.push(&frag) {
                        debug!(error = ?e, "failed to push video fragment through sink");
                    }
                    if let Some(obs) = observer_video.as_ref() {
                        obs.on_fragment(&stream_name, "0.mp4", &frag);
                    }
                }
                FlvVideoTag::EndOfSequence => {
                    stream.video_sink.finish_current_group();
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
                    // Capture the sample rate before moving `config`
                    // into `stream.audio_config`; the observer needs
                    // it as the track timescale so the LL-HLS bridge
                    // can build a `CmafPolicy` that matches the real
                    // audio sample rate (44.1 kHz vs 48 kHz vs other).
                    let audio_timescale = config.sample_rate;
                    let init = audio_init_segment(&config);
                    stream.audio_sink.set_init_segment(init.clone());
                    stream.audio_config = Some(config);
                    stream.audio_init = Some(init.clone());
                    if let Some(obs) = observer_audio.as_ref() {
                        obs.on_init(&stream_name, "1.mp4", audio_timescale, init);
                    }
                    maybe_write_catalog(stream, &stream_name);
                }
                FlvAudioTag::RawAac(aac_data) => {
                    let Some(ref config) = stream.audio_config else {
                        return;
                    };
                    if stream.audio_init.is_none() {
                        return;
                    }

                    metrics::counter!("lvqr_frames_published_total", "type" => "audio").increment(1);
                    metrics::counter!("lvqr_bytes_ingested_total", "type" => "audio").increment(aac_data.len() as u64);

                    stream.audio_seq += 1;
                    // AAC-LC uses 1024 samples per frame at the audio sample rate
                    let duration: u32 = 1024;
                    let base_dts = (timestamp as u64) * (config.sample_rate as u64) / 1000;

                    if let Some(obs) = raw_observer_audio.as_ref() {
                        let sample = lvqr_cmaf::RawSample {
                            track_id: 2,
                            dts: base_dts,
                            cts_offset: 0,
                            duration,
                            payload: aac_data.clone(),
                            keyframe: true,
                        };
                        // Audio sample: codec tag is defaulted.
                        // Consumers must not branch on it; a later
                        // Opus sibling track will introduce a
                        // dedicated audio-codec type.
                        obs.on_raw_sample(&stream_name, "1.mp4", crate::VideoCodec::default(), &sample);
                    }

                    let seg = audio_segment(stream.audio_seq, base_dts, duration, &aac_data);

                    // Every audio fragment opens its own MoQ group (audio
                    // frames are independently decodable in AAC-LC), so we
                    // tag every one as a keyframe for the sink's purposes.
                    // The sink will close the previous group and open a new
                    // one on each push.
                    let frag = Fragment::new(
                        "1.mp4",
                        stream.audio_seq as u64,
                        0,
                        0,
                        base_dts,
                        base_dts,
                        duration as u64,
                        FragmentFlags::KEYFRAME,
                        seg,
                    );
                    if let Err(e) = stream.audio_sink.push(&frag) {
                        debug!(error = ?e, "failed to push audio fragment through sink");
                    }
                    if let Some(obs) = observer_audio.as_ref() {
                        obs.on_fragment(&stream_name, "1.mp4", &frag);
                    }
                    // Close the group immediately so every audio frame is
                    // its own group on the wire. Subsequent pushes will
                    // open fresh groups.
                    stream.audio_sink.finish_current_group();
                }
                FlvAudioTag::Unknown => {}
            }
        });

        let mut server = RtmpServer::from_callbacks(config, on_video, on_audio, on_publish, on_unpublish);

        // Wire the bridge's auth provider into RTMP publish validation.
        let auth = self.auth.clone();
        let validate: AuthCallback = Arc::new(move |app: &str, key: &str| {
            auth.check(&AuthContext::Publish {
                app: app.to_string(),
                key: key.to_string(),
            })
            .is_allow()
        });
        server.set_validate_publish(validate);
        server
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

/// Write the catalog track whenever codec configuration changes.
///
/// Each call creates a new MoQ group, so late-joining subscribers always
/// get the latest catalog. This handles the case where audio config arrives
/// after video config -- the catalog is rewritten with both tracks.
fn maybe_write_catalog(stream: &mut ActiveStream, stream_name: &str) {
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
            info!(
                stream = %stream_name,
                has_video = stream.video_config.is_some(),
                has_audio = stream.audio_config.is_some(),
                "catalog published"
            );
        }
        Err(e) => {
            debug!(error = ?e, "failed to append catalog group");
        }
    }
}
