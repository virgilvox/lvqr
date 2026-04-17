//! Smoke test: the soak harness itself runs without errors on a
//! short run for every supported codec.
//!
//! Verifies run_soak drives the server + publisher + subscribers
//! end-to-end for H.264 video, HEVC video, AAC audio, and Opus
//! audio. Thresholds are loosened from the defaults so the tests
//! stay robust on slow CI machines; the real nightly soak run
//! uses the stock thresholds.

use std::time::Duration;

use lvqr_soak::{Codec, SoakConfig, run_soak};

fn short_config(codec: Codec, broadcast: &str) -> SoakConfig {
    SoakConfig {
        duration: Duration::from_secs(2),
        subscribers: 2,
        fragment_hz: 30,
        // 2 s < 5 s SR interval so no SR is guaranteed in-window.
        rtp_packets_per_subscriber_min: Some(1),
        rtcp_packets_per_subscriber_min: Some(0),
        metrics_interval: Duration::from_millis(500),
        broadcast: broadcast.to_string(),
        codec,
        video_width: 640,
        video_height: 360,
    }
}

async fn assert_traffic_flows(config: SoakConfig) {
    let codec = config.codec;
    let report = run_soak(config).await.expect("run_soak completes");
    assert_eq!(report.subscribers.len(), 2, "two subscribers reported");
    for sub in &report.subscribers {
        assert!(
            sub.error.is_none(),
            "{codec:?} subscriber {} errored: {:?}",
            sub.id,
            sub.error
        );
        assert!(
            sub.rtp_packets > 0,
            "{codec:?} subscriber {} got zero RTP packets in a 2 s run",
            sub.id
        );
        assert!(
            sub.first_rtp_after.is_some(),
            "{codec:?} subscriber {} never saw first RTP",
            sub.id
        );
    }
    assert!(report.fragments_emitted > 0, "{codec:?} publisher emitted >=1 fragment");
    assert!(!report.metrics.is_empty(), "{codec:?} metrics collector sampled");
    assert!(
        report.passed,
        "{codec:?} report passes with loose thresholds: {:?}",
        report.failure_reason
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn soak_h264_runs_for_short_duration_and_reports_traffic() {
    assert_traffic_flows(short_config(Codec::H264, "live/smoke-h264")).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn soak_hevc_runs_for_short_duration_and_reports_traffic() {
    assert_traffic_flows(short_config(Codec::Hevc, "live/smoke-hevc")).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn soak_aac_runs_for_short_duration_and_reports_traffic() {
    assert_traffic_flows(short_config(Codec::Aac, "live/smoke-aac")).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn soak_opus_runs_for_short_duration_and_reports_traffic() {
    assert_traffic_flows(short_config(Codec::Opus, "live/smoke-opus")).await;
}

/// MetricsSample.cpu_ticks is populated on Linux and `None` on every
/// other platform. Verifies the sampling path is wired correctly per
/// target OS without asserting on an absolute CPU value (which is
/// too noisy on a 2 s smoke run).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn soak_cpu_sampling_matches_platform_expectation() {
    let report = run_soak(short_config(Codec::H264, "live/cpu-smoke"))
        .await
        .expect("run_soak completes");
    assert!(!report.metrics.is_empty(), "metrics must have at least one sample");
    for sample in &report.metrics {
        if cfg!(target_os = "linux") {
            assert!(
                sample.cpu_ticks.is_some(),
                "Linux cpu_ticks must be Some, got None at t={:?}",
                sample.elapsed
            );
        } else {
            assert!(
                sample.cpu_ticks.is_none(),
                "non-Linux cpu_ticks must be None, got {:?} at t={:?}",
                sample.cpu_ticks,
                sample.elapsed
            );
        }
    }
}

/// With an absurdly high RTP threshold the report should pass=false
/// even though traffic flowed. Pins the pass/fail logic against a
/// future change that accidentally lets a zero-packet run through.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn soak_report_flags_failure_when_threshold_unmet() {
    let config = SoakConfig {
        duration: Duration::from_secs(1),
        subscribers: 1,
        fragment_hz: 30,
        rtp_packets_per_subscriber_min: Some(1_000_000),
        rtcp_packets_per_subscriber_min: Some(0),
        metrics_interval: Duration::from_millis(250),
        broadcast: "live/threshold".to_string(),
        codec: Codec::H264,
        video_width: 320,
        video_height: 240,
    };
    let report = run_soak(config).await.expect("run_soak setup");
    assert!(!report.passed, "unreachable RTP threshold must fail the run");
    assert!(report.failure_reason.is_some(), "failure reason set when not passed");
}
