//! SDP builder for the RTSP DESCRIBE response on the PLAY path.
//!
//! Pure rendering: given decoded parameter sets (SPS/PPS for H.264,
//! VPS/SPS/PPS for HEVC) and the track timing carried on the
//! broadcaster meta, produce a well-formed SDP body that every
//! mainstream RTSP client (VLC, ffplay, gstreamer's rtspsrc, Exoplayer)
//! will accept as a PLAY description.
//!
//! The builder is intentionally small: it renders one session-level
//! block plus one media block per populated track. H.264 follows RFC
//! 6184 (`profile-level-id`, `packetization-mode`,
//! `sprop-parameter-sets`); HEVC follows RFC 7798 (`profile-space`,
//! `profile-id`, `tier-flag`, `level-id`, separate `sprop-vps` /
//! `sprop-sps` / `sprop-pps`). Audio support lands in a follow-up.
//!
//! Session-level and media-level control URIs are relative and
//! resolved against the `Content-Base` header the DESCRIBE response
//! already carries, which the client composes with SETUP requests.
//! A=control:* at session level is idiomatic and keeps VLC happy.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use lvqr_cmaf::{AacConfig, AvcParameterSets, HevcParameterSets};
use std::fmt::Write as _;
use std::net::IpAddr;

/// SDP for a H.264 video track on the PLAY response.
#[derive(Debug, Clone)]
pub struct H264TrackDescription {
    /// Dynamic RTP payload type (96-127). LVQR uses 96 by convention.
    pub payload_type: u8,
    /// RTP timestamp clock rate. 90000 Hz for H.264.
    pub clock_rate: u32,
    /// Relative control URI the client uses on SETUP, e.g. `"track1"`.
    pub control: String,
    /// Parameter sets extracted from the fMP4 init segment. The
    /// builder encodes them into the `sprop-parameter-sets` fmtp
    /// attribute in base64.
    pub params: AvcParameterSets,
}

/// SDP for an HEVC video track on the PLAY response. Mirrors
/// [`H264TrackDescription`] but follows RFC 7798 for the fmtp line:
/// profile-space / profile-id / tier-flag / level-id instead of the
/// H.264 `profile-level-id`, and three separate `sprop-vps` /
/// `sprop-sps` / `sprop-pps` attributes instead of the joined
/// `sprop-parameter-sets`.
#[derive(Debug, Clone)]
pub struct HevcTrackDescription {
    pub payload_type: u8,
    pub clock_rate: u32,
    pub control: String,
    pub params: HevcParameterSets,
}

/// Either an H.264 or an HEVC video track for the PLAY SDP. A single
/// broadcast is one codec at a time; the enum enforces that only one
/// variant is rendered.
#[derive(Debug, Clone)]
pub enum VideoTrackDescription {
    H264(H264TrackDescription),
    Hevc(HevcTrackDescription),
}

/// SDP for an AAC audio track on the PLAY response. Rendered as the
/// AAC-hbr mode from RFC 3640 -- the `a=fmtp` line carries the
/// `sizelength=13;indexlength=3;indexdeltalength=3;config=<hex>`
/// descriptor the [`crate::rtp::AacPacketizer`] output is designed
/// to match.
#[derive(Debug, Clone)]
pub struct AacTrackDescription {
    pub payload_type: u8,
    /// Relative control URI the client uses on SETUP, e.g. `"track2"`.
    pub control: String,
    /// AAC decoder configuration extracted from the init segment.
    pub config: AacConfig,
}

/// Top-level SDP description rendered in response to DESCRIBE.
#[derive(Debug, Clone)]
pub struct PlaySdp {
    /// Stream name for the SDP `s=` line. Typically the broadcast id.
    pub session_name: String,
    /// Host address for the SDP `o=` line. Usually the server's
    /// bound address.
    pub host_ip: IpAddr,
    /// Video description, present when a video broadcaster exists
    /// for the requested broadcast.
    pub video: Option<VideoTrackDescription>,
    /// Audio description, present when a `1.mp4` broadcaster
    /// carries an AAC init segment.
    pub audio: Option<AacTrackDescription>,
}

impl PlaySdp {
    /// Render the SDP body as a `\r\n`-terminated string. The output
    /// is always valid UTF-8 since every byte is ASCII.
    pub fn render(&self) -> String {
        let mut out = String::with_capacity(512);

        let addr_family = match self.host_ip {
            IpAddr::V4(_) => "IP4",
            IpAddr::V6(_) => "IP6",
        };

        // Session-level block. The `o=` line carries an opaque
        // session identifier; constant zero is fine for the live
        // playback case (we do not rebase across reconnects).
        let _ = writeln!(out, "v=0\r");
        let _ = writeln!(out, "o=- 0 0 IN {addr_family} {}\r", self.host_ip);
        let _ = writeln!(out, "s={}\r", self.session_name);
        let _ = writeln!(out, "t=0 0\r");
        let _ = writeln!(out, "a=control:*\r");

        match self.video.as_ref() {
            Some(VideoTrackDescription::H264(v)) => render_h264_block(&mut out, addr_family, v),
            Some(VideoTrackDescription::Hevc(v)) => render_hevc_block(&mut out, addr_family, v),
            None => {}
        }
        if let Some(ref a) = self.audio {
            render_aac_block(&mut out, addr_family, a);
        }
        out
    }
}

fn render_h264_block(out: &mut String, addr_family: &str, v: &H264TrackDescription) {
    let _ = writeln!(out, "m=video 0 RTP/AVP {}\r", v.payload_type);
    let _ = writeln!(out, "c=IN {addr_family} 0.0.0.0\r");
    let _ = writeln!(out, "a=rtpmap:{} H264/{}\r", v.payload_type, v.clock_rate);

    let profile_level_id = profile_level_id_from_avc(&v.params);
    let sprop = sprop_parameter_sets(&v.params);
    let fmtp_parts = [
        format!("profile-level-id={profile_level_id}"),
        // packetization-mode=1 is the normative LL-HLS / live
        // default: allows single-NAL, STAP-A, and FU-A packets.
        // The LVQR packetizer emits single-NAL + FU-A; FU-B and
        // MTAP are not used.
        "packetization-mode=1".to_string(),
        format!("sprop-parameter-sets={sprop}"),
    ];
    let _ = writeln!(out, "a=fmtp:{} {}\r", v.payload_type, fmtp_parts.join(";"));
    let _ = writeln!(out, "a=control:{}\r", v.control);
}

fn render_hevc_block(out: &mut String, addr_family: &str, v: &HevcTrackDescription) {
    let _ = writeln!(out, "m=video 0 RTP/AVP {}\r", v.payload_type);
    let _ = writeln!(out, "c=IN {addr_family} 0.0.0.0\r");
    let _ = writeln!(out, "a=rtpmap:{} H265/{}\r", v.payload_type, v.clock_rate);

    let tier_flag: u8 = if v.params.general_tier_flag { 1 } else { 0 };
    // Per RFC 7798 section 7.1 the 4-byte profile_compatibility_flags
    // + 6-byte general_constraint_indicator_flags arrays are rendered
    // as unsigned hex integers in the fmtp line. Leading zeros are
    // allowed to be trimmed; keep them to match typical ffmpeg
    // output for easier interoperability comparisons.
    let profile_compat = u32::from_be_bytes(v.params.general_profile_compatibility_flags);
    let interop_constraints = {
        let b = v.params.general_constraint_indicator_flags;
        format!(
            "{:02X}{:02X}{:02X}{:02X}{:02X}{:02X}",
            b[0], b[1], b[2], b[3], b[4], b[5]
        )
    };
    let fmtp_parts = [
        format!("profile-space={}", v.params.general_profile_space),
        format!("profile-id={}", v.params.general_profile_idc),
        format!("tier-flag={tier_flag}"),
        format!("level-id={}", v.params.general_level_idc),
        format!("profile-compatibility-indicator={profile_compat:08X}"),
        format!("interop-constraints={interop_constraints}"),
        format!("sprop-vps={}", join_base64(&v.params.vps_list)),
        format!("sprop-sps={}", join_base64(&v.params.sps_list)),
        format!("sprop-pps={}", join_base64(&v.params.pps_list)),
    ];
    let _ = writeln!(out, "a=fmtp:{} {}\r", v.payload_type, fmtp_parts.join(";"));
    let _ = writeln!(out, "a=control:{}\r", v.control);
}

/// Encode a list of NAL units into the comma-separated base64 form
/// RFC 7798 requires for `sprop-vps` / `sprop-sps` / `sprop-pps`.
/// Empty list renders as the empty string, which is legal.
fn join_base64(nalus: &[Vec<u8>]) -> String {
    nalus.iter().map(|n| BASE64.encode(n)).collect::<Vec<_>>().join(",")
}

fn render_aac_block(out: &mut String, addr_family: &str, a: &AacTrackDescription) {
    let _ = writeln!(out, "m=audio 0 RTP/AVP {}\r", a.payload_type);
    let _ = writeln!(out, "c=IN {addr_family} 0.0.0.0\r");
    // RFC 3640 rtpmap: `mpeg4-generic/<sample_rate>[/<channels>]`.
    // LVQR always emits the optional channel count so clients that
    // rely on it for downmix decisions never see an empty field.
    let _ = writeln!(
        out,
        "a=rtpmap:{} mpeg4-generic/{}/{}\r",
        a.payload_type, a.config.sample_rate, a.config.channels
    );
    // Hex-encode the ASC bytes uppercase; RFC 3640 examples use
    // uppercase and ffplay / vlc accept either.
    let config_hex: String = a.config.asc.iter().map(|b| format!("{b:02X}")).collect();
    let fmtp_parts = [
        "streamtype=5".to_string(),
        format!("profile-level-id={}", aac_profile_level_id(&a.config)),
        "mode=AAC-hbr".to_string(),
        "sizelength=13".to_string(),
        "indexlength=3".to_string(),
        "indexdeltalength=3".to_string(),
        format!("config={config_hex}"),
    ];
    let _ = writeln!(out, "a=fmtp:{} {}\r", a.payload_type, fmtp_parts.join(";"));
    let _ = writeln!(out, "a=control:{}\r", a.control);
}

/// Conservative `profile-level-id` for the AAC fmtp line. The full
/// MPEG-4 Audio profile/level table is long; every LVQR publisher
/// today ships AAC-LC which maps to profile-level-id=1 ("Main
/// audio profile L1") under most client tolerance matrices. A
/// follow-up can pick a finer value per object type + sample rate.
fn aac_profile_level_id(_config: &AacConfig) -> u8 {
    1
}

/// Hex-encode the 3-byte profile_level_id (AVCProfileIndication,
/// profile_compatibility, AVCLevelIndication) the DESCRIBE fmtp line
/// needs. Pulls the bytes off the first SPS NAL unit in the
/// parameter-set list, matching what an `avcC` would carry.
///
/// Returns `"640028"` (High profile 4.0) as a safe default when the
/// SPS list is empty or the first SPS is too short; no real stream
/// produces either case but a panicking builder would be worse.
fn profile_level_id_from_avc(params: &AvcParameterSets) -> String {
    let sps = params.sps_list.first();
    match sps {
        Some(nal) if nal.len() >= 4 => format!("{:02X}{:02X}{:02X}", nal[1], nal[2], nal[3]),
        _ => "640028".to_string(),
    }
}

/// Build the `sprop-parameter-sets=<base64(sps)>,<base64(pps)>...`
/// value from every parameter set in the extracted record. The NAL
/// bytes go in verbatim -- no Annex B start code and no AVCC length
/// prefix, which matches RFC 6184 section 8.2.
fn sprop_parameter_sets(params: &AvcParameterSets) -> String {
    let mut items = Vec::with_capacity(params.sps_list.len() + params.pps_list.len());
    for sps in &params.sps_list {
        items.push(BASE64.encode(sps));
    }
    for pps in &params.pps_list {
        items.push(BASE64.encode(pps));
    }
    items.join(",")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_avc_params() -> AvcParameterSets {
        AvcParameterSets {
            // Real SPS captured from lvqr-cmaf's x264 corpus: Baseline
            // 3.1 at 1280x720. First 4 bytes are nal_header, profile,
            // compat, level.
            sps_list: vec![vec![
                0x67, 0x42, 0x00, 0x1F, 0xD9, 0x40, 0x50, 0x04, 0xFB, 0x01, 0x10, 0x00,
            ]],
            pps_list: vec![vec![0x68, 0xEB, 0xE3, 0xCB, 0x22, 0xC0]],
        }
    }

    #[test]
    fn render_h264_video_only_sdp() {
        let sdp = PlaySdp {
            session_name: "live/cam1".into(),
            host_ip: "127.0.0.1".parse().unwrap(),
            video: Some(VideoTrackDescription::H264(H264TrackDescription {
                payload_type: 96,
                clock_rate: 90_000,
                control: "track1".into(),
                params: sample_avc_params(),
            })),
            audio: None,
        };
        let rendered = sdp.render();
        eprintln!("--- SDP ---\n{rendered}--- end ---");

        assert!(rendered.contains("v=0\r\n"));
        assert!(rendered.contains("o=- 0 0 IN IP4 127.0.0.1\r\n"));
        assert!(rendered.contains("s=live/cam1\r\n"));
        assert!(rendered.contains("t=0 0\r\n"));
        assert!(rendered.contains("a=control:*\r\n"));
        assert!(rendered.contains("m=video 0 RTP/AVP 96\r\n"));
        assert!(rendered.contains("c=IN IP4 0.0.0.0\r\n"));
        assert!(rendered.contains("a=rtpmap:96 H264/90000\r\n"));
        assert!(
            rendered.contains("profile-level-id=42001F"),
            "profile bytes from SPS[1..4]"
        );
        assert!(rendered.contains("packetization-mode=1"));
        assert!(rendered.contains("sprop-parameter-sets=Z0IAH9lAUAT7ARAA,aOvjyyLA"));
        assert!(rendered.contains("a=control:track1\r\n"));
    }

    #[test]
    fn render_video_only_produces_no_audio_m_line() {
        let sdp = PlaySdp {
            session_name: "live/cam1".into(),
            host_ip: "127.0.0.1".parse().unwrap(),
            video: None,
            audio: None,
        };
        let rendered = sdp.render();
        assert!(!rendered.contains("m=video"));
        assert!(!rendered.contains("m=audio"));
        assert!(rendered.contains("v=0"));
        assert!(rendered.contains("s=live/cam1"));
    }

    #[test]
    fn render_ipv6_host() {
        let sdp = PlaySdp {
            session_name: "live/v6".into(),
            host_ip: "::1".parse().unwrap(),
            video: None,
            audio: None,
        };
        let rendered = sdp.render();
        assert!(rendered.contains("o=- 0 0 IN IP6 ::1\r\n"));
    }

    #[test]
    fn profile_level_id_empty_sps_defaults_to_high_40() {
        let params = AvcParameterSets::default();
        assert_eq!(profile_level_id_from_avc(&params), "640028");
    }

    #[test]
    fn profile_level_id_short_sps_defaults_to_high_40() {
        let params = AvcParameterSets {
            sps_list: vec![vec![0x67, 0x42]], // < 4 bytes
            pps_list: vec![],
        };
        assert_eq!(profile_level_id_from_avc(&params), "640028");
    }

    #[test]
    fn sprop_handles_multiple_sps_and_pps() {
        let params = AvcParameterSets {
            sps_list: vec![vec![1, 2, 3], vec![4, 5, 6]],
            pps_list: vec![vec![7, 8]],
        };
        let sprop = sprop_parameter_sets(&params);
        // SPS first, PPS second; both base64 without padding stripping.
        assert_eq!(sprop, "AQID,BAUG,Bwg=");
    }

    #[test]
    fn render_round_trips_through_parse_sdp_tracks() {
        let sdp = PlaySdp {
            session_name: "live/cam1".into(),
            host_ip: "10.0.0.5".parse().unwrap(),
            video: Some(VideoTrackDescription::H264(H264TrackDescription {
                payload_type: 96,
                clock_rate: 90_000,
                control: "track1".into(),
                params: sample_avc_params(),
            })),
            audio: None,
        };
        let rendered = sdp.render();
        let tracks = crate::session::parse_sdp_tracks(&rendered);
        assert_eq!(tracks.len(), 1);
        assert_eq!(tracks[0].codec, crate::session::TrackCodec::H264);
        assert_eq!(tracks[0].clock_rate, 90_000);
        assert_eq!(tracks[0].control, "track1");
        let fmtp = tracks[0].fmtp.as_deref().expect("fmtp captured");
        assert!(fmtp.contains("packetization-mode=1"));
        assert!(fmtp.contains("sprop-parameter-sets="));
    }

    fn sample_hevc_params() -> HevcParameterSets {
        HevcParameterSets {
            vps_list: vec![vec![0x40, 0x01, 0x0C, 0x01]],
            sps_list: vec![vec![0x42, 0x01, 0x01, 0x01]],
            pps_list: vec![vec![0x44, 0x01, 0xC0]],
            general_profile_space: 0,
            general_tier_flag: false,
            general_profile_idc: 1,
            general_profile_compatibility_flags: 0x60000000u32.to_be_bytes(),
            general_constraint_indicator_flags: [0, 0, 0, 0, 0, 0],
            general_level_idc: 60,
        }
    }

    #[test]
    fn render_hevc_video_sdp_per_rfc_7798() {
        let sdp = PlaySdp {
            session_name: "live/hevc".into(),
            host_ip: "127.0.0.1".parse().unwrap(),
            video: Some(VideoTrackDescription::Hevc(HevcTrackDescription {
                payload_type: 96,
                clock_rate: 90_000,
                control: "track1".into(),
                params: sample_hevc_params(),
            })),
            audio: None,
        };
        let rendered = sdp.render();

        assert!(rendered.contains("m=video 0 RTP/AVP 96\r\n"));
        assert!(rendered.contains("a=rtpmap:96 H265/90000\r\n"));
        assert!(rendered.contains("profile-space=0"));
        assert!(rendered.contains("profile-id=1"));
        assert!(rendered.contains("tier-flag=0"));
        assert!(rendered.contains("level-id=60"));
        assert!(rendered.contains("profile-compatibility-indicator=60000000"));
        assert!(rendered.contains("interop-constraints=000000000000"));
        // Base64 of the sample NAL bytes above.
        assert!(rendered.contains("sprop-vps=QAEMAQ=="));
        assert!(rendered.contains("sprop-sps=QgEBAQ=="));
        assert!(rendered.contains("sprop-pps=RAHA"));
        assert!(rendered.contains("a=control:track1"));
        // Must NOT emit H264-only attributes.
        assert!(!rendered.contains("profile-level-id="));
        assert!(!rendered.contains("sprop-parameter-sets="));
    }

    #[test]
    fn hevc_round_trips_through_parse_sdp_tracks() {
        let sdp = PlaySdp {
            session_name: "live/hevc".into(),
            host_ip: "10.0.0.5".parse().unwrap(),
            video: Some(VideoTrackDescription::Hevc(HevcTrackDescription {
                payload_type: 96,
                clock_rate: 90_000,
                control: "track1".into(),
                params: sample_hevc_params(),
            })),
            audio: None,
        };
        let rendered = sdp.render();
        let tracks = crate::session::parse_sdp_tracks(&rendered);
        assert_eq!(tracks.len(), 1);
        assert_eq!(tracks[0].codec, crate::session::TrackCodec::H265);
        assert_eq!(tracks[0].clock_rate, 90_000);
        assert_eq!(tracks[0].control, "track1");
        let fmtp = tracks[0].fmtp.as_deref().expect("fmtp captured");
        assert!(fmtp.contains("sprop-vps="));
        assert!(fmtp.contains("sprop-sps="));
        assert!(fmtp.contains("sprop-pps="));
    }

    // --- AAC audio tests ---

    fn sample_aac_config() -> AacConfig {
        AacConfig {
            asc: vec![0x12, 0x10],
            object_type: 2,
            sample_rate: 44_100,
            channels: 2,
        }
    }

    #[test]
    fn render_aac_audio_block_per_rfc_3640() {
        let sdp = PlaySdp {
            session_name: "live/av".into(),
            host_ip: "127.0.0.1".parse().unwrap(),
            video: None,
            audio: Some(AacTrackDescription {
                payload_type: 97,
                control: "track2".into(),
                config: sample_aac_config(),
            }),
        };
        let rendered = sdp.render();

        assert!(rendered.contains("m=audio 0 RTP/AVP 97\r\n"));
        assert!(rendered.contains("a=rtpmap:97 mpeg4-generic/44100/2\r\n"));
        assert!(rendered.contains("streamtype=5"));
        assert!(rendered.contains("mode=AAC-hbr"));
        assert!(rendered.contains("sizelength=13"));
        assert!(rendered.contains("indexlength=3"));
        assert!(rendered.contains("indexdeltalength=3"));
        assert!(rendered.contains("config=1210"));
        assert!(rendered.contains("a=control:track2"));
    }

    #[test]
    fn render_video_and_audio_together() {
        let sdp = PlaySdp {
            session_name: "live/av".into(),
            host_ip: "127.0.0.1".parse().unwrap(),
            video: Some(VideoTrackDescription::H264(H264TrackDescription {
                payload_type: 96,
                clock_rate: 90_000,
                control: "track1".into(),
                params: sample_avc_params(),
            })),
            audio: Some(AacTrackDescription {
                payload_type: 97,
                control: "track2".into(),
                config: sample_aac_config(),
            }),
        };
        let rendered = sdp.render();
        assert!(rendered.contains("m=video"));
        assert!(rendered.contains("m=audio"));
        // Track control URIs are independent.
        assert!(rendered.contains("a=control:track1"));
        assert!(rendered.contains("a=control:track2"));
    }

    #[test]
    fn hevc_tier_flag_is_rendered_when_high_tier() {
        let mut params = sample_hevc_params();
        params.general_tier_flag = true;
        let sdp = PlaySdp {
            session_name: "live/hevc".into(),
            host_ip: "127.0.0.1".parse().unwrap(),
            video: Some(VideoTrackDescription::Hevc(HevcTrackDescription {
                payload_type: 96,
                clock_rate: 90_000,
                control: "track1".into(),
                params,
            })),
            audio: None,
        };
        let rendered = sdp.render();
        assert!(rendered.contains("tier-flag=1"));
    }
}
