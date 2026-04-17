//! Tier 3 session C: broadcast-ownership integration test.
//!
//! Exercises [`Cluster::claim_broadcast`] and
//! [`Cluster::find_broadcast_owner`] across two nodes wired over a
//! shared [`chitchat::transport::ChannelTransport`]. The scenarios
//! match the `tracking/TIER_3_PLAN.md` session C acceptance row:
//! node A claims a broadcast, node B sees it; A drops the claim, B
//! stops seeing it.
//!
//! Coverage:
//!
//! * `claim_visible_to_peer` -- B's `find_broadcast_owner` resolves
//!   to A after the claim gossips through; a name no one claimed
//!   resolves to `None`.
//! * `drop_releases_claim` -- dropping the claim on A tombstones
//!   the key and B's lookup eventually returns `None`.
//!
//! Deadline-based filtering (an unrenewed lease eventually treated
//! as absent) is covered by the `Lease` decode + compare logic in
//! `broadcast.rs`'s unit tests; exercising it end-to-end would
//! require forcing the renewer to stop without running its Drop
//! tombstone path, which is not part of the public API.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::Duration;

use chitchat::transport::ChannelTransport;
use lvqr_cluster::{Cluster, ClusterConfig, NodeId};

/// Build a [`ClusterConfig`] tuned for fast two-node tests. Matches
/// the pattern in `tests/two_nodes.rs` so the tests share a
/// consistent timing profile.
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

/// Poll `find_broadcast_owner` until it matches `expected` or
/// `timeout` elapses.
async fn wait_for_owner(cluster: &Cluster, name: &str, expected: Option<&NodeId>, timeout: Duration) -> bool {
    let start = tokio::time::Instant::now();
    while start.elapsed() < timeout {
        let owner = cluster.find_broadcast_owner(name).await;
        if owner.as_ref() == expected {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    false
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn claim_visible_to_peer() {
    let transport = ChannelTransport::default();

    let a = Cluster::bootstrap_with_transport(node_config(20101, vec![], "lvqr-test-c1"), &transport)
        .await
        .expect("bootstrap A");
    let b = Cluster::bootstrap_with_transport(
        node_config(20102, vec!["127.0.0.1:20101".to_string()], "lvqr-test-c1"),
        &transport,
    )
    .await
    .expect("bootstrap B");

    let a_id = a.self_id().clone();

    let claim = a
        .claim_broadcast("live/test", Duration::from_secs(5))
        .await
        .expect("claim");
    assert_eq!(claim.owner, a_id);
    assert_eq!(claim.broadcast, "live/test");

    // A's own lookup returns immediately -- the claim write is
    // synchronous on the self node state.
    assert_eq!(a.find_broadcast_owner("live/test").await, Some(a_id.clone()));

    // B discovers the lease after one or two gossip rounds.
    assert!(
        wait_for_owner(&b, "live/test", Some(&a_id), Duration::from_secs(3)).await,
        "B never observed A as owner of live/test"
    );

    // A broadcast with no claim anywhere is `None`.
    assert_eq!(b.find_broadcast_owner("live/unclaimed").await, None);

    drop(claim);
    a.shutdown().await.expect("A shutdown");
    b.shutdown().await.expect("B shutdown");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn drop_releases_claim() {
    let transport = ChannelTransport::default();

    let a = Cluster::bootstrap_with_transport(node_config(20103, vec![], "lvqr-test-c2"), &transport)
        .await
        .expect("bootstrap A");
    let b = Cluster::bootstrap_with_transport(
        node_config(20104, vec!["127.0.0.1:20103".to_string()], "lvqr-test-c2"),
        &transport,
    )
    .await
    .expect("bootstrap B");

    let a_id = a.self_id().clone();

    let claim = a
        .claim_broadcast("live/test", Duration::from_secs(5))
        .await
        .expect("claim");

    assert!(
        wait_for_owner(&b, "live/test", Some(&a_id), Duration::from_secs(3)).await,
        "B never saw A as owner pre-drop"
    );

    // Dropping releases the claim. The renewer task receives the
    // stop signal, tombstones the key, and exits. B sees the
    // tombstoned key through gossip on the next round.
    drop(claim);

    assert!(
        wait_for_owner(&b, "live/test", None, Duration::from_secs(5)).await,
        "B still sees an owner for live/test after claim was dropped"
    );

    a.shutdown().await.expect("A shutdown");
    b.shutdown().await.expect("B shutdown");
}
