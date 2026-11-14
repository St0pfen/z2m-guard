# Development

## Workspace

Six crates, ~2.900 lines of Rust, 49 tests:

| Crate | Role |
|---|---|
| `guard-core` | I/O-free state machine — the only place that decides `planned` vs `outage_confirmed`, covered by unit + property tests |
| `guard-mqtt` | MQTT transport: subscribe, publish, reconnect with capped backoff |
| `guard-ha` | Home Assistant discovery payloads (the JSON in `tests/golden/`) |
| `guard-docker` | Docker event attribution, opt-in (`--features stdin-events`) |
| `guard-proxy` | placeholder for the optional proxy mode (M5) — currently a doc comment, nothing links it into the runtime |
| `z2m-guard` | binary: config, wiring, HTTP, metrics |

The split exists so the decision logic can be tested without a broker, a container,
or a network. If a change to the state machine needs an integration test to be
believable, the change is in the wrong crate.

## Gates

Every push runs all of these; a PR is expected to have run them locally:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
cargo audit --deny warnings
```

Plus the MSRV build (`cargo +1.85.0 check --workspace --all-features --locked`) and
a job that **builds the image and runs it** — a green `docker build` proved nothing
once already (the build stage moved to trixie, the binary demanded `GLIBC_2.38`, the
distroless runtime ships 2.36, and the container died on startup on both
architectures).

## Tests worth knowing

- `guard-core` property tests: no sequence of events produces a suppression window
  that outlives its deadline. This is the safety property from
  [How it decides](How-it-decides), and it is the reason the state machine is I/O-free.
- `guard-ha/tests/golden/*.json` — the three discovery payloads, byte-exact. They
  carry `sw_version`, so a version bump re-blesses six lines:
  `cargo run -p guard-ha --example print_discovery`.
- Runtime tests with a stub broker: a window closes even when the broker is
  unreachable (the `tick_ms` housekeeping path).

## Release

```bash
# version bump: Cargo.toml [workspace.package], golden files, docs, CHANGELOG
./deploy/build-release.sh        # static musl binary, SHA256SUMS, CycloneDX SBOM, licence texts
git tag -a v0.1.x -m "z2m-guard 0.1.x" && git push origin v0.1.x
```

The tag pipeline builds amd64 + arm64 musl, an SBOM, a multi-arch image pushed to
GHCR, smoke-tests the image by running `--version`, and opens a release with every
asset attached. Full checklist, including the two traps that hit it in practice (a
stale buildx cache, and `download-artifact`'s top-level glob), is in
[docs/RELEASE.md](https://github.com/St0pfen/z2m-guard/blob/main/docs/RELEASE.md).

**Scan the SBOM before calling a release done.** 0.1.1 shipped an end-of-life TLS
stack that no code path could reach — and that a `cargo audit` on the lockfile
would have missed if the feature flag differed. The published SBOM is the artifact
that matters: POST the 104 `(name, version)` pairs to
`https://api.osv.dev/v1/querybatch` and expect zero findings. 0.1.2 does.

## Wiki workflow

Wiki sources live in `wiki/` in the repository and are published by script — the
GitHub wiki is a git repository, not an API resource:

```bash
# edit wiki/*.md, then
./scripts/publish-wiki.sh
```

`Installation.md` is **generated** from `docs/INSTALL.md` at publish time, so the
installation page cannot drift from the reviewed file. If you want to change it,
change `docs/INSTALL.md`.

## Roadmap

- [x] **M0** — workspace, licences, CI
- [x] **M1** — core state machine (pure, incl. property tests)
- [x] **M2** — MQTT + Home Assistant discovery
- [x] **M3** — attribution (pre-restart signal, docker events parser) + deployment
- [x] **M4** — static musl artifacts, SBOM, multi-arch image, docs, release pipeline
- [ ] **M5** — optional proxy mode: sit between Z2M and the broker and drop duplicate
      discovery payloads (`z2m_guard_discovery_deduped_total` is already part of the
      metric contract, permanently 0 until this exists)

Named but not scheduled: a configurable entity namespace (see the two-stack answer
in [FAQ](FAQ)), and an optional TLS build feature — the latter blocked on an upstream
rumqttc release that does not pull a dead `rustls-webpki` branch.

## Contributing

Issues use templates; the bug template asks for the guard version, the Z2M version
and the MQTT protocol because those three answer most reports. A PR should state what
it actually ran — the four gates above, and for I/O changes a run against a real
broker.

Non-negotiable rules: the guard writes only to `z2m_guard/#` and its own discovery
configs, never to `zigbee2mqtt/#`; it never gets the Docker socket; every suppression
window stays deadline-bound.