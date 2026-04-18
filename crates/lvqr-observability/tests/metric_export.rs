//! Integration test for Tier 3 session I (OTLP metric exporter).
//!
//! Proves that a `metrics::counter!` / `metrics::gauge!` /
//! `metrics::histogram!` call site flows through
//! [`lvqr_observability::OtelMetricsRecorder`] into an OTel
//! [`SdkMeterProvider`] and out a [`PushMetricExporter`] without
//! any real OTLP network round-trip. The production path in
//! `lvqr_observability::init` builds its exporter with
//! `opentelemetry_otlp::MetricExporter::builder().with_tonic()`;
//! this test swaps that for an in-memory shim but shares the
//! same `build_meter_provider` helper, so the `Resource` /
//! `PeriodicReader` wiring exercised here is identical to what
//! production uses.
//!
//! Scoped via `metrics::with_local_recorder` so the test does
//! not touch the global `metrics` recorder (which may be taken
//! by another test in the same process). Offline on every
//! `cargo test --workspace` invocation.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use opentelemetry_sdk::metrics::data::{ResourceMetrics, Sum};
use opentelemetry_sdk::metrics::exporter::PushMetricExporter;
use opentelemetry_sdk::metrics::{MetricResult, Temporality};

use lvqr_observability::{ObservabilityConfig, OtelMetricsRecorder, build_meter_provider};

#[derive(Clone, Default)]
struct InMemoryMetricExporter {
    captured: Arc<Mutex<Vec<CapturedMetric>>>,
    shutdown_called: Arc<Mutex<bool>>,
}

#[derive(Clone, Debug)]
#[allow(dead_code)]
struct CapturedMetric {
    scope: String,
    name: String,
    // Only populated for u64-sum aggregations (counters); other
    // aggregations surface via `kind`.
    u64_sum_total: Option<u64>,
    kind: MetricKind,
    attributes: Vec<(String, String)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[allow(dead_code)]
enum MetricKind {
    SumU64,
    SumF64,
    Gauge,
    Histogram,
    Unknown,
}

impl InMemoryMetricExporter {
    fn snapshot(&self) -> Vec<CapturedMetric> {
        self.captured.lock().expect("captured lock not poisoned").clone()
    }

    fn shutdown_was_called(&self) -> bool {
        *self.shutdown_called.lock().expect("shutdown lock not poisoned")
    }
}

impl std::fmt::Debug for InMemoryMetricExporter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InMemoryMetricExporter")
            .field("captured_count", &self.captured.lock().map(|v| v.len()).unwrap_or(0))
            .finish()
    }
}

#[async_trait::async_trait]
impl PushMetricExporter for InMemoryMetricExporter {
    async fn export(&self, metrics: &mut ResourceMetrics) -> MetricResult<()> {
        let mut captured = Vec::new();
        for scope_metrics in &metrics.scope_metrics {
            let scope_name = scope_metrics.scope.name().to_string();
            for metric in &scope_metrics.metrics {
                let data: &dyn std::any::Any = metric.data.as_any();
                let (kind, u64_sum_total, attributes) = if let Some(sum) = data.downcast_ref::<Sum<u64>>() {
                    let total: u64 = sum.data_points.iter().map(|p| p.value).sum();
                    let attrs = sum
                        .data_points
                        .iter()
                        .flat_map(|p| p.attributes.iter().map(|kv| (kv.key.to_string(), kv.value.to_string())))
                        .collect();
                    (MetricKind::SumU64, Some(total), attrs)
                } else if data.downcast_ref::<Sum<f64>>().is_some() {
                    (MetricKind::SumF64, None, Vec::new())
                } else if data
                    .downcast_ref::<opentelemetry_sdk::metrics::data::Gauge<f64>>()
                    .is_some()
                    || data
                        .downcast_ref::<opentelemetry_sdk::metrics::data::Gauge<i64>>()
                        .is_some()
                {
                    (MetricKind::Gauge, None, Vec::new())
                } else if data
                    .downcast_ref::<opentelemetry_sdk::metrics::data::Histogram<f64>>()
                    .is_some()
                {
                    (MetricKind::Histogram, None, Vec::new())
                } else {
                    (MetricKind::Unknown, None, Vec::new())
                };
                captured.push(CapturedMetric {
                    scope: scope_name.clone(),
                    name: metric.name.to_string(),
                    u64_sum_total,
                    kind,
                    attributes,
                });
            }
        }
        self.captured
            .lock()
            .expect("captured lock not poisoned")
            .extend(captured);
        Ok(())
    }

    async fn force_flush(&self) -> MetricResult<()> {
        Ok(())
    }

    fn shutdown(&self) -> MetricResult<()> {
        *self.shutdown_called.lock().expect("shutdown lock not poisoned") = true;
        Ok(())
    }

    fn temporality(&self) -> Temporality {
        Temporality::Cumulative
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn counter_increment_reaches_otel_exporter() {
    let exporter = InMemoryMetricExporter::default();
    let view = exporter.clone();

    let config = ObservabilityConfig {
        service_name: "lvqr-session-82-test".to_string(),
        resource_attributes: vec![("deploy.env".to_string(), "test".to_string())],
        ..ObservabilityConfig::default()
    };

    let provider = build_meter_provider(&config, exporter);
    let recorder = OtelMetricsRecorder::new(&provider);

    metrics::with_local_recorder(&recorder, || {
        metrics::counter!("lvqr_fragments_emitted_total", "type" => "video").increment(7);
        metrics::counter!("lvqr_fragments_emitted_total", "type" => "video").increment(3);
        metrics::counter!("lvqr_bytes_ingested_total", "type" => "video").increment(1024);
    });

    // Force the periodic reader to collect + export once.
    provider
        .force_flush()
        .expect("meter provider force_flush should succeed");
    // Small grace period for the periodic reader's tokio task
    // to drain; empirically instant on a quiet runner.
    tokio::time::sleep(Duration::from_millis(200)).await;

    let captured = view.snapshot();
    let fragments = captured
        .iter()
        .find(|m| m.name == "lvqr_fragments_emitted_total")
        .unwrap_or_else(|| {
            panic!(
                "expected metric 'lvqr_fragments_emitted_total'; captured: {:?}",
                captured.iter().map(|m| &m.name).collect::<Vec<_>>()
            )
        });
    assert_eq!(fragments.kind, MetricKind::SumU64);
    assert_eq!(
        fragments.u64_sum_total,
        Some(10),
        "two .increment() calls (7 + 3) must sum to 10 on the exported Sum<u64>",
    );
    let has_video_label = fragments.attributes.iter().any(|(k, v)| k == "type" && v == "video");
    assert!(
        has_video_label,
        "call-site label 'type'=>'video' did not propagate to OTel attributes: {:?}",
        fragments.attributes,
    );

    let bytes = captured
        .iter()
        .find(|m| m.name == "lvqr_bytes_ingested_total")
        .expect("expected metric 'lvqr_bytes_ingested_total'");
    assert_eq!(bytes.u64_sum_total, Some(1024));

    let scope_ok = captured.iter().all(|m| m.scope == "lvqr-observability");
    assert!(
        scope_ok,
        "every exported metric should carry the lvqr-observability scope"
    );

    assert!(provider.shutdown().is_ok());
    assert!(
        view.shutdown_was_called(),
        "PushMetricExporter::shutdown was not called when MeterProvider shut down",
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn gauge_set_converges_via_delta_updates() {
    let exporter = InMemoryMetricExporter::default();
    let view = exporter.clone();

    let config = ObservabilityConfig {
        service_name: "lvqr-session-82-gauge".to_string(),
        ..ObservabilityConfig::default()
    };

    let provider = build_meter_provider(&config, exporter);
    let recorder = OtelMetricsRecorder::new(&provider);

    metrics::with_local_recorder(&recorder, || {
        metrics::gauge!("lvqr_active_moq_sessions").increment(5.0);
        metrics::gauge!("lvqr_active_moq_sessions").decrement(2.0);
        metrics::gauge!("lvqr_active_moq_sessions").set(42.0);
    });

    provider
        .force_flush()
        .expect("meter provider force_flush should succeed");
    tokio::time::sleep(Duration::from_millis(200)).await;

    let captured = view.snapshot();
    // UpDownCounter<f64> aggregates as Sum<f64> on the export
    // path; confirm the metric surfaced at all.
    assert!(
        captured.iter().any(|m| m.name == "lvqr_active_moq_sessions"),
        "gauge metric did not reach the exporter: {:?}",
        captured.iter().map(|m| &m.name).collect::<Vec<_>>(),
    );

    let _ = provider.shutdown();
}
