# Home Assistant

The guard publishes **MQTT discovery** payloads, so Home Assistant creates the
entities by itself — no integration, no HACS, no `configuration.yaml` entry. The
only requirement is that HA's MQTT integration is connected to the same broker.

## The device and its entities

Everything appears under one device: **Z2M Guard** (`manufacturer: z2m-guard`,
`model: restart watchdog`). The `sw_version` in the device registry is the version
that published the payload — the quickest way to check what is actually running.

| Entity | Name | Payload / unit | Notes |
|---|---|---|---|
| `binary_sensor.z2m_guard_suppress` | Restart window | `ON` / `OFF` | `ON` = a suppression window is open right now |
| `sensor.z2m_guard_state` | State | `idle` · `suspect` · `planned` · `settling` · `outage_confirmed` | the full state machine, for visibility and debugging |
| `sensor.z2m_guard_last_restart` | Last suppressed downtime | seconds (`device_class: duration`) | attribute `reason`: `prior_signal`, `compose_recreate`, `crash`, `unknown` |

Availability: all three share `z2m_guard/availability` as their availability topic.
If the guard stops, they go `unavailable` — that is the intended signal, not a
problem to be worked around.

State topics, if you want to read them directly rather than through HA:

```
z2m_guard/suppress       ON | OFF      (retained)
z2m_guard/state          idle | suspect | planned | settling | outage_confirmed  (retained)
z2m_guard/last_restart   {"downtime_ms": 8727, "reason": "prior_signal"}          (retained)
```

## Recipe 1 — keep existing automations quiet

The most useful pattern is the smallest one: every alarm that watches Zigbee
availability gets one condition.

```yaml
- alias: Zigbee device offline
  trigger:
    - platform: state
      to: "unavailable"
      entity_id: !include ../../helpers/zigbee_entities.yaml
  condition:
    - condition: template
      value_template: "{{ is_state('binary_sensor.z2m_guard_suppress', 'off') }}"
  action:
    - service: notify.mobile_app_phone
      data:
        message: "{{ trigger.to_state.name }} is offline"
```

Nothing else changes: the automation keeps its trigger, and simply does not fire
while a planned restart is being attributed.

## Recipe 2 — one sensor that means "this one is real"

Instead of gating dozens of automations, gate **one** and point everything at it.
This is what `deploy/homeassistant/z2m_guard.yaml` contains:

```yaml
template:
  - binary_sensor:
      - name: "Zigbee outage unplanned"
        unique_id: zigbee_outage_unplanned
        device_class: problem
        state: >-
          {{ is_state('sensor.z2m_guard_state', 'outage_confirmed')
             and not is_state('binary_sensor.z2m_guard_suppress', 'on') }}
        attributes:
          guard_state: "{{ states('sensor.z2m_guard_state') }}"
          last_downtime: "{{ states('sensor.z2m_guard_last_restart') }}"
```

`outage_confirmed` only ever appears after the grace period expired without an
announcement, so this sensor is true exactly when an alarm is warranted.

## Recipe 3 — notify after an outage with the numbers attached

```yaml
- alias: Zigbee outage confirmed
  trigger:
    - platform: state
      entity_id: sensor.z2m_guard_state
      to: "outage_confirmed"
  action:
    - service: notify.mobile_app_phone
      data:
        title: "Zigbee outage"
        message: >-
          Zigbee2MQTT did not come back after
          {{ state_attr('sensor.z2m_guard_last_restart', 'reason') or 'an unattributed stop' }}.
          Last suppressed downtime: {{ states('sensor.z2m_guard_last_restart') }} s.
```

Because `last_restart` is retained and carries `reason`, this text survives a Home
Assistant restart in between.

## Recipe 4 — a dashboard line that explains itself

```yaml
type: markdown
content: >-
  {% set s = states('sensor.z2m_guard_state') %}
  **Guard:** {{ s }} ·
  window {{ 'open' if is_state('binary_sensor.z2m_guard_suppress','on') else 'closed' }} ·
  last restart {{ states('sensor.z2m_guard_last_restart') }} s
  ({{ state_attr('sensor.z2m_guard_last_restart','reason') }})
```

## Recipe 5 — run a post-restart task only after a *planned* restart

```yaml
- alias: After a planned Z2M restart
  trigger:
    - platform: state
      entity_id: sensor.z2m_guard_state
      from: "settling"
      to: "idle"
  action:
    - service: script.sync_zigbee_names
```

The `settling → idle` edge only happens when the bridge came back inside an
attributed window, so this never runs after a real outage.

## Removing the entities

Deleting the container leaves the retained discovery payloads behind and HA keeps
showing `unavailable` entities. Clear them explicitly:

```bash
for t in binary_sensor/z2m_guard/suppress sensor/z2m_guard/state sensor/z2m_guard/last_restart; do
  mosquitto_pub -h <broker> -u <user> -P '<password>' -r -n -t "homeassistant/$t/config"
done
```

Then restart Home Assistant, or reload the MQTT integration.