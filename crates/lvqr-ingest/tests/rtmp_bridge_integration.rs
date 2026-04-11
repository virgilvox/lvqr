//! Integration test: RTMP client -> RtmpMoqBridge -> MoQ subscriber.
//!
//! Sends real RTMP handshake and publish data over TCP, then verifies
//! the bridge creates a MoQ broadcast with CMAF-formatted tracks.

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

// -- Helpers for constructing valid FLV tag data --

/// Build an FLV AVC sequence header (video codec config).
fn flv_video_seq_header() -> Bytes {
    let sps = [0x67, 0x64, 0x00, 0x1F, 0xAC, 0xD9];
    let pps = [0x68, 0xEE, 0x3C, 0x80];
    let mut tag = vec![
        0x17, // keyframe + AVC
        0x00, // AVC sequence header
        0x00, 0x00, 0x00, // CTS = 0
        // AVCDecoderConfigurationRecord
        0x01, // configurationVersion
        0x64, // profile (High)
        0x00, // compat
        0x1F, // level (3.1)
        0xFF, // lengthSizeMinusOne=3 | reserved
        0xE1, // numSPS=1 | reserved
    ];
    tag.extend_from_slice(&(sps.len() as u16).to_be_bytes());
    tag.extend_from_slice(&sps);
    tag.push(0x01); // numPPS=1
    tag.extend_from_slice(&(pps.len() as u16).to_be_bytes());
    tag.extend_from_slice(&pps);
    Bytes::from(tag)
}

/// Build an FLV AVC NALU (keyframe or delta).
fn flv_video_nalu(keyframe: bool, cts: i32, nalu_data: &[u8]) -> Bytes {
    let frame_type = if keyframe { 0x17 } else { 0x27 };
    let mut tag = vec![
        frame_type,
        0x01, // AVC NALU
        (cts >> 16) as u8,
        (cts >> 8) as u8,
        cts as u8,
    ];
    tag.extend_from_slice(nalu_data);
    Bytes::from(tag)
}

/// Build an FLV AAC sequence header (audio codec config).
fn flv_audio_seq_header() -> Bytes {
    // AAC-LC (obj=2), 44100 Hz (freq_idx=4), stereo (ch=2)
    let b0: u8 = (2 << 3) | (4 >> 1);
    let b1: u8 = (4 << 7) | (2 << 3);
    Bytes::from(vec![0xAF, 0x00, b0, b1])
}

/// Build an FLV AAC raw data frame.
fn flv_audio_raw(aac_data: &[u8]) -> Bytes {
    let mut tag = vec![0xAF, 0x01];
    tag.extend_from_slice(aac_data);
    Bytes::from(tag)
}

// -- RTMP client helpers --

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

async fn connect_and_publish(port: u16, app: &str, stream_key: &str) -> (TcpStream, ClientSession) {
    let mut stream = tokio::time::timeout(TIMEOUT, TcpStream::connect(format!("127.0.0.1:{port}")))
        .await
        .expect("connect timed out")
        .expect("connect failed");

    stream.set_nodelay(true).unwrap();

    let remaining = rtmp_client_handshake(&mut stream).await;

    let config = ClientSessionConfig::new();
    let (mut session, initial_results) = ClientSession::new(config).expect("client session init failed");

    send_results(&mut stream, &initial_results).await;

    if !remaining.is_empty() {
        let results = session.handle_input(&remaining).unwrap();
        send_results(&mut stream, &results).await;
    }

    tokio::time::sleep(Duration::from_millis(50)).await;

    let connect_result = session
        .request_connection(app.to_string())
        .expect("request_connection failed");
    send_result(&mut stream, &connect_result).await;

    read_until(&mut stream, &mut session, |e| {
        matches!(e, ClientSessionEvent::ConnectionRequestAccepted)
    })
    .await;

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

// -- Tests --

#[tokio::test]
async fn rtmp_video_produces_cmaf() {
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

    let (mut stream, mut session) = connect_and_publish(port, "live", "test").await;

    assert_eq!(bridge.active_stream_count(), 1);

    // 1. Send video sequence header (codec config)
    let seq_header = flv_video_seq_header();
    let result = session
        .publish_video_data(seq_header, RtmpTimestamp::new(0), false)
        .expect("publish_video_data failed");
    send_result(&mut stream, &result).await;

    // 2. Send a keyframe (AVCC-format NALU)
    let nalu = vec![0x00, 0x00, 0x00, 0x04, 0x65, 0x88, 0x84, 0x00]; // length-prefixed IDR
    let keyframe = flv_video_nalu(true, 0, &nalu);
    let result = session
        .publish_video_data(keyframe, RtmpTimestamp::new(0), false)
        .expect("publish_video_data failed");
    send_result(&mut stream, &result).await;

    // 3. Send a delta frame
    let delta_nalu = vec![0x00, 0x00, 0x00, 0x03, 0x41, 0x9A, 0x00];
    let delta = flv_video_nalu(false, 0, &delta_nalu);
    let result = session
        .publish_video_data(delta, RtmpTimestamp::new(33), false)
        .expect("publish_video_data failed");
    send_result(&mut stream, &result).await;

    tokio::time::sleep(Duration::from_millis(100)).await;

    // Verify MoQ broadcast exists
    let mut consumer = origin.consume();
    let (path, bc) = tokio::time::timeout(TIMEOUT, consumer.announced())
        .await
        .expect("announce timed out")
        .expect("origin closed");
    assert_eq!(path.as_str(), "live/test");
    let bc = bc.expect("expected announce");

    // Subscribe to video track (now named "0.mp4")
    let mut track_sub = bc
        .subscribe_track(&Track::new("0.mp4"))
        .expect("subscribe_track failed");

    let mut group = tokio::time::timeout(TIMEOUT, track_sub.next_group())
        .await
        .expect("next_group timed out")
        .expect("next_group failed")
        .expect("video track closed");

    // Frame 0: fMP4 init segment (ftyp + moov)
    let init_frame = tokio::time::timeout(TIMEOUT, group.read_frame())
        .await
        .expect("read_frame timed out")
        .expect("read_frame failed")
        .expect("group closed");

    assert!(init_frame.len() > 8, "init segment too small");
    assert_eq!(&init_frame[4..8], b"ftyp", "init segment should start with ftyp box");

    // Frame 1: fMP4 media segment (moof + mdat) for the keyframe
    let media_frame = tokio::time::timeout(TIMEOUT, group.read_frame())
        .await
        .expect("read_frame timed out")
        .expect("read_frame failed")
        .expect("group closed");

    assert!(media_frame.len() > 8, "media segment too small");
    assert_eq!(&media_frame[4..8], b"moof", "media segment should start with moof box");

    // mdat should contain our original NALU data
    let moof_size = u32::from_be_bytes([media_frame[0], media_frame[1], media_frame[2], media_frame[3]]) as usize;
    let mdat_payload = &media_frame[moof_size + 8..]; // skip mdat header
    assert_eq!(mdat_payload, &nalu, "mdat should contain the original AVCC NALU data");

    // Frame 2: delta frame segment
    let delta_frame = tokio::time::timeout(TIMEOUT, group.read_frame())
        .await
        .expect("read_frame timed out")
        .expect("read_frame failed")
        .expect("group closed");

    assert_eq!(&delta_frame[4..8], b"moof");
    let moof_size = u32::from_be_bytes([delta_frame[0], delta_frame[1], delta_frame[2], delta_frame[3]]) as usize;
    let mdat_payload = &delta_frame[moof_size + 8..];
    assert_eq!(mdat_payload, &delta_nalu);

    // Verify catalog track exists
    let mut catalog_sub = bc
        .subscribe_track(&Track::new(".catalog"))
        .expect("subscribe catalog failed");
    let mut cat_group = tokio::time::timeout(TIMEOUT, catalog_sub.next_group())
        .await
        .expect("catalog group timed out")
        .expect("catalog group failed")
        .expect("catalog closed");
    let cat_frame = tokio::time::timeout(TIMEOUT, cat_group.read_frame())
        .await
        .expect("catalog read timed out")
        .expect("catalog read failed")
        .expect("catalog group closed");

    let catalog_str = std::str::from_utf8(&cat_frame).expect("catalog not valid UTF-8");
    assert!(
        catalog_str.contains("avc1.64001F"),
        "catalog should contain video codec"
    );

    drop(stream);
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(bridge.active_stream_count(), 0);

    server_handle.abort();
}

#[tokio::test]
async fn rtmp_audio_produces_cmaf() {
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

    // 1. Send audio sequence header
    let seq_header = flv_audio_seq_header();
    let result = session
        .publish_audio_data(seq_header, RtmpTimestamp::new(0), false)
        .expect("publish_audio_data failed");
    send_result(&mut stream, &result).await;

    // 2. Send raw AAC frame
    let aac_data = vec![0x01, 0x02, 0x03, 0x04, 0x05];
    let raw = flv_audio_raw(&aac_data);
    let result = session
        .publish_audio_data(raw, RtmpTimestamp::new(0), false)
        .expect("publish_audio_data failed");
    send_result(&mut stream, &result).await;

    tokio::time::sleep(Duration::from_millis(100)).await;

    let mut consumer = origin.consume();
    let (path, bc) = tokio::time::timeout(TIMEOUT, consumer.announced())
        .await
        .expect("announce timed out")
        .expect("origin closed");
    assert_eq!(path.as_str(), "live/audio-test");
    let bc = bc.expect("expected announce");

    // Subscribe to audio track (now "1.mp4")
    let mut audio_track = bc
        .subscribe_track(&Track::new("1.mp4"))
        .expect("subscribe audio track failed");

    let mut group = tokio::time::timeout(TIMEOUT, audio_track.next_group())
        .await
        .expect("audio next_group timed out")
        .expect("audio next_group failed")
        .expect("audio track closed");

    // Frame 0: audio init segment
    let init_frame = tokio::time::timeout(TIMEOUT, group.read_frame())
        .await
        .expect("audio init read timed out")
        .expect("audio init read failed")
        .expect("audio group closed");

    assert_eq!(&init_frame[4..8], b"ftyp", "audio init should start with ftyp");

    // Frame 1: audio media segment (moof + mdat)
    let media_frame = tokio::time::timeout(TIMEOUT, group.read_frame())
        .await
        .expect("audio media read timed out")
        .expect("audio media read failed")
        .expect("audio group closed");

    assert_eq!(&media_frame[4..8], b"moof", "audio media should start with moof");

    // mdat should contain our raw AAC data
    let moof_size = u32::from_be_bytes([media_frame[0], media_frame[1], media_frame[2], media_frame[3]]) as usize;
    let mdat_payload = &media_frame[moof_size + 8..];
    assert_eq!(mdat_payload, &aac_data);

    drop(stream);
    server_handle.abort();
}
