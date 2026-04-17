//! Observability setup for LVQR (Tier 3 sessions G-J).
//!
//! This is the crate `lvqr-cli::main` calls at the top of the
//! process to wire tracing + metrics subscribers in one place.
//! Today (session 80, Tier 3 session G) it installs a single
//! stdout `fmt` layer gated by `RUST_LOG` / env filter -- the
//! same behavior the previous inline `tracing_subscriber::fmt()
//! .init()` call produced -- behind a
//! [`ObservabilityConfig::from_env`] facade so future sessions
//! can layer in OTLP span (H), OTLP metric (I), and JSON-log +
//! `trace_id` correlation (J) without touching the call site.
//!
//! ## Scope of session G
//!
//! * [`ObservabilityConfig`] with five fields: `otlp_endpoint`,
//!   `service_name`, `resource_attributes`, `json_logs`,
//!   `trace_sample_ratio`. All parsed from environment variables
//!   by [`ObservabilityConfig::from_env`] so operators can
//!   change behavior without a binary change.
//! * [`ObservabilityHandle`] -- an empty guard struct today.
//!   Sessions H / I will park the tracer-provider and
//!   meter-provider drop guards here so the OTLP background
//!   flushers do not leak past `main`.
//! * [`init`] installs the stdout fmt subscriber (honouring the
//!   `EnvFilter` rules the previous call used) and returns the
//!   handle. If `otlp_endpoint` is set, a warning is emitted
//!   that OTLP support is not yet wired; the stdout path still
//!   runs.
//!
//! ## What is deliberately NOT in this crate yet
//!
//! * OTLP span export via `opentelemetry-otlp`. Lands session H.
//! * OTLP metric export. Lands session I.
//! * JSON log formatting + `trace_id` correlation on every log
//!   line. Lands session J.
//! * Dependency on `opentelemetry*` crates -- deliberately held
//!   back so session-80 bring-up stays dependency-light and
//!   does not cascade version churn across the workspace.
//!
//! ## Env var surface
//!
//! Every config field reads from a single environment variable
//! with a well-known prefix. Unset values fall through to the
//! compiled-in defaults from [`ObservabilityConfig::default`].
//!
//! | env | field | default | future use |
//! |-----|-------|---------|------------|
//! | `LVQR_OTLP_ENDPOINT` | `otlp_endpoint` | `None` | session H: gRPC OTLP target |
//! | `LVQR_SERVICE_NAME` | `service_name` | `"lvqr"` | span / metric `service.name` resource |
//! | `LVQR_OTLP_RESOURCE` | `resource_attributes` | `[]` | comma-separated `k=v` pairs added to every span / metric |
//! | `LVQR_LOG_JSON` | `json_logs` | `false` | session J: flip the fmt layer from pretty → JSON |
//! | `LVQR_TRACE_SAMPLE_RATIO` | `trace_sample_ratio` | `1.0` | session H: head-based sampling ratio |
//!
//! Test infrastructure (`lvqr-test-utils::init_test_tracing`) is
//! intentionally left untouched -- tests keep the existing stdout
//! subscriber and skip OTLP entirely per the
//! `tracking/TIER_3_PLAN.md` observability-plane scope.

use std::str::FromStr;

use anyhow::{Context, Result};
use tracing_subscriber::EnvFilter;

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
    /// entirely; stdout logs still run. Sessions H (spans) and
    /// I (metrics) consume this.
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
        if let Some(v) = get(ENV_TRACE_SAMPLE_RATIO).filter(|s| !s.is_empty()) {
            if let Ok(parsed) = f64::from_str(&v) {
                cfg.trace_sample_ratio = parsed.clamp(0.0, 1.0);
            }
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

/// Lifetime guard for the observability subsystem. Drop semantics
/// are a no-op today; sessions H / I park the OTLP tracer and
/// meter provider guards here so the background flushers do
/// not leak past the end of `main`.
#[derive(Debug, Default)]
#[must_use = "dropping ObservabilityHandle shuts down OTLP exporters; hold it for the process lifetime"]
pub struct ObservabilityHandle {
    /// Reserved for session H: `opentelemetry_sdk::trace::TracerProvider`.
    /// Concrete type omitted until the dep is added.
    _tracer_provider: (),
    /// Reserved for session I: `opentelemetry_sdk::metrics::SdkMeterProvider`.
    _meter_provider: (),
}

/// Install the global tracing subscriber. Returns an
/// [`ObservabilityHandle`] that the caller should hold for the
/// process lifetime; dropping it before the process exits
/// shortens the background flusher windows of future sessions'
/// OTLP exporters.
///
/// Current behavior (session 80 / Tier 3 session G):
/// * Installs a single stdout `fmt` layer with an
///   [`EnvFilter`] sourced from `RUST_LOG` (or the
///   `"lvqr=info"` default when that env var is unset).
/// * Emits a `tracing::warn!` if
///   `config.otlp_endpoint.is_some()` telling the operator
///   OTLP support lands in session 81.
/// * Returns an empty handle.
///
/// Calling `init` more than once per process is an error
/// (`tracing::dispatcher::set_global_default` can only win
/// once). In tests, prefer `lvqr_test_utils::init_test_tracing`
/// which is idempotent on re-run.
pub fn init(config: ObservabilityConfig) -> Result<ObservabilityHandle> {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("lvqr=info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .try_init()
        .map_err(|e| anyhow::anyhow!("install global tracing subscriber: {e}"))
        .context("lvqr-observability init")?;

    if let Some(ref endpoint) = config.otlp_endpoint {
        tracing::warn!(
            endpoint = %endpoint,
            "LVQR_OTLP_ENDPOINT configured but OTLP export is not wired yet; \
             span export lands in Tier 3 session H (lvqr session 81)",
        );
    }

    tracing::info!(
        service_name = %config.service_name,
        json_logs = config.json_logs,
        trace_sample_ratio = config.trace_sample_ratio,
        "observability initialized (stdout fmt only; OTLP stub)",
    );

    Ok(ObservabilityHandle::default())
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
        // Empty strings don't override defaults.
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
        // If this ever breaks, the handle cannot be held on an
        // Arc across threads alongside the server state.
        fn require<T: Send + Sync>(_: &T) {}
        let h = ObservabilityHandle::default();
        require(&h);
    }
}
