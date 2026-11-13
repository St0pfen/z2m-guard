//! Optional transparent MQTT proxy (V2).
//!
//! Sits between Zigbee2MQTT and the broker to deduplicate discovery payload
//! storms and smooth availability flaps. Feature-gated because it turns the
//! guard into a hop in the Zigbee data path.

#![forbid(unsafe_code)]
