//! Optional Docker event attribution.
//!
//! Only useful when the guard may read the Docker socket; the core state
//! machine works without it and falls back to MQTT heuristics. The feature is
//! named explicitly so that enabling it is a conscious security decision.

#![forbid(unsafe_code)]
