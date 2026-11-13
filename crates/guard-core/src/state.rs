//! The state machine: planned restart vs. real outage.
//!
//! Time is always passed in as `now_ms`; nothing here reads a clock. That is
//! what makes the interesting cases testable without waiting.

use crate::events::{Event, Output, Reason};

/// Where the guard currently stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Idle,
    /// Bridge went away, nobody asked for it, grace window running.
    Suspect,
    /// Bridge went away and we can explain it.
    Planned,
    /// Bridge is back; holding suppression while the mesh settles.
    Settling,
    /// Grace expired while the bridge was still gone: a real outage.
    OutageConfirmed,
}

impl State {
    pub fn as_str(self) -> &'static str {
        match self {
            State::Idle => "idle",
            State::Suspect => "suspect",
            State::Planned => "planned",
            State::Settling => "settling",
            State::OutageConfirmed => "outage_confirmed",
        }
    }
}

/// One thing that happened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Input {
    /// Something told us a restart is coming (HTTP hook, compose wrapper).
    PriorSignal { reason: Reason, window_ms: u64 },
    /// `zigbee2mqtt/bridge/state` went to `offline` (or the retained value was lost).
    BridgeOffline,
    /// `zigbee2mqtt/bridge/state` went to `online`.
    BridgeOnline,
    /// Optional Docker attribution.
    ContainerDie { exit_code: i32 },
    /// Periodic tick so windows can expire without other input.
    Tick,
}

/// Window lengths. Defaults are grounded in a measured ~9 s flap on the
/// reference host: settle = 2x that with margin, grace bounds how long an
/// unexplained outage may stay suppressed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timings {
    pub grace_ms: u64,
    pub settle_ms: u64,
    pub attribution_window_ms: u64,
    pub prior_window_ms: u64,
}

impl Default for Timings {
    fn default() -> Self {
        Self {
            grace_ms: 15_000,
            settle_ms: 20_000,
            attribution_window_ms: 30_000,
            prior_window_ms: 120_000,
        }
    }
}

/// The guard.
#[derive(Debug, Clone)]
pub struct Guard {
    timings: Timings,
    state: State,
    suppression: bool,
    bridge_online: bool,
    offline_since: Option<u64>,
    attribution: Option<Reason>,
    planned_cap: Option<u64>,
    suspect_deadline: Option<u64>,
    settle_deadline: Option<u64>,
    last_die: Option<(u64, i32)>,
}

impl Guard {
    pub fn new(timings: Timings) -> Self {
        Self {
            timings,
            state: State::Idle,
            suppression: false,
            bridge_online: true,
            offline_since: None,
            attribution: None,
            planned_cap: None,
            suspect_deadline: None,
            settle_deadline: None,
            last_die: None,
        }
    }

    pub fn state(&self) -> State {
        self.state
    }

    pub fn suppression(&self) -> bool {
        self.suppression
    }

    pub fn attribution(&self) -> Option<Reason> {
        self.attribution
    }

    /// True when the outage currently in progress can be explained.
    pub fn is_attributed(&self) -> bool {
        self.attribution.is_some()
    }

    /// True when an active suppression cannot outlive a deadline.
    ///
    /// This is the invariant that makes the tool trustworthy: suppression may
    /// run without attribution (during grace or the settle window), but never
    /// without a clock attached to it.
    pub fn suppression_is_bounded(&self) -> bool {
        !self.suppression
            || self.planned_cap.is_some()
            || self.suspect_deadline.is_some()
            || self.settle_deadline.is_some()
    }

    /// Feeds one input. Returns everything a consumer may want to publish.
    pub fn step(&mut self, input: Input, now_ms: u64) -> Vec<Output> {
        let mut out = Vec::new();
        self.expire(now_ms, &mut out);

        match input {
            Input::PriorSignal { reason, window_ms } => {
                self.on_prior_signal(reason, window_ms, now_ms, &mut out)
            }
            Input::BridgeOffline => self.on_bridge_offline(now_ms, &mut out),
            Input::BridgeOnline => self.on_bridge_online(now_ms, &mut out),
            Input::ContainerDie { exit_code } => {
                self.last_die = Some((now_ms, exit_code));
            }
            Input::Tick => {}
        }

        out
    }

    fn on_prior_signal(
        &mut self,
        reason: Reason,
        window_ms: u64,
        now_ms: u64,
        out: &mut Vec<Output>,
    ) {
        let window = if window_ms == 0 {
            self.timings.prior_window_ms
        } else {
            window_ms
        };
        self.attribution = Some(reason);
        self.planned_cap = Some(now_ms.saturating_add(window));
        self.transition(State::Planned, out);
        self.set_suppression(true, out);
    }

    fn on_bridge_offline(&mut self, now_ms: u64, out: &mut Vec<Output>) {
        self.bridge_online = false;
        if self.offline_since.is_none() {
            self.offline_since = Some(now_ms);
        }

        match self.state {
            // Already attributable: keep the window we have.
            State::Planned => {
                self.set_suppression(true, out);
            }
            // A new outage while settling is a new question: re-evaluate.
            State::Settling | State::Idle => self.attribute_or_suspect(now_ms, out),
            State::Suspect | State::OutageConfirmed => {}
        }
    }

    fn attribute_or_suspect(&mut self, now_ms: u64, out: &mut Vec<Output>) {
        // A container exit explains exactly one outage; never let a stale
        // die event cover the next one.
        let die = self.last_die.take();
        match die {
            Some((died_at, exit_code))
                if now_ms.saturating_sub(died_at) <= self.timings.attribution_window_ms =>
            {
                if exit_code == 0 {
                    self.attribution = Some(Reason::ComposeRecreate);
                    self.planned_cap = Some(now_ms.saturating_add(self.timings.prior_window_ms));
                    self.transition(State::Planned, out);
                    self.set_suppression(true, out);
                } else {
                    // A crash is not a plan: bounded suppression, then escalate.
                    self.attribution = Some(Reason::Crash);
                    self.suspect_deadline = Some(now_ms.saturating_add(self.timings.grace_ms));
                    self.transition(State::Suspect, out);
                    self.set_suppression(true, out);
                }
            }
            _ => {
                self.attribution = None;
                self.suspect_deadline = Some(now_ms.saturating_add(self.timings.grace_ms));
                self.transition(State::Suspect, out);
                self.set_suppression(true, out);
            }
        }
    }

    fn on_bridge_online(&mut self, now_ms: u64, out: &mut Vec<Output>) {
        self.bridge_online = true;
        let offline_ms = self
            .offline_since
            .map(|t| now_ms.saturating_sub(t))
            .unwrap_or(0);
        self.offline_since = None;

        match self.state {
            State::Planned | State::Suspect => {
                out.push(Output::Event(Event::RestartSuppressed {
                    reason: self.attribution.unwrap_or(Reason::Unknown),
                    downtime_ms: offline_ms,
                }));
                self.transition(State::Settling, out);
                self.settle_deadline = Some(now_ms.saturating_add(self.timings.settle_ms));
                self.set_suppression(true, out);
            }
            State::OutageConfirmed => {
                self.transition(State::Settling, out);
                self.settle_deadline = Some(now_ms.saturating_add(self.timings.settle_ms));
                self.set_suppression(true, out);
            }
            State::Settling => {
                self.settle_deadline = Some(now_ms.saturating_add(self.timings.settle_ms));
                self.set_suppression(true, out);
            }
            State::Idle => {}
        }
    }

    fn expire(&mut self, now_ms: u64, out: &mut Vec<Output>) {
        if let Some(cap) = self.planned_cap {
            if now_ms >= cap && self.state == State::Planned {
                self.planned_cap = None;
                if self.bridge_online {
                    self.transition(State::Idle, out);
                    self.set_suppression(false, out);
                } else {
                    // The plan window passed and the bridge is still away:
                    // stop pretending we know why. The grace window started
                    // when the plan window ended, not when this tick arrived —
                    // otherwise a late tick would restart the clock and delay
                    // escalation.
                    self.attribution = None;
                    self.suspect_deadline = Some(cap.saturating_add(self.timings.grace_ms));
                    self.transition(State::Suspect, out);
                    self.set_suppression(true, out);
                }
            }
        }

        if let Some(deadline) = self.suspect_deadline {
            if now_ms >= deadline && self.state == State::Suspect && !self.bridge_online {
                self.suspect_deadline = None;
                let offline_ms = self
                    .offline_since
                    .map(|t| now_ms.saturating_sub(t))
                    .unwrap_or(0);
                self.transition(State::OutageConfirmed, out);
                self.set_suppression(false, out);
                out.push(Output::Event(Event::OutageConfirmed { offline_ms }));
            }
        }

        if let Some(deadline) = self.settle_deadline {
            if now_ms >= deadline && self.state == State::Settling {
                self.settle_deadline = None;
                self.transition(State::Idle, out);
                self.set_suppression(false, out);
            }
        }
    }

    fn transition(&mut self, next: State, out: &mut Vec<Output>) {
        if self.state == next {
            return;
        }

        match self.state {
            State::Planned => self.planned_cap = None,
            State::Suspect => self.suspect_deadline = None,
            State::Settling => self.settle_deadline = None,
            State::Idle | State::OutageConfirmed => {}
        }

        if next == State::Idle {
            self.attribution = None;
            self.planned_cap = None;
            self.suspect_deadline = None;
            self.settle_deadline = None;
        }

        self.state = next;
        out.push(Output::StateChanged(next));
    }

    fn set_suppression(&mut self, on: bool, out: &mut Vec<Output>) {
        if self.suppression != on {
            self.suppression = on;
            out.push(Output::Suppression(on));
        }
    }
}
