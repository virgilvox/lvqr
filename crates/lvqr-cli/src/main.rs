use anyhow::Result;
use axum::extract::ws::{Message, WebSocket};
use axum::extract::{Path, State, WebSocketUpgrade};
use axum::response::IntoResponse;
use axum::routing::get;
use bytes::Bytes;
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
    let mut relay = lvqr_relay::RelayServer::new(relay_config);
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

    // WebSocket fMP4 relay + WebSocket ingest
    let ws_state = WsRelayState {
        origin: relay.origin().clone(),
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
            tokio::spawn(async move {
                let mut interval = tokio::time::interval(std::time::Duration::from_secs(5));
                loop {
                    interval.tick().await;
                    let dead = mesh_for_reaper.find_dead_peers();
                    for peer_id in dead {
                        tracing::info!(peer = %peer_id, "mesh: removing dead peer");
                        let orphans = mesh_for_reaper.remove_peer(&peer_id);
                        for orphan in orphans {
                            let _ = mesh_for_reaper.reassign_peer(&orphan);
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
    };

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
) -> impl IntoResponse {
    tracing::info!(broadcast = %broadcast, "WebSocket ingest request");
    ws.on_upgrade(move |socket| ws_ingest_session(socket, state, broadcast))
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
    let mut catalog_written = false;

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
            // Video config: AVCDecoderConfigurationRecord
            0 => match parse_avcc_record(&payload) {
                Some(config) => {
                    tracing::info!(
                        broadcast = %broadcast,
                        codec = %config.codec_string(),
                        "WS ingest: video config received"
                    );
                    let init = remux::video_init_segment(&config);
                    _video_config = Some(config.clone());
                    video_init = Some(init);

                    if !catalog_written {
                        let json = remux::generate_catalog(Some(&config), None);
                        if let Ok(mut group) = catalog_track.append_group() {
                            let _ = group.write_frame(Bytes::from(json));
                            let _ = group.finish();
                            catalog_written = true;
                        }
                    }
                }
                None => {
                    tracing::warn!("invalid AVCC record from browser");
                }
            },
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
    tracing::info!(broadcast = %broadcast, "WS ingest session ended");
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
    let mut sps = Vec::new();
    for _ in 0..num_sps {
        if offset + 2 > data.len() {
            return None;
        }
        let len = u16::from_be_bytes([data[offset], data[offset + 1]]) as usize;
        offset += 2;
        if offset + len > data.len() {
            return None;
        }
        sps = data[offset..offset + len].to_vec();
        offset += len;
    }

    if offset >= data.len() {
        return None;
    }
    let num_pps = data[offset] as usize;
    offset += 1;
    let mut pps = Vec::new();
    for _ in 0..num_pps {
        if offset + 2 > data.len() {
            return None;
        }
        let len = u16::from_be_bytes([data[offset], data[offset + 1]]) as usize;
        offset += 2;
        if offset + len > data.len() {
            return None;
        }
        pps = data[offset..offset + len].to_vec();
        offset += len;
    }

    if sps.is_empty() || pps.is_empty() {
        return None;
    }

    Some(lvqr_ingest::remux::VideoConfig {
        sps,
        pps,
        profile,
        compat,
        level,
        nalu_length_size,
    })
}
