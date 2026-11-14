# FAQ

**Why isn't this a HACS integration or a custom component?**
Because it has to outlive Home Assistant. A suppression window that dies with an HA
restart is worthless exactly when you need it — during an HA upgrade, which is
usually the moment you also restart Zigbee2MQTT. The guard is a separate process
with its own broker credentials, and the entities arrive through MQTT discovery, so
nothing is installed *inside* HA.

**Does it restart Zigbee2MQTT?**
No. It subscribes to one topic, publishes to `z2m_guard/#` and its own discovery
domains, and exposes a small HTTP endpoint. It cannot restart, stop or reconfigure
anything — see [Security](https://github.com/St0pfen/z2m-guard/blob/main/SECURITY.md).

**Do I need Docker?**
No. The static musl binary plus systemd is a first-class path
([Installation](Installation#variant-b--static-binary--systemd)). Docker is needed
only for the compose variant and for the optional attribution via container events —
the pre-restart script works without it.

**Do I need Home Assistant?**
No. The guard needs an MQTT broker. With `discovery = false` it publishes no
entities, and you can drive everything from the JSON-Lines event log and the
Prometheus metrics alone.

**Does it support TLS (`mqtts://`)?**
No, deliberately. `mqtts://` is rejected at startup and the binary links no TLS
stack at all — the one it used to carry was an end-of-life `rustls-webpki` branch
with open advisories. Keep the broker on a private Docker network. Rationale and the
removed dependency chain are in
[SECURITY.md](https://github.com/St0pfen/z2m-guard/blob/main/SECURITY.md).

**Two Zigbee stacks (e.g. one per floor) — can I run two guards?**
Not against the same Home Assistant yet: the entity ids are fixed
(`sensor.z2m_guard_state`, …), so a second instance would collide with the first.
Workarounds today: run the second one with `discovery = false` and monitor it
through metrics/logs only, or give it its own HA instance. A configurable entity
namespace is on the roadmap, not in 0.1.x.

**Why does a *planned* restart still show ~9 s of downtime?**
Because the downtime is real — the container is genuinely down for those seconds.
The guard does not pretend otherwise; it records the duration in
`last_restart` and suppresses the *alarm* during that time.

**What if a restart takes longer than the announced window?**
The window expires, the state becomes `outage_confirmed`, and the alarm passes. A
slow restart is a real outage from the alarm chain's point of view — which is the
safe direction. Announce a longer window (`deploy/restart-z2m.sh 300`) if your stack
needs it.

**Does suppression survive a guard restart?**
No. State is in memory only. After a restart the guard re-reads the retained bridge
state and starts from `idle`/`suspect`. Nothing to migrate, nothing to clean up.

**Does it send anything anywhere?**
It talks to your MQTT broker and nothing else. No telemetry, no update check, no
outbound calls. The HTTP endpoint is a listener, not a client.

**Why is there no state file?**
Because a stale state file is worse than no state: after a long outage the guard
would resume believing in a window that no announcement backs. A deadline that
exists only in memory cannot outlive its evidence.

**Can I build it myself?**
Yes — Rust 1.85 or newer, `cargo build --release` (no C toolchain needed since 0.1.2).
Building is what the release pipeline does; see [Development](Development).

**Which version should I pin?**
Tags are immutable and the image tags are per-version; `:latest` moves. For a
homelab, pin the full version (`ghcr.io/st0pfen/z2m-guard:0.1.2`) and let
`/metrics` + releases tell you when to move.