//! Smoke test: the soak harness itself runs without errors on a
//! short run.
//!
//! Verifies run_soak drives the server + publisher + subscribers
//! end-to-end and returns a coherent report. The thresholds are
//! loosened from the defaults so the test is robust to slow CI
//! machines; the real nightly soak run uses the stock thresholds.

use std::time::Duration;

use lvqr_soak::{SoakConfig, run_soak};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn soak_runs_for_short_duration_and_reports_traffic() {
    let config = SoakConfig {
        duration: Duration::from_secs(2),
        subscribers: 2,
        fragment_hz: 30,
        // Loosen the default thresholds so loaded CI machines don't
        // flake: accept any positive RTP count and zero RTCP
        // (2 s < 5 s SR interval means the first SR may not arrive
        // in the window).
        rtp_packets_per_subscriber_min: Some(1),
        rtcp_packets_per_subscriber_min: Some(0),
        metrics_interval: Duration::from_millis(500),
        broadcast: "live/smoke".to_string(),
        video_width: 640,
        video_height: 360,
    };

    let report = run_soak(config)
        .await
        .expect("run_soak completes without a setup error");

    assert_eq!(report.subscribers.len(), 2, "two subscribers reported");
    for sub in &report.subscribers {
        assert!(sub.error.is_none(), "subscriber {} errored: {:?}", sub.id, sub.error);
        assert!(
            sub.rtp_packets > 0,
            "subscriber {} got zero RTP packets in a 2 s run",
            sub.id
        );
        assert!(
            sub.first_rtp_after.is_some(),
            "subscriber {} never saw first RTP",
            sub.id
        );
    }
    assert!(report.fragments_emitted > 0, "publisher emitted at least one fragment");
    assert!(!report.metrics.is_empty(), "metrics collector sampled at least once");
    assert!(
        report.passed,
        "report passes with loose thresholds: {:?}",
        report.failure_reason
    );
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
        video_width: 320,
        video_height: 240,
    };
    let report = run_soak(config).await.expect("run_soak setup");
    assert!(!report.passed, "unreachable RTP threshold must fail the run");
    assert!(report.failure_reason.is_some(), "failure reason set when not passed");
}
