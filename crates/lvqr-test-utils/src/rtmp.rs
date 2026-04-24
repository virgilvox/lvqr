//! Shared RTMP client-handshake helper for integration tests
//! (session 133, PLAN row 122-E).
//!
//! 12 integration tests historically reimplemented the same
//! `rml_rtmp::handshake::Handshake` driver loop: generate client
//! P0+P1, write them, read server P0+P1+P2, write client P2, read
//! any trailing bytes that the server slipped into the last
//! handshake packet and return them so the caller can feed them
//! into `ClientSession::handle_input` before the first RTMP chunk.
//! This module centralizes that loop.
//!
//! # Panic contract
//!
//! [`rtmp_client_handshake`] panics with a readable message on
//! connect / read / write / parse failure. Panicking is the
//! right default for integration tests because a test that sees
//! a mid-handshake error cannot meaningfully recover; surfacing
//! the panic as a test failure is the desired behavior.
//!
//! For the one test target that needs to distinguish
//! "handshake succeeded + server rejected publish" from
//! "server never accepted the connection" by inspecting an
//! `Err(std::io::Error)` (`one_token_all_protocols.rs`), the
//! test keeps its local Result-returning variant rather than
//! consuming this helper. That is a single-caller contract; no
//! `_try` variant is factored here until a second caller appears.

use rml_rtmp::handshake::{Handshake, HandshakeProcessResult, PeerType};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

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
