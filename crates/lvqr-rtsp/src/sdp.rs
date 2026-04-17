//! SDP builder for the RTSP DESCRIBE response on the PLAY path.
//!
//! Pure rendering: given decoded parameter sets (SPS/PPS for H.264,
//! VPS/SPS/PPS for HEVC) and the track timing carried on the
//! broadcaster meta, produce a well-formed SDP body that every
//! mainstream RTSP client (VLC, ffplay, gstreamer's rtspsrc, Exoplayer)
//! will accept as a PLAY description.
//!
//! The builder is intentionally small: it renders one session-level
//! block plus one media block per populated track. Audio + HEVC
//! lands in a follow-up change; today the PLAY path is H.264-only.
//!
//! Session-level and media-level control URIs are relative and
//! resolved against the `Content-Base` header the DESCRIBE response
//! already carries, which the client composes with SETUP requests.
//! A=control:* at session level is idiomatic and keeps VLC happy.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use lvqr_cmaf::AvcParameterSets;
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
    pub video: Option<H264TrackDescription>,
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

        if let Some(ref v) = self.video {
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
        out
    }
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
            video: Some(H264TrackDescription {
                payload_type: 96,
                clock_rate: 90_000,
                control: "track1".into(),
                params: sample_avc_params(),
            }),
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
            video: Some(H264TrackDescription {
                payload_type: 96,
                clock_rate: 90_000,
                control: "track1".into(),
                params: sample_avc_params(),
            }),
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
}
