//! Integration tests for the LVQR relay.
//!
//! These tests start a real MoQ relay, connect publishers and subscribers
//! over QUIC, and verify that data flows correctly. No mocks.

use moq_lite::{Origin, Track};
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(10);

/// Start a relay, publish a track, subscribe to it, and verify data arrives.
#[tokio::test]
async fn publish_subscribe_single_track() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("lvqr=debug,moq=debug")
        .with_test_writer()
        .try_init();

    // Start the relay
    let relay_config = lvqr_relay::RelayConfig::new("[::]:0".parse().unwrap());
    let relay = lvqr_relay::RelayServer::new(relay_config);
    let (mut server, relay_addr) = relay.init_server().expect("failed to init relay server");

    // Run the relay in the background
    let relay_origin = relay.origin().clone();
    let relay_handle = tokio::spawn(async move { relay.accept_loop(&mut server).await });

    // -- Publisher side --
    // Create a broadcast with a video track and write data to it
    let pub_origin = relay_origin.clone();
    let mut broadcast = pub_origin
        .create_broadcast("live/test")
        .expect("failed to create broadcast");
    let mut track = broadcast
        .create_track(Track::new("video"))
        .expect("failed to create track");

    // Write a GOP: one group with two frames
    let mut group = track.append_group().expect("failed to append group");
    group
        .write_frame(b"keyframe-data".as_ref())
        .expect("failed to write keyframe");
    group
        .write_frame(b"delta-frame-1".as_ref())
        .expect("failed to write delta frame");
    group.finish().expect("failed to finish group");

    // -- Subscriber side --
    // Connect a client and subscribe to the track
    let sub_origin = Origin::produce();
    let mut announcements = sub_origin.consume();

    let mut client_config = moq_native::ClientConfig::default();
    client_config.tls.disable_verify = Some(true);
    let client = client_config.init().expect("failed to init client");

    let url: url::Url = format!("https://localhost:{}", relay_addr.port()).parse().unwrap();

    let client = client.with_consume(sub_origin);
    let session = tokio::time::timeout(TIMEOUT, client.connect(url))
        .await
        .expect("client connect timed out")
        .expect("client connect failed");

    // Wait for the broadcast announcement
    let (path, bc) = tokio::time::timeout(TIMEOUT, announcements.announced())
        .await
        .expect("announce timed out")
        .expect("origin closed");

    assert_eq!(path.as_str(), "live/test");
    let bc = bc.expect("expected announce, got unannounce");

    // Subscribe to the video track
    let mut track_sub = bc
        .subscribe_track(&Track::new("video"))
        .expect("subscribe_track failed");

    // Read the group
    let mut group_sub = tokio::time::timeout(TIMEOUT, track_sub.next_group())
        .await
        .expect("next_group timed out")
        .expect("next_group failed")
        .expect("track closed prematurely");

    // Read frames and verify payloads
    let frame1 = tokio::time::timeout(TIMEOUT, group_sub.read_frame())
        .await
        .expect("read_frame timed out")
        .expect("read_frame failed")
        .expect("group closed prematurely");
    assert_eq!(&*frame1, b"keyframe-data");

    let frame2 = tokio::time::timeout(TIMEOUT, group_sub.read_frame())
        .await
        .expect("read_frame timed out")
        .expect("read_frame failed")
        .expect("group closed prematurely");
    assert_eq!(&*frame2, b"delta-frame-1");

    // Cleanup
    drop(session);
    drop(broadcast);
    drop(track);
    relay_handle.abort();
}

/// Test fanout: one publisher, multiple subscribers all receive the same data.
#[tokio::test]
async fn fanout_multiple_subscribers() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("lvqr=debug")
        .with_test_writer()
        .try_init();

    let relay_config = lvqr_relay::RelayConfig::new("[::]:0".parse().unwrap());
    let relay = lvqr_relay::RelayServer::new(relay_config);
    let (mut server, relay_addr) = relay.init_server().expect("failed to init relay server");

    let relay_origin = relay.origin().clone();
    let relay_handle = tokio::spawn(async move { relay.accept_loop(&mut server).await });

    // Publish a broadcast with known data
    let mut broadcast = relay_origin
        .create_broadcast("live/fanout")
        .expect("failed to create broadcast");
    let mut track = broadcast
        .create_track(Track::new("video"))
        .expect("failed to create track");

    let mut group = track.append_group().expect("failed to append group");
    group
        .write_frame(b"shared-frame".as_ref())
        .expect("failed to write frame");
    group.finish().expect("failed to finish group");

    // Connect 3 subscribers
    let mut subscribers = Vec::new();
    for i in 0..3 {
        let sub_origin = Origin::produce();
        let mut announcements = sub_origin.consume();

        let mut client_config = moq_native::ClientConfig::default();
        client_config.tls.disable_verify = Some(true);
        let client = client_config.init().expect("failed to init client");

        let url: url::Url = format!("https://localhost:{}", relay_addr.port()).parse().unwrap();

        let client = client.with_consume(sub_origin);
        let session = tokio::time::timeout(TIMEOUT, client.connect(url))
            .await
            .expect("client connect timed out")
            .expect("client connect failed");

        let (path, bc) = tokio::time::timeout(TIMEOUT, announcements.announced())
            .await
            .unwrap_or_else(|_| panic!("announce timed out for subscriber {i}"))
            .expect("origin closed");

        assert_eq!(path.as_str(), "live/fanout");
        let bc = bc.expect("expected announce");

        let track_sub = bc
            .subscribe_track(&Track::new("video"))
            .expect("subscribe_track failed");

        subscribers.push((session, track_sub));
    }

    // All 3 subscribers should receive the same frame
    for (i, (_session, track_sub)) in subscribers.iter_mut().enumerate() {
        let mut group_sub = tokio::time::timeout(TIMEOUT, track_sub.next_group())
            .await
            .unwrap_or_else(|_| panic!("next_group timed out for subscriber {i}"))
            .unwrap_or_else(|_| panic!("next_group failed for subscriber {i}"))
            .unwrap_or_else(|| panic!("track closed prematurely for subscriber {i}"));

        let frame = tokio::time::timeout(TIMEOUT, group_sub.read_frame())
            .await
            .unwrap_or_else(|_| panic!("read_frame timed out for subscriber {i}"))
            .unwrap_or_else(|_| panic!("read_frame failed for subscriber {i}"))
            .unwrap_or_else(|| panic!("group closed prematurely for subscriber {i}"));

        assert_eq!(&*frame, b"shared-frame", "subscriber {i} got wrong data");
    }

    // Cleanup
    for (session, _) in subscribers {
        drop(session);
    }
    drop(broadcast);
    drop(track);
    relay_handle.abort();
}

/// Test that the relay tracks connection metrics correctly.
#[tokio::test]
async fn relay_metrics() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("lvqr=debug")
        .with_test_writer()
        .try_init();

    let relay_config = lvqr_relay::RelayConfig::new("[::]:0".parse().unwrap());
    let relay = lvqr_relay::RelayServer::new(relay_config);
    let (mut server, relay_addr) = relay.init_server().expect("failed to init relay server");
    let metrics = relay.metrics().clone();

    assert_eq!(metrics.connections_total.load(std::sync::atomic::Ordering::Relaxed), 0);

    let relay_handle = tokio::spawn(async move { relay.accept_loop(&mut server).await });

    // Connect a client
    let sub_origin = Origin::produce();

    let mut client_config = moq_native::ClientConfig::default();
    client_config.tls.disable_verify = Some(true);
    let client = client_config.init().expect("failed to init client");

    let url: url::Url = format!("https://localhost:{}", relay_addr.port()).parse().unwrap();

    let client = client.with_consume(sub_origin);
    let session = tokio::time::timeout(TIMEOUT, client.connect(url))
        .await
        .expect("client connect timed out")
        .expect("client connect failed");

    // Give the relay a moment to register the connection
    tokio::time::sleep(Duration::from_millis(100)).await;

    assert_eq!(metrics.connections_total.load(std::sync::atomic::Ordering::Relaxed), 1);
    assert_eq!(metrics.connections_active.load(std::sync::atomic::Ordering::Relaxed), 1);

    // Disconnect
    drop(session);
    tokio::time::sleep(Duration::from_millis(200)).await;

    assert_eq!(metrics.connections_active.load(std::sync::atomic::Ordering::Relaxed), 0);

    relay_handle.abort();
}

/// Test that the connection callback fires on connect and disconnect.
#[tokio::test]
async fn connection_callback() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("lvqr=debug")
        .with_test_writer()
        .try_init();

    let relay_config = lvqr_relay::RelayConfig::new("[::]:0".parse().unwrap());
    let mut relay = lvqr_relay::RelayServer::new(relay_config);
    let (mut server, relay_addr) = relay.init_server().expect("failed to init relay server");

    let connects = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
    let disconnects = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));

    let c = connects.clone();
    let d = disconnects.clone();
    relay.set_connection_callback(std::sync::Arc::new(move |_conn_id, connected| {
        if connected {
            c.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        } else {
            d.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
    }));

    let relay_handle = tokio::spawn(async move { relay.accept_loop(&mut server).await });

    // Connect a client
    let sub_origin = Origin::produce();
    let mut client_config = moq_native::ClientConfig::default();
    client_config.tls.disable_verify = Some(true);
    let client = client_config.init().expect("failed to init client");

    let url: url::Url = format!("https://localhost:{}", relay_addr.port()).parse().unwrap();
    let client = client.with_consume(sub_origin);
    let session = tokio::time::timeout(TIMEOUT, client.connect(url))
        .await
        .expect("connect timed out")
        .expect("connect failed");

    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(connects.load(std::sync::atomic::Ordering::Relaxed), 1);
    assert_eq!(disconnects.load(std::sync::atomic::Ordering::Relaxed), 0);

    // Disconnect
    drop(session);
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(disconnects.load(std::sync::atomic::Ordering::Relaxed), 1);

    relay_handle.abort();
}
