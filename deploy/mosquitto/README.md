# Mosquitto hardening for z2m-guard

The guard does not need the broker's service credentials. This directory holds the
ACL used on the reference host and the procedure to roll it out without an alarm.

## Why it matters

Before hardening there is a single user (`hass`) with full access, shared by Home
Assistant, Zigbee2MQTT and Node-RED. The guard only ever reads one retained topic
and writes its own namespace — giving it the same all-powerful credential widens
the blast radius for no reason.

## Rollout (≈5 s of MQTT downtime)

```bash
# 0. back up the working config
docker exec -u 0 mosquitto sh -c 'mkdir -p /mosquitto/config/backup && \
  cp /mosquitto/config/mosquitto.conf /mosquitto/config/password.txt /mosquitto/config/backup/'

# 1. add the user (generates the hash; never write the plaintext to disk)
docker exec -u 0 mosquitto mosquitto_passwd -b /mosquitto/config/password.txt z2mguard "$Z2M_GUARD_MQTT_PASSWORD"

# 2. install the ACL and point the broker at it
docker exec -i -u 0 mosquitto sh -c 'cat > /mosquitto/config/acl' < acl.example
docker exec -u 0 mosquitto sh -c "printf '\nacl_file /mosquitto/config/acl\n' >> /mosquitto/config/mosquitto.conf"
docker exec -u 0 mosquitto sh -c 'chown 1883:1883 /mosquitto/config/* && chmod 640 /mosquitto/config/mosquitto.conf'

# 3. dry-run the exact config on a throwaway port BEFORE touching the live broker
docker run -d --rm --name mosq-acl-test -v /opt/home-assistant/mosquitto/config:/mosquitto/config:ro \
  -p 127.0.0.1:1885:1883 eclipse-mosquitto:2

# 4. announce the window so the guard attributes the client drop, then restart
curl -X POST http://172.22.1.51:8089/v1/restart-intent -H "x-guard-token: $TOKEN" \
  -H 'content-type: application/json' -d '{"reason":"broker-acl","window_s":180}'
docker restart mosquitto
```

Step 3 is the one that saves you: the live broker never sees an ACL it has not
already accepted.

## Acceptance checks

| Check | Expected |
|---|---|
| publish as `z2mguard` to `zigbee2mqtt/probe`, listener as `hass` | listener receives **nothing** |
| publish as `z2mguard` to `homeassistant/switch/other/config` | **nothing** |
| publish as `z2mguard` to `z2m_guard/probe` | delivered |
| subscribe as `z2mguard` to `zigbee2mqtt/#` | only `zigbee2mqtt/bridge/state` |
| subscribe as `z2mguard` to `homeassistant/#` | nothing |
| guard restart with the new credentials | `mqtt connected`, `mqtt_messages_total{kind="bridge"} 1` |
| Home Assistant | three `z2m_guard` entities present, no MQTT auth errors |

Denied publishes are **silent** — `mosquitto_pub` still prints `PUBACK RC:0`. Only
the second-client check proves the drop.

## Rollback

```bash
docker exec -u 0 mosquitto sh -c 'cp /mosquitto/config/backup/mosquitto.conf /mosquitto/config/ && \
  cp /mosquitto/config/backup/password.txt /mosquitto/config/'
docker restart mosquitto
```

Old credentials work again immediately; the guard can keep using `z2mguard` until
you fix the ACL, or revert its `config.toml` to the service user.

## Gotcha that cost a debugging round

`docker compose` gives **exported shell variables precedence over `.env`**. After
`set -a; . .env; set +a` in an interactive shell, a later `docker compose up` sends
the old value even though `.env` is correct. Symptom: the guard logs
`Connection refused, return code: NotAuthorized` while the password in `.env` is
provably right. Compare with `docker compose config --format json`, and `unset` the
variable (or run compose in a clean shell).