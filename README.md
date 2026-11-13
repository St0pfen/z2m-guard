# z2m-guard

Tells a **planned** Zigbee2MQTT restart apart from a **real** Zigbee outage — and
publishes that verdict where your alarm chain can read it.

> Status: M0 (skeleton). No functionality yet.

## The problem

Every `docker compose up -d` recreates the Zigbee2MQTT container. For ~9 seconds
every Zigbee entity goes `unavailable`, because all of them share one availability
topic:

```json
"availability": [{ "topic": "zigbee2mqtt/bridge/state",
                   "value_template": "{{ value_json.state }}" }]
```

Any alarm rule, automation or bridge that watches those entities fires — for a
routine container update. Worse, Z2M republishes its whole discovery tree on
startup, which on this host produced **179 messages / 4.49M tokens** in a single
weekly run.

## The idea

A small daemon subscribes to `zigbee2mqtt/bridge/state`, correlates the flap with
container events or a pre-restart signal, and publishes:

- `binary_sensor.z2m_guard_suppress` — `on` while the outage is attributable
- `sensor.z2m_guard_state` — `idle | suspect | planned | settling | outage_confirmed`
- Prometheus metrics (`z2m_guard_suppressed_events_total{reason}` …)
- JSON-Lines events on stdout / webhook

**Safety property:** suppression requires attribution. An outage nobody asked for
escalates after a 15 s grace window instead of being swallowed.

## Scope

V1 is an observer — no changes to Zigbee2MQTT's data path. V2 (feature-gated) adds
an optional MQTT proxy that also collapses discovery republish storms.

## Status / roadmap

- [x] M0 — workspace, licences, CI
- [ ] M1 — core state machine (pure, unit-tested)
- [ ] M2 — MQTT + Home Assistant discovery
- [ ] M3 — attribution (Docker events / pre-restart signal) + deployment
- [ ] M4 — release artifacts, docs, announcement
- [ ] M5 — optional proxy mode

## Licence

MIT OR Apache-2.0, at your option.