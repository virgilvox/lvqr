//! RTMP ingest -> MPEG-DASH HTTP egress end-to-end integration test.
//!
//! Sister test to `rtmp_hls_e2e.rs`. Where the HLS E2E drives the
//! RTMP -> Fragment -> CmafChunk -> MultiHlsServer -> axum pipeline
//! and reads back an LL-HLS media playlist, this one drives the
//! RTMP -> Fragment -> shared FragmentBroadcasterRegistry ->
//! BroadcasterDashBridge drain task -> MultiDashServer -> axum
//! pipeline. There are no mocks: a real `rml_rtmp` client publishes,
//! a real `lvqr_cli::start`-driven server drains fragments out of
//! the registry into the DASH server, and a real raw-TCP HTTP/1.1
//! client reads the `/dash/{broadcast}/manifest.mpd` and a numbered
//! `seg-video-N.m4s` off the bound DASH listener.

use lvqr_test_utils::flv::{flv_video_nalu, flv_video_seq_header};
use lvqr_test_utils::http::{HttpGetOptions, HttpResponse, http_get_with};
use lvqr_test_utils::{TestServer, TestServerConfig};
use rml_rtmp::handshake::{Handshake, HandshakeProcessResult, PeerType};
use rml_rtmp::sessions::{
    ClientSession, ClientSessionConfig, ClientSessionEvent, ClientSessionResult, PublishRequestType,
};
use rml_rtmp::time::RtmpTimestamp;
use std::net::SocketAddr;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

const TIMEOUT: Duration = Duration::from_secs(10);

async fn rtmp_client_handshake(stream: &mut TcpStream) -> Vec<u8> {
    let mut handshake = Handshake::new(PeerType::Client);
    let p0_and_p1 = handshake.generate_outbound_p0_and_p1().unwrap();
    stream.write_all(&p0_and_p1).await.unwrap();

    let mut buf = vec![0u8; 8192];
    loop {
        let n = stream.read(&mut buf).await.unwrap();
        assert!(n > 0, "server closed during handshake");
        match handshake.process_bytes(&buf[..n]).unwrap() {
            HandshakeProcessResult::InProgress { response_bytes } => {
                if !response_bytes.is_empty() {
                    stream.write_all(&response_bytes).await.unwrap();
                }
            }
            HandshakeProcessResult::Completed {
                response_bytes,
                remaining_bytes,
            } => {
                if !response_bytes.is_empty() {
                    stream.write_all(&response_bytes).await.unwrap();
                }
                return remaining_bytes;
            }
        }
    }
}

async fn send_results(stream: &mut TcpStream, results: &[ClientSessionResult]) {
    for result in results {
        if let ClientSessionResult::OutboundResponse(packet) = result {
            stream.write_all(&packet.bytes).await.unwrap();
        }
    }
}

async fn send_result(stream: &mut TcpStream, result: &ClientSessionResult) {
    if let ClientSessionResult::OutboundResponse(packet) = result {
        stream.write_all(&packet.bytes).await.unwrap();
    }
}

async fn read_until<F>(stream: &mut TcpStream, session: &mut ClientSession, predicate: F)
where
    F: Fn(&ClientSessionEvent) -> bool,
{
    let mut buf = vec![0u8; 65536];
    let deadline = tokio::time::Instant::now() + TIMEOUT;
    loop {
        let remaining = deadline - tokio::time::Instant::now();
        let n = match tokio::time::timeout(remaining, stream.read(&mut buf)).await {
            Ok(Ok(n)) if n > 0 => n,
            Ok(Ok(_)) => panic!("server closed connection unexpectedly"),
            Ok(Err(e)) => panic!("read error: {e}"),
            Err(_) => panic!("timed out waiting for expected RTMP event"),
        };
        let results = session.handle_input(&buf[..n]).unwrap();
        for result in results {
            match result {
                ClientSessionResult::OutboundResponse(packet) => {
                    stream.write_all(&packet.bytes).await.unwrap();
                }
                ClientSessionResult::RaisedEvent(ref event) => {
                    if predicate(event) {
                        return;
                    }
                }
                _ => {}
            }
        }
    }
}

async fn connect_and_publish(addr: SocketAddr, app: &str, stream_key: &str) -> (TcpStream, ClientSession) {
    let mut stream = tokio::time::timeout(TIMEOUT, TcpStream::connect(addr))
        .await
        .unwrap()
        .unwrap();
    stream.set_nodelay(true).unwrap();
    let remaining = rtmp_client_handshake(&mut stream).await;

    let config = ClientSessionConfig::new();
    let (mut session, initial_results) = ClientSession::new(config).unwrap();
    send_results(&mut stream, &initial_results).await;
    if !remaining.is_empty() {
        let results = session.handle_input(&remaining).unwrap();
        send_results(&mut stream, &results).await;
    }
    tokio::time::sleep(Duration::from_millis(50)).await;

    let connect_result = session.request_connection(app.to_string()).unwrap();
    send_result(&mut stream, &connect_result).await;
    read_until(&mut stream, &mut session, |e| {
        matches!(e, ClientSessionEvent::ConnectionRequestAccepted)
    })
    .await;

    let publish_result = session
        .request_publishing(stream_key.to_string(), PublishRequestType::Live)
        .unwrap();
    send_result(&mut stream, &publish_result).await;
    read_until(&mut stream, &mut session, |e| {
        matches!(e, ClientSessionEvent::PublishRequestAccepted)
    })
    .await;

    (stream, session)
}

async fn http_get(addr: SocketAddr, path: &str) -> HttpResponse {
    http_get_with(
        addr,
        path,
        HttpGetOptions {
            timeout: TIMEOUT,
            ..Default::default()
        },
    )
    .await
}

async fn publish_two_keyframes(addr: SocketAddr, app: &str, key: &str) -> (TcpStream, ClientSession) {
    let (mut rtmp_stream, mut session) = connect_and_publish(addr, app, key).await;

    let seq = flv_video_seq_header();
    let result = session.publish_video_data(seq, RtmpTimestamp::new(0), false).unwrap();
    send_result(&mut rtmp_stream, &result).await;

    let nalu = vec![0x00, 0x00, 0x00, 0x04, 0x65, 0x88, 0x84, 0x00];
    let kf0 = flv_video_nalu(true, 0, &nalu);
    let result = session.publish_video_data(kf0, RtmpTimestamp::new(0), false).unwrap();
    send_result(&mut rtmp_stream, &result).await;

    // Second keyframe past the 2 s segment boundary so the segmenter
    // closes the first segment into the DASH bridge observer path.
    let kf1 = flv_video_nalu(true, 0, &nalu);
    let result = session
        .publish_video_data(kf1, RtmpTimestamp::new(2100), false)
        .unwrap();
    send_result(&mut rtmp_stream, &result).await;

    (rtmp_stream, session)
}

/// Real end-to-end: one RTMP broadcast -> RtmpMoqBridge ->
/// shared FragmentBroadcasterRegistry -> BroadcasterDashBridge drain
/// task -> MultiDashServer -> axum HTTP. Verifies the
/// /dash/live/test/manifest.mpd endpoint renders a syntactically
/// plausible MPD, the init-video.m4s endpoint serves the init
/// segment with the expected ftyp prefix, and at least one numbered
/// media segment URI resolves to a non-empty moof-prefixed body.
#[tokio::test]
async fn rtmp_publish_reaches_dash_router() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("lvqr=debug")
        .with_test_writer()
        .try_init();

    let server = TestServer::start(TestServerConfig::default().with_dash())
        .await
        .expect("start TestServer with DASH");
    let rtmp_addr = server.rtmp_addr();
    let dash_addr = server.dash_addr();

    let (_rtmp_stream, _session) = publish_two_keyframes(rtmp_addr, "live", "test").await;

    // The fragment observer path is fully synchronous for DASH (no
    // task spawn per fragment), but the RTMP bridge itself hands
    // work to a tokio task when it parses each FLV tag. Give it a
    // tick to land on the MultiDashServer state before reading.
    tokio::time::sleep(Duration::from_millis(500)).await;

    let manifest = http_get(dash_addr, "/dash/live/test/manifest.mpd").await;
    assert_eq!(manifest.status, 200, "manifest GET status");
    let body = std::str::from_utf8(&manifest.body).expect("manifest body utf-8");
    eprintln!("--- manifest.mpd ---\n{body}\n--- end ---");
    assert!(body.contains("<MPD"));
    assert!(body.contains("type=\"dynamic\""));
    assert!(body.contains("<AdaptationSet id=\"0\""));
    assert!(body.contains("seg-video-$Number$.m4s"));

    let init = http_get(dash_addr, "/dash/live/test/init-video.m4s").await;
    assert_eq!(init.status, 200, "init-video GET status");
    assert!(init.body.len() >= 8, "init-video body too short");
    assert_eq!(&init.body[4..8], b"ftyp", "init-video segment did not start with ftyp");

    let seg = http_get(dash_addr, "/dash/live/test/seg-video-1.m4s").await;
    assert_eq!(seg.status, 200, "seg-video-1 GET status");
    assert!(
        seg.body.len() >= 8,
        "seg-video-1 body too short: {} bytes",
        seg.body.len()
    );
    assert_eq!(
        &seg.body[4..8],
        b"moof",
        "expected seg-video-1 to start with a moof box"
    );

    let unknown = http_get(dash_addr, "/dash/live/ghost/manifest.mpd").await;
    assert_eq!(unknown.status, 404);

    drop(_rtmp_stream);
    server.shutdown().await.expect("shutdown");
}

/// Publish two keyframes, disconnect the RTMP client, then verify the
/// DASH manifest switches from type="dynamic" to type="static". The
/// disconnect fires BroadcastStopped, which the session-41 DASH
/// finalize subscriber picks up and calls
/// MultiDashServer::finalize_broadcast.
#[tokio::test]
async fn rtmp_disconnect_produces_static_dash_manifest() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("lvqr=debug")
        .with_test_writer()
        .try_init();

    let server = TestServer::start(TestServerConfig::default().with_dash())
        .await
        .expect("start TestServer with DASH");
    let rtmp_addr = server.rtmp_addr();
    let dash_addr = server.dash_addr();

    let (rtmp_stream, session) = publish_two_keyframes(rtmp_addr, "live", "fin").await;

    tokio::time::sleep(Duration::from_millis(500)).await;

    // Before disconnect: manifest is dynamic.
    let resp = http_get(dash_addr, "/dash/live/fin/manifest.mpd").await;
    assert_eq!(resp.status, 200);
    let body = std::str::from_utf8(&resp.body).expect("utf-8");
    assert!(
        body.contains(r#"type="dynamic""#),
        "before disconnect, MPD must be dynamic:\n{body}"
    );

    // Drop the RTMP stream to trigger disconnect -> BroadcastStopped
    // -> finalize_broadcast.
    drop(rtmp_stream);
    drop(session);

    tokio::time::sleep(Duration::from_millis(500)).await;

    // After disconnect: manifest must be static.
    let resp = http_get(dash_addr, "/dash/live/fin/manifest.mpd").await;
    assert_eq!(resp.status, 200);
    let body = std::str::from_utf8(&resp.body).expect("utf-8");
    assert!(
        body.contains(r#"type="static""#),
        "after disconnect, MPD must be static:\n{body}"
    );
    assert!(
        !body.contains("minimumUpdatePeriod="),
        "after disconnect, MPD must omit minimumUpdatePeriod:\n{body}"
    );

    server.shutdown().await.expect("shutdown");
}
