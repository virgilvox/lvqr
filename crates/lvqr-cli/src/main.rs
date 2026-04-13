use anyhow::Result;
use clap::Parser;
use lvqr_auth::{JwtAuthConfig, JwtAuthProvider, NoopAuthProvider, SharedAuth, StaticAuthConfig, StaticAuthProvider};
use lvqr_cli::{ServeConfig, start};
use std::path::PathBuf;
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
    tls_cert: Option<PathBuf>,

    /// Path to TLS private key (PEM). Auto-generates self-signed if omitted.
    #[arg(long, env = "LVQR_TLS_KEY")]
    tls_key: Option<PathBuf>,

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
    record_dir: Option<PathBuf>,

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
        Cli::Serve(args) => serve_from_args(args).await,
    }
}

async fn serve_from_args(args: ServeArgs) -> Result<()> {
    // Build auth provider from CLI/env. JWT takes precedence when
    // `--jwt-secret` is set; otherwise fall back to the static-token
    // provider when any individual token is configured; otherwise open
    // access (`NoopAuthProvider`).
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
        Arc::new(provider) as SharedAuth
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

    let config = ServeConfig {
        relay_addr: ([0, 0, 0, 0], args.port).into(),
        rtmp_addr: ([0, 0, 0, 0], args.rtmp_port).into(),
        admin_addr: ([0, 0, 0, 0], args.admin_port).into(),
        mesh_enabled: args.mesh_enabled,
        max_peers: args.max_peers,
        auth: Some(auth),
        record_dir: args.record_dir,
        install_prometheus: true,
        tls_cert: args.tls_cert,
        tls_key: args.tls_key,
    };

    let handle = start(config).await?;

    tokio::select! {
        res = tokio::signal::ctrl_c() => {
            if res.is_ok() {
                tracing::info!("ctrl-c received, initiating graceful shutdown");
            }
        }
    }

    handle.shutdown().await
}
