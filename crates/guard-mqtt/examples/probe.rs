//! Manual probe: connect, subscribe, print every packet for a few seconds.
//!
//! `cargo run -p guard-mqtt --example probe -- mqtt://127.0.0.1:18830`

use std::time::Duration;

use guard_mqtt::{connect, parse_bridge_state, subscribe_bridge_state, MqttConfig};
use rumqttc::Event;

#[tokio::main]
async fn main() {
    let url = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "mqtt://127.0.0.1:18830".to_string());
    let cfg = MqttConfig {
        url: url.clone(),
        client_id: "z2m-guard-probe".to_string(),
        username: None,
        password: None,
        base_topic: "zigbee2mqtt".to_string(),
        last_will: None,
    };
    println!("connecting to {url}");
    let (client, mut eventloop) = connect(&cfg).expect("client builds");
    subscribe_bridge_state(&client, &cfg)
        .await
        .expect("subscribe accepted");
    println!("subscribed to {}", cfg.bridge_state_topic());

    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_secs(2), eventloop.poll()).await {
            Ok(Ok(Event::Incoming(p))) => match p {
                rumqttc::Packet::Publish(m) => {
                    println!(
                        "PUBLISH {} -> {:?} parsed={:?}",
                        m.topic,
                        String::from_utf8_lossy(&m.payload),
                        parse_bridge_state(&m.payload)
                    );
                }
                other => println!("incoming: {other:?}"),
            },
            Ok(Ok(Event::Outgoing(o))) => println!("outgoing: {o:?}"),
            Ok(Err(e)) => {
                println!("error: {e}");
                break;
            }
            Err(_) => println!("(silent 2s)"),
        }
    }
}
