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
//! What this module still deliberately does NOT do:
//!
//! * Packetize incoming `RawSample`s into RTP. The `H264Packetizer`
//!   already exists in `crate::rtp`; the next session threads it
//!   into `on_raw_sample` via an mpsc into the poll task. Until then
//!   `on_raw_sample` logs a one-shot warning and drops the sample.
//! * Accept trickle ICE fragments. WHEP typically does not need
//!   trickle once the offer already embeds every host candidate,
//!   but the HTTP surface still accepts PATCH bodies so conformant
//!   clients do not error out. `add_trickle` logs once and returns
//!   success.
//!
//! The point of landing the poll loop on its own is to prove the
//! trait boundary in `server.rs` can host a real WebRTC stack
//! without mixing the I/O cycle into the same commit as the
//! media-write path. A real browser that follows the 201 response
//! will now at least see ICE progress and, once a DTLS handshake
//! finishes, a quiet SRTP session with no media. That is a large
//! step up from "answer returned, socket dead" and the smallest
//! delta that lets session 22 focus entirely on media write.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use bytes::Bytes;
use lvqr_cmaf::RawSample;
use str0m::change::{SdpAnswer, SdpOffer};
use str0m::net::{Protocol, Receive};
use str0m::{Candidate, Event, IceConnectionState, Input, Output, Rtc, RtcConfig};
use tokio::net::UdpSocket;
use tokio::sync::oneshot;

use crate::server::{SdpAnswerer, SessionHandle, WhepError};

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
pub struct Str0mAnswerer {
    config: Str0mConfig,
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
        Self { config }
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
        let bind_addr = SocketAddr::new(self.config.host_ip, 0);
        let std_socket = std::net::UdpSocket::bind(bind_addr)
            .map_err(|e| WhepError::AnswererFailed(format!("udp bind {bind_addr} failed: {e}")))?;
        std_socket
            .set_nonblocking(true)
            .map_err(|e| WhepError::AnswererFailed(format!("set_nonblocking failed: {e}")))?;
        let local_addr = std_socket
            .local_addr()
            .map_err(|e| WhepError::AnswererFailed(format!("local_addr failed: {e}")))?;
        let socket = UdpSocket::from_std(std_socket)
            .map_err(|e| WhepError::AnswererFailed(format!("tokio from_std failed: {e}")))?;

        let mut rtc = RtcConfig::new()
            .enable_h264(true)
            .enable_opus(true)
            .build(Instant::now());

        let candidate = Candidate::host(local_addr, Protocol::Udp)
            .map_err(|e| WhepError::AnswererFailed(format!("host candidate failed: {e}")))?;
        rtc.add_local_candidate(candidate);

        let answer: SdpAnswer = rtc
            .sdp_api()
            .accept_offer(offer)
            .map_err(|e| WhepError::MalformedOffer(format!("accept_offer failed: {e}")))?;
        let answer_bytes = Bytes::from(answer.to_sdp_string());

        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
        let broadcast_owned = broadcast.to_string();

        // Spawn the sans-IO poll loop. `tokio::spawn` requires an
        // active runtime; `WhepServer` is only constructed inside
        // `lvqr-cli`'s tokio-based axum server, so this is always
        // satisfied in real deployments. Tests that hit this code
        // path must use `#[tokio::test]`.
        tokio::spawn(run_session_loop(rtc, socket, local_addr, shutdown_rx, broadcast_owned));

        tracing::debug!(
            broadcast = %broadcast,
            local = %local_addr,
            "str0m session spawned; poll loop running",
        );

        let handle: Box<dyn SessionHandle> = Box::new(Str0mSessionHandle {
            shutdown: Some(shutdown_tx),
            trickle_warned: AtomicBool::new(false),
            sample_warned: AtomicBool::new(false),
        });
        Ok((handle, answer_bytes))
    }
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
async fn run_session_loop(
    mut rtc: Rtc,
    socket: UdpSocket,
    local_addr: SocketAddr,
    mut shutdown: oneshot::Receiver<()>,
    broadcast: String,
) {
    let mut buf = vec![0u8; 2048];
    loop {
        // Drain outputs until `Rtc` blocks on a timeout.
        let wait_until = loop {
            match rtc.poll_output() {
                Ok(Output::Timeout(when)) => break when,
                Ok(Output::Transmit(transmit)) => {
                    if let Err(e) = socket.send_to(&transmit.contents, transmit.destination).await {
                        tracing::warn!(%broadcast, error = %e, dest = %transmit.destination, "udp send failed");
                    }
                }
                Ok(Output::Event(event)) => {
                    if let Event::IceConnectionStateChange(state) = &event {
                        tracing::debug!(%broadcast, ?state, "ice state change");
                        if *state == IceConnectionState::Disconnected {
                            tracing::info!(%broadcast, "ice disconnected; ending session loop");
                            return;
                        }
                    } else if matches!(event, Event::Connected) {
                        tracing::info!(%broadcast, "webrtc connected");
                    } else {
                        tracing::trace!(%broadcast, "rtc event");
                    }
                }
                Err(e) => {
                    tracing::warn!(%broadcast, error = %e, "rtc poll_output error; ending session loop");
                    return;
                }
            }
        };

        let now = Instant::now();
        let sleep_dur = wait_until.saturating_duration_since(now).max(Duration::from_millis(0));

        tokio::select! {
            biased;
            _ = &mut shutdown => {
                tracing::debug!(%broadcast, "session shutdown signalled");
                return;
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

/// Per-session handle produced by [`Str0mAnswerer::create_session`].
///
/// Owns the `oneshot` sender whose receiver the spawned poll task
/// waits on. Dropping the handle closes the sender, the receiver
/// resolves, and the loop exits at its next wakeup. The warn flags
/// keep the log quiet after the first surprise: a real WHEP client
/// sending trickle or expecting media will hit the same code path
/// thousands of times per second and we do not want that to drown
/// the tracing output.
pub struct Str0mSessionHandle {
    shutdown: Option<oneshot::Sender<()>>,
    trickle_warned: AtomicBool,
    sample_warned: AtomicBool,
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

    fn on_raw_sample(&self, _track: &str, _sample: &RawSample) {
        if !self.sample_warned.swap(true, Ordering::Relaxed) {
            tracing::warn!("str0m RTP packetization not yet wired; dropping sample");
        }
    }
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
}
