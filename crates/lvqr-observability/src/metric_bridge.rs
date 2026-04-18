//! `metrics` crate → OpenTelemetry SDK bridge (Tier 3 session I).
//!
//! Exposes [`OtelMetricsRecorder`], a [`metrics::Recorder`] impl
//! that forwards every `metrics::counter!` / `gauge!` /
//! `histogram!` call site to an equivalent OTel instrument on a
//! shared [`opentelemetry::metrics::Meter`]. The resulting
//! instruments flow out whatever `PushMetricExporter` the
//! caller wired onto the `SdkMeterProvider` -- production calls
//! [`crate::build_meter_provider`] with an OTLP gRPC exporter;
//! the integration test uses an in-memory shim.
//!
//! ## Label handling
//!
//! Every call-site label (e.g. `"type" => "video"`) is mapped
//! to an [`opentelemetry::KeyValue`] and passed per-record.
//! OTel instruments in 0.27 are name-keyed only, so we intern a
//! single instrument per metric name (across all label sets) in
//! a [`dashmap::DashMap`]; the record call decorates with the
//! per-callsite attributes.
//!
//! ## Counter / histogram semantics
//!
//! Counters and histograms round-trip cleanly:
//! * `counter!(name, ...).increment(v)` ->
//!   `Counter<u64>::add(v, &attrs)`.
//! * `histogram!(name, ...).record(v)` ->
//!   `Histogram<f64>::record(v, &attrs)`.
//!
//! ## Gauge semantics (best-effort in session I)
//!
//! `metrics::GaugeFn` has three methods: `increment`, `decrement`,
//! `set`. OTel 0.27 has a single synchronous instrument per
//! semantic: an `UpDownCounter<f64>` is natural for inc/dec but
//! has no absolute-set operation, and a `Gauge<f64>` supports
//! `record` (absolute) but has no inc/dec. We model gauges with
//! an `UpDownCounter<f64>` to cover the dominant call pattern in
//! the LVQR codebase today (see `lvqr_active_streams`,
//! `lvqr_active_moq_sessions`, `lvqr_mesh_peers`). `set(v)`
//! falls back to emitting `(v - last_seen)` as an inc/dec delta
//! via a per-instrument `AtomicU64` holding the last-written
//! value. This keeps the exported counter monotonically
//! reflecting the latest set, at the cost of losing fidelity
//! when multiple tasks race on `set` concurrently (which none
//! of our current call sites do). Session J can switch this to
//! a per-`(name, labels)` `ObservableGauge` path if fidelity
//! under concurrent `set` ever matters.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use dashmap::DashMap;
use metrics::{Counter, CounterFn, Gauge, GaugeFn, Histogram, HistogramFn, Key, KeyName, Metadata, SharedString, Unit};
use opentelemetry::KeyValue;
use opentelemetry::metrics::{
    Counter as OtelCounter, Histogram as OtelHistogram, Meter, MeterProvider as _, UpDownCounter,
};
use opentelemetry_sdk::metrics::SdkMeterProvider;

/// Name under which the bridge registers its OTel scope. Shows
/// up as `instrumentation.scope.name` on every exported metric.
const SCOPE_NAME: &str = "lvqr-observability";

/// A [`metrics::Recorder`] implementation that forwards every
/// `metrics::counter!` / `gauge!` / `histogram!` call site to an
/// OTel [`SdkMeterProvider`]. Construct via
/// [`OtelMetricsRecorder::new`] from a provider handed out by
/// [`crate::build_meter_provider`] or
/// [`crate::ObservabilityHandle::take_metrics_recorder`].
#[derive(Clone)]
pub struct OtelMetricsRecorder {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for OtelMetricsRecorder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OtelMetricsRecorder")
            .field("counters", &self.inner.counters.len())
            .field("gauges", &self.inner.gauges.len())
            .field("histograms", &self.inner.histograms.len())
            .finish()
    }
}

struct Inner {
    meter: Meter,
    counters: DashMap<String, OtelCounter<u64>>,
    gauges: DashMap<String, GaugeSlot>,
    histograms: DashMap<String, OtelHistogram<f64>>,
}

struct GaugeSlot {
    instrument: UpDownCounter<f64>,
    // Last-written absolute value per (name), encoded as f64::to_bits.
    // Used by `GaugeFn::set` to emit a delta on the up-down counter.
    last: Arc<AtomicU64>,
}

impl OtelMetricsRecorder {
    /// Build a recorder that forwards into `meter_provider`.
    /// The returned value is cheap to clone and is `Send +
    /// Sync`, so it can be handed to `metrics::set_global_recorder`
    /// or wrapped in a `metrics-util::FanoutBuilder`.
    pub fn new(meter_provider: &SdkMeterProvider) -> Self {
        Self {
            inner: Arc::new(Inner {
                meter: meter_provider.meter(SCOPE_NAME),
                counters: DashMap::new(),
                gauges: DashMap::new(),
                histograms: DashMap::new(),
            }),
        }
    }

    fn counter(&self, name: &str) -> OtelCounter<u64> {
        self.inner
            .counters
            .entry(name.to_string())
            .or_insert_with(|| self.inner.meter.u64_counter(name.to_string()).build())
            .clone()
    }

    fn gauge(&self, name: &str) -> GaugeSlot {
        self.inner
            .gauges
            .entry(name.to_string())
            .or_insert_with(|| GaugeSlot {
                instrument: self.inner.meter.f64_up_down_counter(name.to_string()).build(),
                last: Arc::new(AtomicU64::new(0.0f64.to_bits())),
            })
            .value_clone()
    }

    fn histogram(&self, name: &str) -> OtelHistogram<f64> {
        self.inner
            .histograms
            .entry(name.to_string())
            .or_insert_with(|| self.inner.meter.f64_histogram(name.to_string()).build())
            .clone()
    }
}

impl GaugeSlot {
    fn value_clone(&self) -> Self {
        Self {
            instrument: self.instrument.clone(),
            last: self.last.clone(),
        }
    }
}

fn key_to_attrs(key: &Key) -> Vec<KeyValue> {
    key.labels()
        .map(|label| KeyValue::new(label.key().to_string(), label.value().to_string()))
        .collect()
}

impl metrics::Recorder for OtelMetricsRecorder {
    fn describe_counter(&self, _key: KeyName, _unit: Option<Unit>, _description: SharedString) {
        // OTel 0.27 instruments are late-bound (first registration
        // wins); no-op is the right behavior for describe_* when
        // the instrument hasn't been touched yet. Instruments that
        // have already been built ignore description updates too.
    }

    fn describe_gauge(&self, _key: KeyName, _unit: Option<Unit>, _description: SharedString) {}

    fn describe_histogram(&self, _key: KeyName, _unit: Option<Unit>, _description: SharedString) {}

    fn register_counter(&self, key: &Key, _metadata: &Metadata<'_>) -> Counter {
        let counter = self.counter(key.name());
        let attrs = key_to_attrs(key);
        Counter::from_arc(Arc::new(OtelCounterHandle { counter, attrs }))
    }

    fn register_gauge(&self, key: &Key, _metadata: &Metadata<'_>) -> Gauge {
        let slot = self.gauge(key.name());
        let attrs = key_to_attrs(key);
        Gauge::from_arc(Arc::new(OtelGaugeHandle {
            instrument: slot.instrument,
            last: slot.last,
            attrs,
        }))
    }

    fn register_histogram(&self, key: &Key, _metadata: &Metadata<'_>) -> Histogram {
        let histogram = self.histogram(key.name());
        let attrs = key_to_attrs(key);
        Histogram::from_arc(Arc::new(OtelHistogramHandle { histogram, attrs }))
    }
}

struct OtelCounterHandle {
    counter: OtelCounter<u64>,
    attrs: Vec<KeyValue>,
}

impl CounterFn for OtelCounterHandle {
    fn increment(&self, value: u64) {
        self.counter.add(value, &self.attrs);
    }

    fn absolute(&self, value: u64) {
        // OTel has no true "absolute counter" concept on the sync
        // path; closest semantic is to treat absolute as a
        // monotonic add. Call sites in lvqr use .increment
        // exclusively today, so this branch is exercised rarely.
        self.counter.add(value, &self.attrs);
    }
}

struct OtelGaugeHandle {
    instrument: UpDownCounter<f64>,
    last: Arc<AtomicU64>,
    attrs: Vec<KeyValue>,
}

impl GaugeFn for OtelGaugeHandle {
    fn increment(&self, value: f64) {
        self.instrument.add(value, &self.attrs);
        let _ = self.last.fetch_update(Ordering::AcqRel, Ordering::Acquire, |bits| {
            Some((f64::from_bits(bits) + value).to_bits())
        });
    }

    fn decrement(&self, value: f64) {
        self.instrument.add(-value, &self.attrs);
        let _ = self.last.fetch_update(Ordering::AcqRel, Ordering::Acquire, |bits| {
            Some((f64::from_bits(bits) - value).to_bits())
        });
    }

    fn set(&self, value: f64) {
        // Emit the delta from the last observed absolute value so
        // the UpDownCounter's running total converges on `value`.
        let mut current = self.last.load(Ordering::Acquire);
        let target_bits = value.to_bits();
        loop {
            match self
                .last
                .compare_exchange(current, target_bits, Ordering::AcqRel, Ordering::Acquire)
            {
                Ok(_) => {
                    let delta = value - f64::from_bits(current);
                    if delta != 0.0 {
                        self.instrument.add(delta, &self.attrs);
                    }
                    break;
                }
                Err(actual) => current = actual,
            }
        }
    }
}

struct OtelHistogramHandle {
    histogram: OtelHistogram<f64>,
    attrs: Vec<KeyValue>,
}

impl HistogramFn for OtelHistogramHandle {
    fn record(&self, value: f64) {
        self.histogram.record(value, &self.attrs);
    }
}
