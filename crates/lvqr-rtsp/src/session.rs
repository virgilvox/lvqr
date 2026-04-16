//! RTSP session state machine.
//!
//! Each RTSP session progresses through a defined set of states.
//! Playback sessions (server -> client):
//!     Init -> Described -> Ready -> Playing -> Teardown
//! Ingest sessions (client -> server via ANNOUNCE/RECORD):
//!     Init -> Announced -> Ready -> Recording -> Teardown

use std::collections::HashMap;
use std::fmt;

/// Unique identifier for an RTSP session.
pub type SessionId = String;

/// Generate a random session ID (hex string).
pub fn generate_session_id() -> SessionId {
    use std::time::{SystemTime, UNIX_EPOCH};
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("{ts:016X}")
}

/// State of an RTSP session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionState {
    Init,
    Ready,
    Playing,
    Recording,
}

impl fmt::Display for SessionState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Init => f.write_str("Init"),
            Self::Ready => f.write_str("Ready"),
            Self::Playing => f.write_str("Playing"),
            Self::Recording => f.write_str("Recording"),
        }
    }
}

/// Direction of the media flow for this session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionMode {
    /// Server sends RTP to client (DESCRIBE/SETUP/PLAY).
    Playback,
    /// Client sends RTP to server (ANNOUNCE/SETUP/RECORD).
    Ingest,
}

/// Per-track transport configuration, established during SETUP.
#[derive(Debug, Clone)]
pub struct TrackTransport {
    pub control_path: String,
    pub interleaved: Option<(u8, u8)>,
}

/// SDP media description for one track.
#[derive(Debug, Clone)]
pub struct TrackInfo {
    pub media_type: MediaType,
    pub codec: TrackCodec,
    pub clock_rate: u32,
    pub control: String,
    pub fmtp: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaType {
    Video,
    Audio,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrackCodec {
    H264,
    H265,
    Aac,
    Opus,
    Unknown,
}

/// Per-session state.
#[derive(Debug)]
pub struct Session {
    pub id: SessionId,
    pub state: SessionState,
    pub mode: SessionMode,
    pub broadcast: String,
    pub tracks: Vec<TrackInfo>,
    pub transports: HashMap<String, TrackTransport>,
}

impl Session {
    pub fn new(id: SessionId, mode: SessionMode, broadcast: String) -> Self {
        Self {
            id,
            state: SessionState::Init,
            mode,
            broadcast,
            tracks: Vec::new(),
            transports: HashMap::new(),
        }
    }

    pub fn setup_track(&mut self, control: &str, interleaved: Option<(u8, u8)>) {
        self.transports.insert(
            control.to_string(),
            TrackTransport {
                control_path: control.to_string(),
                interleaved,
            },
        );
        self.state = SessionState::Ready;
    }

    pub fn play(&mut self) -> Result<(), StateError> {
        if self.state != SessionState::Ready {
            return Err(StateError::InvalidTransition(self.state, "PLAY"));
        }
        if self.mode != SessionMode::Playback {
            return Err(StateError::WrongMode(self.mode, "PLAY"));
        }
        self.state = SessionState::Playing;
        Ok(())
    }

    pub fn record(&mut self) -> Result<(), StateError> {
        if self.state != SessionState::Ready {
            return Err(StateError::InvalidTransition(self.state, "RECORD"));
        }
        if self.mode != SessionMode::Ingest {
            return Err(StateError::WrongMode(self.mode, "RECORD"));
        }
        self.state = SessionState::Recording;
        Ok(())
    }
}

#[derive(Debug)]
pub enum StateError {
    InvalidTransition(SessionState, &'static str),
    WrongMode(SessionMode, &'static str),
}

impl fmt::Display for StateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidTransition(state, method) => {
                write!(f, "cannot {method} in state {state}")
            }
            Self::WrongMode(mode, method) => {
                write!(f, "cannot {method} in {mode:?} mode")
            }
        }
    }
}

impl std::error::Error for StateError {}

/// Minimal SDP parser for RTSP DESCRIBE responses and ANNOUNCE bodies.
/// Extracts media-level attributes needed for track setup.
pub fn parse_sdp_tracks(sdp: &str) -> Vec<TrackInfo> {
    let mut tracks = Vec::new();
    let mut current: Option<TrackInfo> = None;

    for line in sdp.lines() {
        let line = line.trim_end_matches('\r');
        if let Some(rest) = line.strip_prefix("m=") {
            if let Some(track) = current.take() {
                tracks.push(track);
            }
            let parts: Vec<&str> = rest.split_whitespace().collect();
            if parts.len() >= 4 {
                let media_type = match parts[0] {
                    "video" => MediaType::Video,
                    "audio" => MediaType::Audio,
                    _ => continue,
                };
                let clock_rate = parts.get(3).and_then(|p| {
                    // payload type -> look for rtpmap later
                    p.parse::<u32>().ok()
                });
                current = Some(TrackInfo {
                    media_type,
                    codec: TrackCodec::Unknown,
                    clock_rate: clock_rate.unwrap_or(90_000),
                    control: String::new(),
                    fmtp: None,
                });
            }
        } else if let Some(rest) = line.strip_prefix("a=") {
            if let Some(ref mut track) = current {
                if let Some(val) = rest.strip_prefix("control:") {
                    track.control = val.trim().to_string();
                } else if let Some(val) = rest.strip_prefix("rtpmap:") {
                    // "96 H264/90000"
                    let payload = val.trim();
                    let codec_part = payload.split_whitespace().nth(1).unwrap_or("");
                    let (codec_name, rate_str) = codec_part.split_once('/').unwrap_or((codec_part, ""));
                    track.codec = match codec_name.to_ascii_uppercase().as_str() {
                        "H264" => TrackCodec::H264,
                        "H265" | "HEVC" => TrackCodec::H265,
                        "MPEG4-GENERIC" => TrackCodec::Aac,
                        "OPUS" => TrackCodec::Opus,
                        _ => TrackCodec::Unknown,
                    };
                    if let Ok(rate) = rate_str.split('/').next().unwrap_or("").parse::<u32>() {
                        track.clock_rate = rate;
                    }
                } else if let Some(val) = rest.strip_prefix("fmtp:") {
                    track.fmtp = Some(val.trim().to_string());
                }
            }
        }
    }
    if let Some(track) = current {
        tracks.push(track);
    }
    tracks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_playback_lifecycle() {
        let mut session = Session::new("test1".into(), SessionMode::Playback, "live/cam1".into());
        assert_eq!(session.state, SessionState::Init);

        session.setup_track("track1", Some((0, 1)));
        assert_eq!(session.state, SessionState::Ready);
        assert!(session.transports.contains_key("track1"));

        session.play().unwrap();
        assert_eq!(session.state, SessionState::Playing);
    }

    #[test]
    fn session_ingest_lifecycle() {
        let mut session = Session::new("test2".into(), SessionMode::Ingest, "publish/cam1".into());
        session.setup_track("track1", Some((0, 1)));
        session.record().unwrap();
        assert_eq!(session.state, SessionState::Recording);
    }

    #[test]
    fn play_requires_ready_state() {
        let mut session = Session::new("test3".into(), SessionMode::Playback, "live/test".into());
        assert!(session.play().is_err());
    }

    #[test]
    fn record_rejects_playback_mode() {
        let mut session = Session::new("test4".into(), SessionMode::Playback, "live/test".into());
        session.setup_track("track1", None);
        assert!(session.record().is_err());
    }

    #[test]
    fn parse_sdp_h264_audio() {
        let sdp = "\
v=0\r\n\
o=- 0 0 IN IP4 0.0.0.0\r\n\
s=Stream\r\n\
t=0 0\r\n\
m=video 0 RTP/AVP 96\r\n\
a=rtpmap:96 H264/90000\r\n\
a=fmtp:96 packetization-mode=1;profile-level-id=640028\r\n\
a=control:track1\r\n\
m=audio 0 RTP/AVP 97\r\n\
a=rtpmap:97 MPEG4-GENERIC/44100/2\r\n\
a=fmtp:97 streamtype=5;profile-level-id=1;mode=AAC-hbr\r\n\
a=control:track2\r\n";

        let tracks = parse_sdp_tracks(sdp);
        assert_eq!(tracks.len(), 2);

        assert_eq!(tracks[0].media_type, MediaType::Video);
        assert_eq!(tracks[0].codec, TrackCodec::H264);
        assert_eq!(tracks[0].clock_rate, 90000);
        assert_eq!(tracks[0].control, "track1");
        assert!(tracks[0].fmtp.as_ref().unwrap().contains("packetization-mode=1"));

        assert_eq!(tracks[1].media_type, MediaType::Audio);
        assert_eq!(tracks[1].codec, TrackCodec::Aac);
        assert_eq!(tracks[1].clock_rate, 44100);
        assert_eq!(tracks[1].control, "track2");
    }

    #[test]
    fn parse_sdp_hevc() {
        let sdp = "\
v=0\r\n\
o=- 0 0 IN IP4 0.0.0.0\r\n\
s=Test\r\n\
m=video 0 RTP/AVP 96\r\n\
a=rtpmap:96 H265/90000\r\n\
a=control:video\r\n";

        let tracks = parse_sdp_tracks(sdp);
        assert_eq!(tracks.len(), 1);
        assert_eq!(tracks[0].codec, TrackCodec::H265);
    }

    #[test]
    fn generate_session_id_is_nonempty() {
        let id = generate_session_id();
        assert!(!id.is_empty());
        assert!(id.len() >= 16);
    }
}
