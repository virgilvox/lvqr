//! Per-node endpoint advertisement (Tier 3 session F1).
//!
//! Each LVQR node serves egress protocols (HLS, DASH, RTSP, ...) on
//! URLs that are not derivable from its chitchat gossip address.
//! When a subscriber hits node B asking for a broadcast owned by A,
//! the redirect-to-owner path in `lvqr-hls` / `lvqr-dash` /
//! `lvqr-rtsp` needs to know A's HLS / DASH / RTSP URL.
//!
//! This module adds a single `endpoints` KV entry per node, holding
//! the node's externally-reachable URLs. Peers read the entry via
//! [`ClusterNode::endpoints`](crate::ClusterNode::endpoints) when
//! iterating [`Cluster::members`](crate::Cluster::members), and via
//! the higher-level [`Cluster::find_owner_endpoints`](crate::Cluster::find_owner_endpoints)
//! helper when resolving a redirect target for a broadcast.
//!
//! ## Wire shape
//!
//! One entry per node under the key `endpoints`:
//!
//! ```text
//! {"hls":"http://a.local:8888","dash":null,"rtsp":null}
//! ```
//!
//! All three URLs are optional; a node that does not serve HLS
//! simply leaves `hls` as `None`.
//!
//! ## Why per-node, not cluster-wide LWW
//!
//! Unlike [`config`](crate::config), endpoints are per-node identity
//! data -- two nodes with the same `hls` URL is a misconfiguration,
//! not a conflict-resolution problem. The reader iterates each
//! peer's state and returns what that peer itself advertised; there
//! is no cross-node tiebreak because there is no cross-node
//! ambiguity.
//!
//! LBD #5 (chitchat scope discipline): endpoints change only on
//! restart or config reload. Not per-fragment state.

use anyhow::{Context, Result};
use chitchat::ChitchatHandle;
use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::NodeId;

/// Well-known KV key each node uses for its endpoint entry.
pub const ENDPOINTS_KEY: &str = "endpoints";

/// External URLs one node advertises. All fields are optional so a
/// node that serves only a subset of egress protocols leaves the
/// others as `None`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeEndpoints {
    /// Base URL for HLS. Example: `http://a.local:8888`. Egress
    /// paths (e.g. `/hls/<broadcast>/master.m3u8`) are appended by
    /// the redirect resolver; the value here should NOT include a
    /// trailing slash or a broadcast path.
    pub hls: Option<String>,
    /// Base URL for DASH. Example: `http://a.local:8888`. Same
    /// rules as `hls`: no trailing slash, no broadcast path.
    pub dash: Option<String>,
    /// Base URL for RTSP. Example: `rtsp://a.local:8554`. Same
    /// rules: no trailing slash.
    pub rtsp: Option<String>,
}

impl NodeEndpoints {
    /// Returns `true` if every field is `None`. Nodes advertising
    /// an empty endpoints struct generally should not -- the
    /// publisher on that node will never be reachable by redirects.
    pub fn is_empty(&self) -> bool {
        self.hls.is_none() && self.dash.is_none() && self.rtsp.is_none()
    }

    /// Decode a chitchat KV value into an endpoints struct. Returns
    /// `None` on any decode failure; callers treat that as "no
    /// endpoints advertised" and fall back to 404 / error.
    pub fn decode(raw: &str) -> Option<Self> {
        serde_json::from_str(raw).ok()
    }

    fn encode(&self) -> Result<String> {
        serde_json::to_string(self).context("serialize node endpoints")
    }
}

/// Write `endpoints` onto the self node's chitchat state. Gossip
/// then carries the value to every peer. Overwrites the previous
/// entry; callers that want to update a single field should read
/// the current value first, mutate, and pass the full struct here.
pub(crate) async fn set(handle: &ChitchatHandle, endpoints: &NodeEndpoints) -> Result<()> {
    let encoded = endpoints.encode()?;
    handle
        .with_chitchat(|c| {
            c.self_node_state().set(ENDPOINTS_KEY, encoded.as_str());
        })
        .await;
    Ok(())
}

/// Look up `node_id`'s advertised endpoints. Returns `None` if the
/// node is unknown, the entry has not been gossipped yet, or the
/// entry failed to decode.
pub(crate) async fn get(handle: &ChitchatHandle, node_id: &NodeId) -> Option<NodeEndpoints> {
    handle
        .with_chitchat(|c| {
            for (cid, state) in c.node_states() {
                if cid.node_id != node_id.as_str() {
                    continue;
                }
                let raw = state.get(ENDPOINTS_KEY)?;
                return match NodeEndpoints::decode(raw) {
                    Some(e) => Some(e),
                    None => {
                        warn!(node = %node_id, raw, "endpoints entry failed to decode; skipping");
                        None
                    }
                };
            }
            None
        })
        .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_empty_detects_all_none() {
        assert!(NodeEndpoints::default().is_empty());
        let with_hls = NodeEndpoints {
            hls: Some("http://x:1".into()),
            ..Default::default()
        };
        assert!(!with_hls.is_empty());
    }

    #[test]
    fn endpoints_roundtrip_json() {
        let e = NodeEndpoints {
            hls: Some("http://a.local:8888".into()),
            dash: Some("http://a.local:8888".into()),
            rtsp: Some("rtsp://a.local:8554".into()),
        };
        let encoded = e.encode().expect("encode");
        let decoded = NodeEndpoints::decode(&encoded).expect("decode");
        assert_eq!(decoded, e);
    }

    #[test]
    fn endpoints_decode_rejects_garbage() {
        assert!(NodeEndpoints::decode("not json").is_none());
        assert!(NodeEndpoints::decode("{\"hls\":42}").is_none());
    }

    #[test]
    fn endpoints_decode_tolerates_missing_fields() {
        // Forward/backward compat: a node advertising only hls is
        // a perfectly reasonable partial deployment.
        let raw = r#"{"hls":"http://a:1"}"#;
        let decoded = NodeEndpoints::decode(raw).expect("decode");
        assert_eq!(decoded.hls, Some("http://a:1".to_string()));
        assert!(decoded.dash.is_none());
        assert!(decoded.rtsp.is_none());
    }

    #[test]
    fn endpoints_decode_tolerates_extra_fields() {
        // A future schema bump adding a new protocol must not
        // break older readers.
        let raw = r#"{"hls":"http://a:1","dash":null,"rtsp":null,"future":"whep"}"#;
        let decoded = NodeEndpoints::decode(raw).expect("decode");
        assert_eq!(decoded.hls, Some("http://a:1".to_string()));
    }
}
