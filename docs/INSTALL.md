# Installing z2m-guard

z2m-guard is a standalone service, not a Home Assistant integration. It subscribes
to one retained MQTT topic, decides whether Zigbee2MQTT's outage was planned, and
publishes its verdict — plus three Home Assistant entities via MQTT discovery.
Nothing is installed into Home Assistant itself.

```
docker compose up ──► zigbee2mqtt recreated ──► all 103 entities "unavailable"
                                                        │
z2m-guard ── hears zigbee2mqtt/bridge/state ────────────┘
          ── knows a restart was announced ──► suppresses the alarm window
          ── publishes z2m_guard/{state,suppress,last_restart}
          ── Home Assistant shows the three entities
```

What it needs: an MQTT broker, a Z2M instance publishing `zigbee2mqtt/bridge/state`,
and Home Assistant's MQTT integration (for the entities). No Docker socket, no
privileged mode, no host ports.

- [Requirements](#requirements)
- [Variant A — Docker Compose (recommended)](#variant-a--docker-compose-recommended)
- [Home Assistant: entities](#home-assistant-entities)
- [Operating it](#operating-it)
- [Variant B — static binary + systemd](#variant-b--static-binary--systemd)
- [Variant C — Home Assistant App (HAOS / Supervised only)](#variant-c--home-assistant-app-haos--supervised-only)
- [Why not HACS / the Marketplace](#why-not-hacs--the-marketplace)
- [Monitoring](#monitoring)
- [Troubleshooting](#troubleshooting)
- [Upgrades and removal](#upgrades-and-removal)

## Requirements

| What | Why |
|---|---|
| MQTT broker (Mosquitto etc.) | transport for everything the guard does |
| Zigbee2MQTT ≥ 1.34 with `homeassistant: true` | publishes `zigbee2mqtt/bridge/state`, retained |
| Home Assistant with the MQTT integration | creates the three entities from discovery |
| Docker + Compose (variant A) or a Linux host with systemd (variant B) | runs the process |
| A free address in the broker's network | the guard only talks to the broker and to Prometheus |

If Zigbee2MQTT publishes under a different base topic, set `base_topic` accordingly.
Check before you start:

```bash
mosquitto_sub -h <broker> -u <user> -P '<password>' -t 'zigbee2mqtt/bridge/state' -C 1 -v
# expect: zigbee2mqtt/bridge/state {"state":"online"}
```

If that prints nothing, the guard will see no outages at all — the topic is the
single input it has.

## Variant A — Docker Compose (recommended)

Five steps. The example uses `172.22.1.50` for the broker and `172.22.1.51` for the
guard; substitute your own.

### 1. Get the code and build the image

```bash
git clone https://github.com/St0pfen/z2m-guard.git
cd z2m-guard
docker build -t z2m-guard:local .
```

The image is distroless, runs as `nonroot` and needs no writable filesystem.

### 2. Create a broker user with a least-privilege ACL

Do not reuse the Home Assistant service account. `deploy/mosquitto/README.md` has
the full procedure; the short version:

```bash
docker exec -u 0 mosquitto mosquitto_passwd -b /mosquitto/config/password.txt z2mguard '<password>'
docker exec -i -u 0 mosquitto sh -c 'cat > /mosquitto/config/acl' < deploy/mosquitto/acl.example
docker exec -u 0 mosquitto sh -c "printf '\nacl_file /mosquitto/config/acl\n' >> /mosquitto/config/mosquitto.conf"
docker exec -u 0 mosquitto sh -c 'chown 1883:1883 /mosquitto/config/* && chmod 640 /mosquitto/config/mosquitto.conf'
docker restart mosquitto          # ~1 s of MQTT downtime
```

Roll it out during a quiet moment, or announce it to the guard first (step 4).

### 3. Configuration

`./z2m-guard/config.toml` — copy from `config/example.toml` and edit:

```toml
[mqtt]
url = "mqtt://172.22.1.50:1883"
client_id = "z2m-guard"
username = "z2mguard"
password_env = "Z2M_GUARD_MQTT_PASSWORD"   # never write the password here
base_topic = "zigbee2mqtt"

[guard]
grace_ms = 15000            # how long an unexplained outage stays suppressed
settle_ms = 20000           # hold suppression after the bridge returns
attribution_window_ms = 30000
prior_window_ms = 180000    # cap for an announced restart
tick_ms = 1000

[homeassistant]
discovery_prefix = "homeassistant"
device_name = "Z2M Guard"
discovery = true            # false = observe only, no entities

[http]
bind = "0.0.0.0:8089"       # inside the container network only
token_env = "Z2M_GUARD_HTTP_TOKEN"
```

### 4. Secrets

```bash
cd ~/docker/home-assistent            # wherever your compose file lives
umask 077
printf 'Z2M_GUARD_MQTT_PASSWORD=%s\n' '<broker password>' > .env
printf 'Z2M_GUARD_HTTP_TOKEN=%s\n' "$(head -c 24 /dev/urandom | base64 | tr -d '/+=')" >> .env
chmod 600 .env
```

The HTTP token gates `POST /v1/restart-intent`. It is the only thing that can open
a suppression window, so keep it out of the compose file and out of git.

### 5. Compose service

Paste into your existing stack (full version: `deploy/docker-compose.snippet.yml`):

```yaml
services:
  z2m-guard:
    container_name: z2m-guard
    image: z2m-guard:local
    restart: always
    read_only: true
    environment:
      - TZ=Europe/Berlin
      - RUST_LOG=info
      - Z2M_GUARD_MQTT_PASSWORD=${Z2M_GUARD_MQTT_PASSWORD}
      - Z2M_GUARD_HTTP_TOKEN=${Z2M_GUARD_HTTP_TOKEN}
    volumes:
      - ./z2m-guard/config.toml:/config.toml:ro
    command: ["--config", "/config.toml"]
    networks:
      hass_net:
        ipv4_address: 172.22.1.51         # must be free, must match config.toml
      monitoring:
        aliases: [z2m-guard]             # optional, for Prometheus

networks:
  hass_net:
    external: true
    name: hass_net
  monitoring:
    external: true
    name: monitoring_default
```

Two things that bite here: a fixed IP that is already taken, and a `z2m_guard`
service block accidentally placed outside `services:` (compose then fails with
`additional properties 'z2m-guard' not allowed`).

```bash
docker compose up -d z2m-guard
```

### Verify the install

```bash
docker logs --tail 20 z2m-guard
#   INFO http listening bind=0.0.0.0:8089
#   INFO starting broker=mqtt://… base_topic=zigbee2mqtt discovery=true
#   INFO mqtt connected
#   {"kind":"connected","state":"idle","ts_ms":2}

docker exec prometheus wget -qO- http://z2m-guard:8089/healthz   # ok
docker exec prometheus wget -qO- http://z2m-guard:8089/readyz    # ready
#   /readyz answers 503 until the guard has an MQTT connection: that is the
#   honest probe for a container healthcheck, /healthz only proves the process.
docker exec prometheus wget -qO- http://z2m-guard:8089/metrics | grep z2m_guard_mqtt_messages_total
#   z2m_guard_mqtt_messages_total{kind="bridge"} 1
```

Then look at the broker, not at the logs — this is what Home Assistant will read:

```bash
mosquitto_sub -h <broker> -u <user> -P '<password>' \
  -t 'homeassistant/+/z2m_guard/+/config' -v
# three discovery configs: binary_sensor/z2m_guard/suppress,
#                          sensor/z2m_guard/state, sensor/z2m_guard/last_restart
```

A `NotAuthorized` in the log means the ACL or the password is wrong, not the tool.

## Home Assistant: entities

Nothing to add by hand. Home Assistant's MQTT integration picks up the discovery
configs on its own, usually within seconds:

- `binary_sensor.z2m_guard_suppress` — `on` while a suppression window is open
- `sensor.z2m_guard_state` — `idle` / `suspect` / `planned` / `settling` / `outage_confirmed`
- `sensor.z2m_guard_last_restart` — duration of the last suppressed outage, in seconds

They appear under **Settings → Devices & services → MQTT → Z2M Guard**. If they do
not appear: check that Home Assistant's MQTT integration uses the same broker and
that `discovery_prefix` matches its `discovery_prefix` (default `homeassistant`).
Note that the guard needs write permission for
`homeassistant/{binary_sensor,sensor}/z2m_guard/#` — the sample ACL grants exactly
that and nothing more.

Alarm automation that stays quiet during planned restarts:

```yaml
alias: Zigbee outage (unplanned)
trigger:
  - platform: mqtt
    topic: zigbee2mqtt/bridge/state
    payload: '{"state":"offline"}'
condition:
  - condition: state
    entity_id: binary_sensor.z2m_guard_suppress
    state: "off"
action:
  - service: notify.mobile_app_<phone>
    data:
      message: "Zigbee2MQTT is offline and no restart was announced."
```

Because the guard only ever suppresses for a bounded window, a crash that nobody
announced still reaches this automation after `grace_ms`.

`deploy/homeassistant/z2m_guard.yaml` additionally defines a template binary sensor
`binary_sensor.zigbee_outage_unplanned` (`device_class: problem`) that is only `on`
when the guard reports `outage_confirmed` **and** no window is open — handy as a
single alarm entity. Copy it to `/config/packages/` if you want it.

## Operating it

**Planned restart (the normal path).** The script posts an intent first, then
recreates Z2M; the guard attributes the outage instead of escalating it:

```bash
Z2M_GUARD_HTTP_TOKEN=… Z2M_GUARD_URL=http://172.22.1.51:8089 \
  ./deploy/restart-z2m.sh 180
```

**Planned restart from Home Assistant.** Call the endpoint with `rest_command`:

```yaml
rest_command:
  z2m_guard_restart_intent:
    url: http://172.22.1.51:8089/v1/restart-intent
    method: POST
    headers:
      x-guard-token: !secret z2m_guard_http_token
      content-type: application/json
    payload: '{"reason":"ha-restart","window_s":180}'
```

Announce, wait a second, then restart the Z2M app/container however you do that.

**Manual suppression window without a restart** (useful while repairing the mesh):

```bash
curl -X POST http://172.22.1.51:8089/v1/restart-intent \
  -H "x-guard-token: $Z2M_GUARD_HTTP_TOKEN" \
  -H 'content-type: application/json' \
  -d '{"reason":"maintenance","window_s":600}'
```

The window is a deadline, never open-ended: after `window_s` the guard is back to
alarming normally even if nothing happened.

## Variant B — static binary + systemd

For hosts without Docker (or when the guard should sit on another machine than the
broker). Take the musl binary from `dist/` or the release page:

```bash
install -m 0755 z2m-guard-0.1.2-x86_64-unknown-linux-musl /usr/local/bin/z2m-guard
install -d -m 0750 /etc/z2m-guard
install -m 0600 /dev/null /etc/z2m-guard/z2m-guard.env
```

`/etc/z2m-guard/z2m-guard.env`:

```ini
RUST_LOG=info
Z2M_GUARD_MQTT_PASSWORD=<broker password>
Z2M_GUARD_HTTP_TOKEN=<token>
```

`/etc/systemd/system/z2m-guard.service`:

```ini
[Unit]
Description=z2m-guard — planned Zigbee2MQTT restarts
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
ExecStart=/usr/local/bin/z2m-guard --config /etc/z2m-guard/config.toml
EnvironmentFile=/etc/z2m-guard/z2m-guard.env
DynamicUser=yes
NoNewPrivileges=yes
ProtectSystem=strict
ProtectHome=yes
PrivateTmp=yes
PrivateDevices=yes
RestrictAddressFamilies=AF_INET AF_INET6
Restart=on-failure
RestartSec=2s

[Install]
WantedBy=multi-user.target
```

```bash
systemctl daemon-reload && systemctl enable --now z2m-guard
systemctl status z2m-guard --no-pager
```

With `DynamicUser=yes` the environment file must be readable by it — either drop
`DynamicUser` or keep the secrets in the unit via `Environment=`; simplest is to run
as a dedicated user instead:

```bash
useradd --system --no-create-home --shell /usr/sbin/nologin z2m-guard
```

If the host has no `docker` binary, attribution relies purely on the restart-intent
endpoint. That is the normal case and works fine; the Docker event path is only a
convenience for `docker compose up -d` done by hand.

### Docker event attribution (host mode only)

The `stdin-events` build feature parses `docker events` JSON and turns a container
exit into an attribution. It only makes sense where the guard can actually be fed
that stream — the systemd variant on the Docker host. The container image is
therefore built **without** it, and `--docker-events-stdin` fails loudly if the
flag is passed to a binary that lacks the feature.

Build it for the host variant:

```bash
cargo build --release --features stdin-events -p z2m-guard
```

and wrap the process so the event stream is its stdin:

```ini
ExecStart=/bin/bash -c 'exec docker events \
  --filter container=zigbee2mqtt \
  --format "{{json .}}" | exec /usr/local/bin/z2m-guard \
  --config /etc/z2m-guard/config.toml --docker-events-stdin'
```

`docker events` keeps running as long as the pipe is open; if it exits, systemd
restarts the unit. Without this wiring the guard still works — planned restarts are
then attributed solely by the intent endpoint.

## Variant C — Home Assistant App (HAOS / Supervised only)

Container-based Home Assistant (the setup in this repo's reference host) has **no**
app store; variant A is the path there. On Home Assistant OS or Supervised you can
ship the guard as an app instead, so users install it from Settings → Apps with one
click. A minimal repository layout:

```text
repository.yaml
z2m_guard/
  config.yaml
  Dockerfile
  run.sh
```

`repository.yaml`:

```yaml
name: z2m-guard
url: https://github.com/St0pfen/z2m-guard
maintainer: St0pfen
```

`z2m_guard/config.yaml`:

```yaml
name: z2m-guard
version: "0.1.2"
slug: z2m_guard
description: Separate planned Zigbee2MQTT restarts from real outages
arch: [amd64, aarch64]
startup: application
boot: auto
host_network: false
options:
  broker_url: "mqtt://core-mosquitto:1883"
  username: z2mguard
  base_topic: zigbee2mqtt
  grace_ms: 15000
  window_s: 180
schema:
  broker_url: str
  username: str
  base_topic: str
  grace_ms: int
  window_s: int
```

`run.sh` renders `/data/config.toml` from `/data/options.json` (bashio) and execs
the binary; the password comes from the app's own MQTT credentials, which the user
enters in the app configuration screen. Two consequences to plan for: the app runs
inside Supervisor's network, so a Mosquitto app should be addressed as
`core-mosquitto`, and `docker events` is not available to an app — attribution then
relies on the intent endpoint, which is exactly what `deploy/restart-z2m.sh` does.

This variant is not part of the repo yet; it is the packaging step that would make
the tool installable for HAOS users without touching compose.

## Why not HACS / the Marketplace

HACS (and, from Home Assistant 2026.11, its replacement **Marketplace**) installs
content *into* Home Assistant's process: custom integrations into
`custom_components/<domain>/`, dashboard cards into `www/community/`, themes into
`themes/`, Jinja templates into `custom_templates/`. All of it is Python or
JavaScript loaded by Home Assistant, described by a `manifest.json` (integrations)
or `hacs.json` (cards, themes).

z2m-guard is none of those things:

- it is a compiled Rust process with its own lifecycle, not a Python module
- it must sit next to the MQTT broker and survive Home Assistant restarts — an
  integration would stop when HA stops
- it already delivers its entities through **MQTT discovery**, which is the
  broker-level mechanism Home Assistant supports natively; a custom integration
  would duplicate what discovery does for free

So there is no HACS installation path, and adding one would be worse than what
exists: you would get an integration that does nothing unless a container is
running anyway. The honest distribution ladder is:

| Audience | Mechanism |
|---|---|
| Compose users (this host) | `docker compose up -d` with the snippet above |
| Bare-metal / other hosts | musl binary + systemd unit |
| Home Assistant OS / Supervised | app repository (variant C), installed from Settings → Apps |
| Everyone else | nothing to install — the entities arrive via MQTT discovery |

If the guard ever grows an HA-side companion (for example a diagnostic sensor that
proxies the HTTP API because MQTT is unavailable), *that* could legitimately be a
HACS/Marketplace integration. The guard itself cannot.

## Monitoring

Prometheus (`deploy/prometheus/scrape-snippet.yml`):

```yaml
- job_name: z2m-guard
  static_configs:
    - targets: ["z2m-guard:8089"]     # network alias from the compose snippet
```

Series worth an alert: `z2m_guard_outage_confirmed_total` increasing, and
`z2m_guard_window_active == 1` for longer than expected (a window that never closes
means the guard is not receiving bridge state). A ready-made dashboard is in
`deploy/grafana/z2m-guard.json`.

Scraping over the internal network keeps `/metrics` off the host — the guard
publishes no ports at all.

## Troubleshooting

| Symptom | Cause | Fix |
|---|---|---|
| `Connection refused, return code: NotAuthorized` | wrong password, or the ACL does not grant the topics | compare `docker compose config --format json` with `.env` — an exported shell variable overrides `.env` |
| Guard connects, but `mqtt_messages_total{kind="bridge"} 0` | wrong `base_topic`, or Z2M does not publish bridge state | subscribe to the topic by hand |
| No entities in Home Assistant | `discovery_prefix` differs, or no write permission on `homeassistant/...` | check the retained topics on the broker |
| Alarm still fires during planned restarts | the restart was not announced | use `deploy/restart-z2m.sh`, or call `/v1/restart-intent` first |
| `401` from `/v1/restart-intent` | token missing or different | the token is compared against `Z2M_GUARD_HTTP_TOKEN` |
| Entities stay `unavailable` after a restart | the MQTT last-will published `offline` and the guard has not reconnected yet | wait one reconnect backoff (≤30 s), check broker reachability |
| Container restarts in a loop | config or secret missing | `docker logs z2m-guard` names the field |

## Upgrades and removal

```bash
git pull && docker build -t z2m-guard:local .
docker compose up -d --no-deps z2m-guard
```

Announce the recreate first if you want it attributed. The state is in memory only:
after a guard restart the entities show `idle`/`off` and `last_restart` starts over.

Removal leaves retained discovery payloads behind — Home Assistant keeps showing
`unavailable` entities. Clear them explicitly:

```bash
for t in binary_sensor/z2m_guard/suppress sensor/z2m_guard/state sensor/z2m_guard/last_restart; do
  mosquitto_pub -h <broker> -u <user> -P '<password>' -r -n -t "homeassistant/$t/config"
done
docker compose rm -sf z2m-guard
```

Same for the verdict topics if you want a clean broker:
`z2m_guard/#` with empty retained payloads.