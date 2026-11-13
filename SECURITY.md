# Security

## Threat model

z2m-guard reads one retained MQTT topic and publishes to its own namespace. It has
no network listener beyond an optional HTTP port for metrics and the
restart-intent endpoint, and it writes nothing outside `z2m_guard/#` and the
Home Assistant discovery prefix.

Deliberate non-goals: the guard cannot restart, stop or reconfigure Zigbee2MQTT.

## Trust boundaries

- **MQTT credentials** — the guard needs the ability to subscribe to
  `zigbee2mqtt/bridge/state` and publish its own topics. It is deployed with the
  existing service credentials; a dedicated Mosquitto user with an ACL limited to
  those topics is the recommended hardening step (needs a coordinated broker
  restart, so it is a separate change).
- **HTTP endpoint** — `POST /v1/restart-intent` requires a shared token
  (`Z2M_GUARD_HTTP_TOKEN`, compared in constant time). An accepted intent only
  opens a bounded suppression window; it cannot trigger any action.
- **Docker socket** — *not* mounted. The `docker-socket` feature exists but is
  unimplemented in v0.1 precisely because the socket is root-equivalent host
  access. Attribution uses the pre-restart script or stdin events instead.
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
