//! RTMP ingest -> WebSocket relay end-to-end integration test.
//!
//! This is the "one honest E2E" that Tier 0 of the roadmap demands. It runs
//! the full data path with zero mocks:
//!
//!   rml_rtmp client (real TCP) ->
//!   lvqr_ingest::RtmpServer (real RTMP handshake) ->
//!   lvqr_ingest::RtmpMoqBridge (FLV -> fMP4 remux) ->
//!   lvqr_moq::OriginProducer (real MoQ fanout) ->
//!   axum /ws/{broadcast} handler (real WebSocket upgrade) ->
//!   tokio_tungstenite client (real TCP) ->
//!   verification that an fMP4 init segment (ftyp) and a media segment (moof)
//!   arrive over the WebSocket.
//!
//! The minimal `ws_relay_session` fn below mirrors the production handler in
//! `crates/lvqr-cli/src/main.rs`. Binary crates can't be imported from test
//! code so this test reimplements the same ~40-line MoQ subscribe + frame
//! forward loop. If the production version changes, update the copy below.

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, State};
use axum::response::Response;
use axum::routing::get;
use futures::StreamExt;
use lvqr_moq::Track;
use lvqr_test_utils::flv::{flv_video_nalu, flv_video_seq_header};
use rml_rtmp::handshake::{Handshake, HandshakeProcessResult, PeerType};
use rml_rtmp::sessions::{
    ClientSession, ClientSessionConfig, ClientSessionEvent, ClientSessionResult, PublishRequestType,
};
use rml_rtmp::time::RtmpTimestamp;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::protocol::Message as WsMessage;
use tokio_util::sync::CancellationToken;

const TIMEOUT: Duration = Duration::from_secs(10);

fn find_available_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .expect("failed to bind ephemeral port")
        .local_addr()
        .unwrap()
        .port()
}

// =====================================================================
// RTMP client handshake + publish (copied from lvqr-ingest integration)
// =====================================================================

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

async fn connect_and_publish(port: u16, app: &str, stream_key: &str) -> (TcpStream, ClientSession) {
    let mut stream = tokio::time::timeout(TIMEOUT, TcpStream::connect(format!("127.0.0.1:{port}")))
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

// =====================================================================
// Minimal WS relay handler mirroring crates/lvqr-cli/src/main.rs
// =====================================================================

#[derive(Clone)]
struct TestWsState {
    origin: lvqr_moq::OriginProducer,
}

async fn ws_relay_handler(
    ws: WebSocketUpgrade,
    State(state): State<TestWsState>,
    Path(broadcast): Path<String>,
) -> Response {
    ws.on_upgrade(move |socket| ws_relay_session(socket, state, broadcast))
}

async fn ws_relay_session(mut socket: WebSocket, state: TestWsState, broadcast: String) {
    let consumer = state.origin.consume();
    let Some(bc) = consumer.consume_broadcast(&broadcast) else {
        return;
    };
    let Ok(mut video_track) = bc.subscribe_track(&Track::new("0.mp4")) else {
        return;
    };
    let cancel = CancellationToken::new();
    loop {
        let group = tokio::select! {
            res = video_track.next_group() => res,
            _ = cancel.cancelled() => return,
        };
        let mut group = match group {
            Ok(Some(g)) => g,
            _ => return,
        };
        loop {
            let frame = match group.read_frame().await {
                Ok(Some(b)) => b,
                _ => break,
            };
            let mut framed = Vec::with_capacity(1 + frame.len());
            framed.push(0u8);
            framed.extend_from_slice(&frame);
            if socket.send(Message::Binary(framed.into())).await.is_err() {
                return;
            }
        }
    }
}

// =====================================================================
// The test
// =====================================================================

/// Real end-to-end: RTMP publish -> RtmpMoqBridge -> MoQ -> axum WS relay ->
/// tungstenite WebSocket client. Verifies an fMP4 init segment (ftyp) and a
/// media segment (moof) for the video keyframe arrive over the WebSocket.
#[tokio::test]
async fn rtmp_publish_reaches_ws_subscriber_as_fmp4() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("lvqr=debug")
        .with_test_writer()
        .try_init();

    // --- RTMP bridge + server ---
    let origin = lvqr_moq::OriginProducer::new();
    let bridge = lvqr_ingest::RtmpMoqBridge::new(origin.clone());
    let rtmp_port = find_available_port();
    let rtmp_config = lvqr_ingest::RtmpConfig {
        bind_addr: ([127, 0, 0, 1], rtmp_port).into(),
    };
    let rtmp_server = bridge.create_rtmp_server(rtmp_config);
    let rtmp_cancel = CancellationToken::new();
    let rtmp_cancel_inner = rtmp_cancel.clone();
    let rtmp_handle = tokio::spawn(async move { rtmp_server.run(rtmp_cancel_inner).await });

    // --- axum WS relay server ---
    let ws_port = find_available_port();
    let ws_state = TestWsState { origin: origin.clone() };
    let ws_router = axum::Router::new()
        .route("/ws/{*broadcast}", get(ws_relay_handler))
        .with_state(ws_state);
    let ws_addr: std::net::SocketAddr = ([127, 0, 0, 1], ws_port).into();
    let ws_cancel = CancellationToken::new();
    let ws_cancel_inner = ws_cancel.clone();
    let ws_handle = tokio::spawn(async move {
        let listener = tokio::net::TcpListener::bind(ws_addr).await.unwrap();
        axum::serve(listener, ws_router)
            .with_graceful_shutdown(async move { ws_cancel_inner.cancelled().await })
            .await
            .unwrap();
    });

    // Give both listeners a moment to come up.
    tokio::time::sleep(Duration::from_millis(100)).await;

    // --- RTMP publisher: complete the publish handshake FIRST so the
    // bridge creates the MoQ broadcast before the WS subscriber connects.
    // Otherwise consume_broadcast returns None and the relay session closes
    // immediately (same behavior as the production handler).
    let (mut rtmp_stream, mut session) = connect_and_publish(rtmp_port, "live", "test").await;

    // Allow the on_publish callback to register the broadcast with the origin.
    tokio::time::sleep(Duration::from_millis(50)).await;

    // --- Now open the WebSocket subscriber. The broadcast is already
    // announced so consume_broadcast + subscribe_track will both succeed.
    let ws_url = format!("ws://127.0.0.1:{ws_port}/ws/live/test");
    let (mut ws_stream, _resp) = tokio::time::timeout(TIMEOUT, tokio_tungstenite::connect_async(&ws_url))
        .await
        .expect("ws connect timed out")
        .expect("ws connect failed");

    // Give the handler a moment to subscribe before we push media.
    tokio::time::sleep(Duration::from_millis(50)).await;

    // --- Push sequence header + keyframe. The keyframe triggers append_group,
    // which writes the init segment as frame 0 and the media segment as
    // frame 1; both should propagate to the WS subscriber.
    let seq = flv_video_seq_header();
    let result = session.publish_video_data(seq, RtmpTimestamp::new(0), false).unwrap();
    send_result(&mut rtmp_stream, &result).await;

    // SEI + IDR NALU (AVCC length-prefixed)
    let nalu = vec![0x00, 0x00, 0x00, 0x04, 0x65, 0x88, 0x84, 0x00];
    let keyframe = flv_video_nalu(true, 0, &nalu);
    let result = session
        .publish_video_data(keyframe, RtmpTimestamp::new(0), false)
        .unwrap();
    send_result(&mut rtmp_stream, &result).await;

    // --- Read frames off the WebSocket until we see an init + a media segment ---
    let mut saw_init = false;
    let mut saw_media = false;
    let deadline = tokio::time::Instant::now() + TIMEOUT;
    while (!saw_init || !saw_media) && tokio::time::Instant::now() < deadline {
        let remaining = deadline - tokio::time::Instant::now();
        let msg = match tokio::time::timeout(remaining, ws_stream.next()).await {
            Ok(Some(Ok(m))) => m,
            Ok(Some(Err(e))) => panic!("ws recv error: {e}"),
            Ok(None) => panic!("ws closed before media arrived"),
            Err(_) => panic!("timed out waiting for ws frames (init={saw_init} media={saw_media})"),
        };
        let bin = match msg {
            WsMessage::Binary(b) => b,
            _ => continue,
        };
        // Wire format: [u8 track_id][fMP4 payload]
        assert!(bin.len() > 9, "ws frame unexpectedly short: {}", bin.len());
        assert_eq!(bin[0], 0, "expected track_id=0 (video)");
        let box_type = &bin[5..9]; // box header starts at offset 1 (payload) + 4 (size) = 5
        if box_type == b"ftyp" {
            saw_init = true;
        } else if box_type == b"moof" {
            saw_media = true;
        }
    }

    assert!(saw_init, "never saw an fMP4 init segment over WebSocket");
    assert!(saw_media, "never saw an fMP4 media segment over WebSocket");

    // --- Clean shutdown ---
    drop(rtmp_stream);
    rtmp_cancel.cancel();
    ws_cancel.cancel();
    let _ = rtmp_handle.await;
    let _ = ws_handle.await;
}
