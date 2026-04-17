//! Tier 3 session F2c: ingest auto-claim bridge integration test.
//!
//! Exercises
//! [`lvqr_cli::cluster_claim::install_cluster_claim_bridge`] in
//! isolation from `lvqr_cli::start`: two real UDP loopback
//! clusters, the bridge installed on A's registry, and a
//! `get_or_create` on that registry fires
//! `on_entry_created` which triggers the auto-claim pipeline.
//!
//! Coverage:
//!
//! * `auto_claim_fires_on_first_broadcast` -- a single
//!   `get_or_create` on the registry causes A's self-lookup to
//!   return itself as owner, and B's lookup to converge on A via
//!   gossip.
//! * `auto_claim_deduplicates_across_tracks` -- calling
//!   `get_or_create` twice (once per track) for the same
//!   broadcast produces exactly one logical claim; B still sees
//!   A as the sole owner.
//! * `auto_claim_released_when_broadcaster_closes` -- dropping
//!   the last Arc on the broadcaster tears it down, the drain
//!   task observes the close, drops the `Claim`, and both A and
//!   B converge on `None`.
//!
//! Feature-gated on `cluster` (default-on); skipped otherwise.

#![cfg(feature = "cluster")]

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use lvqr_cli::cluster_claim::install_cluster_claim_bridge;
use lvqr_cluster::{Cluster, ClusterConfig, NodeId};
use lvqr_fragment::{FragmentBroadcasterRegistry, FragmentMeta};

const GOSSIP_PORT_A: u16 = 20901;
const GOSSIP_PORT_B: u16 = 20902;
const GOSSIP_PORT_C: u16 = 20903;
const GOSSIP_PORT_D: u16 = 20904;
const GOSSIP_PORT_E: u16 = 20905;
const GOSSIP_PORT_F: u16 = 20906;

fn cluster_config(listen_port: u16, seeds: Vec<String>, node_id: &str, cluster_id: &str) -> ClusterConfig {
    let mut cfg = ClusterConfig::for_test();
    cfg.listen = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), listen_port);
    cfg.seeds = seeds;
    cfg.node_id = Some(NodeId::new(node_id.to_string()));
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
async fn auto_claim_fires_on_first_broadcast() {
    let a = Cluster::bootstrap(cluster_config(GOSSIP_PORT_A, vec![], "node-a-f2c-1", "lvqr-test-f2c-1"))
        .await
        .expect("bootstrap A");
    let a = Arc::new(a);
    let b = Cluster::bootstrap(cluster_config(
        GOSSIP_PORT_B,
        vec![format!("127.0.0.1:{GOSSIP_PORT_A}")],
        "node-b-f2c-1",
        "lvqr-test-f2c-1",
    ))
    .await
    .expect("bootstrap B");

    let registry = FragmentBroadcasterRegistry::new();
    install_cluster_claim_bridge(a.clone(), Duration::from_secs(5), &registry);

    // Before any broadcaster is created, no claim exists.
    assert_eq!(a.find_broadcast_owner("live/auto").await, None);

    // Create a broadcaster -- this fires `on_entry_created`
    // which spawns the auto-claim task.
    let _bc = registry.get_or_create("live/auto", "0.mp4", FragmentMeta::new("avc1", 90_000));

    let a_id = a.self_id().clone();
    assert!(
        wait_until(
            || async { a.find_broadcast_owner("live/auto").await == Some(a_id.clone()) },
            Duration::from_secs(3),
        )
        .await,
        "A never observed its own auto-claim"
    );
    assert!(
        wait_until(
            || async { b.find_broadcast_owner("live/auto").await == Some(a_id.clone()) },
            Duration::from_secs(5),
        )
        .await,
        "B never observed A's auto-claim via gossip"
    );

    // Clean up. Dropping the broadcaster arc tears the broadcaster
    // down, which causes the drain task to exit and drop the claim.
    drop(_bc);
    // The dedup release + tombstone propagation is covered by the
    // dedicated `..._released_when_broadcaster_closes` test below;
    // here we just exit.

    // Drop the local Arc<Cluster> -- the spawned auto-claim task
    // still holds its own clone. The background chitchat server is
    // torn down when the Cluster's last Arc drops via `Cluster::Drop`
    // (which cancels + aborts the advertiser) and the
    // `ChitchatHandle` drop.
    drop(a);
    b.shutdown().await.expect("B shutdown");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn auto_claim_deduplicates_across_tracks() {
    let a = Cluster::bootstrap(cluster_config(GOSSIP_PORT_C, vec![], "node-a-f2c-2", "lvqr-test-f2c-2"))
        .await
        .expect("bootstrap A");
    let a = Arc::new(a);
    let b = Cluster::bootstrap(cluster_config(
        GOSSIP_PORT_D,
        vec![format!("127.0.0.1:{GOSSIP_PORT_C}")],
        "node-b-f2c-2",
        "lvqr-test-f2c-2",
    ))
    .await
    .expect("bootstrap B");

    let registry = FragmentBroadcasterRegistry::new();
    install_cluster_claim_bridge(a.clone(), Duration::from_secs(5), &registry);

    // Create both video and audio broadcasters for the same
    // broadcast. The callback fires twice; the bridge must
    // dedup to exactly one claim.
    let _video = registry.get_or_create("live/two-tracks", "0.mp4", FragmentMeta::new("avc1", 90_000));
    let _audio = registry.get_or_create("live/two-tracks", "1.mp4", FragmentMeta::new("mp4a", 44_100));

    let a_id = a.self_id().clone();
    assert!(
        wait_until(
            || async { a.find_broadcast_owner("live/two-tracks").await == Some(a_id.clone()) },
            Duration::from_secs(3),
        )
        .await,
        "A never observed its auto-claim for the two-track broadcast"
    );
    assert!(
        wait_until(
            || async { b.find_broadcast_owner("live/two-tracks").await == Some(a_id.clone()) },
            Duration::from_secs(5),
        )
        .await,
        "B never observed A's claim for the two-track broadcast"
    );

    // `list_broadcasts` on A reports the broadcast exactly once.
    // (The LWW reducer would hide duplicates but we also want to
    // ensure only one claim was actually made so the renewer
    // does not double up.)
    let list = a.list_broadcasts().await;
    let hits = list.iter().filter(|e| e.name == "live/two-tracks").count();
    assert_eq!(hits, 1, "expected exactly one listed broadcast, got {hits}");

    drop(_video);
    drop(_audio);

    // Drop the local Arc<Cluster> -- the spawned auto-claim task
    // still holds its own clone. The background chitchat server is
    // torn down when the Cluster's last Arc drops via `Cluster::Drop`
    // (which cancels + aborts the advertiser) and the
    // `ChitchatHandle` drop.
    drop(a);
    b.shutdown().await.expect("B shutdown");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn auto_claim_released_when_broadcaster_closes() {
    let a = Cluster::bootstrap(cluster_config(GOSSIP_PORT_E, vec![], "node-a-f2c-3", "lvqr-test-f2c-3"))
        .await
        .expect("bootstrap A");
    let a = Arc::new(a);
    let b = Cluster::bootstrap(cluster_config(
        GOSSIP_PORT_F,
        vec![format!("127.0.0.1:{GOSSIP_PORT_E}")],
        "node-b-f2c-3",
        "lvqr-test-f2c-3",
    ))
    .await
    .expect("bootstrap B");

    let registry = FragmentBroadcasterRegistry::new();
    install_cluster_claim_bridge(a.clone(), Duration::from_secs(5), &registry);

    let bc = registry.get_or_create("live/ephemeral", "0.mp4", FragmentMeta::new("avc1", 90_000));
    let a_id = a.self_id().clone();

    // Wait for the claim to land on both nodes.
    assert!(
        wait_until(
            || async { b.find_broadcast_owner("live/ephemeral").await == Some(a_id.clone()) },
            Duration::from_secs(5),
        )
        .await,
        "B never saw A's initial claim"
    );

    // Drop the last producer clone AND any claim the registry
    // holds internally by dropping both `bc` and clearing the
    // registry's map. In production every ingest protocol's
    // session teardown fulfills this implicitly when the last
    // publisher disconnects.
    drop(bc);
    // `registry` still owns an Arc to the broadcaster in its
    // internal HashMap. Dropping the registry severs that Arc
    // too, letting the broadcaster fully tear down and our
    // drain task observe the close.
    drop(registry);

    assert!(
        wait_until(
            || async { b.find_broadcast_owner("live/ephemeral").await.is_none() },
            Duration::from_secs(5),
        )
        .await,
        "B never saw the auto-claim released after the broadcaster closed"
    );

    // Drop the local Arc<Cluster> -- the spawned auto-claim task
    // still holds its own clone. The background chitchat server is
    // torn down when the Cluster's last Arc drops via `Cluster::Drop`
    // (which cancels + aborts the advertiser) and the
    // `ChitchatHandle` drop.
    drop(a);
    b.shutdown().await.expect("B shutdown");
}
