//! Integration test: RTMP client -> RtmpMoqBridge -> MoQ subscriber.
//!
//! Sends real RTMP handshake and publish data over TCP, then verifies
//! the bridge creates a MoQ broadcast with CMAF-formatted tracks.

use lvqr_moq::Track;
use lvqr_test_utils::find_available_port;
use lvqr_test_utils::flv::{
    flv_audio_aac_lc_seq_header_44k_stereo, flv_audio_raw, flv_video_nalu, flv_video_seq_header,
};
use lvqr_test_utils::rtmp::{read_until, rtmp_client_handshake, send_result, send_results};
use rml_rtmp::sessions::{ClientSession, ClientSessionConfig, ClientSessionEvent, PublishRequestType};
use rml_rtmp::time::RtmpTimestamp;
use std::time::Duration;
use tokio::net::TcpStream;

const TIMEOUT: Duration = Duration::from_secs(10);

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

    read_until(&mut stream, &mut session, TIMEOUT, |e| {
        matches!(e, ClientSessionEvent::ConnectionRequestAccepted)
    })
    .await;

    let publish_result = session
        .request_publishing(stream_key.to_string(), PublishRequestType::Live)
        .expect("request_publishing failed");
    send_result(&mut stream, &publish_result).await;

    read_until(&mut stream, &mut session, TIMEOUT, |e| {
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

    let origin = lvqr_moq::OriginProducer::new();
    let bridge = lvqr_ingest::RtmpMoqBridge::new(origin.clone());

    let port = find_available_port();
    let rtmp_config = lvqr_ingest::RtmpConfig {
        bind_addr: ([127, 0, 0, 1], port).into(),
    };
    let rtmp_server = bridge.create_rtmp_server(rtmp_config);
    let server_handle = tokio::spawn(async move { rtmp_server.run(tokio_util::sync::CancellationToken::new()).await });

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

    let origin = lvqr_moq::OriginProducer::new();
    let bridge = lvqr_ingest::RtmpMoqBridge::new(origin.clone());

    let port = find_available_port();
    let rtmp_config = lvqr_ingest::RtmpConfig {
        bind_addr: ([127, 0, 0, 1], port).into(),
    };
    let rtmp_server = bridge.create_rtmp_server(rtmp_config);
    let server_handle = tokio::spawn(async move { rtmp_server.run(tokio_util::sync::CancellationToken::new()).await });

    tokio::time::sleep(Duration::from_millis(50)).await;

    let (mut stream, mut session) = connect_and_publish(port, "live", "audio-test").await;

    // 1. Send audio sequence header
    let seq_header = flv_audio_aac_lc_seq_header_44k_stereo();
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
