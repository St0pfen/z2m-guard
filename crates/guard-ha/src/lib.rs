//! Home Assistant MQTT discovery payloads (`binary_sensor` + `sensor`).
//!
//! Everything lives under the `z2m_guard/` topic namespace. The guard never
//! writes into `zigbee2mqtt/#` or into another integration's discovery topics —
//! a second publisher on those would race with the original owner.
//!
//! Payload shapes are locked down by golden-file tests in `tests/`.

#![forbid(unsafe_code)]

use serde::Serialize;

/// Topic namespace owned by the guard.
pub const NAMESPACE: &str = "z2m_guard";

/// Device-independent settings for the discovery payloads.
#[derive(Debug, Clone)]
pub struct HaMeta {
    pub discovery_prefix: String,
    pub node_id: String,
    pub device_name: String,
    pub sw_version: String,
    pub support_url: String,
}

impl Default for HaMeta {
    fn default() -> Self {
        Self {
            discovery_prefix: "homeassistant".to_string(),
            node_id: "z2m_guard".to_string(),
            device_name: "Z2M Guard".to_string(),
            sw_version: env!("CARGO_PKG_VERSION").to_string(),
            support_url: "https://github.com/St0pfen/z2m-guard".to_string(),
        }
    }
}

impl HaMeta {
    fn config_topic(&self, component: &str, object_id: &str) -> String {
        format!(
            "{}/{}/{}/{}/config",
            self.discovery_prefix, component, self.node_id, object_id
        )
    }

    fn device(&self) -> Device {
        Device {
            identifiers: vec![self.node_id.clone()],
            name: self.device_name.clone(),
            manufacturer: "z2m-guard".to_string(),
            model: "restart watchdog".to_string(),
            sw_version: self.sw_version.clone(),
        }
    }

    fn origin(&self) -> Origin {
        Origin {
            name: "z2m-guard".to_string(),
            sw_version: self.sw_version.clone(),
            support_url: self.support_url.clone(),
        }
    }
}

/// One retained discovery message to publish.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Discovery {
    pub topic: String,
    pub payload: String,
}

#[derive(Debug, Clone, Serialize)]
struct Device {
    identifiers: Vec<String>,
    name: String,
    manufacturer: String,
    model: String,
    sw_version: String,
}

#[derive(Debug, Clone, Serialize)]
struct Origin {
    name: String,
    sw_version: String,
    support_url: String,
}

#[derive(Debug, Clone, Serialize)]
struct Availability {
    topic: String,
    payload_available: String,
    payload_not_available: String,
}

fn availability() -> Vec<Availability> {
    vec![Availability {
        topic: format!("{NAMESPACE}/availability"),
        payload_available: "online".to_string(),
        payload_not_available: "offline".to_string(),
    }]
}

#[derive(Debug, Serialize)]
struct BinarySensorConfig {
    name: String,
    default_entity_id: String,
    unique_id: String,
    state_topic: String,
    payload_on: String,
    payload_off: String,
    icon: String,
    device: Device,
    origin: Origin,
    availability: Vec<Availability>,
}

#[derive(Debug, Serialize)]
struct SensorConfig {
    name: String,
    default_entity_id: String,
    unique_id: String,
    state_topic: String,
    icon: String,
    device: Device,
    origin: Origin,
    availability: Vec<Availability>,
    #[serde(skip_serializing_if = "Option::is_none")]
    value_template: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    unit_of_measurement: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    device_class: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    state_class: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    json_attributes_topic: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    json_attributes_template: Option<String>,
}

/// `binary_sensor.z2m_guard_suppress` — `on` while suppression is active.
pub fn suppress_sensor(meta: &HaMeta) -> Discovery {
    let cfg = BinarySensorConfig {
        name: "Restart window".to_string(),
        default_entity_id: "binary_sensor.z2m_guard_suppress".to_string(),
        unique_id: "z2m_guard_suppress".to_string(),
        state_topic: format!("{NAMESPACE}/suppress"),
        payload_on: "ON".to_string(),
        payload_off: "OFF".to_string(),
        icon: "mdi:shield-sync".to_string(),
        device: meta.device(),
        origin: meta.origin(),
        availability: availability(),
    };
    Discovery {
        topic: meta.config_topic("binary_sensor", "suppress"),
        payload: serde_json::to_string(&cfg).expect("discovery payload is serialisable"),
    }
}

/// `sensor.z2m_guard_state` — `idle | suspect | planned | settling | outage_confirmed`.
pub fn state_sensor(meta: &HaMeta) -> Discovery {
    let cfg = SensorConfig {
        name: "State".to_string(),
        default_entity_id: "sensor.z2m_guard_state".to_string(),
        unique_id: "z2m_guard_state".to_string(),
        state_topic: format!("{NAMESPACE}/state"),
        icon: "mdi:state-machine".to_string(),
        device: meta.device(),
        origin: meta.origin(),
        availability: availability(),
        value_template: None,
        unit_of_measurement: None,
        device_class: None,
        state_class: None,
        json_attributes_topic: None,
        json_attributes_template: None,
    };
    Discovery {
        topic: meta.config_topic("sensor", "state"),
        payload: serde_json::to_string(&cfg).expect("discovery payload is serialisable"),
    }
}

/// `sensor.z2m_guard_last_restart` — duration of the last suppressed downtime.
pub fn last_restart_sensor(meta: &HaMeta) -> Discovery {
    let cfg = SensorConfig {
        name: "Last suppressed downtime".to_string(),
        default_entity_id: "sensor.z2m_guard_last_restart".to_string(),
        unique_id: "z2m_guard_last_restart".to_string(),
        state_topic: format!("{NAMESPACE}/last_restart"),
        icon: "mdi:timer-sand".to_string(),
        device: meta.device(),
        origin: meta.origin(),
        availability: availability(),
        value_template: Some("{{ (value_json.downtime_ms / 1000) | round(1) }}".to_string()),
        unit_of_measurement: Some("s".to_string()),
        device_class: Some("duration".to_string()),
        state_class: Some("measurement".to_string()),
        json_attributes_topic: Some(format!("{NAMESPACE}/last_restart")),
        json_attributes_template: Some("{{ {'reason': value_json.reason} | tojson }}".to_string()),
    };
    Discovery {
        topic: meta.config_topic("sensor", "last_restart"),
        payload: serde_json::to_string(&cfg).expect("discovery payload is serialisable"),
    }
}

/// All three discovery messages, in publish order.
pub fn all(meta: &HaMeta) -> Vec<Discovery> {
    vec![
        suppress_sensor(meta),
        state_sensor(meta),
        last_restart_sensor(meta),
    ]
}
