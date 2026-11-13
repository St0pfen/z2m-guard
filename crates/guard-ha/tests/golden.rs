//! Golden-file tests: the discovery payloads are a public contract. HA parses
//! them, users debug them, and a silent field rename breaks somebody's
//! dashboard.

use guard_ha::{all, HaMeta};

fn golden(name: &str) -> serde_json::Value {
    let path = format!("{}/tests/golden/{name}.json", env!("CARGO_MANIFEST_DIR"));
    let raw = std::fs::read_to_string(&path).expect("golden file exists");
    serde_json::from_str(&raw).expect("golden file is valid json")
}

#[test]
fn topics_are_stable() {
    let topics: Vec<String> = all(&HaMeta::default())
        .into_iter()
        .map(|d| d.topic)
        .collect();
    assert_eq!(
        topics,
        vec![
            "homeassistant/binary_sensor/z2m_guard/suppress/config",
            "homeassistant/sensor/z2m_guard/state/config",
            "homeassistant/sensor/z2m_guard/last_restart/config",
        ]
    );
}

#[test]
fn suppress_payload_matches_golden() {
    let all = all(&HaMeta::default());
    let payload: serde_json::Value =
        serde_json::from_str(&all[0].payload).expect("payload is valid json");
    assert_eq!(payload, golden("suppress"));
}

#[test]
fn state_payload_matches_golden() {
    let all = all(&HaMeta::default());
    let payload: serde_json::Value =
        serde_json::from_str(&all[1].payload).expect("payload is valid json");
    assert_eq!(payload, golden("state"));
}

#[test]
fn last_restart_payload_matches_golden() {
    let all = all(&HaMeta::default());
    let payload: serde_json::Value =
        serde_json::from_str(&all[2].payload).expect("payload is valid json");
    assert_eq!(payload, golden("last_restart"));
}

#[test]
fn payloads_never_touch_foreign_topics() {
    for d in all(&HaMeta::default()) {
        assert!(
            !d.payload.contains("zigbee2mqtt/"),
            "guard must not publish into the Z2M namespace: {}",
            d.topic
        );
        assert!(!d.payload.is_empty());
    }
}
