//! Events and outputs emitted by the guard.

/// Why a restart was considered planned (or not).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    /// An explicit pre-restart signal arrived (HTTP hook, compose wrapper).
    PriorSignal,
    /// The container exited with code 0 shortly before the bridge went away.
    ComposeRecreate,
    /// The container exited with a non-zero code: not a plan, a failure.
    Crash,
    /// No attribution available.
    Unknown,
}

impl Reason {
    pub fn as_str(self) -> &'static str {
        match self {
            Reason::PriorSignal => "prior_signal",
            Reason::ComposeRecreate => "compose_recreate",
            Reason::Crash => "crash",
            Reason::Unknown => "unknown",
        }
    }
}

/// Something worth telling the outside world about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// A restart window was closed and the bridge came back.
    RestartSuppressed { reason: Reason, downtime_ms: u64 },
    /// The bridge stayed away longer than the grace window without attribution.
    OutageConfirmed { offline_ms: u64 },
}

/// What `step` produces. Consumers may ignore anything they do not need.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Output {
    /// Emitted only when the value actually changes.
    Suppression(bool),
    /// Emitted only when the state actually changes.
    StateChanged(State),
    Event(Event),
}

use crate::state::State;
