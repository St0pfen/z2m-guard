# Monitoring

Metrics are hand-rolled and exposed on `GET /metrics` in the Prometheus text format
(0.0.4) — the binary stays dependency-light. They answer one question: **did the
guard do its job**, nothing else. No host metrics, no broker instrumentation.

## The seven series

| Series | Type | Meaning |
|---|---|---|
| `z2m_guard_window_active` | gauge | `1` while a suppression window is open |
| `z2m_guard_suppressed_events_total{reason=…}` | counter | windows suppressed, split by attribution: `prior_signal`, `compose_recreate`, `crash`, `unknown` |
| `z2m_guard_outage_confirmed_total` | counter | unattributed outages that escalated (i.e. alarms that were allowed through) |
| `z2m_guard_flaps_total` | counter | availability flaps seen |
| `z2m_guard_mqtt_messages_total{kind="bridge"}` | counter | bridge-state messages consumed |
| `z2m_guard_discovery_deduped_total` | counter | discovery payloads dropped as duplicates — stays 0 unless the proxy mode (M5) is enabled |
| `z2m_guard_bridge_online_timestamp_seconds` | gauge | **milliseconds since guard start** of the last online transition, not a Unix timestamp |

That last one is a name/meaning mismatch worth knowing before writing queries:
`time() - z2m_guard_bridge_online_timestamp_seconds` is meaningless. Use it only as
a within-process marker ("the bridge came back 42 s after the guard started").

## Scraping

`deploy/prometheus/scrape-snippet.yml`:

```yaml
scrape_configs:
  - job_name: z2m-guard
    scrape_interval: 30s
    static_configs:
      - targets: ["172.22.1.51:8089"]   # or the compose network alias z2m-guard:8089
```

The guard publishes **no host ports**. Scrape it over the internal Docker network; if
Prometheus lives outside that network, publish `127.0.0.1:8089` only.

## PromQL worth having

```promql
# suppressed restarts per day, split by reason
sum by (reason) (increase(z2m_guard_suppressed_events_total[24h]))

# real outages that escalated today
increase(z2m_guard_outage_confirmed_total[24h])

# is a window open right now?
z2m_guard_window_active

# is the guard even receiving bridge state? (flat counter means it is not)
rate(z2m_guard_mqtt_messages_total{kind="bridge"}[1h])
```

A healthy week on a homelab stack looks like this: `prior_signal` climbing with your
update cadence, `compose_recreate` occasionally, `outage_confirmed_total` at zero,
`mqtt_messages_total` flat between restarts — the bridge topic is retained and only
changes when the bridge actually changes.

## Alert rules

```yaml
groups:
  - name: z2m-guard
    rules:
      - alert: ZigbeeOutageConfirmed
        expr: increase(z2m_guard_outage_confirmed_total[10m]) > 0
        for: 0m
        annotations:
          summary: "Zigbee2MQTT went away without an announcement"

      - alert: Z2mGuardWindowStuck
        expr: z2m_guard_window_active == 1
        for: 5m
        annotations:
          summary: "Suppression window open for 5 min — the bridge is not reporting online"

      - alert: Z2mGuardSilent
        expr: up{job="z2m-guard"} == 0
        for: 5m
        annotations:
          summary: "z2m-guard is not reachable, planned restarts are no longer attributed"
```

`Z2mGuardWindowStuck` is the one that catches the failure mode you cannot see in
Home Assistant: a window that never closes because the guard is not receiving bridge
state. The bound `prior_window_ms + settle_ms` keeps it below ~2.5 min with default
timings, so `for: 5m` is a safe threshold.

`Z2mGuardSilent` matters more than it looks: while the guard is down, planned
restarts are *not* attributed, and your alarms start firing again. That is a
deliberate fail-open design — silence of the guard must never hide a real outage.

## Grafana

`deploy/grafana/z2m-guard.json` imports as-is (Prometheus datasource UID `prometheus`
— rename if yours differs). It shows suppressed restarts per reason, confirmed
outages, the current window state and the last suppressed downtime.