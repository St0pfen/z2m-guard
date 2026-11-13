//! MQTT transport: subscribes to `zigbee2mqtt/bridge/state`, publishes
//! guard-owned topics, handles reconnect with backoff.

#![forbid(unsafe_code)]
