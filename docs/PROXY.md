# Proxy mode (M5) — implementation notes

Status: **implemented on the `feat/m5-proxy` branch, not validated, not released.**
The guard still does its normal job without it; nothing in a released artifact
changes until this mode is enabled *and* has been measured against a real
Zigbee2MQTT.

## Why it exists

Z2M republishes its whole discovery tree on every start. The payloads are identical
and retained, so the broker already holds them — but every republish still travels
to every subscriber. On the reference host that was **179 messages / 4.49 M tokens**
in a single weekly run, which is a token bill and a Home Assistant restart-storm.

The guard already knows when a restart was planned. Proxy mode is what turns that
knowledge into a smaller storm: it sits between Z2M and the broker and drops a
retained payload it has already forwarded inside the dedupe window.

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

Build with the feature, then turn it on in config:

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

Then point Z2M at the proxy instead of the broker:

```yaml
# docker-compose, zigbee2mqtt service
environment:
  MQTT_SERVER: mqtt://z2m-guard:1884
```

A binary **without** `--features proxy` refuses `proxy.enabled = true` at startup
rather than silently running a guard whose proxy a deployment believes is there.

The proxy connects upstream with the guard's own MQTT user (`mqtt.username`,
`mqtt.password_env`), not with Z2M's — the proxy is the one holding the
least-privilege account.

## Open items before this can be released

These are *not* done, and none of them can be replaced by the unit tests:

1. **MQTT version measurement.** The plan requires measuring which protocol version
   Z2M and HA actually negotiate against the real broker before shipping this. The
   code preserves the client's version and forwards frames verbatim, but that is a
   design argument, not a measurement.
2. **Load test.** Target from the plan: 200 messages/s sustained. Not run.
3. **Failover behaviour.** What Z2M does when the proxy dies mid-session (reconnect
   timing, retained-state effects) has to be observed on a real stack.
4. **Fast-path cost.** The relay reads packet-by-packet with several small reads;
   throughput on the reference host is unmeasured.

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