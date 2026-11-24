# Configuration

Config is a TOML file (`--config /path/to/config.toml`) plus environment variables.
Precedence: **environment wins over the file**, the file wins over the built-in
defaults. Secrets are never stored in the file — the file names the variable that
holds them.

Without `--config` the guard runs on defaults plus environment variables, so a
minimal deployment is possible with `Z2M_GUARD_MQTT_URL` and a password variable
alone.

## `[mqtt]`

| Key | Default | Meaning |
|---|---|---|
| `url` | `mqtt://127.0.0.1:1883` | Broker URL. Only `mqtt://` (plain TCP) is accepted — see below |
| `client_id` | `z2m-guard` | MQTT client id |
| `username` | `z2mguard` | Broker user. Use a dedicated one, not the Home Assistant account |
| `password_env` | `Z2M_GUARD_MQTT_PASSWORD` | **Name** of the variable holding the password |
| `base_topic` | `zigbee2mqtt` | Prefix of the bridge topic; the guard subscribes to `<base_topic>/bridge/state` |

`mqtts://` is rejected at startup, deliberately: the binary links no TLS stack.
Keep the broker on a private Docker network instead of exposing it — reasoning and
the removed dependency chain are in
[Security](https://github.com/St0pfen/z2m-guard/blob/main/SECURITY.md).

## `[guard]` — the timings

| Key | Default | Meaning |
|---|---|---|
| `grace_ms` | `15000` | How long an **unexplained** outage may stay suppressed before it escalates to `outage_confirmed` |
| `settle_ms` | `20000` | How long suppression is held after the bridge returns, so the mesh can stabilise |
| `attribution_window_ms` | `30000` | A container exit only explains an outage if it happened this close to it |
| `prior_window_ms` | `120000` | Upper bound for an announced restart (the script overrides it per call) |
| `tick_ms` | `1000` | Housekeeping interval — this is what expires windows even when no messages arrive |

These five are only settable in the file; there are no environment overrides for
them. Raising `grace_ms` delays every real alarm by that much — the whole safety
argument in [How it decides](How-it-decides) rests on it being small and finite.

## `[homeassistant]`

| Key | Default | Meaning |
|---|---|---|
| `discovery_prefix` | `homeassistant` | Must match HA's MQTT integration; otherwise the entities never appear |
| `device_name` | `Z2M Guard` | Name of the device the three entities are grouped under |
| `discovery` | `true` | `false` = observe-only run that publishes no entities (useful for a trial against a production broker) |

## `[http]`

| Key | Default | Meaning |
|---|---|---|
| `bind` | `127.0.0.1:8089` | Listen address for `/metrics` and `/v1/restart-intent`. In compose set `0.0.0.0:8089` and publish **no** host port |
| `token_env` | `Z2M_GUARD_HTTP_TOKEN` | **Name** of the variable holding the shared restart-intent token |

`POST /v1/restart-intent` compares the token in constant time and answers `401`
when it is missing or different. No token set (and no variable present) means
intents are refused — the endpoint never becomes open by accident.

Endpoints: `GET /healthz` (liveness, always `200` while the process runs),
`GET /readyz` (`503` until the guard holds an MQTT connection — use this for a
container healthcheck, so a daemon that lost its broker stops reporting ready),
`GET /metrics`, `POST /v1/restart-intent`.

`/readyz` deliberately does **not** follow the bridge itself: it asks whether the
guard can observe the bridge, not whether the bridge is up. A `503` means "my
verdict is not currently backed by broker state", not "Zigbee is broken".

## Environment overrides

| Variable | Overrides |
|---|---|
| `Z2M_GUARD_MQTT_URL` | `mqtt.url` |
| `Z2M_GUARD_MQTT_USERNAME` | `mqtt.username` |
| `Z2M_GUARD_BASE_TOPIC` | `mqtt.base_topic` |
| `Z2M_GUARD_BIND` | `http.bind` |
| `Z2M_GUARD_DISCOVERY` | `homeassistant.discovery` (`1`/`true` enables) |
| `Z2M_GUARD_MQTT_PASSWORD` | the password itself (variable named by `mqtt.password_env`) |
| `Z2M_GUARD_HTTP_TOKEN` | the intent token (variable named by `http.token_env`) |
| `RUST_LOG` | log level, default `info` |

## Command line

```bash
z2m-guard --config /etc/z2m-guard/config.toml   # normal run
z2m-guard --print-discovery                     # print the three HA payloads, exit (no broker needed)
z2m-guard --docker-events-stdin                 # attribution from piped docker events (build with --features stdin-events)
z2m-guard --version
```

## Complete example

`config/example.toml` in the repository is this, with comments:

```toml
[mqtt]
url = "mqtt://172.22.1.50:1883"
client_id = "z2m-guard"
username = "z2mguard"
password_env = "Z2M_GUARD_MQTT_PASSWORD"
base_topic = "zigbee2mqtt"

[guard]
grace_ms = 15000
settle_ms = 20000
attribution_window_ms = 30000
prior_window_ms = 120000
tick_ms = 1000

[homeassistant]
discovery_prefix = "homeassistant"
device_name = "Z2M Guard"
discovery = true

[http]
bind = "0.0.0.0:8089"
token_env = "Z2M_GUARD_HTTP_TOKEN"
```

## Checking a config without touching a running setup

```bash
docker run --rm -v "$PWD/config:/cfg:ro" --entrypoint /usr/local/bin/z2m-guard \
  ghcr.io/st0pfen/z2m-guard:0.1.2 --config /cfg/local.toml --print-discovery
```

`--print-discovery` exits before connecting to anything, so a wrong key in `[guard]`
or `[homeassistant]` shows up here rather than as a crash-looping container in
production.

## Startup validation

Before anything binds, the guard rejects a config that parsed but cannot work:

- `mqtt.url` and `mqtt.client_id` must be non-empty,
- `mqtt.base_topic` must be a topic *root*: no wildcards (`#`, `+`), no leading,
  trailing or doubled `/`,
- when `proxy.enabled = true`, `proxy.allow` must contain at least one entry and
  every entry must be a syntactically valid MQTT filter (`#` only as the last
  level, `+` only as a whole level).

The last check exists because a typo like `zigbee2mqtt#` (missing slash) is a
valid single literal level: the proxy would relay nothing and close every
session, which looks like a broken bridge rather than a broken config. The
error names the offending entry.