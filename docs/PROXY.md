# Proxy mode (M5) — implementation notes

Status: **implemented and measured; off by default, deployed nowhere.** The guard does
its normal job without it, and no released artifact enables it. Read
[Does it solve the documented pain?](#does-it-solve-the-documented-pain--no-and-that-was-measured)
before switching it on: the measurement that was meant to validate the mode refuted the
reason it was built.

## Why it was built (hypothesis)

The starting claim was that Z2M republishes its whole discovery tree on every start, and
that those identical, retained payloads reach every subscriber again — on the reference
host quoted as **179 messages / 4.49 M tokens** in one weekly run.

That hypothesis is **wrong for Z2M 2.x**, and the measurement below shows why. The mode
is kept because it is still a correct, tested relay with an allowlist, will preservation
and protocol fidelity — not because it saves tokens.

## Where it sits

```
Zigbee2MQTT ──► guard proxy (listen 0.0.0.0:1884) ──► Mosquitto :1883
Home Assistant ─────────────────────────────────────► Mosquitto :1883
```

Only Z2M is rerouted. Home Assistant keeps talking to the broker directly, so a
proxy failure can never hide the guard's own entities from HA.

## What it does

- Terminates the downstream CONNECT, re-issues it upstream with the **same protocol
  level** (3.1.1 or 5.0) and **the client's will**. Without the will, the broker
  would never publish `offline` for the bridge when Z2M dies — the availability
  topic the whole design rests on would go stale.
- Forwards every other packet **unmodified** in both directions. No re-encoding, so
  MQTT 5 properties and reason codes survive.
- Enforces a topic allowlist on PUBLISH and SUBSCRIBE. A violation closes the
  session (fail closed) instead of quietly forwarding an unauthorised topic.
- Drops a **retained** publish whose topic+payload was already forwarded inside
  `dedupe_window_ms`, and answers QoS 1 **itself** — otherwise Z2M retransmits the
  exact storm the dedupe just removed. Non-retained publishes always pass: a state
  change is not a duplicate even when the value repeats.
- Exposes `z2m_guard_discovery_deduped_total` (the name has been part of the metric
  contract since 0.1.0; it stayed 0 until this mode existed).

## What it is not

- Not a broker: no retained store of its own, no sessions, no `$SYS`.
- No topic rewriting, no per-client isolation — it relays **one** downstream client.
- No TLS, same as the rest of the project.
- Not a high-availability component. If the proxy is down, Z2M cannot connect, and
  that has to be measured, not assumed (see below).

## Enabling it

No proxy image is published — build one yourself:

```bash
docker build --build-arg FEATURES=proxy -t z2m-guard:proxy .
```

The default image is built **without** the feature on purpose: `--build-arg FEATURES=proxy`
is what puts the mode into the binary, and `<version>` keeps meaning the same
feature-less binary it always meant. A binary built without the feature refuses the
config at startup instead of silently ignoring it.

For a host build:

```bash
cargo build --release --features proxy
```

```toml
[proxy]
enabled = true
listen = "0.0.0.0:1884"
# upstream = "mqtt://172.22.1.50:1883"   # default: mqtt.url
dedupe_window_ms = 60000
allow = ["zigbee2mqtt/#"]
```

Then point Z2M at the proxy instead of the broker — the setting lives in Z2M's own
`configuration.yaml` on most installs (`--config` installs read it from there, not from
`MQTT_SERVER`):

```yaml
mqtt:
  server: mqtt://z2m-guard:1884
```

A binary **without** `--features proxy` refuses `proxy.enabled = true` at startup
rather than silently running a guard whose proxy a deployment believes is there.

The proxy connects upstream with the guard's own MQTT user (`mqtt.username`,
`mqtt.password_env`), not with Z2M's — the proxy is the one holding the
least-privilege account.

## Measurements

All numbers below were measured on 2026-11-14 against a throwaway Mosquitto 2 and the
reference host's broker log — not estimated.

**Protocol versions in the real stack** (from `/opt/home-assistant/mosquitto/log`, last
7 days; the log's `p4` = MQTT 3.1.1, `p5` = MQTT 5.0, confirmed empirically by putting a
v5 client through the proxy and reading `p5` in the broker's own log):

| client | address | version |
| --- | --- | --- |
| Zigbee2MQTT (`mqttjs_*`) | 172.22.1.2 | MQTT 3.1.1 (`p4`) |
| Home Assistant (`u'hass'`) | 172.22.1.1 | **both** 3.1.1 and 5.0 |
| z2m-guard | 172.22.1.51 | MQTT 3.1.1 (`p4`) |

That measurement paid off immediately: it exposed two real MQTT 5 defects in the
codec, both fixed and covered by tests — `parse_connect` read the will payload length
from the v5 *will property* bytes (corrupting the will of every v5 client with a will),
and `build_connect` re-issued a v5 CONNECT with no will property block at all. Proxy
traffic for Z2M is 3.1.1 today, so neither would have shown up in production; HA
already speaks 5.0 to the broker and would break the day it is routed through the proxy.

**Throughput** (5000 QoS 1 publishes from one client, loopback, same broker):

- direct to the broker: **11 212 msg/s**
- through the proxy: **10 296 msg/s** → ~8 % cost, ~50× the 200 msg/s target

**Discovery storm** (20 topics × 10 rounds, identical retained payloads, through the
proxy): 200 publishes → **20 arrived at the broker**, `z2m_guard_discovery_deduped_total
= 180`, exactly one retained config per topic afterwards. No duplicate in the broker.

**Proxy death** (`SIGKILL` mid-session):

- the listening socket closes at once, no half-open session, client sees an abrupt close
- a connected client reconnects **on its own** as soon as the proxy is back
  (`session established client=fo-client` after restart) — measured with `mosquitto_sub`;
  Z2M's own retry timing (mqtt.js backoff) is *not* measured and needs a real Z2M
- retained messages are delivered again on that reconnect: dedupe is publish-side only,
  it never suppresses broker → client traffic
- while the proxy is down, Z2M has no broker at all, so the guard sees an unavailable
  bridge and — correctly — raises an outage. **Operationally a proxy crash therefore
  looks like a Z2M outage.** Whoever deploys this has to know that before trusting the
  alarm.

## Does it solve the documented pain? — no, and that was measured

The reason this mode exists is a note in the production compose file: an HA update on
2026-10-24 "broke MQTT/Z2M and produced a discovery message wave — 179 messages,
4.49 M tokens in one weekly run". That note puts the wave on the **consumer** side
(Home Assistant re-subscribing and receiving retained configs), not on Zigbee2MQTT.

Measured on the reference host, 2026-11-14:

| what | measured |
| --- | --- |
| discovery publishes during a **Z2M restart** | **1** (one `bridge/...` publish) |
| retained configs under `homeassistant/#` | **106 topics, 97 132 bytes, Ø 916 B** |
| of those: sensor 35, select 17, button 11, update 10, device_automation 10, binary_sensor 9, switch 7, light 7 (3 are the guard's own) | |

So Z2M 2.x does **not** republish its discovery tree on restart — it publishes on device
or config changes only. The 106 configs are delivered to every client that subscribes,
which is exactly what happens when Home Assistant restarts. A proxy between Z2M and the
broker never sees that traffic and cannot reduce it; suppressing broker → client
retained delivery would also break MQTT semantics (a subscriber is entitled to its
retained state).

Consequence: proxy mode does not fix the problem that motivated it. It remains useful as
a tool (allowlist, will preservation, dedupe of repeated *publishes* when a tool or a
script does republish), but it must not be sold as a cure for the discovery wave. The
wave has to be attacked where it is produced: fewer discovery configs (disable the HA
integration for devices that do not need it, prune unused entities), or in the consumer
that turns those messages into tokens.

This is why it was **not** switched on in production: routing Zigbee through a single
process (a new single point of failure, a widened broker ACL) buys nothing when the
traffic it targets does not occur.

## Open items before this can be released

1. **Z2M's own reconnect behaviour** through the proxy (backoff, retained-state
   effects). The client-side behaviour is measured, the Z2M-side is not.
2. **A load test with a real Z2M** during a restart, i.e. the actual storm shape,
   instead of a synthetic one.
3. **arm64 throughput.** The multi-arch image builds and runs, but every number above
   was taken on amd64.

## Tests that do exist

18 unit tests in `guard-proxy` (`cargo test -p guard-proxy`), run without a broker:

- packet codec: CONNECT with/without will, MQTT 5 properties skipping, PUBLISH QoS 0/1
  with payload extraction, SUBSCRIBE filters, CONNACK/PUBACK shapes per protocol level
- allowlist: wildcard matching, `$`-topic rule, filter-vs-filter coverage
  (`zigbee2mqtt/#` permits `zigbee2mqtt/+/set`, but `#` is never permitted by it)
- dedupe: identical retained payload dropped, changed payload passes, non-retained
  always passes, window expiry, distinct topics
- end-to-end against an in-process fake broker: duplicate discovery is dropped **and
  acked** while the broker sees exactly one publish; a publish outside the allowlist
  closes the session; non-retained repeats pass; the client's will reaches upstream.

One of those tests exists because of a real bug found while writing it: the QoS 1
acknowledgement for a dropped packet was first written to the *upstream* socket. The
broker had never seen that packet, and the client blocked forever waiting for its
acknowledgement. The test (`duplicate_retained_discovery_is_dropped_and_acked`) hangs
without the fix, which is what made it findable.