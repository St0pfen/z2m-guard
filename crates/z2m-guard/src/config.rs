//! Configuration: TOML file + environment overrides.
//!
//! No secrets in the file. Passwords and tokens are read from the environment
//! variables named in the config, so a config file can be committed or shared.

use std::path::Path;

use anyhow::{bail, Context, Result};
use serde::Deserialize;

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Config {
    pub mqtt: MqttSection,
    pub guard: GuardSection,
    pub homeassistant: HaSection,
    pub http: HttpSection,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct MqttSection {
    pub url: String,
    pub client_id: String,
    pub username: String,
    /// Name of the environment variable holding the password.
    pub password_env: String,
    pub base_topic: String,
}

impl Default for MqttSection {
    fn default() -> Self {
        Self {
            url: "mqtt://127.0.0.1:1883".to_string(),
            client_id: "z2m-guard".to_string(),
            username: "z2mguard".to_string(),
            password_env: "Z2M_GUARD_MQTT_PASSWORD".to_string(),
            base_topic: "zigbee2mqtt".to_string(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct GuardSection {
    pub grace_ms: u64,
    pub settle_ms: u64,
    pub attribution_window_ms: u64,
    pub prior_window_ms: u64,
    pub tick_ms: u64,
}

impl Default for GuardSection {
    fn default() -> Self {
        Self {
            grace_ms: 15_000,
            settle_ms: 20_000,
            attribution_window_ms: 30_000,
            prior_window_ms: 120_000,
            tick_ms: 1_000,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct HaSection {
    pub discovery_prefix: String,
    pub device_name: String,
    /// Publish HA discovery on connect. Set to false for observe-only runs.
    pub discovery: bool,
}

impl Default for HaSection {
    fn default() -> Self {
        Self {
            discovery_prefix: "homeassistant".to_string(),
            device_name: "Z2M Guard".to_string(),
            discovery: true,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct HttpSection {
    pub bind: String,
    /// Name of the environment variable holding the restart-intent token.
    pub token_env: String,
}

impl Default for HttpSection {
    fn default() -> Self {
        Self {
            bind: "127.0.0.1:8089".to_string(),
            token_env: "Z2M_GUARD_HTTP_TOKEN".to_string(),
        }
    }
}

impl Config {
    /// Loads the file (if it exists) and applies `Z2M_GUARD_*` overrides.
    pub fn load(path: Option<&Path>) -> Result<Self> {
        let mut cfg = match path {
            Some(p) if p.exists() => {
                let raw = std::fs::read_to_string(p)
                    .with_context(|| format!("reading config {}", p.display()))?;
                toml::from_str(&raw).with_context(|| format!("parsing config {}", p.display()))?
            }
            Some(p) => bail!("config file not found: {}", p.display()),
            None => Config::default(),
        };

        if let Ok(v) = std::env::var("Z2M_GUARD_MQTT_URL") {
            cfg.mqtt.url = v;
        }
        if let Ok(v) = std::env::var("Z2M_GUARD_BASE_TOPIC") {
            cfg.mqtt.base_topic = v;
        }
        if let Ok(v) = std::env::var("Z2M_GUARD_MQTT_USERNAME") {
            cfg.mqtt.username = v;
        }
        if let Ok(v) = std::env::var("Z2M_GUARD_BIND") {
            cfg.http.bind = v;
        }
        if let Ok(v) = std::env::var("Z2M_GUARD_DISCOVERY") {
            cfg.homeassistant.discovery = v == "1" || v.eq_ignore_ascii_case("true");
        }
        Ok(cfg)
    }

    pub fn timings(&self) -> guard_core::Timings {
        guard_core::Timings {
            grace_ms: self.guard.grace_ms,
            settle_ms: self.guard.settle_ms,
            attribution_window_ms: self.guard.attribution_window_ms,
            prior_window_ms: self.guard.prior_window_ms,
        }
    }

    pub fn mqtt_password(&self) -> Option<String> {
        std::env::var(&self.mqtt.password_env).ok()
    }

    pub fn http_token(&self) -> Option<String> {
        std::env::var(&self.http.token_env).ok()
    }

    pub fn mqtt_config(&self) -> guard_mqtt::MqttConfig {
        guard_mqtt::MqttConfig {
            url: self.mqtt.url.clone(),
            client_id: self.mqtt.client_id.clone(),
            username: Some(self.mqtt.username.clone()),
            password: self.mqtt_password(),
            base_topic: self.mqtt.base_topic.clone(),
            // Availability is a retained last-will: the broker flips it to
            // "offline" whenever the guard's connection drops, no cooperation
            // from the dying process required.
            last_will: Some((
                format!("{}/availability", guard_ha::NAMESPACE),
                "offline".to_string(),
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_sane() {
        let cfg = Config::default();
        assert_eq!(cfg.mqtt.base_topic, "zigbee2mqtt");
        assert_eq!(cfg.http.bind, "127.0.0.1:8089");
        assert!(cfg.homeassistant.discovery);
        assert_eq!(cfg.timings().grace_ms, 15_000);
    }

    #[test]
    fn missing_config_file_is_an_error() {
        let err = Config::load(Some(Path::new("/nonexistent/z2m-guard.toml")));
        assert!(err.is_err());
    }

    #[test]
    fn parses_partial_toml_with_defaults() {
        let dir = std::env::temp_dir().join("z2m-guard-config-test");
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let p = dir.join("partial.toml");
        std::fs::write(
            &p,
            "[mqtt]\nbase_topic = \"zigbee\"\n\n[guard]\ngrace_ms = 5000\n",
        )
        .expect("write test config");
        let cfg = Config::load(Some(&p)).expect("config loads");
        assert_eq!(cfg.mqtt.base_topic, "zigbee");
        assert_eq!(cfg.guard.grace_ms, 5_000);
        assert_eq!(cfg.guard.settle_ms, 20_000, "unset keys keep defaults");
        assert_eq!(cfg.http.bind, "127.0.0.1:8089");
    }
}
