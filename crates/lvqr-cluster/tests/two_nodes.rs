//! Tier 3 session B: two-node integration test.
//!
//! Drives two [`Cluster`] instances over a shared
//! [`chitchat::transport::ChannelTransport`] so the test can verify
//! convergence and failure-detector-driven tear-down without
//! reserving real UDP ports on the CI host.
//!
//! Coverage:
//!
//! * `two_nodes_converge_via_channel_transport` -- with A as seed
//!   for B, both nodes eventually see each other in `members()`
//!   within a short bounded window.
//! * `shutdown_drops_peer_from_members` -- after A calls
//!   `shutdown()`, the failure detector on B marks A dead and B's
//!   `members()` eventually stops returning A.
//! * `cluster_id_isolation_prevents_convergence` -- two nodes with
//!   different `cluster_id` values do not converge even though
//!   they share the transport and one is seeded with the other's
//!   address. This pins the rejection check chitchat's SYN
//!   handler already implements so a regression in our config
//!   plumbing is caught.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::Duration;

use chitchat::transport::ChannelTransport;
use lvqr_cluster::{Cluster, ClusterConfig, NodeId};

/// Build a [`ClusterConfig`] with deterministic `node_id` + aggressive
/// gossip / GC / failure-detector timings so the assertions complete
/// in under a couple seconds on loaded CI hosts.
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

/// Poll `cluster.members()` until `peer_id` appears or `timeout`
/// elapses. Returns `true` on success.
async fn wait_until_peer_known(cluster: &Cluster, peer_id: &NodeId, timeout: Duration) -> bool {
    let start = tokio::time::Instant::now();
    while start.elapsed() < timeout {
        let members = cluster.members().await;
        if members.iter().any(|m| m.id == *peer_id) {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    false
}

/// Poll `cluster.members()` until `peer_id` is gone or `timeout`
/// elapses.
async fn wait_until_peer_gone(cluster: &Cluster, peer_id: &NodeId, timeout: Duration) -> bool {
    let start = tokio::time::Instant::now();
    while start.elapsed() < timeout {
        let members = cluster.members().await;
        if !members.iter().any(|m| m.id == *peer_id) {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    false
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_nodes_converge_via_channel_transport() {
    let transport = ChannelTransport::default();

    let a = Cluster::bootstrap_with_transport(node_config(20001, vec![], "lvqr-test-b1"), &transport)
        .await
        .expect("bootstrap A");
    let b = Cluster::bootstrap_with_transport(
        node_config(20002, vec!["127.0.0.1:20001".to_string()], "lvqr-test-b1"),
        &transport,
    )
    .await
    .expect("bootstrap B");

    let a_id = a.self_id().clone();
    let b_id = b.self_id().clone();

    assert!(
        wait_until_peer_known(&a, &b_id, Duration::from_secs(3)).await,
        "A never saw B: members = {:?}",
        a.members().await
    );
    assert!(
        wait_until_peer_known(&b, &a_id, Duration::from_secs(3)).await,
        "B never saw A: members = {:?}",
        b.members().await
    );

    a.shutdown().await.expect("A shutdown");
    b.shutdown().await.expect("B shutdown");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shutdown_drops_peer_from_members() {
    let transport = ChannelTransport::default();

    let a = Cluster::bootstrap_with_transport(node_config(20003, vec![], "lvqr-test-b2"), &transport)
        .await
        .expect("bootstrap A");
    let b = Cluster::bootstrap_with_transport(
        node_config(20004, vec!["127.0.0.1:20003".to_string()], "lvqr-test-b2"),
        &transport,
    )
    .await
    .expect("bootstrap B");

    let a_id = a.self_id().clone();

    assert!(
        wait_until_peer_known(&b, &a_id, Duration::from_secs(3)).await,
        "B never saw A pre-shutdown"
    );

    a.shutdown().await.expect("A shutdown");

    // Failure detector + grace period should converge B within a few
    // seconds. 8 s tolerates a slow CI runner without masking a true
    // hang: the gossip interval is 50 ms and the grace period is
    // 500 ms, so the happy path is well under 2 s.
    assert!(
        wait_until_peer_gone(&b, &a_id, Duration::from_secs(8)).await,
        "B still sees A after shutdown: members = {:?}",
        b.members().await
    );

    b.shutdown().await.expect("B shutdown");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cluster_id_isolation_prevents_convergence() {
    let transport = ChannelTransport::default();

    let a = Cluster::bootstrap_with_transport(node_config(20005, vec![], "cluster-alpha"), &transport)
        .await
        .expect("bootstrap A");
    // Seed B with A's address even though they are in different clusters.
    let b = Cluster::bootstrap_with_transport(
        node_config(20006, vec!["127.0.0.1:20005".to_string()], "cluster-beta"),
        &transport,
    )
    .await
    .expect("bootstrap B");

    let a_id = a.self_id().clone();
    let b_id = b.self_id().clone();

    // Give gossip plenty of time to run. A and B exchange SYNs but
    // chitchat rejects the cross-cluster SYN on receipt so no node
    // state crosses over.
    tokio::time::sleep(Duration::from_millis(500)).await;

    let a_members = a.members().await;
    let b_members = b.members().await;

    assert!(
        !a_members.iter().any(|m| m.id == b_id),
        "A saw B across cluster boundary: {a_members:?}"
    );
    assert!(
        !b_members.iter().any(|m| m.id == a_id),
        "B saw A across cluster boundary: {b_members:?}"
    );
    // Self-membership still holds.
    assert_eq!(a_members.len(), 1);
    assert_eq!(b_members.len(), 1);

    a.shutdown().await.expect("A shutdown");
    b.shutdown().await.expect("B shutdown");
}
