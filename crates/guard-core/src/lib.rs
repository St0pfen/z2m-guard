//! I/O-free core of `z2m-guard`.
//!
//! Everything here is pure: inputs go in with an explicit timestamp, outputs
//! come out. No clocks, no sockets, no files — so the tricky part (deciding
//! whether an outage was planned) is testable in microseconds.
//!
//! The safety property this crate must never violate: suppression is only
//! allowed when the outage is *attributable* (planned recreate, container
//! exit, pre-restart signal). An unexplained outage escalates instead of
//! being swallowed.

#![forbid(unsafe_code)]
