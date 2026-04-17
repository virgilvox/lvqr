//! Tier 3 session D: capacity-advertisement integration test.
//!
//! Drives two [`Cluster`] instances over a shared
//! [`chitchat::transport::ChannelTransport`] and verifies that
//! capacity values written into one node's [`CapacityGauge`]
//! propagate to the peer's [`Cluster::members`] view via gossip.
//!
//! The session D row in `tracking/TIER_3_PLAN.md` calls for an
//! integration test that reads `members()` and asserts capacity
//! fields populate. This file implements that row.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::Duration;

use chitchat::transport::ChannelTransport;
use lvqr_cluster::{Cluster, ClusterConfig, NodeCapacity, NodeId};

/// Build a [`ClusterConfig`] tuned for fast two-node tests. Mirrors
/// the pattern in `tests/two_nodes.rs` and `tests/ownership.rs`:
/// a loopback listen address on a fixed port, aggressive gossip
/// interval, and a short capacity-advertise interval so the first
/// publish lands inside the test's wait window.
fn node_config(port: u16, seeds: Vec<String>, cluster_id: &str) -> ClusterConfig {
    let mut cfg = ClusterConfig::for_test();
    cfg.listen = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port);
    cfg.seeds = seeds;
    cfg.node_id = Some(NodeId::new(format!("node-{port}")));
    cfg.cluster_id = cluster_id.to_string();
    cfg.gossip_interval = Duration::from_millis(50);
    cfg.marked_for_deletion_grace_period = Duration::from_millis(500);
    // 150 ms: first publish fires 150 ms after bootstrap, leaving
    // enough headroom for the caller to stage real gauge values
    // between bootstrap and the first tick.
    cfg.capacity_advertise_interval = Duration::from_millis(150);
    cfg
}

/// Poll `cluster.members()` until the peer at `peer_id` carries a
/// non-empty capacity matching `predicate`, or `timeout` elapses.
/// Returns the observed capacity on success.
async fn wait_for_capacity(
    cluster: &Cluster,
    peer_id: &NodeId,
    predicate: impl Fn(&NodeCapacity) -> bool,
    timeout: Duration,
) -> Option<NodeCapacity> {
    let start = tokio::time::Instant::now();
    while start.elapsed() < timeout {
        for member in cluster.members().await {
            if member.id != *peer_id {
                continue;
            }
            if let Some(cap) = member.capacity {
                if predicate(&cap) {
                    return Some(cap);
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    None
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn capacity_reaches_peer_via_gossip() {
    let transport = ChannelTransport::default();

    let a = Cluster::bootstrap_with_transport(node_config(20201, vec![], "lvqr-test-d1"), &transport)
        .await
        .expect("bootstrap A");
    let b = Cluster::bootstrap_with_transport(
        node_config(20202, vec!["127.0.0.1:20201".to_string()], "lvqr-test-d1"),
        &transport,
    )
    .await
    .expect("bootstrap B");

    let a_id = a.self_id().clone();

    // Stage non-zero values on A's gauge before the first advertise
    // tick fires (150 ms after bootstrap). The first publish will
    // pick these up; gossip propagation adds ~1-2 rounds (50 ms
    // each) before B sees them.
    a.capacity_gauge().set_cpu_pct(42.5);
    a.capacity_gauge().set_rss_bytes(1_234_567);
    a.capacity_gauge().set_bytes_out_per_sec(890_000);

    let observed = wait_for_capacity(
        &b,
        &a_id,
        |cap| cap.rss_bytes == 1_234_567 && cap.bytes_out_per_sec == 890_000,
        Duration::from_secs(3),
    )
    .await
    .expect("B never observed A's advertised capacity");

    // cpu_pct is f32; compare with a small epsilon rather than
    // exact equality to tolerate any representation round-trip.
    assert!(
        (observed.cpu_pct - 42.5).abs() < 1e-5,
        "unexpected cpu_pct: {}",
        observed.cpu_pct
    );
    assert_eq!(observed.rss_bytes, 1_234_567);
    assert_eq!(observed.bytes_out_per_sec, 890_000);

    a.shutdown().await.expect("A shutdown");
    b.shutdown().await.expect("B shutdown");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn capacity_updates_reflect_in_subsequent_snapshots() {
    // Second scenario: once the first snapshot has been observed,
    // pushing a new gauge value is picked up on the following
    // advertise tick and gossipped out. Pins the "rolling snapshot"
    // behaviour: there is no leaked state from the initial
    // capacity, and later writes override.
    let transport = ChannelTransport::default();

    let a = Cluster::bootstrap_with_transport(node_config(20203, vec![], "lvqr-test-d2"), &transport)
        .await
        .expect("bootstrap A");
    let b = Cluster::bootstrap_with_transport(
        node_config(20204, vec!["127.0.0.1:20203".to_string()], "lvqr-test-d2"),
        &transport,
    )
    .await
    .expect("bootstrap B");

    let a_id = a.self_id().clone();

    a.capacity_gauge().set_rss_bytes(100);
    wait_for_capacity(&b, &a_id, |cap| cap.rss_bytes == 100, Duration::from_secs(3))
        .await
        .expect("B never saw initial rss_bytes=100");

    // Update: bump the value and wait for propagation.
    a.capacity_gauge().set_rss_bytes(500);
    let observed = wait_for_capacity(&b, &a_id, |cap| cap.rss_bytes == 500, Duration::from_secs(3))
        .await
        .expect("B never saw updated rss_bytes=500");
    assert_eq!(observed.rss_bytes, 500);

    a.shutdown().await.expect("A shutdown");
    b.shutdown().await.expect("B shutdown");
}
