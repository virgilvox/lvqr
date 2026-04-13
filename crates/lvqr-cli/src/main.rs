use anyhow::Result;
use axum::extract::ws::{Message, WebSocket};
use axum::extract::{Path, Query, State, WebSocketUpgrade};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use bytes::Bytes;
use clap::Parser;
use lvqr_auth::{
    AuthContext, AuthDecision, JwtAuthConfig, JwtAuthProvider, NoopAuthProvider, SharedAuth, StaticAuthConfig,
    StaticAuthProvider,
};
use lvqr_core::{EventBus, RelayEvent};
use moq_lite::Track;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use tokio_util::sync::CancellationToken;
use tower_http::cors::CorsLayer;

#[derive(Parser, Debug)]
#[command(name = "lvqr", version, about = "Live Video QUIC Relay")]
enum Cli {
    /// Start the LVQR relay server.
    Serve(ServeArgs),
}

#[derive(Parser, Debug)]
struct ServeArgs {
    /// QUIC/MoQ listen port.
    #[arg(long, default_value = "4443", env = "LVQR_PORT")]
    port: u16,

    /// RTMP ingest listen port.
    #[arg(long, default_value = "1935", env = "LVQR_RTMP_PORT")]
    rtmp_port: u16,

    /// Admin HTTP API listen port.
    #[arg(long, default_value = "8080", env = "LVQR_ADMIN_PORT")]
    admin_port: u16,

    /// Enable peer mesh relay.
    #[arg(long, env = "LVQR_MESH_ENABLED")]
    mesh_enabled: bool,

    /// Max peer relay connections per viewer.
    #[arg(long, default_value = "3", env = "LVQR_MAX_PEERS")]
    max_peers: usize,

    /// Path to TLS certificate (PEM). Auto-generates self-signed if omitted.
    #[arg(long, env = "LVQR_TLS_CERT")]
    tls_cert: Option<String>,

    /// Path to TLS private key (PEM). Auto-generates self-signed if omitted.
    #[arg(long, env = "LVQR_TLS_KEY")]
    tls_key: Option<String>,

    /// Path to TOML config file.
    #[arg(long, short, env = "LVQR_CONFIG")]
    config: Option<String>,

    /// Bearer token required for /api/v1/* admin endpoints. Leave unset for open access.
    #[arg(long, env = "LVQR_ADMIN_TOKEN")]
    admin_token: Option<String>,

    /// Required publish key (RTMP stream key, WS ingest ?token=). Leave unset for open access.
    #[arg(long, env = "LVQR_PUBLISH_KEY")]
    publish_key: Option<String>,

    /// Required viewer token (WS relay/MoQ subscribe ?token=). Leave unset for open access.
    #[arg(long, env = "LVQR_SUBSCRIBE_TOKEN")]
    subscribe_token: Option<String>,

    /// Directory to record broadcasts into. Omit to disable recording.
    #[arg(long, env = "LVQR_RECORD_DIR")]
    record_dir: Option<String>,

    /// HS256 shared secret enabling JWT authentication. When set, the JWT
    /// provider replaces the static-token provider and all auth surfaces
    /// validate bearer tokens as signed JWTs.
    #[arg(long, env = "LVQR_JWT_SECRET")]
    jwt_secret: Option<String>,

    /// Expected `iss` claim for JWT validation. When unset, issuer is not
    /// checked. Only meaningful with `--jwt-secret`.
    #[arg(long, env = "LVQR_JWT_ISSUER")]
    jwt_issuer: Option<String>,

    /// Expected `aud` claim for JWT validation. When unset, audience is not
    /// checked. Only meaningful with `--jwt-secret`.
    #[arg(long, env = "LVQR_JWT_AUDIENCE")]
    jwt_audience: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "lvqr=info".parse().unwrap()),
        )
        .init();

    let cli = Cli::parse();

    match cli {
        Cli::Serve(args) => serve(args).await,
    }
}

/// Shared state for WebSocket relay and ingest handlers.
#[derive(Clone)]
struct WsRelayState {
    origin: moq_lite::OriginProducer,
    /// Stored init segments per broadcast, so viewers get them immediately on connect.
    init_segments: Arc<dashmap::DashMap<String, Bytes>>,
    /// Authentication provider applied to WS subscribe and ingest sessions.
    auth: SharedAuth,
    /// Event bus so ingest sessions can publish lifecycle events that the
    /// recorder (and future hooks) consume.
    events: EventBus,
}

async fn serve(args: ServeArgs) -> Result<()> {
    tracing::info!(
        quic_port = args.port,
        rtmp_port = args.rtmp_port,
        admin_port = args.admin_port,
        mesh = args.mesh_enabled,
        "starting LVQR relay"
    );

    // Install Prometheus exporter recorder so all `metrics::*` macro calls
    // throughout the workspace are captured for the /metrics endpoint.
    let prom_handle = metrics_exporter_prometheus::PrometheusBuilder::new()
        .install_recorder()
        .map_err(|e| anyhow::anyhow!("failed to install Prometheus recorder: {e}"))?;

    // Cancellation token used to coordinate graceful shutdown across all subsystems.
    let shutdown = CancellationToken::new();
    let shutdown_signal = shutdown.clone();
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            tracing::info!("ctrl-c received, initiating graceful shutdown");
            shutdown_signal.cancel();
        }
    });

    // Build authentication provider from CLI/env. JWT takes precedence when
    // `--jwt-secret` is set: every auth surface then validates bearer tokens
    // as HS256-signed JWTs with the configured issuer and audience. Otherwise
    // fall back to the static-token provider when any individual token is
    // configured, and finally to `NoopAuthProvider` (open access) when nothing
    // is set so v0.3.1 deployments continue working with no config changes.
    let auth: SharedAuth = if let Some(secret) = args.jwt_secret.clone() {
        tracing::info!(
            issuer = args.jwt_issuer.is_some(),
            audience = args.jwt_audience.is_some(),
            "auth: JWT provider enabled"
        );
        let provider = JwtAuthProvider::new(JwtAuthConfig {
            secret,
            issuer: args.jwt_issuer.clone(),
            audience: args.jwt_audience.clone(),
        })
        .map_err(|e| anyhow::anyhow!("failed to init JWT auth provider: {e}"))?;
        Arc::new(provider)
    } else {
        let auth_config = StaticAuthConfig {
            admin_token: args.admin_token.clone(),
            publish_key: args.publish_key.clone(),
            subscribe_token: args.subscribe_token.clone(),
        };
        if auth_config.has_any() {
            tracing::info!(
                admin = auth_config.admin_token.is_some(),
                publish = auth_config.publish_key.is_some(),
                subscribe = auth_config.subscribe_token.is_some(),
                "auth: static-token provider enabled"
            );
            Arc::new(StaticAuthProvider::new(auth_config)) as SharedAuth
        } else {
            tracing::info!("auth: open access (no tokens configured)");
            Arc::new(NoopAuthProvider) as SharedAuth
        }
    };

    // Single process-wide event bus: lifecycle events (broadcast started/stopped,
    // viewer joined/left) are broadcast on this, and any hook (recorder,
    // webhook dispatcher, future webhook/ops tools) subscribes to it.
    let events = EventBus::default();

    // MoQ relay
    let relay_config = lvqr_relay::RelayConfig::new(([0, 0, 0, 0], args.port).into());
    let mut relay = lvqr_relay::RelayServer::new(relay_config);
    relay.set_auth_provider(auth.clone());
    let (mut moq_server, relay_addr) = relay.init_server()?;
    tracing::info!(addr = %relay_addr, "MoQ relay listening");

    // RTMP ingest bridged to MoQ. The bridge emits BroadcastStarted/Stopped
    // on the shared EventBus so the recorder does not have to poll.
    let bridge = Arc::new(
        lvqr_ingest::RtmpMoqBridge::with_auth(relay.origin().clone(), auth.clone()).with_events(events.clone()),
    );
    let rtmp_config = lvqr_ingest::RtmpConfig {
        bind_addr: ([0, 0, 0, 0], args.rtmp_port).into(),
    };
    let rtmp_server = bridge.create_rtmp_server(rtmp_config);

    // Optional disk recorder. When --record-dir is set, every broadcast is
    // recorded asynchronously by an extra MoQ subscriber. The recorder
    // listens to lifecycle events rather than polling the RTMP bridge, so
    // WS-ingested broadcasts are recorded too.
    if let Some(ref dir) = args.record_dir {
        let recorder = lvqr_record::BroadcastRecorder::new(dir);
        let origin = relay.origin().clone();
        let event_rx = events.subscribe();
        let record_shutdown = shutdown.clone();
        tracing::info!(dir = %dir, "recording enabled");
        tokio::spawn(async move {
            spawn_recordings(recorder, origin, event_rx, record_shutdown).await;
        });
    }

    // Admin HTTP API wired to real relay metrics and bridge state
    let metrics = relay.metrics().clone();
    let bridge_for_stats = bridge.clone();
    let bridge_for_streams = bridge.clone();

    let admin_state = lvqr_admin::AdminState::new(
        move || {
            let active = bridge_for_stats.active_stream_count() as u64;
            lvqr_core::RelayStats {
                publishers: active,
                tracks: active * 2,
                subscribers: metrics.connections_active.load(Ordering::Relaxed),
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
    .with_auth(auth.clone())
    .with_metrics(Arc::new(move || prom_handle.render()));

    let admin_addr: std::net::SocketAddr = ([0, 0, 0, 0], args.admin_port).into();

    // WebSocket fMP4 relay + WebSocket ingest
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
        let admin_router = if args.mesh_enabled {
            // Set up mesh coordinator
            let mesh_config = lvqr_mesh::MeshConfig {
                max_children: args.max_peers,
                ..Default::default()
            };
            let mesh = Arc::new(lvqr_mesh::MeshCoordinator::new(mesh_config));

            // Wire mesh to relay connection events
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

            // Background dead peer detection
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

            // Wire signal server with mesh assignments
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
                max_children = args.max_peers,
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

    tracing::info!(addr = %admin_addr, "admin API listening");

    // Run all servers concurrently and wait for every subsystem to drain.
    //
    // Each subsystem respects the shared cancellation token, so ctrl-c (which
    // cancels the token from the signal task above) triggers orderly shutdown
    // in every subsystem at once. If any subsystem exits on its own (error or
    // natural completion), its wrapper fires the token so the others also
    // stop. Using `tokio::join!` instead of `tokio::select!` guarantees we
    // wait for all three to finish rather than racing cancellation against
    // in-flight work, which previously truncated the final GOP on ctrl-c.
    let relay_shutdown = shutdown.clone();
    let rtmp_shutdown = shutdown.clone();
    let admin_shutdown = shutdown.clone();

    let shutdown_on_exit_relay = shutdown.clone();
    let relay_fut = async move {
        let result = relay.accept_loop(&mut moq_server, relay_shutdown).await;
        if let Err(e) = &result {
            tracing::error!(error = %e, "relay server error");
        }
        shutdown_on_exit_relay.cancel();
        result
    };

    let shutdown_on_exit_rtmp = shutdown.clone();
    let rtmp_fut = async move {
        let result = rtmp_server.run(rtmp_shutdown).await;
        if let Err(e) = &result {
            tracing::error!(error = %e, "RTMP server error");
        }
        shutdown_on_exit_rtmp.cancel();
        result
    };

    let shutdown_on_exit_admin = shutdown.clone();
    let admin_fut = async move {
        let result: Result<()> = async {
            let listener = tokio::net::TcpListener::bind(admin_addr).await?;
            axum::serve(listener, combined_router)
                .with_graceful_shutdown(async move { admin_shutdown.cancelled().await })
                .await?;
            Ok(())
        }
        .await;
        if let Err(e) = &result {
            tracing::error!(error = %e, "admin server error");
        }
        shutdown_on_exit_admin.cancel();
        result
    };

    let _ = tokio::join!(relay_fut, rtmp_fut, admin_fut);

    tracing::info!("shutdown complete");
    Ok(())
}

/// WebSocket relay handler: upgrades to WS, subscribes to MoQ tracks,
/// forwards fMP4 frames as binary messages.
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

/// Handle a single WebSocket relay session.
///
/// Wire format: `[u8 track_id][fMP4 payload]`
///   - track_id 0 = video (0.mp4)
///   - track_id 1 = audio (1.mp4)
///
/// Both tracks are multiplexed onto the single WebSocket via an internal mpsc
/// channel. This is a breaking change vs. the v0.3.x raw-binary protocol.
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

    // Subscribe to video and audio tracks. Both are optional.
    let video_track = bc.subscribe_track(&Track::new("0.mp4")).ok();
    let audio_track = bc.subscribe_track(&Track::new("1.mp4")).ok();

    if video_track.is_none() && audio_track.is_none() {
        tracing::warn!(broadcast = %broadcast, "no playable tracks for WS relay");
        return;
    }

    // mpsc channel multiplexes (track_id, payload) from track readers to the socket writer.
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

/// Read groups+frames from a MoQ track and forward each frame to the mpsc
/// channel tagged with the supplied track_id. Stops when the track ends, the
/// receiver is dropped, or `cancel` fires.
async fn relay_track(
    mut track: moq_lite::TrackConsumer,
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

// =====================================================================
// WebSocket Ingest: browser VideoEncoder H.264 -> fMP4 -> MoQ
// =====================================================================
//
// Wire format (binary WebSocket messages):
//   [u8 type][u32 BE timestamp_ms][payload]
//
// Types:
//   0 = video config (AVCDecoderConfigurationRecord from VideoEncoder)
//   1 = video keyframe (AVCC-format NALUs)
//   2 = video delta frame (AVCC-format NALUs)

/// WebSocket ingest handler: browser pushes H.264 frames, server publishes to MoQ.
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
/// The preferred transport is the `Sec-WebSocket-Protocol` header with a value
/// of `lvqr.bearer.<token>`. When the client offers that, the matching
/// subprotocol string is echoed back so axum's upgrade handshake accepts it.
/// The legacy `?token=` query parameter is still accepted as a fallback, but
/// logs a deprecation warning so operators can migrate clients.
struct WsTokenResolution {
    token: Option<String>,
    offered_subprotocol: Option<String>,
}

fn resolve_ws_token(headers: &HeaderMap, params: &HashMap<String, String>, entry: &'static str) -> WsTokenResolution {
    // Sec-WebSocket-Protocol is a comma-separated list. Find any entry
    // starting with the `lvqr.bearer.` prefix, strip it, and echo it back
    // verbatim so the upgrade response picks a valid subprotocol.
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

    // Legacy: ?token=... in the query string. Keep accepting it to avoid
    // hard-breaking v0.3.1 clients mid-upgrade, but warn so logs flag it.
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

    // Create MoQ broadcast and tracks
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

    // Announce this broadcast on the event bus so the recorder and other
    // subscribers can wire up before the first media frame arrives.
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
    let mut video_group: Option<moq_lite::GroupProducer> = None;
    let mut video_seq: u32 = 0;
    // Confirm to the browser that ingest is ready
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
            // Video config: [u16 BE width][u16 BE height][AVCDecoderConfigurationRecord]
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
            // Video keyframe
            1 => {
                let Some(ref init) = video_init else { continue };

                // Finish previous group
                if let Some(mut g) = video_group.take() {
                    let _ = g.finish();
                }

                video_seq += 1;
                let base_dts = (timestamp as u64) * 90;
                let sample = remux::VideoSample {
                    data: payload,
                    duration: 3000, // ~33ms at 90kHz
                    cts_offset: 0,
                    keyframe: true,
                };

                if let Ok(mut group) = video_track.append_group() {
                    let _ = group.write_frame(init.clone());
                    let seg = remux::video_segment(video_seq, base_dts, &[sample]);
                    let _ = group.write_frame(seg);
                    video_group = Some(group);
                }
            }
            // Video delta frame
            2 => {
                if video_init.is_none() {
                    continue;
                }

                video_seq += 1;
                let base_dts = (timestamp as u64) * 90;
                let sample = remux::VideoSample {
                    data: payload,
                    duration: 3000,
                    cts_offset: 0,
                    keyframe: false,
                };

                if let Some(ref mut group) = video_group {
                    let seg = remux::video_segment(video_seq, base_dts, &[sample]);
                    let _ = group.write_frame(seg);
                }
            }
            _ => {}
        }
    }

    // Cleanup
    if let Some(mut g) = video_group.take() {
        let _ = g.finish();
    }
    state.events.emit(RelayEvent::BroadcastStopped {
        name: broadcast.clone(),
    });
    tracing::info!(broadcast = %broadcast, "WS ingest session ended");
}

/// Background task that listens on the event bus for new broadcasts and
/// starts a recorder for each one. The recorder runs as a regular MoQ
/// subscriber, so it never affects the live data path. Recordings stop on
/// shutdown.
///
/// This is event-driven rather than polling the RTMP bridge, so WS-ingested
/// broadcasts (which never touch the RTMP bridge) are recorded identically.
async fn spawn_recordings(
    recorder: lvqr_record::BroadcastRecorder,
    origin: moq_lite::OriginProducer,
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
                // The per-broadcast recorder task observes the track ending
                // on its own and exits, so no explicit cancellation needed.
            }
            RelayEvent::ViewerJoined { .. } | RelayEvent::ViewerLeft { .. } => {}
        }
    }
}

/// Parse an AVCDecoderConfigurationRecord (from VideoEncoder's decoderConfig.description).
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
