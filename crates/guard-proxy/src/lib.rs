//! Optional transparent MQTT proxy (M5).
//!
//! Sits between Zigbee2MQTT and the broker to deduplicate discovery payload storms
//! and keep the data path verified. Feature-gated because it turns the guard into a
//! hop in the Zigbee data path.
//!
//! What it does, precisely:
//!
//! - terminates the downstream CONNECT and re-issues it upstream with the same
//!   protocol level (3.1.1 or 5.0) and the **client's will**, so the broker still
//!   publishes `offline` for the bridge when Z2M dies,
//! - forwards every other packet unmodified in both directions — no re-encoding, so
//!   properties and reason codes survive an MQTT 5 session,
//! - enforces a topic allowlist on publishes and subscriptions; a violation closes
//!   the session (fail closed) instead of silently passing an unauthorised topic,
//! - drops **retained** publishes whose topic+payload it already forwarded inside
//!   the dedupe window, and acknowledges QoS 1 itself so Z2M does not retransmit.
//!
//! What it does not do: it is not a broker (no retained store of its own, no
//! sessions), it does not rewrite topics, and it does not speak TLS.

#![forbid(unsafe_code)]

pub mod allow;
pub mod dedupe;
pub mod packet;
pub mod relay;

use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::net::TcpListener;
use tracing::{info, warn};

/// Everything one relayed session needs, resolved by the caller.
#[derive(Debug, Clone)]
pub struct RelaySettings {
    /// Broker the proxy forwards to.
    pub upstream: SocketAddr,
    /// MQTT topic filters the downstream client may publish to and subscribe from.
    pub allow: Vec<String>,
    /// Window inside which an identical retained payload is dropped.
    pub dedupe_window: Duration,
    /// Prefix for the upstream client id; the session counter is appended.
    pub client_prefix: String,
    /// Broker credentials used for the upstream connection (the client's own are
    /// not reused — the proxy is the one holding the least-privilege account).
    pub username: Option<String>,
    pub password: Option<String>,
}

impl RelaySettings {
    pub fn new(upstream: SocketAddr) -> Self {
        Self {
            upstream,
            allow: vec!["zigbee2mqtt/#".to_string()],
            dedupe_window: Duration::from_secs(60),
            client_prefix: "z2m-guard-proxy".to_string(),
            username: None,
            password: None,
        }
    }
}

/// Counters the proxy owns; the guard's `/metrics` reads them.
#[derive(Debug, Default)]
pub struct Stats {
    pub sessions: AtomicU64,
    pub deduped: AtomicU64,
    pub denied: AtomicU64,
    pub session_errors: AtomicU64,
}

impl Stats {
    pub fn deduped(&self) -> u64 {
        self.deduped.load(Ordering::Relaxed)
    }
    pub fn denied(&self) -> u64 {
        self.denied.load(Ordering::Relaxed)
    }
    pub fn sessions(&self) -> u64 {
        self.sessions.load(Ordering::Relaxed)
    }
    pub fn session_errors(&self) -> u64 {
        self.session_errors.load(Ordering::Relaxed)
    }
}

/// Accepts downstream clients until the future is dropped.
pub async fn serve(
    listen: SocketAddr,
    settings: RelaySettings,
    stats: Arc<Stats>,
) -> std::io::Result<()> {
    let listener = TcpListener::bind(listen).await?;
    info!(
        %listen,
        upstream = %settings.upstream,
        allow = ?settings.allow,
        dedupe_ms = settings.dedupe_window.as_millis() as u64,
        "proxy listening"
    );

    // One dedupe window for the whole process, not one per connection: Z2M
    // reconnects between restarts, and a per-session window would forget exactly
    // the payloads it saw a second earlier (found by a live test against a real
    // broker, not by the unit tests).
    let dedupe = Arc::new(std::sync::Mutex::new(dedupe::Dedupe::new(
        settings.dedupe_window,
    )));

    let settings = Arc::new(settings);
    loop {
        match listener.accept().await {
            Ok((sock, _)) => {
                let settings = Arc::clone(&settings);
                let stats = Arc::clone(&stats);
                let dedupe = Arc::clone(&dedupe);
                tokio::spawn(
                    async move { relay::run_session(sock, settings, stats, dedupe).await },
                );
            }
            Err(e) => {
                warn!(error = %e, "accept failed");
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::packet::{
        body, build_connack, build_connect, build_puback, packet_type, parse_connect,
        parse_publish, publish_payload, read_packet, Will, CONNACK, PUBACK, PUBLISH,
    };
    use std::net::SocketAddr;
    use std::time::Duration;
    use tokio::io::AsyncWriteExt;
    use tokio::net::TcpStream;

    /// A pretend broker: answers CONNECT with CONNACK(0), acknowledges every QoS 1
    /// publish, and records the topics + payloads that actually reached it.
    async fn fake_broker(
        listener: TcpListener,
    ) -> (SocketAddr, tokio::sync::mpsc::Receiver<(String, Vec<u8>)>) {
        let addr = listener.local_addr().expect("test fixture");
        let (tx, rx) = tokio::sync::mpsc::channel(64);
        tokio::spawn(async move {
            // Accepts repeatedly: every downstream session opens its own upstream
            // connection, so a broker that accepts once would deadlock the second.
            loop {
                let Ok((mut sock, _)) = listener.accept().await else {
                    return;
                };
                let tx = tx.clone();
                tokio::spawn(async move {
                    let Ok(Some(connect)) = read_packet(&mut sock).await else {
                        return;
                    };
                    let Some(info) = parse_connect(&connect) else {
                        return;
                    };
                    sock.write_all(&build_connack(info.protocol_level, false, 0))
                        .await
                        .expect("test fixture");

                    loop {
                        let Ok(Some(pkt)) = read_packet(&mut sock).await else {
                            return;
                        };
                        if packet_type(&pkt) == PUBLISH {
                            let info = parse_publish(&pkt).expect("test fixture");
                            let payload = publish_payload(&pkt, &info).to_vec();
                            let _ = tx.send((info.topic.clone(), payload)).await;
                            if let Some(id) = info.packet_id {
                                sock.write_all(&build_puback(4, id))
                                    .await
                                    .expect("test fixture");
                            }
                        }
                    }
                });
            }
        });
        (addr, rx)
    }

    async fn proxy_for(upstream: SocketAddr, allow: Vec<String>) -> (SocketAddr, Arc<Stats>) {
        let probe = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("test fixture");
        let port = probe.local_addr().expect("test fixture");
        drop(probe);

        let mut settings = RelaySettings::new(upstream);
        settings.allow = allow;
        settings.dedupe_window = Duration::from_secs(5);
        let stats = Arc::new(Stats::default());
        let st = Arc::clone(&stats);
        tokio::spawn(async move {
            let _ = serve(port, settings, st).await;
        });
        tokio::time::sleep(Duration::from_millis(50)).await;
        (port, stats)
    }

    fn connect_bytes(level: u8) -> Vec<u8> {
        build_connect(level, "zigbee2mqtt", None, None, None, 60)
    }

    fn publish_bytes(topic: &str, payload: &[u8], qos1_id: Option<u16>, retain: bool) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(&(topic.len() as u16).to_be_bytes());
        b.extend_from_slice(topic.as_bytes());
        if let Some(id) = qos1_id {
            b.extend_from_slice(&id.to_be_bytes());
        }
        b.extend_from_slice(payload);
        let mut flags = PUBLISH << 4;
        if retain {
            flags |= 0x01;
        }
        if qos1_id.is_some() {
            flags |= 0x02;
        }
        let mut out = vec![flags];
        let mut len = b.len();
        loop {
            let mut byte = (len % 128) as u8;
            len /= 128;
            if len > 0 {
                byte |= 0x80;
            }
            out.push(byte);
            if len == 0 {
                break;
            }
        }
        out.extend_from_slice(&b);
        out
    }

    #[tokio::test]
    async fn duplicate_retained_discovery_is_dropped_and_acked() {
        let broker = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("test fixture");
        let (upstream, mut received) = fake_broker(broker).await;
        let (proxy, stats) = proxy_for(
            upstream,
            vec!["zigbee2mqtt/#".into(), "homeassistant/#".into()],
        )
        .await;

        let mut client = TcpStream::connect(proxy).await.expect("test fixture");
        client
            .write_all(&connect_bytes(4))
            .await
            .expect("test fixture");
        let connack = read_packet(&mut client)
            .await
            .expect("test fixture")
            .expect("test fixture");
        assert_eq!(packet_type(&connack), CONNACK);
        assert_eq!(body(&connack)[1], 0);

        let config_payload = br#"{"name":"temperature"}"#;
        for id in 1..=2u16 {
            client
                .write_all(&publish_bytes(
                    "homeassistant/sensor/x/config",
                    config_payload,
                    Some(id),
                    true,
                ))
                .await
                .expect("test fixture");
            let ack = read_packet(&mut client)
                .await
                .expect("test fixture")
                .expect("test fixture");
            assert_eq!(packet_type(&ack), PUBACK, "both publishes are acked");
        }

        let first = tokio::time::timeout(Duration::from_secs(2), received.recv())
            .await
            .expect("broker sees the first publish")
            .expect("channel");
        assert_eq!(first.0, "homeassistant/sensor/x/config");
        assert_eq!(first.1, config_payload);

        assert!(
            tokio::time::timeout(Duration::from_millis(300), received.recv())
                .await
                .is_err(),
            "duplicate must not reach the broker"
        );
        assert_eq!(stats.deduped(), 1);
    }

    #[tokio::test]
    async fn dedupe_window_survives_reconnects() {
        // Found by a live test against a real broker: the window was per session, so
        // two connections each had their own and the duplicate went through.
        let broker = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("test fixture");
        let (upstream, mut received) = fake_broker(broker).await;
        let (proxy, stats) = proxy_for(upstream, vec!["homeassistant/#".into()]).await;

        for id in 1..=2u16 {
            let mut client = TcpStream::connect(proxy).await.expect("test fixture");
            client
                .write_all(&connect_bytes(4))
                .await
                .expect("test fixture");
            let _ = read_packet(&mut client)
                .await
                .expect("test fixture")
                .expect("test fixture");

            client
                .write_all(&publish_bytes(
                    "homeassistant/sensor/y/config",
                    br#"{"n":1}"#,
                    Some(id),
                    true,
                ))
                .await
                .expect("test fixture");
            let ack = read_packet(&mut client)
                .await
                .expect("test fixture")
                .expect("test fixture");
            assert_eq!(packet_type(&ack), PUBACK, "each session gets its ack");
            drop(client);
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        let first = tokio::time::timeout(Duration::from_secs(2), received.recv())
            .await
            .expect("broker sees the first publish")
            .expect("channel");
        assert_eq!(first.0, "homeassistant/sensor/y/config");
        assert!(
            tokio::time::timeout(Duration::from_millis(300), received.recv())
                .await
                .is_err(),
            "the reconnected duplicate must not reach the broker"
        );
        assert_eq!(stats.deduped(), 1);
    }

    #[tokio::test]
    async fn publish_outside_allowlist_closes_the_session() {
        let broker = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("test fixture");
        let (upstream, _rx) = fake_broker(broker).await;
        let (proxy, stats) = proxy_for(upstream, vec!["zigbee2mqtt/#".into()]).await;

        let mut client = TcpStream::connect(proxy).await.expect("test fixture");
        client
            .write_all(&connect_bytes(4))
            .await
            .expect("test fixture");
        let _ = read_packet(&mut client).await.expect("test fixture");

        client
            .write_all(&publish_bytes("z2m_guard/state", b"idle", None, false))
            .await
            .expect("test fixture");

        let closed = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                match read_packet(&mut client).await {
                    Ok(None) | Err(_) => break,
                    Ok(Some(_)) => continue,
                }
            }
        })
        .await;
        assert!(closed.is_ok(), "session must be closed");
        assert_eq!(stats.denied(), 1);
    }

    #[tokio::test]
    async fn non_retained_repeats_pass_through() {
        let broker = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("test fixture");
        let (upstream, mut received) = fake_broker(broker).await;
        let (proxy, stats) = proxy_for(upstream, vec!["zigbee2mqtt/#".into()]).await;

        let mut client = TcpStream::connect(proxy).await.expect("test fixture");
        client
            .write_all(&connect_bytes(4))
            .await
            .expect("test fixture");
        let _ = read_packet(&mut client).await.expect("test fixture");

        for id in 1..=2u16 {
            client
                .write_all(&publish_bytes(
                    "zigbee2mqtt/0x01/state",
                    br#"{"state":"ON"}"#,
                    Some(id),
                    false,
                ))
                .await
                .expect("test fixture");
            let _ = read_packet(&mut client).await.expect("test fixture");
        }

        for _ in 0..2 {
            let got = tokio::time::timeout(Duration::from_secs(2), received.recv())
                .await
                .expect("forwarded")
                .expect("channel");
            assert_eq!(got.0, "zigbee2mqtt/0x01/state");
        }
        assert_eq!(stats.deduped(), 0);
    }

    #[tokio::test]
    async fn will_is_forwarded_upstream() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("test fixture");
        let addr = listener.local_addr().expect("test fixture");
        let seen = tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.expect("test fixture");
            let pkt = read_packet(&mut sock)
                .await
                .expect("test fixture")
                .expect("test fixture");
            parse_connect(&pkt).expect("test fixture")
        });

        let (proxy, _stats) = proxy_for(addr, vec!["zigbee2mqtt/#".into()]).await;
        let mut client = TcpStream::connect(proxy).await.expect("test fixture");
        let will = Will {
            topic: "zigbee2mqtt/bridge/state".into(),
            payload: b"offline".to_vec(),
            qos: 0,
            retain: true,
        };
        client
            .write_all(&build_connect(
                4,
                "zigbee2mqtt",
                Some(&will),
                None,
                None,
                60,
            ))
            .await
            .expect("test fixture");
        let _ = tokio::time::timeout(Duration::from_secs(2), read_packet(&mut client)).await;

        let upstream_connect = tokio::time::timeout(Duration::from_secs(2), seen)
            .await
            .expect("upstream connect arrived")
            .expect("join");
        assert_eq!(upstream_connect.will, Some(will));
        assert_ne!(
            upstream_connect.client_id, "zigbee2mqtt",
            "proxy uses its own id"
        );
    }
}
