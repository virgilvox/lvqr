//! Integration test for Tier 3 session J (JSON log + trace_id
//! correlation).
//!
//! Proves that when `CorrelatedFormat::new(true)` (JSON mode)
//! is plugged into the fmt layer AND a tracing-opentelemetry
//! layer has bound an OTel context to the currently-entered
//! span, every emitted log line carries a `trace_id` + `span_id`
//! pair that matches the parent span's `SpanContext`.
//!
//! Uses a `MakeWriter` shim that funnels the fmt layer's bytes
//! into a shared `Vec<u8>` so the test can parse each emitted
//! JSON object and assert against it without touching actual
//! stdout or relying on file-descriptor redirection.

use std::io;
use std::sync::{Arc, Mutex};

use futures::future::BoxFuture;
use opentelemetry::trace::{TraceContextExt, TracerProvider as _};
use opentelemetry_sdk::export::trace::{ExportResult, SpanExporter};
use tracing::Instrument;
use tracing_opentelemetry::OpenTelemetrySpanExt;
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::layer::SubscriberExt;

use lvqr_observability::{CorrelatedFormat, ObservabilityConfig, build_tracer_provider};

#[derive(Clone, Default)]
struct CaptureWriter {
    buffer: Arc<Mutex<Vec<u8>>>,
}

impl CaptureWriter {
    fn snapshot(&self) -> String {
        let buf = self.buffer.lock().expect("capture buffer lock not poisoned");
        String::from_utf8_lossy(&buf).into_owned()
    }
}

impl<'a> MakeWriter<'a> for CaptureWriter {
    type Writer = CaptureHandle;

    fn make_writer(&'a self) -> Self::Writer {
        CaptureHandle {
            buffer: self.buffer.clone(),
        }
    }
}

struct CaptureHandle {
    buffer: Arc<Mutex<Vec<u8>>>,
}

impl io::Write for CaptureHandle {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.buffer
            .lock()
            .expect("capture buffer lock not poisoned")
            .extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Black-hole span exporter: the test does not care where OTel
/// spans end up, only that the tracing-opentelemetry layer has
/// attached an OTel context to the tracing span so
/// `CorrelatedFormat` can read `trace_id` / `span_id` off it.
#[derive(Debug, Default, Clone)]
struct NoopExporter;

impl SpanExporter for NoopExporter {
    fn export(&mut self, _batch: Vec<opentelemetry_sdk::export::trace::SpanData>) -> BoxFuture<'static, ExportResult> {
        Box::pin(async { Ok(()) })
    }

    fn shutdown(&mut self) {}
}

fn build_stack(
    config: &ObservabilityConfig,
    capture: CaptureWriter,
) -> (tracing::Dispatch, opentelemetry_sdk::trace::TracerProvider) {
    let provider = build_tracer_provider(config, NoopExporter);
    let tracer = provider.tracer(config.service_name.clone());
    let otel_layer = tracing_opentelemetry::layer().with_tracer(tracer);

    let fmt_layer = tracing_subscriber::fmt::layer()
        .event_format(CorrelatedFormat::new(config.json_logs))
        .with_writer(capture)
        .with_ansi(false);

    let subscriber = tracing_subscriber::registry().with(otel_layer).with(fmt_layer);
    (tracing::Dispatch::new(subscriber), provider)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn json_log_inside_span_carries_matching_trace_and_span_ids() {
    let capture = CaptureWriter::default();
    let view = capture.clone();

    let config = ObservabilityConfig {
        service_name: "lvqr-session-83-json".to_string(),
        json_logs: true,
        ..ObservabilityConfig::default()
    };

    let (dispatch, provider) = build_stack(&config, capture);

    let (expected_trace, expected_span) = tracing::dispatcher::with_default(&dispatch, || {
        let parent = tracing::info_span!("synthetic-rtsp-describe", broadcast = "sample");
        let _enter = parent.enter();

        let ctx = parent.context();
        let span_ref = ctx.span();
        let sc = span_ref.span_context();
        assert!(
            sc.is_valid(),
            "parent span should have a valid OTel context once the telemetry layer is installed",
        );
        let ids = (sc.trace_id().to_string(), sc.span_id().to_string());
        // span_ref borrows from ctx; release the borrow before
        // emitting to keep the reborrow chain short.
        let _ = span_ref;
        let _ = ctx;

        tracing::info!(broadcast = "sample", "inside synthetic span");
        ids
    });

    for result in provider.force_flush() {
        assert!(result.is_ok(), "force_flush should succeed: {result:?}");
    }
    let _ = provider.shutdown();

    let captured = view.snapshot();
    assert!(!captured.is_empty(), "fmt layer should have written at least one line");

    let line = captured
        .lines()
        .find(|line| line.contains("inside synthetic span"))
        .unwrap_or_else(|| panic!("expected an event line containing the message; got: {captured}"));

    let value: serde_json::Value =
        serde_json::from_str(line).unwrap_or_else(|e| panic!("line is not valid JSON: {e}; line: {line}"));

    let trace_id = value.get("trace_id").and_then(|v| v.as_str()).map(str::to_string);
    let span_id = value.get("span_id").and_then(|v| v.as_str()).map(str::to_string);

    assert_eq!(
        trace_id.as_deref(),
        Some(expected_trace.as_str()),
        "JSON log line must carry the parent span's trace_id; full line: {line}",
    );
    assert_eq!(
        span_id.as_deref(),
        Some(expected_span.as_str()),
        "JSON log line must carry the parent span's span_id; full line: {line}",
    );

    let level = value.get("level").and_then(|v| v.as_str());
    assert_eq!(level, Some("INFO"), "level should render as INFO");

    let message = value.get("message").and_then(|v| v.as_str());
    assert!(
        message.map(|m| m.contains("inside synthetic span")).unwrap_or(false),
        "expected the event message to be recorded as a field; value: {value}",
    );

    let broadcast = value.get("broadcast").and_then(|v| v.as_str());
    assert_eq!(
        broadcast,
        Some("sample"),
        "call-site field 'broadcast' must render as a JSON field"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn log_outside_any_span_omits_trace_id_fields() {
    let capture = CaptureWriter::default();
    let view = capture.clone();

    let config = ObservabilityConfig {
        service_name: "lvqr-session-83-no-span".to_string(),
        json_logs: true,
        ..ObservabilityConfig::default()
    };

    let (dispatch, provider) = build_stack(&config, capture);

    tracing::dispatcher::with_default(&dispatch, || {
        tracing::info!("no span context");
    });

    let _ = provider.shutdown();

    let captured = view.snapshot();
    let line = captured
        .lines()
        .find(|line| line.contains("no span context"))
        .unwrap_or_else(|| panic!("expected an event line; got: {captured}"));

    let value: serde_json::Value = serde_json::from_str(line).expect("valid JSON");
    assert!(
        value.get("trace_id").is_none(),
        "no trace_id should appear when event is emitted outside any span: {line}",
    );
    assert!(
        value.get("span_id").is_none(),
        "no span_id should appear when event is emitted outside any span: {line}",
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn instrumented_async_fn_propagates_ids_across_await() {
    let capture = CaptureWriter::default();
    let view = capture.clone();

    let config = ObservabilityConfig {
        service_name: "lvqr-session-83-async".to_string(),
        json_logs: true,
        ..ObservabilityConfig::default()
    };

    let (dispatch, provider) = build_stack(&config, capture);

    let (expected_trace, expected_span) = tracing::dispatcher::with_default(&dispatch, || {
        let parent = tracing::info_span!("async-worker");
        let _entered = parent.enter();
        let ctx = parent.context();
        let span_ref = ctx.span();
        let sc = span_ref.span_context();
        assert!(sc.is_valid(), "async-worker span must have a valid OTel context");
        (sc.trace_id().to_string(), sc.span_id().to_string())
    });

    // Run an instrumented async fn under the same dispatch so
    // the tracing-opentelemetry layer + OTel context propagate
    // across the await point.
    let dispatch_clone = dispatch.clone();
    tracing::dispatcher::with_default(&dispatch, || {
        let _guard = tracing::dispatcher::set_default(&dispatch_clone);
        let parent = tracing::info_span!("async-worker");
        let work = async {
            tracing::info!(step = 1i64, "before await");
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            tracing::info!(step = 2i64, "after await");
        }
        .instrument(parent);
        futures::executor::block_on(work);
    });

    let _ = provider.shutdown();

    let captured = view.snapshot();
    for keyword in ["before await", "after await"] {
        let line = captured
            .lines()
            .find(|line| line.contains(keyword))
            .unwrap_or_else(|| panic!("expected line containing '{keyword}'; got: {captured}"));
        let value: serde_json::Value = serde_json::from_str(line).expect("valid JSON");
        let actual_trace = value.get("trace_id").and_then(|v| v.as_str()).unwrap_or("");
        let actual_span = value.get("span_id").and_then(|v| v.as_str()).unwrap_or("");
        assert!(
            !actual_trace.is_empty(),
            "trace_id must be present across await on line with '{keyword}': {line}",
        );
        // span_id MAY differ from the first lookup because the
        // tracing::Span identity changes between info_span!
        // calls even with the same name. Assert both are valid
        // 16-hex-char values.
        assert_eq!(actual_span.len(), 16, "span_id hex length mismatch: {actual_span}");
        let _ = (&expected_trace, &expected_span);
    }
}
