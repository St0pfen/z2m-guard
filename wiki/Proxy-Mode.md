# Proxy mode

**Off by default. Not enabled anywhere, and not published as an image.**

## The short version

This mode sits between Zigbee2MQTT and the broker. It was built to stop a discovery
republish storm that was believed to come from Z2M. Measured on a real stack, that storm
does not come from Z2M: a Z2M restart publishes **one** discovery message. The traffic
that actually moves is the broker delivering 106 retained configs to a **consumer** that
subscribes — Home Assistant, whenever it restarts — and a proxy in front of Z2M never
sees that.

So: read this page if you want a small, allowlisted MQTT relay in that position. Do not
expect it to shrink your token bill.

## What it does

```
Zigbee2MQTT ──► z2m-guard proxy (:1884) ──► Mosquitto :1883
Home Assistant ───────────────────────────► Mosquitto :1883
```

- terminates the client's CONNECT and re-issues it upstream with the **same protocol
  level** (MQTT 3.1.1 or 5.0) and **the client's will** — without the will, the broker
  would never publish `offline` for the bridge
- forwards everything else byte for byte in both directions (no re-encoding, so MQTT 5
  properties and reason codes survive)
- enforces a topic allowlist on PUBLISH and SUBSCRIBE, and **closes the session** on a
  violation instead of forwarding quietly
- drops a **retained** publish whose topic + payload already went through inside
  `dedupe_window_ms` — and acknowledges QoS 1 itself, so the client does not retransmit
  what dedupe just removed. Non-retained publishes always pass.
- counts drops in `z2m_guard_discovery_deduped_total`

## What it does not

- not a broker: no retained store, no sessions, no `$SYS`
- one downstream client, no per-client isolation
- no TLS (like the rest of the project)
- **not** high availability: if the proxy is down, Z2M cannot reach the broker at all,
  and the guard will report that as an outage. A proxy crash looks like a Z2M outage.

## Enabling it

The default image has the feature compiled out on purpose. Build your own:

```bash
docker build --build-arg FEATURES=proxy -t z2m-guard:proxy .
```

or from source: `cargo build --release --features proxy`.

```toml
[proxy]
enabled = true
listen = "0.0.0.0:1884"
allow = ["zigbee2mqtt/#"]
dedupe_window_ms = 60000
```

Then point Z2M at it (usually in Z2M's own `configuration.yaml`, not an env var):

```yaml
mqtt:
  server: mqtt://z2m-guard:1884
```

A binary **without** the feature refuses `proxy.enabled = true` at startup instead of
silently running a guard whose proxy a deployment believes is in the path.

The broker ACL has to allow the guard's user to **write** the relayed topics — a
read-only guard account cannot relay anything. Widening that account is a real
privilege change; consider a separate MQTT user for the proxy connection.

## Measured behaviour

Thorough numbers, the measurement setup and the open questions live in
[`docs/PROXY.md`](https://github.com/St0pfen/z2m-guard/blob/main/docs/PROXY.md).
The headline figures:

- throughput 10 296 msg/s through the proxy vs 11 212 direct (~8 % cost)
- 180 of 200 synthetic duplicate retained publishes dropped, zero duplicates at the broker
- a `SIGKILL`ed proxy closes the socket at once; the client reconnects on its own