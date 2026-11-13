//! MQTT transport: subscribes to `zigbee2mqtt/bridge/state`, publishes
//! guard-owned topics, handles reconnect with backoff.

#![forbid(unsafe_code)]

use std::time::Duration;

use rumqttc::{AsyncClient, EventLoop, MqttOptions, QoS};
use thiserror::Error;

/// How the guard reaches the broker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MqttConfig {
    /// e.g. `mqtt://172.22.1.50:1883`
    pub url: String,
    pub client_id: String,
    pub username: Option<String>,
    pub password: Option<String>,
    /// Zigbee2MQTT `mqtt.base_topic`.
    pub base_topic: String,
    /// Retained last-will: (topic, payload). The broker publishes it when the
    /// connection drops — that is how availability survives a crashed guard.
    pub last_will: Option<(String, String)>,
}

impl MqttConfig {
    pub fn bridge_state_topic(&self) -> String {
        format!("{}/bridge/state", self.base_topic)
    }
}

#[derive(Debug, Error)]
pub enum MqttError {
    #[error("unsupported or malformed broker url: {0}")]
    BadUrl(String),
    #[error("mqtt client error: {0}")]
    Client(#[from] rumqttc::ClientError),
}

/// Broker address split out of the URL.
#[derive(Debug, PartialEq, Eq)]
pub struct HostPort {
    pub host: String,
    pub port: u16,
}

/// Parses `mqtt://host:port` into its parts. TLS schemes are deliberately not
/// supported yet — the guard talks to a broker on the local Docker network.
pub fn parse_broker_url(url: &str) -> Result<HostPort, MqttError> {
    let rest = url
        .strip_prefix("mqtt://")
        .ok_or_else(|| MqttError::BadUrl(url.to_string()))?;
    let (host, port) = match rest.rsplit_once(':') {
        Some((h, p)) => (
            h.to_string(),
            p.parse::<u16>()
                .map_err(|_| MqttError::BadUrl(url.to_string()))?,
        ),
        None => (rest.to_string(), 1883),
    };
    if host.is_empty() {
        return Err(MqttError::BadUrl(url.to_string()));
    }
    Ok(HostPort { host, port })
}

/// Builds a client and its event loop. The caller drives the event loop.
pub fn connect(cfg: &MqttConfig) -> Result<(AsyncClient, EventLoop), MqttError> {
    let HostPort { host, port } = parse_broker_url(&cfg.url)?;
    let mut opts = MqttOptions::new(cfg.client_id.clone(), host, port);
    opts.set_keep_alive(Duration::from_secs(30));
    opts.set_clean_session(true);
    if let (Some(user), Some(pass)) = (&cfg.username, &cfg.password) {
        opts.set_credentials(user.clone(), pass.clone());
    }
    if let Some((topic, payload)) = &cfg.last_will {
        opts.set_last_will(rumqttc::LastWill::new(
            topic.clone(),
            payload.clone(),
            QoS::AtLeastOnce,
            true,
        ));
    }
    let (client, eventloop) = AsyncClient::new(opts, 32);
    Ok((client, eventloop))
}

/// Subscribes to the one topic that decides availability of every Zigbee entity.
pub async fn subscribe_bridge_state(
    client: &AsyncClient,
    cfg: &MqttConfig,
) -> Result<(), MqttError> {
    client
        .subscribe(cfg.bridge_state_topic(), QoS::AtLeastOnce)
        .await?;
    Ok(())
}

/// Reconnect delay for attempt `n` (0-based): 1s, 2s, 4s … capped at 30s.
pub fn backoff(attempt: u32) -> Duration {
    let millis = 1_000u64.saturating_mul(1u64 << attempt.min(5));
    Duration::from_millis(millis.min(30_000))
}

/// Parses the retained bridge state payload: `{"state":"online"|"offline"}`.
pub fn parse_bridge_state(payload: &[u8]) -> Option<bool> {
    #[derive(serde::Deserialize)]
    struct BridgeState {
        state: String,
    }
    let parsed: BridgeState = serde_json::from_slice(payload).ok()?;
    match parsed.state.as_str() {
        "online" => Some(true),
        "offline" => Some(false),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_online_and_offline() {
        assert_eq!(parse_bridge_state(br#"{"state":"online"}"#), Some(true));
        assert_eq!(parse_bridge_state(br#"{"state":"offline"}"#), Some(false));
    }

    #[test]
    fn rejects_unknown_states_and_garbage() {
        assert_eq!(parse_bridge_state(br#"{"state":"restarting"}"#), None);
        assert_eq!(parse_bridge_state(b"not json"), None);
        assert_eq!(parse_bridge_state(b""), None);
    }

    #[test]
    fn splits_broker_url() {
        assert_eq!(
            parse_broker_url("mqtt://172.22.1.50:1883").expect("valid url"),
            HostPort {
                host: "172.22.1.50".to_string(),
                port: 1883
            }
        );
        assert_eq!(
            parse_broker_url("mqtt://mosquitto").expect("default port"),
            HostPort {
                host: "mosquitto".to_string(),
                port: 1883
            }
        );
        assert!(parse_broker_url("mqtts://broker:8883").is_err());
        assert!(parse_broker_url("mqtt://:1883").is_err());
    }

    #[test]
    fn backoff_is_capped() {
        assert_eq!(backoff(0), Duration::from_secs(1));
        assert_eq!(backoff(3), Duration::from_secs(8));
        assert_eq!(backoff(50), Duration::from_secs(30));
    }

    #[test]
    fn bridge_topic_follows_base_topic() {
        let cfg = MqttConfig {
            url: "mqtt://127.0.0.1:1883".into(),
            client_id: "z2m-guard".into(),
            username: None,
            password: None,
            base_topic: "zigbee2mqtt".into(),
            last_will: None,
        };
        assert_eq!(cfg.bridge_state_topic(), "zigbee2mqtt/bridge/state");
    }
}
