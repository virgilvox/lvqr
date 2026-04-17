//! `lvqr-soak` CLI entry point.
//!
//! Thin wrapper around [`lvqr_soak::run_soak`]. Parses a handful of
//! flags into a [`SoakConfig`], runs the harness, prints the report,
//! and exits non-zero on failure. Intended to run both as a
//! developer-laptop smoke (short duration) and as a nightly 24 h
//! CI job (long duration).

use std::process::ExitCode;
use std::time::Duration;

use anyhow::Result;
use clap::Parser;
use lvqr_soak::{Codec, SoakConfig, run_soak};
use tracing_subscriber::EnvFilter;

#[derive(Debug, Clone, Parser)]
#[command(name = "lvqr-soak", about = "Long-duration soak harness for LVQR RTSP PLAY")]
struct Cli {
    /// Soak duration in seconds.
    #[arg(long, default_value_t = 60)]
    duration_secs: u64,

    /// Concurrent RTSP PLAY subscriber count.
    #[arg(long, default_value_t = 10)]
    subscribers: usize,

    /// Fragment emission rate in Hz.
    #[arg(long, default_value_t = 30)]
    fragment_hz: u32,

    /// Resource sampling interval in seconds.
    #[arg(long, default_value_t = 5)]
    metrics_interval_secs: u64,

    /// Pass threshold: minimum RTP packets per subscriber. Omit to
    /// derive from duration * fragment_hz with a 20 % slack.
    #[arg(long)]
    min_rtp_per_subscriber: Option<u64>,

    /// Pass threshold: minimum RTCP SR packets per subscriber. Omit
    /// to derive from duration / 5 s (the default SR cadence).
    #[arg(long)]
    min_rtcp_per_subscriber: Option<u64>,

    /// Broadcast identifier.
    #[arg(long, default_value = "live/soak")]
    broadcast: String,

    /// Codec the synthetic publisher emits.
    #[arg(long, value_enum, default_value_t = Codec::H264)]
    codec: Codec,
}

fn init_tracing() {
    let filter = EnvFilter::try_from_env("LVQR_LOG").unwrap_or_else(|_| EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt().with_env_filter(filter).try_init();
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<ExitCode> {
    init_tracing();
    let cli = Cli::parse();

    let config = SoakConfig {
        duration: Duration::from_secs(cli.duration_secs),
        subscribers: cli.subscribers,
        fragment_hz: cli.fragment_hz,
        rtp_packets_per_subscriber_min: cli.min_rtp_per_subscriber,
        rtcp_packets_per_subscriber_min: cli.min_rtcp_per_subscriber,
        metrics_interval: Duration::from_secs(cli.metrics_interval_secs),
        broadcast: cli.broadcast,
        codec: cli.codec,
        video_width: 1280,
        video_height: 720,
    };

    let report = run_soak(config).await?;
    println!("{}", report.render_summary());

    Ok(if report.passed {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}
