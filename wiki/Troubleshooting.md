# Troubleshooting

## Does it actually work? (test plan)

Ten minutes, no real incident needed. Run it once after installing — it is the
difference between "the container is up" and "the alarm chain behaves".

**1. An unannounced restart must escalate.**

```bash
# note the counter before
curl -s localhost:8089/metrics | grep outage_confirmed_total
docker compose up -d --force-recreate zigbee2mqtt
```

Expected: `sensor.z2m_guard_state` goes `suspect` immediately, and after **15 s**
(`grace_ms`) to `outage_confirmed`; `binary_sensor.z2m_guard_suppress` stays `off`;
`outage_confirmed_total` increases by 1; your phone rings once, at ~t+15 s.

**2. An announced restart must stay silent.**

```bash
Z2M_GUARD_HTTP_TOKEN=… ./deploy/restart-z2m.sh 180
```

Expected: `suppress` goes `on`, state `planned` → `settling` → `idle`;
`suppressed_events_total{reason="prior_signal"}` increases; `last_restart` shows
~8–9 s and `reason` is `prior_signal`; no notification at all.

**3. The guard must not be the thing that is broken.**

```bash
docker stop z2m-guard && docker compose up -d --force-recreate zigbee2mqtt
```

Expected: the alarm fires normally. If it does not, the automation is gated
somewhere else — the guard is not in the path.

## Symptom → cause → fix

| Symptom | Cause | Fix |
|---|---|---|
| Guard exits at startup: `mqtts://` refused | TLS is not supported (by design, no TLS stack in the binary) | use `mqtt://` and keep the broker on a private network |
| `Connection refused, return code: NotAuthorized` | wrong password, or the ACL does not grant the topics | compare `docker compose config --format json` with `.env`; an **exported shell variable overrides `.env`** |
| Guard connected, but `z2m_guard_mqtt_messages_total{kind="bridge"}` stays at 1 | wrong `base_topic`, or Z2M publishes elsewhere | `mosquitto_sub -t 'zigbee2mqtt/bridge/state' -v` by hand |
| No entities in Home Assistant | `discovery_prefix` differs from HA's MQTT prefix, or the ACL denies writes to `homeassistant/...` | read the retained config topic (`mosquitto_sub -t 'homeassistant/binary_sensor/z2m_guard/suppress/config' -C 1 -v`) |
| Entities show an old `sw_version` | HA updates the device registry only when a discovery payload arrives | restart the guard (payloads are republished on every connect) |
| Alarm still fires during planned restarts | the restart was not announced, or the automation is not gated | use `deploy/restart-z2m.sh`, or add the `suppress` condition ([Home Assistant](Home-Assistant)) |
| `401` from `/v1/restart-intent` | token missing inside the container, or a different one | the value is compared against `Z2M_GUARD_HTTP_TOKEN` **in the container**, not on your shell |
| `z2m_guard_window_active` stays `1` for minutes | the guard is not receiving bridge state, so the window cannot close | see the `base_topic` and broker rows above |
| Metrics empty from the host | `http.bind` is `127.0.0.1` inside the container | bind `0.0.0.0:8089` and publish no host port; scrape over the Docker network |
| Container restarts in a loop | config or secret missing | `docker logs z2m-guard` names the field |
| Entities `unavailable` right after a Z2M restart | the guard has not reconnected to the broker yet (last-will published `offline`) | wait one reconnect backoff (≤ 30 s) |
| `last_restart` stays `0` | no *attributed* restart has happened yet | run test 2 above |

## Reading the guard's own log

```bash
docker logs -f z2m-guard
```

One JSON line per event, plus structured `INFO` lines for the connection lifecycle:

```
INFO starting broker=mqtt://172.22.1.50:1883 base_topic=zigbee2mqtt discovery=true
INFO mqtt connected
{"kind":"connected","state":"idle","ts_ms":1}
```

`RUST_LOG=debug` adds the state transitions with their deadlines — the fastest way
to see *why* a window opened or closed.

## A state that looks wrong

The guard keeps no state on disk. If the entities disagree with reality after a
manual intervention:

```bash
docker compose restart z2m-guard      # or: docker restart z2m-guard
```

It reconnects, reads the retained bridge state, and republishes discovery — `idle`
or `suspect`, depending on what the broker says right now. There is nothing to
migrate, nothing to clean up.

## Removing the installation cleanly

Deleting the container leaves retained discovery payloads and HA entities behind:

```bash
for t in binary_sensor/z2m_guard/suppress sensor/z2m_guard/state sensor/z2m_guard/last_restart; do
  mosquitto_pub -h <broker> -u <user> -P '<password>' -r -n -t "homeassistant/$t/config"
done
docker compose rm -sf z2m-guard
```

Then reload the MQTT integration in Home Assistant.