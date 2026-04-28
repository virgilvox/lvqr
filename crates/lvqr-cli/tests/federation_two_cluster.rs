//! Two-cluster end-to-end federation test (Tier 4 item 4.4 session B).
//!
//! Spins up two full-stack `TestServer` instances on loopback:
//!
//! * **Server A** runs a standard LVQR relay with no federation
//!   configuration.
//! * **Server B** runs with a single `FederationLink` pointing at
//!   A's relay URL (TLS verification disabled because each
//!   TestServer generates its own self-signed cert). The link
//!   forwards exactly one broadcast name, `"live/room1"`.
//!
//! After both servers come up, the test injects a broadcast
//! directly into A's `OriginProducer` (via `TestServer::origin()`;
//! no RTMP-style ingest to keep the verification path short), adds
//! one video track `"0.mp4"`, and writes a single GOP carrying a
//! known frame payload.
//!
//! A MoQ client then connects to B's relay port and reads the
//! announcement stream. The test asserts:
//!
//! 1. B announces `live/room1` (the federation runner's
//!    `forward_broadcast` helper opened a shadow broadcast on B's
//!    origin).
//! 2. Subscribing to `0.mp4` on B produces the same frame bytes A
//!    wrote.
//!
//! The test deliberately uses the MoQ egress surface on B rather
//! than peeking into B's origin directly: that's the surface every
//! real subscriber (WebTransport browser, `moq-clock` CLI, another
//! federation peer) will use, so the test verifies the same path.

use bytes::Bytes;
use lvqr_cluster::{FederationConnectState, FederationLink};
use lvqr_moq::{Origin, Track};
use lvqr_test_utils::{TestServer, TestServerConfig};
use std::time::{Duration, Instant};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const PROPAGATION_TIMEOUT: Duration = Duration::from_secs(10);

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn federation_link_propagates_broadcast_between_two_clusters() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("lvqr=debug,moq_lite=info")
        .with_test_writer()
        .try_init();

    // --- Server A: vanilla TestServer, will host the source broadcast. ---
    let server_a = TestServer::start(TestServerConfig::default())
        .await
        .expect("start server A");
    let relay_a = server_a.relay_addr();
    let url_a = format!("https://127.0.0.1:{}/", relay_a.port());

    // --- Server B: TestServer with a federation link pointing at A. ---
    // TLS verify off because TestServer's RelayServer generates its own
    // self-signed cert; a production deployment would use real certs
    // via the OS trust store.
    let link = FederationLink::new(url_a.clone(), "", vec!["live/room1".into()]).with_disable_tls_verify(true);
    let server_b = TestServer::start(TestServerConfig::default().with_federation_link(link))
        .await
        .expect("start server B");
    let relay_b = server_b.relay_addr();

    let runner = server_b
        .federation_runner()
        .expect("B must have installed a FederationRunner for the configured link");

    // Active-wait until the federation runner reports its outbound
    // link as Connected. A previous version of this test blind-slept
    // 500 ms here under the assumption that QUIC + TLS handshake
    // would finish in under 100 ms on loopback. That assumption holds
    // on dev hardware but races under macOS CI runner load, where the
    // handshake can occasionally take longer than 500 ms and the
    // subsequent subscribe lands before the federation MoQ session is
    // open -- surfacing as `subscribe error code=13` warnings and a
    // panic at the next_group expect below. Polling at 25 ms keeps
    // the happy-path latency similar (~50 - 150 ms typical on
    // loopback) while letting a contended runner take up to
    // CONNECT_TIMEOUT.
    let status_handle = runner.status_handle();
    let connect_deadline = Instant::now() + CONNECT_TIMEOUT;
    loop {
        let snap = status_handle.snapshot();
        if snap
            .iter()
            .any(|s| s.state == FederationConnectState::Connected)
        {
            break;
        }
        if Instant::now() >= connect_deadline {
            panic!(
                "federation link on B did not reach Connected within {:?}; latest snapshot: {:?}",
                CONNECT_TIMEOUT, snap
            );
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }

    // --- Inject a broadcast on A's origin. ---
    let mut broadcast_a = server_a
        .origin()
        .create_broadcast("live/room1")
        .expect("create broadcast on server A");
    let mut track_a = broadcast_a
        .create_track(Track::new("0.mp4"))
        .expect("create 0.mp4 track on A");
    let mut group_a = track_a.append_group().expect("append first group on A");
    group_a
        .write_frame(Bytes::from_static(b"hello-federation"))
        .expect("write frame on A");
    group_a.finish().expect("finish group on A");

    // --- Connect a MoQ client to B's relay and read announcements. ---
    let mut client_config = moq_native::ClientConfig::default();
    client_config.tls.disable_verify = Some(true);
    let client = client_config.init().expect("init moq client");

    let sub_origin = Origin::produce();
    let mut announcements = sub_origin.consume();
    let client = client.with_consume(sub_origin);

    let url_b: url::Url = format!("https://127.0.0.1:{}/", relay_b.port())
        .parse()
        .expect("valid url for B");
    let _session = tokio::time::timeout(CONNECT_TIMEOUT, client.connect(url_b))
        .await
        .expect("client connect to B timed out")
        .expect("client connect to B failed");

    // Wait for the announcement to arrive on B. The propagation path
    // is: A.origin -> A.relay -> federation MoQ session on B's
    // federation runner -> forward_broadcast spawn -> B.origin ->
    // B.relay -> this MoQ client. Each hop is sub-100 ms on loopback.
    let (path, bc) = tokio::time::timeout(PROPAGATION_TIMEOUT, announcements.announced())
        .await
        .expect("announcement timeout on B")
        .expect("announcement stream on B closed");
    assert_eq!(
        path.as_str(),
        "live/room1",
        "B must announce the federated broadcast under the same name as A"
    );
    let bc = bc.expect("expected B announce, got unannounce");

    // Subscribe to the `0.mp4` track and read the frame bytes that A
    // wrote. The forward_track loop copies the bytes verbatim.
    let mut track_sub = bc.subscribe_track(&Track::new("0.mp4")).expect("subscribe 0.mp4 on B");
    let mut group_sub = tokio::time::timeout(PROPAGATION_TIMEOUT, track_sub.next_group())
        .await
        .expect("next_group timeout on B")
        .expect("next_group error on B")
        .expect("0.mp4 track on B closed before a group landed");

    let frame = tokio::time::timeout(PROPAGATION_TIMEOUT, group_sub.read_frame())
        .await
        .expect("read_frame timeout on B")
        .expect("read_frame error on B")
        .expect("group on B closed before the federated frame arrived");
    assert_eq!(
        &*frame, b"hello-federation",
        "federated frame bytes must equal the source"
    );

    // Keep the broadcast alive until we finish the assertions so B
    // does not see an unannounce mid-test.
    drop(broadcast_a);
    drop(track_a);

    server_a.shutdown().await.expect("shutdown A");
    server_b.shutdown().await.expect("shutdown B");
}
