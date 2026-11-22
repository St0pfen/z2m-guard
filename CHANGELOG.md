# Changelog

## Unreleased

### Added
- Startup config validation (`Config::validate`) runs after loading and before
  anything binds: empty `mqtt.url`/`mqtt.client_id`, a `mqtt.base_topic` that is
  not a topic root (wildcards, leading/trailing/doubled `/`) and, when
  `proxy.enabled = true`, an empty or malformed `proxy.allow` are now named
  startup errors instead of failures at a distance. `guard_proxy::allow::is_valid_filter`
  checks the MQTT filter grammar, so a typo like `zigbee2mqtt#` — which the
  matcher treats as one literal level and therefore matches nothing — is refused
  up front rather than surfacing as "Z2M silently stopped working".

### Fixed
- `/metrics` reported `z2m_guard_bridge_online_timestamp_seconds` in **milliseconds**
  despite the `_seconds` suffix, and never moved on an offline transition — so no query
  could tell whether the bridge was up. Split into `z2m_guard_bridge_online` (gauge,
  `1`/`0`, follows every bridge-state message) and `z2m_guard_bridge_last_online_seconds`
  (seconds since guard start at the last online transition). An `Z2mGuardBridgeOffline`
  alert rule now has a series that actually expresses the condition.

### M5 — proxy mode (implemented, measured, **premise disproved**)
- The mode was built to dedupe discovery republish waves from Zigbee2MQTT. Measured on
  the reference host, that wave does not come from Z2M: a real Z2M restart produced **1**
  discovery publish, while the 106 retained `homeassistant/#` configs (97 132 bytes) are
  delivered to every *consumer* that subscribes — i.e. Home Assistant on restart. A
  proxy on the Z2M leg cannot see that traffic. **Not switched on in production**, because
  it would add a single point of failure and a widened broker ACL for no gain.
- Still useful as a tool: allowlist (fail closed), will preservation, protocol-level
  fidelity, dedupe of repeated publishes when a script republishes.
- Details, numbers and the open items: `docs/PROXY.md`.
- `guard-proxy` is no longer a stub: a feature-gated MQTT relay that sits between
  Zigbee2MQTT and the broker, preserves the client's protocol level and **will**,
  enforces a topic allowlist (fail closed) and drops duplicate **retained** discovery
  publishes inside a window, acknowledging QoS 1 itself so Z2M cannot retransmit them.
- 18 unit and end-to-end tests against an in-process fake broker; two real defects
  were found by them — a PUBACK written to the wrong socket (the client would block
  forever) and an allowlist test that asserted the opposite of the intended coverage
  semantics.
- `[proxy]` config section, `--features proxy` build gate, and a startup refusal when
  `proxy.enabled = true` meets a binary without the feature.
- The `Dockerfile` gained `ARG FEATURES=""`. Before that the shipped image was compiled
  without the feature, so proxy mode was unreachable in a container by construction — a
  gap found only by building on the remote builder. No proxy image is published: build
  one with `--build-arg FEATURES=proxy`.
- CI now checks **both** sides of the gate: the plain image must refuse
  `proxy.enabled = true`, the proxy image must open the listener.
- **Measurement against the real stack** (broker log, last 7 days): Zigbee2MQTT speaks
  MQTT 3.1.1, Home Assistant negotiates both 3.1.1 and 5.0. That immediately exposed
  two MQTT 5 defects in the codec — a v5 client's will was parsed from the wrong offset
  (the will property block was ignored) and the re-issued v5 CONNECT carried no will
  property block at all. Both fixed, covered by tests, and verified live: a v5 client's
  will reached the broker when the client was killed.
- Throughput 10 296 msg/s through the proxy vs 11 212 direct (~8 % cost); the synthetic
  discovery storm dropped 180 of 200 duplicate retained publishes with zero duplicates
  at the broker; a `SIGKILL`ed proxy closes the socket immediately and the client
  reconnects on its own.
- Deliberately **not** released: Z2M's own reconnect behaviour through the proxy, a
  load test with a real Z2M restart (the actual storm shape) and arm64 throughput are
  still open. See `docs/PROXY.md`.

## 0.1.2 — 2026-11-14

### Security
- **Removed an EOL TLS stack from the artifact.** rumqttc's default feature
  `use-rustls` pulled `rustls-webpki` 0.102.8 into the binary 0.1.1 shipped. An
  OSV scan of that release's SBOM (127 components) returned ten findings, all in
  two crates: eight advisories against webpki 0.102.8 — including
  `GHSA-82j2-j2ch-gfr8` **HIGH** (DoS via panic on a malformed CRL BIT STRING) and
  `GHSA-pwjx-qhcg-rvj4` **MODERATE** (CRLs not treated as authoritative) — plus
  `RUSTSEC-2025-0134`, the unmaintained `rustls-pemfile` 2.2.0. Every webpki fix
  landed on 0.103.x only; rumqttc 0.25.1 (current latest) pins `^0.102.8`.
- The reachable surface was nil — `parse_broker_url` rejects `mqtts://` by design,
  so the guard could never have used that stack — but it was compiled, linked and
  listed in the SBOM. `rumqttc` now builds with `default-features = false`; plain
  TCP to the broker is the supported transport, documented in `SECURITY.md`.
- CI runs `cargo audit` on every push, so the next advisory shows up as a red
  build instead of in a release SBOM.

### Repository
- golden discovery files reblessed for `sw_version` 0.1.2 (six lines, version only).

### Verified on the reference host
- binary `z2m-guard-0.1.2-x86_64-unknown-linux-musl`: **2,988,680 B** (0.1.1:
  6,609,856 B — the TLS stack was the largest single block), static-pie, stripped,
  sha256 `84f35aa5772e…9756`, downloaded from the published release, hash matched
  against its `.sha256` sidecar, runs and prints `z2m-guard 0.1.2`.
- published SBOM: **104 components** (0.1.1: 127), no `rustls*` entry, and an
  osv.dev querybatch over all 104 `(name, version)` pairs returned **0 findings**
  (0.1.1: 10).
- image `ghcr.io/st0pfen/z2m-guard:0.1.2` (`:0.1`, `:latest`), index digest
  `sha256:51cd4a08560a…c285`; the `0.1.1` tag still points at its own digest.
- reference host: container recreated on 0.1.2, `/healthz` ok, 10 metric series,
  MQTT connected, retained discovery reads `sw_version 0.1.2`.
- CI run `43591084924` green on `test`, `audit`, `msrv`, `image`; tag run
  `43591094537` produced all four jobs green and attached 8 assets.

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
- Hygiene before the switch to public: merged branches deleted (6 local, 3
  remote — each verified with `git cherry main <branch>`, all `-`, i.e. the patch
  is already upstream), `delete_branch_on_merge` enabled so squash-merged
  Dependabot branches disappear by themselves, and the duplicate 21 MB
  `dist/*-wsl.oci.tar` plus the superseded 0.1.0 archive removed.
- `docs/PUBLISH.md` (removed in 0.1.2 once executed): the go-public checklist with
  the two traps measured on this
  repo — the wiki flag that GitHub accepts and ignores on a private repo, and
  `GET /user/packages/container/…` returning 403 / `PATCH …/visibility` 404
  because the token has no `read:packages`/`write:packages`.
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
- release automation learned two things from its own first run: artifacts are
  downloaded by pattern (`bin-*`, `sbom`) instead of "everything", because a
  failed blob download of setup-buildx's BuildKit artifact aborted the release
  step after five retries; and the sbom artifact's `dist/` tree is flattened
  before the upload, otherwise `files: artifacts/*` silently skips
  `THIRD-PARTY-LICENSES.txt`, `LICENSE-MIT` and `LICENSE-APACHE`.
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
