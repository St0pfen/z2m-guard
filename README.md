# z2m-guard

Tells a **planned** Zigbee2MQTT restart apart from a **real** Zigbee outage — and
publishes that verdict where your alarm chain can read it.

> Status: M4 — running in production on a Docker-based Home Assistant stack; static
> musl artifacts, SBOM and a multi-arch image ship with every tag.

## The problem

Every `docker compose up -d` recreates the Zigbee2MQTT container. For ~9 seconds
(measured: 8.7 s) every Zigbee entity goes `unavailable`, because all of them share
one availability topic:

```json
"availability": [{ "topic": "zigbee2mqtt/bridge/state",
                   "value_template": "{{ value_json.state }}" }]
```

Any alarm rule, automation or bridge that watches those entities fires — for a
routine container update. Worse, Z2M republishes its whole discovery tree on
startup, which on the reference host produced **179 messages / 4.49M tokens** in a
single weekly run.

## The idea

A small daemon subscribes to `zigbee2mqtt/bridge/state`, correlates the flap with a
pre-restart signal or container events, and publishes:

- `binary_sensor.z2m_guard_suppress` — `on` while the window is attributed
- `sensor.z2m_guard_state` — `idle | suspect | planned | settling | outage_confirmed`
- `sensor.z2m_guard_last_restart` — duration of the last suppressed downtime
- Prometheus metrics (`z2m_guard_suppressed_events_total{reason}` …)
- JSON-Lines events on stdout

## How it decides

| Situation | Verdict |
|---|---|
| pre-restart signal, then bridge goes away | **planned**, suppressed |
| container exit code 0 within 30 s of the flap | **planned**, suppressed |
| container exit code ≠ 0 or no attribution | **suspect** → 15 s grace |
| still gone after grace | **outage_confirmed**, suppression OFF, alarm passes |
| bridge back | **settling** for 20 s, then idle |

**Safety property:** suppression is never unbounded. Every suppression is attached
to a deadline, and an outage nobody asked for escalates after 15 s instead of being
swallowed. Proven by property tests over random input sequences.

## Deploy

Step-by-step guide: **`docs/INSTALL.md`** — compose variant, systemd variant, HA
entities, ACL, monitoring, troubleshooting, and why HACS/the Marketplace cannot
install this.

```bash
docker build -t z2m-guard:local .
# service definition: deploy/docker-compose.snippet.yml
# config:             config/example.toml
# HA package:         deploy/homeassistant/z2m_guard.yaml
# Prometheus:         deploy/prometheus/scrape-snippet.yml
```

Announce a planned restart so it is attributed:

```bash
Z2M_GUARD_HTTP_TOKEN=… ./deploy/restart-z2m.sh 180
```

That posts an intent, then runs `docker compose up -d --force-recreate` for Z2M.

## Measured on the reference host

- planned recreate → `suppressed_events_total{reason="prior_signal"} = 1`,
  `outage_confirmed_total = 0`, downtime 8727 ms
- unexplained `docker stop` → `outage_confirmed_total = 1` after 15.5 s,
  suppression released, alarm passes

## Limitations

- Docker attribution needs either the pre-restart script (recommended, no
  privileges) or events piped on stdin (`--features stdin-events`). Reading the
  Docker socket directly is deliberately not implemented in v0.1: it is
  root-equivalent host access.
- The MQTT connection is **plain TCP**. `mqtts://` is rejected, and since 0.1.2
  the binary links no TLS stack at all — the one it used to carry was an EOL
  `rustls-webpki` branch with open advisories (see `SECURITY.md`). Keep the broker
  on a private network as the reference stack does.
- The guard observes; it does not restart anything.
- Discovery republish storms are not reduced in v1 — that is the optional proxy
  mode (M5).
- Runs under a dedicated Mosquitto user with a least-privilege ACL
  (`deploy/mosquitto/acl.example`); it does not use the shared service credential.

## Build & release

```bash
cargo build --release                 # needs a C toolchain (rustls/ring)
./deploy/build-release.sh             # static musl binary + SHA256SUMS + CycloneDX SBOM
docker buildx build --platform linux/amd64,linux/arm64 -t z2m-guard:0.1.2 .
```

Full checklist: `docs/RELEASE.md`. Internals and the decisions behind them:
`docs/ARCHITECTURE.md`. Security model: `SECURITY.md`.

## Status / roadmap

- [x] M0 — workspace, licences, CI
- [x] M1 — core state machine (pure, 20 tests incl. 3 properties)
- [x] M2 — MQTT + Home Assistant discovery
- [x] M3 — attribution (pre-restart signal, docker events parser) + deployment
- [x] M4 — static musl artifacts, SBOM, multi-arch image, docs, release pipeline
- [ ] M5 — optional proxy mode

## Contributing

Issues use templates (bug reports ask for version, Z2M version and MQTT protocol —
those three answer most questions). Pull requests are expected to state what they
actually ran: `cargo fmt --check`, `clippy -D warnings`, `cargo test --workspace
--all-features`, and for I/O changes a run against a real broker.

Rules that are not negotiable: the guard writes only to `z2m_guard/#` and its own
discovery configs, never to `zigbee2mqtt/#`; it never gets the Docker socket; and
every suppression window stays deadline-bound.

## Licence

Dual-licensed under **MIT OR Apache-2.0**, at your option — the Rust ecosystem
default. Pick MIT if you want the shortest possible terms, Apache-2.0 if you want
the explicit patent grant. Both are GPLv3-compatible, so the tool can be bundled
into a GPL distribution.

- `LICENSE-MIT`, `LICENSE-APACHE` — the full texts
- `THIRD-PARTY.md` — every one of the 143 linked crates with its SPDX identifier;
  all permissive, no copyleft. Generated by `scripts/license-notices.py`
- release artifacts ship `THIRD-PARTY-LICENSES.txt` (verbatim texts) so the
  attribution terms travel with the binary

Contributions are accepted under the same dual licence (Apache-2.0 §5); no CLA.

## Wiki

Wiki sources live in `wiki/` (`Home.md`, `_Sidebar.md`, `_Footer.md`); the
installation page is generated from `docs/INSTALL.md` at publish time, so it cannot
drift from the reviewed file. Publish with:

```bash
./scripts/publish-wiki.sh
```

The script is honest about the one hard blocker: GitHub does not offer wikis on
**private** repositories unless the account has Pro/Team/Enterprise. On a private
repo the API accepts `{"has_wiki": true}` with HTTP 200 and silently keeps the flag
false, and `<repo>.wiki.git` does not exist — the script checks both and exits with
that reason instead of failing on a confusing clone error.

Everything else that only works once the repo is public — visibility switch, wiki,
GHCR package visibility and link, anonymous-pull proof, README sweep, announcement
order — is ordered in **`docs/PUBLISH.md`**.

## CI note

Two workflows live in `.github/workflows/`: `ci.yml` (fmt + clippy + tests on
every push and PR, plus a job that builds the locked tree on the declared MSRV
1.85) and `release.yml` (tag push → musl binaries for amd64/arm64, CycloneDX
SBOM, licence notices, multi-arch image pushed to GHCR, draft release with every
asset attached).

Creating those files needs a token with the `workflow` scope. An OAuth token
without it gets a bare HTTP 404 from the contents API and a push rejection —
GitHub hides the real reason, so don't read the 404 as "path wrong".
