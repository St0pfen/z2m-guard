//! Sliding-window flap detection.
//!
//! Independent of the restart state machine: a flapping device or a bridge that
//! keeps reconnecting is worth its own signal even when no restart happened.

use std::collections::VecDeque;

/// Counts availability toggles inside a sliding window.
#[derive(Debug, Clone)]
pub struct FlapDetector {
    window_ms: u64,
    threshold: u32,
    toggles: VecDeque<u64>,
}

impl FlapDetector {
    pub fn new(window_ms: u64, threshold: u32) -> Self {
        Self {
            window_ms,
            threshold: threshold.max(1),
            toggles: VecDeque::new(),
        }
    }

    /// Records one toggle at `now_ms` and reports whether the threshold is met.
    pub fn observe(&mut self, now_ms: u64) -> bool {
        self.toggles.push_back(now_ms);
        self.prune(now_ms);
        self.toggles.len() >= self.threshold as usize
    }

    /// Toggles currently inside the window.
    pub fn toggles_in_window(&mut self, now_ms: u64) -> u32 {
        self.prune(now_ms);
        self.toggles.len() as u32
    }

    fn prune(&mut self, now_ms: u64) {
        while let Some(front) = self.toggles.front().copied() {
            if now_ms.saturating_sub(front) > self.window_ms {
                self.toggles.pop_front();
            } else {
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn three_toggles_inside_window_is_flap() {
        let mut d = FlapDetector::new(60_000, 3);
        assert!(!d.observe(1_000));
        assert!(!d.observe(2_000));
        assert!(d.observe(3_000));
    }

    #[test]
    fn two_toggles_is_not_flap() {
        let mut d = FlapDetector::new(60_000, 3);
        assert!(!d.observe(1_000));
        assert!(!d.observe(2_000));
        assert_eq!(d.toggles_in_window(2_000), 2);
    }

    #[test]
    fn toggles_outside_window_age_out() {
        let mut d = FlapDetector::new(60_000, 3);
        d.observe(0);
        d.observe(10_000);
        // 70s later both earlier toggles are older than the window.
        assert!(!d.observe(70_500), "aged-out toggles must not count");
        assert_eq!(d.toggles_in_window(70_500), 1);
    }

    #[test]
    fn threshold_is_clamped_to_at_least_one() {
        let mut d = FlapDetector::new(1_000, 0);
        assert!(d.observe(0));
    }
}
