//! Custom `tracing_subscriber::fmt` event formatter that adds
//! OTel `trace_id` + `span_id` fields to every log event when a
//! span context is active, and optionally renders as one JSON
//! object per line.
//!
//! Tier 3 session J. Replaces the default fmt layer so both
//! `LVQR_LOG_JSON=true` (JSON mode) and the default pretty mode
//! carry the same correlation fields, which is what Loki /
//! Promtail / Grafana Logs need to cross-reference OTLP traces.
//!
//! Design:
//!
//! * Single formatter type parameterised by an internal `Mode`
//!   enum (pretty / JSON) so the caller in `init()` can pick the
//!   mode at runtime without ending up with two different
//!   `fmt::Layer<S, ...>` types that cannot share a
//!   `tracing_subscriber::registry()` composition.
//! * `trace_id` + `span_id` read from
//!   `tracing_opentelemetry::OpenTelemetrySpanExt::context()` on
//!   the currently-entered `tracing::Span`. If no OTel-bound
//!   span is active (either because no span is entered or
//!   because the tracing-opentelemetry layer is not installed),
//!   both fields are omitted and the output is otherwise
//!   unchanged.
//! * Pretty mode renders a single-line RFC 3339 timestamp, level,
//!   target, optional `[trace_id=... span_id=...]` marker,
//!   message, and inline-formatted event fields. Deliberately
//!   close to the default `tracing_subscriber` pretty format so
//!   existing operator muscle memory still works.
//! * JSON mode emits one `serde_json::Value::Object` per event
//!   with `timestamp` / `level` / `target` / (`trace_id` /
//!   `span_id`) / event fields / open-span chain. Parseable by
//!   Loki, Promtail, Vector, and every log aggregator in the
//!   OTel ecosystem.

use std::fmt;

use chrono::{SecondsFormat, Utc};
use opentelemetry::trace::{SpanId, TraceId};
use serde_json::{Map, Value};
use tracing::{Event, Subscriber};
use tracing_opentelemetry::OtelData;
use tracing_subscriber::fmt::FmtContext;
use tracing_subscriber::fmt::format::{FormatEvent, FormatFields, Writer};
use tracing_subscriber::registry::LookupSpan;

/// Pretty (single-line human-readable) or JSON (one object per
/// line). Selected at `init()` time based on
/// [`crate::ObservabilityConfig::json_logs`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Pretty,
    Json,
}

/// Event formatter that renders tracing events with optional
/// OpenTelemetry correlation fields. Implements
/// [`FormatEvent`] so it can be plugged into
/// `fmt::layer().event_format(...)`.
///
/// Exposed as a public type so embedders that want to compose
/// their own `tracing_subscriber::fmt::layer()` (e.g. to swap
/// the writer in a test, or to stack additional layers behind
/// it) can still get the OTel correlation behaviour
/// `lvqr-cli::start` gets out of the box.
#[derive(Debug, Clone, Copy)]
pub struct CorrelatedFormat {
    mode: Mode,
}

impl CorrelatedFormat {
    pub fn new(json_logs: bool) -> Self {
        Self {
            mode: if json_logs { Mode::Json } else { Mode::Pretty },
        }
    }
}

impl<S, N> FormatEvent<S, N> for CorrelatedFormat
where
    S: Subscriber + for<'a> LookupSpan<'a>,
    N: for<'a> FormatFields<'a> + 'static,
{
    fn format_event(&self, ctx: &FmtContext<'_, S, N>, mut writer: Writer<'_>, event: &Event<'_>) -> fmt::Result {
        let ids = current_otel_ids(ctx);
        match self.mode {
            Mode::Pretty => write_pretty(&mut writer, ctx, event, ids),
            Mode::Json => write_json(&mut writer, ctx, event, ids),
        }
    }
}

/// Look up the OTel `(trace_id, span_id)` for the innermost
/// span the event was emitted inside. Returns `None` when the
/// event was emitted outside any span, or when the
/// `tracing-opentelemetry` layer has not attached an
/// [`OtelData`] extension to the span (e.g. because the
/// observability subsystem was initialised without OTLP).
///
/// Reads the extension directly rather than calling
/// `tracing::Span::current().context()` because inside a
/// `FormatEvent` impl the tracing dispatcher installs a
/// re-entrance guard that makes `Span::current()` return the
/// disabled span.
fn current_otel_ids<S, N>(ctx: &FmtContext<'_, S, N>) -> Option<(TraceId, SpanId)>
where
    S: Subscriber + for<'a> LookupSpan<'a>,
    N: for<'a> FormatFields<'a> + 'static,
{
    let span_ref = ctx.event_scope()?.from_root().last()?;
    let ext = span_ref.extensions();
    let otel_data = ext.get::<OtelData>()?;
    let trace_id = otel_data.builder.trace_id?;
    let span_id = otel_data.builder.span_id?;
    Some((trace_id, span_id))
}

fn write_pretty<S, N>(
    writer: &mut Writer<'_>,
    ctx: &FmtContext<'_, S, N>,
    event: &Event<'_>,
    ids: Option<(TraceId, SpanId)>,
) -> fmt::Result
where
    S: Subscriber + for<'a> LookupSpan<'a>,
    N: for<'a> FormatFields<'a> + 'static,
{
    let ts = Utc::now().to_rfc3339_opts(SecondsFormat::Micros, true);
    let metadata = event.metadata();
    write!(writer, "{ts} {:>5} {}", metadata.level(), metadata.target())?;

    if let Some((trace_id, span_id)) = ids {
        write!(writer, " [trace_id={trace_id} span_id={span_id}]")?;
    }

    write!(writer, ": ")?;
    ctx.field_format().format_fields(writer.by_ref(), event)?;
    writeln!(writer)
}

fn write_json<S, N>(
    writer: &mut Writer<'_>,
    ctx: &FmtContext<'_, S, N>,
    event: &Event<'_>,
    ids: Option<(TraceId, SpanId)>,
) -> fmt::Result
where
    S: Subscriber + for<'a> LookupSpan<'a>,
    N: for<'a> FormatFields<'a> + 'static,
{
    let mut obj = Map::new();
    obj.insert(
        "timestamp".into(),
        Value::String(Utc::now().to_rfc3339_opts(SecondsFormat::Micros, true)),
    );
    let metadata = event.metadata();
    obj.insert("level".into(), Value::String(metadata.level().to_string()));
    obj.insert("target".into(), Value::String(metadata.target().to_string()));
    if !metadata.name().is_empty() {
        obj.insert("name".into(), Value::String(metadata.name().to_string()));
    }

    if let Some((trace_id, span_id)) = ids {
        obj.insert("trace_id".into(), Value::String(trace_id.to_string()));
        obj.insert("span_id".into(), Value::String(span_id.to_string()));
    }

    let mut visitor = JsonVisitor::default();
    event.record(&mut visitor);
    for (key, value) in visitor.into_fields() {
        obj.insert(key, value);
    }

    // Open span chain, root-first. Each entry carries its name
    // + target + pre-rendered fields string (the default
    // `FormattedFields` shape tracing_subscriber stores on the
    // span's extensions at creation time).
    if let Some(scope) = ctx.event_scope() {
        let mut spans: Vec<Value> = Vec::new();
        for span in scope.from_root() {
            let mut entry = Map::new();
            entry.insert("name".into(), Value::String(span.name().to_string()));
            entry.insert("target".into(), Value::String(span.metadata().target().to_string()));
            {
                let ext = span.extensions();
                if let Some(formatted) = ext.get::<tracing_subscriber::fmt::FormattedFields<N>>()
                    && !formatted.fields.is_empty()
                {
                    entry.insert("fields".into(), Value::String(formatted.fields.clone()));
                }
            }
            spans.push(Value::Object(entry));
        }
        if !spans.is_empty() {
            obj.insert("spans".into(), Value::Array(spans));
        }
    }

    let line = serde_json::to_string(&obj).map_err(|_| fmt::Error)?;
    writeln!(writer, "{line}")
}

#[derive(Default)]
struct JsonVisitor {
    fields: Vec<(String, Value)>,
}

impl JsonVisitor {
    fn into_fields(self) -> Vec<(String, Value)> {
        self.fields
    }
}

impl tracing::field::Visit for JsonVisitor {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn fmt::Debug) {
        self.fields
            .push((field.name().to_string(), Value::String(format!("{value:?}"))));
    }

    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        self.fields
            .push((field.name().to_string(), Value::String(value.to_string())));
    }

    fn record_i64(&mut self, field: &tracing::field::Field, value: i64) {
        self.fields.push((field.name().to_string(), Value::from(value)));
    }

    fn record_u64(&mut self, field: &tracing::field::Field, value: u64) {
        self.fields.push((field.name().to_string(), Value::from(value)));
    }

    fn record_bool(&mut self, field: &tracing::field::Field, value: bool) {
        self.fields.push((field.name().to_string(), Value::Bool(value)));
    }

    fn record_f64(&mut self, field: &tracing::field::Field, value: f64) {
        // serde_json panics on NaN / infinity; f64::from returns Null in that case via Number::from_f64.
        self.fields.push((
            field.name().to_string(),
            serde_json::Number::from_f64(value)
                .map(Value::Number)
                .unwrap_or(Value::Null),
        ));
    }

    fn record_error(&mut self, field: &tracing::field::Field, value: &(dyn std::error::Error + 'static)) {
        self.fields
            .push((field.name().to_string(), Value::String(value.to_string())));
    }
}
