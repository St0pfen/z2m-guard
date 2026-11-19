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
    bridge_online: AtomicBool,
    bridge_last_online_s: AtomicU64,
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

    /// Set from the proxy's own counter (the proxy owns the tally, `/metrics`
    /// mirrors it once a second).
    #[cfg_attr(not(feature = "proxy"), allow(dead_code))]
    pub fn set_discovery_deduped(&self, total: u64) {
        self.discovery_deduped.store(total, Ordering::Relaxed);
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
        self.bridge_online.store(online, Ordering::Relaxed);
        if online {
            self.bridge_last_online_s
                .store(now_ms / 1_000, Ordering::Relaxed);
        }
    }

    /// Whether the last bridge-state message said `online`. Defaults to `false`
    /// until the retained state arrives, so a guard that never heard from the
    /// bridge does not claim it is healthy.
    pub fn bridge_online(&self) -> bool {
        self.bridge_online.load(Ordering::Relaxed)
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

        s.push_str("# HELP z2m_guard_bridge_online 1 while the bridge last reported online, 0 otherwise.\n");
        s.push_str("# TYPE z2m_guard_bridge_online gauge\n");
        s.push_str(&format!(
            "z2m_guard_bridge_online {}\n",
            u8::from(self.bridge_online())
        ));

        // Seconds, not milliseconds: the name says `_seconds`, and a series whose
        // unit disagrees with its suffix is a trap for whoever writes the query.
        // The value is still an offset since guard start, not a Unix timestamp —
        // use it to compare against other guard-local timings.
        s.push_str("# HELP z2m_guard_bridge_last_online_seconds Seconds since guard start at the last online transition.\n");
        s.push_str("# TYPE z2m_guard_bridge_last_online_seconds gauge\n");
        s.push_str(&format!(
            "z2m_guard_bridge_last_online_seconds {}\n",
            self.bridge_last_online_s.load(Ordering::Relaxed)
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
    fn bridge_online_gauge_follows_the_last_message() {
        let m = Metrics::new();
        // Retained state has not arrived yet: must not claim healthy.
        assert!(m.render().contains("z2m_guard_bridge_online 0"));

        m.set_bridge_online(true, 91_500);
        let text = m.render();
        assert!(text.contains("z2m_guard_bridge_online 1"));
        // 91500 ms is 91 s, and the old name lied about the unit.
        assert!(
            text.contains("z2m_guard_bridge_last_online_seconds 91"),
            "timestamp must be seconds, got: {text}"
        );

        m.set_bridge_online(false, 200_000);
        assert!(m.render().contains("z2m_guard_bridge_online 0"));
        // Offline must not move the marker; it records the last online transition.
        assert!(m
            .render()
            .contains("z2m_guard_bridge_last_online_seconds 91"));
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
