//! Integration test against a throwaway mosquitto container.
//!
//! Two contracts are proven without touching the real broker:
//! 1. a retained `bridge/state` message reaches a subscriber that connects
//!    later (that is what makes the guard work after its own restart),
//! 2. the guard's publisher actually reaches the broker.
//!
//! Skipped (with a printed note) when Docker is unavailable.

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use guard_mqtt::{backoff, connect, parse_bridge_state, subscribe_bridge_state, MqttConfig};
use rumqttc::{Event, Packet, QoS};

const CONTAINER: &str = "z2m-guard-it-mqtt";
const PORT: u16 = 18_830;

fn docker(args: &[&str]) -> std::process::Output {
    Command::new("docker")
        .args(args)
        .output()
        .expect("docker binary is runnable")
}

fn docker_available() -> bool {
    Command::new("docker")
        .arg("version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn start_broker() {
    let _ = docker(&["rm", "-f", CONTAINER]);
    let port_map = format!("127.0.0.1:{PORT}:1883");
    let out = docker(&[
        "run",
        "--rm",
        "-d",
        "--name",
        CONTAINER,
        "-p",
        &port_map,
        "eclipse-mosquitto:2",
        "mosquitto",
        "-c",
        "/mosquitto-no-auth.conf",
    ]);
    assert!(
        out.status.success(),
        "starting mosquitto failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn stop_broker() {
    let _ = docker(&["rm", "-f", CONTAINER]);
}

fn cfg(client_id: &str) -> MqttConfig {
    MqttConfig {
        url: format!("mqtt://127.0.0.1:{PORT}"),
        client_id: client_id.to_string(),
        username: None,
        password: None,
        base_topic: "zigbee2mqtt".to_string(),
        last_will: None,
    }
}

fn wait_for_port() {
    for _ in 0..50 {
        if std::net::TcpStream::connect(("127.0.0.1", PORT)).is_ok() {
            return;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    panic!("broker port never opened");
}

/// Publishes with the broker's own client — removes our publisher from the
/// equation when the subscribe path is what is under test.
fn publish_via_cli(topic: &str, payload: &str, retain: bool) {
    let mut args = vec!["exec", CONTAINER, "mosquitto_pub", "-h", "127.0.0.1"];
    args.extend_from_slice(&["-t", topic, "-m", payload]);
    if retain {
        args.push("-r");
    }
    let out = docker(&args);
    assert!(
        out.status.success(),
        "mosquitto_pub failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[tokio::test]
async fn broker_contract() {
    if !docker_available() {
        eprintln!("skipping: docker not available");
        return;
    }
    start_broker();
    wait_for_port();

    // --- Phase 1: retained bridge state reaches a late subscriber -----------
    let sub_cfg = cfg("z2m-guard-it-sub");
    let topic = sub_cfg.bridge_state_topic();
    publish_via_cli(&topic, r#"{"state":"offline"}"#, true);

    let (subscriber, mut sub_loop) = connect(&sub_cfg).expect("subscriber connects");
    subscribe_bridge_state(&subscriber, &sub_cfg)
        .await
        .expect("subscribe accepted");

    let mut received = None;
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_secs(2), sub_loop.poll()).await {
            Ok(Ok(Event::Incoming(Packet::Publish(p)))) if p.topic == topic => {
                received = Some(p.payload.to_vec());
                break;
            }
            Ok(Ok(_)) => continue,
            Ok(Err(e)) => panic!("subscriber connection error: {e}"),
            Err(_) => continue,
        }
    }
    let payload = received.expect("retained message must reach a late subscriber");
    assert_eq!(parse_bridge_state(&payload), Some(false));

    // --- Phase 2: the guard's own publisher reaches the broker --------------
    let out_topic = "z2m-guard/it/publish";
    let watcher = Command::new("docker")
        .args([
            "exec",
            CONTAINER,
            "mosquitto_sub",
            "-h",
            "127.0.0.1",
            "-t",
            out_topic,
            "-C",
            "1",
            "-W",
            "10",
            "-v",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn mosquitto_sub");
    tokio::time::sleep(Duration::from_millis(800)).await;

    let pub_cfg = cfg("z2m-guard-it-pub");
    let (client, mut eventloop) = connect(&pub_cfg).expect("publisher connects");
    let driver = tokio::spawn(async move { while eventloop.poll().await.is_ok() {} });
    client
        .publish(out_topic, QoS::AtLeastOnce, false, "hello from the guard")
        .await
        .expect("publish accepted");

    // Waiting for the child must not block the runtime: the publisher's event
    // loop runs on this same (current-thread) runtime.
    let out = tokio::task::spawn_blocking(move || watcher.wait_with_output())
        .await
        .expect("join watcher")
        .expect("watcher finishes");
    driver.abort();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("hello from the guard"),
        "broker side saw nothing; stdout={stdout} stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );

    // --- Phase 3: the last will flips availability on a dead connection -----
    let will_topic = "z2m-guard/it/availability";
    let mut will_cfg = cfg("z2m-guard-it-will");
    will_cfg.last_will = Some((will_topic.to_string(), "offline".to_string()));

    let (will_client, mut will_loop) = connect(&will_cfg).expect("will client connects");
    // Wait until the broker accepted the CONNECT (will is registered then).
    let mut acked = false;
    let connect_deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < connect_deadline && !acked {
        match tokio::time::timeout(Duration::from_secs(2), will_loop.poll()).await {
            Ok(Ok(Event::Incoming(Packet::ConnAck(_)))) => acked = true,
            Ok(Ok(_)) | Err(_) => continue,
            Ok(Err(e)) => panic!("will client connection error: {e}"),
        }
    }
    assert!(acked, "will client never connected");

    // Drop the socket without DISCONNECT — exactly what a killed guard does.
    drop(will_client);
    drop(will_loop);
    tokio::time::sleep(Duration::from_millis(500)).await;

    let out = docker(&[
        "exec",
        CONTAINER,
        "mosquitto_sub",
        "-h",
        "127.0.0.1",
        "-t",
        will_topic,
        "-C",
        "1",
        "-W",
        "5",
        "-v",
    ]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("offline"),
        "last will was not published; stdout={stdout} stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );

    stop_broker();
}

#[tokio::test]
async fn backoff_grows_and_caps() {
    // Pure invariant, no broker needed: guards the reconnect loop's timing.
    assert!(backoff(0) < backoff(1));
    assert!(backoff(1) < backoff(2));
    assert_eq!(backoff(9), Duration::from_secs(30));
}
