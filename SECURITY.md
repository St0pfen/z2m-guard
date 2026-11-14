# Security

## Threat model

z2m-guard reads one retained MQTT topic and publishes to its own namespace. It has
no network listener beyond an optional HTTP port for metrics and the
restart-intent endpoint, and it writes nothing outside `z2m_guard/#` and the
Home Assistant discovery prefix.

Deliberate non-goals: the guard cannot restart, stop or reconfigure Zigbee2MQTT.

## Trust boundaries

- **MQTT credentials** — the guard runs under a dedicated user `z2mguard` with an
  ACL that grants exactly: read `zigbee2mqtt/bridge/state`, readwrite `z2m_guard/#`
  and write access to its own two discovery domains. Everything else — device
  topics, bridge commands, foreign discovery, `$SYS` — is denied. Rollout and the
  acceptance checks are in `deploy/mosquitto/README.md`; the reference host runs
  exactly this ACL.
- **HTTP endpoint** — `POST /v1/restart-intent` requires a shared token
  (`Z2M_GUARD_HTTP_TOKEN`, compared in constant time). An accepted intent only
  opens a bounded suppression window; it cannot trigger any action.
- **Docker socket** — *not* mounted. The `docker-socket` feature exists but is
  unimplemented in v0.1 precisely because the socket is root-equivalent host
  access. Attribution uses the pre-restart script or stdin events instead.
- **MQTT transport is plain TCP, on purpose.** `mqtts://` URLs are rejected by
  `parse_broker_url`, and since 0.1.2 the binary no longer links a TLS stack at
  all: rumqttc's `use-rustls` feature pulled `rustls-webpki` 0.102.8 (eight
  advisories filed 2026, fixed only on the 0.103 branch) and the unmaintained
  `rustls-pemfile` 2.2.0 into the artifact while nothing could reach them. Keep
  the broker on a private Docker network (as the reference host does) instead of
  exposing it; if TLS becomes a requirement, it needs an upstream rumqttc release
  on webpki 0.103, not a local patch of the dead branch.
- **Container** — distroless base, read-only root filesystem, no published host
  ports, Watchtower disabled (no unsigned image pulls).

## Failure modes worth knowing

- If the guard dies, nothing is suppressed and alarms behave exactly as before.
  Fail-safe, not fail-open.
- If the guard's window is open when a *real* outage starts, the alarm is delayed
  by at most that window (max 180 s with the shipped config). This is the price of
  suppressing planned restarts; the pre-restart signal exists so the window is
  short and intentional.

## Reporting

Open a private security advisory on the repository. Please include the version
(`z2m-guard --version`), the config section involved, and whether the Docker
socket or the proxy feature was enabled.
