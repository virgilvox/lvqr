//! Integration test for Tier 3 session H (OTLP span exporter).
//!
//! Proves that a `tracing::info_span!` emitted through a
//! `tracing_opentelemetry` layer reaches a `SpanExporter`
//! without any real OTLP network round-trip, by installing an
//! in-memory exporter that buffers exported `SpanData` into a
//! shared `Vec` and then emitting a synthetic span.
//!
//! The production path in `lvqr_observability::init` builds its
//! exporter with `opentelemetry_otlp::SpanExporter::builder()
//! .with_tonic()`; this test swaps that for the in-memory
//! shim but shares the same `build_tracer_provider` helper, so
//! the `Resource` / `Sampler` / `BatchSpanProcessor` wiring
//! exercised here is identical to what production uses.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::future::BoxFuture;
use opentelemetry::trace::TracerProvider as _;
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::export::trace::{ExportResult, SpanData, SpanExporter};
use tracing_subscriber::layer::SubscriberExt;

use lvqr_observability::{ObservabilityConfig, build_tracer_provider};

#[derive(Debug, Default, Clone)]
struct InMemoryExporter {
    captured: Arc<Mutex<Vec<SpanData>>>,
    resource: Arc<Mutex<Option<Resource>>>,
    shutdown_called: Arc<Mutex<bool>>,
}

impl InMemoryExporter {
    fn snapshot(&self) -> Vec<SpanData> {
        self.captured.lock().expect("captured lock not poisoned").clone()
    }

    fn resource_snapshot(&self) -> Option<Resource> {
        self.resource.lock().expect("resource lock not poisoned").clone()
    }

    fn shutdown_was_called(&self) -> bool {
        *self.shutdown_called.lock().expect("shutdown lock not poisoned")
    }
}

impl SpanExporter for InMemoryExporter {
    fn export(&mut self, batch: Vec<SpanData>) -> BoxFuture<'static, ExportResult> {
        let captured = self.captured.clone();
        Box::pin(async move {
            captured.lock().expect("captured lock not poisoned").extend(batch);
            Ok(())
        })
    }

    fn shutdown(&mut self) {
        *self.shutdown_called.lock().expect("shutdown lock not poisoned") = true;
    }

    fn set_resource(&mut self, resource: &Resource) {
        *self.resource.lock().expect("resource lock not poisoned") = Some(resource.clone());
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn in_memory_exporter_captures_synthetic_span() {
    let exporter = InMemoryExporter::default();
    let view = exporter.clone();

    let config = ObservabilityConfig {
        service_name: "lvqr-session-81-test".to_string(),
        resource_attributes: vec![("deploy.env".to_string(), "test".to_string())],
        trace_sample_ratio: 1.0,
        ..ObservabilityConfig::default()
    };

    let provider = build_tracer_provider(&config, exporter);
    let tracer = provider.tracer(config.service_name.clone());
    let otel_layer = tracing_opentelemetry::layer().with_tracer(tracer);
    let subscriber = tracing_subscriber::registry().with(otel_layer);

    tracing::subscriber::with_default(subscriber, || {
        let span = tracing::info_span!("synthetic-rtsp-describe", broadcast = "sample", subsystem = "rtsp",);
        let _enter = span.enter();
        tracing::info!("span body");
    });

    // BatchSpanProcessor buffers; flush synchronously before
    // asserting. force_flush returns per-processor results; any
    // error here would indicate an exporter issue worth failing
    // the test on.
    for result in provider.force_flush() {
        assert!(result.is_ok(), "force_flush reported error: {result:?}");
    }

    // Wait briefly for the batch runtime to drain its tokio task
    // even if force_flush returned. Empirically the flush is
    // near-instant; 200 ms gives a comfortable margin on a
    // loaded CI runner without extending test runtime
    // noticeably.
    tokio::time::sleep(Duration::from_millis(200)).await;

    let spans = view.snapshot();
    assert!(!spans.is_empty(), "expected at least one exported span, got zero");
    let span = spans
        .iter()
        .find(|s| s.name.as_ref() == "synthetic-rtsp-describe")
        .unwrap_or_else(|| {
            panic!(
                "expected a span named 'synthetic-rtsp-describe' in {:?}",
                spans.iter().map(|s| s.name.to_string()).collect::<Vec<_>>()
            )
        });

    // Resource propagation: the SDK calls `set_resource` on the
    // exporter when the BatchSpanProcessor registers; in 0.27
    // the Resource is NOT stored on each SpanData. Assert the
    // resource the exporter was handed matches the config.
    let resource = view
        .resource_snapshot()
        .expect("SpanExporter::set_resource was never called");
    let service_name = resource
        .get(opentelemetry::Key::from_static_str("service.name"))
        .map(|v| v.to_string());
    assert_eq!(
        service_name.as_deref(),
        Some("lvqr-session-81-test"),
        "service.name resource attribute did not propagate onto exporter Resource",
    );
    let deploy_env = resource
        .get(opentelemetry::Key::from_static_str("deploy.env"))
        .map(|v| v.to_string());
    assert_eq!(
        deploy_env.as_deref(),
        Some("test"),
        "custom resource attribute did not propagate onto exporter Resource",
    );

    // Sanity on the span itself: the tracing instrumentation
    // scope should have been populated by tracing-opentelemetry.
    let scope_name = span.instrumentation_scope.name();
    assert!(
        !scope_name.is_empty(),
        "expected non-empty instrumentation scope name on exported span",
    );

    // Explicit shutdown should drive shutdown on the in-memory
    // exporter. This matches the Drop path on
    // `ObservabilityHandle` in production.
    assert!(provider.shutdown().is_ok(), "provider shutdown reported an error");
    assert!(
        view.shutdown_was_called(),
        "SpanExporter::shutdown was not called when TracerProvider shut down",
    );

    // Silence unused: `span` keeps the reference live for the
    // scope_name assertion.
    let _ = span;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn zero_sample_ratio_drops_every_span() {
    let exporter = InMemoryExporter::default();
    let view = exporter.clone();

    let config = ObservabilityConfig {
        service_name: "lvqr-session-81-zero".to_string(),
        trace_sample_ratio: 0.0,
        ..ObservabilityConfig::default()
    };

    let provider = build_tracer_provider(&config, exporter);
    let tracer = provider.tracer("zero-sample");
    let otel_layer = tracing_opentelemetry::layer().with_tracer(tracer);
    let subscriber = tracing_subscriber::registry().with(otel_layer);

    tracing::subscriber::with_default(subscriber, || {
        for i in 0..8 {
            let span = tracing::info_span!("dropped", i);
            let _enter = span.enter();
        }
    });

    for result in provider.force_flush() {
        assert!(result.is_ok(), "force_flush reported error: {result:?}");
    }
    tokio::time::sleep(Duration::from_millis(100)).await;

    let spans = view.snapshot();
    assert!(
        spans.is_empty(),
        "TraceIdRatioBased(0.0) should drop every span, got {} exported",
        spans.len(),
    );

    let _ = provider.shutdown();
}
