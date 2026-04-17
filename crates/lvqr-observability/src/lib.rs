//! Observability setup for LVQR (Tier 3 sessions G-J).
//!
//! This is the crate `lvqr-cli::main` calls at the top of the
//! process to wire tracing + metrics subscribers in one place.
//! As of session 81 (Tier 3 session H) it can optionally
//! install an OTLP gRPC span exporter alongside the stdout
//! `fmt` layer, gated on `LVQR_OTLP_ENDPOINT`; when that env
//! var is unset, the original stdout-only path runs unchanged.
//! Sessions I and J will layer in OTLP metric export and JSON
//! log + `trace_id` correlation respectively.
//!
//! ## Scope of session H (this session)
//!
//! * Depend on the OTel workspace crates at the versions pinned
//!   in `tracking/TIER_3_PLAN.md`'s "Dependencies to pin" table:
//!   `opentelemetry` 0.27 / `opentelemetry_sdk` 0.27 (rt-tokio)
//!   / `opentelemetry-otlp` 0.27 (grpc-tonic) /
//!   `tracing-opentelemetry` 0.28.
//! * When `config.otlp_endpoint.is_some()`, build a
//!   [`opentelemetry_sdk::trace::TracerProvider`] backed by a
//!   `BatchSpanProcessor` over an OTLP gRPC exporter pointing at
//!   the configured endpoint. Compose a `tracing_opentelemetry`
//!   layer with the fmt layer through
//!   [`tracing_subscriber::registry`] so tracing spans flow out
//!   via OTLP AND still render on stdout for operators.
//! * Honour `config.service_name` and
//!   `config.resource_attributes` through the `Resource` applied
//!   to the `TracerProvider`.
//! * Honour `config.trace_sample_ratio` via
//!   `opentelemetry_sdk::trace::Sampler::TraceIdRatioBased`.
//! * Park the `TracerProvider` on
//!   [`ObservabilityHandle::tracer_provider`] and force-flush +
//!   shut it down on drop so pending spans are not lost when
//!   the process exits.
//!
//! ## Scope of session G (already landed, unchanged here)
//!
//! * [`ObservabilityConfig`] + [`ObservabilityConfig::from_env`]
//!   parse five env vars (`LVQR_OTLP_ENDPOINT`,
//!   `LVQR_SERVICE_NAME`, `LVQR_OTLP_RESOURCE`, `LVQR_LOG_JSON`,
//!   `LVQR_TRACE_SAMPLE_RATIO`). See the env-var surface table
//!   below.
//! * [`ObservabilityHandle`] is `#[must_use]`; dropping it
//!   flushes and shuts down the OTLP exporter (session H
//!   addition).
//!
//! ## What is still deliberately NOT in this crate yet
//!
//! * OTLP metric export via `opentelemetry_sdk::metrics`. Lands
//!   session I.
//! * JSON log formatting + `trace_id` correlation on every log
//!   line. Lands session J.
//!
//! ## Env var surface
//!
//! Every config field reads from a single environment variable
//! with a well-known prefix. Unset values fall through to the
//! compiled-in defaults from [`ObservabilityConfig::default`].
//!
//! | env | field | default | consumer |
//! |-----|-------|---------|----------|
//! | `LVQR_OTLP_ENDPOINT` | `otlp_endpoint` | `None` | OTLP gRPC target (session H span; session I metric) |
//! | `LVQR_SERVICE_NAME` | `service_name` | `"lvqr"` | span / metric `service.name` resource |
//! | `LVQR_OTLP_RESOURCE` | `resource_attributes` | `[]` | comma-separated `k=v` pairs added to every span / metric |
//! | `LVQR_LOG_JSON` | `json_logs` | `false` | session J: flip the fmt layer from pretty to JSON |
//! | `LVQR_TRACE_SAMPLE_RATIO` | `trace_sample_ratio` | `1.0` | head-based sampling ratio, clamped to `[0.0, 1.0]` |
//!
//! Test infrastructure (`lvqr-test-utils::init_test_tracing`) is
//! intentionally left untouched -- tests keep the existing stdout
//! subscriber and skip OTLP entirely per the
//! `tracking/TIER_3_PLAN.md` observability-plane scope.

use std::str::FromStr;

use anyhow::{Context, Result};
use opentelemetry::KeyValue;
use opentelemetry::trace::TracerProvider as _;
use opentelemetry_otlp::WithExportConfig;
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::export::trace::SpanExporter;
use opentelemetry_sdk::runtime;
use opentelemetry_sdk::trace::{Sampler, TracerProvider};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, fmt};

/// Environment variable that, when set, supplies the OTLP
/// collector endpoint. Example: `http://localhost:4317`.
pub const ENV_OTLP_ENDPOINT: &str = "LVQR_OTLP_ENDPOINT";
/// Environment variable that overrides `service.name` resource.
pub const ENV_SERVICE_NAME: &str = "LVQR_SERVICE_NAME";
/// Environment variable for extra resource attributes. Format:
/// comma-separated `k=v` pairs (`deploy.env=prod,region=us-east-1`).
pub const ENV_RESOURCE_ATTRIBUTES: &str = "LVQR_OTLP_RESOURCE";
/// Environment variable that flips the fmt layer to JSON when
/// set to a truthy value (`1` / `true` / `yes`, case-insensitive).
pub const ENV_JSON_LOGS: &str = "LVQR_LOG_JSON";
/// Environment variable for head-based trace sampling. Clamped
/// to `[0.0, 1.0]` on parse.
pub const ENV_TRACE_SAMPLE_RATIO: &str = "LVQR_TRACE_SAMPLE_RATIO";

/// Configuration for the observability subsystem. Every field
/// has a sensible default; see [`ObservabilityConfig::default`].
#[derive(Debug, Clone)]
pub struct ObservabilityConfig {
    /// OTLP collector endpoint. `None` disables OTLP export
    /// entirely; stdout logs still run. Session H (spans) and
    /// session I (metrics) consume this.
    pub otlp_endpoint: Option<String>,
    /// `service.name` resource attribute applied to every span
    /// / metric. Operators set this to distinguish multiple
    /// LVQR processes in a single Jaeger / Tempo instance.
    pub service_name: String,
    /// Additional resource attributes applied alongside
    /// `service.name`. Format:
    /// `[("deploy.env", "prod"), ("region", "us-east-1")]`.
    pub resource_attributes: Vec<(String, String)>,
    /// Render logs as JSON (one event per line) rather than the
    /// human-readable default. Session J wires in the fmt-layer
    /// switch and adds `trace_id` / `span_id` correlation fields.
    pub json_logs: bool,
    /// Head-based trace sampling ratio in `[0.0, 1.0]`. `1.0`
    /// records every trace; `0.0` records none. Tail sampling
    /// is explicitly out of scope per
    /// `tracking/TIER_3_PLAN.md`.
    pub trace_sample_ratio: f64,
}

impl Default for ObservabilityConfig {
    fn default() -> Self {
        Self {
            otlp_endpoint: None,
            service_name: "lvqr".to_string(),
            resource_attributes: Vec::new(),
            json_logs: false,
            trace_sample_ratio: 1.0,
        }
    }
}

impl ObservabilityConfig {
    /// Build a config from the process environment. Every
    /// missing env var falls through to the default in
    /// [`Self::default`]. Parse failures fall through to the
    /// default too rather than failing `init`; a misspelled
    /// value should not take the server down.
    pub fn from_env() -> Self {
        Self::from_env_reader(|k| std::env::var(k).ok())
    }

    /// Pure-function variant of [`Self::from_env`] that takes
    /// an arbitrary lookup closure. Used by unit tests to
    /// exercise every env-parse branch without mutating the
    /// process env (which is unsafe under tokio test
    /// parallelism).
    pub fn from_env_reader<F>(get: F) -> Self
    where
        F: Fn(&str) -> Option<String>,
    {
        let mut cfg = Self::default();
        if let Some(v) = get(ENV_OTLP_ENDPOINT).filter(|s| !s.is_empty()) {
            cfg.otlp_endpoint = Some(v);
        }
        if let Some(v) = get(ENV_SERVICE_NAME).filter(|s| !s.is_empty()) {
            cfg.service_name = v;
        }
        if let Some(v) = get(ENV_RESOURCE_ATTRIBUTES).filter(|s| !s.is_empty()) {
            cfg.resource_attributes = parse_resource_attributes(&v);
        }
        if let Some(v) = get(ENV_JSON_LOGS).filter(|s| !s.is_empty()) {
            cfg.json_logs = parse_truthy(&v);
        }
        if let Some(v) = get(ENV_TRACE_SAMPLE_RATIO).filter(|s| !s.is_empty())
            && let Ok(parsed) = f64::from_str(&v)
        {
            cfg.trace_sample_ratio = parsed.clamp(0.0, 1.0);
        }
        cfg
    }
}

/// Parse a comma-separated `k=v` list into a flat `Vec`.
/// Trims whitespace per token, drops entries with an empty
/// key or missing `=`. Duplicates are not deduplicated --
/// downstream OTel code sees the last-wins order.
fn parse_resource_attributes(raw: &str) -> Vec<(String, String)> {
    raw.split(',')
        .filter_map(|token| {
            let token = token.trim();
            if token.is_empty() {
                return None;
            }
            let (k, v) = token.split_once('=')?;
            let k = k.trim();
            if k.is_empty() {
                return None;
            }
            Some((k.to_string(), v.trim().to_string()))
        })
        .collect()
}

fn parse_truthy(raw: &str) -> bool {
    matches!(raw.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on")
}

/// Lifetime guard for the observability subsystem. Hold this
/// for the process lifetime; dropping it force-flushes and
/// shuts down the OTLP tracer provider (session H) so the
/// background batch span processor does not lose pending
/// spans. Single-node deployments without
/// `LVQR_OTLP_ENDPOINT` set carry `None` here and drop is
/// a no-op.
#[derive(Debug, Default)]
#[must_use = "dropping ObservabilityHandle shuts down OTLP exporters; hold it for the process lifetime"]
pub struct ObservabilityHandle {
    /// The OTLP tracer provider, when
    /// `config.otlp_endpoint.is_some()`. `None` for the default
    /// stdout-only path.
    tracer_provider: Option<TracerProvider>,
}

impl Drop for ObservabilityHandle {
    fn drop(&mut self) {
        if let Some(provider) = self.tracer_provider.take() {
            for result in provider.force_flush() {
                if let Err(err) = result {
                    eprintln!("lvqr-observability: force_flush reported error: {err}");
                }
            }
            if let Err(err) = provider.shutdown() {
                eprintln!("lvqr-observability: tracer provider shutdown error: {err}");
            }
        }
    }
}

/// Build a resource from the service name + user-supplied
/// attribute list. Split out so the integration test can build
/// an equivalent `TracerProvider` without duplicating the
/// attribute-merge logic.
fn build_resource(config: &ObservabilityConfig) -> Resource {
    let mut kvs: Vec<KeyValue> = Vec::with_capacity(1 + config.resource_attributes.len());
    kvs.push(KeyValue::new("service.name", config.service_name.clone()));
    for (k, v) in &config.resource_attributes {
        kvs.push(KeyValue::new(k.clone(), v.clone()));
    }
    Resource::new(kvs)
}

/// Build a [`TracerProvider`] configured per `config` with a
/// caller-supplied [`SpanExporter`]. Production path calls this
/// with an OTLP gRPC exporter built by [`init`]; integration
/// tests call it with an in-memory exporter so a synthetic span
/// can be asserted end-to-end without a real network
/// round-trip.
///
/// The exporter is wrapped in a `BatchSpanProcessor` backed by
/// [`runtime::Tokio`], so the caller MUST be inside a Tokio
/// runtime when this function is invoked. `lvqr-cli::main` is
/// already `#[tokio::main]`; tests mark themselves
/// `#[tokio::test]`.
pub fn build_tracer_provider<E>(config: &ObservabilityConfig, exporter: E) -> TracerProvider
where
    E: SpanExporter + 'static,
{
    TracerProvider::builder()
        .with_batch_exporter(exporter, runtime::Tokio)
        .with_resource(build_resource(config))
        .with_sampler(Sampler::TraceIdRatioBased(config.trace_sample_ratio))
        .build()
}

/// Install the global tracing subscriber. Returns an
/// [`ObservabilityHandle`] that the caller MUST hold for the
/// process lifetime; dropping it force-flushes and shuts down
/// the OTLP exporter so pending spans are not lost.
///
/// Behavior as of session 81 (Tier 3 session H):
/// * Installs a stdout `fmt` layer with an [`EnvFilter`]
///   sourced from `RUST_LOG` (or the `"lvqr=info"` default when
///   that env var is unset). Matches the previous inline
///   `tracing_subscriber::fmt().init()` behavior.
/// * If `config.otlp_endpoint.is_some()`, ALSO installs a
///   `tracing_opentelemetry` layer backed by an OTLP gRPC
///   exporter. Spans emitted through `tracing::instrument`,
///   `tracing::info_span!`, etc. flow both to stdout AND out
///   the OTLP exporter.
/// * If the OTLP exporter fails to build (invalid endpoint,
///   DNS miss at process start, tonic initialization failure),
///   the init call returns `Err`; the caller can decide
///   whether to fail fast or continue without tracing.
///
/// Calling `init` more than once per process returns an error
/// (`tracing::dispatcher::set_global_default` can only win
/// once). In tests, prefer `lvqr_test_utils::init_test_tracing`
/// which is idempotent on re-run.
pub fn init(config: ObservabilityConfig) -> Result<ObservabilityHandle> {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("lvqr=info"));
    let fmt_layer = fmt::layer();

    let tracer_provider = if let Some(endpoint) = config.otlp_endpoint.as_deref() {
        let exporter = opentelemetry_otlp::SpanExporter::builder()
            .with_tonic()
            .with_endpoint(endpoint)
            .build()
            .context("build OTLP gRPC span exporter")?;
        Some(build_tracer_provider(&config, exporter))
    } else {
        None
    };

    let otel_layer = tracer_provider.as_ref().map(|provider| {
        let tracer = provider.tracer(config.service_name.clone());
        tracing_opentelemetry::layer().with_tracer(tracer)
    });

    tracing_subscriber::registry()
        .with(filter)
        .with(fmt_layer)
        .with(otel_layer)
        .try_init()
        .map_err(|e| anyhow::anyhow!("install global tracing subscriber: {e}"))
        .context("lvqr-observability init")?;

    if let Some(ref provider) = tracer_provider {
        opentelemetry::global::set_tracer_provider(provider.clone());
    }

    tracing::info!(
        service_name = %config.service_name,
        otlp_enabled = tracer_provider.is_some(),
        otlp_endpoint = config.otlp_endpoint.as_deref().unwrap_or(""),
        json_logs = config.json_logs,
        trace_sample_ratio = config.trace_sample_ratio,
        "observability initialized",
    );

    Ok(ObservabilityHandle { tracer_provider })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn mk_env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect();
        move |k: &str| map.get(k).cloned()
    }

    #[test]
    fn default_matches_shipped_defaults() {
        let cfg = ObservabilityConfig::default();
        assert!(cfg.otlp_endpoint.is_none());
        assert_eq!(cfg.service_name, "lvqr");
        assert!(cfg.resource_attributes.is_empty());
        assert!(!cfg.json_logs);
        assert!((cfg.trace_sample_ratio - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn from_env_reader_empty_falls_back_to_defaults() {
        let cfg = ObservabilityConfig::from_env_reader(mk_env(&[]));
        let default = ObservabilityConfig::default();
        assert_eq!(cfg.otlp_endpoint, default.otlp_endpoint);
        assert_eq!(cfg.service_name, default.service_name);
        assert_eq!(cfg.json_logs, default.json_logs);
    }

    #[test]
    fn from_env_reader_parses_every_field() {
        let cfg = ObservabilityConfig::from_env_reader(mk_env(&[
            (ENV_OTLP_ENDPOINT, "http://localhost:4317"),
            (ENV_SERVICE_NAME, "lvqr-edge-01"),
            (ENV_RESOURCE_ATTRIBUTES, "deploy.env=prod, region=us-east-1"),
            (ENV_JSON_LOGS, "true"),
            (ENV_TRACE_SAMPLE_RATIO, "0.25"),
        ]));
        assert_eq!(cfg.otlp_endpoint.as_deref(), Some("http://localhost:4317"));
        assert_eq!(cfg.service_name, "lvqr-edge-01");
        assert_eq!(
            cfg.resource_attributes,
            vec![
                ("deploy.env".to_string(), "prod".to_string()),
                ("region".to_string(), "us-east-1".to_string()),
            ]
        );
        assert!(cfg.json_logs);
        assert!((cfg.trace_sample_ratio - 0.25).abs() < f64::EPSILON);
    }

    #[test]
    fn empty_env_var_value_is_treated_as_unset() {
        let cfg = ObservabilityConfig::from_env_reader(mk_env(&[(ENV_OTLP_ENDPOINT, ""), (ENV_SERVICE_NAME, "")]));
        assert!(cfg.otlp_endpoint.is_none());
        assert_eq!(cfg.service_name, "lvqr");
    }

    #[test]
    fn json_logs_truthy_variants() {
        for val in ["1", "true", "TRUE", "yes", "On"] {
            let cfg = ObservabilityConfig::from_env_reader(mk_env(&[(ENV_JSON_LOGS, val)]));
            assert!(cfg.json_logs, "{val} should parse as truthy");
        }
    }

    #[test]
    fn json_logs_falsy_variants() {
        for val in ["0", "false", "FALSE", "no", "off"] {
            let cfg = ObservabilityConfig::from_env_reader(mk_env(&[(ENV_JSON_LOGS, val)]));
            assert!(!cfg.json_logs, "{val} should parse as falsy");
        }
    }

    #[test]
    fn json_logs_unknown_value_defaults_to_false() {
        let cfg = ObservabilityConfig::from_env_reader(mk_env(&[(ENV_JSON_LOGS, "banana")]));
        assert!(!cfg.json_logs);
    }

    #[test]
    fn trace_sample_ratio_clamps_out_of_range() {
        let high = ObservabilityConfig::from_env_reader(mk_env(&[(ENV_TRACE_SAMPLE_RATIO, "5.0")]));
        assert!((high.trace_sample_ratio - 1.0).abs() < f64::EPSILON);
        let low = ObservabilityConfig::from_env_reader(mk_env(&[(ENV_TRACE_SAMPLE_RATIO, "-0.5")]));
        assert!((low.trace_sample_ratio - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn trace_sample_ratio_unparseable_falls_back_to_default() {
        let cfg = ObservabilityConfig::from_env_reader(mk_env(&[(ENV_TRACE_SAMPLE_RATIO, "banana")]));
        assert!((cfg.trace_sample_ratio - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn resource_attributes_tolerates_whitespace_and_empty_tokens() {
        let raw = " a=1 , b = 2 ,,,c=  ,  d=  4 , =broken , nokv ";
        let parsed = parse_resource_attributes(raw);
        assert_eq!(
            parsed,
            vec![
                ("a".to_string(), "1".to_string()),
                ("b".to_string(), "2".to_string()),
                ("c".to_string(), "".to_string()),
                ("d".to_string(), "4".to_string()),
            ]
        );
    }

    #[test]
    fn resource_attributes_empty_returns_empty() {
        assert!(parse_resource_attributes("").is_empty());
        assert!(parse_resource_attributes("  ").is_empty());
    }

    #[test]
    fn observability_handle_is_send_and_sync() {
        fn require<T: Send + Sync>(_: &T) {}
        let h = ObservabilityHandle::default();
        require(&h);
    }

    #[test]
    fn build_resource_merges_service_name_and_attrs() {
        let cfg = ObservabilityConfig {
            service_name: "lvqr-a".to_string(),
            resource_attributes: vec![
                ("deploy.env".to_string(), "prod".to_string()),
                ("region".to_string(), "us-east-1".to_string()),
            ],
            ..ObservabilityConfig::default()
        };
        let resource = build_resource(&cfg);
        let service_name = resource
            .get(opentelemetry::Key::from_static_str("service.name"))
            .map(|v| v.to_string());
        assert_eq!(service_name.as_deref(), Some("lvqr-a"));
        let env = resource
            .get(opentelemetry::Key::from_static_str("deploy.env"))
            .map(|v| v.to_string());
        assert_eq!(env.as_deref(), Some("prod"));
        let region = resource
            .get(opentelemetry::Key::from_static_str("region"))
            .map(|v| v.to_string());
        assert_eq!(region.as_deref(), Some("us-east-1"));
    }
}
