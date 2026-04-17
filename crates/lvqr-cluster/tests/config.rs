//! Tier 3 session E: cluster-wide config integration tests.
//!
//! Drives two [`Cluster`] instances over a shared
//! [`chitchat::transport::ChannelTransport`] and verifies that
//! `Cluster::config_set` on one node propagates via gossip to
//! `Cluster::config_get` + `Cluster::list_config` on a peer.
//!
//! Coverage:
//!
//! * `config_set_reaches_peer` -- A sets a key, B eventually
//!   resolves the same value through both `config_get` and
//!   `list_config`.
//! * `config_lww_prefers_later_write` -- A then B both write the
//!   same key; the later write wins on both nodes after gossip.
//! * `list_broadcasts_enumerates_active_claims` -- A claims two
//!   broadcasts, B's `list_broadcasts` eventually enumerates both.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::Duration;

use chitchat::transport::ChannelTransport;
use lvqr_cluster::{Cluster, ClusterConfig, NodeId};

fn node_config(port: u16, seeds: Vec<String>, cluster_id: &str) -> ClusterConfig {
    let mut cfg = ClusterConfig::for_test();
    cfg.listen = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port);
    cfg.seeds = seeds;
    cfg.node_id = Some(NodeId::new(format!("node-{port}")));
    cfg.cluster_id = cluster_id.to_string();
    cfg.gossip_interval = Duration::from_millis(50);
    cfg.marked_for_deletion_grace_period = Duration::from_millis(500);
    cfg
}

async fn wait_until<F, Fut>(mut probe: F, timeout: Duration) -> bool
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let start = tokio::time::Instant::now();
    while start.elapsed() < timeout {
        if probe().await {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    false
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn config_set_reaches_peer() {
    let transport = ChannelTransport::default();

    let a = Cluster::bootstrap_with_transport(node_config(20301, vec![], "lvqr-test-e1"), &transport)
        .await
        .expect("bootstrap A");
    let b = Cluster::bootstrap_with_transport(
        node_config(20302, vec!["127.0.0.1:20301".to_string()], "lvqr-test-e1"),
        &transport,
    )
    .await
    .expect("bootstrap B");

    // No one has set the key yet; both nodes agree on None.
    assert_eq!(a.config_get("hls.low-latency.enabled").await, None);
    assert_eq!(b.config_get("hls.low-latency.enabled").await, None);

    a.config_set("hls.low-latency.enabled", "true").await.expect("set on A");

    // A's own lookup sees the new value synchronously because the
    // write lands on A's self node state before config_set returns.
    assert_eq!(a.config_get("hls.low-latency.enabled").await, Some("true".to_string()));

    // B observes the value after one or two gossip rounds.
    assert!(
        wait_until(
            || async { b.config_get("hls.low-latency.enabled").await == Some("true".to_string()) },
            Duration::from_secs(3),
        )
        .await,
        "B never saw the config value written on A"
    );

    // list_config enumerates the key with a non-zero timestamp.
    let entries = b.list_config().await;
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].key, "hls.low-latency.enabled");
    assert_eq!(entries[0].value, "true");
    assert!(entries[0].ts_ms > 0);

    a.shutdown().await.expect("A shutdown");
    b.shutdown().await.expect("B shutdown");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn config_lww_prefers_later_write() {
    let transport = ChannelTransport::default();

    let a = Cluster::bootstrap_with_transport(node_config(20303, vec![], "lvqr-test-e2"), &transport)
        .await
        .expect("bootstrap A");
    let b = Cluster::bootstrap_with_transport(
        node_config(20304, vec!["127.0.0.1:20303".to_string()], "lvqr-test-e2"),
        &transport,
    )
    .await
    .expect("bootstrap B");

    a.config_set("feature.flag", "a-wrote-first").await.expect("set A");

    // Wait for B to see A's write, then B overrides.
    assert!(
        wait_until(
            || async { b.config_get("feature.flag").await == Some("a-wrote-first".to_string()) },
            Duration::from_secs(3),
        )
        .await,
        "B never saw A's initial write"
    );

    // Sleep a few ms so B's ts_ms is strictly greater than A's.
    // Even without this sleep, the LWW tiebreak on the value
    // string would still produce a deterministic winner -- but we
    // want to exercise the common real-world case where the
    // operator's second write is strictly later in wall time.
    tokio::time::sleep(Duration::from_millis(5)).await;

    b.config_set("feature.flag", "b-wrote-second").await.expect("set B");

    // Both nodes eventually converge on B's value.
    assert!(
        wait_until(
            || async { a.config_get("feature.flag").await == Some("b-wrote-second".to_string()) },
            Duration::from_secs(3),
        )
        .await,
        "A never converged on B's override"
    );
    assert!(
        wait_until(
            || async { b.config_get("feature.flag").await == Some("b-wrote-second".to_string()) },
            Duration::from_secs(3),
        )
        .await,
        "B never saw its own override through the LWW reader"
    );

    a.shutdown().await.expect("A shutdown");
    b.shutdown().await.expect("B shutdown");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn list_broadcasts_enumerates_active_claims() {
    let transport = ChannelTransport::default();

    let a = Cluster::bootstrap_with_transport(node_config(20305, vec![], "lvqr-test-e3"), &transport)
        .await
        .expect("bootstrap A");
    let b = Cluster::bootstrap_with_transport(
        node_config(20306, vec!["127.0.0.1:20305".to_string()], "lvqr-test-e3"),
        &transport,
    )
    .await
    .expect("bootstrap B");

    let a_id = a.self_id().clone();
    let _c1 = a
        .claim_broadcast("live/one", Duration::from_secs(5))
        .await
        .expect("claim 1");
    let _c2 = a
        .claim_broadcast("live/two", Duration::from_secs(5))
        .await
        .expect("claim 2");

    assert!(
        wait_until(
            || async {
                let list = b.list_broadcasts().await;
                list.len() == 2 && list.iter().all(|e| e.owner == a_id)
            },
            Duration::from_secs(3),
        )
        .await,
        "B never observed both broadcasts from A; last list: {:?}",
        b.list_broadcasts().await
    );

    let list = b.list_broadcasts().await;
    let names: Vec<&str> = list.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, vec!["live/one", "live/two"]);
    for entry in &list {
        assert!(entry.expires_at_ms > 0);
    }

    a.shutdown().await.expect("A shutdown");
    b.shutdown().await.expect("B shutdown");
}
