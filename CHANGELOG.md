# Changelog

## 0.1.0 — 2026-11-13

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
- Release tooling: static musl build via `deploy/build-release.sh` (runs unprivileged
  in a builder image), CycloneDX 1.5 SBOM, multi-arch image (`linux/amd64`,
  `linux/arm64`), tag-triggered release workflow with `SHA256SUMS`.
- Docs: `docs/ARCHITECTURE.md` (decisions), `docs/RELEASE.md` (checklist),
  `SECURITY.md` (threat model), `docs/ANNOUNCEMENT.md` (draft).
- Docs: `docs/INSTALL.md` — full installation guide (compose, systemd, HA app,
  entities, ACL, monitoring, troubleshooting) plus `wiki/` sources and
  `scripts/publish-wiki.sh`.

### Licensing
- dual `MIT OR Apache-2.0` with both texts in the repo; README states the choice,
  the GPLv3 compatibility and the no-CLA contribution rule
- `THIRD-PARTY.md` + `scripts/license-notices.py`: the shipped binary links 143
  third-party crates, all permissive (MIT/Apache-2.0/BSD-3-Clause/ISC/Unlicense/
  Unicode-3.0); every licence text is reproduced in `THIRD-PARTY-LICENSES.txt`
- release artifacts carry `LICENSE-MIT`, `LICENSE-APACHE` and the notices file,
  all covered by `SHA256SUMS` — binary redistribution keeps the attribution terms

### Verified on the reference host
- planned recreate: one suppressed window, 8727 ms downtime, no escalation
- unexplained `docker stop`: escalation after 15.5 s, suppression released
- broker restart inside a planned window: 1048 ms of downtime attributed, no escalation
- Mosquitto ACL: dedicated user `z2mguard` cannot publish to `zigbee2mqtt/*` or a
  foreign discovery topic (second-client listener received nothing) and cannot
  subscribe to device topics; its own two discovery domains still publish
- release artifacts: `static-pie linked` musl binary runs on alpine (`--version`,
  `--print-discovery` produce output), `SHA256SUMS` verifies, CycloneDX 1.5 SBOM
  with 118 components
- multi-arch image: `linux/amd64` and `linux/arm64` manifests present in the OCI
  archive; the arm64 layer carries an aarch64 ELF that executes under emulation

### Not done
- Direct Docker socket access (deliberate: root-equivalent privilege)
- Discovery-republish deduplication (planned for the optional proxy mode)
