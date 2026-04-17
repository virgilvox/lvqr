//! Cluster-wide config channel (Tier 3 session E).
//!
//! A narrow set of feature flags -- for example
//! `hls.low-latency.enabled`, `rtsp.tcp.interleaved.enabled` -- is
//! gossipped through chitchat KV so a single configuration change
//! converges across every node without a restart. Node-local
//! config (ports, keys, TLS certs) stays in TOML; only
//! cluster-wide knobs flow through this channel.
//!
//! ## Wire shape
//!
//! Each entry is written to the *setting* node's chitchat KV under
//! `config.<key>` with a JSON value:
//!
//! ```text
//! {"value":"<string>","ts_ms":<unix-ms>}
//! ```
//!
//! ## Cluster-wide semantics under eventual consistency
//!
//! chitchat KV is per-node. "Setting a cluster-wide config" here
//! means: the caller's node writes the entry into its own state;
//! gossip carries the entry to every other node; readers iterate
//! every known node's state under the `config.<key>` prefix and
//! resolve conflicts by picking the highest `ts_ms`. Ties break
//! lexicographically on the value string so every reader converges
//! on the same answer without coordination.
//!
//! Implications:
//! * Writes always succeed locally. Conflict resolution happens on
//!   read. Two operators setting the same key at the same time on
//!   different nodes produce two entries; the later timestamp wins
//!   across the cluster.
//! * Clock skew affects tiebreaks. The LVQR target is a single LAN
//!   where skew is bounded to milliseconds by NTP; that is
//!   sufficient for feature-flag granularity.
//! * Deletion is not supported in this session. To disable a
//!   feature flag, set the key to a sentinel value (e.g. `"off"`).
//!   A future session can add an explicit `config_delete` that
//!   writes a tombstone.
//!
//! LBD #5 (chitchat scope discipline): config is a slow-moving
//! control surface. Per-fragment state does not flow through here.

use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use chitchat::ChitchatHandle;
use serde::{Deserialize, Serialize};
use tracing::warn;

/// Prefix every cluster-wide config KV entry shares. Chosen to
/// live in the same flat namespace as `broadcast.*` so a future
/// admin dump can filter by prefix with a single `iter_prefix`
/// call.
pub const CONFIG_KEY_PREFIX: &str = "config.";

pub(crate) fn config_key(key: &str) -> String {
    format!("{CONFIG_KEY_PREFIX}{key}")
}

fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct ConfigValue {
    value: String,
    ts_ms: u64,
}

impl ConfigValue {
    fn encode(&self) -> Result<String> {
        serde_json::to_string(self).context("serialize config entry")
    }

    fn decode(raw: &str) -> Option<Self> {
        serde_json::from_str(raw).ok()
    }
}

/// One cluster-wide config entry, as returned by
/// [`Cluster::list_config`](crate::Cluster::list_config).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigEntry {
    /// Config key without the `config.` prefix.
    pub key: String,
    /// Current value, per cross-node LWW tiebreak.
    pub value: String,
    /// Wall-clock millisecond timestamp the winning entry was
    /// written with. Callers can use this to reason about
    /// staleness without peeking at the wire shape.
    pub ts_ms: u64,
}

/// Write `value` under `config.<key>` on the local node's state.
/// Gossip then carries the entry to every peer. Conflicts with
/// concurrent writes elsewhere in the cluster resolve by LWW on
/// `ts_ms` when readers call [`get`].
pub(crate) async fn set(handle: &ChitchatHandle, key: &str, value: &str) -> Result<()> {
    let full_key = config_key(key);
    let entry = ConfigValue {
        value: value.to_string(),
        ts_ms: now_unix_ms(),
    };
    let encoded = entry.encode()?;
    handle
        .with_chitchat(|c| {
            c.self_node_state().set(full_key.as_str(), encoded.as_str());
        })
        .await;
    Ok(())
}

/// Resolve the current cluster-wide value of `key`. Returns `None`
/// if no node has ever written the key.
pub(crate) async fn get(handle: &ChitchatHandle, key: &str) -> Option<String> {
    let full_key = config_key(key);
    handle
        .with_chitchat(|c| {
            let candidates = c.node_states().values().filter_map(|state| {
                let raw = state.get(&full_key)?;
                match ConfigValue::decode(raw) {
                    Some(v) => Some(v),
                    None => {
                        warn!(%full_key, raw, "config entry failed to decode; skipping");
                        None
                    }
                }
            });
            select_winner(candidates).map(|v| v.value)
        })
        .await
}

/// Enumerate every cluster-wide config key any node has ever set,
/// reduced to the LWW winner per key. Sorted by key for stable
/// admin output.
pub(crate) async fn list(handle: &ChitchatHandle) -> Vec<ConfigEntry> {
    handle
        .with_chitchat(|c| {
            let mut winners: std::collections::BTreeMap<String, ConfigValue> = Default::default();
            for state in c.node_states().values() {
                for (raw_key, raw_value) in state.iter_prefix(CONFIG_KEY_PREFIX) {
                    let stripped = match raw_key.strip_prefix(CONFIG_KEY_PREFIX) {
                        Some(k) => k.to_string(),
                        None => continue,
                    };
                    let Some(decoded) = ConfigValue::decode(&raw_value.value) else {
                        warn!(full_key = %raw_key, "config entry failed to decode; skipping");
                        continue;
                    };
                    match winners.get(&stripped) {
                        None => {
                            winners.insert(stripped, decoded);
                        }
                        Some(current) => {
                            if decoded.ts_ms > current.ts_ms
                                || (decoded.ts_ms == current.ts_ms && decoded.value > current.value)
                            {
                                winners.insert(stripped, decoded);
                            }
                        }
                    }
                }
            }
            winners
                .into_iter()
                .map(|(key, v)| ConfigEntry {
                    key,
                    value: v.value,
                    ts_ms: v.ts_ms,
                })
                .collect()
        })
        .await
}

/// Pure-function LWW tiebreak. Latest `ts_ms` wins; on ties, the
/// lexicographically larger `value` wins so every reader converges
/// on the same answer.
fn select_winner<I>(candidates: I) -> Option<ConfigValue>
where
    I: IntoIterator<Item = ConfigValue>,
{
    let mut best: Option<ConfigValue> = None;
    for entry in candidates {
        match &best {
            None => best = Some(entry),
            Some(current) => {
                if entry.ts_ms > current.ts_ms || (entry.ts_ms == current.ts_ms && entry.value > current.value) {
                    best = Some(entry);
                }
            }
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(value: &str, ts_ms: u64) -> ConfigValue {
        ConfigValue {
            value: value.to_string(),
            ts_ms,
        }
    }

    #[test]
    fn config_key_includes_prefix() {
        assert_eq!(config_key("hls.low-latency.enabled"), "config.hls.low-latency.enabled");
        assert_eq!(config_key(""), "config.");
    }

    #[test]
    fn config_value_roundtrip_json() {
        let v = entry("true", 1_700_000_000_000);
        let encoded = v.encode().expect("encode");
        let decoded = ConfigValue::decode(&encoded).expect("decode");
        assert_eq!(v, decoded);
    }

    #[test]
    fn config_value_decode_rejects_garbage() {
        assert!(ConfigValue::decode("not json").is_none());
        assert!(ConfigValue::decode("{\"value\":42}").is_none());
    }

    #[test]
    fn select_winner_empty_returns_none() {
        assert!(select_winner::<Vec<_>>(vec![]).is_none());
    }

    #[test]
    fn select_winner_prefers_latest_ts() {
        let cands = vec![entry("old", 1_000), entry("new", 2_000)];
        assert_eq!(select_winner(cands).unwrap().value, "new");
    }

    #[test]
    fn select_winner_breaks_tie_by_value_lexicographic() {
        // Two writes at the exact same millisecond from different
        // nodes -- every reader must agree on the winner without
        // coordination.
        let cands = vec![entry("alpha", 1_000), entry("bravo", 1_000)];
        assert_eq!(select_winner(cands).unwrap().value, "bravo");
    }

    #[test]
    fn select_winner_deterministic_across_insertion_order() {
        let a = vec![entry("alpha", 2_000), entry("bravo", 1_000)];
        let b = vec![entry("bravo", 1_000), entry("alpha", 2_000)];
        assert_eq!(select_winner(a).unwrap().value, "alpha");
        assert_eq!(select_winner(b).unwrap().value, "alpha");
    }
}
