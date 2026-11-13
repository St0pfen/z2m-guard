# z2m-guard

Separates **planned** Zigbee2MQTT restarts from **real** Zigbee outages.

Every Zigbee entity in Home Assistant shares one availability topic,
`zigbee2mqtt/bridge/state`. A routine `docker compose up -d` recreates the
container, that topic goes `offline` for a few seconds, and all ~100 entities turn
`unavailable` — every watchdog, every notification automation, every dashboard
fires at once. The values were never wrong; the alarm is.

z2m-guard watches that one topic, knows whether a restart was announced, and
publishes a bounded suppression window instead of letting the noise through. On the
reference host a planned recreate costs **8.7 s** of downtime that stays silent,
while an unexplained `docker stop` still escalates after **15.5 s**.

## What you get

| Entity | Meaning |
|---|---|
| `binary_sensor.z2m_guard_suppress` | `on` while a suppression window is open |
| `sensor.z2m_guard_state` | `idle` · `suspect` · `planned` · `settling` · `outage_confirmed` |
| `sensor.z2m_guard_last_restart` | duration of the last suppressed outage, in seconds |

All three arrive through **MQTT discovery** — there is no integration to install.

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

## Documentation

- **[Installation](Installation)** — the full guide: compose, systemd, HAOS app, entities, ACL, monitoring, troubleshooting
- [Architecture](https://github.com/St0pfen/z2m-guard/blob/main/docs/ARCHITECTURE.md) — the decisions and the state machine
- [Security](https://github.com/St0pfen/z2m-guard/blob/main/SECURITY.md) — threat model, least-privilege MQTT, fail-safe behaviour
- [Release](https://github.com/St0pfen/z2m-guard/blob/main/docs/RELEASE.md) — artifacts, SBOM, multi-arch builds
- [Changelog](https://github.com/St0pfen/z2m-guard/blob/main/CHANGELOG.md)

## Not installable via HACS

HACS and the Home Assistant Marketplace install in-process content — custom
integrations, cards, themes, templates. z2m-guard is a separate service that must
outlive Home Assistant restarts, and its entities already come from MQTT discovery.
The wiki page [Installation](Installation#why-not-hacs--the-marketplace) explains the
distribution ladder: compose, static binary + systemd, or a Home Assistant app
repository for HAOS/Supervised.

## Licence

MIT OR Apache-2.0, at your option.