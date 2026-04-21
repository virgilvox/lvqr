//! Asset-hygiene test for the Tier 4 item 4.7 session B Grafana /
//! Prometheus pack. Guards against silent drift between the code
//! (metric name, transport label, alert runbook URLs) and the
//! YAML / JSON files shipped under `deploy/grafana/`.
//!
//! Verifies:
//!
//! 1. `deploy/grafana/alerts/lvqr-slo.rules.yaml` exists, names the
//!    five expected alerts, uses the canonical metric name, and
//!    references the `docs/slo.md` runbook.
//! 2. `deploy/grafana/dashboards/lvqr-slo.json` is valid JSON with
//!    the expected dashboard `uid`, title, and panel titles, and
//!    queries the same metric name used by the alert pack.
//! 3. `docs/slo.md` exists and carries the runbook-anchor headers
//!    the alert pack's `runbook_url` annotations point at.
//!
//! Deliberately string-based checks rather than full YAML/JSON
//! deserialization so we do not take on a `serde_yaml` dep just for
//! asset hygiene. Anyone who introduces a rule-pack regression will
//! see a failing test with a clear message; the actual validity of
//! the YAML against the Prometheus rule schema is the CI / operator
//! tooling's job (`promtool check rules`).

use std::path::{Path, PathBuf};

fn workspace_root() -> PathBuf {
    // CARGO_MANIFEST_DIR is `.../crates/lvqr-admin`; step up twice
    // to reach the workspace root.
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .parent()
        .and_then(|p| p.parent())
        .unwrap_or(&manifest_dir)
        .to_path_buf()
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display()))
}

#[test]
fn prometheus_rule_pack_names_every_expected_alert() {
    let path = workspace_root().join("deploy/grafana/alerts/lvqr-slo.rules.yaml");
    let body = read(&path);

    for alert in [
        "LvqrSloLatencyP99VeryHigh",
        "LvqrSloLatencyP99High",
        "LvqrSloLatencyP95High",
        "LvqrSloLatencyP50High",
        "LvqrSloNoRecentSamples",
    ] {
        assert!(
            body.contains(&format!("alert: {alert}")),
            "rule pack missing `alert: {alert}`"
        );
    }

    // Metric names referenced by the expressions MUST match the
    // constants fired from `lvqr-admin::slo::LatencyTracker::record`.
    assert!(
        body.contains("lvqr_subscriber_glass_to_glass_ms_bucket"),
        "rule pack should query the histogram bucket metric"
    );
    assert!(
        body.contains("lvqr_subscriber_glass_to_glass_ms_count"),
        "no-recent-samples alert should key off the histogram count metric"
    );

    // Every alert references the operator runbook so an on-call
    // engineer lands on the docs rather than the repo root.
    let runbook_count = body.matches("docs/slo.md").count();
    assert!(
        runbook_count >= 5,
        "rule pack should reference docs/slo.md from at least five alert runbook_urls (one per alert); saw {runbook_count}",
    );
}

#[test]
fn grafana_dashboard_json_parses_and_queries_the_slo_metric() {
    let path = workspace_root().join("deploy/grafana/dashboards/lvqr-slo.json");
    let body = read(&path);

    let parsed: serde_json::Value = serde_json::from_str(&body).expect("dashboard should be valid JSON");

    assert_eq!(
        parsed["uid"].as_str(),
        Some("lvqr-slo"),
        "dashboard uid must stay stable for runbook links"
    );
    assert_eq!(
        parsed["title"].as_str(),
        Some("LVQR Latency SLO"),
        "dashboard title must match the docs"
    );

    let panels = parsed["panels"].as_array().expect("panels array");
    assert!(
        panels.len() >= 4,
        "dashboard should panel p50 / p95 / p99 / sample-rate (4 panels min); got {}",
        panels.len()
    );

    // At least one panel must query the histogram bucket metric so
    // the dashboard stays in sync with the alert pack.
    let has_bucket_query = panels.iter().any(|p| {
        let targets = p["targets"].as_array().cloned().unwrap_or_default();
        targets.iter().any(|t| {
            t["expr"]
                .as_str()
                .map(|s| s.contains("lvqr_subscriber_glass_to_glass_ms_bucket"))
                .unwrap_or(false)
        })
    });
    assert!(
        has_bucket_query,
        "at least one dashboard panel must query `lvqr_subscriber_glass_to_glass_ms_bucket`",
    );
}

#[test]
fn operator_runbook_has_alert_anchors() {
    let path = workspace_root().join("docs/slo.md");
    let body = read(&path);

    for anchor in [
        "Critical p99 above 4s",
        "Warning p99 above 2s",
        "Warning p95 above 1.5s",
        "Info p50 above 500ms",
        "No recent samples",
        "Threshold tuning by transport",
    ] {
        assert!(body.contains(anchor), "docs/slo.md missing runbook section `{anchor}`");
    }

    // The metric + admin-route shapes documented in the runbook
    // MUST match the code.
    assert!(
        body.contains("lvqr_subscriber_glass_to_glass_ms"),
        "runbook should reference the histogram metric name"
    );
    assert!(body.contains("/api/v1/slo"), "runbook should reference the admin route");
}
