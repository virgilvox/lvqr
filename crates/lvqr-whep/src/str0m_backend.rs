//! `str0m`-backed [`SdpAnswerer`] implementation.
//!
//! Session 20 landed the offer / answer half: parse offer, bind a
//! UDP socket, call `Rtc::sdp_api().accept_offer`, return the SDP
//! answer. Session 21 (this file) wires the sans-IO poll loop so
//! ICE, DTLS, and SRTP can actually complete against a real browser.
//! Each successful `create_session` now spawns a tokio task that
//! owns the `Rtc` state machine and the UDP socket and runs the
//! canonical `poll_output` / `handle_input` cycle. Dropping the
//! session handle closes a `oneshot` shutdown signal and the loop
//! exits cleanly on the next wakeup.
//!
//! Session 22 adds the video media-write path. Each successful
//! `create_session` now also builds an `mpsc::unbounded_channel`
//! for `SessionMsg::Sample`, and `on_raw_sample` forwards every
//! video sample to the poll task. The task tracks the video `Mid`
//! via `Event::MediaAdded`, resolves the H.264 `Pt` via
//! `Writer::payload_params`, converts the AVCC-framed payload to
//! Annex B (str0m's `H264Packetizer` scans for Annex B start codes
//! and silently drops input that has none, so this conversion is
//! load-bearing, not cosmetic), and calls `Writer::write` once per
//! sample. Writes before `Event::Connected` are dropped explicitly;
//! str0m documents that they would be dropped internally anyway,
//! but skipping them at the source avoids churning `&mut Rtc` for
//! no effect.
//!
//! Session 28 adds HEVC alongside H.264. `RtcConfig` now enables
//! both `h264` and `h265`, `SessionCtx` stores parallel
//! `video_pt_h264` / `video_pt_h265` slots resolved in one sweep
//! over `Writer::payload_params`, and `write_video_sample`
//! receives the incoming sample's [`lvqr_ingest::MediaCodec`] tag
//! (carried through `SessionMsg::Video.codec`) and picks the
//! matching pt. A sample whose codec is not in the negotiated
//! payload params -- e.g. an HEVC publisher fanning out to a
//! Firefox subscriber that offered only H.264 -- is dropped with
//! a one-shot warn. The AVCC -> Annex B converter is codec-
//! agnostic (length-prefixed framing is the same for both), so
//! the write path is shared.
//!
//! Session 113 closes the AAC-on-Opus gap: when the answerer is
//! built with [`Str0mAnswerer::with_aac_to_opus_factory`] and the
//! subscriber negotiates Opus, AAC samples arriving on `1.mp4` are
//! routed through an [`lvqr_transcode::AacToOpusEncoder`] spawned
//! once the AAC AudioSpecificConfig is known. The encoder's Opus
//! output flows back into the same poll loop via a `tokio::sync::
//! mpsc::UnboundedReceiver` arm and lands on `Writer::write` through
//! the negotiated Opus `Pt`. The feature is gated behind the
//! `aac-opus` Cargo feature so deployments without GStreamer
//! continue to build the crate with the legacy drop-and-warn shape.
//!
//! What this module still deliberately does NOT do:
//!
//! * Trickle ICE ingestion. WHEP rarely needs trickle once the offer
//!   already embeds every host candidate; the HTTP surface still
//!   accepts PATCH bodies so conformant clients do not error out.
//!   `add_trickle` logs once and returns success.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
#[cfg(feature = "aac-opus")]
use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use bytes::Bytes;
use lvqr_admin::LatencyTracker;
use lvqr_cmaf::RawSample;
use lvqr_core::now_unix_ms;
use lvqr_ingest::MediaCodec;
use str0m::change::{SdpAnswer, SdpOffer};
use str0m::format::Codec;
use str0m::media::{MediaKind, MediaTime, Mid, Pt};
use str0m::net::{Protocol, Receive};
use str0m::{Candidate, Event, IceConnectionState, Input, Output, Rtc, RtcConfig};
use tokio::net::UdpSocket;
use tokio::sync::{mpsc, oneshot};

#[cfg(feature = "aac-opus")]
use lvqr_transcode::{AacAudioConfig, AacToOpusEncoder, AacToOpusEncoderFactory, OpusFrame};

use crate::server::{SdpAnswerer, SessionHandle, WhepError};

/// Transport label the str0m backend reports to the shared
/// [`LatencyTracker`]. Matches the `"whep"` row in the operator
/// runbook's threshold decision table.
const TRANSPORT_LABEL: &str = "whep";

/// Shared configuration for the str0m-backed answerer.
///
/// `host_ip` is the IP address advertised as an ICE host candidate.
/// For a LAN deployment this is typically the server's primary
/// interface address; for a test this is `127.0.0.1`. Binding a UDP
/// socket to `host_ip:0` is how the OS hands us a free port per
/// session. If `host_ip` is unreachable to the client, no ICE pair
/// will succeed — that is a deployment question, not a code one.
#[derive(Debug, Clone)]
pub struct Str0mConfig {
    pub host_ip: IpAddr,
}

impl Default for Str0mConfig {
    fn default() -> Self {
        Self {
            host_ip: IpAddr::V4(Ipv4Addr::LOCALHOST),
        }
    }
}

/// [`SdpAnswerer`] backed by the `str0m` crate.
///
/// Construct with a [`Str0mConfig`] naming the host IP to advertise
/// and clone into [`crate::WhepServer::new`]. The answerer installs
/// the process-wide `str0m` crypto provider on first construction
/// via a `OnceLock`; subsequent constructions are no-ops.
///
/// Tier 4 item 4.7 session 110 B: [`Str0mAnswerer::with_slo_tracker`]
/// attaches an optional [`LatencyTracker`] so the per-session poll
/// loop records one sample per `rtc.writer(mid).write` call under
/// `transport="whep"`. The default `new` constructor leaves the
/// tracker unset (compat with tests + deployments that have not
/// wired the admin tracker yet).
pub struct Str0mAnswerer {
    config: Str0mConfig,
    slo: Option<LatencyTracker>,
    #[cfg(feature = "aac-opus")]
    aac_opus_factory: Option<Arc<AacToOpusEncoderFactory>>,
}

impl Str0mAnswerer {
    /// Build a new answerer and ensure the str0m crypto provider is
    /// installed for this process. `install_process_default` is
    /// idempotent (backed by a `OnceLock` inside str0m-proto), so
    /// calling it from more than one answerer is safe.
    pub fn new(config: Str0mConfig) -> Self {
        static INIT: OnceLock<()> = OnceLock::new();
        INIT.get_or_init(|| {
            str0m::crypto::from_feature_flags().install_process_default();
        });
        Self {
            config,
            slo: None,
            #[cfg(feature = "aac-opus")]
            aac_opus_factory: None,
        }
    }

    /// Attach a shared [`LatencyTracker`] to this answerer. Every
    /// session the answerer creates clones the tracker into its
    /// poll loop and records one sample per successful
    /// `Writer::write` call under the `"whep"` transport label.
    /// Tier 4 item 4.7 session 110 B.
    pub fn with_slo_tracker(mut self, tracker: LatencyTracker) -> Self {
        self.slo = Some(tracker);
        self
    }

    /// Attach an AAC-to-Opus transcoder factory. Each session the
    /// answerer creates clones the factory handle and lazily spawns
    /// a per-session encoder once the publisher's AAC
    /// AudioSpecificConfig arrives and the first AAC sample needs
    /// to be routed through the Opus writer.
    ///
    /// Session 113. Available only when the crate is built with
    /// `--features aac-opus`. Without the feature (or when the
    /// factory is `None`), AAC samples routed to a WHEP subscriber
    /// with an Opus-only answer are still dropped with a one-shot
    /// warn, matching the pre-113 behaviour.
    #[cfg(feature = "aac-opus")]
    pub fn with_aac_to_opus_factory(mut self, factory: Arc<AacToOpusEncoderFactory>) -> Self {
        self.aac_opus_factory = Some(factory);
        self
    }
}

impl SdpAnswerer for Str0mAnswerer {
    fn create_session(&self, broadcast: &str, offer: &[u8]) -> Result<(Box<dyn SessionHandle>, Bytes), WhepError> {
        let offer_text =
            std::str::from_utf8(offer).map_err(|e| WhepError::MalformedOffer(format!("offer is not utf8: {e}")))?;
        let offer = SdpOffer::from_sdp_string(offer_text)
            .map_err(|e| WhepError::MalformedOffer(format!("sdp parse failed: {e}")))?;

        // One UDP socket per session. Binding on port 0 lets the OS
        // pick a free ephemeral port. Using the configured host IP
        // ensures the ICE candidate we advertise is actually
        // reachable from the client's network namespace.
        //
        // We bind with `std::net::UdpSocket` first so we can set
        // nonblocking mode before handing the FD to tokio via
        // `tokio::net::UdpSocket::from_std`, which is tokio's
        // required conversion contract.
        // Wildcard bind so the OS can route outbound packets through
        // any local interface; the ICE host candidate we ADVERTISE is
        // `host_ip:port` (a routable address). Mirrors the WHIP-side
        // fix: a loopback bind cannot send to non-loopback ICE-pair
        // destinations, which fails with `Can't assign requested
        // address` once the browser nominates an srflx candidate.
        let bind_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 0);
        let std_socket = std::net::UdpSocket::bind(bind_addr)
            .map_err(|e| WhepError::AnswererFailed(format!("udp bind {bind_addr} failed: {e}")))?;
        std_socket
            .set_nonblocking(true)
            .map_err(|e| WhepError::AnswererFailed(format!("set_nonblocking failed: {e}")))?;
        let bound = std_socket
            .local_addr()
            .map_err(|e| WhepError::AnswererFailed(format!("local_addr failed: {e}")))?;
        let candidate_addr = SocketAddr::new(self.config.host_ip, bound.port());
        // The session loop hands `local_addr` to str0m as the
        // `destination` on every received datagram for ICE pair
        // matching. It must equal the advertised candidate address,
        // not the wildcard bind. Mirrors the WHIP-side fix.
        let local_addr = candidate_addr;
        let socket = UdpSocket::from_std(std_socket)
            .map_err(|e| WhepError::AnswererFailed(format!("tokio from_std failed: {e}")))?;

        let mut rtc = RtcConfig::new()
            .enable_h264(true)
            .enable_h265(true)
            .enable_opus(true)
            .build(Instant::now());

        let candidate = Candidate::host(candidate_addr, Protocol::Udp)
            .map_err(|e| WhepError::AnswererFailed(format!("host candidate failed: {e}")))?;
        rtc.add_local_candidate(candidate);

        let answer: SdpAnswer = rtc
            .sdp_api()
            .accept_offer(offer)
            .map_err(|e| WhepError::MalformedOffer(format!("accept_offer failed: {e}")))?;
        let answer_bytes = Bytes::from(answer.to_sdp_string());

        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
        let (sample_tx, sample_rx) = mpsc::unbounded_channel::<SessionMsg>();
        // Session 113: separate channel for Opus frames the AAC-to-
        // Opus encoder emits asynchronously on its GStreamer worker
        // thread. The poll loop adds a new select arm on the
        // receiver; the encoder is built lazily (if at all) so the
        // sender is handed to the encoder at build time and the
        // poll loop holds the receiver on the stack frame.
        let (opus_tx, opus_rx) = mpsc::unbounded_channel::<OpusMsg>();
        let broadcast_owned = broadcast.to_string();

        #[cfg(feature = "aac-opus")]
        let aac_opus_factory = self.aac_opus_factory.clone();

        // Spawn the sans-IO poll loop. `tokio::spawn` requires an
        // active runtime; `WhepServer` is only constructed inside
        // `lvqr-cli`'s tokio-based axum server, so this is always
        // satisfied in real deployments. Tests that hit this code
        // path must use `#[tokio::test]`.
        tokio::spawn(run_session_loop(
            rtc,
            socket,
            local_addr,
            shutdown_rx,
            sample_rx,
            opus_tx,
            opus_rx,
            broadcast_owned,
            self.slo.clone(),
            #[cfg(feature = "aac-opus")]
            aac_opus_factory,
        ));

        tracing::debug!(
            broadcast = %broadcast,
            local = %local_addr,
            "str0m session spawned; poll loop running",
        );

        let handle: Box<dyn SessionHandle> = Box::new(Str0mSessionHandle {
            samples: sample_tx,
            shutdown: Some(shutdown_tx),
            trickle_warned: AtomicBool::new(false),
            unknown_track_warned: AtomicBool::new(false),
        });
        Ok((handle, answer_bytes))
    }

    /// Override the trait default to also accept `Aac` when the
    /// answerer was wired with an `AacToOpusEncoderFactory`. The
    /// router gate (audit C-9) reads this at session-start time
    /// to surface a 422 instead of accepting a session whose
    /// audio path would silently drop on a non-transcode build.
    /// The `aac_opus_factory` field is feature-gated on
    /// `aac-opus`, so the AAC arm is only reachable when the
    /// crate was built with the feature; otherwise AAC falls
    /// through to the `false` default.
    fn supports_audio_codec(&self, codec: MediaCodec) -> bool {
        match codec {
            MediaCodec::Opus => true,
            #[cfg(feature = "aac-opus")]
            MediaCodec::Aac => self.aac_opus_factory.is_some(),
            _ => false,
        }
    }
}

/// Opus frames flowing from the AAC-to-Opus encoder worker back
/// into the session poll loop. The poll loop owns the receiver and
/// gets a dedicated `select!` arm; the sender clones through the
/// encoder's appsink callback. Kept as a discrete type so the build
/// without `aac-opus` still has a nameable channel shape even
/// though the encoder never writes to it.
#[derive(Debug, Clone)]
struct OpusMsg {
    payload: Bytes,
    duration_ticks: u32,
}

/// Message pumped from `Str0mSessionHandle::on_raw_sample` /
/// `on_audio_config` into the poll loop task. Kept private so the
/// channel shape can evolve without affecting the public
/// `SessionHandle` trait.
enum SessionMsg {
    /// A video sample ready to hand to `Writer::write`. `payload` is
    /// the AVCC-framed bytes straight from the ingest bridge; the
    /// poll task converts to Annex B before calling str0m. `codec`
    /// picks which negotiated `Pt` the sample is routed through
    /// (session 28 added H265 alongside the existing H264 path).
    /// `ingest_time_ms` is the upstream bridge's wall-clock stamp
    /// (session 110 B); the poll loop records
    /// `now_unix_ms - ingest_time_ms` under `transport="whep"`
    /// after a successful `Writer::write`. Zero means "unset";
    /// the egress guard skips it, matching the HLS + DASH drains.
    Video {
        payload: Bytes,
        dts: u64,
        keyframe: bool,
        codec: MediaCodec,
        ingest_time_ms: u64,
    },
    /// An AAC raw access unit and its source-clock dts. Session 113
    /// routes this into the per-session AAC-to-Opus encoder when the
    /// answerer was built with a factory and the AudioSpecificConfig
    /// has arrived; otherwise the poll loop drops with a one-shot
    /// warn, matching the pre-113 behaviour. The dts is in the
    /// publisher's audio-clock ticks (matching the rate on the
    /// AudioConfig message); the encoder handles the rescale to
    /// Opus 48 kHz internally.
    Aac {
        payload: Bytes,
        dts: u64,
        ingest_time_ms: u64,
    },
    /// The publisher's AAC AudioSpecificConfig. Session 113 stores
    /// this on `SessionCtx` so the AAC-to-Opus encoder can be built
    /// lazily on the first AAC sample; `config_bytes` is the raw
    /// 2-byte (or longer for HE-AAC) ASC payload exactly as FLV
    /// delivered it. Only fires when the incoming track is `1.mp4`
    /// and codec is [`MediaCodec::Aac`].
    AudioConfig {
        config_bytes: Bytes,
        sample_rate: u32,
        channels: u8,
        object_type: u8,
    },
}

/// Poll-task-local state captured across iterations. The task is
/// the sole owner of the `Rtc` so it can safely mutate this on the
/// stack frame without any locks.
#[derive(Default)]
struct SessionCtx {
    /// Mid of the negotiated video media section, learned from
    /// `Event::MediaAdded`. `None` until the event arrives.
    video_mid: Option<Mid>,
    /// Mid of the negotiated audio media section. Session 30
    /// added this slot so WHIP publishers sending Opus can
    /// reach a matching WHEP subscriber through the same poll
    /// loop.
    audio_mid: Option<Mid>,
    /// Negotiated H.264 payload type for the video mid, resolved
    /// lazily from `Writer::payload_params`. `None` when the
    /// client did not include H.264 in its offer.
    video_pt_h264: Option<Pt>,
    /// Negotiated H.265 payload type for the video mid, resolved
    /// the same way. `None` when the client did not include H.265
    /// in its offer (common: Chrome without the experimental
    /// flag, Firefox). Session 28 added this slot so HEVC
    /// publishers can reach a matching WHEP subscriber through
    /// the same poll loop as H.264 subscribers.
    video_pt_h265: Option<Pt>,
    /// Negotiated Opus payload type for the audio mid. `None`
    /// when the client did not include Opus in its offer
    /// (uncommon: every major browser supports Opus by default).
    /// Session 30 added this slot so WHIP Opus publishers can
    /// fan out audio to WHEP subscribers without transcoding.
    audio_pt_opus: Option<Pt>,
    /// True once `Event::Connected` has fired. Samples that arrive
    /// before this point are dropped rather than written.
    connected: bool,
    /// One-shot logging guard: log the first write error, then go
    /// silent so a wedged stream does not drown the tracing output.
    write_error_logged: bool,
    /// One-shot logging guard for the first successful write.
    first_write_logged: bool,
    /// One-shot logging guard: log the first sample whose codec
    /// was not present in the negotiated payload params. After
    /// the warn fires, subsequent unmatched samples are dropped
    /// silently so a mismatched publisher/subscriber pairing
    /// does not spam the log.
    unmatched_codec_logged: bool,

    /// Session 113: cached AAC AudioSpecificConfig the publisher
    /// sent via `on_audio_config`. Required to spawn the
    /// AAC-to-Opus encoder. `None` until the first config lands.
    #[cfg(feature = "aac-opus")]
    aac_config: Option<AacAudioConfig>,
    /// Session 113: the per-session AAC-to-Opus encoder. Built
    /// lazily on the first AAC sample after `aac_config` is
    /// known and the answerer was constructed with a factory.
    #[cfg(feature = "aac-opus")]
    aac_encoder: Option<AacToOpusEncoder>,
    /// Session 113: running decode timestamp in Opus 48 kHz ticks
    /// for writing Opus frames through `Writer::write`. Advanced
    /// by each [`OpusFrame::duration_ticks`] the encoder emits.
    opus_write_dts: u64,
    /// Session 113: most-recent AAC sample's ingest wall-clock
    /// (ms since UNIX epoch). Used as an approximate
    /// `ingest_time_ms` stamp for Opus SLO samples on the WHEP
    /// audio path; the opusenc worker buffers internally so there
    /// is no 1:1 correspondence between AAC in and Opus out, but
    /// 20 ms Opus frames vs ~21-23 ms AAC frames line up closely
    /// enough for a histogram bucket. `0` is "unset"; the record
    /// guard skips it to match the HLS + DASH + video WHEP drains.
    last_aac_ingest_ms: u64,
    /// Session 113: one-shot warn guard for AAC drops when the
    /// session was created without an AAC-to-Opus factory.
    aac_without_factory_warned: bool,
}

/// Run the sans-IO `Rtc` state machine forward.
///
/// Canonical `str0m` event loop: drain `poll_output` until it yields
/// a `Timeout`, then wait for whichever of `(shutdown signal, socket
/// readiness, timeout deadline)` fires first and feed the resulting
/// `Input` back in. Exits on:
///
/// * `shutdown` oneshot resolved (session handle dropped).
/// * `poll_output` or `handle_input` returning an error.
/// * ICE connection transitioning to `Disconnected`.
///
/// Any of these unwinds the task cleanly; `tokio::spawn` drops the
/// `Rtc` and closes the `UdpSocket` on return.
#[allow(clippy::too_many_arguments)]
async fn run_session_loop(
    mut rtc: Rtc,
    socket: UdpSocket,
    local_addr: SocketAddr,
    mut shutdown: oneshot::Receiver<()>,
    mut samples: mpsc::UnboundedReceiver<SessionMsg>,
    opus_tx: mpsc::UnboundedSender<OpusMsg>,
    mut opus_rx: mpsc::UnboundedReceiver<OpusMsg>,
    broadcast: String,
    slo: Option<LatencyTracker>,
    #[cfg(feature = "aac-opus")] aac_opus_factory: Option<Arc<AacToOpusEncoderFactory>>,
) {
    let mut ctx = SessionCtx::default();
    let mut buf = vec![0u8; 2048];

    // Hand the opus_tx into the aac encoder when we build one;
    // keep a separate clone on the stack so the lifetime is not
    // tied to a single build attempt (the encoder builds lazily
    // once both the factory and the AAC config are ready). Without
    // the feature, the sender is held but never written.
    #[cfg(feature = "aac-opus")]
    let encoder_opus_tx = opus_tx.clone();
    let _ = &opus_tx; // silence unused when feature is off
    loop {
        // Drain outputs until `Rtc` blocks on a timeout. Events are
        // absorbed into the local `SessionCtx` so later sample-arm
        // iterations know whether writes are allowed yet.
        let wait_until = loop {
            match rtc.poll_output() {
                Ok(Output::Timeout(when)) => break when,
                Ok(Output::Transmit(transmit)) => {
                    if let Err(e) = socket.send_to(&transmit.contents, transmit.destination).await {
                        tracing::warn!(%broadcast, error = %e, dest = %transmit.destination, "udp send failed");
                    }
                }
                Ok(Output::Event(event)) => {
                    absorb_event(&event, &mut ctx, &broadcast);
                    if let Event::IceConnectionStateChange(IceConnectionState::Disconnected) = &event {
                        tracing::info!(%broadcast, "ice disconnected; ending session loop");
                        return;
                    }
                }
                Err(e) => {
                    tracing::warn!(%broadcast, error = %e, "rtc poll_output error; ending session loop");
                    return;
                }
            }
        };

        // Lazily resolve the negotiated `Pt` values the first
        // time both the video mid is known and at least one slot
        // is still empty. Doing this outside `poll_output`
        // avoids borrowing conflicts and keeps the resolution
        // near the place that consumes the pt. Session 28
        // resolves H.264 and H.265 in the same pass so a single
        // subscriber can handle either codec if the client
        // offered both.
        if (ctx.video_pt_h264.is_none() || ctx.video_pt_h265.is_none())
            && let Some(mid) = ctx.video_mid
            && let Some(writer) = rtc.writer(mid)
        {
            for params in writer.payload_params() {
                match params.spec().codec {
                    Codec::H264 if ctx.video_pt_h264.is_none() => {
                        ctx.video_pt_h264 = Some(params.pt());
                        tracing::debug!(%broadcast, pt = ?params.pt(), "resolved h264 pt");
                    }
                    Codec::H265 if ctx.video_pt_h265.is_none() => {
                        ctx.video_pt_h265 = Some(params.pt());
                        tracing::debug!(%broadcast, pt = ?params.pt(), "resolved h265 pt");
                    }
                    _ => {}
                }
            }
        }
        // Parallel sweep for the audio mid. Session 30 added
        // this block so an Opus publisher reaches a subscriber
        // that offered Opus without the session backend having
        // to care about codec ordering in the SDP negotiation.
        if ctx.audio_pt_opus.is_none()
            && let Some(mid) = ctx.audio_mid
            && let Some(writer) = rtc.writer(mid)
        {
            for params in writer.payload_params() {
                if params.spec().codec == Codec::Opus {
                    ctx.audio_pt_opus = Some(params.pt());
                    tracing::debug!(%broadcast, pt = ?params.pt(), "resolved opus pt");
                    break;
                }
            }
        }

        let now = Instant::now();
        let sleep_dur = wait_until.saturating_duration_since(now).max(Duration::from_millis(0));

        tokio::select! {
            biased;
            _ = &mut shutdown => {
                tracing::debug!(%broadcast, "session shutdown signalled");
                return;
            }
            msg = samples.recv() => {
                match msg {
                    Some(SessionMsg::Video { payload, dts, keyframe, codec, ingest_time_ms }) => {
                        match write_sample(&mut rtc, &mut ctx, &broadcast, payload, dts, keyframe, codec) {
                            Ok(true) => {
                                // Tier 4 item 4.7 session 110 B: one
                                // SLO sample per successful
                                // `Writer::write`. Skip when no
                                // tracker was wired at answerer
                                // construction or when the producer
                                // passed `0` (synthetic test, pre-
                                // 110 caller). The recording point
                                // is after str0m accepts the write
                                // because the RTP packetization is
                                // what hits the wire; pre-Connected
                                // drops and codec-mismatch drops
                                // both return `Ok(false)` and are
                                // excluded from the histogram.
                                if let Some(tracker) = slo.as_ref()
                                    && ingest_time_ms > 0
                                {
                                    let latency = now_unix_ms().saturating_sub(ingest_time_ms);
                                    tracker.record(&broadcast, TRANSPORT_LABEL, latency);
                                }
                            }
                            Ok(false) => {
                                // Pre-Connected, codec mismatch, or
                                // AAC-on-Opus drop. Not a wire
                                // event; no SLO sample recorded.
                            }
                            Err(()) => {
                                // `write_sample` logged already.
                            }
                        }
                    }
                    Some(SessionMsg::Aac { payload, dts, ingest_time_ms }) => {
                        ctx.last_aac_ingest_ms = ingest_time_ms;
                        #[cfg(feature = "aac-opus")]
                        {
                            // Build the encoder lazily on the first
                            // AAC sample whenever the factory and
                            // the AudioSpecificConfig are both
                            // ready. If either is missing, drop
                            // with a one-shot warn.
                            if ctx.aac_encoder.is_none()
                                && let (Some(factory), Some(aac_cfg)) =
                                    (aac_opus_factory.as_ref(), ctx.aac_config.as_ref())
                            {
                                match factory.build(aac_cfg.clone(), opus_frame_sender_bridge(encoder_opus_tx.clone())) {
                                    Some(enc) => {
                                        tracing::info!(
                                            %broadcast,
                                            sample_rate = aac_cfg.sample_rate,
                                            channels = aac_cfg.channels,
                                            "whep: AAC-to-Opus encoder spawned",
                                        );
                                        ctx.aac_encoder = Some(enc);
                                    }
                                    None => {
                                        if !ctx.aac_without_factory_warned {
                                            ctx.aac_without_factory_warned = true;
                                            tracing::warn!(
                                                %broadcast,
                                                "whep: AAC-to-Opus factory returned None (GStreamer elements missing?); dropping AAC samples",
                                            );
                                        }
                                    }
                                }
                            }
                            if let Some(enc) = ctx.aac_encoder.as_ref() {
                                enc.push(&payload, dts);
                            } else if !ctx.aac_without_factory_warned {
                                ctx.aac_without_factory_warned = true;
                                tracing::warn!(
                                    %broadcast,
                                    "whep: AAC sample arrived before AudioSpecificConfig or without factory; dropping",
                                );
                            }
                        }
                        #[cfg(not(feature = "aac-opus"))]
                        {
                            let _ = (payload, dts);
                            if !ctx.aac_without_factory_warned {
                                ctx.aac_without_factory_warned = true;
                                tracing::warn!(
                                    %broadcast,
                                    "whep: built without `aac-opus` feature; dropping AAC audio samples",
                                );
                            }
                        }
                    }
                    Some(SessionMsg::AudioConfig { config_bytes, sample_rate, channels, object_type }) => {
                        #[cfg(feature = "aac-opus")]
                        {
                            ctx.aac_config = Some(AacAudioConfig {
                                asc: config_bytes.clone(),
                                sample_rate,
                                channels,
                                object_type,
                            });
                            tracing::debug!(
                                %broadcast,
                                sample_rate,
                                channels,
                                object_type,
                                asc_len = config_bytes.len(),
                                "whep: AAC AudioSpecificConfig received",
                            );
                        }
                        #[cfg(not(feature = "aac-opus"))]
                        {
                            let _ = (config_bytes, sample_rate, channels, object_type);
                        }
                    }
                    None => {
                        // All senders dropped (handle dropped). The
                        // shutdown oneshot also fires in that case but
                        // arrives via a separate arm; receiving None
                        // here just means the ingest side has gone
                        // away and we can coalesce onto shutdown.
                        tracing::debug!(%broadcast, "sample channel closed");
                        return;
                    }
                }
            }
            opus = opus_rx.recv() => {
                match opus {
                    Some(OpusMsg { payload, duration_ticks }) => {
                        // Session 113: write each Opus frame from
                        // the AAC-to-Opus encoder through the
                        // negotiated Opus Pt. The encoder runs on
                        // its own GStreamer worker thread and
                        // emits frames asynchronously; the poll
                        // loop owns the outbound wire and the SLO
                        // record guard.
                        if ctx.connected
                            && let (Some(mid), Some(pt)) = (ctx.audio_mid, ctx.audio_pt_opus)
                        {
                            let ok = write_opus_frame(&mut rtc, &mut ctx, &broadcast, mid, pt, payload, duration_ticks);
                            if ok
                                && let Some(tracker) = slo.as_ref()
                                && ctx.last_aac_ingest_ms > 0
                            {
                                let latency = now_unix_ms().saturating_sub(ctx.last_aac_ingest_ms);
                                tracker.record(&broadcast, TRANSPORT_LABEL, latency);
                            }
                        }
                    }
                    None => {
                        // opus_tx was dropped; this only happens
                        // when the encoder's appsink callback loses
                        // the tx clone on teardown. The outer
                        // samples channel drives real shutdown.
                    }
                }
            }
            recv = socket.recv_from(&mut buf) => {
                match recv {
                    Ok((n, source)) => {
                        let datagram = match (&buf[..n]).try_into() {
                            Ok(d) => d,
                            Err(e) => {
                                tracing::trace!(%broadcast, error = ?e, "unparseable datagram, skipping");
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
                            tracing::warn!(%broadcast, error = %e, "rtc handle_input(receive) failed");
                            return;
                        }
                    }
                    Err(e) => {
                        tracing::warn!(%broadcast, error = %e, "udp recv_from failed");
                        return;
                    }
                }
            }
            _ = tokio::time::sleep(sleep_dur) => {
                if let Err(e) = rtc.handle_input(Input::Timeout(Instant::now())) {
                    tracing::warn!(%broadcast, error = %e, "rtc handle_input(timeout) failed");
                    return;
                }
            }
        }
    }
}

fn absorb_event(event: &Event, ctx: &mut SessionCtx, broadcast: &str) {
    match event {
        Event::IceConnectionStateChange(state) => {
            tracing::debug!(%broadcast, ?state, "ice state change");
        }
        Event::Connected => {
            ctx.connected = true;
            tracing::info!(%broadcast, "webrtc connected");
        }
        Event::MediaAdded(added) if matches!(added.kind, MediaKind::Video) => {
            ctx.video_mid = Some(added.mid);
            tracing::debug!(%broadcast, mid = ?added.mid, "media added: video");
        }
        Event::MediaAdded(added) if matches!(added.kind, MediaKind::Audio) => {
            ctx.audio_mid = Some(added.mid);
            tracing::debug!(%broadcast, mid = ?added.mid, "media added: audio");
        }
        Event::MediaAdded(added) => {
            tracing::trace!(%broadcast, mid = ?added.mid, kind = ?added.kind, "media added: other");
        }
        _ => {}
    }
}

/// Write one sample (video or audio) through the negotiated
/// `str0m::Writer`. Session 30 generalised this from the old
/// `write_video_sample` so Opus audio can flow through the same
/// poll loop that H.264 / H.265 video already uses.
///
/// Video codecs go through `avcc_to_annex_b` because str0m's
/// H.264 / H.265 packetizers scan for Annex B start codes. Audio
/// codecs (Opus) are opaque payloads: str0m's Opus packetizer
/// emits one RTP packet per `Writer::write` call without framing
/// inspection, so the AVCC buffer we received from the bridge
/// passes through unchanged.
///
/// Session 110 B widened the return type: `Ok(true)` when str0m
/// accepted the write and an RTP packet will go out the wire,
/// `Ok(false)` when the sample was deliberately dropped
/// (pre-Connected, codec mismatch, AAC into an Opus-only session,
/// missing mid/pt, empty Annex B), `Err(())` when str0m surfaced
/// a write error. Only the `Ok(true)` branch records an SLO
/// sample; the rest are pre-wire drops and would bias the
/// histogram toward zero-ish samples.
fn write_sample(
    rtc: &mut Rtc,
    ctx: &mut SessionCtx,
    broadcast: &str,
    payload: Bytes,
    dts: u64,
    _keyframe: bool,
    codec: MediaCodec,
) -> Result<bool, ()> {
    if !ctx.connected {
        return Ok(false);
    }

    // Route to the matching mid + pt + clock domain based on
    // the incoming sample's codec tag.
    let (mid, pt, rtp_freq) = match codec {
        MediaCodec::H264 => (ctx.video_mid, ctx.video_pt_h264, str0m::media::Frequency::NINETY_KHZ),
        MediaCodec::H265 => (ctx.video_mid, ctx.video_pt_h265, str0m::media::Frequency::NINETY_KHZ),
        MediaCodec::Opus => (
            ctx.audio_mid,
            ctx.audio_pt_opus,
            str0m::media::Frequency::FORTY_EIGHT_KHZ,
        ),
        MediaCodec::Aac => {
            // RTMP ingest is AAC-only. There's no in-tree
            // AAC -> Opus transcoder, so WHEP drops these
            // samples here. Warn once per session so a
            // misconfigured RTMP-to-WHEP flow is obvious in
            // the logs without spamming. The Prometheus
            // counter increments unconditionally so the drop
            // rate is visible to dashboards even after the
            // one-shot warn fires.
            if !ctx.unmatched_codec_logged {
                ctx.unmatched_codec_logged = true;
                tracing::warn!(
                    %broadcast,
                    "whep: AAC audio publisher -> Opus-only subscriber surface; dropping audio (no transcoder)",
                );
            }
            metrics::counter!(
                "lvqr_whep_codec_mismatch_drops_total",
                "broadcast" => broadcast.to_string(),
                "codec" => "aac",
                "reason" => "no_transcoder",
            )
            .increment(1);
            return Ok(false);
        }
    };
    let Some(mid) = mid else {
        return Ok(false);
    };
    let Some(pt) = pt else {
        if !ctx.unmatched_codec_logged {
            ctx.unmatched_codec_logged = true;
            tracing::warn!(
                %broadcast,
                ?codec,
                "whep: publisher codec not present in subscriber offer; dropping samples"
            );
        }
        // Same observability story as the AAC branch above: the warn
        // is one-shot, but every dropped sample increments the
        // counter so a stream that mismatches throughout its lifetime
        // surfaces as a non-zero rate on the dashboard.
        let codec_label = match codec {
            MediaCodec::H264 => "h264",
            MediaCodec::H265 => "h265",
            MediaCodec::Aac => "aac",
            MediaCodec::Opus => "opus",
        };
        metrics::counter!(
            "lvqr_whep_codec_mismatch_drops_total",
            "broadcast" => broadcast.to_string(),
            "codec" => codec_label,
            "reason" => "no_subscriber_pt",
        )
        .increment(1);
        return Ok(false);
    };

    // Video codecs need AVCC -> Annex B. Audio codecs pass
    // through unchanged (Opus bytes are opaque).
    let bytes: Vec<u8> = match codec {
        MediaCodec::H264 | MediaCodec::H265 => {
            let annex_b = avcc_to_annex_b(&payload);
            if annex_b.is_empty() {
                tracing::trace!(%broadcast, "avcc->annex_b produced empty output; dropping sample");
                return Ok(false);
            }
            annex_b
        }
        MediaCodec::Opus => payload.to_vec(),
        MediaCodec::Aac => unreachable!("AAC handled above"),
    };

    let Some(writer) = rtc.writer(mid) else {
        return Ok(false);
    };

    let wallclock = Instant::now();
    let rtp_time = MediaTime::new(dts, rtp_freq);
    match writer.write(pt, wallclock, rtp_time, bytes) {
        Ok(()) => {
            if !ctx.first_write_logged {
                ctx.first_write_logged = true;
                tracing::info!(%broadcast, ?codec, dts, "first sample written to str0m");
            }
            Ok(true)
        }
        Err(e) => {
            if !ctx.write_error_logged {
                ctx.write_error_logged = true;
                tracing::warn!(%broadcast, error = %e, "writer.write failed (logging once)");
            }
            Err(())
        }
    }
}

/// Write one Opus packet through the negotiated Opus writer.
/// Returns `true` when str0m accepted the packet (an RTP packet
/// went out the wire), `false` when str0m rejected it or the
/// session is not in a writable state.
fn write_opus_frame(
    rtc: &mut Rtc,
    ctx: &mut SessionCtx,
    broadcast: &str,
    mid: Mid,
    pt: Pt,
    payload: Bytes,
    duration_ticks: u32,
) -> bool {
    let Some(writer) = rtc.writer(mid) else {
        return false;
    };
    let wallclock = Instant::now();
    let rtp_time = MediaTime::new(ctx.opus_write_dts, str0m::media::Frequency::FORTY_EIGHT_KHZ);
    match writer.write(pt, wallclock, rtp_time, payload.to_vec()) {
        Ok(()) => {
            ctx.opus_write_dts = ctx.opus_write_dts.wrapping_add(duration_ticks as u64);
            if !ctx.first_write_logged {
                ctx.first_write_logged = true;
                tracing::info!(%broadcast, "first Opus frame written to str0m");
            }
            true
        }
        Err(e) => {
            if !ctx.write_error_logged {
                ctx.write_error_logged = true;
                tracing::warn!(%broadcast, error = %e, "writer.write(opus) failed (logging once)");
            }
            false
        }
    }
}

/// Adapter that turns the poll loop's [`OpusMsg`] channel sender
/// into the [`lvqr_transcode::OpusFrame`] sender that
/// [`AacToOpusEncoderFactory::build`] expects. The encoder does not
/// know about the poll loop's internal message shape; this adapter
/// spawns a small relay task that translates between the two types.
#[cfg(feature = "aac-opus")]
fn opus_frame_sender_bridge(out_tx: mpsc::UnboundedSender<OpusMsg>) -> mpsc::UnboundedSender<OpusFrame> {
    let (in_tx, mut in_rx) = mpsc::unbounded_channel::<OpusFrame>();
    tokio::spawn(async move {
        while let Some(frame) = in_rx.recv().await {
            let msg = OpusMsg {
                payload: frame.payload,
                duration_ticks: frame.duration_ticks,
            };
            if out_tx.send(msg).is_err() {
                // Session poll loop ended; drop remaining.
                break;
            }
        }
    });
    in_tx
}

/// Convert an AVCC length-prefixed NAL sequence into an Annex B
/// byte stream.
///
/// str0m's `H264Packetizer` scans for Annex B start codes
/// (`0x00 0x00 0x01` / `0x00 0x00 0x00 0x01`) to split the input
/// into NAL units. AVCC passes through the start-code scanner
/// without matching anything, which sends the whole buffer
/// (including the 4-byte length prefix) into the emit path, where
/// the length prefix is misread as a NAL header byte of type 0 and
/// the sample is silently dropped. We convert at the boundary so
/// str0m sees what it expects.
///
/// Malformed AVCC entries (truncated, zero-length, length field
/// overruns the buffer) are skipped; the converter is safe to call
/// on arbitrary attacker-shaped input. Returns an empty `Vec` when
/// nothing survives.
fn avcc_to_annex_b(avcc: &[u8]) -> Vec<u8> {
    const START_CODE: [u8; 4] = [0x00, 0x00, 0x00, 0x01];
    let mut out = Vec::with_capacity(avcc.len() + 16);
    let mut i = 0;
    while i + 4 <= avcc.len() {
        let len = u32::from_be_bytes([avcc[i], avcc[i + 1], avcc[i + 2], avcc[i + 3]]) as usize;
        i += 4;
        if len == 0 {
            continue;
        }
        if i + len > avcc.len() {
            break;
        }
        out.extend_from_slice(&START_CODE);
        out.extend_from_slice(&avcc[i..i + len]);
        i += len;
    }
    out
}

/// Per-session handle produced by [`Str0mAnswerer::create_session`].
///
/// Owns the sample `mpsc::UnboundedSender` and the shutdown
/// `oneshot::Sender`. The poll task owns the corresponding
/// receivers. Dropping the handle drops both senders; the task's
/// `select!` sees either the shutdown resolve or the sample
/// channel return `None` and exits cleanly on the next wakeup.
///
/// Two warn-once flags ride alongside the senders. Trickle ICE
/// ingestion is still TODO, and unknown track kinds (anything
/// outside the negotiated H.264 / HEVC video + Opus audio set)
/// are dropped with a one-shot warn. Each flag fires once per
/// session so a wedged stream cannot drown the tracing output.
/// AAC publishers reach Opus-negotiated subscribers via the
/// `aac-opus` feature's [`lvqr_transcode::AacToOpusEncoder`]
/// (session 113); the audio path is wired when that feature is on.
pub struct Str0mSessionHandle {
    samples: mpsc::UnboundedSender<SessionMsg>,
    shutdown: Option<oneshot::Sender<()>>,
    trickle_warned: AtomicBool,
    unknown_track_warned: AtomicBool,
}

impl Drop for Str0mSessionHandle {
    fn drop(&mut self) {
        if let Some(tx) = self.shutdown.take() {
            // Ignore send failure: if the receiver already dropped,
            // the task has already exited, which is exactly the
            // state we were trying to reach.
            let _ = tx.send(());
        }
    }
}

impl SessionHandle for Str0mSessionHandle {
    fn add_trickle(&self, _sdp_fragment: &[u8]) -> Result<(), WhepError> {
        if !self.trickle_warned.swap(true, Ordering::Relaxed) {
            tracing::warn!("str0m trickle ICE not yet wired; ignoring fragment");
        }
        Ok(())
    }

    fn on_raw_sample(&self, track: &str, codec: MediaCodec, sample: &RawSample, ingest_time_ms: u64) {
        // Track convention matches `lvqr-ingest::RawSampleObserver`:
        // `0.mp4` is video, `1.mp4` is audio. Anything else is a
        // future track slot we do not know how to write yet.
        // Session 30 removed the old hard-drop on non-video
        // tracks; the codec tag is now authoritative and
        // `write_sample` routes audio through the Opus mid.
        if track != "0.mp4" && track != "1.mp4" {
            if !self.unknown_track_warned.swap(true, Ordering::Relaxed) {
                tracing::warn!(
                    track = %track,
                    "whep unknown-track write; dropping samples (only 0.mp4/1.mp4 wired)",
                );
            }
            return;
        }
        // Session 113: route AAC audio samples to the transcoder
        // pipeline instead of the video writer. The poll loop
        // decides whether the encoder exists (factory wired + ASC
        // known) and either pushes through opusenc or drops with a
        // one-shot warn. Non-AAC codecs (H.264, H.265, Opus) stay
        // on the `Video` path; `write_sample` routes them to the
        // matching Mid via the codec tag.
        let msg = if matches!(codec, MediaCodec::Aac) {
            SessionMsg::Aac {
                payload: sample.payload.clone(),
                dts: sample.dts,
                ingest_time_ms,
            }
        } else {
            SessionMsg::Video {
                payload: sample.payload.clone(),
                dts: sample.dts,
                keyframe: sample.keyframe,
                codec,
                ingest_time_ms,
            }
        };
        // Ignore send failure: the task has exited and we will be
        // dropped soon. Nothing useful for the caller to do.
        let _ = self.samples.send(msg);
    }

    fn on_audio_config(&self, _track: &str, codec: MediaCodec, codec_config: &[u8]) {
        // Session 113: only AAC carries a meaningful config today.
        // Non-AAC codecs either pass through (Opus) or have not
        // wired an on_audio_config path (HEVC's hvcC lands only as
        // a future session).
        if !matches!(codec, MediaCodec::Aac) {
            return;
        }
        let Some((object_type, sample_rate, channels)) = parse_aac_asc(codec_config) else {
            tracing::warn!(
                len = codec_config.len(),
                "whep: AAC AudioSpecificConfig too short; dropping",
            );
            return;
        };
        let msg = SessionMsg::AudioConfig {
            config_bytes: Bytes::copy_from_slice(codec_config),
            sample_rate,
            channels,
            object_type,
        };
        let _ = self.samples.send(msg);
    }
}

/// Parse an AAC AudioSpecificConfig into `(object_type, sample_rate_hz, channels)`.
///
/// ISO/IEC 14496-3 section 1.6.2.1 layout:
/// * 5 bits: `audioObjectType`
/// * 4 bits: `samplingFrequencyIndex`
/// * 4 bits: `channelConfiguration` (when the frequency index is not 15)
///
/// Escape path (`samplingFrequencyIndex == 15`) is not handled today;
/// FLV AAC-LC publishers never produce it because the FLV header
/// canonicalises the sample rate via the ASC table. The function
/// returns `None` when the input is shorter than 2 bytes.
fn parse_aac_asc(bytes: &[u8]) -> Option<(u8, u32, u8)> {
    if bytes.len() < 2 {
        return None;
    }
    let b0 = bytes[0];
    let b1 = bytes[1];
    let object_type = b0 >> 3;
    let freq_idx = ((b0 & 0x07) << 1) | (b1 >> 7);
    let channels = (b1 >> 3) & 0x0F;
    let sample_rate = aac_freq_index_to_hz(freq_idx);
    Some((object_type, sample_rate, channels))
}

/// ISO/IEC 14496-3 Table 1.16: AAC samplingFrequencyIndex -> Hz.
fn aac_freq_index_to_hz(idx: u8) -> u32 {
    const RATES: [u32; 13] = [
        96_000, 88_200, 64_000, 48_000, 44_100, 32_000, 24_000, 22_050, 16_000, 12_000, 11_025, 8_000, 7_350,
    ];
    RATES.get(idx as usize).copied().unwrap_or(44_100)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Chrome-shaped audio-only offer captured from str0m's own
    /// parser test at `src/sdp/parser.rs:parse_offer_sdp_chrome`.
    /// Audio is enough to exercise the whole offer -> answer path
    /// without needing to construct a video section by hand; the
    /// point of this test is to prove the module wires up and
    /// returns a structurally valid answer, not to exercise H264.
    const CHROME_AUDIO_OFFER: &str = "v=0\r\n\
        o=- 5058682828002148772 3 IN IP4 127.0.0.1\r\n\
        s=-\r\n\
        t=0 0\r\n\
        a=group:BUNDLE 0\r\n\
        a=msid-semantic: WMS 5UUdwiuY7OML2EkQtF38pJtNP5v7In1LhjEK\r\n\
        m=audio 9 UDP/TLS/RTP/SAVPF 111 103 104 9 0 8 106 105 13 110 112 113 126\r\n\
        c=IN IP4 0.0.0.0\r\n\
        a=rtcp:9 IN IP4 0.0.0.0\r\n\
        a=ice-ufrag:S5hk\r\n\
        a=ice-pwd:0zV/Yu3y8aDzbHgqWhnVQhqP\r\n\
        a=ice-options:trickle\r\n\
        a=fingerprint:sha-256 8C:64:ED:03:76:D0:3D:B4:88:08:91:64:08:80:A8:C6:5A:BF:8B:4E:38:27:96:CA:08:49:25:73:46:60:20:DC\r\n\
        a=setup:actpass\r\n\
        a=mid:0\r\n\
        a=extmap:1 urn:ietf:params:rtp-hdrext:ssrc-audio-level\r\n\
        a=extmap:2 http://www.webrtc.org/experiments/rtp-hdrext/abs-send-time\r\n\
        a=extmap:3 http://www.ietf.org/id/draft-holmer-rmcat-transport-wide-cc-extensions-01\r\n\
        a=extmap:4 urn:ietf:params:rtp-hdrext:sdes:mid\r\n\
        a=extmap:5 urn:ietf:params:rtp-hdrext:sdes:rtp-stream-id\r\n\
        a=extmap:6 urn:ietf:params:rtp-hdrext:sdes:repaired-rtp-stream-id\r\n\
        a=sendrecv\r\n\
        a=msid:5UUdwiuY7OML2EkQtF38pJtNP5v7In1LhjEK f78dde68-7055-4e20-bb37-433803dd1ed1\r\n\
        a=rtcp-mux\r\n\
        a=rtpmap:111 opus/48000/2\r\n\
        a=rtcp-fb:111 transport-cc\r\n\
        a=fmtp:111 minptime=10;useinbandfec=1\r\n\
        a=rtpmap:103 ISAC/16000\r\n\
        a=rtpmap:104 ISAC/32000\r\n\
        a=rtpmap:9 G722/8000\r\n\
        a=rtpmap:0 PCMU/8000\r\n\
        a=rtpmap:8 PCMA/8000\r\n\
        a=rtpmap:106 CN/32000\r\n\
        a=rtpmap:105 CN/16000\r\n\
        a=rtpmap:13 CN/8000\r\n\
        a=rtpmap:110 telephone-event/48000\r\n\
        a=rtpmap:112 telephone-event/32000\r\n\
        a=rtpmap:113 telephone-event/16000\r\n\
        a=rtpmap:126 telephone-event/8000\r\n\
        a=ssrc:3948621874 cname:xeXs3aE9AOBn00yJ\r\n\
        a=ssrc:3948621874 msid:5UUdwiuY7OML2EkQtF38pJtNP5v7In1LhjEK f78dde68-7055-4e20-bb37-433803dd1ed1\r\n\
        a=ssrc:3948621874 mslabel:5UUdwiuY7OML2EkQtF38pJtNP5v7In1LhjEK\r\n\
        a=ssrc:3948621874 label:f78dde68-7055-4e20-bb37-433803dd1ed1\r\n\
        ";

    #[tokio::test]
    async fn accepts_chrome_audio_offer_and_returns_parseable_answer() {
        let answerer = Str0mAnswerer::new(Str0mConfig::default());
        let (handle, answer) = answerer
            .create_session("live/test", CHROME_AUDIO_OFFER.as_bytes())
            .expect("chrome audio offer should be accepted by str0m");

        let answer_text = std::str::from_utf8(&answer).expect("answer is utf8");
        assert!(
            answer_text.starts_with("v=0"),
            "answer must be a valid SDP: {answer_text}"
        );
        assert!(
            answer_text.contains("m=audio"),
            "answer must contain an audio media section: {answer_text}"
        );
        // str0m must advertise a host candidate for the socket we bound.
        assert!(
            answer_text.contains("a=candidate:"),
            "answer must contain at least one ICE candidate: {answer_text}"
        );
        // Round-trip the answer through str0m's own parser as an
        // independent sanity check.
        SdpAnswer::from_sdp_string(answer_text).expect("answer must re-parse as SDP");

        // Placeholder methods must not error or panic; they log once
        // and return success.
        handle
            .add_trickle(b"a=candidate:0 1 udp 1 127.0.0.1 9 typ host\r\n")
            .unwrap();
        handle.add_trickle(b"more").unwrap();

        // Give the spawned poll task a tick to move past its first
        // poll_output drain. We are not asserting a specific state
        // machine progression here (no real peer is ever going to
        // complete ICE inside the test), just that the task is
        // alive and the shutdown path fires cleanly when we drop
        // the handle at the end of the scope.
        tokio::time::sleep(Duration::from_millis(20)).await;
        drop(handle);
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    fn expect_err(result: Result<(Box<dyn SessionHandle>, Bytes), WhepError>) -> WhepError {
        match result {
            Ok(_) => panic!("expected create_session to fail"),
            Err(e) => e,
        }
    }

    #[tokio::test]
    async fn rejects_non_utf8_offer() {
        let answerer = Str0mAnswerer::new(Str0mConfig::default());
        let err = expect_err(answerer.create_session("live/test", &[0xff, 0xfe, 0xfd]));
        assert!(matches!(err, WhepError::MalformedOffer(_)), "got {err:?}");
    }

    #[tokio::test]
    async fn rejects_malformed_sdp() {
        let answerer = Str0mAnswerer::new(Str0mConfig::default());
        let err = expect_err(answerer.create_session("live/test", b"not an sdp document"));
        assert!(matches!(err, WhepError::MalformedOffer(_)), "got {err:?}");
    }

    // ---- avcc_to_annex_b ----

    fn avcc_buf(nals: &[&[u8]]) -> Vec<u8> {
        let mut buf = Vec::new();
        for nal in nals {
            buf.extend_from_slice(&(nal.len() as u32).to_be_bytes());
            buf.extend_from_slice(nal);
        }
        buf
    }

    #[test]
    fn avcc_to_annex_b_single_nal_emits_start_code_and_body() {
        let nal: &[u8] = &[0x65, 0xAA, 0xBB, 0xCC];
        let out = avcc_to_annex_b(&avcc_buf(&[nal]));
        assert_eq!(out, vec![0x00, 0x00, 0x00, 0x01, 0x65, 0xAA, 0xBB, 0xCC]);
    }

    #[test]
    fn avcc_to_annex_b_multiple_nals_emits_one_start_code_each() {
        let a: &[u8] = &[0x67, 0x01];
        let b: &[u8] = &[0x68, 0x02];
        let c: &[u8] = &[0x65, 0x03, 0x04];
        let out = avcc_to_annex_b(&avcc_buf(&[a, b, c]));
        assert_eq!(
            out,
            vec![
                0x00, 0x00, 0x00, 0x01, 0x67, 0x01, 0x00, 0x00, 0x00, 0x01, 0x68, 0x02, 0x00, 0x00, 0x00, 0x01, 0x65,
                0x03, 0x04,
            ]
        );
    }

    #[test]
    fn avcc_to_annex_b_empty_input() {
        assert!(avcc_to_annex_b(&[]).is_empty());
    }

    #[test]
    fn avcc_to_annex_b_truncated_length_is_skipped() {
        // 3 bytes is less than a 4-byte length prefix.
        assert!(avcc_to_annex_b(&[0, 0, 0]).is_empty());
    }

    #[test]
    fn avcc_to_annex_b_length_overruns_buffer() {
        // length = 1000, body is only 3 bytes.
        let bad = vec![0x00, 0x00, 0x03, 0xE8, 0x01, 0x02, 0x03];
        assert!(avcc_to_annex_b(&bad).is_empty());
    }

    #[test]
    fn avcc_to_annex_b_zero_length_nal_is_skipped() {
        let mut buf = vec![0, 0, 0, 0]; // zero-length entry
        let real: &[u8] = &[0x65, 1, 2, 3];
        buf.extend_from_slice(&(real.len() as u32).to_be_bytes());
        buf.extend_from_slice(real);
        let out = avcc_to_annex_b(&buf);
        assert_eq!(out, vec![0x00, 0x00, 0x00, 0x01, 0x65, 1, 2, 3]);
    }

    // ---- end-to-end: on_raw_sample pushes through the channel ----

    // Tier 4 item 4.7 session 110 B: negative assertion that the
    // WHEP SLO tracker does NOT record a sample when str0m has not
    // yet reached `Event::Connected`. `write_sample` short-circuits
    // on `!ctx.connected` with `Ok(false)` so the poll loop's
    // recording arm is skipped; without this guard the histogram
    // would see a burst of near-zero samples during every session's
    // ICE + DTLS warm-up. The positive path (Connected -> successful
    // writer.write -> record under `transport="whep"`) is covered
    // by the `e2e_str0m_loopback*.rs` integration tests, which run
    // against a real DTLS-completed peer via str0m's test harness.
    #[tokio::test]
    async fn slo_tracker_skips_pre_connected_writes() {
        use bytes::Bytes as B;
        let tracker = LatencyTracker::new();
        let answerer = Str0mAnswerer::new(Str0mConfig::default()).with_slo_tracker(tracker.clone());
        let (handle, _answer) = answerer
            .create_session("live/test", CHROME_AUDIO_OFFER.as_bytes())
            .expect("chrome offer accepted");

        let avcc_video = avcc_buf(&[&[0x65, 0xAA, 0xBB, 0xCC][..]]);
        let sample = RawSample {
            track_id: 1,
            dts: 1000,
            cts_offset: 0,
            duration: 3000,
            payload: B::from(avcc_video),
            keyframe: true,
        };
        let backdated = now_unix_ms().saturating_sub(200);
        handle.on_raw_sample("0.mp4", MediaCodec::H264, &sample, backdated);
        handle.on_raw_sample("0.mp4", MediaCodec::H264, &sample, backdated);
        handle.on_raw_sample("0.mp4", MediaCodec::H264, &sample, backdated);

        // The poll loop reads SessionMsg::Video off the mpsc, calls
        // write_sample (returns Ok(false) because connected is
        // never set without a real peer), and skips the record
        // arm. No sample must land under `transport="whep"`.
        tokio::time::sleep(Duration::from_millis(80)).await;
        drop(handle);
        tokio::time::sleep(Duration::from_millis(20)).await;

        let snap = tracker.snapshot();
        assert!(
            snap.iter().all(|e| e.transport != TRANSPORT_LABEL),
            "pre-Connected writes must not record SLO samples: {snap:?}",
        );
    }

    #[tokio::test]
    async fn on_raw_sample_forwards_video_and_drops_audio() {
        use bytes::Bytes as B;
        let answerer = Str0mAnswerer::new(Str0mConfig::default());
        let (handle, _answer) = answerer
            .create_session("live/test", CHROME_AUDIO_OFFER.as_bytes())
            .expect("chrome offer accepted");

        // Build a minimal RawSample: a single AVCC-wrapped NAL. The
        // poll task will attempt to route this as video; without a
        // real peer it will never reach Event::Connected, so the
        // write path short-circuits on `connected == false` and we
        // are just asserting `on_raw_sample` does not panic, does
        // not block, and the audio path logs rather than sending.
        let avcc_video = avcc_buf(&[&[0x65, 0xAA, 0xBB, 0xCC][..]]);
        let sample = RawSample {
            track_id: 1,
            dts: 1000,
            cts_offset: 0,
            duration: 3000,
            payload: B::from(avcc_video),
            keyframe: true,
        };
        handle.on_raw_sample("0.mp4", MediaCodec::H264, &sample, 0); // video path
        handle.on_raw_sample("1.mp4", MediaCodec::H264, &sample, 0); // audio path, warn-once
        handle.on_raw_sample("1.mp4", MediaCodec::H264, &sample, 0); // audio path, already warned

        // Give the poll task a beat to absorb the sample. No assert:
        // the point is that none of the above panic, and the
        // subsequent handle drop still shuts the task down cleanly.
        tokio::time::sleep(Duration::from_millis(20)).await;
        drop(handle);
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    // ---- Session 113: AAC AudioSpecificConfig parsing ----

    #[test]
    fn parse_aac_asc_aac_lc_48khz_stereo() {
        // AAC-LC (object_type=2), freq_idx=3 (48 kHz), channels=2.
        // Packed into 2 bytes: 0001_0001 1001_0000 = (0x11, 0x90).
        let asc = [0x11u8, 0x90];
        let parsed = parse_aac_asc(&asc).expect("2-byte ASC parses");
        assert_eq!(parsed, (2, 48_000, 2));
    }

    #[test]
    fn parse_aac_asc_aac_lc_44100_stereo_matches_flv_test_fixture() {
        // Byte layout identical to `flv_audio_seq_header` in the
        // rtmp_hls_e2e test: object_type=2, freq_idx=4 (44100),
        // channels=2. Packed: (0x12, 0x10).
        let asc = [0x12u8, 0x10];
        let parsed = parse_aac_asc(&asc).expect("2-byte ASC parses");
        assert_eq!(parsed, (2, 44_100, 2));
    }

    #[test]
    fn parse_aac_asc_returns_none_for_short_input() {
        assert!(parse_aac_asc(&[]).is_none());
        assert!(parse_aac_asc(&[0x11]).is_none());
    }

    #[test]
    fn aac_freq_index_to_hz_known_values() {
        assert_eq!(aac_freq_index_to_hz(0), 96_000);
        assert_eq!(aac_freq_index_to_hz(3), 48_000);
        assert_eq!(aac_freq_index_to_hz(4), 44_100);
        assert_eq!(aac_freq_index_to_hz(7), 22_050);
        assert_eq!(aac_freq_index_to_hz(12), 7_350);
        // Out-of-table values fall back to 44.1 kHz.
        assert_eq!(aac_freq_index_to_hz(13), 44_100);
        assert_eq!(aac_freq_index_to_hz(15), 44_100);
    }

    #[tokio::test]
    async fn on_audio_config_aac_does_not_panic_and_survives_drop() {
        // Integration-lite: call on_audio_config with a real
        // FLV-shaped AAC ASC against a live session handle. Without
        // the `aac-opus` feature the poll loop drops the
        // SessionMsg::AudioConfig on arrival; the test is about
        // proving the handle path parses the bytes + forwards the
        // message without panicking and that the session still
        // shuts down cleanly afterwards.
        let answerer = Str0mAnswerer::new(Str0mConfig::default());
        let (handle, _answer) = answerer
            .create_session("live/test", CHROME_AUDIO_OFFER.as_bytes())
            .expect("chrome offer accepted");

        // AAC-LC 44.1 kHz stereo ASC from the RTMP test fixture.
        handle.on_audio_config("1.mp4", MediaCodec::Aac, &[0x12, 0x10]);
        // Degenerate empty config is tolerated (warn-and-drop
        // inside on_audio_config).
        handle.on_audio_config("1.mp4", MediaCodec::Aac, &[]);
        // Non-AAC codec flows through the early-return branch.
        handle.on_audio_config("1.mp4", MediaCodec::Opus, &[0x00, 0x00]);

        tokio::time::sleep(Duration::from_millis(20)).await;
        drop(handle);
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}
