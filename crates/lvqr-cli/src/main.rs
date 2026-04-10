use anyhow::Result;
use clap::Parser;

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
        Cli::Serve(args) => {
            tracing::info!(
                port = args.port,
                rtmp_port = args.rtmp_port,
                admin_port = args.admin_port,
                mesh = args.mesh_enabled,
                "starting LVQR relay"
            );

            let registry = std::sync::Arc::new(lvqr_core::Registry::new());

            // Start admin HTTP server
            let admin_registry = registry.clone();
            let admin_addr: std::net::SocketAddr = ([0, 0, 0, 0], args.admin_port).into();
            let admin_router = lvqr_admin::build_router(admin_registry);

            tracing::info!(%admin_addr, "admin API listening");
            let listener = tokio::net::TcpListener::bind(admin_addr).await?;
            axum::serve(listener, admin_router).await?;

            Ok(())
        }
    }
}
