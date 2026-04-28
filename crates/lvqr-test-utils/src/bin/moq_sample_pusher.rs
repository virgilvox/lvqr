//! `lvqr-moq-sample-pusher` -- session 159 PATH-X bin.
//!
//! Closes Phase A v1.1 #5 (MoQ egress latency SLO) for pure-MoQ
//! subscribers. Subscribes to `<broadcast>/0.mp4` + the sibling
//! `<broadcast>/0.timing` track (session 159 wire shape), joins each
//! arriving video frame against the most-recent timing anchor by
//! `group_id`, computes `latency_ms = now_unix_ms() -
//! anchor.ingest_time_ms`, and POSTs JSON samples to
//! `POST /api/v1/slo/client-sample`.
//!
//! Used by:
//!
//! 1. `crates/lvqr-test-utils/tests/moq_timing_e2e.rs` -- Rust
//!    integration test that drives the full RTMP -> relay -> bin
//!    -> SLO endpoint loop and asserts a non-empty entry on
//!    `GET /api/v1/slo` under `transport="moq"`.
//!
//! 2. Operators running their own MoQ-only deployment who want
//!    pure-MoQ glass-to-glass histogram coverage matching the HLS
//!    side from the session 156 follow-up.
//!
//! ## Anti-scope
//!
//! Best-effort: failed POSTs are logged and dropped so a flaky
//! admin endpoint cannot stall the subscribe path. The bin does not
//! retry pushes; the SLO histogram absorbs gaps gracefully. There is
//! no built-in WHEP / WS / HLS fallback -- this is the pure-MoQ
//! transport label for a reason.
//!
//! On exit, prints a single JSON line on stdout the spawning harness
//! can capture:
//!
//! ```text
//! {"samples_pushed":42,"frames_observed":120,"anchors_observed":4,"duration_secs":5.012}
//! ```
//!
//! stderr carries `tracing` info-level logs.

use std::process::ExitCode;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};
use clap::Parser;
use lvqr_core::now_unix_ms;
use lvqr_fragment::{TIMING_ANCHOR_SIZE, TIMING_TRACK_NAME, TimingAnchor};
use lvqr_moq::Track;
use lvqr_test_utils::http::http_post_json;
use lvqr_test_utils::timing_anchor::TimingAnchorJoin;
use tokio::sync::Mutex;
use tokio::time::timeout;
use tracing::{debug, info, warn};

/// CLI surface for `lvqr-moq-sample-pusher`. See module-level docs
/// for the wire shape on stdout / stderr.
#[derive(Debug, Parser)]
#[command(
    version,
    about = "Session 159 PATH-X: pure-MoQ glass-to-glass SLO sample pusher (closes Phase A v1.1 #5)."
)]
struct Cli {
    /// MoQ relay URL (WebTransport over QUIC). Example:
    /// `https://localhost:4443`. The bin parses the URL, opens a
    /// moq-native client, and waits for the configured broadcast
    /// to announce.
    #[arg(long)]
    relay_url: String,

    /// Broadcast name to subscribe to (e.g. `live/demo`).
    #[arg(long)]
    broadcast: String,

    /// Target endpoint for SLO sample POSTs. Example:
    /// `http://localhost:8080/api/v1/slo/client-sample`. Must be
    /// `http` (TLS-terminated upstream); the bin uses raw TCP
    /// HTTP/1.1 to avoid the reqwest/rustls graph cost on the test
    /// dep tree.
    #[arg(long)]
    slo_endpoint: String,

    /// Optional bearer token for the SLO endpoint. Rides as
    /// `Authorization: Bearer <token>`. Empty string skips the
    /// header (the dual-auth route allows admin OR subscribe; the
    /// admin-token-less anonymous path requires the relay's auth
    /// provider to be Noop, which TestServer's default is).
    #[arg(long, default_value = "")]
    token: String,

    /// Minimum seconds between pushes. The bin drops samples in
    /// the gap to avoid flooding the endpoint when frame rate is
    /// high. Default 5 s matches the `@lvqr/dvr-player` HLS-side
    /// sampler from session 156 follow-up.
    #[arg(long, default_value_t = 5.0)]
    push_interval_secs: f64,

    /// Optional cap on samples pushed before exit. `None` means
    /// unbounded; the integration test sets a small cap to bound
    /// the loop.
    #[arg(long)]
    max_samples: Option<u32>,

    /// Run for at most this many seconds, then exit cleanly.
    /// `None` means unbounded; the integration test sets this to
    /// bound the test runtime.
    #[arg(long)]
    duration_secs: Option<f64>,

    /// Transport label sent in the SLO push body. Default `"moq"`.
    /// Operators with multiple MoQ flavors in flight may want a
    /// more specific label (`"moq-quic"`, `"moq-ws"`, etc.).
    #[arg(long, default_value = "moq")]
    transport_label: String,

    /// Disable TLS certificate verification on the outbound MoQ
    /// session. The integration test always sets this because
    /// `TestServer` runs with a self-signed cert. Production
    /// operators leave it off (default `false`).
    #[arg(long, default_value_t = false)]
    insecure: bool,
}

#[derive(Debug, Default)]
struct PushSummary {
    samples_pushed: u32,
    frames_observed: u32,
    anchors_observed: u32,
    duration_secs: f64,
}

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() -> ExitCode {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .with_writer(std::io::stderr)
        .try_init();

    let cli = Cli::parse();
    match run(cli).await {
        Ok(summary) => {
            println!(
                "{{\"samples_pushed\":{},\"frames_observed\":{},\"anchors_observed\":{},\"duration_secs\":{:.3}}}",
                summary.samples_pushed, summary.frames_observed, summary.anchors_observed, summary.duration_secs
            );
            ExitCode::SUCCESS
        }
        Err(e) => {
            warn!("lvqr-moq-sample-pusher failed: {e:#}");
            println!(
                "{{\"samples_pushed\":0,\"frames_observed\":0,\"anchors_observed\":0,\"duration_secs\":0.000,\"error\":\"{e}\"}}"
            );
            ExitCode::FAILURE
        }
    }
}

/// Parse the SLO endpoint URL into a SocketAddr + path so the raw-TCP
/// `http_post_json` helper can use it. Only `http` is supported; any
/// production deployment that needs HTTPS termination should run a
/// reverse proxy in front of the admin port.
fn parse_slo_endpoint(raw: &str) -> Result<(std::net::SocketAddr, String)> {
    let url: url::Url = raw
        .parse()
        .with_context(|| format!("slo_endpoint `{raw}` is not a URL"))?;
    if url.scheme() != "http" {
        anyhow::bail!("slo_endpoint must use scheme `http`; got `{}`", url.scheme());
    }
    let host = url.host_str().ok_or_else(|| anyhow!("slo_endpoint URL missing host"))?;
    let port = url.port().unwrap_or(80);
    // Resolve `host` to a SocketAddr. Synchronous resolution is fine
    // here; the bin runs once at startup and the integration test
    // points at `127.0.0.1`.
    let addrs: Vec<std::net::SocketAddr> = std::net::ToSocketAddrs::to_socket_addrs(&(host, port))
        .with_context(|| format!("resolving slo_endpoint host `{host}:{port}`"))?
        .collect();
    let addr = *addrs
        .first()
        .ok_or_else(|| anyhow!("slo_endpoint host `{host}` resolved to zero addresses"))?;
    let mut path = url.path().to_string();
    if let Some(query) = url.query() {
        path.push('?');
        path.push_str(query);
    }
    Ok((addr, path))
}

async fn run(cli: Cli) -> Result<PushSummary> {
    let push_interval = Duration::from_secs_f64(cli.push_interval_secs.max(0.0));
    let total_budget = cli.duration_secs.map(Duration::from_secs_f64);
    let (slo_addr, slo_path) = parse_slo_endpoint(&cli.slo_endpoint)?;

    info!(
        relay_url = %cli.relay_url,
        broadcast = %cli.broadcast,
        slo_endpoint = %cli.slo_endpoint,
        push_interval_secs = cli.push_interval_secs,
        max_samples = ?cli.max_samples,
        duration_secs = ?cli.duration_secs,
        transport_label = %cli.transport_label,
        insecure = cli.insecure,
        "lvqr-moq-sample-pusher starting"
    );

    // Build the outbound MoQ client. `insecure` flips
    // `tls.disable_verify` which is what TestServer + the relay's
    // self-signed cert require.
    let mut client_config = moq_native::ClientConfig::default();
    if cli.insecure {
        client_config.tls.disable_verify = Some(true);
    }
    let client = client_config.init().context("init moq client")?;

    // Subscribe-side origin we drain announcements from. Pattern
    // mirrors `crates/lvqr-cluster/src/federation.rs:539-542`.
    let sub_origin = moq_lite::Origin::produce();
    let mut announcements = sub_origin.consume();
    let client = client.with_consume(sub_origin);

    let url: url::Url = cli
        .relay_url
        .parse()
        .with_context(|| format!("relay_url `{}` is not a URL", cli.relay_url))?;

    let connect_timeout = Duration::from_secs(10);
    let session = timeout(connect_timeout, client.connect(url))
        .await
        .with_context(|| format!("moq connect to {} timed out after {:?}", cli.relay_url, connect_timeout))?
        .with_context(|| format!("moq connect to {}", cli.relay_url))?;

    info!("moq session connected; waiting for broadcast announcement");

    // Wait for a matching announcement. The bin filters by exact
    // broadcast name; unmatched announcements are skipped.
    let bc = wait_for_broadcast(&mut announcements, &cli.broadcast, Duration::from_secs(15)).await?;
    info!(broadcast = %cli.broadcast, "broadcast announced; subscribing to 0.mp4 + 0.timing");

    // Subscribe to both tracks. The video subscribe is required;
    // the timing subscribe is also required because without it the
    // bin has nothing to do.
    let video_track = bc
        .subscribe_track(&Track::new("0.mp4"))
        .with_context(|| format!("subscribe `{}/0.mp4`", cli.broadcast))?;
    let timing_track = bc
        .subscribe_track(&Track::new(TIMING_TRACK_NAME))
        .with_context(|| format!("subscribe `{}/{}`", cli.broadcast, TIMING_TRACK_NAME))?;

    // Shared anchor join. Tokio's std Mutex would be fine here too,
    // but tokio::Mutex composes better with the await-heavy paths.
    let join = Arc::new(Mutex::new(TimingAnchorJoin::new()));

    // Spawn the timing-track drain. It runs until the track closes
    // or the duration / sample budget triggers shutdown via the
    // shared cancel token.
    let cancel = tokio_util::sync::CancellationToken::new();
    let join_for_timing = join.clone();
    let cancel_for_timing = cancel.clone();
    let timing_handle = tokio::spawn(async move {
        let _ = drain_timing_track(timing_track, join_for_timing, cancel_for_timing).await;
    });

    // Drive the video drain on the main task. It owns the push loop.
    let start = Instant::now();
    let mut summary = PushSummary::default();
    let mut last_push = Instant::now().checked_sub(push_interval).unwrap_or_else(Instant::now);

    let mut video_track = video_track;
    let video_drain_result = drain_video_and_push(
        &mut video_track,
        &join,
        &cli,
        slo_addr,
        &slo_path,
        &mut summary,
        &mut last_push,
        push_interval,
        total_budget,
        start,
        &cancel,
    )
    .await;

    cancel.cancel();
    let _ = tokio::time::timeout(Duration::from_secs(2), timing_handle).await;
    drop(session);

    summary.duration_secs = start.elapsed().as_secs_f64();

    if let Err(e) = video_drain_result {
        warn!("video drain exited with error: {e:#}");
    }

    Ok(summary)
}

async fn wait_for_broadcast(
    announcements: &mut moq_lite::OriginConsumer,
    target_name: &str,
    deadline: Duration,
) -> Result<moq_lite::BroadcastConsumer> {
    let start = Instant::now();
    loop {
        let elapsed = start.elapsed();
        if elapsed >= deadline {
            anyhow::bail!("timed out after {:?} waiting for broadcast `{}`", deadline, target_name);
        }
        let remaining = deadline - elapsed;
        let announced = timeout(remaining, announcements.announced())
            .await
            .with_context(|| format!("timed out waiting for broadcast `{target_name}`"))?
            .ok_or_else(|| anyhow!("origin consumer closed before broadcast `{target_name}` announced"))?;
        let (path, maybe_bc) = announced;
        if path.as_str() != target_name {
            debug!(announced = %path.as_str(), "ignoring unmatched announcement");
            continue;
        }
        match maybe_bc {
            Some(bc) => return Ok(bc),
            None => anyhow::bail!("broadcast `{target_name}` was announced as unannounce"),
        }
    }
}

async fn drain_timing_track(
    mut track: lvqr_moq::TrackConsumer,
    join: Arc<Mutex<TimingAnchorJoin>>,
    cancel: tokio_util::sync::CancellationToken,
) -> Result<()> {
    loop {
        let next = tokio::select! {
            g = track.next_group() => g,
            _ = cancel.cancelled() => return Ok(()),
        };
        let mut group = match next.context("timing next_group")? {
            Some(g) => g,
            None => return Ok(()),
        };
        // Each timing group is single-framed by the producer-side
        // sink contract. Read one frame; ignore any subsequent
        // (defensive against future wire shape evolution).
        let frame = tokio::select! {
            f = group.read_frame() => f,
            _ = cancel.cancelled() => return Ok(()),
        };
        let Some(payload) = frame.context("timing read_frame")? else {
            continue;
        };
        if payload.len() != TIMING_ANCHOR_SIZE {
            warn!(
                len = payload.len(),
                expected = TIMING_ANCHOR_SIZE,
                "timing anchor wrong size; skipping"
            );
            continue;
        }
        let Some(anchor) = TimingAnchor::decode(&payload) else {
            warn!("timing anchor decode failed; skipping");
            continue;
        };
        debug!(
            group_id = anchor.group_id,
            ingest_time_ms = anchor.ingest_time_ms,
            "timing anchor"
        );
        let mut g = join.lock().await;
        g.push(anchor);
    }
}

#[allow(clippy::too_many_arguments)]
async fn drain_video_and_push(
    track: &mut lvqr_moq::TrackConsumer,
    join: &Arc<Mutex<TimingAnchorJoin>>,
    cli: &Cli,
    slo_addr: std::net::SocketAddr,
    slo_path: &str,
    summary: &mut PushSummary,
    last_push: &mut Instant,
    push_interval: Duration,
    total_budget: Option<Duration>,
    start: Instant,
    cancel: &tokio_util::sync::CancellationToken,
) -> Result<()> {
    loop {
        if let Some(budget) = total_budget {
            if start.elapsed() >= budget {
                info!("duration budget exhausted; exiting");
                return Ok(());
            }
        }
        if let Some(cap) = cli.max_samples {
            if summary.samples_pushed >= cap {
                info!("max-samples cap reached; exiting");
                return Ok(());
            }
        }
        let next = tokio::select! {
            g = track.next_group() => g,
            _ = cancel.cancelled() => return Ok(()),
        };
        let mut group = match next.context("video next_group")? {
            Some(g) => g,
            None => return Ok(()),
        };
        let group_seq = group.info.sequence;
        loop {
            let frame = tokio::select! {
                f = group.read_frame() => f,
                _ = cancel.cancelled() => return Ok(()),
            };
            let Some(_payload) = frame.context("video read_frame")? else {
                break;
            };
            summary.frames_observed = summary.frames_observed.saturating_add(1);

            // Throttle pushes to push_interval. Skip the lookup
            // entirely when we are inside the throttle window so a
            // 30 fps video stream does not drown the join lock.
            if last_push.elapsed() < push_interval {
                continue;
            }

            // Look up the timing anchor for this group. Exact match
            // first, fallback to largest-less-than per the join
            // helper's contract.
            let anchor = {
                let g = join.lock().await;
                if g.is_empty() {
                    summary.anchors_observed = 0;
                } else {
                    summary.anchors_observed = g.len() as u32;
                }
                g.lookup(group_seq)
            };
            let Some(anchor) = anchor else {
                debug!(group_seq, "no anchor for group; skipping push");
                continue;
            };
            let now = now_unix_ms();
            if anchor.ingest_time_ms == 0 || now < anchor.ingest_time_ms {
                debug!(
                    group_seq,
                    ingest = anchor.ingest_time_ms,
                    now,
                    "skipping push: zero anchor or clock skew"
                );
                continue;
            }
            let render_ts_ms = now;
            let body = serde_json::json!({
                "broadcast": cli.broadcast,
                "transport": cli.transport_label,
                "ingest_ts_ms": anchor.ingest_time_ms,
                "render_ts_ms": render_ts_ms,
            })
            .to_string();
            let bearer = if cli.token.is_empty() {
                None
            } else {
                Some(cli.token.as_str())
            };
            let response = http_post_json(slo_addr, slo_path, bearer, body.as_bytes()).await;
            if (200..300).contains(&response.status) {
                summary.samples_pushed = summary.samples_pushed.saturating_add(1);
                *last_push = Instant::now();
                info!(
                    status = response.status,
                    latency_ms = render_ts_ms - anchor.ingest_time_ms,
                    "SLO sample pushed"
                );
            } else {
                warn!(status = response.status, "SLO endpoint rejected push; continuing");
            }
        }
    }
}
