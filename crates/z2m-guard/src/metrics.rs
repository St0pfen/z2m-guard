//! Prometheus metrics, hand-rolled so the binary stays dependency-light.
//!
//! Only numbers the guard itself observes — no host metrics, no broker
//! instrumentation. That is the point: these series answer "did the guard work"
//! and nothing else.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use guard_core::Reason;

/// Fixed label order for `z2m_guard_suppressed_events_total{reason=...}`.
pub const REASONS: [&str; 4] = ["prior_signal", "compose_recreate", "crash", "unknown"];

fn index(reason: Reason) -> usize {
    match reason {
        Reason::PriorSignal => 0,
        Reason::ComposeRecreate => 1,
        Reason::Crash => 2,
        Reason::Unknown => 3,
    }
}

#[derive(Debug, Default)]
pub struct Metrics {
    suppressed: [AtomicU64; REASONS.len()],
    outage_confirmed: AtomicU64,
    flaps: AtomicU64,
    mqtt_bridge: AtomicU64,
    discovery_deduped: AtomicU64,
    window_active: AtomicBool,
    bridge_online_since_ms: AtomicU64,
}

impl Metrics {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record_suppressed(&self, reason: Reason) {
        self.suppressed[index(reason)].fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_outage_confirmed(&self) {
        self.outage_confirmed.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_flap(&self) {
        self.flaps.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_bridge_message(&self) {
        self.mqtt_bridge.fetch_add(1, Ordering::Relaxed);
    }

    /// Only incremented by the optional proxy mode (M5); kept here so the
    /// metric name is part of the contract from day one.
    #[allow(dead_code)]
    pub fn record_discovery_deduped(&self) {
        self.discovery_deduped.fetch_add(1, Ordering::Relaxed);
    }

    pub fn set_window_active(&self, active: bool) {
        self.window_active.store(active, Ordering::Relaxed);
    }

    /// Current suppression state — the runtime test asserts on this to prove a
    /// window closes without a reachable broker.
    pub fn window_active(&self) -> bool {
        self.window_active.load(Ordering::Relaxed)
    }

    pub fn set_bridge_online(&self, online: bool, now_ms: u64) {
        if online {
            self.bridge_online_since_ms.store(now_ms, Ordering::Relaxed);
        }
    }

    /// Prometheus text exposition format (version 0.0.4).
    pub fn render(&self) -> String {
        let mut s = String::new();
        s.push_str("# HELP z2m_guard_window_active 1 while a suppression window is open.\n");
        s.push_str("# TYPE z2m_guard_window_active gauge\n");
        s.push_str(&format!(
            "z2m_guard_window_active {}\n",
            u8::from(self.window_active())
        ));

        s.push_str("# HELP z2m_guard_suppressed_events_total Restart windows that were suppressed, by attribution.\n");
        s.push_str("# TYPE z2m_guard_suppressed_events_total counter\n");
        for (i, reason) in REASONS.iter().enumerate() {
            s.push_str(&format!(
                "z2m_guard_suppressed_events_total{{reason=\"{reason}\"}} {}\n",
                self.suppressed[i].load(Ordering::Relaxed)
            ));
        }

        s.push_str(
            "# HELP z2m_guard_outage_confirmed_total Unattributed outages that escalated.\n",
        );
        s.push_str("# TYPE z2m_guard_outage_confirmed_total counter\n");
        s.push_str(&format!(
            "z2m_guard_outage_confirmed_total {}\n",
            self.outage_confirmed.load(Ordering::Relaxed)
        ));

        s.push_str(
            "# HELP z2m_guard_flaps_total Availability flaps detected in the sliding window.\n",
        );
        s.push_str("# TYPE z2m_guard_flaps_total counter\n");
        s.push_str(&format!(
            "z2m_guard_flaps_total {}\n",
            self.flaps.load(Ordering::Relaxed)
        ));

        s.push_str("# HELP z2m_guard_mqtt_messages_total Bridge-state messages consumed.\n");
        s.push_str("# TYPE z2m_guard_mqtt_messages_total counter\n");
        s.push_str(&format!(
            "z2m_guard_mqtt_messages_total{{kind=\"bridge\"}} {}\n",
            self.mqtt_bridge.load(Ordering::Relaxed)
        ));

        s.push_str("# HELP z2m_guard_discovery_deduped_total Discovery payloads dropped as duplicates (proxy mode).\n");
        s.push_str("# TYPE z2m_guard_discovery_deduped_total counter\n");
        s.push_str(&format!(
            "z2m_guard_discovery_deduped_total {}\n",
            self.discovery_deduped.load(Ordering::Relaxed)
        ));

        s.push_str("# HELP z2m_guard_bridge_online_timestamp_seconds Unix-like timestamp (ms since guard start) of the last online transition.\n");
        s.push_str("# TYPE z2m_guard_bridge_online_timestamp_seconds gauge\n");
        s.push_str(&format!(
            "z2m_guard_bridge_online_timestamp_seconds {}\n",
            self.bridge_online_since_ms.load(Ordering::Relaxed)
        ));
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_every_series_with_help_and_type() {
        let m = Metrics::new();
        m.record_suppressed(Reason::ComposeRecreate);
        m.record_outage_confirmed();
        m.set_window_active(true);
        let text = m.render();

        assert!(text.contains("# TYPE z2m_guard_window_active gauge"));
        assert!(text.contains("z2m_guard_window_active 1"));
        assert!(text.contains("z2m_guard_suppressed_events_total{reason=\"compose_recreate\"} 1"));
        assert!(text.contains("z2m_guard_suppressed_events_total{reason=\"prior_signal\"} 0"));
        assert!(text.contains("z2m_guard_outage_confirmed_total 1"));
        for line in text.lines().filter(|l| !l.starts_with('#')) {
            assert!(
                line.split_once(' ').is_some(),
                "malformed exposition line: {line}"
            );
        }
    }

    #[test]
    fn counters_are_per_reason() {
        let m = Metrics::new();
        m.record_suppressed(Reason::PriorSignal);
        m.record_suppressed(Reason::PriorSignal);
        m.record_suppressed(Reason::Unknown);
        assert!(m.render().contains("{reason=\"prior_signal\"} 2"));
        assert!(m.render().contains("{reason=\"unknown\"} 1"));
    }
}
