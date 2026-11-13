# z2m-guard

Tells a **planned** Zigbee2MQTT restart apart from a **real** Zigbee outage — and
publishes that verdict where your alarm chain can read it.

> Status: M3 — running in production on a Docker-based Home Assistant stack.

## The problem

Every `docker compose up -d` recreates the Zigbee2MQTT container. For ~9 seconds
(measured: 8.7 s) every Zigbee entity goes `unavailable`, because all of them share
one availability topic:

```json
"availability": [{ "topic": "zigbee2mqtt/bridge/state",
                   "value_template": "{{ value_json.state }}" }]
```

Any alarm rule, automation or bridge that watches those entities fires — for a
routine container update. Worse, Z2M republishes its whole discovery tree on
startup, which on the reference host produced **179 messages / 4.49M tokens** in a
single weekly run.

## The idea

A small daemon subscribes to `zigbee2mqtt/bridge/state`, correlates the flap with a
pre-restart signal or container events, and publishes:

- `binary_sensor.z2m_guard_suppress` — `on` while the window is attributed
- `sensor.z2m_guard_state` — `idle | suspect | planned | settling | outage_confirmed`
- `sensor.z2m_guard_last_restart` — duration of the last suppressed downtime
- Prometheus metrics (`z2m_guard_suppressed_events_total{reason}` …)
- JSON-Lines events on stdout

## How it decides

| Situation | Verdict |
|---|---|
| pre-restart signal, then bridge goes away | **planned**, suppressed |
| container exit code 0 within 30 s of the flap | **planned**, suppressed |
| container exit code ≠ 0 or no attribution | **suspect** → 15 s grace |
| still gone after grace | **outage_confirmed**, suppression OFF, alarm passes |
| bridge back | **settling** for 20 s, then idle |

**Safety property:** suppression is never unbounded. Every suppression is attached
to a deadline, and an outage nobody asked for escalates after 15 s instead of being
swallowed. Proven by property tests over random input sequences.

## Deploy

```bash
docker build -t z2m-guard:local .
# service definition: deploy/docker-compose.snippet.yml
# config:             config/example.toml
# HA package:         deploy/homeassistant/z2m_guard.yaml
# Prometheus:         deploy/prometheus/scrape-snippet.yml
```

Announce a planned restart so it is attributed:

```bash
Z2M_GUARD_HTTP_TOKEN=… ./deploy/restart-z2m.sh 180
```

That posts an intent, then runs `docker compose up -d --force-recreate` for Z2M.

## Measured on the reference host

- planned recreate → `suppressed_events_total{reason="prior_signal"} = 1`,
  `outage_confirmed_total = 0`, downtime 8727 ms
- unexplained `docker stop` → `outage_confirmed_total = 1` after 15.5 s,
  suppression released, alarm passes

## Limitations

- Docker attribution needs either the pre-restart script (recommended, no
  privileges) or events piped on stdin (`--features stdin-events`). Reading the
  Docker socket directly is deliberately not implemented in v0.1: it is
  root-equivalent host access.
- The guard observes; it does not restart anything.
- Discovery republish storms are not reduced in v1 — that is the optional proxy
  mode (M5).
- Runs under a dedicated Mosquitto user with a least-privilege ACL
  (`deploy/mosquitto/acl.example`); it does not use the shared service credential.

## Build & release

```bash
cargo build --release                 # needs a C toolchain (rustls/ring)
./deploy/build-release.sh             # static musl binary + SHA256SUMS + CycloneDX SBOM
docker buildx build --platform linux/amd64,linux/arm64 -t z2m-guard:0.1.0 .
```

Full checklist: `docs/RELEASE.md`. Internals and the decisions behind them:
`docs/ARCHITECTURE.md`. Security model: `SECURITY.md`.

## Status / roadmap

- [x] M0 — workspace, licences, CI
- [x] M1 — core state machine (pure, 20 tests incl. 3 properties)
- [x] M2 — MQTT + Home Assistant discovery
- [x] M3 — attribution (pre-restart signal, docker events parser) + deployment
- [x] M4 — static musl artifacts, SBOM, multi-arch image, docs, release pipeline
- [ ] M5 — optional proxy mode

## Licence

MIT OR Apache-2.0, at your option.

## CI note

The workflow lives in `deploy/ci/github-actions-ci.yml`. It moves to
`.github/workflows/ci.yml` as soon as a token with the `workflow` scope is
available — the current one cannot create workflow files.
