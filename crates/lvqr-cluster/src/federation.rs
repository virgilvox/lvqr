//! Cross-cluster federation links.
//!
//! **Tier 4 item 4.4.** Each [`FederationLink`] describes a
//! one-way subscription from the local cluster to a remote cluster's
//! MoQ relay: the local node opens a single authenticated MoQ session
//! to `remote_url`, subscribes to the remote origin's announcement
//! stream, and for every broadcast name in `forwarded_broadcasts`
//! bridges the remote broadcast into the local origin so every
//! egress surface (LL-HLS, DASH, WHEP, MoQ relay) serves it as if it
//! had been ingested locally.
//!
//! # Session roll-up
//!
//! * Session 101 A landed the [`FederationLink`] config + the
//!   per-link announcement-subscribe loop (no track copy; logs
//!   matched announcements).
//! * Session 102 B extended the matched-announcement arm with
//!   `forward_broadcast` + `forward_track` so video + audio +
//!   catalog tracks re-publish into the local origin.
//! * Session 103 C (this file as it stands) adds the per-link
//!   status store ([`FederationLinkStatus`] +
//!   [`FederationStatusHandle`]) and the exponential-backoff
//!   reconnect loop around [`run_link_once`]. The admin HTTP route
//!   `GET /api/v1/cluster/federation` reads a snapshot of the
//!   status handle; see `lvqr-admin::cluster_routes`.
//!
//! # Authentication
//!
//! The `auth_token` is a JWT minted for the remote cluster's
//! audience claim (see Tier 4 item 4.8 for the JWT minting path).
//! [`FederationLink::subscription_url`] appends `?token=<jwt>` to
//! the configured `remote_url` so the remote relay's
//! `parse_url_token` + `AuthContext::Subscribe` check authenticates
//! the federation session under the same auth surface every
//! LVQR-protocol subscribe uses. Token refresh across reconnect
//! attempts is OUT of scope for v1: if the token expires while a
//! link is in the reconnect loop, subsequent attempts reuse the
//! same stale token and fail with 401 on the remote. Operators can
//! observe this via the admin route's `last_error` field and
//! rotate the config manually.

use std::sync::{Arc, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use rand::Rng;
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
    ///
    /// Token refresh across reconnect attempts is not implemented
    /// in v1: a short-lived JWT that expires while the link is
    /// failing will keep re-attempting with the same stale token
    /// until an operator rotates the config. The admin route
    /// surfaces this failure via `last_error`.
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

/// Current connection phase for a federation link. The three states
/// map 1:1 to the outer retry loop in [`run_link`]:
///
/// * [`Connecting`](FederationConnectState::Connecting) -- the
///   per-link task is inside [`run_link_once`] before the MoQ
///   session handshake completes, or the retry-sleep between two
///   connect attempts.
/// * [`Connected`](FederationConnectState::Connected) -- the MoQ
///   session is established and the announcement-subscribe loop
///   is draining.
/// * [`Failed`](FederationConnectState::Failed) -- the most recent
///   attempt returned an error; the retry wrapper is either
///   sleeping on backoff or about to re-enter [`run_link_once`].
///   Inspect `last_error` for the cause.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FederationConnectState {
    Connecting,
    Connected,
    Failed,
}

/// External-facing status snapshot for one [`FederationLink`]. The
/// admin route `GET /api/v1/cluster/federation` serializes a
/// `Vec<FederationLinkStatus>` directly onto the wire; the field
/// names are therefore part of the public HTTP contract.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FederationLinkStatus {
    /// Remote relay URL, exactly as configured. The token query
    /// parameter is NOT appended here; the admin route is
    /// read-only and must not leak credentials.
    pub remote_url: String,
    /// Configured broadcast names the link forwards (exact-match).
    pub forwarded_broadcasts: Vec<String>,
    /// Current connection phase; see [`FederationConnectState`].
    pub state: FederationConnectState,
    /// Wall-clock millis since the Unix epoch when the link last
    /// transitioned to [`FederationConnectState::Connected`]. `None`
    /// until the first successful connect.
    pub last_connected_at_ms: Option<u64>,
    /// Human-readable error from the most recent failed connect or
    /// mid-session error. Cleared on a successful connect.
    pub last_error: Option<String>,
    /// Total connect attempts for this link since runner startup.
    /// Increments on every entry into [`run_link_once`], successful
    /// or not. Useful for operators diagnosing repeated auth
    /// failures (e.g. expired JWT).
    pub connect_attempts: u64,
    /// How many distinct remote announcements have matched this
    /// link's forward list since runner startup. Each match spawns
    /// one forwarder task.
    pub forwarded_broadcasts_seen: u64,
}

/// Cloneable read handle over the runner's per-link status store.
/// Safe to clone across tokio tasks; the admin route's handler
/// holds one such clone and calls [`Self::snapshot`] on each
/// request.
#[derive(Clone)]
pub struct FederationStatusHandle {
    inner: Arc<RwLock<Vec<FederationLinkStatus>>>,
}

impl FederationStatusHandle {
    fn new(links: &[FederationLink]) -> Self {
        let entries = links
            .iter()
            .map(|link| FederationLinkStatus {
                remote_url: link.remote_url.clone(),
                forwarded_broadcasts: link.forwarded_broadcasts.clone(),
                state: FederationConnectState::Connecting,
                last_connected_at_ms: None,
                last_error: None,
                connect_attempts: 0,
                forwarded_broadcasts_seen: 0,
            })
            .collect();
        Self {
            inner: Arc::new(RwLock::new(entries)),
        }
    }

    /// Snapshot the current per-link status vec. Cheap: one
    /// `Vec::clone` + the inner `FederationLinkStatus` clones, all
    /// small.
    pub fn snapshot(&self) -> Vec<FederationLinkStatus> {
        self.inner.read().expect("federation status RwLock poisoned").clone()
    }

    fn mutate<F: FnOnce(&mut FederationLinkStatus)>(&self, index: usize, f: F) {
        if let Ok(mut guard) = self.inner.write() {
            if let Some(entry) = guard.get_mut(index) {
                f(entry);
            }
        }
    }

    fn set_connecting(&self, index: usize) {
        self.mutate(index, |s| {
            s.state = FederationConnectState::Connecting;
            s.connect_attempts = s.connect_attempts.saturating_add(1);
        });
    }

    fn set_connected(&self, index: usize) {
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        self.mutate(index, |s| {
            s.state = FederationConnectState::Connected;
            s.last_connected_at_ms = Some(now_ms);
            s.last_error = None;
        });
    }

    fn set_failed(&self, index: usize, err: &anyhow::Error) {
        let msg = format!("{err:#}");
        self.mutate(index, |s| {
            s.state = FederationConnectState::Failed;
            s.last_error = Some(msg);
        });
    }

    fn increment_forwarded(&self, index: usize) {
        self.mutate(index, |s| {
            s.forwarded_broadcasts_seen = s.forwarded_broadcasts_seen.saturating_add(1);
        });
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
    /// Shared status store; one entry per link, in the same order
    /// the caller passed to [`Self::start`]. Exposed via
    /// [`Self::status_handle`] for the admin route.
    status: FederationStatusHandle,
}

impl FederationRunner {
    /// Start one tokio task per link. Each task opens a MoQ session
    /// against the link's remote relay, subscribes to the remote
    /// announcement stream, and auto-reconnects on transient
    /// failures via an exponential-backoff wrapper.
    ///
    /// The returned handle MUST be held for the cluster's lifetime;
    /// dropping it cancels the shared shutdown token and lets every
    /// per-link task wind down naturally.
    ///
    /// `local_origin` is the [`lvqr_moq::OriginProducer`] every egress
    /// surface consumes from; matched forwarded broadcasts land
    /// there via [`forward_broadcast`] + [`forward_track`].
    pub fn start(
        links: Vec<FederationLink>,
        local_origin: lvqr_moq::OriginProducer,
        shutdown: CancellationToken,
    ) -> Self {
        let configured = links.len();
        let status = FederationStatusHandle::new(&links);
        let mut tasks = Vec::with_capacity(configured);
        for (index, link) in links.into_iter().enumerate() {
            let origin = local_origin.clone();
            let cancel = shutdown.clone();
            let status_for_task = status.clone();
            let task = tokio::spawn(async move {
                run_link(index, link, origin, status_for_task, cancel).await;
            });
            tasks.push(task);
        }
        info!(links = configured, "federation runner started");
        Self {
            tasks,
            shutdown,
            configured,
            status,
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

    /// Cloneable handle to the per-link status store. The admin
    /// route `GET /api/v1/cluster/federation` reads a snapshot via
    /// [`FederationStatusHandle::snapshot`].
    pub fn status_handle(&self) -> FederationStatusHandle {
        self.status.clone()
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

/// Minimum backoff delay between reconnect attempts. The first
/// reconnect waits `[0.9s, 1.1s]` after jitter.
const BACKOFF_INITIAL: Duration = Duration::from_secs(1);

/// Maximum backoff delay. Jitter can push the realized sleep up to
/// `BACKOFF_MAX * (1.0 + JITTER_FRAC)`.
const BACKOFF_MAX: Duration = Duration::from_secs(60);

/// Symmetric jitter fraction applied to the doubled-base delay.
/// `±10%` per the session 103 C plan.
const BACKOFF_JITTER_FRAC: f64 = 0.1;

/// Per-attempt connect timeout. QUIC clients with no peer response
/// will retransmit Initial packets for tens of seconds on a silent
/// path; without a bound the retry loop can never observe a
/// Failed state on a dead peer and the admin route stays pinned
/// at Connecting. 10 s is well above the 99th percentile for
/// healthy loopback / LAN handshakes and still short enough that
/// an operator watching the admin route sees progress on a
/// re-tried reconnect cycle.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

impl Drop for FederationRunner {
    fn drop(&mut self) {
        self.shutdown.cancel();
        for task in self.tasks.drain(..) {
            task.abort();
        }
    }
}

/// Compute the next reconnect delay for attempt index `attempt`
/// (0-based: `attempt == 0` is the delay before the second
/// connect, after the first attempt failed).
///
/// Doubles from [`BACKOFF_INITIAL`] to [`BACKOFF_MAX`] and then
/// applies a symmetric `±BACKOFF_JITTER_FRAC` jitter. The jitter
/// envelope is clipped by saturating arithmetic on `u64`
/// milliseconds; at the cap the realized sleep is still
/// guaranteed within `[BACKOFF_MAX * 0.9, BACKOFF_MAX * 1.1]`.
fn next_delay(attempt: u32) -> Duration {
    let base_ms = BACKOFF_INITIAL.as_millis() as u64;
    // `1 << attempt` explodes fast; cap at a shift that still fits
    // in u64 before the later min() clamps against BACKOFF_MAX.
    let shift = attempt.min(20);
    let doubled = base_ms.saturating_mul(1u64 << shift);
    let capped = doubled.min(BACKOFF_MAX.as_millis() as u64);
    let jitter = rand::thread_rng().gen_range(-BACKOFF_JITTER_FRAC..=BACKOFF_JITTER_FRAC);
    let ms = ((capped as f64) * (1.0 + jitter)).round().max(0.0) as u64;
    Duration::from_millis(ms)
}

/// Per-link retry wrapper. Runs [`run_link_once`] in a loop with
/// exponential-backoff sleeps between attempts; exits only when
/// the shared shutdown token is cancelled.
///
/// Each pass increments `status.connect_attempts`. On success the
/// state flips to [`FederationConnectState::Connected`]; on error
/// to [`FederationConnectState::Failed`] with the error string on
/// `last_error`.
async fn run_link(
    index: usize,
    link: FederationLink,
    local_origin: lvqr_moq::OriginProducer,
    status: FederationStatusHandle,
    shutdown: CancellationToken,
) {
    let link = Arc::new(link);
    let mut attempt: u32 = 0;
    loop {
        if shutdown.is_cancelled() {
            return;
        }
        status.set_connecting(index);
        let result = run_link_once(
            index,
            link.clone(),
            local_origin.clone(),
            status.clone(),
            shutdown.clone(),
        )
        .await;
        match result {
            Ok(()) => {
                if shutdown.is_cancelled() {
                    return;
                }
                // Clean remote close, not shutdown: reset the backoff
                // window so the first reconnect fires fast. Stays in
                // "failed" state until the next attempt flips to
                // connecting.
                status.mutate(index, |s| {
                    s.state = FederationConnectState::Failed;
                    s.last_error = Some("remote announcement stream closed".to_string());
                });
                attempt = 0;
            }
            Err(e) => {
                warn!(
                    remote_url = %link.remote_url,
                    error = %e,
                    attempt,
                    "federation link attempt failed; scheduling reconnect"
                );
                status.set_failed(index, &e);
                attempt = attempt.saturating_add(1);
            }
        }
        let delay = next_delay(attempt);
        debug!(
            remote_url = %link.remote_url,
            delay_ms = delay.as_millis() as u64,
            attempt,
            "federation link waiting before next reconnect"
        );
        tokio::select! {
            _ = tokio::time::sleep(delay) => {}
            _ = shutdown.cancelled() => return,
        }
    }
}

/// Single connect + announcement-drain pass. Opens an outbound MoQ
/// session against the remote relay, subscribes to the remote
/// origin's announcement stream, and for every announcement whose
/// broadcast name matches [`FederationLink::forwards`], spawns a
/// [`forward_broadcast`] task.
///
/// Returns `Ok(())` on a clean remote-close (the retry wrapper
/// schedules a reconnect anyway) or `Err(_)` when the connect /
/// setup / stream surfaces an error. The retry wrapper treats
/// both as failure modes that warrant a backoff-sleep before the
/// next attempt.
async fn run_link_once(
    index: usize,
    link: Arc<FederationLink>,
    local_origin: lvqr_moq::OriginProducer,
    status: FederationStatusHandle,
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
    // state; race it with shutdown and an explicit timeout. Without
    // the timeout arm, a silently-dropped Initial against an
    // unroutable peer would pin the per-link task in Connecting
    // forever (see CONNECT_TIMEOUT docstring).
    let session = tokio::select! {
        result = tokio::time::timeout(CONNECT_TIMEOUT, client.connect(url.clone())) => {
            match result {
                Ok(connect_result) => connect_result
                    .with_context(|| format!("moq connect to {}", link.remote_url))?,
                Err(_) => {
                    return Err(anyhow::anyhow!(
                        "moq connect to {} timed out after {:?}",
                        link.remote_url,
                        CONNECT_TIMEOUT
                    ));
                }
            }
        }
        _ = shutdown.cancelled() => {
            debug!(remote_url = %link.remote_url, "federation link cancelled before connect");
            return Ok(());
        }
    };
    status.set_connected(index);
    info!(remote_url = %link.remote_url, "federation link connected");

    loop {
        tokio::select! {
            announced = announcements.announced() => {
                let Some((path, maybe_bc)) = announced else {
                    debug!("federation link remote announcement stream closed");
                    break;
                };
                let path_str = path.as_str();
                let Some(bc) = maybe_bc else {
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
                status.increment_forwarded(index);
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
            closed = session.closed() => {
                // Remote closed the transport session (peer shut down,
                // network partition, etc.). The local sub_origin does
                // not surface this through `announced()` on its own;
                // without this arm the per-link task would block
                // forever on an already-dead session. Propagate as an
                // error so the outer retry loop records Failed and
                // schedules a reconnect.
                let err_msg = match closed {
                    Ok(()) => "moq session closed".to_string(),
                    Err(e) => format!("moq session closed: {e}"),
                };
                return Err(anyhow::anyhow!(err_msg));
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

    #[test]
    fn next_delay_attempt_zero_within_jitter_window_of_initial() {
        // attempt = 0 -> base = 1000 ms; jitter ±10% -> [900, 1100]
        let mut saw_below_1000 = false;
        let mut saw_above_1000 = false;
        for _ in 0..200 {
            let d = next_delay(0);
            assert!(
                d >= Duration::from_millis(900) && d <= Duration::from_millis(1100),
                "delay {:?} outside [900, 1100] ms jitter window",
                d
            );
            if d < Duration::from_millis(1000) {
                saw_below_1000 = true;
            }
            if d > Duration::from_millis(1000) {
                saw_above_1000 = true;
            }
        }
        // Probabilistically with 200 samples the jitter should cover
        // both sides of the midpoint essentially always. If this ever
        // flakes, widen to 1000 samples; but the rand::thread_rng
        // PRNG is fine on the scale of 200 samples.
        assert!(saw_below_1000, "no sub-1000ms sample in 200 draws");
        assert!(saw_above_1000, "no above-1000ms sample in 200 draws");
    }

    #[test]
    fn next_delay_doubles_for_small_attempts() {
        // attempt = 1 -> base = 2000 ms; attempt = 2 -> 4000 ms.
        // Check the *capped* midpoint is within the jitter window.
        let d1 = next_delay(1);
        assert!(d1 >= Duration::from_millis(1800) && d1 <= Duration::from_millis(2200));
        let d2 = next_delay(2);
        assert!(d2 >= Duration::from_millis(3600) && d2 <= Duration::from_millis(4400));
    }

    #[test]
    fn next_delay_clamps_at_max_for_large_attempts() {
        // attempt = 10 -> 1024s nominal, clamped to 60s +/- 10%
        // -> [54000, 66000] ms. attempt = 20 clamps the same way.
        for attempt in [6u32, 10, 20] {
            let d = next_delay(attempt);
            assert!(
                d >= Duration::from_millis(54_000) && d <= Duration::from_millis(66_000),
                "attempt {attempt} produced {:?} outside clamped ±10% window of 60s",
                d
            );
        }
    }

    #[test]
    fn federation_link_status_round_trips_through_json() {
        let status = FederationLinkStatus {
            remote_url: "https://peer:4443/".into(),
            forwarded_broadcasts: vec!["live/a".into()],
            state: FederationConnectState::Connected,
            last_connected_at_ms: Some(1_700_000_000_000),
            last_error: None,
            connect_attempts: 3,
            forwarded_broadcasts_seen: 7,
        };
        let json = serde_json::to_string(&status).expect("serialize");
        let parsed: FederationLinkStatus = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed.remote_url, status.remote_url);
        assert_eq!(parsed.state, FederationConnectState::Connected);
        assert_eq!(parsed.connect_attempts, 3);
        assert_eq!(parsed.forwarded_broadcasts_seen, 7);
        // And the wire shape uses lowercase state names per the
        // external HTTP contract.
        assert!(json.contains(r#""state":"connected""#));
    }

    #[test]
    fn federation_connect_state_serializes_as_lowercase() {
        let out = serde_json::to_string(&FederationConnectState::Connecting).expect("ser");
        assert_eq!(out, r#""connecting""#);
        let out = serde_json::to_string(&FederationConnectState::Failed).expect("ser");
        assert_eq!(out, r#""failed""#);
    }

    #[test]
    fn status_handle_snapshot_reflects_init_state() {
        let link = FederationLink::new("https://peer:4443/", "t", vec!["live/x".into()]);
        let handle = FederationStatusHandle::new(&[link]);
        let snap = handle.snapshot();
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0].remote_url, "https://peer:4443/");
        assert_eq!(snap[0].state, FederationConnectState::Connecting);
        assert_eq!(snap[0].connect_attempts, 0);
        assert_eq!(snap[0].forwarded_broadcasts_seen, 0);
        assert!(snap[0].last_connected_at_ms.is_none());
        assert!(snap[0].last_error.is_none());
    }

    #[test]
    fn status_handle_mutators_update_fields() {
        let link = FederationLink::new("https://peer:4443/", "t", Vec::new());
        let handle = FederationStatusHandle::new(&[link]);

        handle.set_connecting(0);
        let snap = handle.snapshot();
        assert_eq!(snap[0].state, FederationConnectState::Connecting);
        assert_eq!(snap[0].connect_attempts, 1);

        handle.set_connected(0);
        let snap = handle.snapshot();
        assert_eq!(snap[0].state, FederationConnectState::Connected);
        assert!(snap[0].last_connected_at_ms.is_some());
        assert!(snap[0].last_error.is_none());

        let err = anyhow::anyhow!("connect refused");
        handle.set_failed(0, &err);
        let snap = handle.snapshot();
        assert_eq!(snap[0].state, FederationConnectState::Failed);
        assert_eq!(snap[0].last_error.as_deref(), Some("connect refused"));

        handle.increment_forwarded(0);
        handle.increment_forwarded(0);
        let snap = handle.snapshot();
        assert_eq!(snap[0].forwarded_broadcasts_seen, 2);
    }

    #[test]
    fn status_handle_clones_share_state() {
        let link = FederationLink::new("https://peer:4443/", "t", Vec::new());
        let handle_a = FederationStatusHandle::new(&[link]);
        let handle_b = handle_a.clone();

        handle_a.set_connecting(0);
        assert_eq!(handle_b.snapshot()[0].connect_attempts, 1);
        handle_b.set_connecting(0);
        assert_eq!(handle_a.snapshot()[0].connect_attempts, 2);
    }

    #[test]
    fn status_handle_out_of_bounds_mutate_is_noop() {
        let link = FederationLink::new("https://peer:4443/", "t", Vec::new());
        let handle = FederationStatusHandle::new(&[link]);
        // Should not panic.
        handle.set_connecting(42);
        handle.set_connected(42);
        handle.increment_forwarded(42);
        handle.set_failed(42, &anyhow::anyhow!("oops"));
        let snap = handle.snapshot();
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0].connect_attempts, 0);
    }
}
