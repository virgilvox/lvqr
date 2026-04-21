# LVQR Grafana / Prometheus alert pack

Shipped as part of Tier 4 item 4.7 session B. Contents:

* [`alerts/lvqr-slo.rules.yaml`](alerts/lvqr-slo.rules.yaml) --
  Prometheus-format alert rules for the latency SLO. Five rules:
  three severity tiers on p99 / p95 / p50, one on p99 with a shorter
  fire window (critical), and an availability alert that catches an
  egress drain stall without triggering on a clean publisher
  disconnect.
* [`dashboards/lvqr-slo.json`](dashboards/lvqr-slo.json) --
  Grafana dashboard (schema version 38, tested against Grafana 10.4
  and 11.x). Imports via Grafana's "Import" UI or via a provisioning
  YAML under `/etc/grafana/provisioning/dashboards/`.

See [`../../docs/slo.md`](../../docs/slo.md) for the operator runbook
that links each alert to its diagnostic checklist.

## Importing the alert rules

### Prometheus

```yaml
# prometheus.yml
rule_files:
  - /etc/prometheus/rules/lvqr-slo.rules.yaml
```

Drop the YAML under `/etc/prometheus/rules/` and reload Prometheus
(`kill -HUP <pid>` or the `/-/reload` endpoint if `--web.enable-lifecycle`
is on).

### Grafana Cloud (managed alerts)

Navigate to **Alerting -> Alert rules -> Import**, pick
**Prometheus format**, and paste the file contents. Grafana Cloud
splits the rule group into individual rules automatically.

## Importing the dashboard

### Grafana UI

**Dashboards -> New -> Import -> Upload JSON file**. Pick
`dashboards/lvqr-slo.json`. Select the Prometheus datasource when
prompted (the dashboard uses `${DS_PROMETHEUS}` as the variable name
so any Prometheus-shaped datasource works).

### Grafana provisioning

```yaml
# /etc/grafana/provisioning/dashboards/lvqr.yaml
apiVersion: 1
providers:
  - name: lvqr
    folder: LVQR
    type: file
    options:
      path: /etc/grafana/dashboards/lvqr
```

Drop `lvqr-slo.json` into `/etc/grafana/dashboards/lvqr/` and restart
Grafana. The dashboard's `uid: lvqr-slo` makes it linkable from
external tools (runbooks, the alert pack's `runbook_url` annotations,
etc.).

## Threshold tuning

The default rule thresholds target LL-HLS (2 s target segment, 200 ms
partial target). Operators on WebRTC / MoQ paths with sub-second
latency SLOs should:

1. Copy `lvqr-slo.rules.yaml` to a new file (e.g.
   `lvqr-slo-whep.rules.yaml`).
2. Add a label matcher to every `expr:` block, e.g. append
   `and on (broadcast, transport) (label_replace(...))` filters, or
   simply hard-code `transport="whep"` on the `sum by (...)` line.
3. Tighten thresholds (typical WebRTC: p99 > 500 ms critical, p95 >
   250 ms warning, p50 > 100 ms info).

See [`../../docs/slo.md#threshold-tuning-by-transport`](../../docs/slo.md#threshold-tuning-by-transport)
for the canonical decision table.
