//! Tier 3 session F2 end-to-end test: HLS redirect-to-owner.
//!
//! Starts two in-process `lvqr-cli` servers over real UDP loopback
//! chitchat transport, claims a broadcast and advertises endpoints
//! on A, then confirms that B's HLS handler responds with a
//! `302 Found` carrying a `Location` pointing at A when a subscriber
//! asks for A's broadcast through B.
//!
//! This exercises the full wire path:
//!
//! * `Cluster::claim_broadcast` on A publishes the broadcast
//!   ownership KV.
//! * `Cluster::set_endpoints` on A publishes A's advertised HLS URL.
//! * chitchat gossip carries both entries to B.
//! * The `OwnerResolver` closure wired into B's `MultiHlsServer`
//!   by `lvqr_cli::start` calls `Cluster::find_owner_endpoints`.
//! * `handle_multi_get` in `lvqr-hls` sees the broadcast is unknown
//!   locally, consults the resolver, and emits the 302.
//!
//! Feature-gated on `cluster` (default-on). Skipped when the
//! feature is disabled.

#![cfg(feature = "cluster")]

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::Duration;

use lvqr_cli::{ServeConfig, start};
use lvqr_cluster::NodeEndpoints;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

const GOSSIP_PORT_A: u16 = 20801;
const GOSSIP_PORT_B: u16 = 20802;

fn cluster_aware_config(cluster_listen_port: u16, seeds: Vec<String>, node_id: &str) -> ServeConfig {
    let mut cfg = ServeConfig::loopback_ephemeral();
    cfg.cluster_listen = Some(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), cluster_listen_port));
    cfg.cluster_seeds = seeds;
    cfg.cluster_node_id = Some(node_id.to_string());
    cfg.cluster_id = Some("lvqr-test-f2".to_string());
    cfg
}

/// Poll `probe` until it returns true or `timeout` elapses.
async fn wait_until<F, Fut>(mut probe: F, timeout: Duration) -> bool
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let start_t = tokio::time::Instant::now();
    while start_t.elapsed() < timeout {
        if probe().await {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    false
}

/// Send an HTTP/1.1 GET to `addr` for `path` and return
/// `(status, location_header)`. `Connection: close` so the server
/// closes immediately and we can read until EOF.
async fn http_get_raw(addr: SocketAddr, path: &str) -> (u16, Option<String>) {
    let mut stream = TcpStream::connect(addr).await.expect("tcp connect");
    let req = format!(
        "GET {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n",
        host = addr,
    );
    stream.write_all(req.as_bytes()).await.expect("write req");
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).await.expect("read resp");
    let text = String::from_utf8_lossy(&buf);
    let mut lines = text.split("\r\n");
    let status_line = lines.next().unwrap_or_default();
    let status: u16 = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let mut location: Option<String> = None;
    for line in lines {
        if line.is_empty() {
            break;
        }
        if let Some((k, v)) = line.split_once(": ") {
            if k.eq_ignore_ascii_case("location") {
                location = Some(v.to_string());
            }
        }
    }
    (status, location)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn hls_redirect_to_cluster_peer_end_to_end() {
    // Start A with an ephemeral HLS port and a fixed cluster port.
    let a_handle = start(cluster_aware_config(GOSSIP_PORT_A, vec![], "node-a"))
        .await
        .expect("start A");
    let a_hls_addr = a_handle.hls_addr().expect("A HLS bound");
    let a_cluster = a_handle.cluster().cloned().expect("A cluster handle");

    // Advertise A's real HLS URL now that the ephemeral port is
    // known. In a production deployment the operator would pass
    // `--cluster-advertise-hls=http://public.host:8888` at startup
    // instead; this test drives the same KV write via the public
    // `set_endpoints` API.
    let a_hls_url = format!("http://{a_hls_addr}");
    a_cluster
        .set_endpoints(&NodeEndpoints {
            hls: Some(a_hls_url.clone()),
            dash: None,
            rtsp: None,
        })
        .await
        .expect("A set_endpoints");

    // Claim the broadcast on A. The `Claim` keeps the lease fresh
    // via a background renewer; dropping it tombstones the key.
    let _claim = a_cluster
        .claim_broadcast("live/test", Duration::from_secs(10))
        .await
        .expect("A claim");

    // Start B, seeded with A's gossip address.
    let b_handle = start(cluster_aware_config(
        GOSSIP_PORT_B,
        vec![format!("127.0.0.1:{GOSSIP_PORT_A}")],
        "node-b",
    ))
    .await
    .expect("start B");
    let b_hls_addr = b_handle.hls_addr().expect("B HLS bound");
    let b_cluster = b_handle.cluster().cloned().expect("B cluster handle");

    // Wait for B to resolve the broadcast owner + its endpoints via
    // gossip. Both pieces are needed before the HLS handler can
    // emit a redirect.
    let converged = wait_until(
        || {
            let b_cluster = b_cluster.clone();
            let expected_url = a_hls_url.clone();
            async move {
                match b_cluster.find_owner_endpoints("live/test").await {
                    Some((_, endpoints)) => endpoints.hls.as_deref() == Some(&expected_url),
                    None => false,
                }
            }
        },
        Duration::from_secs(10),
    )
    .await;
    assert!(converged, "B never resolved A's owner endpoints");

    // GET the HLS master playlist for live/test on B. The broadcast
    // is unknown locally (no ingest has published to B) so the
    // handler should consult the owner resolver and return 302 with
    // Location = "<a_hls_url>/hls/live/test/master.m3u8".
    let (status, location) = http_get_raw(b_hls_addr, "/hls/live/test/master.m3u8").await;
    assert_eq!(status, 302, "expected 302, got {status}");
    let loc = location.expect("Location header present on 302");
    let expected_location = format!("{a_hls_url}/hls/live/test/master.m3u8");
    assert_eq!(loc, expected_location);

    // A request for a path with no local broadcaster AND no cluster
    // owner falls through to the existing 404 path.
    let (status_404, _) = http_get_raw(b_hls_addr, "/hls/live/unclaimed/master.m3u8").await;
    assert_eq!(status_404, 404, "expected 404, got {status_404}");

    // Clean teardown: drop the claim first so the renewer runs its
    // tombstone path, then shut both servers down (which each tear
    // down their cluster handles via `ServerHandle::shutdown`).
    drop(_claim);
    // Drop cluster arc refs before shutdown so Cluster::Arc drops
    // cleanly inside shutdown's own teardown.
    drop(a_cluster);
    drop(b_cluster);
    a_handle.shutdown().await.expect("A shutdown");
    b_handle.shutdown().await.expect("B shutdown");
}
