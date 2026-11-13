# Changelog

## 0.1.0 — unreleased

### Added
- Core state machine (`guard-core`): planned vs. suspect vs. confirmed outage,
  bounded suppression windows, flap detector. 20 unit/property tests.
- MQTT transport (`guard-mqtt`): retained bridge-state handling, broker-URL
  parsing, reconnect backoff, MQTT last-will for availability.
- Home Assistant discovery (`guard-ha`): `binary_sensor.z2m_guard_suppress`,
  `sensor.z2m_guard_state`, `sensor.z2m_guard_last_restart`, golden-file contract.
- Binary: TOML + env config, Prometheus metrics endpoint, `/healthz`,
  `POST /v1/restart-intent` with shared-token auth, JSON-Lines events.
- Docker event parser (`guard-docker`) with fixtures recorded from a real daemon;
  optional stdin source (`--features stdin-events`).
- Deploy artifacts: Dockerfile (distroless), compose snippet, pre-restart script,
  HA package example, Prometheus scrape snippet, Grafana dashboard.

### Verified on the reference host
- planned recreate: one suppressed window, 8727 ms downtime, no escalation
- unexplained `docker stop`: escalation after 15.5 s, suppression released

### Not done
- Direct Docker socket access (deliberate: root-equivalent privilege)
- Discovery-republish deduplication (planned for the optional proxy mode)