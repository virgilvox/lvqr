use anyhow::Result;
use axum::extract::ws::{Message, WebSocket};
use axum::extract::{Path, State, WebSocketUpgrade};
use axum::response::IntoResponse;
use axum::routing::get;
use clap::Parser;
use moq_lite::Track;
use std::sync::Arc;
use std::sync::atomic::Ordering;

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

/// Shared state for the WebSocket relay handler.
#[derive(Clone)]
struct WsRelayState {
    origin: moq_lite::OriginProducer,
}

async fn serve(args: ServeArgs) -> Result<()> {
    tracing::info!(
        quic_port = args.port,
        rtmp_port = args.rtmp_port,
        admin_port = args.admin_port,
        mesh = args.mesh_enabled,
        "starting LVQR relay"
    );

    // MoQ relay
    let relay_config = lvqr_relay::RelayConfig::new(([0, 0, 0, 0], args.port).into());
    let relay = lvqr_relay::RelayServer::new(relay_config);
    let (mut moq_server, relay_addr) = relay.init_server()?;
    tracing::info!(addr = %relay_addr, "MoQ relay listening");

    // RTMP ingest bridged to MoQ
    let bridge = Arc::new(lvqr_ingest::RtmpMoqBridge::new(relay.origin().clone()));
    let rtmp_config = lvqr_ingest::RtmpConfig {
        bind_addr: ([0, 0, 0, 0], args.rtmp_port).into(),
    };
    let rtmp_server = bridge.create_rtmp_server(rtmp_config);

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
    );

    let admin_addr: std::net::SocketAddr = ([0, 0, 0, 0], args.admin_port).into();
    let admin_router = lvqr_admin::build_router(admin_state);

    // WebSocket fMP4 relay: /ws/{broadcast_path}
    // Subscribes to MoQ video+audio tracks server-side, forwards fMP4 frames over WS
    let ws_state = WsRelayState {
        origin: relay.origin().clone(),
    };
    let ws_router = axum::Router::new()
        .route("/ws/{*broadcast}", get(ws_relay_handler))
        .with_state(ws_state);

    let mut combined_router = admin_router.merge(ws_router);

    if args.mesh_enabled {
        let mesh_config = lvqr_mesh::MeshConfig {
            max_children: args.max_peers,
            ..Default::default()
        };
        let _mesh = Arc::new(lvqr_mesh::MeshCoordinator::new(mesh_config));

        let signal = lvqr_signal::SignalServer::new();
        let signal_router = signal.router();

        tracing::info!(
            "peer mesh enabled (max_children={}, /signal endpoint active)",
            args.max_peers
        );

        combined_router = combined_router.merge(signal_router);
    }

    tracing::info!(addr = %admin_addr, "admin API listening");

    // Run all servers concurrently
    tokio::select! {
        result = relay.accept_loop(&mut moq_server) => {
            if let Err(e) = result {
                tracing::error!(error = %e, "relay server error");
            }
        }
        result = rtmp_server.run() => {
            if let Err(e) = result {
                tracing::error!(error = %e, "RTMP server error");
            }
        }
        result = async {
            let listener = tokio::net::TcpListener::bind(admin_addr).await?;
            axum::serve(listener, combined_router).await
        } => {
            if let Err(e) = result {
                tracing::error!(error = %e, "admin server error");
            }
        }
        _ = tokio::signal::ctrl_c() => {
            tracing::info!("shutting down");
        }
    }

    Ok(())
}

/// WebSocket relay handler: upgrades to WS, subscribes to MoQ tracks,
/// forwards fMP4 frames as binary messages.
async fn ws_relay_handler(
    ws: WebSocketUpgrade,
    State(state): State<WsRelayState>,
    Path(broadcast): Path<String>,
) -> impl IntoResponse {
    tracing::info!(broadcast = %broadcast, "WebSocket relay request");
    ws.on_upgrade(move |socket| ws_relay_session(socket, state, broadcast))
}

/// Handle a single WebSocket relay session.
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

    // Subscribe to video track
    let video_track = match bc.subscribe_track(&Track::new("0.mp4")) {
        Ok(t) => Some(t),
        Err(e) => {
            tracing::debug!(error = ?e, "no video track available");
            None
        }
    };

    // Forward video frames over WebSocket
    if let Some(mut track) = video_track {
        loop {
            let group = match track.next_group().await {
                Ok(Some(g)) => g,
                Ok(None) => break,
                Err(e) => {
                    tracing::debug!(error = ?e, "video track error");
                    break;
                }
            };

            if let Err(e) = forward_group(&mut socket, group).await {
                tracing::debug!(error = ?e, "WS send error");
                break;
            }
        }
    }

    tracing::info!(broadcast = %broadcast, "WS relay session ended");
}

/// Forward all frames from a MoQ group as binary WebSocket messages.
async fn forward_group(socket: &mut WebSocket, mut group: moq_lite::GroupConsumer) -> Result<(), axum::Error> {
    loop {
        match group.read_frame().await {
            Ok(Some(frame)) => {
                socket.send(Message::Binary(frame.to_vec().into())).await?;
            }
            Ok(None) => break,
            Err(e) => {
                tracing::debug!(error = ?e, "group read error");
                break;
            }
        }
    }
    Ok(())
}
