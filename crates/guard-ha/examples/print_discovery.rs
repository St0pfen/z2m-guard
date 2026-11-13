//! Prints the discovery payloads as pretty JSON, one section per message.
//! Used to regenerate `tests/golden/*.json`.

use guard_ha::{all, HaMeta};

fn main() {
    let meta = HaMeta::default();
    for d in all(&meta) {
        let value: serde_json::Value =
            serde_json::from_str(&d.payload).expect("payload is valid json");
        println!(
            "=== {}\n{}",
            d.topic,
            serde_json::to_string_pretty(&value).expect("pretty print")
        );
    }
}
