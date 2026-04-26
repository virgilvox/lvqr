//! Shared RTMP client-side integration-test helpers.
//!
//! Sessions 133 + 134 factor the four helpers every RTMP-class
//! integration test historically reimplemented verbatim:
//!
//! * [`rtmp_client_handshake`] (session 133, PLAN row 122-E) --
//!   the `rml_rtmp::handshake::Handshake` driver loop.
//! * [`send_results`], [`send_result`], [`read_until`] (session
//!   134, PLAN row 122-F) -- the packet-write + event-loop
//!   helpers that every test's `connect_and_publish` composed.
//!
//! # Panic contract
//!
//! These helpers panic with readable messages on connect / read /
//! write / parse failure. Panicking is the right default for
//! integration tests because a test that sees a mid-RTMP error
//! cannot meaningfully recover; surfacing the panic as a test
//! failure is the desired behavior.
//!
//! For the one test target that needs to distinguish
//! "handshake succeeded + server rejected publish" from
//! "server never accepted the connection" by inspecting an
//! `Err(std::io::Error)` (`one_token_all_protocols.rs`), the
//! test keeps its local Result-returning handshake variant
//! rather than consuming [`rtmp_client_handshake`]. That is a
//! single-caller contract; no `_try` variant is factored here
//! until a second caller appears.

use std::time::Duration;

use rml_rtmp::handshake::{Handshake, HandshakeProcessResult, PeerType};
use rml_rtmp::sessions::{ClientSession, ClientSessionEvent, ClientSessionResult};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::Instant;

/// Drive the client side of the RTMP handshake on `stream` to
/// completion and return any trailing bytes the server included in
/// the last handshake packet. The returned `Vec<u8>` is whatever
/// bytes arrived after the final handshake ACK; callers must feed
/// those into `ClientSession::handle_input` before any further
/// reads, or the RTMP chunk stream falls out of sync.
///
/// Panics if any of the `rml_rtmp` state-machine calls return an
/// error, if a read returns 0 (server closed mid-handshake), or
/// if a socket write fails. Integration tests surface those
/// failures as test failures via the panic.
pub async fn rtmp_client_handshake(stream: &mut TcpStream) -> Vec<u8> {
    let mut handshake = Handshake::new(PeerType::Client);
    let p0_and_p1 = handshake
        .generate_outbound_p0_and_p1()
        .expect("rtmp_client_handshake: generate_outbound_p0_and_p1");
    stream
        .write_all(&p0_and_p1)
        .await
        .expect("rtmp_client_handshake: write P0+P1");

    let mut buf = vec![0u8; 8192];
    loop {
        let n = stream.read(&mut buf).await.expect("rtmp_client_handshake: read");
        assert!(n > 0, "rtmp_client_handshake: server closed during handshake");
        match handshake
            .process_bytes(&buf[..n])
            .expect("rtmp_client_handshake: process_bytes")
        {
            HandshakeProcessResult::InProgress { response_bytes } => {
                if !response_bytes.is_empty() {
                    stream
                        .write_all(&response_bytes)
                        .await
                        .expect("rtmp_client_handshake: write response (InProgress)");
                }
            }
            HandshakeProcessResult::Completed {
                response_bytes,
                remaining_bytes,
            } => {
                if !response_bytes.is_empty() {
                    stream
                        .write_all(&response_bytes)
                        .await
                        .expect("rtmp_client_handshake: write response (Completed)");
                }
                return remaining_bytes;
            }
        }
    }
}

/// Write every `OutboundResponse` packet in `results` to the
/// stream in order. No-ops on non-`OutboundResponse` variants.
/// Panics on a socket-write error (see module-level panic
/// contract).
pub async fn send_results(stream: &mut TcpStream, results: &[ClientSessionResult]) {
    for r in results {
        if let ClientSessionResult::OutboundResponse(packet) = r {
            stream.write_all(&packet.bytes).await.expect("send_results: write");
        }
    }
}

/// Write one `OutboundResponse` packet to the stream. No-op on
/// non-`OutboundResponse` variants. Panics on a socket-write
/// error.
pub async fn send_result(stream: &mut TcpStream, result: &ClientSessionResult) {
    if let ClientSessionResult::OutboundResponse(packet) = result {
        stream.write_all(&packet.bytes).await.expect("send_result: write");
    }
}

/// Read from `stream`, hand each chunk to
/// `ClientSession::handle_input`, write any `OutboundResponse`
/// packets the session produces back out, and return when the
/// session raises an event for which `predicate` returns true.
/// Panics on read / write / parse failure or when the deadline
/// `Instant::now() + timeout` elapses first.
///
/// The deadline is computed once at entry; the whole event wait
/// budget is `timeout`, not a per-read timeout.
///
/// Session 155: ALL `OutboundResponse` packets in the result batch
/// are written BEFORE the predicate returns -- previously this
/// helper short-circuited on the first matching event and skipped
/// later responses in the same batch, which silently dropped the
/// post-connect `SetChunkSize` packet (it follows the
/// `ConnectionRequestAccepted` event in the rml_rtmp client's
/// result vector). The skipped `SetChunkSize` left the server's
/// deserializer at the default 128-byte chunk size while the
/// client's serializer ramped to 4096, breaking subsequent
/// long-message sends (e.g. `publish_amf0_data` with a base64
/// payload).
pub async fn read_until<F>(stream: &mut TcpStream, session: &mut ClientSession, timeout: Duration, predicate: F)
where
    F: Fn(&ClientSessionEvent) -> bool,
{
    let mut buf = vec![0u8; 65536];
    let deadline = Instant::now() + timeout;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let n = match tokio::time::timeout(remaining, stream.read(&mut buf)).await {
            Ok(Ok(n)) if n > 0 => n,
            Ok(Ok(_)) => panic!("read_until: server closed connection unexpectedly"),
            Ok(Err(e)) => panic!("read_until: read error: {e}"),
            Err(_) => panic!("read_until: timed out waiting for expected RTMP event"),
        };
        let results = session
            .handle_input(&buf[..n])
            .expect("read_until: ClientSession::handle_input");
        let mut event_matched = false;
        for r in &results {
            match r {
                ClientSessionResult::OutboundResponse(packet) => {
                    stream
                        .write_all(&packet.bytes)
                        .await
                        .expect("read_until: write outbound");
                }
                ClientSessionResult::RaisedEvent(event) if predicate(event) => {
                    event_matched = true;
                }
                _ => {}
            }
        }
        if event_matched {
            return;
        }
    }
}
