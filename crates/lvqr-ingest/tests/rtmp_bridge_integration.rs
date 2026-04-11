//! Integration test: RTMP client -> RtmpMoqBridge -> MoQ subscriber.
//!
//! Sends real RTMP handshake and publish data over TCP, then verifies
//! the bridge creates a MoQ broadcast and a subscriber receives frames.

use bytes::Bytes;
use moq_lite::Track;
use rml_rtmp::handshake::{Handshake, HandshakeProcessResult, PeerType};
use rml_rtmp::sessions::{
    ClientSession, ClientSessionConfig, ClientSessionEvent, ClientSessionResult, PublishRequestType,
};
use rml_rtmp::time::RtmpTimestamp;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

const TIMEOUT: Duration = Duration::from_secs(10);

fn find_available_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .expect("failed to bind ephemeral port")
        .local_addr()
        .unwrap()
        .port()
}

async fn rtmp_client_handshake(stream: &mut TcpStream) -> Vec<u8> {
    let mut handshake = Handshake::new(PeerType::Client);
    let p0_and_p1 = handshake
        .generate_outbound_p0_and_p1()
        .expect("client handshake generate failed");
    stream.write_all(&p0_and_p1).await.unwrap();

    let mut buf = vec![0u8; 8192];
    loop {
        let n = stream.read(&mut buf).await.unwrap();
        assert!(n > 0, "server closed during handshake");

        match handshake.process_bytes(&buf[..n]).expect("client handshake error") {
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

/// Send outbound packets from a list of session results to the stream.
async fn send_results(stream: &mut TcpStream, results: &[ClientSessionResult]) {
    for result in results {
        if let ClientSessionResult::OutboundResponse(packet) = result {
            stream.write_all(&packet.bytes).await.unwrap();
        }
    }
}

/// Send a single ClientSessionResult to the stream if it's an outbound packet.
async fn send_result(stream: &mut TcpStream, result: &ClientSessionResult) {
    if let ClientSessionResult::OutboundResponse(packet) = result {
        stream.write_all(&packet.bytes).await.unwrap();
    }
}

/// Read from the stream, process through the client session, send any outbound
/// responses, and collect events. Keeps reading until the predicate matches an event
/// or the deadline expires.
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

        let results = session.handle_input(&buf[..n]).expect("client session input error");
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

/// Connect as RTMP client, complete handshake, and establish a publish session.
async fn connect_and_publish(port: u16, app: &str, stream_key: &str) -> (TcpStream, ClientSession) {
    let mut stream = tokio::time::timeout(TIMEOUT, TcpStream::connect(format!("127.0.0.1:{port}")))
        .await
        .expect("connect timed out")
        .expect("connect failed");

    // Disable Nagle for prompt delivery of small RTMP messages
    stream.set_nodelay(true).unwrap();

    let remaining = rtmp_client_handshake(&mut stream).await;

    let config = ClientSessionConfig::new();
    let (mut session, initial_results) = ClientSession::new(config).expect("client session init failed");

    send_results(&mut stream, &initial_results).await;

    // Feed any remaining bytes from the handshake to the session
    if !remaining.is_empty() {
        let results = session.handle_input(&remaining).unwrap();
        send_results(&mut stream, &results).await;
    }

    // Small delay to let the server finish its handshake and enter the read loop
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Request connection to app
    let connect_result = session
        .request_connection(app.to_string())
        .expect("request_connection failed");
    send_result(&mut stream, &connect_result).await;

    // Read responses until ConnectionRequestAccepted (skips onBWDone etc.)
    read_until(&mut stream, &mut session, |e| {
        matches!(e, ClientSessionEvent::ConnectionRequestAccepted)
    })
    .await;

    // Request publish
    let publish_result = session
        .request_publishing(stream_key.to_string(), PublishRequestType::Live)
        .expect("request_publishing failed");
    send_result(&mut stream, &publish_result).await;

    read_until(&mut stream, &mut session, |e| {
        matches!(e, ClientSessionEvent::PublishRequestAccepted)
    })
    .await;

    (stream, session)
}

#[tokio::test]
async fn rtmp_publish_creates_moq_broadcast() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("lvqr=debug")
        .with_test_writer()
        .try_init();

    let origin = moq_lite::OriginProducer::new();
    let bridge = lvqr_ingest::RtmpMoqBridge::new(origin.clone());

    assert_eq!(bridge.active_stream_count(), 0);

    let port = find_available_port();
    let rtmp_config = lvqr_ingest::RtmpConfig {
        bind_addr: ([127, 0, 0, 1], port).into(),
    };
    let rtmp_server = bridge.create_rtmp_server(rtmp_config);
    let server_handle = tokio::spawn(async move { rtmp_server.run().await });

    tokio::time::sleep(Duration::from_millis(50)).await;

    let (mut stream, mut session) = connect_and_publish(port, "live", "test").await;

    // Bridge should show 1 active stream
    assert_eq!(bridge.active_stream_count(), 1);
    assert_eq!(bridge.stream_names(), vec!["live/test"]);

    // Send video keyframe (FLV: frame type 1, codec 7 = 0x17)
    let keyframe_data = Bytes::from(vec![0x17, 0x01, 0x00, 0x00, 0x00, 0xAA, 0xBB, 0xCC]);
    let video_result = session
        .publish_video_data(keyframe_data, RtmpTimestamp::new(0), false)
        .expect("publish_video_data failed");
    send_result(&mut stream, &video_result).await;

    // Send a delta frame
    let delta_data = Bytes::from(vec![0x27, 0x01, 0x00, 0x00, 0x00, 0xDD, 0xEE]);
    let video_result = session
        .publish_video_data(delta_data, RtmpTimestamp::new(33), false)
        .expect("publish_video_data failed");
    send_result(&mut stream, &video_result).await;

    // Give the server time to process
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Verify MoQ side has the broadcast
    let mut consumer = origin.consume();
    let (path, bc) = tokio::time::timeout(TIMEOUT, consumer.announced())
        .await
        .expect("announce timed out")
        .expect("origin closed");
    assert_eq!(path.as_str(), "live/test");
    let bc = bc.expect("expected announce, got unannounce");

    // Subscribe to video track and read the keyframe
    let mut track_sub = bc
        .subscribe_track(&Track::new("video"))
        .expect("subscribe_track failed");

    let mut group = tokio::time::timeout(TIMEOUT, track_sub.next_group())
        .await
        .expect("next_group timed out")
        .expect("next_group failed")
        .expect("video track closed");

    let frame = tokio::time::timeout(TIMEOUT, group.read_frame())
        .await
        .expect("read_frame timed out")
        .expect("read_frame failed")
        .expect("group closed");

    assert_eq!(&frame[..], &[0x17, 0x01, 0x00, 0x00, 0x00, 0xAA, 0xBB, 0xCC]);

    // Disconnect
    drop(stream);
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(bridge.active_stream_count(), 0);

    server_handle.abort();
}

#[tokio::test]
async fn rtmp_audio_data_reaches_moq() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("lvqr=debug")
        .with_test_writer()
        .try_init();

    let origin = moq_lite::OriginProducer::new();
    let bridge = lvqr_ingest::RtmpMoqBridge::new(origin.clone());

    let port = find_available_port();
    let rtmp_config = lvqr_ingest::RtmpConfig {
        bind_addr: ([127, 0, 0, 1], port).into(),
    };
    let rtmp_server = bridge.create_rtmp_server(rtmp_config);
    let server_handle = tokio::spawn(async move { rtmp_server.run().await });

    tokio::time::sleep(Duration::from_millis(50)).await;

    let (mut stream, mut session) = connect_and_publish(port, "live", "audio-test").await;

    // Send audio data (AAC: 0xAF = format 10, rate 3, size 1, type 1)
    let audio_data = Bytes::from(vec![0xAF, 0x01, 0x12, 0x34, 0x56]);
    let audio_result = session
        .publish_audio_data(audio_data, RtmpTimestamp::new(0), false)
        .expect("publish_audio_data failed");
    send_result(&mut stream, &audio_result).await;

    tokio::time::sleep(Duration::from_millis(100)).await;

    // Verify audio track has data
    let mut consumer = origin.consume();
    let (path, bc) = tokio::time::timeout(TIMEOUT, consumer.announced())
        .await
        .expect("announce timed out")
        .expect("origin closed");
    assert_eq!(path.as_str(), "live/audio-test");
    let bc = bc.expect("expected announce");

    let mut audio_track = bc
        .subscribe_track(&Track::new("audio"))
        .expect("subscribe audio track failed");

    let mut group = tokio::time::timeout(TIMEOUT, audio_track.next_group())
        .await
        .expect("audio next_group timed out")
        .expect("audio next_group failed")
        .expect("audio track closed");

    let frame = tokio::time::timeout(TIMEOUT, group.read_frame())
        .await
        .expect("audio read_frame timed out")
        .expect("audio read_frame failed")
        .expect("audio group closed");

    assert_eq!(&frame[..], &[0xAF, 0x01, 0x12, 0x34, 0x56]);

    drop(stream);
    server_handle.abort();
}
