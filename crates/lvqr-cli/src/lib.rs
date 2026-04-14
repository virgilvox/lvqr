//! LVQR server library entry point.
//!
//! `main.rs` parses CLI args and hands off to [`start`]; tests and embedders
//! can call [`start`] directly with a pre-built [`ServeConfig`]. Every
//! listener (MoQ/QUIC, RTMP/TCP, admin/TCP) is bound inside `start` before
//! it returns, so callers who pass `port: 0` can read the real bound
//! addresses back off the returned [`ServerHandle`] and point test clients
//! at them without polling.
//!
//! This is the library target used by `lvqr-test-utils::TestServer` to
//! spin up a full-stack LVQR instance on ephemeral ports inside
//! integration tests.

mod hls;

use anyhow::Result;
use axum::extract::ws::{Message, WebSocket};
use axum::extract::{Path, Query, State, WebSocketUpgrade};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use bytes::Bytes;
use lvqr_auth::{AuthContext, AuthDecision, NoopAuthProvider, SharedAuth};
use lvqr_core::{EventBus, RelayEvent};
use lvqr_hls::{MultiHlsServer, PlaylistBuilderConfig};
use lvqr_ingest::SharedFragmentObserver;
use lvqr_moq::Track;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use tokio_util::sync::CancellationToken;
use tower_http::cors::CorsLayer;

use crate::hls::HlsFragmentBridge;

/// Configuration passed to [`start`] to bring up a full-stack LVQR server.
///
/// Every `*_addr` field accepts port `0` for ephemeral port binding; the
/// real bound address is reported back through [`ServerHandle`].
#[derive(Clone)]
pub struct ServeConfig {
    /// QUIC/MoQ relay bind address.
    pub relay_addr: SocketAddr,
    /// RTMP ingest bind address.
    pub rtmp_addr: SocketAddr,
    /// Admin HTTP (and WS relay/ingest) bind address.
    pub admin_addr: SocketAddr,
    /// Optional LL-HLS HTTP bind address. When `Some`, `start()` spins up a
    /// dedicated `HlsServer` axum router on this address that observes the
    /// RTMP bridge's fragment output and serves `/playlist.m3u8`,
    /// `/init.mp4`, and the per-chunk media URIs the playlist references.
    /// When `None`, no HLS surface is exposed.
    pub hls_addr: Option<SocketAddr>,
    /// Enable the peer mesh coordinator and `/signal` endpoint.
    pub mesh_enabled: bool,
    /// Max children per mesh parent when `mesh_enabled`.
    pub max_peers: usize,
    /// Pre-built auth provider. `None` means open access (`NoopAuthProvider`).
    pub auth: Option<SharedAuth>,
    /// Recording directory. `None` disables recording.
    pub record_dir: Option<PathBuf>,
    /// Install the global Prometheus recorder. Must be `false` in tests
    /// because `metrics-exporter-prometheus` panics on the second install
    /// in a process. `main.rs` sets this to `true`.
    pub install_prometheus: bool,
    /// Path to TLS certificate (PEM). Reserved; not consumed yet. The
    /// relay auto-generates self-signed certs when unset.
    pub tls_cert: Option<PathBuf>,
    /// Path to TLS private key (PEM). Reserved; not consumed yet.
    pub tls_key: Option<PathBuf>,
}

impl ServeConfig {
    /// Minimal loopback config for tests: every listener on `127.0.0.1:0`,
    /// open access, no recording, no Prometheus install.
    pub fn loopback_ephemeral() -> Self {
        let loopback: std::net::IpAddr = std::net::Ipv4Addr::LOCALHOST.into();
        Self {
            relay_addr: (loopback, 0).into(),
            rtmp_addr: (loopback, 0).into(),
            admin_addr: (loopback, 0).into(),
            hls_addr: Some((loopback, 0).into()),
            mesh_enabled: false,
            max_peers: 3,
            auth: None,
            record_dir: None,
            install_prometheus: false,
            tls_cert: None,
            tls_key: None,
        }
    }
}

/// Handle to a running LVQR server. Dropping the handle cancels the shared
/// shutdown token but does not block on subsystem drain; call [`shutdown`]
/// explicitly in tests that need deterministic teardown before the next
/// fixture starts.
///
/// [`shutdown`]: ServerHandle::shutdown
pub struct ServerHandle {
    relay_addr: SocketAddr,
    rtmp_addr: SocketAddr,
    admin_addr: SocketAddr,
    hls_addr: Option<SocketAddr>,
    shutdown: CancellationToken,
    join: Option<tokio::task::JoinHandle<()>>,
}

impl ServerHandle {
    /// Bound QUIC/MoQ relay address.
    pub fn relay_addr(&self) -> SocketAddr {
        self.relay_addr
    }

    /// Bound RTMP ingest address.
    pub fn rtmp_addr(&self) -> SocketAddr {
        self.rtmp_addr
    }

    /// Bound admin HTTP (and WS relay/ingest) address.
    pub fn admin_addr(&self) -> SocketAddr {
        self.admin_addr
    }

    /// Bound LL-HLS HTTP address, when HLS composition is enabled.
    pub fn hls_addr(&self) -> Option<SocketAddr> {
        self.hls_addr
    }

    /// HTTP base URL for the admin / WS surface.
    pub fn http_base(&self) -> String {
        format!("http://{}", self.admin_addr)
    }

    /// HTTP URL pointing at a path on the LL-HLS surface, e.g.
    /// `hls_url("/playlist.m3u8")`. Returns `None` when HLS is not
    /// enabled.
    pub fn hls_url(&self, path: &str) -> Option<String> {
        let addr = self.hls_addr?;
        let path = if path.starts_with('/') {
            path.to_string()
        } else {
            format!("/{path}")
        };
        Some(format!("http://{addr}{path}"))
    }

    /// Construct the WebSocket subscribe URL for a broadcast.
    pub fn ws_url(&self, broadcast: &str) -> String {
        format!("ws://{}/ws/{broadcast}", self.admin_addr)
    }

    /// Construct the WebSocket ingest URL for a broadcast.
    pub fn ws_ingest_url(&self, broadcast: &str) -> String {
        format!("ws://{}/ingest/{broadcast}", self.admin_addr)
    }

    /// Construct the RTMP publish URL for an app + stream key.
    pub fn rtmp_url(&self, app: &str, stream_key: &str) -> String {
        format!("rtmp://{}/{app}/{stream_key}", self.rtmp_addr)
    }

    /// Trigger graceful shutdown and wait for every subsystem to drain.
    pub async fn shutdown(mut self) -> Result<()> {
        self.shutdown.cancel();
        if let Some(join) = self.join.take()
            && let Err(e) = join.await
            && !e.is_cancelled()
        {
            return Err(anyhow::anyhow!("server task panicked: {e}"));
        }
        Ok(())
    }
}

impl Drop for ServerHandle {
    fn drop(&mut self) {
        // Best-effort: signal shutdown so the background tasks wind down
        // even if the caller forgot to `.shutdown().await`. We cannot block
        // on the join handle from a sync drop inside an async runtime
        // without risking a deadlock, so we just cancel and return.
        self.shutdown.cancel();
        if let Some(join) = self.join.take() {
            join.abort();
        }
    }
}

/// Start a full-stack LVQR server. All listeners are bound before the
/// function returns, so the [`ServerHandle`] immediately reports real
/// addresses even when the config requested ephemeral ports.
///
/// The returned handle owns a background task that runs the relay, RTMP,
/// and admin subsystems under a shared cancellation token. Use
/// [`ServerHandle::shutdown`] for deterministic teardown.
pub async fn start(config: ServeConfig) -> Result<ServerHandle> {
    tracing::info!(
        relay = %config.relay_addr,
        rtmp = %config.rtmp_addr,
        admin = %config.admin_addr,
        mesh = config.mesh_enabled,
        "starting LVQR server"
    );

    // Optional Prometheus install. Process-wide, must be skipped in tests.
    let prom_handle = if config.install_prometheus {
        Some(
            metrics_exporter_prometheus::PrometheusBuilder::new()
                .install_recorder()
                .map_err(|e| anyhow::anyhow!("failed to install Prometheus recorder: {e}"))?,
        )
    } else {
        None
    };

    let shutdown = CancellationToken::new();

    // Auth provider: caller-provided, or fall back to open access.
    let auth: SharedAuth = config
        .auth
        .clone()
        .unwrap_or_else(|| Arc::new(NoopAuthProvider) as SharedAuth);

    // Shared lifecycle bus: bridge and WS ingest emit
    // BroadcastStarted/Stopped here; recorder subscribes to it.
    let events = EventBus::default();

    // MoQ relay: init_server() binds the QUIC socket and reports the real
    // bound address, which we surface back through ServerHandle.
    let relay_config = lvqr_relay::RelayConfig::new(config.relay_addr);
    let mut relay = lvqr_relay::RelayServer::new(relay_config);
    relay.set_auth_provider(auth.clone());
    let (mut moq_server, relay_bound) = relay.init_server()?;
    tracing::info!(addr = %relay_bound, "MoQ relay bound");

    // Optional multi-broadcast LL-HLS server. Built before the bridge
    // so we can hand the bridge a `FragmentObserver` that pumps
    // fragments into the shared `MultiHlsServer` state. Each
    // broadcast gets its own per-broadcast `HlsServer` on first
    // publish; the axum router demultiplexes requests under
    // `/hls/{broadcast}/...`.
    let hls_server = config
        .hls_addr
        .map(|_| MultiHlsServer::new(PlaylistBuilderConfig::default()));

    // RTMP ingest bridged to MoQ. Pre-bind the TCP listener so we can
    // report the real bound port (for ephemeral-port test setups).
    let mut bridge_builder =
        lvqr_ingest::RtmpMoqBridge::with_auth(relay.origin().clone(), auth.clone()).with_events(events.clone());
    if let Some(hls) = hls_server.clone() {
        let observer: SharedFragmentObserver = Arc::new(HlsFragmentBridge::new(hls));
        bridge_builder = bridge_builder.with_observer(observer);
    }
    let bridge = Arc::new(bridge_builder);
    let rtmp_config = lvqr_ingest::RtmpConfig {
        bind_addr: config.rtmp_addr,
    };
    let rtmp_server = bridge.create_rtmp_server(rtmp_config);
    let rtmp_listener = tokio::net::TcpListener::bind(config.rtmp_addr).await?;
    let rtmp_bound = rtmp_listener.local_addr()?;
    tracing::info!(addr = %rtmp_bound, "RTMP ingest bound");

    // Admin listener: pre-bind to capture the real port.
    let admin_listener = tokio::net::TcpListener::bind(config.admin_addr).await?;
    let admin_bound = admin_listener.local_addr()?;
    tracing::info!(addr = %admin_bound, "admin HTTP bound");

    // HLS listener: pre-bind so the test harness can read the
    // ephemeral port back via `ServerHandle::hls_addr` immediately
    // after `start()` returns.
    let (hls_listener, hls_bound) = if let Some(addr) = config.hls_addr {
        let listener = tokio::net::TcpListener::bind(addr).await?;
        let bound = listener.local_addr()?;
        tracing::info!(addr = %bound, "LL-HLS HTTP bound");
        (Some(listener), Some(bound))
    } else {
        (None, None)
    };

    // Optional disk recorder.
    if let Some(ref dir) = config.record_dir {
        let recorder = lvqr_record::BroadcastRecorder::new(dir);
        let origin = relay.origin().clone();
        let event_rx = events.subscribe();
        let record_shutdown = shutdown.clone();
        tracing::info!(dir = %dir.display(), "recording enabled");
        tokio::spawn(async move {
            spawn_recordings(recorder, origin, event_rx, record_shutdown).await;
        });
    }

    // Admin HTTP state and router.
    let metrics_state = relay.metrics().clone();
    let bridge_for_stats = bridge.clone();
    let bridge_for_streams = bridge.clone();

    let admin_state = lvqr_admin::AdminState::new(
        move || {
            let active = bridge_for_stats.active_stream_count() as u64;
            lvqr_core::RelayStats {
                publishers: active,
                tracks: active * 2,
                subscribers: metrics_state.connections_active.load(Ordering::Relaxed),
                bytes_received: 0,
                bytes_sent: 0,
                uptime_secs: 0,
            }
        },
        move || {
            bridge_for_streams
                .stream_names()
                .into_iter()
                .map(|name| lvqr_admin::StreamInfo { name, subscribers: 0 })
                .collect()
        },
    )
    .with_auth(auth.clone());
    let admin_state = if let Some(prom) = prom_handle {
        admin_state.with_metrics(Arc::new(move || prom.render()))
    } else {
        admin_state
    };

    // WebSocket fMP4 relay + WebSocket ingest state.
    let ws_state = WsRelayState {
        origin: relay.origin().clone(),
        init_segments: Arc::new(dashmap::DashMap::new()),
        auth: auth.clone(),
        events: events.clone(),
    };
    let ws_router = axum::Router::new()
        .route("/ws/{*broadcast}", get(ws_relay_handler))
        .route("/ingest/{*broadcast}", get(ws_ingest_handler))
        .with_state(ws_state);

    let combined_router = {
        let admin_router = if config.mesh_enabled {
            let mesh_config = lvqr_mesh::MeshConfig {
                max_children: config.max_peers,
                ..Default::default()
            };
            let mesh = Arc::new(lvqr_mesh::MeshCoordinator::new(mesh_config));

            let mesh_for_cb = mesh.clone();
            relay.set_connection_callback(Arc::new(move |conn_id, connected| {
                let peer_id = format!("conn-{conn_id}");
                if connected {
                    match mesh_for_cb.add_peer(peer_id.clone(), "default".to_string()) {
                        Ok(a) => {
                            tracing::info!(peer = %peer_id, role = ?a.role, depth = a.depth, "mesh: peer assigned");
                        }
                        Err(e) => {
                            tracing::warn!(peer = %peer_id, error = ?e, "mesh: assign failed");
                        }
                    }
                } else {
                    let orphans = mesh_for_cb.remove_peer(&peer_id);
                    for orphan in orphans {
                        let _ = mesh_for_cb.reassign_peer(&orphan);
                    }
                }
            }));

            let mesh_for_reaper = mesh.clone();
            let reaper_shutdown = shutdown.clone();
            tokio::spawn(async move {
                let mut interval = tokio::time::interval(std::time::Duration::from_secs(5));
                loop {
                    tokio::select! {
                        _ = interval.tick() => {
                            let dead = mesh_for_reaper.find_dead_peers();
                            for peer_id in dead {
                                tracing::info!(peer = %peer_id, "mesh: removing dead peer");
                                let orphans = mesh_for_reaper.remove_peer(&peer_id);
                                for orphan in orphans {
                                    let _ = mesh_for_reaper.reassign_peer(&orphan);
                                }
                            }
                        }
                        _ = reaper_shutdown.cancelled() => {
                            tracing::debug!("mesh reaper shutting down");
                            break;
                        }
                    }
                }
            });

            let mesh_for_signal = mesh.clone();
            let mut signal = lvqr_signal::SignalServer::new();
            signal.set_peer_callback(Arc::new(move |peer_id, track, connected| {
                if connected {
                    match mesh_for_signal.add_peer(peer_id.to_string(), track.to_string()) {
                        Ok(a) => {
                            tracing::info!(peer = %peer_id, role = ?a.role, depth = a.depth, "mesh: signal peer assigned");
                            Some(lvqr_signal::SignalMessage::AssignParent {
                                peer_id: peer_id.to_string(),
                                role: format!("{:?}", a.role),
                                parent_id: a.parent,
                                depth: a.depth,
                            })
                        }
                        Err(e) => {
                            tracing::warn!(peer = %peer_id, error = ?e, "mesh: signal assign failed");
                            None
                        }
                    }
                } else {
                    let orphans = mesh_for_signal.remove_peer(peer_id);
                    for orphan in orphans {
                        let _ = mesh_for_signal.reassign_peer(&orphan);
                    }
                    None
                }
            }));

            let mesh_for_admin = mesh.clone();
            let admin_with_mesh = admin_state.with_mesh(move || lvqr_admin::MeshState {
                enabled: true,
                peer_count: mesh_for_admin.peer_count(),
                offload_percentage: mesh_for_admin.offload_percentage(),
            });

            tracing::info!(
                max_children = config.max_peers,
                "peer mesh enabled (/signal endpoint active)"
            );

            let router = lvqr_admin::build_router(admin_with_mesh);
            router.merge(signal.router())
        } else {
            lvqr_admin::build_router(admin_state)
        };

        admin_router.merge(ws_router)
    }
    .layer(CorsLayer::permissive());

    // Spawn a single background task that joins relay + RTMP + admin and
    // signals the shared shutdown token if any subsystem exits early.
    let relay_shutdown = shutdown.clone();
    let rtmp_shutdown = shutdown.clone();
    let admin_shutdown = shutdown.clone();
    let hls_shutdown = shutdown.clone();
    let bg_shutdown_for_task = shutdown.clone();
    let hls_router_pair =
        hls_listener.map(|listener| (listener, hls_server.expect("hls_server set when listener is set")));

    let join = tokio::spawn(async move {
        let shutdown_on_exit_relay = bg_shutdown_for_task.clone();
        let relay_fut = async move {
            if let Err(e) = relay.accept_loop(&mut moq_server, relay_shutdown).await {
                tracing::error!(error = %e, "relay server error");
            }
            shutdown_on_exit_relay.cancel();
        };

        let shutdown_on_exit_rtmp = bg_shutdown_for_task.clone();
        let rtmp_server_task = rtmp_server;
        let rtmp_fut = async move {
            if let Err(e) = rtmp_server_task.run_with_listener(rtmp_listener, rtmp_shutdown).await {
                tracing::error!(error = %e, "RTMP server error");
            }
            shutdown_on_exit_rtmp.cancel();
        };

        let shutdown_on_exit_admin = bg_shutdown_for_task.clone();
        let admin_fut = async move {
            let result = axum::serve(admin_listener, combined_router)
                .with_graceful_shutdown(async move { admin_shutdown.cancelled().await })
                .await;
            if let Err(e) = &result {
                tracing::error!(error = %e, "admin server error");
            }
            shutdown_on_exit_admin.cancel();
        };

        let shutdown_on_exit_hls = bg_shutdown_for_task.clone();
        let hls_fut = async move {
            let Some((listener, server)) = hls_router_pair else {
                return;
            };
            let router = server.router();
            let result = axum::serve(listener, router)
                .with_graceful_shutdown(async move { hls_shutdown.cancelled().await })
                .await;
            if let Err(e) = &result {
                tracing::error!(error = %e, "HLS server error");
            }
            shutdown_on_exit_hls.cancel();
        };

        let _ = tokio::join!(relay_fut, rtmp_fut, admin_fut, hls_fut);
        tracing::info!("shutdown complete");
    });

    Ok(ServerHandle {
        relay_addr: relay_bound,
        rtmp_addr: rtmp_bound,
        admin_addr: admin_bound,
        hls_addr: hls_bound,
        shutdown,
        join: Some(join),
    })
}

// =====================================================================
// Internal WS relay + WS ingest handlers
// =====================================================================

/// Shared state for WebSocket relay and ingest handlers.
#[derive(Clone)]
struct WsRelayState {
    origin: lvqr_moq::OriginProducer,
    /// Stored init segments per broadcast, so viewers get them immediately
    /// on connect.
    init_segments: Arc<dashmap::DashMap<String, Bytes>>,
    /// Authentication provider applied to WS subscribe and ingest sessions.
    auth: SharedAuth,
    /// Lifecycle event bus.
    events: EventBus,
}

async fn ws_relay_handler(
    ws: WebSocketUpgrade,
    State(state): State<WsRelayState>,
    Path(broadcast): Path<String>,
    Query(params): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Response {
    tracing::info!(broadcast = %broadcast, "WebSocket relay request");
    let resolved = resolve_ws_token(&headers, &params, "ws_subscribe");
    let decision = state.auth.check(&AuthContext::Subscribe {
        token: resolved.token,
        broadcast: broadcast.clone(),
    });
    if let AuthDecision::Deny { reason } = decision {
        tracing::warn!(broadcast = %broadcast, reason = %reason, "WS relay denied");
        metrics::counter!("lvqr_auth_failures_total", "entry" => "ws").increment(1);
        return (StatusCode::UNAUTHORIZED, reason).into_response();
    }
    metrics::counter!("lvqr_ws_connections_total", "direction" => "subscribe").increment(1);
    let ws = match resolved.offered_subprotocol {
        Some(ref p) => ws.protocols(std::iter::once(p.clone())),
        None => ws,
    };
    ws.on_upgrade(move |socket| ws_relay_session(socket, state, broadcast))
        .into_response()
}

async fn ws_relay_session(mut socket: WebSocket, state: WsRelayState, broadcast: String) {
    let consumer = state.origin.consume();
    let Some(bc) = consumer.consume_broadcast(&broadcast) else {
        tracing::warn!(broadcast = %broadcast, "broadcast not found for WS relay");
        let _ = socket
            .send(Message::Close(Some(axum::extract::ws::CloseFrame {
                code: 4404,
                reason: "broadcast not found".into(),
            })))
            .await;
        return;
    };

    tracing::info!(broadcast = %broadcast, "WS relay session started");

    let video_track = bc.subscribe_track(&Track::new("0.mp4")).ok();
    let audio_track = bc.subscribe_track(&Track::new("1.mp4")).ok();

    if video_track.is_none() && audio_track.is_none() {
        tracing::warn!(broadcast = %broadcast, "no playable tracks for WS relay");
        return;
    }

    let (tx, mut rx) = tokio::sync::mpsc::channel::<(u8, Bytes)>(64);
    let cancel = CancellationToken::new();

    if let Some(track) = video_track {
        let tx = tx.clone();
        let cancel = cancel.clone();
        tokio::spawn(async move {
            relay_track(track, 0u8, tx, cancel).await;
        });
    }
    if let Some(track) = audio_track {
        let tx = tx.clone();
        let cancel = cancel.clone();
        tokio::spawn(async move {
            relay_track(track, 1u8, tx, cancel).await;
        });
    }
    drop(tx);

    while let Some((track_id, payload)) = rx.recv().await {
        let mut framed = Vec::with_capacity(1 + payload.len());
        framed.push(track_id);
        framed.extend_from_slice(&payload);
        let len = framed.len() as u64;
        if let Err(e) = socket.send(Message::Binary(framed.into())).await {
            tracing::debug!(error = ?e, "WS send error");
            break;
        }
        metrics::counter!("lvqr_frames_relayed_total", "transport" => "ws").increment(1);
        metrics::counter!("lvqr_bytes_relayed_total", "transport" => "ws").increment(len);
    }

    cancel.cancel();
    tracing::info!(broadcast = %broadcast, "WS relay session ended");
}

async fn relay_track(
    mut track: lvqr_moq::TrackConsumer,
    track_id: u8,
    tx: tokio::sync::mpsc::Sender<(u8, Bytes)>,
    cancel: CancellationToken,
) {
    loop {
        let group = tokio::select! {
            res = track.next_group() => res,
            _ = cancel.cancelled() => return,
        };
        let mut group = match group {
            Ok(Some(g)) => g,
            Ok(None) => return,
            Err(e) => {
                tracing::debug!(track_id, error = ?e, "track error");
                return;
            }
        };
        loop {
            let frame = tokio::select! {
                res = group.read_frame() => res,
                _ = cancel.cancelled() => return,
            };
            match frame {
                Ok(Some(bytes)) => {
                    if tx.send((track_id, bytes)).await.is_err() {
                        return;
                    }
                }
                Ok(None) => break,
                Err(e) => {
                    tracing::debug!(track_id, error = ?e, "group read error");
                    return;
                }
            }
        }
    }
}

async fn ws_ingest_handler(
    ws: WebSocketUpgrade,
    State(state): State<WsRelayState>,
    Path(broadcast): Path<String>,
    Query(params): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Response {
    tracing::info!(broadcast = %broadcast, "WebSocket ingest request");
    let resolved = resolve_ws_token(&headers, &params, "ws_ingest");
    let decision = state.auth.check(&AuthContext::Publish {
        app: "ws".to_string(),
        key: resolved.token.clone().unwrap_or_default(),
    });
    if let AuthDecision::Deny { reason } = decision {
        tracing::warn!(broadcast = %broadcast, reason = %reason, "WS ingest denied");
        metrics::counter!("lvqr_auth_failures_total", "entry" => "ws_ingest").increment(1);
        return (StatusCode::UNAUTHORIZED, reason).into_response();
    }
    metrics::counter!("lvqr_ws_connections_total", "direction" => "publish").increment(1);
    let ws = match resolved.offered_subprotocol {
        Some(ref p) => ws.protocols(std::iter::once(p.clone())),
        None => ws,
    };
    ws.on_upgrade(move |socket| ws_ingest_session(socket, state, broadcast))
        .into_response()
}

/// Result of extracting a bearer token from a WebSocket upgrade request.
///
/// The preferred transport is the `Sec-WebSocket-Protocol` header with a
/// value of `lvqr.bearer.<token>`. When the client offers that, the
/// matching subprotocol string is echoed back so axum's upgrade handshake
/// accepts it. The legacy `?token=` query parameter is still accepted as
/// a fallback but logs a deprecation warning.
struct WsTokenResolution {
    token: Option<String>,
    offered_subprotocol: Option<String>,
}

fn resolve_ws_token(headers: &HeaderMap, params: &HashMap<String, String>, entry: &'static str) -> WsTokenResolution {
    if let Some(hv) = headers.get("sec-websocket-protocol")
        && let Ok(raw) = hv.to_str()
    {
        for item in raw.split(',') {
            let proto = item.trim();
            if let Some(tok) = proto.strip_prefix("lvqr.bearer.")
                && !tok.is_empty()
            {
                return WsTokenResolution {
                    token: Some(tok.to_string()),
                    offered_subprotocol: Some(proto.to_string()),
                };
            }
        }
    }

    if let Some(tok) = params.get("token").filter(|t| !t.is_empty()) {
        tracing::warn!(
            entry = entry,
            "deprecated: ?token= query parameter; migrate to Sec-WebSocket-Protocol: lvqr.bearer.<token>"
        );
        return WsTokenResolution {
            token: Some(tok.clone()),
            offered_subprotocol: None,
        };
    }

    WsTokenResolution {
        token: None,
        offered_subprotocol: None,
    }
}

async fn ws_ingest_session(mut socket: WebSocket, state: WsRelayState, broadcast: String) {
    use lvqr_ingest::remux;

    tracing::info!(broadcast = %broadcast, "WS ingest session started");

    let Some(mut bc) = state.origin.create_broadcast(&broadcast) else {
        tracing::warn!(broadcast = %broadcast, "broadcast creation failed");
        let _ = socket
            .send(Message::Close(Some(axum::extract::ws::CloseFrame {
                code: 4409,
                reason: "broadcast already exists".into(),
            })))
            .await;
        return;
    };

    state.events.emit(RelayEvent::BroadcastStarted {
        name: broadcast.clone(),
    });

    let Ok(mut video_track) = bc.create_track(Track::new("0.mp4")) else {
        tracing::warn!("failed to create video track");
        return;
    };
    let Ok(mut catalog_track) = bc.create_track(Track::new(".catalog")) else {
        tracing::warn!("failed to create catalog track");
        return;
    };

    let mut _video_config: Option<remux::VideoConfig> = None;
    let mut video_init: Option<Bytes> = None;
    let mut video_group: Option<lvqr_moq::GroupProducer> = None;
    let mut video_seq: u32 = 0;
    let _ = socket.send(Message::Text(r#"{"status":"ready"}"#.into())).await;

    while let Some(msg) = socket.recv().await {
        let data = match msg {
            Ok(Message::Binary(data)) => data,
            Ok(Message::Close(_)) => break,
            Ok(_) => continue,
            Err(e) => {
                tracing::debug!(error = ?e, "WS ingest recv error");
                break;
            }
        };

        if data.len() < 5 {
            continue;
        }

        let msg_type = data[0];
        let timestamp = u32::from_be_bytes([data[1], data[2], data[3], data[4]]);
        let payload = Bytes::from(data[5..].to_vec());

        match msg_type {
            0 => {
                if payload.len() < 6 {
                    continue;
                }
                let vid_width = u16::from_be_bytes([payload[0], payload[1]]);
                let vid_height = u16::from_be_bytes([payload[2], payload[3]]);
                let avcc_data = &payload[4..];

                match parse_avcc_record(avcc_data) {
                    Some(config) => {
                        tracing::info!(
                            broadcast = %broadcast,
                            codec = %config.codec_string(),
                            width = vid_width,
                            height = vid_height,
                            "WS ingest: video config received"
                        );
                        let init = remux::video_init_segment_with_size(&config, vid_width, vid_height);
                        _video_config = Some(config.clone());
                        video_init = Some(init.clone());
                        state.init_segments.insert(broadcast.clone(), init);

                        let json = remux::generate_catalog(Some(&config), None);
                        if let Ok(mut group) = catalog_track.append_group() {
                            let _ = group.write_frame(Bytes::from(json));
                            let _ = group.finish();
                        }
                    }
                    None => {
                        tracing::warn!("invalid AVCC record from browser");
                    }
                }
            }
            1 => {
                let Some(ref init) = video_init else { continue };
                if let Some(mut g) = video_group.take() {
                    let _ = g.finish();
                }
                video_seq += 1;
                let base_dts = (timestamp as u64) * 90;
                let sample = lvqr_cmaf::RawSample {
                    track_id: 1,
                    dts: base_dts,
                    cts_offset: 0,
                    duration: 3000,
                    payload,
                    keyframe: true,
                };
                if let Ok(mut group) = video_track.append_group() {
                    let _ = group.write_frame(init.clone());
                    let seg = lvqr_cmaf::build_moof_mdat(video_seq, 1, base_dts, &[sample]);
                    let _ = group.write_frame(seg);
                    video_group = Some(group);
                }
            }
            2 => {
                if video_init.is_none() {
                    continue;
                }
                video_seq += 1;
                let base_dts = (timestamp as u64) * 90;
                let sample = lvqr_cmaf::RawSample {
                    track_id: 1,
                    dts: base_dts,
                    cts_offset: 0,
                    duration: 3000,
                    payload,
                    keyframe: false,
                };
                if let Some(ref mut group) = video_group {
                    let seg = lvqr_cmaf::build_moof_mdat(video_seq, 1, base_dts, &[sample]);
                    let _ = group.write_frame(seg);
                }
            }
            _ => {}
        }
    }

    if let Some(mut g) = video_group.take() {
        let _ = g.finish();
    }
    state.events.emit(RelayEvent::BroadcastStopped {
        name: broadcast.clone(),
    });
    tracing::info!(broadcast = %broadcast, "WS ingest session ended");
}

/// Background task that listens on the event bus for new broadcasts and
/// starts a recorder for each one. Event-driven so WS-ingested broadcasts
/// are recorded identically to RTMP-ingested ones.
async fn spawn_recordings(
    recorder: lvqr_record::BroadcastRecorder,
    origin: lvqr_moq::OriginProducer,
    mut events: tokio::sync::broadcast::Receiver<RelayEvent>,
    shutdown: CancellationToken,
) {
    let mut active: std::collections::HashSet<String> = std::collections::HashSet::new();
    loop {
        let event = tokio::select! {
            res = events.recv() => res,
            _ = shutdown.cancelled() => return,
        };
        let event = match event {
            Ok(e) => e,
            Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                tracing::warn!(missed = n, "recorder event stream lagged");
                continue;
            }
            Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
        };
        match event {
            RelayEvent::BroadcastStarted { name } => {
                if !active.insert(name.clone()) {
                    continue;
                }
                let consumer = origin.consume();
                let Some(broadcast) = consumer.consume_broadcast(&name) else {
                    tracing::warn!(broadcast = %name, "recorder: broadcast not resolvable yet");
                    active.remove(&name);
                    continue;
                };
                let recorder = recorder.clone();
                let cancel = shutdown.clone();
                tracing::info!(broadcast = %name, "starting recording");
                let name_clone = name.clone();
                tokio::spawn(async move {
                    let _ = recorder
                        .record_broadcast(&name_clone, broadcast, lvqr_record::RecordOptions::default(), cancel)
                        .await;
                });
            }
            RelayEvent::BroadcastStopped { name } => {
                active.remove(&name);
            }
            RelayEvent::ViewerJoined { .. } | RelayEvent::ViewerLeft { .. } => {}
        }
    }
}

/// Parse an AVCDecoderConfigurationRecord from a WS ingest `type=0` payload.
fn parse_avcc_record(data: &[u8]) -> Option<lvqr_ingest::remux::VideoConfig> {
    if data.len() < 6 {
        return None;
    }
    let profile = data[1];
    let compat = data[2];
    let level = data[3];
    let nalu_length_size = (data[4] & 0x03) + 1;

    let num_sps = (data[5] & 0x1F) as usize;
    let mut offset = 6;
    let mut sps_list = Vec::with_capacity(num_sps);
    for _ in 0..num_sps {
        if offset + 2 > data.len() {
            return None;
        }
        let len = u16::from_be_bytes([data[offset], data[offset + 1]]) as usize;
        offset += 2;
        if offset + len > data.len() {
            return None;
        }
        sps_list.push(data[offset..offset + len].to_vec());
        offset += len;
    }

    if offset >= data.len() {
        return None;
    }
    let num_pps = data[offset] as usize;
    offset += 1;
    let mut pps_list = Vec::with_capacity(num_pps);
    for _ in 0..num_pps {
        if offset + 2 > data.len() {
            return None;
        }
        let len = u16::from_be_bytes([data[offset], data[offset + 1]]) as usize;
        offset += 2;
        if offset + len > data.len() {
            return None;
        }
        pps_list.push(data[offset..offset + len].to_vec());
        offset += len;
    }

    if sps_list.is_empty() || pps_list.is_empty() {
        return None;
    }

    Some(lvqr_ingest::remux::VideoConfig {
        sps_list,
        pps_list,
        profile,
        compat,
        level,
        nalu_length_size,
    })
}
