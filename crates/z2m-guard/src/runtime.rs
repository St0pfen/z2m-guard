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

    loop {
        let (client, mut eventloop) = match guard_mqtt::connect(&mqtt_cfg) {
            Ok(pair) => pair,
            Err(e) => {
                warn!(error = %e, attempt, "cannot build mqtt client");
                tokio::time::sleep(guard_mqtt::backoff(attempt)).await;
                attempt = attempt.saturating_add(1);
                continue;
            }
        };

        if let Err(e) = guard_mqtt::subscribe_bridge_state(&client, &mqtt_cfg).await {
            warn!(error = %e, "subscribe failed");
        }

        let mut ticker = interval(Duration::from_millis(cfg.guard.tick_ms.max(100)));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut connected_once = false;

        loop {
            tokio::select! {
                event = eventloop.poll() => {
                    match event {
                        Ok(MqttEvent::Incoming(Packet::ConnAck(_))) => {
                            attempt = 0;
                            connected_once = true;
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
                                    publish_outputs(&client, &mut guard, &metrics, &cfg, out, now).await;
                                } else {
                                    warn!(payload = %String::from_utf8_lossy(&publish.payload), "unparsable bridge state");
                                }
                            }
                        }
                        Ok(_) => {}
                        Err(e) => {
                            warn!(error = %e, connected_once, "mqtt connection ended");
                            break;
                        }
                    }
                }
                cmd = rx.recv() => {
                    match cmd {
                        Some(Command::PriorSignal { reason, window_ms }) => {
                            info!(reason = %reason, window_ms, "prior signal accepted");
                            let now = now_ms(&start);
                            let out = guard.step(
                                Input::PriorSignal { reason: Reason::PriorSignal, window_ms },
                                now,
                            );
                            publish_outputs(&client, &mut guard, &metrics, &cfg, out, now).await;
                        }
                        Some(Command::ContainerDie { exit_code }) => {
                            info!(exit_code, "container die event");
                            let now = now_ms(&start);
                            let out = guard.step(Input::ContainerDie { exit_code }, now);
                            publish_outputs(&client, &mut guard, &metrics, &cfg, out, now).await;
                        }
                        Some(Command::Shutdown) => {
                            // Availability is the MQTT last-will: once this
                            // process closes the socket the broker publishes
                            // "offline" itself. No flush dance needed.
                            info!("shutdown requested");
                            return Ok(());
                        }
                        None => {}
                    }
                }
                _ = ticker.tick() => {
                    let now = now_ms(&start);
                    let out = guard.step(Input::Tick, now);
                    publish_outputs(&client, &mut guard, &metrics, &cfg, out, now).await;
                }
            }
        }

        tokio::time::sleep(guard_mqtt::backoff(attempt)).await;
        attempt = attempt.saturating_add(1);
    }
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
    client: &AsyncClient,
    guard: &mut Guard,
    metrics: &Metrics,
    _cfg: &Config,
    outputs: Vec<Output>,
    now: u64,
) {
    for out in outputs {
        match out {
            Output::Suppression(on) => {
                metrics.set_window_active(on);
                publish_retained(
                    client,
                    &format!("{NAMESPACE}/suppress"),
                    if on { "ON" } else { "OFF" },
                )
                .await;
                log_event(json!({"kind": "suppression", "ts_ms": now, "on": on}));
            }
            Output::StateChanged(state) => {
                publish_retained(client, &format!("{NAMESPACE}/state"), state.as_str()).await;
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
                publish_retained(client, &format!("{NAMESPACE}/last_restart"), &payload).await;
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
