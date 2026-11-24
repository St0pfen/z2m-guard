//! The wiring: MQTT in, state machine, MQTT/HA/metrics out.

use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use guard_core::{Event, FlapDetector, Guard, Input, Output, Reason};
use guard_ha::{Discovery, HaMeta};
use rumqttc::{AsyncClient, Event as MqttEvent, Packet, QoS};
use serde_json::json;
use tokio::sync::mpsc;
use tokio::time::interval;
use tracing::{info, warn};

use crate::config::Config;
use crate::http::Command;
use crate::metrics::Metrics;

const NAMESPACE: &str = guard_ha::NAMESPACE;

/// Where the flap detector looks: 3 toggles inside 60 s.
const FLAP_WINDOW_MS: u64 = 60_000;
const FLAP_THRESHOLD: u32 = 3;

pub async fn run(
    cfg: Config,
    metrics: Arc<Metrics>,
    mut rx: mpsc::Receiver<Command>,
) -> Result<()> {
    let mqtt_cfg = cfg.mqtt_config();
    let meta = HaMeta {
        discovery_prefix: cfg.homeassistant.discovery_prefix.clone(),
        device_name: cfg.homeassistant.device_name.clone(),
        ..HaMeta::default()
    };
    let start = Instant::now();
    let mut guard = Guard::new(cfg.timings());
    let mut flap = FlapDetector::new(FLAP_WINDOW_MS, FLAP_THRESHOLD);
    let mut attempt: u32 = 0;

    info!(
        broker = %mqtt_cfg.url,
        base_topic = %mqtt_cfg.base_topic,
        discovery = cfg.homeassistant.discovery,
        "starting"
    );

    // The ticker is created once, outside the connection loop, and the reconnect
    // pause serves ticks and intents as well. Windows must expire on wall-clock
    // time, not on broker availability: with the ticker inside the connection
    // loop a refused connection tore that loop down and `Guard::expire()` never
    // ran, leaving a suppression open for the whole outage.
    let mut ticker = interval(Duration::from_millis(cfg.guard.tick_ms.max(100)));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        match guard_mqtt::connect(&mqtt_cfg) {
            Ok((client, mut eventloop)) => {
                if let Err(e) = guard_mqtt::subscribe_bridge_state(&client, &mqtt_cfg).await {
                    warn!(error = %e, "subscribe failed");
                }
                let mut connected_once = false;

                loop {
                    tokio::select! {
                        event = eventloop.poll() => {
                            match event {
                                Ok(MqttEvent::Incoming(Packet::ConnAck(_))) => {
                                    attempt = 0;
                                    connected_once = true;
                                    metrics.set_mqtt_connected(true);
                                    info!("mqtt connected");
                                    publish_connected(&client, &cfg, &meta, &guard, &start).await;
                                }
                                Ok(MqttEvent::Incoming(Packet::Publish(publish))) => {
                                    if publish.topic == mqtt_cfg.bridge_state_topic() {
                                        metrics.record_bridge_message();
                                        if let Some(online) = guard_mqtt::parse_bridge_state(&publish.payload) {
                                            let now = now_ms(&start);
                                            metrics.set_bridge_online(online, now);
                                            let input = if online { Input::BridgeOnline } else { Input::BridgeOffline };
                                            // Attributed restarts are already reported as
                                            // restart_suppressed; counting them as flaps too
                                            // would double-report every deployment.
                                            if !guard.is_attributed() && flap.observe(now) {
                                                metrics.record_flap();
                                                log_event(json!({"kind": "flap", "ts_ms": now}));
                                            }
                                            let out = guard.step(input, now);
                                            publish_outputs(Some(&client), &mut guard, &metrics, out, now).await;
                                        } else {
                                            warn!(payload = %String::from_utf8_lossy(&publish.payload), "unparsable bridge state");
                                        }
                                    }
                                }
                                Ok(_) => {}
                                Err(e) => {
                                    warn!(error = %e, connected_once, "mqtt connection ended");
                                    metrics.set_mqtt_connected(false);
                                    break;
                                }
                            }
                        }
                        cmd = rx.recv() => {
                            if handle_command(cmd, Some(&client), &mut guard, &metrics, &start).await
                                == Flow::Stop
                            {
                                return Ok(());
                            }
                        }
                        _ = ticker.tick() => {
                            handle_tick(Some(&client), &mut guard, &metrics, &start).await;
                        }
                    }
                }
            }
            Err(e) => {
                // No client at all: `/readyz` must not stay `200` from a previous
                // connection while the daemon cannot subscribe to anything.
                metrics.set_mqtt_connected(false);
                warn!(error = %e, attempt, "cannot build mqtt client");
            }
        }

        // Reconnect pause. There is no client to publish through, but ticks and
        // intents keep driving the state machine (metrics and the JSON-lines log
        // still move); `publish_connected` republishes the state to the broker
        // once the next connection is up, so nothing is lost.
        let deadline = Instant::now() + guard_mqtt::backoff(attempt);
        attempt = attempt.saturating_add(1);
        loop {
            tokio::select! {
                cmd = rx.recv() => {
                    if handle_command(cmd, None, &mut guard, &metrics, &start).await == Flow::Stop {
                        return Ok(());
                    }
                }
                _ = ticker.tick() => {
                    handle_tick(None, &mut guard, &metrics, &start).await;
                }
                _ = tokio::time::sleep_until(deadline.into()) => break,
            }
        }
    }
}

/// Whether the runtime should keep going.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Flow {
    Continue,
    Stop,
}

/// One intent from the HTTP layer.
///
/// `client` is `None` while the daemon is reconnecting: the state machine is
/// authoritative and advances regardless, only the publishes are skipped.
async fn handle_command(
    cmd: Option<Command>,
    client: Option<&AsyncClient>,
    guard: &mut Guard,
    metrics: &Metrics,
    start: &Instant,
) -> Flow {
    match cmd {
        Some(Command::PriorSignal { reason, window_ms }) => {
            info!(reason = %reason, window_ms, "prior signal accepted");
            let now = now_ms(start);
            let out = guard.step(
                Input::PriorSignal {
                    reason: Reason::PriorSignal,
                    window_ms,
                },
                now,
            );
            publish_outputs(client, guard, metrics, out, now).await;
        }
        Some(Command::ContainerDie { exit_code }) => {
            info!(exit_code, "container die event");
            let now = now_ms(start);
            let out = guard.step(Input::ContainerDie { exit_code }, now);
            publish_outputs(client, guard, metrics, out, now).await;
        }
        Some(Command::Shutdown) => {
            // Availability is the MQTT last-will: once this process closes the
            // socket the broker publishes "offline" itself. No flush dance needed.
            info!("shutdown requested");
            return Flow::Stop;
        }
        None => {}
    }
    Flow::Continue
}

/// One clock step: the only thing that makes a window/deadline expire.
async fn handle_tick(
    client: Option<&AsyncClient>,
    guard: &mut Guard,
    metrics: &Metrics,
    start: &Instant,
) {
    let now = now_ms(start);
    let out = guard.step(Input::Tick, now);
    publish_outputs(client, guard, metrics, out, now).await;
}

fn now_ms(start: &Instant) -> u64 {
    u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX)
}

async fn publish_connected(
    client: &AsyncClient,
    cfg: &Config,
    meta: &HaMeta,
    guard: &Guard,
    start: &Instant,
) {
    // Availability first: HA ignores entities whose availability topic is dead.
    publish_retained(client, &format!("{NAMESPACE}/availability"), "online").await;

    if cfg.homeassistant.discovery {
        for d in guard_ha::all(meta) {
            publish_discovery(client, &d).await;
        }
    }

    publish_retained(
        client,
        &format!("{NAMESPACE}/state"),
        guard.state().as_str(),
    )
    .await;
    publish_retained(
        client,
        &format!("{NAMESPACE}/suppress"),
        if guard.suppression() { "ON" } else { "OFF" },
    )
    .await;

    let now = now_ms(start);
    log_event(json!({"kind": "connected", "ts_ms": now, "state": guard.state().as_str()}));
}

async fn publish_discovery(client: &AsyncClient, d: &Discovery) {
    if let Err(e) = client
        .publish(&d.topic, QoS::AtLeastOnce, true, d.payload.as_str())
        .await
    {
        warn!(error = %e, topic = %d.topic, "discovery publish failed");
    }
}

async fn publish_retained(client: &AsyncClient, topic: &str, payload: &str) {
    if let Err(e) = client.publish(topic, QoS::AtLeastOnce, true, payload).await {
        warn!(error = %e, topic, "publish failed");
    }
}

async fn publish_outputs(
    client: Option<&AsyncClient>,
    guard: &mut Guard,
    metrics: &Metrics,
    outputs: Vec<Output>,
    now: u64,
) {
    for out in outputs {
        match out {
            Output::Suppression(on) => {
                metrics.set_window_active(on);
                if let Some(client) = client {
                    publish_retained(
                        client,
                        &format!("{NAMESPACE}/suppress"),
                        if on { "ON" } else { "OFF" },
                    )
                    .await;
                }
                log_event(json!({"kind": "suppression", "ts_ms": now, "on": on}));
            }
            Output::StateChanged(state) => {
                if let Some(client) = client {
                    publish_retained(client, &format!("{NAMESPACE}/state"), state.as_str()).await;
                }
                log_event(json!({"kind": "state", "ts_ms": now, "state": state.as_str()}));
            }
            Output::Event(Event::RestartSuppressed {
                reason,
                downtime_ms,
            }) => {
                metrics.record_suppressed(reason);
                let payload = json!({
                    "downtime_ms": downtime_ms,
                    "reason": reason.as_str(),
                    "ts_ms": now,
                })
                .to_string();
                if let Some(client) = client {
                    publish_retained(client, &format!("{NAMESPACE}/last_restart"), &payload).await;
                }
                log_event(json!({
                    "kind": "restart_suppressed",
                    "ts_ms": now,
                    "reason": reason.as_str(),
                    "downtime_ms": downtime_ms,
                }));
            }
            Output::Event(Event::OutageConfirmed { offline_ms }) => {
                metrics.record_outage_confirmed();
                log_event(json!({
                    "kind": "outage_confirmed",
                    "ts_ms": now,
                    "offline_ms": offline_ms,
                }));
            }
        }
    }
    // Keep the gauge honest even if no Suppression output was emitted.
    metrics.set_window_active(guard.suppression());
}

/// JSON-Lines event contract: one object per line on stdout.
fn log_event(event: serde_json::Value) {
    println!("{event}");
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Nothing listens on port 1, so every connection attempt is refused — the
    /// exact condition that used to freeze the ticker.
    fn unreachable_broker_cfg() -> Config {
        let mut cfg = Config::default();
        cfg.mqtt.url = "mqtt://127.0.0.1:1".to_string();
        cfg.guard.tick_ms = 50;
        cfg.guard.prior_window_ms = 200;
        cfg.guard.settle_ms = 200;
        cfg.homeassistant.discovery = false;
        cfg
    }

    async fn wait_until(mut check: impl FnMut() -> bool, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if check() {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        check()
    }

    #[tokio::test]
    async fn readiness_stays_false_while_the_broker_is_unreachable() {
        let metrics = Arc::new(Metrics::new());
        let (tx, rx) = mpsc::channel(8);
        let task = tokio::spawn(run(unreachable_broker_cfg(), Arc::clone(&metrics), rx));

        // Several connection attempts and ticks happen within this window; none of
        // them may flip the readiness gauge, otherwise a container reports healthy
        // while no bridge state is being observed.
        tokio::time::sleep(Duration::from_millis(400)).await;
        assert!(!metrics.mqtt_connected(), "readiness lied about the broker");

        tx.send(Command::Shutdown).await.expect("send shutdown");
        task.await
            .expect("runtime task panicked")
            .expect("runtime returned an error");
    }

    #[tokio::test]
    async fn suppression_expires_while_the_broker_is_unreachable() {
        let metrics = Arc::new(Metrics::new());
        let (tx, rx) = mpsc::channel(8);
        let task = tokio::spawn(run(unreachable_broker_cfg(), Arc::clone(&metrics), rx));

        tx.send(Command::PriorSignal {
            reason: "test".into(),
            window_ms: 200,
        })
        .await
        .expect("send prior signal");

        assert!(
            wait_until(|| metrics.window_active(), Duration::from_secs(3)).await,
            "the intent never opened a window"
        );
        // The regression this test exists for: 200 ms window, 50 ms tick, no
        // broker. The window must close anyway.
        assert!(
            wait_until(|| !metrics.window_active(), Duration::from_secs(3)).await,
            "suppression stayed open while the broker was unreachable"
        );

        tx.send(Command::Shutdown).await.expect("send shutdown");
        task.await
            .expect("runtime task panicked")
            .expect("runtime returned an error");
    }
}
