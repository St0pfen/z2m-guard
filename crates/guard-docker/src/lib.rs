//! Optional Docker event attribution: tells a clean container exit (planned
//! recreate) apart from a crash (non-zero exit code).
//!
//! Two sources, both optional and both off by default:
//! - `stdin`: reads `docker events --format '{{json .}}'` lines piped in by the
//!   operator. No privileges at all — the guard just parses text.
//! - `docker-socket`: reserved for the direct Engine API. Not implemented in
//!   v0.1 on purpose: the socket is root-equivalent host access, and the guard
//!   has to work without it.
//!
//! The parser is pure and covered by fixtures recorded from a real daemon, so
//! the attribution logic can be tested without Docker running.

#![forbid(unsafe_code)]

use std::collections::HashMap;

use serde::Deserialize;

/// What the guard cares about in a Docker event stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContainerEvent {
    /// Container exited. `exit_code == 0` means a clean stop (planned).
    Die { name: String, exit_code: i32 },
    /// Container started.
    Start { name: String },
}

#[derive(Debug, Deserialize)]
struct RawEvent {
    #[serde(rename = "Type", default)]
    event_type: Option<String>,
    #[serde(rename = "Action", default)]
    action: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(rename = "Actor", default)]
    actor: Option<RawActor>,
}

#[derive(Debug, Deserialize)]
struct RawActor {
    #[serde(rename = "Attributes", default)]
    attributes: HashMap<String, String>,
}

/// Parses one event line (JSON from `docker events --format '{{json .}}'` or the
/// Engine HTTP stream). Returns `None` for anything the guard does not track.
pub fn parse_event(line: &str) -> Option<ContainerEvent> {
    let line = line.trim();
    if line.is_empty() {
        return None;
    }
    let raw: RawEvent = serde_json::from_str(line).ok()?;

    // The CLI sets "Type"/"Action"; the raw API stream uses "status".
    if let Some(t) = &raw.event_type {
        if t != "container" {
            return None;
        }
    }
    let action = raw.action.as_deref().or(raw.status.as_deref())?;
    let attributes = raw.actor.map(|a| a.attributes).unwrap_or_default();
    let name = attributes.get("name").cloned().unwrap_or_default();

    match action {
        "die" => {
            let exit_code = attributes
                .get("exitCode")
                .and_then(|c| c.parse::<i32>().ok())
                .unwrap_or(-1);
            Some(ContainerEvent::Die { name, exit_code })
        }
        "start" => Some(ContainerEvent::Start { name }),
        _ => None,
    }
}

/// Reads event lines from anything `AsyncBufRead` and hands every recognised
/// event to `on_event`. Ends when the source closes (e.g. the pipe breaks).
#[cfg(feature = "stdin")]
pub async fn read_events<R, F>(reader: R, mut on_event: F)
where
    R: tokio::io::AsyncBufRead + Unpin,
    F: FnMut(ContainerEvent),
{
    use tokio::io::AsyncBufReadExt;

    let mut lines = reader.lines();
    while let Ok(Some(line)) = lines.next_line().await {
        if let Some(event) = parse_event(&line) {
            on_event(event);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_clean_and_crash_exits_from_recorded_events() {
        let fixture = include_str!("../tests/fixtures/docker-events.jsonl");
        let mut events = Vec::new();
        for line in fixture.lines() {
            if let Some(event) = parse_event(line) {
                events.push(event);
            }
        }
        assert_eq!(
            events,
            vec![
                ContainerEvent::Die {
                    name: "zigbee2mqtt".to_string(),
                    exit_code: 137
                },
                ContainerEvent::Start {
                    name: "zigbee2mqtt".to_string()
                },
            ],
            "only die and start are attributed, in stream order"
        );
    }

    #[test]
    fn ignores_create_kill_destroy_and_foreign_types() {
        let fixture = include_str!("../tests/fixtures/docker-events.jsonl");
        for line in fixture.lines() {
            let parsed = parse_event(line);
            if let Some(event) = parsed {
                assert!(
                    matches!(
                        event,
                        ContainerEvent::Die { .. } | ContainerEvent::Start { .. }
                    ),
                    "unexpected event for line: {line}"
                );
            }
        }
        let network_event = r#"{"Type":"network","Action":"create","Actor":{"ID":"x","Attributes":{"name":"hass_net"}}}"#;
        assert_eq!(parse_event(network_event), None);
    }

    #[test]
    fn tolerates_missing_exit_code_and_garbage() {
        let no_code = r#"{"Type":"container","Action":"die","Actor":{"ID":"x","Attributes":{"name":"zigbee2mqtt"}}}"#;
        assert_eq!(
            parse_event(no_code),
            Some(ContainerEvent::Die {
                name: "zigbee2mqtt".to_string(),
                exit_code: -1
            })
        );
        assert_eq!(parse_event("not json"), None);
        assert_eq!(parse_event(""), None);
        assert_eq!(parse_event("   "), None);
    }

    #[test]
    fn accepts_the_raw_api_shape_with_status_field() {
        let api_line = r#"{"status":"die","id":"abc","Type":"container","Action":"die","Actor":{"ID":"abc","Attributes":{"name":"zigbee2mqtt","exitCode":"0"}}}"#;
        assert_eq!(
            parse_event(api_line),
            Some(ContainerEvent::Die {
                name: "zigbee2mqtt".to_string(),
                exit_code: 0
            })
        );
    }
}
