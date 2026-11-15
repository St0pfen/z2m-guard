//! Duplicate-discovery detection.
//!
//! Zigbee2MQTT republishes its whole discovery tree on every start. The payloads
//! are identical, and because they are retained the broker already holds them —
//! forwarding them again produces the message storm that shows up as a token bill
//! in Home Assistant. Dropping an identical retained payload inside the window is
//! therefore not "losing data": the broker's retained copy is the same bytes.
//!
//! Only retained publishes are eligible. A non-retained publish is a state change
//! and must always pass, even if it repeats the previous value.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::time::{Duration, Instant};

pub struct Dedupe {
    window: Duration,
    seen: HashMap<u64, Instant>,
}

impl Dedupe {
    pub fn new(window: Duration) -> Self {
        Self {
            window,
            seen: HashMap::new(),
        }
    }

    /// Returns `true` when the packet should be forwarded.
    ///
    /// `now` is passed in so tests do not have to sleep.
    pub fn should_forward(
        &mut self,
        topic: &str,
        payload: &[u8],
        retain: bool,
        now: Instant,
    ) -> bool {
        if !retain {
            return true;
        }

        // Cheap and good enough: collisions only mean an extra drop inside the
        // window, and the window is bounded by the same timings as a restart.
        let mut h = std::collections::hash_map::DefaultHasher::new();
        topic.hash(&mut h);
        payload.hash(&mut h);
        let key = h.finish();

        self.expire(now);

        match self.seen.get(&key) {
            Some(first) if now.duration_since(*first) < self.window => false,
            _ => {
                self.seen.insert(key, now);
                true
            }
        }
    }

    fn expire(&mut self, now: Instant) {
        let window = self.window;
        self.seen
            .retain(|_, first| now.duration_since(*first) < window);
    }

    pub fn len(&self) -> usize {
        self.seen.len()
    }

    pub fn is_empty(&self) -> bool {
        self.seen.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> Dedupe {
        Dedupe::new(Duration::from_secs(60))
    }

    #[test]
    fn identical_retained_payload_is_dropped_inside_the_window() {
        let mut d = cfg();
        let t0 = Instant::now();
        assert!(d.should_forward("homeassistant/sensor/x/config", b"{\"a\":1}", true, t0));
        assert!(!d.should_forward("homeassistant/sensor/x/config", b"{\"a\":1}", true, t0));
    }

    #[test]
    fn changed_payload_passes() {
        let mut d = cfg();
        let t0 = Instant::now();
        assert!(d.should_forward("homeassistant/sensor/x/config", b"{\"a\":1}", true, t0));
        assert!(d.should_forward("homeassistant/sensor/x/config", b"{\"a\":2}", true, t0));
    }

    #[test]
    fn non_retained_state_changes_always_pass() {
        let mut d = cfg();
        let t0 = Instant::now();
        assert!(d.should_forward("zigbee2mqtt/0x01/state", b"on", false, t0));
        assert!(d.should_forward("zigbee2mqtt/0x01/state", b"on", false, t0));
        assert!(
            d.is_empty(),
            "non-retained publishes must not be remembered"
        );
    }

    #[test]
    fn same_payload_passes_again_after_the_window() {
        let mut d = Dedupe::new(Duration::from_millis(50));
        let t0 = Instant::now();
        assert!(d.should_forward("homeassistant/sensor/x/config", b"{}", true, t0));
        assert!(!d.should_forward("homeassistant/sensor/x/config", b"{}", true, t0));
        assert!(d.should_forward(
            "homeassistant/sensor/x/config",
            b"{}",
            true,
            t0 + Duration::from_millis(80)
        ));
    }

    #[test]
    fn distinct_topics_do_not_collide() {
        let mut d = cfg();
        let t0 = Instant::now();
        assert!(d.should_forward("homeassistant/sensor/a/config", b"{}", true, t0));
        assert!(d.should_forward("homeassistant/sensor/b/config", b"{}", true, t0));
        assert_eq!(d.len(), 2);
    }
}
