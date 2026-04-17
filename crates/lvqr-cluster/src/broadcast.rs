//! Broadcast-ownership KV surface (Tier 3 session C).
//!
//! A broadcast name -- for example `"live/test"` -- is claimed by
//! exactly one LVQR node at a time. The claim is published via
//! chitchat's KV so other nodes can discover who owns a broadcast
//! without maintaining their own directory. See
//! `tracking/TIER_3_PLAN.md` for the tier-level decomposition.
//!
//! ## Wire shape
//!
//! Each claim is written to the *owner node's* chitchat state under
//! the key `broadcast.<name>` with a JSON value of the form:
//!
//! ```text
//! {"owner":"<node-id>","expires_at_ms":<unix-ms-deadline>}
//! ```
//!
//! The owner node is also the only writer: chitchat KV is per-node,
//! so `find_broadcast_owner` iterates every known node's state and
//! picks the first non-expired entry. If two nodes both claimed the
//! same name during a brief partition the reader sees both entries
//! and breaks the tie by picking the latest expiry, then by the
//! owner string -- this is eventual consistency, not a distributed
//! lock.
//!
//! ## Lease lifecycle
//!
//! * `claim_broadcast` writes the initial lease synchronously so
//!   callers of `find_broadcast_owner` on the same node see the
//!   claim immediately.
//! * A renewer task spawned from the claim rewrites the key every
//!   `lease / 4` so transient gossip loss does not evict the lease.
//! * Dropping [`Claim`] sends a best-effort stop signal over a
//!   `oneshot::Sender`. The renewer task receives the signal, marks
//!   the key for deletion via chitchat's tombstone API, and exits.
//!   Drop is synchronous so the actual delete happens on the
//!   renewer's async runtime instead of blocking the caller.
//! * If the renewer task is already gone (e.g. the runtime is
//!   shutting down), the signal is lost and the lease expires
//!   naturally at `expires_at_ms`. Peers converge on "no owner"
//!   after the expiry passes.
//!
//! LBD #5 (chitchat scope discipline): this surface carries a
//! *pointer* to the owner, not per-frame counters or per-subscriber
//! bitrate. Fast-changing state stays node-local.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use chitchat::{Chitchat, ChitchatHandle};
use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, oneshot};
use tokio::task::JoinHandle;
use tracing::{debug, warn};

use crate::NodeId;

/// Prefix all broadcast-ownership KV keys share. Chosen to be
/// unambiguous in the chitchat admin dump (`broadcast.live/test`
/// reads cleanly) and to keep a single `iter_prefix("broadcast.")`
/// call sufficient for future bulk-listing endpoints.
pub const BROADCAST_KEY_PREFIX: &str = "broadcast.";

/// Minimum lease duration. Leases shorter than this quantize to
/// renew intervals below chitchat's gossip cadence, which wastes
/// CPU without buying liveness. Callers asking for a shorter lease
/// are bumped up.
pub const MIN_LEASE: Duration = Duration::from_millis(100);

/// Build the KV key for `name`. Private; callers should go through
/// [`Cluster::claim_broadcast`](crate::Cluster::claim_broadcast)
/// / [`Cluster::find_broadcast_owner`](crate::Cluster::find_broadcast_owner).
pub(crate) fn broadcast_key(name: &str) -> String {
    format!("{BROADCAST_KEY_PREFIX}{name}")
}

/// Millisecond-precision unix timestamp. Saturates to 0 if the
/// system clock is before the epoch; that is a misconfigured host
/// and the resulting `expires_at_ms = 0` makes every lease look
/// expired, which is the safe failure mode.
fn to_unix_ms(t: SystemTime) -> u64 {
    t.duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

fn now_unix_ms() -> u64 {
    to_unix_ms(SystemTime::now())
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct Lease {
    owner: String,
    expires_at_ms: u64,
}

impl Lease {
    fn encode(&self) -> Result<String> {
        serde_json::to_string(self).context("serialize broadcast lease")
    }

    fn decode(raw: &str) -> Option<Self> {
        serde_json::from_str(raw).ok()
    }
}

/// Write the initial lease on `chitchat`'s self node state for
/// `name`. Split out so the claim path can be unit-tested in
/// isolation from the renewer loop.
fn write_lease(chitchat: &mut Chitchat, key: &str, owner: &str, expires_at_ms: u64) -> Result<()> {
    let lease = Lease {
        owner: owner.to_string(),
        expires_at_ms,
    };
    let encoded = lease.encode()?;
    chitchat.self_node_state().set(key, encoded);
    Ok(())
}

fn delete_lease(chitchat: &mut Chitchat, key: &str) {
    // `delete` warns + no-ops if the key never existed on this node.
    // We only call it from the renewer task (which wrote the key on
    // bootstrap) so the warn path is unreachable in practice.
    chitchat.self_node_state().delete(key);
}

/// Handle to an active broadcast claim.
///
/// Holding this value keeps the lease renewed in the background.
/// Dropping it fires a best-effort tombstone write so peers see the
/// broadcast slot freed within one gossip round; if the tombstone
/// write is lost the lease still expires at `expires_at_ms` of the
/// most recently gossipped value.
///
/// `Claim` is intentionally *not* `Clone`: the semantic is
/// single-ownership, and cloning would make it ambiguous which drop
/// releases the lease.
pub struct Claim {
    /// Broadcast name this claim covers.
    pub broadcast: String,
    /// Node that owns the lease. Always the self node of the
    /// cluster that produced this claim.
    pub owner: NodeId,
    /// Initial lease expiry. The renewer task extends the on-wire
    /// deadline transparently, so the actual expiry observed by
    /// peers is always `SystemTime::now() + lease` within one
    /// renewal interval. Exposed as the first deadline rather than
    /// a live-updating handle because callers that need real-time
    /// expiry should query chitchat directly via
    /// `Cluster::find_broadcast_owner`.
    pub expires_at: SystemTime,
    /// Sends on this channel to ask the renewer to tombstone the
    /// key and exit. Wrapped in an `Option` so `Drop` can take
    /// ownership.
    stop: Option<oneshot::Sender<()>>,
    /// JoinHandle for the renewer task. Aborted on `Drop` as a
    /// belt-and-braces measure if the stop channel fails.
    task: Option<JoinHandle<()>>,
}

impl std::fmt::Debug for Claim {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Claim")
            .field("broadcast", &self.broadcast)
            .field("owner", &self.owner)
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

impl Drop for Claim {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            // `send` failing means the renewer task already exited
            // (e.g. the runtime was shut down). Nothing to do; the
            // lease will expire on its own.
            let _ = stop.send(());
        }
        if let Some(task) = self.task.take() {
            // The renewer should exit cleanly after receiving the
            // stop signal. `abort` is a no-op if the task already
            // finished and guarantees no dangling task if the stop
            // channel was closed before sending.
            task.abort();
        }
    }
}

/// Spawn a renewer task that extends `key` every `renew_interval`
/// until the returned `oneshot::Sender` is signalled. On stop the
/// task writes a tombstone (`delete`) and exits.
fn spawn_renewer(
    chitchat: Arc<Mutex<Chitchat>>,
    key: String,
    owner: String,
    lease: Duration,
    renew_interval: Duration,
) -> (oneshot::Sender<()>, JoinHandle<()>) {
    let (stop_tx, mut stop_rx) = oneshot::channel::<()>();
    let handle = tokio::spawn(async move {
        let start = tokio::time::Instant::now() + renew_interval;
        let mut ticker = tokio::time::interval_at(start, renew_interval);
        // Delay on missed ticks instead of bursting catch-ups: if the
        // chitchat mutex was contended we would rather publish one
        // fresh value than replay a backlog.
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                biased;
                _ = &mut stop_rx => {
                    let mut guard = chitchat.lock().await;
                    delete_lease(&mut guard, &key);
                    debug!(%key, "broadcast claim released");
                    return;
                }
                _ = ticker.tick() => {
                    let new_expiry_ms = now_unix_ms().saturating_add(lease.as_millis() as u64);
                    let mut guard = chitchat.lock().await;
                    if let Err(err) = write_lease(&mut guard, &key, &owner, new_expiry_ms) {
                        warn!(%err, %key, "failed to renew broadcast lease");
                    }
                }
            }
        }
    });
    (stop_tx, handle)
}

/// Claim `name` for the duration of `lease`. Writes the initial
/// value synchronously on `handle`'s self node state, then spawns a
/// renewer that rewrites the key every `lease / 4` until the
/// returned [`Claim`] is dropped.
pub(crate) async fn claim(handle: &ChitchatHandle, owner: &NodeId, name: &str, lease: Duration) -> Result<Claim> {
    let effective_lease = lease.max(MIN_LEASE);
    let key = broadcast_key(name);
    let owner_str = owner.as_str().to_string();
    let now = SystemTime::now();
    let expires_at = now + effective_lease;
    let expires_at_ms = to_unix_ms(expires_at);

    handle
        .with_chitchat(|c| write_lease(c, &key, &owner_str, expires_at_ms))
        .await
        .context("publish initial broadcast lease")?;

    let renew_interval = effective_lease / 4;
    let (stop, task) = spawn_renewer(
        handle.chitchat(),
        key,
        owner_str,
        effective_lease,
        renew_interval.max(Duration::from_millis(10)),
    );

    Ok(Claim {
        broadcast: name.to_string(),
        owner: owner.clone(),
        expires_at,
        stop: Some(stop),
        task: Some(task),
    })
}

/// Pick the winning lease from a stream of candidates, filtering
/// out entries whose `expires_at_ms` is at or before `now_ms`.
///
/// Tie-breaking when multiple nodes claim the same broadcast (a
/// transient condition during partition healing): the entry with
/// the highest `expires_at_ms` wins; if those match, the owner
/// string with the lexicographically largest value wins. This is
/// deterministic across nodes so peers converge on the same answer
/// without coordination.
fn select_winner<I>(candidates: I, now_ms: u64) -> Option<Lease>
where
    I: IntoIterator<Item = Lease>,
{
    let mut best: Option<Lease> = None;
    for lease in candidates {
        if lease.expires_at_ms <= now_ms {
            continue;
        }
        match &best {
            None => best = Some(lease),
            Some(current) => {
                if lease.expires_at_ms > current.expires_at_ms
                    || (lease.expires_at_ms == current.expires_at_ms && lease.owner > current.owner)
                {
                    best = Some(lease);
                }
            }
        }
    }
    best
}

/// Resolve the current non-expired owner of `name` by scanning every
/// known node's state for the `broadcast.<name>` key. Returns `None`
/// if no live lease exists.
pub(crate) async fn find_owner(handle: &ChitchatHandle, name: &str) -> Option<NodeId> {
    let key = broadcast_key(name);
    let now_ms = now_unix_ms();
    handle
        .with_chitchat(|c| {
            let candidates = c.node_states().values().filter_map(|state| {
                let raw = state.get(&key)?;
                match Lease::decode(raw) {
                    Some(lease) => Some(lease),
                    None => {
                        warn!(%key, raw, "broadcast KV entry failed to decode; skipping");
                        None
                    }
                }
            });
            select_winner(candidates, now_ms).map(|l| NodeId::new(l.owner))
        })
        .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn broadcast_key_includes_prefix() {
        assert_eq!(broadcast_key("live/test"), "broadcast.live/test");
        assert_eq!(broadcast_key(""), "broadcast.");
    }

    #[test]
    fn lease_roundtrip_json() {
        let lease = Lease {
            owner: "node-x".to_string(),
            expires_at_ms: 1_700_000_000_000,
        };
        let encoded = lease.encode().expect("encode");
        let decoded = Lease::decode(&encoded).expect("decode");
        assert_eq!(lease, decoded);
    }

    #[test]
    fn lease_decode_rejects_garbage() {
        assert!(Lease::decode("not json").is_none());
        assert!(Lease::decode("{\"owner\":42}").is_none());
    }

    #[test]
    fn to_unix_ms_past_epoch_saturates() {
        let past = UNIX_EPOCH - Duration::from_secs(1);
        assert_eq!(to_unix_ms(past), 0);
    }

    #[test]
    fn to_unix_ms_monotonic() {
        let a = to_unix_ms(UNIX_EPOCH + Duration::from_millis(1));
        let b = to_unix_ms(UNIX_EPOCH + Duration::from_millis(2));
        assert!(b > a);
    }

    fn lease(owner: &str, expires_at_ms: u64) -> Lease {
        Lease {
            owner: owner.to_string(),
            expires_at_ms,
        }
    }

    #[test]
    fn select_winner_returns_none_for_empty() {
        assert!(select_winner::<Vec<_>>(vec![], 1_000).is_none());
    }

    #[test]
    fn select_winner_filters_expired_leases() {
        // Two expired + one live: only the live one survives.
        let now = 1_000;
        let candidates = vec![
            lease("expired-1", 500),
            lease("live", 2_000),
            lease("expired-2", 1_000), // equal-to-now counts as expired
        ];
        let winner = select_winner(candidates, now).expect("a winner");
        assert_eq!(winner.owner, "live");
    }

    #[test]
    fn select_winner_returns_none_when_all_expired() {
        let now = 5_000;
        let candidates = vec![lease("a", 1_000), lease("b", 2_000)];
        assert!(select_winner(candidates, now).is_none());
    }

    #[test]
    fn select_winner_prefers_later_expiry() {
        let now = 1_000;
        let candidates = vec![lease("earlier", 2_000), lease("later", 3_000)];
        let winner = select_winner(candidates, now).expect("a winner");
        assert_eq!(winner.owner, "later");
    }

    #[test]
    fn select_winner_breaks_tie_by_owner_string() {
        // Deterministic across nodes because every reader iterates
        // the same BTreeMap and applies the same tiebreak.
        let now = 1_000;
        let candidates = vec![lease("alpha", 2_000), lease("bravo", 2_000)];
        let winner = select_winner(candidates, now).expect("a winner");
        assert_eq!(winner.owner, "bravo");
    }
}
