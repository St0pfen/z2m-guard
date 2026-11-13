//! `z2m-guard` binary — wiring of the crates: config, MQTT, HA discovery,
//! metrics, restart signals.

#![forbid(unsafe_code)]

fn main() {
    println!("z2m-guard {}", env!("CARGO_PKG_VERSION"));
}
