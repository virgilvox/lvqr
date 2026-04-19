//! Cross-protocol authentication end-to-end test (Tier 4 item 4.8
//! session B).
//!
//! Brings up a single `TestServer` with every ingest protocol
//! enabled (RTMP, WHIP, SRT, RTSP) plus a `JwtAuthProvider`, then
//! drives each surface with three token variants:
//!
//! 1. A publish-scoped JWT bound to `live/cam1` (the happy path).
//! 2. A JWT signed with the wrong shared secret (every protocol
//!    must deny).
//! 3. A publish-scoped JWT bound to `live/other` published against
//!    `live/cam1` (WHIP/SRT/RTSP must deny because they carry the
//!    target broadcast at auth time; RTMP must accept because the
//!    stream key IS the JWT, so `extract_rtmp` passes
//!    `broadcast: None` and `JwtAuthProvider` skips the binding
//!    check).
//!
//! No mocks: the test drives `lvqr_cli::start` through `TestServer`
//! and uses real `rml_rtmp` / `srt_tokio` / raw TCP clients to hit
//! each ingest's actual deny / accept path. The matching
//! per-protocol carrier conventions are documented in
//! `docs/auth.md`.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use jsonwebtoken::{EncodingKey, Header, encode};
use lvqr_auth::{AuthScope, JwtAuthConfig, JwtAuthProvider, JwtClaims, SharedAuth};
use lvqr_test_utils::{TestServer, TestServerConfig};
use rml_rtmp::handshake::{Handshake, HandshakeProcessResult, PeerType};
use rml_rtmp::sessions::{
    ClientSession, ClientSessionConfig, ClientSessionEvent, ClientSessionResult, PublishRequestType,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

const SECRET: &str = "session-96-b-shared-secret-please-change";
const BROADCAST: &str = "live/cam1";
const OFF_BROADCAST: &str = "live/other";

const RTMP_TIMEOUT: Duration = Duration::from_secs(5);
const HTTP_TIMEOUT: Duration = Duration::from_secs(5);
const SRT_CONNECT_TIMEOUT: Duration = Duration::from_secs(3);

// =====================================================================
// JWT minting + server bring-up
// =====================================================================

fn future_exp() -> usize {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
    now as usize + 3600
}

fn mint_token(secret: &str, broadcast: Option<&str>) -> String {
    let claims = JwtClaims {
        sub: "session96b".into(),
        exp: future_exp(),
        scope: AuthScope::Publish,
        iss: None,
        aud: None,
        broadcast: broadcast.map(String::from),
    };
    encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )
    .expect("encode JWT")
}

async fn server_with_jwt() -> TestServer {
    let provider = JwtAuthProvider::new(JwtAuthConfig {
        secret: SECRET.into(),
        issuer: None,
        audience: None,
    })
    .expect("JwtAuthProvider::new");
    let auth: SharedAuth = Arc::new(provider);
    TestServer::start(
        TestServerConfig::default()
            .with_whip()
            .with_srt()
            .with_rtsp()
            .with_auth(auth),
    )
    .await
    .expect("start TestServer")
}

// =====================================================================
// RTMP: rml_rtmp publish handshake (lifted from rtmp_archive_e2e.rs)
// =====================================================================

async fn rtmp_handshake(stream: &mut TcpStream) -> std::io::Result<Vec<u8>> {
    let mut handshake = Handshake::new(PeerType::Client);
    let p0_and_p1 = handshake
        .generate_outbound_p0_and_p1()
        .map_err(|e| std::io::Error::other(format!("p0p1: {e:?}")))?;
    stream.write_all(&p0_and_p1).await?;
    let mut buf = vec![0u8; 8192];
    loop {
        let n = stream.read(&mut buf).await?;
        if n == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "server closed during RTMP handshake",
            ));
        }
        match handshake
            .process_bytes(&buf[..n])
            .map_err(|e| std::io::Error::other(format!("handshake: {e:?}")))?
        {
            HandshakeProcessResult::InProgress { response_bytes } => {
                if !response_bytes.is_empty() {
                    stream.write_all(&response_bytes).await?;
                }
            }
            HandshakeProcessResult::Completed {
                response_bytes,
                remaining_bytes,
            } => {
                if !response_bytes.is_empty() {
                    stream.write_all(&response_bytes).await?;
                }
                return Ok(remaining_bytes);
            }
        }
    }
}

async fn write_outbound(stream: &mut TcpStream, results: &[ClientSessionResult]) -> std::io::Result<()> {
    for r in results {
        if let ClientSessionResult::OutboundResponse(p) = r {
            stream.write_all(&p.bytes).await?;
        }
    }
    Ok(())
}

/// Drive the RTMP handshake + connect + publish-request flow and
/// wait briefly for `PublishRequestAccepted`. Returns `true` when
/// the publish was accepted (auth passed); `false` when the
/// server dropped the socket mid-handshake (rml_rtmp's
/// `validate_publish` callback returning `false` makes the server
/// `return Ok(())` from the connection task and close the TCP
/// stream, so the client sees an EOF read).
async fn try_rtmp_publish(addr: SocketAddr, app: &str, key: &str) -> bool {
    let attempt = async {
        let mut stream = TcpStream::connect(addr).await?;
        stream.set_nodelay(true)?;
        let remaining = rtmp_handshake(&mut stream).await?;

        let (mut session, initial) = ClientSession::new(ClientSessionConfig::new())
            .map_err(|e| std::io::Error::other(format!("client session: {e:?}")))?;
        write_outbound(&mut stream, &initial).await?;
        if !remaining.is_empty() {
            let r = session
                .handle_input(&remaining)
                .map_err(|e| std::io::Error::other(format!("handle_input: {e:?}")))?;
            write_outbound(&mut stream, &r).await?;
        }

        // Yield briefly so the server's post-handshake control
        // messages (window ack size, set peer bandwidth, onBWDone)
        // arrive before we serialise the connect command. Skipping
        // this race-condition pad makes the connect response chunks
        // arrive interleaved with the prerequisite control messages
        // and the deserializer can't reassemble them. Same wait
        // pattern as `crates/lvqr-cli/tests/rtmp_archive_e2e.rs`.
        tokio::time::sleep(Duration::from_millis(50)).await;

        let connect = session
            .request_connection(app.to_string())
            .map_err(|e| std::io::Error::other(format!("connect req: {e:?}")))?;
        write_outbound(&mut stream, std::slice::from_ref(&connect)).await?;

        let mut buf = vec![0u8; 65536];
        let mut connected = false;
        while !connected {
            let n = stream.read(&mut buf).await?;
            if n == 0 {
                return Ok::<bool, std::io::Error>(false);
            }
            let results = session
                .handle_input(&buf[..n])
                .map_err(|e| std::io::Error::other(format!("handle_input: {e:?}")))?;
            for r in results {
                match r {
                    ClientSessionResult::OutboundResponse(p) => {
                        stream.write_all(&p.bytes).await?;
                    }
                    ClientSessionResult::RaisedEvent(ClientSessionEvent::ConnectionRequestAccepted) => {
                        connected = true;
                    }
                    _ => {}
                }
            }
        }

        let publish = session
            .request_publishing(key.to_string(), PublishRequestType::Live)
            .map_err(|e| std::io::Error::other(format!("publish req: {e:?}")))?;
        write_outbound(&mut stream, std::slice::from_ref(&publish)).await?;

        loop {
            let n = stream.read(&mut buf).await?;
            if n == 0 {
                return Ok(false);
            }
            let results = session
                .handle_input(&buf[..n])
                .map_err(|e| std::io::Error::other(format!("handle_input: {e:?}")))?;
            for r in results {
                match r {
                    ClientSessionResult::OutboundResponse(p) => {
                        stream.write_all(&p.bytes).await?;
                    }
                    ClientSessionResult::RaisedEvent(ClientSessionEvent::PublishRequestAccepted) => {
                        return Ok(true);
                    }
                    _ => {}
                }
            }
        }
    };

    match tokio::time::timeout(RTMP_TIMEOUT, attempt).await {
        Ok(Ok(accepted)) => accepted,
        Ok(Err(_)) => false,
        Err(_) => false,
    }
}

// =====================================================================
// WHIP: minimal SDP POST + Bearer header
// =====================================================================

const MINIMAL_SDP_OFFER: &[u8] = b"v=0\r\nm=video 9 UDP/TLS/RTP/SAVPF 96\r\n";

async fn whip_post(addr: SocketAddr, broadcast: &str, bearer: Option<&str>) -> u16 {
    let mut stream = tokio::time::timeout(HTTP_TIMEOUT, TcpStream::connect(addr))
        .await
        .expect("WHIP connect timed out")
        .expect("WHIP connect failed");
    let auth_line = match bearer {
        Some(t) => format!("Authorization: Bearer {t}\r\n"),
        None => String::new(),
    };
    let header = format!(
        "POST /whip/{broadcast} HTTP/1.1\r\n\
         Host: {addr}\r\n\
         Content-Type: application/sdp\r\n\
         Content-Length: {len}\r\n\
         {auth_line}Connection: close\r\n\r\n",
        len = MINIMAL_SDP_OFFER.len(),
    );
    stream.write_all(header.as_bytes()).await.unwrap();
    stream.write_all(MINIMAL_SDP_OFFER).await.unwrap();
    let mut buf = Vec::new();
    tokio::time::timeout(HTTP_TIMEOUT, stream.read_to_end(&mut buf))
        .await
        .expect("WHIP read timed out")
        .expect("WHIP read failed");
    parse_status_line(&buf)
}

fn parse_status_line(raw: &[u8]) -> u16 {
    let split = raw.windows(4).position(|w| w == b"\r\n\r\n").unwrap_or(raw.len());
    let header_text = std::str::from_utf8(&raw[..split]).expect("response headers not utf-8");
    header_text
        .lines()
        .next()
        .expect("missing status line")
        .split_whitespace()
        .nth(1)
        .expect("missing status code")
        .parse()
        .expect("status code not numeric")
}

// =====================================================================
// SRT: srt-tokio caller with KV streamid payload
// =====================================================================

async fn try_srt_connect(addr: SocketAddr, streamid: &str) -> Result<(), std::io::Error> {
    let socket = srt_tokio::SrtSocket::builder()
        .set(|o| o.connect.timeout = SRT_CONNECT_TIMEOUT)
        .call(addr, Some(streamid))
        .await?;
    drop(socket);
    Ok(())
}

// =====================================================================
// RTSP: ANNOUNCE bytes-on-the-wire (no high-level client crate)
// =====================================================================

async fn rtsp_announce(addr: SocketAddr, broadcast: &str, bearer: Option<&str>) -> u16 {
    let mut stream = tokio::time::timeout(HTTP_TIMEOUT, TcpStream::connect(addr))
        .await
        .expect("RTSP connect timed out")
        .expect("RTSP connect failed");
    let sdp = "v=0\r\n\
               o=- 0 0 IN IP4 127.0.0.1\r\n\
               s=Test\r\n\
               m=video 0 RTP/AVP 96\r\n\
               a=rtpmap:96 H264/90000\r\n\
               a=control:track1\r\n";
    let auth_line = match bearer {
        Some(t) => format!("Authorization: Bearer {t}\r\n"),
        None => String::new(),
    };
    let req = format!(
        "ANNOUNCE rtsp://{addr}/{broadcast} RTSP/1.0\r\n\
         CSeq: 1\r\n\
         Content-Type: application/sdp\r\n\
         Content-Length: {len}\r\n\
         {auth_line}\r\n\
         {sdp}",
        len = sdp.len()
    );
    stream.write_all(req.as_bytes()).await.unwrap();
    let mut buf = vec![0u8; 4096];
    let n = tokio::time::timeout(HTTP_TIMEOUT, stream.read(&mut buf))
        .await
        .expect("RTSP read timed out")
        .expect("RTSP read failed");
    parse_rtsp_status(&buf[..n])
}

fn parse_rtsp_status(buf: &[u8]) -> u16 {
    let text = std::str::from_utf8(buf).expect("RTSP response not utf-8");
    text.lines()
        .next()
        .expect("missing RTSP status line")
        .split_whitespace()
        .nth(1)
        .expect("missing RTSP status code")
        .parse()
        .expect("RTSP status not numeric")
}

// =====================================================================
// Tests
// =====================================================================

/// Positive: a single publish-scoped JWT bound to `live/cam1` is
/// admitted by every ingest surface. RTMP carries the token as the
/// stream key; WHIP/SRT/RTSP carry it as a per-protocol bearer.
#[tokio::test]
async fn one_publish_jwt_admits_every_protocol() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("lvqr=debug")
        .with_test_writer()
        .try_init();

    let server = server_with_jwt().await;
    let token = mint_token(SECRET, Some(BROADCAST));

    // RTMP: stream key IS the JWT, so the broadcast on the wire is
    // `live/<jwt>` and the auth check fires on the key.
    assert!(
        try_rtmp_publish(server.rtmp_addr(), "live", &token).await,
        "RTMP publish with valid JWT must succeed"
    );

    // WHIP: 401 is the auth-failure signal. The minimal SDP body
    // here may or may not parse cleanly through str0m; if it
    // doesn't, the answerer 400s after the auth gate has already
    // fired Allow. Any non-401 is therefore proof the gate
    // admitted the token.
    let whip_status = whip_post(server.whip_addr(), BROADCAST, Some(&token)).await;
    assert_ne!(
        whip_status, 401,
        "WHIP returned 401 with a valid JWT (auth gate denied)"
    );

    // SRT: a passed gate yields a real socket; deny rejects at
    // handshake (`ServerRejectReason::Unauthorized`, code 2401)
    // surfaced through srt-tokio as `io::ErrorKind::ConnectionRefused`.
    let streamid = format!("m=publish,r={BROADCAST},t={token}");
    try_srt_connect(server.srt_addr(), &streamid)
        .await
        .expect("SRT connect with valid JWT must succeed");

    // RTSP: 200 on ANNOUNCE means the gate passed and the server
    // accepted the publish session.
    let rtsp_status = rtsp_announce(server.rtsp_addr(), BROADCAST, Some(&token)).await;
    assert_eq!(rtsp_status, 200, "RTSP ANNOUNCE with valid JWT must 200");

    server.shutdown().await.expect("shutdown");
}

/// Negative -- wrong secret. A token signed with a secret the
/// server does not know decodes to an error; every protocol must
/// reject.
#[tokio::test]
async fn wrong_secret_jwt_is_rejected_everywhere() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("lvqr=debug")
        .with_test_writer()
        .try_init();

    let server = server_with_jwt().await;
    let bad = mint_token("attacker-secret", Some(BROADCAST));

    assert!(
        !try_rtmp_publish(server.rtmp_addr(), "live", &bad).await,
        "RTMP must drop the connection on a wrong-secret JWT"
    );

    assert_eq!(
        whip_post(server.whip_addr(), BROADCAST, Some(&bad)).await,
        401,
        "WHIP must 401 a wrong-secret JWT"
    );

    let streamid = format!("m=publish,r={BROADCAST},t={bad}");
    let srt_err = try_srt_connect(server.srt_addr(), &streamid)
        .await
        .expect_err("SRT must reject a wrong-secret JWT");
    assert_eq!(
        srt_err.kind(),
        std::io::ErrorKind::ConnectionRefused,
        "SRT reject should surface as ConnectionRefused (got {srt_err:?})"
    );

    assert_eq!(
        rtsp_announce(server.rtsp_addr(), BROADCAST, Some(&bad)).await,
        401,
        "RTSP must 401 a wrong-secret JWT"
    );

    server.shutdown().await.expect("shutdown");
}

/// Negative -- wrong broadcast. A JWT bound to `live/other` is
/// rejected by WHIP / SRT / RTSP because they each carry the
/// target broadcast at auth time and the provider enforces the
/// `broadcast` claim. RTMP, by design, accepts: the stream key IS
/// the JWT, so `extract_rtmp` passes `broadcast: None` and the
/// provider skips the per-broadcast binding. This is the
/// documented anti-scope in `crates/lvqr-auth/src/extract.rs` and
/// the rationale baked into `JwtAuthProvider::check`.
#[tokio::test]
async fn wrong_broadcast_jwt_is_rejected_on_whip_srt_rtsp_only() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("lvqr=debug")
        .with_test_writer()
        .try_init();

    let server = server_with_jwt().await;
    let off = mint_token(SECRET, Some(OFF_BROADCAST));

    // RTMP admits the cross-broadcast JWT: the broadcast on the
    // wire is `app/key` = `live/<token>`, and `extract_rtmp` does
    // not pass a broadcast filter, so the provider only checks
    // scope (Publish, OK).
    assert!(
        try_rtmp_publish(server.rtmp_addr(), "live", &off).await,
        "RTMP must admit a cross-broadcast JWT (stream key carries the JWT)"
    );

    assert_eq!(
        whip_post(server.whip_addr(), BROADCAST, Some(&off)).await,
        401,
        "WHIP must 401 a JWT bound to a different broadcast"
    );

    let streamid = format!("m=publish,r={BROADCAST},t={off}");
    let srt_err = try_srt_connect(server.srt_addr(), &streamid)
        .await
        .expect_err("SRT must reject a JWT bound to a different broadcast");
    assert_eq!(
        srt_err.kind(),
        std::io::ErrorKind::ConnectionRefused,
        "SRT reject should surface as ConnectionRefused (got {srt_err:?})"
    );

    assert_eq!(
        rtsp_announce(server.rtsp_addr(), BROADCAST, Some(&off)).await,
        401,
        "RTSP must 401 a JWT bound to a different broadcast"
    );

    server.shutdown().await.expect("shutdown");
}
