# How it decides

The guard has one job: decide whether the bridge going away was **announced** or
**not**, and publish that decision as a suppression window with a deadline. Nothing
about it is heuristic in the sense of "usually works" — the whole thing is a small
state machine over two inputs.

## Two inputs

1. **The bridge topic** — `zigbee2mqtt/bridge/state` (`online` / `offline`, retained).
2. **An announcement** — either a signal before the restart (`POST /v1/restart-intent`,
   used by `deploy/restart-z2m.sh`), or a Docker event with exit code 0 that
   happened within `attribution_window_ms` of the flap (optional host mode).

Nothing else. No scanning of entity topics, no guessing from timing patterns.

## The states

| State | Meaning | Suppression |
|---|---|---|
| `idle` | bridge online, nothing to explain | off |
| `suspect` | bridge went away, nothing announced | **on**, but only until `grace_ms` (15 s) |
| `planned` | an announcement explains the outage | on until the announced deadline |
| `settling` | bridge is back, waiting for the mesh to stabilise | on until `settle_ms` (20 s) |
| `outage_confirmed` | grace expired without an announcement | **off** — the alarm passes |

Transitions, exactly:

| From | Input | To |
|---|---|---|
| `idle` | bridge offline | `suspect` |
| `suspect` | bridge online within grace | `idle` |
| `suspect` | grace deadline reached | `outage_confirmed` |
| `idle` / `suspect` | prior signal, or container exit 0 | `planned` |
| `planned` | bridge offline | `planned` |
| `planned` | bridge online | `settling` |
| `settling` | settle deadline reached | `idle` |
| `settling` | bridge offline again | `outage_confirmed` |

## Two timelines

**Planned recreate** (announced with 180 s):

```
t+0:00  restart-intent accepted          → planned,   suppress on
t+0:03  bridge offline                   → planned,   suppress on
t+0:12  bridge online                    → settling,  suppress on
t+0:32  settle deadline                  → idle,       suppress off
         last_restart = 8.7 s
```

**Unexplained stop:**

```
t+0:00  bridge offline                   → suspect,    suppress on until t+0:15
t+0:15  still gone                       → outage_confirmed, suppress off
         the alarm fires at t+0:15.5 instead of t+0:00
```

The second case is the point of the whole design: the guard **delays** an alarm by
grace, it does not swallow it, and the delay is bounded by a deadline that cannot
be extended by a crash loop or a stuck announcement.

## The safety property

> Suppression is never unbounded. Every window carries a deadline; when it passes
> without the bridge reporting online, the window closes and the state becomes
> `outage_confirmed`.

This is enforced structurally — a window is a timestamp, not a flag — and covered by
property tests over random event sequences: no sequence of inputs produces a
suppression that outlives its deadline. The concrete bound is
`prior_window_ms + settle_ms` in the announced case, `grace_ms` in the unexplained
case.

## What the guard deliberately does not do

- It cannot restart, stop or reconfigure Zigbee2MQTT. It subscribes and publishes to
  its own namespace, nothing else.
- It does not read the Docker socket (root-equivalent host access). Attribution uses
  the pre-restart script or piped `docker events` in host mode.
- It does not call Home Assistant. There is no REST call, no token, no integration —
  the entities are discovery payloads on MQTT.
- It keeps no state on disk. After a guard restart the entities read `idle` / `off`
  and `last_restart` starts over.

## Edge cases

| Situation | What happens |
|---|---|
| Two restarts back to back | the second announcement overwrites the deadline; the window is refreshed, never chained indefinitely |
| Z2M crash loop | exit code ≠ 0 means **no** attribution → `suspect` → `outage_confirmed` after grace. A crashing Z2M is a real outage |
| Guard restarts during an outage | state is in memory: the bridge goes offline again from the guard's point of view and is treated as unexplained unless a new announcement arrives |
| Broker unreachable | the guard reconnects with capped backoff (1 s → 30 s); while disconnected it cannot see bridge state, so no window is opened |
| Announcement, but nothing happens | the window expires at its deadline and the state returns to `idle`. A stale announcement never suppresses a later outage |
| Brief unexplained blip (< grace) | suppressed as `suspect`. That is intentional, and it is the only way a real outage of under 15 s stays hidden |

Measuring the real behaviour of your own stack: see
[Troubleshooting](Troubleshooting#does-it-actually-work-test-plan).