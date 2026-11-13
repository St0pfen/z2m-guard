# Draft announcement (not posted — repository is still private)

## For r/homeassistant and the Zigbee2MQTT discussions

**Stop waking up for planned Zigbee2MQTT restarts**

Every `docker compose up -d` recreates the Zigbee2MQTT container. Because all
entities share one availability topic (`zigbee2mqtt/bridge/state`), every single
one of them goes `unavailable` for about nine seconds — measured 8.7 s on my
stack. Watchdogs, automations and bridges fire for a routine update.

`z2m-guard` subscribes to that one topic, correlates the dip with a pre-restart
signal (or a container exit code), and publishes the verdict as:

- `binary_sensor.z2m_guard_suppress` — `on` while the restart is attributed
- `sensor.z2m_guard_state` — `idle | suspect | planned | settling | outage_confirmed`
- `sensor.z2m_guard_last_restart` — how long the suppressed downtime lasted
- Prometheus metrics, so a Grafana panel shows "planned restarts vs. real outages"

The rule it never breaks: **suppression is always bounded**. An unattributed
outage escalates after 15 s and alarms exactly as before, so it cannot hide a
real Zigbee failure.

It observes only — it cannot delay or rewrite Zigbee traffic, does not need the
Docker socket, and if it dies your alarms behave exactly as they did before.

Single static binary, distroless container, no host ports.
[x86_64 + arm64, SHA256SUMS + CycloneDX SBOM]

Feedback I am explicitly looking for: which of your automations would you point at
the marker, and whether a 15 s grace is the right default for a Zigbee mesh.

## HN title candidate

`z2m-guard: a 9-second container restart should not raise a home automation alarm`

## What to link

- repository (when it goes public)
- `docs/ARCHITECTURE.md` — why observer instead of proxy
- a Grafana screenshot of the "planned vs. confirmed" panel
