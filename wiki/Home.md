# z2m-guard

Separates **planned** Zigbee2MQTT restarts from **real** Zigbee outages — and
publishes that verdict where your alarm chain can read it.

Current release: **0.1.2** · MIT OR Apache-2.0 · static musl binaries, CycloneDX
SBOM and a multi-arch image ship with every tag.

## The problem

Every Zigbee entity in Home Assistant shares one availability topic,
`zigbee2mqtt/bridge/state`. A routine `docker compose up -d` recreates the
container, that topic goes `offline` for a few seconds, and all ~100 entities turn
`unavailable` — every watchdog, every notification automation, every dashboard
fires at once. The values were never wrong; the alarm is.

Worse, Z2M republishes its entire discovery tree on startup: on the reference host
that was **179 messages / 4.49 M tokens** in a single weekly run.

z2m-guard watches that one topic, knows whether a restart was announced, and
publishes a **bounded** suppression window instead of letting the noise through. On
the reference host a planned recreate costs **8.7 s** of downtime that stays
silent, while an unexplained `docker stop` still escalates after **15.5 s**.

## What you get

| Entity | Meaning |
|---|---|
| `binary_sensor.z2m_guard_suppress` | `on` while a suppression window is open |
| `sensor.z2m_guard_state` | `idle` · `suspect` · `planned` · `settling` · `outage_confirmed` |
| `sensor.z2m_guard_last_restart` | duration of the last suppressed outage, in seconds |

All three arrive through **MQTT discovery** — there is no integration to install,
and nothing to configure in the Home Assistant UI.

## Quick start

```bash
git clone https://github.com/St0pfen/z2m-guard.git && cd z2m-guard
docker build -t z2m-guard:local .
# broker user + ACL, config.toml, .env, compose service → see Installation
docker compose up -d z2m-guard
```

Then announce restarts instead of triggering them silently:

```bash
Z2M_GUARD_HTTP_TOKEN=… ./deploy/restart-z2m.sh 180
```

To confirm it works without waiting for a real incident, follow the test plan in
[Troubleshooting](Troubleshooting#does-it-actually-work-test-plan).

## In the wiki

- **[Installation](Installation)** — compose, static binary + systemd, HAOS app, broker ACL, entities, upgrades
- **[How it decides](How-it-decides)** — the state machine, the deadlines, and why suppression can never hide a real outage
- **[Proxy mode](Proxy-Mode)** — the optional MQTT relay: what it does, and the measured reason it is not enabled anywhere
- **[Configuration](Configuration)** — every key, its default, and the environment overrides
- **[Home Assistant](Home-Assistant)** — the three entities, plus automations that stay quiet during planned restarts
- **[Monitoring](Monitoring)** — the seven metric series, PromQL and alert rules
- **[Troubleshooting](Troubleshooting)** — symptom → cause → fix, and a test plan
- **[FAQ](FAQ)** — HACS, TLS, Docker, several Zigbee stacks
- **[Development](Development)** — workspace layout, gates, release pipeline, roadmap

## Project

- Source: <https://github.com/St0pfen/z2m-guard>
- Releases: <https://github.com/St0pfen/z2m-guard/releases>
- Image: `ghcr.io/st0pfen/z2m-guard:0.1.2`
- [Architecture](https://github.com/St0pfen/z2m-guard/blob/main/docs/ARCHITECTURE.md) ·
  [Security](https://github.com/St0pfen/z2m-guard/blob/main/SECURITY.md) ·
  [Changelog](https://github.com/St0pfen/z2m-guard/blob/main/CHANGELOG.md)

## Licence

MIT OR Apache-2.0, at your option.