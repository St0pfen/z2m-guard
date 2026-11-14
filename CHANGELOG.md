# Changelog

## 0.1.1 — 2026-11-14

### Fixed
- **Suppression window never expired while the MQTT broker was unreachable**
  (issue #5). The ticker and the HTTP command receiver lived inside the
  per-connection `select!`, so every refused connection tore that loop down
  before the tick branch could fire: `Guard::expire()` never ran and an intent
  accepted during an outage suppressed indefinitely. Ticker and receiver now
  live outside the connection loop and the reconnect pause serves both, so
  windows expire on wall-clock time; publishes are skipped while disconnected
  and republished by `publish_connected` after the reconnect. Regression test
  `suppression_expires_while_the_broker_is_unreachable` runs the real runtime
  against a closed port and was confirmed to fail on the previous code.

### Dependencies (four majors, each gated on the full suite — 48 tests afterwards)
- `axum` 0.7.9 → 0.8.9 — drop-in: no dynamic path segments, every handler is a
  plain `async fn`; the router status matrix (202/400/401/408/413/503) passes.
- `toml` 0.8.23 → 1.2.1 (+`spec-1.1.0`) — drop-in: one `toml::from_str` call site,
  the config tests (defaults, partial file, missing file) are unchanged.
- `rumqttc` 0.24.0 → 0.25.1 — drop-in: the `MqttOptions`/`Event` surface used here
  did not move; the broker integration test still passes against a live broker.
- `tower-http` 0.5.2 → 0.7.1 — `TimeoutLayer::new` is deprecated since 0.6.7;
  replaced with `TimeoutLayer::with_status_code(REQUEST_TIMEOUT, ..)` and now
  covered by a test (`a_hung_request_answers_408_not_500`), because the 5 s cap
  was the one status code in the matrix nobody asserted.
- MSRV unchanged: the bumped lock still builds on the declared 1.85
  (`cargo +1.85.0 check --workspace --all-features --locked`).

### Repository
- CI is live: `.github/workflows/ci.yml` runs fmt, clippy and the full suite on
  every push/PR and builds the locked tree on the declared MSRV 1.85, so
  `rust-version` is enforced instead of asserted.
- `.github/workflows/release.yml` gained the missing image job — until now the
  README claim "a multi-arch image ships with every tag" was only true for a
  hand-run `buildx` on one host; the tag pipeline now pushes
  `ghcr.io/st0pfen/z2m-guard` for `linux/amd64` and `linux/arm64`.
- **Docker image was unstartable on both architectures.** The build stage used
  `rust:1.100-slim`, whose base moved from bookworm to trixie upstream, so the
  binary linked `GLIBC_2.38` while the distroless `cc-debian12` runtime ships
  glibc 2.36 — `docker build` succeeded, `docker run --version` died with
  `GLIBC_2.38 not found`. Stage 1 is pinned to `rust:1.100-slim-bookworm`; CI now
  builds the image and runs it (`ci.yml` job `image`), and the release job
  smoke-tests the pushed image before the release is cut.
- `tests/golden/*.json` reblessed for `sw_version` 0.1.1; the diff is exactly six
  lines, all `"sw_version"` — no other field of the discovery contract moved.
- per-crate `*.cdx.json` untracked and ignored: `cargo-cyclonedx` writes them next
  to the manifest and `build-release.sh` moves them into `dist/`, so the committed
  copies were stale build intermediates. The SBOM of record is
  `dist/z2m-guard-<version>.cdx.json`.
- gates on this tree: `cargo fmt --check` clean, `clippy --all-targets
  --all-features -D warnings` clean, `cargo test --workspace --all-features`
  **49 passed, 0 failed**.

### Verified on the reference host
- musl artifact: `6,609,856 B`, `static-pie linked`, stripped, sha256
  `f1dd0ada4d48…6944`; runs on alpine, `--version` prints `z2m-guard 0.1.1`;
  `SHA256SUMS` verifies all five entries; CycloneDX 1.5 SBOM with 127 components.
- multi-arch archive `dist/z2m-guard-0.1.1.oci.tar`, manifest list
  `sha256:26c1f7a3a02c…2686`; extracted binaries 6,419,992 B (amd64) and
  5,118,680 B (arm64), both **max `GLIBC_2.34`** — down from the 2.38 of the
  broken build — and both print `z2m-guard 0.1.1` (arm64 under qemu, inside
  `distroless/cc-debian12`).
- GitHub Actions: run `43547692132` on the release commit — `test`, `msrv` and
  `image` all green; the `image` job builds the container **and executes** it.

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

### Repository & hardening
- `.github/`: issue templates (bug report with the three fields that answer most
  questions, feature request, config with a private-security link), PR template,
  Dependabot (cargo weekly in one grouped PR; actions and docker monthly)
- `.gitattributes`: LF normalisation for shell scripts and marked generated files
- `[profile.release]`: `strip = "symbols"`, thin LTO, one codegen unit —
  musl artifact **6,354,320 → 4,233,856 B** (33 %), `file` now reports "stripped"
- HTTP: 4 KiB body limit and a 5 s timeout. A malformed body used to return 202
  with a default 120 s window, because `Option<Json<_>>` turns extractor
  rejections into `None`; it is now parsed explicitly → 400, oversized → 413.
  Five router tests cover 202/400/401/413/503.
- The image no longer compiles `stdin-events`: a container cannot be fed a live
  `docker events` stream, so the feature advertised a capability nothing could
  use. `--docker-events-stdin` now fails before binding the HTTP port (it used to
  report "Address already in use"); host/systemd wiring is documented.
- MSRV corrected: `rust-version` says 1.85, verified (1.75 fails on
  `edition2024` dependencies, 1.85 and 1.88 build the locked tree)
- Tests: 42 → **47**

### Verified on the reference host
- remote builder over SSH documented and measured: 707 s wall time on a 24-thread WSL2 guest, binaries byte-identical to the local build (`amd64` `45110296…`, `arm64` `9015c100…`) even though the archive `index` digest differs (BuildKit 0.30 emits attestation manifests)
- multi-arch OCI archive: `index digest sha256:57b0bd05944f7f`, amd64 and arm64 binaries both report `z2m-guard 0.1.0` (arm64 under qemu)
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
