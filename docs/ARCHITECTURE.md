# Architecture

```
                 ┌──────────────────────┐
zigbee2mqtt ────►│ zigbee2mqtt/bridge/  │ retained {"state":"online"|"offline"}
                 │ state                │
                 └──────────┬───────────┘
                            │ subscribe
┌───────────────────────────▼─────────────────────────────┐
│ z2m-guard                                               │
│                                                         │
│  guard-core (pure)        guard-mqtt       guard-ha      │
│  ┌──────────────────┐    ┌───────────┐   ┌───────────┐  │
│  │ Guard::step      │    │ rumqttc   │   │ discovery │  │
│  │ Idle             │    │ backoff   │   │ payloads  │  │
│  │ Suspect ──grace──┤    │ last will │   │ golden    │  │
│  │ Planned          │    └───────────┘   └───────────┘  │
│  │ Settling         │         ▲                          │
│  │ OutageConfirmed  │         │                          │
│  └────────┬─────────┘    guard-docker (optional)          │
│           │              die/start attribution           │
│           ▼                                              │
│  HTTP /healthz /readyz /metrics POST /v1/restart-intent  │
└──────────────────────────────────────────────────────────┘
```

## Decisions that matter

**The core crate never touches I/O.** `Guard::step(input, now_ms)` returns
outputs; the binary owns the clock, the broker and the socket. That is what makes
the suppression logic provable with property tests over random event sequences
instead of hoping a 9-second container restart behaves like the test.

**Observer, not proxy.** v1 only subscribes. It cannot delay, drop or rewrite
Zigbee traffic, so a guard bug is a wrong alarm — not a broken house. The proxy
mode (deduplicating discovery republishes) is opt-in and comes later, because it
sits in the data path and is therefore a single point of failure.

**Suppression is bounded, not attributed.** Grace and settle windows suppress
without knowing why, because the first seconds of a routine recreate are exactly
what must not alarm. The invariant tested is: every suppression carries a
deadline, and a *confirmed* outage is never suppressed.

**Availability via last will, not shutdown code.** `publish().await` only enqueues;
a shutdown branch inside `select!` never flushes it, and SIGKILL does not
cooperate at all. The broker publishes `offline` itself when the socket drops.

**No Docker socket in v1.** The socket is root-equivalent host access. Attribution
comes from an explicit pre-restart signal (`deploy/restart-z2m.sh`) or from
`docker events` lines piped on stdin — both privilege-free. The socket path stays
a documented, feature-gated option for later.

## State machine

| From | Input | To | Suppression |
|---|---|---|---|
| Idle | bridge offline | Suspect | on until `t+grace` |
| Suspect | bridge online within grace | Idle | off |
| Suspect | grace deadline | OutageConfirmed | off (alarm passes) |
| Idle/Suspect | prior signal / container exit 0 | Planned | on until deadline |
| Planned | bridge offline | Planned | on |
| Planned | bridge online | Settling | on until `t+settle` |
| Settling | settle deadline | Idle | off |
| Settling | bridge offline | OutageConfirmed | off |

Defaults: grace 15 s, settle 20 s, attribution window 30 s, prior window 120 s
(180 s in the deployed config), tick 1 s.
