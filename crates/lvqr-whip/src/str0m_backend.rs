//! `str0m`-backed [`SdpAnswerer`] implementation for WHIP ingest.
//!
//! The sibling of `lvqr_whep::str0m_backend`, but running the poll
//! loop in the ingest direction: inbound RTP -> SRTP decrypt ->
//! str0m depacketizer -> `Event::MediaData` with an Annex B framed
//! H.264 access unit -> bridge callback with an [`IngestSample`].
//!
//! What this module deliberately does NOT do:
//!
//! * **Audio write-through.** The WebRTC offer may include an Opus
//!   media section; we accept it into the answer (str0m handles the
//!   negotiation automatically) and silently drop audio `MediaData`
//!   events on the floor. An AAC encoder is out of scope; a
//!   follow-up session will land the `Opus -> AAC` path or wire an
//!   Opus-native track through a separate sink.
//! * **Trickle ICE ingestion.** The HTTP `PATCH` route accepts
//!   bodies for conformance but the answerer logs once and returns
//!   success. WHIP clients that enumerate host candidates in the
//!   offer do not need trickle.
//! * **Simulcast / RID / layer selection.** Single ingest track
//!   per session; multi-layer ingest lands when we wire up the
//!   ABR / transcoding story.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use bytes::{Bytes, BytesMut};
use str0m::change::{SdpAnswer, SdpOffer};
use str0m::format::Codec as Str0mCodec;
use str0m::media::{Frequency, MediaKind};
use str0m::net::{Protocol, Receive};
use str0m::{Candidate, Event, IceConnectionState, Input, Output, Rtc, RtcConfig};
use tokio::net::UdpSocket;
use tokio::sync::oneshot;

use crate::bridge::{IngestAudioSample, IngestSample, IngestSampleSink};
use crate::server::{SdpAnswerer, SessionHandle, WhipError};
use lvqr_ingest::MediaCodec;

/// Shared configuration for the str0m-backed answerer.
///
/// `host_ip` is the IP address advertised as an ICE host candidate.
/// For a LAN deployment this is the server's primary interface
/// address; for a test this is `127.0.0.1`. If `host_ip` is
/// unreachable to the client, no ICE pair will succeed — that is a
/// deployment question, not a code one.
#[derive(Debug, Clone)]
pub struct Str0mIngestConfig {
    pub host_ip: IpAddr,
}

impl Default for Str0mIngestConfig {
    fn default() -> Self {
        Self {
            host_ip: IpAddr::V4(Ipv4Addr::LOCALHOST),
        }
    }
}

/// [`SdpAnswerer`] backed by the `str0m` crate, running the poll
/// loop in the ingest direction.
pub struct Str0mIngestAnswerer {
    config: Str0mIngestConfig,
    sink: Arc<dyn IngestSampleSink>,
}

impl Str0mIngestAnswerer {
    pub fn new(config: Str0mIngestConfig, sink: Arc<dyn IngestSampleSink>) -> Self {
        static INIT: OnceLock<()> = OnceLock::new();
        INIT.get_or_init(|| {
            str0m::crypto::from_feature_flags().install_process_default();
        });
        Self { config, sink }
    }
}

impl SdpAnswerer for Str0mIngestAnswerer {
    fn create_session(&self, broadcast: &str, offer: &[u8]) -> Result<(Box<dyn SessionHandle>, Bytes), WhipError> {
        let offer_text =
            std::str::from_utf8(offer).map_err(|e| WhipError::MalformedOffer(format!("offer is not utf8: {e}")))?;
        let offer = SdpOffer::from_sdp_string(offer_text)
            .map_err(|e| WhipError::MalformedOffer(format!("sdp parse failed: {e}")))?;

        // One UDP socket per session. Same pattern as
        // `lvqr_whep::Str0mAnswerer::create_session`: bind with
        // `std::net::UdpSocket`, flip to nonblocking, hand to tokio
        // via `from_std`. Host IP comes from the answerer config so
        // deployments with a bridged network interface can advertise
        // the reachable address rather than 127.0.0.1.
        let bind_addr = SocketAddr::new(self.config.host_ip, 0);
        let std_socket = std::net::UdpSocket::bind(bind_addr)
            .map_err(|e| WhipError::AnswererFailed(format!("udp bind {bind_addr} failed: {e}")))?;
        std_socket
            .set_nonblocking(true)
            .map_err(|e| WhipError::AnswererFailed(format!("set_nonblocking failed: {e}")))?;
        let local_addr = std_socket
            .local_addr()
            .map_err(|e| WhipError::AnswererFailed(format!("local_addr failed: {e}")))?;
        let socket = UdpSocket::from_std(std_socket)
            .map_err(|e| WhipError::AnswererFailed(format!("tokio from_std failed: {e}")))?;

        let mut rtc = RtcConfig::new()
            .enable_h264(true)
            .enable_h265(true)
            .enable_opus(true)
            .build(Instant::now());

        let candidate = Candidate::host(local_addr, Protocol::Udp)
            .map_err(|e| WhipError::AnswererFailed(format!("host candidate failed: {e}")))?;
        rtc.add_local_candidate(candidate);

        let answer: SdpAnswer = rtc
            .sdp_api()
            .accept_offer(offer)
            .map_err(|e| WhipError::MalformedOffer(format!("accept_offer failed: {e}")))?;
        let answer_bytes = Bytes::from(answer.to_sdp_string());

        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
        let broadcast_owned = broadcast.to_string();
        let sink = self.sink.clone();

        tokio::spawn(run_session_loop(
            rtc,
            socket,
            local_addr,
            shutdown_rx,
            broadcast_owned,
            sink,
        ));

        tracing::debug!(
            broadcast = %broadcast,
            local = %local_addr,
            "whip str0m session spawned; poll loop running",
        );

        let handle: Box<dyn SessionHandle> = Box::new(Str0mIngestSessionHandle {
            shutdown: Some(shutdown_tx),
            trickle_warned: AtomicBool::new(false),
        });
        Ok((handle, answer_bytes))
    }
}

/// Per-session poll-loop state. Updated each iteration; the task
/// is the sole owner of `Rtc` so no locking is needed.
#[derive(Default)]
struct IngestCtx {
    /// Mid of the negotiated video media section, learned from
    /// `Event::MediaAdded`.
    video_mid: Option<str0m::media::Mid>,
    /// Mid of the negotiated audio media section (Opus). Session
    /// 29 added audio forwarding alongside the existing video
    /// path; `None` when the WHIP publisher did not include an
    /// audio section.
    audio_mid: Option<str0m::media::Mid>,
    /// `true` once `Event::Connected` has fired. Samples that
    /// arrive before the connection is up are unreachable (str0m
    /// has no way to depacketize media before DTLS completes), so
    /// this flag exists mostly for logging.
    connected: bool,
    /// First seen DTS in 90 kHz ticks for the video track. All
    /// emitted video samples are rebased to start at zero so the
    /// downstream fragment model does not carry a random
    /// wall-clock offset.
    dts_base_90k: Option<u64>,
    /// First seen DTS in 48 kHz ticks for the audio track.
    /// Rebased the same way as video but in the Opus sample
    /// rate. Independent of `dts_base_90k` so the two tracks
    /// land at their own zero epochs.
    dts_base_48k: Option<u64>,
    /// One-shot logging guard for the first successful video
    /// sample.
    first_sample_logged: bool,
    /// One-shot logging guard for the first successful audio
    /// sample.
    first_audio_logged: bool,
}

async fn run_session_loop(
    mut rtc: Rtc,
    socket: UdpSocket,
    local_addr: SocketAddr,
    mut shutdown: oneshot::Receiver<()>,
    broadcast: String,
    sink: Arc<dyn IngestSampleSink>,
) {
    let mut ctx = IngestCtx::default();
    let mut buf = vec![0u8; 2048];
    loop {
        let wait_until = loop {
            match rtc.poll_output() {
                Ok(Output::Timeout(when)) => break when,
                Ok(Output::Transmit(transmit)) => {
                    if let Err(e) = socket.send_to(&transmit.contents, transmit.destination).await {
                        tracing::warn!(%broadcast, error = %e, dest = %transmit.destination, "whip udp send failed");
                    }
                }
                Ok(Output::Event(event)) => {
                    if handle_event(event, &mut ctx, &broadcast, sink.as_ref()) {
                        // Terminal event (ice disconnected): exit the loop.
                        return;
                    }
                }
                Err(e) => {
                    tracing::warn!(%broadcast, error = %e, "whip rtc poll_output error; ending session loop");
                    return;
                }
            }
        };

        let now = Instant::now();
        let sleep_dur = wait_until.saturating_duration_since(now).max(Duration::from_millis(0));

        tokio::select! {
            biased;
            _ = &mut shutdown => {
                tracing::debug!(%broadcast, "whip session shutdown signalled");
                return;
            }
            recv = socket.recv_from(&mut buf) => {
                match recv {
                    Ok((n, source)) => {
                        let datagram = match (&buf[..n]).try_into() {
                            Ok(d) => d,
                            Err(e) => {
                                tracing::trace!(%broadcast, error = ?e, "whip: unparseable datagram, skipping");
                                continue;
                            }
                        };
                        let input = Input::Receive(
                            Instant::now(),
                            Receive {
                                proto: Protocol::Udp,
                                source,
                                destination: local_addr,
                                contents: datagram,
                            },
                        );
                        if let Err(e) = rtc.handle_input(input) {
                            tracing::warn!(%broadcast, error = %e, "whip rtc handle_input(receive) failed");
                            return;
                        }
                    }
                    Err(e) => {
                        tracing::warn!(%broadcast, error = %e, "whip udp recv_from failed");
                        return;
                    }
                }
            }
            _ = tokio::time::sleep(sleep_dur) => {
                if let Err(e) = rtc.handle_input(Input::Timeout(Instant::now())) {
                    tracing::warn!(%broadcast, error = %e, "whip rtc handle_input(timeout) failed");
                    return;
                }
            }
        }
    }
}

/// Process one `str0m::Event`. Returns `true` iff the session
/// should exit the poll loop (terminal state).
fn handle_event(event: Event, ctx: &mut IngestCtx, broadcast: &str, sink: &dyn IngestSampleSink) -> bool {
    match event {
        Event::IceConnectionStateChange(state) => {
            tracing::debug!(%broadcast, ?state, "whip ice state change");
            if state == IceConnectionState::Disconnected {
                tracing::info!(%broadcast, "whip ice disconnected; ending session loop");
                return true;
            }
        }
        Event::Connected => {
            ctx.connected = true;
            tracing::info!(%broadcast, "whip webrtc connected");
        }
        Event::MediaAdded(added) if matches!(added.kind, MediaKind::Video) => {
            ctx.video_mid = Some(added.mid);
            tracing::debug!(%broadcast, mid = ?added.mid, "whip media added: video");
        }
        Event::MediaAdded(added) if matches!(added.kind, MediaKind::Audio) => {
            ctx.audio_mid = Some(added.mid);
            tracing::debug!(%broadcast, mid = ?added.mid, "whip media added: audio");
        }
        Event::MediaAdded(added) => {
            tracing::trace!(%broadcast, mid = ?added.mid, kind = ?added.kind, "whip media added: other");
        }
        Event::MediaData(data) => {
            if ctx.video_mid == Some(data.mid) {
                forward_video_sample(data, ctx, broadcast, sink);
            } else if ctx.audio_mid == Some(data.mid) {
                forward_audio_sample(data, ctx, broadcast, sink);
            } else {
                // Unknown mid (data channel?). Drop silently.
            }
        }
        _ => {}
    }
    false
}

fn forward_video_sample(
    data: str0m::media::MediaData,
    ctx: &mut IngestCtx,
    broadcast: &str,
    sink: &dyn IngestSampleSink,
) {
    if data.data.is_empty() {
        return;
    }
    let codec = match data.params.spec().codec {
        Str0mCodec::H264 => MediaCodec::H264,
        Str0mCodec::H265 => MediaCodec::H265,
        other => {
            tracing::trace!(%broadcast, codec = ?other, "whip: ignoring non-H26x video sample");
            return;
        }
    };
    let keyframe = data.is_keyframe();
    let dts_90k_abs = data.time.rebase(Frequency::NINETY_KHZ).numer();
    let dts_base = *ctx.dts_base_90k.get_or_insert(dts_90k_abs);
    let dts_rebased = dts_90k_abs.saturating_sub(dts_base);

    let mut payload = BytesMut::with_capacity(data.data.len());
    payload.extend_from_slice(&data.data);
    let sample = IngestSample {
        dts_90k: dts_rebased,
        keyframe,
        codec,
        annex_b: payload.freeze(),
    };

    if !ctx.first_sample_logged {
        ctx.first_sample_logged = true;
        tracing::info!(
            %broadcast,
            ?codec,
            keyframe,
            dts = dts_rebased,
            bytes = sample.annex_b.len(),
            "whip: first video sample forwarded to bridge",
        );
    }
    sink.on_sample(broadcast, sample);
}

/// Forward one depacketized Opus frame to the bridge via
/// [`IngestSampleSink::on_audio_sample`].
///
/// Session 29 added this path so WHIP publishers negotiating
/// Opus land audio in the MoQ broadcast alongside their video
/// track. DTS is rebased to the first observed audio
/// `MediaTime` rather than the video epoch so the two tracks
/// have independent zero anchors -- MSE / MoQ subscribers
/// align them via wall-clock presentation time, not via a
/// shared track DTS, so an extra epoch does no harm.
fn forward_audio_sample(
    data: str0m::media::MediaData,
    ctx: &mut IngestCtx,
    broadcast: &str,
    sink: &dyn IngestSampleSink,
) {
    if data.data.is_empty() {
        return;
    }
    // Only accept Opus; reject any other audio codec (e.g. PCMA /
    // PCMU fallback). We intentionally do not build a transcoder.
    if data.params.spec().codec != Str0mCodec::Opus {
        tracing::trace!(
            %broadcast,
            codec = ?data.params.spec().codec,
            "whip: ignoring non-Opus audio sample",
        );
        return;
    }
    // str0m's `MediaTime` for an Opus track is in the 48 kHz
    // clock domain; rebase to start at zero.
    let abs = data.time.rebase(Frequency::FORTY_EIGHT_KHZ).numer();
    let base = *ctx.dts_base_48k.get_or_insert(abs);
    let dts_rebased = abs.saturating_sub(base);

    // WebRTC Opus defaults to 20 ms per packet = 960 samples at
    // 48 kHz. str0m does not expose the per-packet duration on
    // `MediaData` in 0.18, so we default to the common value;
    // subscribers tolerate a slightly-wrong `duration` in the
    // `trun` box because MSE reconstructs the actual duration
    // from the Opus packet itself.
    const DEFAULT_OPUS_FRAME_TICKS: u32 = 960;

    let payload = Bytes::copy_from_slice(&data.data);
    let sample = IngestAudioSample {
        dts_48k: dts_rebased,
        duration_48k: DEFAULT_OPUS_FRAME_TICKS,
        payload,
    };

    if !ctx.first_audio_logged {
        ctx.first_audio_logged = true;
        tracing::info!(
            %broadcast,
            dts = dts_rebased,
            bytes = sample.payload.len(),
            "whip: first opus sample forwarded to bridge",
        );
    }
    sink.on_audio_sample(broadcast, sample);
}

/// Per-session handle produced by [`Str0mIngestAnswerer::create_session`].
pub struct Str0mIngestSessionHandle {
    shutdown: Option<oneshot::Sender<()>>,
    trickle_warned: AtomicBool,
}

impl Drop for Str0mIngestSessionHandle {
    fn drop(&mut self) {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
    }
}

impl SessionHandle for Str0mIngestSessionHandle {
    fn add_trickle(&self, _sdp_fragment: &[u8]) -> Result<(), WhipError> {
        if !self.trickle_warned.swap(true, Ordering::Relaxed) {
            tracing::warn!("whip str0m trickle ICE not yet wired; ignoring fragment");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::NoopIngestSampleSink;

    /// Chrome-shaped audio-only offer copied from the whep crate so
    /// the answerer's parse + accept path is covered without
    /// constructing a video section by hand. str0m will produce a
    /// valid SDP answer for this even though the test will never
    /// actually route any video through it.
    const CHROME_AUDIO_OFFER: &str = "v=0\r\n\
        o=- 5058682828002148772 3 IN IP4 127.0.0.1\r\n\
        s=-\r\n\
        t=0 0\r\n\
        a=group:BUNDLE 0\r\n\
        a=msid-semantic: WMS 5UUdwiuY7OML2EkQtF38pJtNP5v7In1LhjEK\r\n\
        m=audio 9 UDP/TLS/RTP/SAVPF 111\r\n\
        c=IN IP4 0.0.0.0\r\n\
        a=rtcp:9 IN IP4 0.0.0.0\r\n\
        a=ice-ufrag:S5hk\r\n\
        a=ice-pwd:0zV/Yu3y8aDzbHgqWhnVQhqP\r\n\
        a=ice-options:trickle\r\n\
        a=fingerprint:sha-256 8C:64:ED:03:76:D0:3D:B4:88:08:91:64:08:80:A8:C6:5A:BF:8B:4E:38:27:96:CA:08:49:25:73:46:60:20:DC\r\n\
        a=setup:actpass\r\n\
        a=mid:0\r\n\
        a=sendrecv\r\n\
        a=msid:5UUdwiuY7OML2EkQtF38pJtNP5v7In1LhjEK f78dde68-7055-4e20-bb37-433803dd1ed1\r\n\
        a=rtcp-mux\r\n\
        a=rtpmap:111 opus/48000/2\r\n\
        a=rtcp-fb:111 transport-cc\r\n\
        a=fmtp:111 minptime=10;useinbandfec=1\r\n\
        a=ssrc:3948621874 cname:xeXs3aE9AOBn00yJ\r\n\
        ";

    #[tokio::test]
    async fn accepts_offer_and_returns_parseable_answer() {
        let answerer = Str0mIngestAnswerer::new(Str0mIngestConfig::default(), Arc::new(NoopIngestSampleSink));
        let (handle, answer) = answerer
            .create_session("live/test", CHROME_AUDIO_OFFER.as_bytes())
            .expect("offer accepted by str0m");

        let answer_text = std::str::from_utf8(&answer).expect("answer is utf8");
        assert!(answer_text.starts_with("v=0"));
        assert!(answer_text.contains("m=audio"));
        assert!(answer_text.contains("a=candidate:"));
        SdpAnswer::from_sdp_string(answer_text).expect("answer re-parses");

        handle
            .add_trickle(b"a=candidate:0 1 udp 1 127.0.0.1 9 typ host\r\n")
            .unwrap();

        tokio::time::sleep(Duration::from_millis(20)).await;
        drop(handle);
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    fn unwrap_err(result: Result<(Box<dyn SessionHandle>, Bytes), WhipError>) -> WhipError {
        match result {
            Ok(_) => panic!("expected create_session to fail"),
            Err(e) => e,
        }
    }

    #[tokio::test]
    async fn rejects_non_utf8_offer() {
        let answerer = Str0mIngestAnswerer::new(Str0mIngestConfig::default(), Arc::new(NoopIngestSampleSink));
        let err = unwrap_err(answerer.create_session("live/test", &[0xff, 0xfe, 0xfd]));
        assert!(matches!(err, WhipError::MalformedOffer(_)), "got {err:?}");
    }

    #[tokio::test]
    async fn rejects_malformed_sdp() {
        let answerer = Str0mIngestAnswerer::new(Str0mIngestConfig::default(), Arc::new(NoopIngestSampleSink));
        let err = unwrap_err(answerer.create_session("live/test", b"not an sdp document"));
        assert!(matches!(err, WhipError::MalformedOffer(_)), "got {err:?}");
    }
}
