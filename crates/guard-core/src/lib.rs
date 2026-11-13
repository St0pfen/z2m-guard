//! I/O-free core of `z2m-guard`.
//!
//! Everything here is pure: inputs go in with an explicit timestamp, outputs
//! come out. No clocks, no sockets, no files — so the tricky part (deciding
//! whether an outage was planned) is testable in microseconds.
//!
//! The safety property this crate must never violate: suppression is bounded.
//! An outage that nobody asked for escalates to [`State::OutageConfirmed`]
//! after the grace window instead of being swallowed.

#![forbid(unsafe_code)]

pub mod events;
pub mod flap;
pub mod state;

pub use events::{Event, Output, Reason};
pub use flap::FlapDetector;
pub use state::{Guard, Input, State, Timings};
