use anyhow::Result;
use clap::Parser;
use std::sync::Arc;

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

async fn serve(args: ServeArgs) -> Result<()> {
    tracing::info!(
        quic_port = args.port,
        rtmp_port = args.rtmp_port,
        admin_port = args.admin_port,
        mesh = args.mesh_enabled,
        "starting LVQR relay"
    );

    let registry = Arc::new(lvqr_core::Registry::new());

    // MoQ relay
    let relay_config = lvqr_relay::RelayConfig::new(([0, 0, 0, 0], args.port).into());
    let relay = lvqr_relay::RelayServer::new(relay_config);
    let (mut moq_server, relay_addr) = relay.init_server()?;
    tracing::info!(addr = %relay_addr, "MoQ relay listening");

    // RTMP ingest bridged to MoQ
    let bridge = lvqr_ingest::RtmpMoqBridge::new(relay.origin().clone());
    let rtmp_config = lvqr_ingest::RtmpConfig {
        bind_addr: ([0, 0, 0, 0], args.rtmp_port).into(),
    };
    let rtmp_server = bridge.create_rtmp_server(rtmp_config);

    // Admin HTTP + optional signal WebSocket
    let admin_addr: std::net::SocketAddr = ([0, 0, 0, 0], args.admin_port).into();
    let admin_router = lvqr_admin::build_router(registry.clone());

    let combined_router = if args.mesh_enabled {
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

        admin_router.merge(signal_router)
    } else {
        admin_router
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
