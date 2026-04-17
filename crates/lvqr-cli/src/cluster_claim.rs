//! Ingest auto-claim bridge (Tier 3 session F2c).
//!
//! Installs a callback on the shared
//! [`lvqr_fragment::FragmentBroadcasterRegistry`] so every new
//! `(broadcast, _)` pair automatically takes a
//! [`lvqr_cluster::Claim`] against the cluster. The claim is
//! held for the lifetime of the broadcaster: as soon as every
//! ingest publisher for that broadcast disconnects the drain
//! task observes the close and drops the claim, which tombstones
//! the KV entry so peers see the slot freed within one gossip
//! round.
//!
//! ## Why this lives in `lvqr-cli`
//!
//! The auto-claim path crosses two orthogonal abstractions:
//! `lvqr-fragment` (the protocol-agnostic broadcaster registry)
//! and `lvqr-cluster` (the gossip plane). Neither crate should
//! depend on the other -- `lvqr-fragment` has no need to know
//! about clustering, and `lvqr-cluster` is deliberately
//! protocol-agnostic per LBD #5. The CLI crate is where the
//! two wires meet in a full LVQR deployment, so the glue lives
//! here.
//!
//! ## Deduplication
//!
//! A single logical broadcast typically produces two entries
//! (`0.mp4` video and `1.mp4` audio). The bridge dedups on the
//! broadcast name so exactly one `Claim` exists per broadcast
//! regardless of track count. A later re-publish (after the
//! first claim has released) can re-claim because the dedup set
//! is cleaned up when the drain task exits.
//!
//! ## Session 64 invariants
//!
//! The spawned drain task owns a `BroadcasterStream` only and
//! never a strong `Arc<FragmentBroadcaster>`. Dropping the last
//! ingest publisher clone tears the broadcaster down, the
//! drain's `.next_fragment()` returns `None`, and the loop
//! exits.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use lvqr_cluster::Cluster;
use lvqr_fragment::{FragmentBroadcasterRegistry, FragmentStream};
use tokio::runtime::Handle;
use tracing::{info, warn};

/// Default lease duration for an auto-claimed broadcast. Ten
/// seconds with the 2.5 s renew interval (lease / 4) the
/// [`lvqr_cluster::Cluster::claim_broadcast`] path already uses
/// matches the rule-of-thumb from `tracking/TIER_3_PLAN.md`
/// session 71's resolved-questions block: "lease > 3× renew >
/// gossip".
pub const DEFAULT_CLAIM_LEASE: Duration = Duration::from_secs(10);

/// Install an `on_entry_created` callback that auto-claims every
/// new broadcast on `cluster` for `lease`, holding the claim
/// until every ingest publisher for that broadcast disconnects.
///
/// Idempotent: multiple calls install multiple callbacks, so
/// callers should invoke this exactly once per
/// `(cluster, registry)` pair. `lvqr-cli::start` enforces this
/// with a `cfg(feature = "cluster")` check and a single
/// invocation guarded by `config.cluster_listen.is_some()`.
pub fn install_cluster_claim_bridge(cluster: Arc<Cluster>, lease: Duration, registry: &FragmentBroadcasterRegistry) {
    let claimed: Arc<Mutex<HashSet<String>>> = Arc::new(Mutex::new(HashSet::new()));
    registry.on_entry_created(move |broadcast, _track, bc| {
        // Dedup on broadcast name only: the two tracks of a
        // single broadcast share ownership.
        {
            let mut set = claimed.lock().expect("cluster-claim dedup set poisoned");
            if !set.insert(broadcast.to_string()) {
                return;
            }
        }

        // Subscribe synchronously inside the callback so no emit
        // can race ahead of the drain loop; same rationale the
        // HLS and DASH bridges document.
        let sub = bc.subscribe();
        let broadcast_name = broadcast.to_string();
        let cluster = cluster.clone();
        let claimed = claimed.clone();
        let Ok(handle) = Handle::try_current() else {
            warn!(broadcast = %broadcast_name, "no tokio handle; cluster claim bridge inactive for this broadcast");
            claimed
                .lock()
                .expect("cluster-claim dedup set poisoned")
                .remove(&broadcast_name);
            return;
        };
        handle.spawn(async move {
            let claim = match cluster.claim_broadcast(&broadcast_name, lease).await {
                Ok(c) => c,
                Err(err) => {
                    warn!(
                        error = %err,
                        broadcast = %broadcast_name,
                        "cluster auto-claim failed; peer redirect disabled for this session",
                    );
                    claimed
                        .lock()
                        .expect("cluster-claim dedup set poisoned")
                        .remove(&broadcast_name);
                    return;
                }
            };
            info!(
                broadcast = %broadcast_name,
                owner = %claim.owner,
                "cluster auto-claim installed",
            );

            // Hold the claim alive until the broadcaster closes.
            // The BroadcasterStream drop-detection is the
            // authoritative signal: when the last ingest
            // publisher disconnects, `next_fragment()` returns
            // `None` and we fall out of the loop.
            let mut sub = sub;
            while sub.next_fragment().await.is_some() {}

            // Producers all gone: dropping `claim` triggers the
            // best-effort tombstone write via the renewer's
            // oneshot stop channel, so peers see the broadcast
            // freed within one gossip round. Remove from the
            // dedup set so a future re-publish re-claims.
            drop(claim);
            claimed
                .lock()
                .expect("cluster-claim dedup set poisoned")
                .remove(&broadcast_name);
            info!(
                broadcast = %broadcast_name,
                "cluster auto-claim released (broadcaster closed)",
            );
        });
    });
}
