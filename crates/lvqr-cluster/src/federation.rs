//! Cross-cluster federation links.
//!
//! **Tier 4 item 4.4 session A.** Each [`FederationLink`] describes a
//! one-way subscription from the local cluster to a remote cluster's
//! MoQ relay: the local node opens a single authenticated MoQ session
//! to `remote_url`, subscribes to the remote origin's announcement
//! stream, and for every broadcast name in `forwarded_broadcasts`
//! bridges the remote broadcast into the local origin so every
//! egress surface (LL-HLS, DASH, WHEP, MoQ relay) serves it as if it
//! had been ingested locally.
//!
//! # What ships in session 101 A
//!
//! * [`FederationLink`]: serde-friendly config struct (TOML / CLI /
//!   admin-API surface).
//! * [`FederationRunner`]: owns one tokio task per link. Each task
//!   opens the outbound MoQ session, subscribes to remote
//!   announcements, filters them against
//!   [`FederationLink::forwarded_broadcasts`], and logs a structured
//!   event when a forwardable broadcast lands.
//!
//! # What session 102 B adds
//!
//! The per-track re-publish from the remote `BroadcastConsumer`
//! into the local `OriginProducer`'s broadcast. That is
//! straightforward once the announcement filter is proven out: for
//! each LVQR track-name convention (`0.mp4` video, `1.mp4` audio,
//! `catalog`), call `remote_bc.subscribe_track(..)` and copy
//! groups (and their frames) into a `local_bc.create_track(..)`
//! producer. Session 102 B also stands up a two-cluster
//! integration test
//! (`crates/lvqr-cli/tests/federation_two_cluster.rs`) that
//! exercises the full wire path end-to-end; the track-copy code
//! lives there rather than in 101 A because the meaningful
//! verification requires two real MoQ relays.
//!
//! # What session 103 C adds
//!
//! `GET /api/v1/cluster/federation` admin route that exposes link
//! status; automatic reconnect-on-failure with exponential backoff.
//!
//! # Authentication
//!
//! The `auth_token` is a JWT minted for the remote cluster's
//! audience claim (see Tier 4 item 4.8 for the JWT minting path).
//! [`FederationLink::subscription_url`] appends `?token=<jwt>` to
//! the configured `remote_url` so the remote relay's
//! `parse_url_token` + `AuthContext::Subscribe` check authenticates
//! the federation session under the same auth surface every
//! LVQR-protocol subscribe uses.

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

/// Reserved query-parameter name for the auth token on the
/// subscription URL. Matches the existing convention in
/// `lvqr_relay::server::parse_url_token` + every ingest / subscribe
/// surface in the project (RTMP stream key, WS `?token=`, etc.).
const TOKEN_QUERY_PARAM: &str = "token";

/// One-way subscription from the local cluster to a remote cluster's
/// MoQ relay endpoint.
///
/// Directionality matters: a link describes "this local node pulls
/// from the remote". Bidirectional federation is expressed as two
/// links, one in each cluster's config. The anti-scope document
/// in `tracking/TIER_4_PLAN.md` section 4.4 rules out auto-discovery,
/// so the operator curates both sides explicitly.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct FederationLink {
    /// Remote relay URL. Must parse as a [`url::Url`]; the scheme is
    /// typically `https://` over WebTransport-capable QUIC.
    ///
    /// Example: `"https://relay.us-west.example.com:4443"`.
    pub remote_url: String,
    /// Bearer token passed to the remote relay via the `?token=`
    /// query parameter. Typically a JWT minted for the remote
    /// cluster's audience claim per Tier 4 item 4.8.
    pub auth_token: String,
    /// Explicit list of broadcast names to forward. Each incoming
    /// announcement from the remote is matched exactly against this
    /// list; unmatched announcements are ignored. Empty list is a
    /// valid no-op configuration (the link opens but forwards
    /// nothing; useful for validating cluster reachability without
    /// trafficking broadcasts).
    ///
    /// Glob / prefix patterns are explicitly out of scope for v1.
    #[serde(default)]
    pub forwarded_broadcasts: Vec<String>,
    /// Disable TLS certificate verification on the outbound MoQ
    /// session. Defaults to `false` (verify against the operator's
    /// trust store). Set `true` when both clusters run self-signed
    /// certs inside a trusted VPC, or when integration tests use
    /// `TestServer`'s auto-generated self-signed cert.
    ///
    /// Security note: disabling verification exposes the federation
    /// `auth_token` to MITM attackers on the link's network path.
    /// Only disable inside a topology where the network itself is
    /// already authenticated (private VPC, mesh WireGuard, etc.).
    #[serde(default)]
    pub disable_tls_verify: bool,
}

impl FederationLink {
    /// New link with the supplied remote URL, auth token, and
    /// forwarded broadcast list. TLS verification defaults to on.
    pub fn new(
        remote_url: impl Into<String>,
        auth_token: impl Into<String>,
        forwarded_broadcasts: Vec<String>,
    ) -> Self {
        Self {
            remote_url: remote_url.into(),
            auth_token: auth_token.into(),
            forwarded_broadcasts,
            disable_tls_verify: false,
        }
    }

    /// Builder: flip TLS verification off. Returns `self` for
    /// chaining. See [`Self::disable_tls_verify`] for the security
    /// caveats.
    pub fn with_disable_tls_verify(mut self, disable: bool) -> Self {
        self.disable_tls_verify = disable;
        self
    }

    /// Resolve the full subscription URL by parsing [`remote_url`]
    /// and appending the auth token as a `token=` query parameter.
    /// Returns an error if `remote_url` is not a valid URL.
    ///
    /// [`remote_url`]: Self::remote_url
    pub fn subscription_url(&self) -> Result<url::Url> {
        let mut url: url::Url = self
            .remote_url
            .parse()
            .with_context(|| format!("federation link remote_url `{}` is not a valid URL", self.remote_url))?;
        url.query_pairs_mut().append_pair(TOKEN_QUERY_PARAM, &self.auth_token);
        Ok(url)
    }

    /// Whether this link is configured to forward broadcast `name`.
    /// Exact-match today; glob support is out of scope for v1.
    pub fn forwards(&self, name: &str) -> bool {
        self.forwarded_broadcasts.iter().any(|f| f == name)
    }
}

/// Runtime handle for a set of [`FederationLink`]s. Holds one tokio
/// task per link alive for the lifetime of this handle; dropping the
/// handle or calling [`Self::shutdown`] cancels every link's task.
pub struct FederationRunner {
    tasks: Vec<JoinHandle<()>>,
    shutdown: CancellationToken,
    /// How many links the caller handed us. Exposed via
    /// [`Self::configured_links`] so admin routes can report the
    /// declared-vs-live gap.
    configured: usize,
}

impl FederationRunner {
    /// Start one tokio task per link. Each task opens a MoQ session
    /// against the link's remote relay and subscribes to the remote
    /// announcement stream.
    ///
    /// The returned handle MUST be held for the cluster's lifetime;
    /// dropping it cancels the shared shutdown token and lets every
    /// per-link task wind down naturally.
    ///
    /// `local_origin` is the [`lvqr_moq::OriginProducer`] every egress
    /// surface consumes from; session 102 B wires the remote -> local
    /// track copy through this origin.
    pub fn start(
        links: Vec<FederationLink>,
        local_origin: lvqr_moq::OriginProducer,
        shutdown: CancellationToken,
    ) -> Self {
        let configured = links.len();
        let mut tasks = Vec::with_capacity(configured);
        for link in links {
            let origin = local_origin.clone();
            let cancel = shutdown.clone();
            let task = tokio::spawn(async move {
                let remote_url_for_log = link.remote_url.clone();
                if let Err(e) = run_link(link, origin, cancel).await {
                    warn!(
                        remote_url = %remote_url_for_log,
                        error = %e,
                        "federation link exited with error"
                    );
                }
            });
            tasks.push(task);
        }
        info!(links = configured, "federation runner started");
        Self {
            tasks,
            shutdown,
            configured,
        }
    }

    /// How many links the runner was asked to manage. Constant from
    /// [`Self::start`] onward; does not reflect whether individual
    /// per-link tasks have since errored out.
    pub fn configured_links(&self) -> usize {
        self.configured
    }

    /// How many per-link tasks are still running. Best-effort: a task
    /// that has just exited but not yet been observed may still
    /// report "active" for a brief window.
    pub fn active_links(&self) -> usize {
        self.tasks.iter().filter(|t| !t.is_finished()).count()
    }

    /// Cancel every per-link task and await their exit. Each task
    /// gets up to [`SHUTDOWN_GRACE`] to observe the cancel signal
    /// and exit cleanly; tasks still running after the grace are
    /// aborted. Bounded shutdown matters because `moq_native::Client`
    /// connect futures can be stuck inside sync TLS / DNS work that
    /// is not cancellation-responsive -- a naive `task.await` would
    /// hang the cluster shutdown for seconds on an unreachable peer.
    pub async fn shutdown(mut self) {
        self.shutdown.cancel();
        for task in self.tasks.drain(..) {
            let abort = task.abort_handle();
            if (tokio::time::timeout(SHUTDOWN_GRACE, task).await).is_err() {
                warn!("federation per-link task exceeded shutdown grace; aborting");
                abort.abort();
            }
        }
        debug!("federation runner shutdown complete");
    }
}

/// Per-link graceful-shutdown budget. Scoped to 1 s because the
/// normal exit path (select arm returns inside the main loop) is
/// sub-millisecond once the cancel is observed; anything longer is
/// a stuck sync primitive (TLS setup, DNS resolve) and the abort
/// path is the correct answer.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(1);

impl Drop for FederationRunner {
    fn drop(&mut self) {
        self.shutdown.cancel();
        for task in self.tasks.drain(..) {
            task.abort();
        }
    }
}

/// Per-link main loop. Opens an outbound MoQ session against the
/// remote relay, subscribes to the remote origin's announcement
/// stream, and for every announcement whose broadcast name matches
/// [`FederationLink::forwards`], logs a structured event. Session
/// 102 B extends this with the actual per-track re-publish into
/// `local_origin`.
///
/// Errors during session setup propagate back to [`FederationRunner`]
/// where they are logged; they do NOT kill other links' tasks. A
/// future session (103 C) will add exponential-backoff reconnect.
async fn run_link(
    link: FederationLink,
    local_origin: lvqr_moq::OriginProducer,
    shutdown: CancellationToken,
) -> Result<()> {
    let url = link.subscription_url()?;

    let mut client_config = moq_native::ClientConfig::default();
    if link.disable_tls_verify {
        client_config.tls.disable_verify = Some(true);
        warn!(
            remote_url = %link.remote_url,
            "federation link has TLS verification disabled; auth token exposure on network path is operator's responsibility"
        );
    }
    let client = client_config.init().context("init federation moq client")?;

    // Announcements from the remote cluster arrive on this origin.
    // Sub-origin pattern mirrors `crates/lvqr-relay/tests/relay_integration.rs`.
    let sub_origin = moq_lite::Origin::produce();
    let mut announcements = sub_origin.consume();

    let client = client.with_consume(sub_origin);

    // The connect future is not cancel-safe w.r.t. partial handshake
    // state; race it with shutdown so the cluster can tear down during
    // a hanging handshake.
    let session = tokio::select! {
        result = client.connect(url.clone()) => {
            result.with_context(|| format!("moq connect to {}", link.remote_url))?
        }
        _ = shutdown.cancelled() => {
            debug!(remote_url = %link.remote_url, "federation link cancelled before connect");
            return Ok(());
        }
    };
    info!(remote_url = %link.remote_url, "federation link connected");

    // Forwarded-broadcast set as a cheap Arc<HashSet>-style lookup.
    // Vec::contains is O(n); with v1 link lists usually <10 entries
    // the allocation-free contains is already strictly faster than
    // hashing, but the FederationLink::forwards accessor keeps the
    // check behind a helper so a future swap to HashSet is local.
    let link = Arc::new(link);

    loop {
        tokio::select! {
            announced = announcements.announced() => {
                let Some((path, maybe_bc)) = announced else {
                    debug!("federation link remote announcement stream closed");
                    break;
                };
                let path_str = path.as_str();
                let Some(bc) = maybe_bc else {
                    // Unannounce event. moq-lite's local origin will
                    // surface the shadow broadcast's natural close
                    // when the remote BroadcastConsumer we captured
                    // here drops (via forward_broadcast's return).
                    debug!(broadcast = %path_str, "federation: remote unannounce");
                    continue;
                };
                if !link.forwards(path_str) {
                    debug!(broadcast = %path_str, "federation: ignoring unmatched announcement");
                    continue;
                }
                let name = path_str.to_string();
                let origin = local_origin.clone();
                let cancel = shutdown.clone();
                info!(
                    broadcast = %name,
                    remote_url = %link.remote_url,
                    "federation: forwarding remote broadcast into local origin"
                );
                tokio::spawn(async move {
                    if let Err(e) = forward_broadcast(bc, origin, name.clone(), cancel).await {
                        warn!(broadcast = %name, error = %e, "federation: forward_broadcast exited with error");
                    }
                });
            }
            _ = shutdown.cancelled() => {
                debug!(remote_url = %link.remote_url, "federation link shutdown requested");
                break;
            }
        }
    }

    // Drop the session to close the underlying connection. moq-native
    // does not expose an explicit `close()` on the session handle as
    // of 0.13; drop is the documented shutdown path.
    drop(session);
    // Small settle window so the QUIC close reaches the peer before
    // the runtime winds down under a dropped runtime. 50 ms is long
    // enough for a loopback flush without being worth measuring.
    tokio::time::sleep(Duration::from_millis(50)).await;
    info!(remote_url = %link.remote_url, "federation link disconnected");
    Ok(())
}

/// LVQR track-name convention. The federation forwarder opens a
/// subscription against each of these on every forwarded broadcast.
/// Remote broadcasts that do not publish one (e.g. audio-only) just
/// see their forwarder sit idle on the absent track until the
/// broadcast closes.
///
/// `catalog` is present so downstream subscribers can discover
/// per-track metadata without re-deriving it; `0.mp4` + `1.mp4`
/// are LVQR's video + audio track-name constants that the ingest
/// bridges (RTMP, WHIP, SRT, RTSP) emit on.
const FEDERATED_TRACK_NAMES: &[&str] = &["0.mp4", "1.mp4", "catalog"];

/// Spawn one forwarder task per LVQR convention track, copying
/// groups + frames from the remote broadcast into a fresh local
/// broadcast. Returns once shutdown fires; the broadcast producer
/// drops on scope exit, closing the shadow broadcast.
async fn forward_broadcast(
    remote_bc: moq_lite::BroadcastConsumer,
    local_origin: lvqr_moq::OriginProducer,
    broadcast_name: String,
    shutdown: CancellationToken,
) -> Result<()> {
    let mut local_bc = local_origin
        .create_broadcast(&broadcast_name)
        .with_context(|| format!("create local federated broadcast `{broadcast_name}`"))?;

    let mut track_handles = Vec::with_capacity(FEDERATED_TRACK_NAMES.len());
    for name in FEDERATED_TRACK_NAMES {
        let remote_track = match remote_bc.subscribe_track(&lvqr_moq::Track::new(*name)) {
            Ok(t) => t,
            Err(e) => {
                debug!(
                    broadcast = %broadcast_name,
                    track = %name,
                    error = %e,
                    "federation: remote subscribe_track failed; skipping"
                );
                continue;
            }
        };
        let local_track = match local_bc.create_track(lvqr_moq::Track::new(*name)) {
            Ok(t) => t,
            Err(e) => {
                debug!(
                    broadcast = %broadcast_name,
                    track = %name,
                    error = %e,
                    "federation: local create_track failed; skipping"
                );
                continue;
            }
        };
        let cancel = shutdown.clone();
        let track_name = name.to_string();
        let broadcast_name_for_log = broadcast_name.clone();
        let handle = tokio::spawn(async move {
            if let Err(e) = forward_track(remote_track, local_track, cancel).await {
                debug!(
                    broadcast = %broadcast_name_for_log,
                    track = %track_name,
                    error = %e,
                    "federation: track forwarder exited with error"
                );
            }
        });
        track_handles.push(handle);
    }

    // Hold the broadcast producer + track forwarders open until
    // shutdown. Dropping `local_bc` terminates the shadow broadcast
    // which is also what we want on natural shutdown.
    shutdown.cancelled().await;
    for handle in track_handles {
        handle.abort();
    }
    drop(local_bc);
    Ok(())
}

/// Copy groups + frames from a remote `TrackConsumer` into a local
/// `TrackProducer`. Exits naturally when the remote track closes
/// (returns `Ok(None)` from `next_group`) or when shutdown fires.
async fn forward_track(
    mut remote: lvqr_moq::TrackConsumer,
    mut local: lvqr_moq::TrackProducer,
    shutdown: CancellationToken,
) -> Result<()> {
    loop {
        let next = tokio::select! {
            g = remote.next_group() => g,
            _ = shutdown.cancelled() => return Ok(()),
        };
        let Some(mut remote_group) = next.context("remote next_group")? else {
            return Ok(());
        };
        let mut local_group = local.append_group().context("append local group")?;
        loop {
            let frame = tokio::select! {
                f = remote_group.read_frame() => f,
                _ = shutdown.cancelled() => {
                    let _ = local_group.finish();
                    return Ok(());
                }
            };
            let Some(frame) = frame.context("remote read_frame")? else {
                break;
            };
            local_group.write_frame(frame).context("write local frame")?;
        }
        local_group.finish().context("finish local group")?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn federation_link_new_round_trips_fields() {
        let link = FederationLink::new("https://peer.example:4443", "jwt-token", vec!["live/a".into()]);
        assert_eq!(link.remote_url, "https://peer.example:4443");
        assert_eq!(link.auth_token, "jwt-token");
        assert_eq!(link.forwarded_broadcasts, vec!["live/a"]);
    }

    #[test]
    fn subscription_url_appends_token_query_param() {
        let link = FederationLink::new("https://peer.example:4443/", "abc123", Vec::new());
        let url = link.subscription_url().expect("valid url");
        let pairs: Vec<(String, String)> = url
            .query_pairs()
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect();
        assert_eq!(pairs, vec![("token".into(), "abc123".into())]);
    }

    #[test]
    fn subscription_url_preserves_existing_query_params() {
        // Real deployments may carry operator-defined query params
        // (e.g. `?region=us-west`); the token append must not clobber
        // them.
        let link = FederationLink::new("https://peer.example:4443/?region=us-west", "t", Vec::new());
        let url = link.subscription_url().expect("valid url");
        let pairs: Vec<(String, String)> = url
            .query_pairs()
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect();
        assert!(pairs.contains(&("region".into(), "us-west".into())));
        assert!(pairs.contains(&("token".into(), "t".into())));
    }

    #[test]
    fn subscription_url_errors_on_malformed_remote_url() {
        let link = FederationLink::new("not-a-url", "t", Vec::new());
        assert!(link.subscription_url().is_err());
    }

    #[test]
    fn forwards_exact_match_only() {
        let link = FederationLink::new(
            "https://peer.example:4443/",
            "t",
            vec!["live/room1".into(), "live/room2".into()],
        );
        assert!(link.forwards("live/room1"));
        assert!(link.forwards("live/room2"));
        assert!(!link.forwards("live/room3"));
        // Prefix NOT a match: v1 is exact-match only.
        assert!(!link.forwards("live/room"));
        assert!(!link.forwards("live/room1/extra"));
    }

    #[test]
    fn forwards_returns_false_for_empty_list() {
        let link = FederationLink::new("https://peer.example:4443/", "t", Vec::new());
        assert!(!link.forwards("anything"));
    }

    #[test]
    fn disable_tls_verify_defaults_to_false() {
        let link = FederationLink::new("https://peer.example:4443/", "t", Vec::new());
        assert!(!link.disable_tls_verify);
    }

    #[test]
    fn with_disable_tls_verify_flips_field() {
        let link = FederationLink::new("https://peer.example:4443/", "t", Vec::new()).with_disable_tls_verify(true);
        assert!(link.disable_tls_verify);
    }
}
