//! Tier 3 session F1: per-node endpoints KV integration test.
//!
//! Drives two [`Cluster`] instances over a shared
//! [`chitchat::transport::ChannelTransport`] and verifies that
//! endpoints advertised on one node reach a peer via both
//! [`Cluster::node_endpoints`] and the redirect helper
//! [`Cluster::find_owner_endpoints`].
//!
//! Coverage:
//!
//! * `endpoints_propagate_via_gossip` -- A sets endpoints, B
//!   eventually reads them via `node_endpoints` and sees them on
//!   the corresponding [`ClusterNode`] via `members`.
//! * `find_owner_endpoints_resolves_claim_to_url` -- A claims a
//!   broadcast and sets endpoints; B's `find_owner_endpoints`
//!   returns A's endpoints keyed to the broadcast.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::Duration;

use chitchat::transport::ChannelTransport;
use lvqr_cluster::{Cluster, ClusterConfig, NodeEndpoints, NodeId};

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
async fn endpoints_propagate_via_gossip() {
    let transport = ChannelTransport::default();

    let a = Cluster::bootstrap_with_transport(node_config(20501, vec![], "lvqr-test-f1-a"), &transport)
        .await
        .expect("bootstrap A");
    let b = Cluster::bootstrap_with_transport(
        node_config(20502, vec!["127.0.0.1:20501".to_string()], "lvqr-test-f1-a"),
        &transport,
    )
    .await
    .expect("bootstrap B");

    let a_id = a.self_id().clone();

    // Before A has advertised: B sees no endpoints for A.
    assert!(b.node_endpoints(&a_id).await.is_none());

    let advertised = NodeEndpoints {
        hls: Some("http://a.local:8888".into()),
        dash: None,
        rtsp: Some("rtsp://a.local:8554".into()),
    };
    a.set_endpoints(&advertised).await.expect("set endpoints");

    // A's own read is immediate (self_node_state set is synchronous).
    assert_eq!(a.node_endpoints(&a_id).await, Some(advertised.clone()));

    // B reads via gossip within a few rounds.
    assert!(
        wait_until(
            || async { b.node_endpoints(&a_id).await.as_ref() == Some(&advertised) },
            Duration::from_secs(3),
        )
        .await,
        "B never saw A's advertised endpoints"
    );

    // Verify the endpoints attach to the ClusterNode view too.
    assert!(
        wait_until(
            || async {
                b.members()
                    .await
                    .into_iter()
                    .any(|m| m.id == a_id && m.endpoints.as_ref() == Some(&advertised))
            },
            Duration::from_secs(3),
        )
        .await,
        "B's members() never reflected A's endpoints; last: {:?}",
        b.members().await,
    );

    a.shutdown().await.expect("A shutdown");
    b.shutdown().await.expect("B shutdown");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn find_owner_endpoints_resolves_claim_to_url() {
    let transport = ChannelTransport::default();

    let a = Cluster::bootstrap_with_transport(node_config(20503, vec![], "lvqr-test-f1-b"), &transport)
        .await
        .expect("bootstrap A");
    let b = Cluster::bootstrap_with_transport(
        node_config(20504, vec!["127.0.0.1:20503".to_string()], "lvqr-test-f1-b"),
        &transport,
    )
    .await
    .expect("bootstrap B");

    let a_id = a.self_id().clone();
    let expected = NodeEndpoints {
        hls: Some("http://a.local:8888".into()),
        ..Default::default()
    };
    a.set_endpoints(&expected).await.expect("set endpoints");
    let _claim = a
        .claim_broadcast("live/demo", Duration::from_secs(5))
        .await
        .expect("claim");

    // B converges on both the broadcast owner AND the endpoint KV
    // before the redirect resolver returns the expected tuple.
    assert!(
        wait_until(
            || async {
                matches!(
                    b.find_owner_endpoints("live/demo").await,
                    Some((ref id, ref ep)) if id == &a_id && ep == &expected
                )
            },
            Duration::from_secs(3),
        )
        .await,
        "B never resolved the owner endpoints for live/demo"
    );

    // Unknown broadcast: resolver returns None.
    assert!(b.find_owner_endpoints("never-claimed").await.is_none());

    drop(_claim);
    a.shutdown().await.expect("A shutdown");
    b.shutdown().await.expect("B shutdown");
}
