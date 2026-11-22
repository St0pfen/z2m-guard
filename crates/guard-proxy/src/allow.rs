//! MQTT topic filters for the proxy allowlist.
//!
//! The proxy must not become a general-purpose broker hop: it forwards exactly
//! the subscriptions and publishes the guard's job requires, and refuses
//! everything else. Filters use the MQTT wildcard semantics (`+` one level, `#`
//! the rest), so the check matches what a broker would do, not a prefix guess.

/// Matches `topic` against a filter with MQTT wildcards.
pub fn matches(filter: &str, topic: &str) -> bool {
    // A leading '$' topic is never matched by a wildcard filter.
    if topic.starts_with('$') && (filter.starts_with('#') || filter.starts_with('+')) {
        return false;
    }
    let f: Vec<&str> = filter.split('/').collect();
    let t: Vec<&str> = topic.split('/').collect();
    let mut fi = 0;
    let mut ti = 0;
    while fi < f.len() {
        match f[fi] {
            "#" => return true,
            "+" => {
                if ti >= t.len() {
                    return false;
                }
            }
            literal => {
                if ti >= t.len() || t[ti] != literal {
                    return false;
                }
            }
        }
        fi += 1;
        ti += 1;
    }
    ti == t.len()
}

/// True when `filter` is a syntactically valid MQTT topic filter.
///
/// The matcher below would treat `zigbee2mqtt#` (a missing slash, the classic
/// typo) as one literal level and match nothing at all, which inside the proxy
/// looks like "Z2M is broken" rather than "the config is wrong". Validating the
/// grammar up front turns that into a startup error naming the offending entry.
///
/// Rules: no empty filter and no empty level; `#` only as the last level, alone;
/// `+` only as a whole level; a level containing a wildcard is not a literal.
pub fn is_valid_filter(filter: &str) -> bool {
    if filter.is_empty() {
        return false;
    }
    let levels: Vec<&str> = filter.split('/').collect();
    let last = levels.len() - 1;
    levels.iter().enumerate().all(|(i, level)| {
        if level.is_empty() {
            return false;
        }
        if *level == "#" {
            i == last
        } else if level.contains('#') {
            false
        } else if level.contains('+') {
            *level == "+"
        } else {
            true
        }
    })
}

/// Allows a *subscription filter*: every filter must be inside one of the
/// allowlist entries. Comparing filter-to-filter is stricter than topic-to-filter
/// on purpose — `zigbee2mqtt/#` may not be used to authorise `#`.
pub fn allows_filter(allow: &[String], filter: &str) -> bool {
    allow.iter().any(|a| a == filter || covers(a, filter))
}

/// True when filter `outer` covers filter `inner`.
fn covers(outer: &str, inner: &str) -> bool {
    let o: Vec<&str> = outer.split('/').collect();
    let i: Vec<&str> = inner.split('/').collect();
    let mut oi = 0;
    let mut ii = 0;
    while oi < o.len() {
        match o[oi] {
            "#" => return true,
            "+" => {
                if ii >= i.len() {
                    return false;
                }
            }
            lit => {
                if ii >= i.len() || i[ii] != lit {
                    return false;
                }
            }
        }
        oi += 1;
        ii += 1;
    }
    ii == i.len()
}

/// Allows a concrete topic for publishing.
pub fn allows_topic(allow: &[String], topic: &str) -> bool {
    allow.iter().any(|a| matches(a, topic))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn allow() -> Vec<String> {
        vec![
            "zigbee2mqtt/#".to_string(),
            "homeassistant/status".to_string(),
        ]
    }

    #[test]
    fn wildcard_matching() {
        assert!(matches("zigbee2mqtt/#", "zigbee2mqtt/bridge/state"));
        assert!(matches(
            "zigbee2mqtt/+/state",
            "zigbee2mqtt/0x00158d0001/state"
        ));
        assert!(!matches("zigbee2mqtt/+/state", "zigbee2mqtt/a/b/state"));
        assert!(matches("homeassistant/status", "homeassistant/status"));
        assert!(!matches("homeassistant/status", "homeassistant/status/x"));
    }

    #[test]
    fn dollar_topics_are_not_matched_by_wildcards() {
        assert!(!matches("#", "$SYS/broker/uptime"));
        assert!(matches("$SYS/#", "$SYS/broker/uptime"));
    }

    #[test]
    fn subscribes_are_checked_filter_against_filter() {
        assert!(allows_filter(&allow(), "zigbee2mqtt/#"));
        assert!(allows_filter(&allow(), "zigbee2mqtt/bridge/state"));
        // A narrower filter inside an allowed tree is a subset of it and therefore
        // allowed — the tree already permits those topics.
        assert!(allows_filter(&allow(), "zigbee2mqtt/+/set"));
        // Broader than the allowlist is not.
        assert!(!allows_filter(&allow(), "#"));
        assert!(!allows_filter(&allow(), "+/bridge/state"));
        assert!(!allows_filter(&allow(), "homeassistant/#"));
    }

    #[test]
    fn publishes_are_checked_topic_against_filter() {
        assert!(allows_topic(&allow(), "zigbee2mqtt/bridge/state"));
        assert!(allows_topic(&allow(), "homeassistant/status"));
        assert!(!allows_topic(
            &allow(),
            "homeassistant/binary_sensor/x/config"
        ));
        assert!(!allows_topic(&allow(), "z2m_guard/state"));
    }

    #[test]
    fn valid_filters_accept_the_grammar() {
        for f in [
            "zigbee2mqtt/#",
            "#",
            "+",
            "zigbee2mqtt/+/set",
            "homeassistant/status",
            "$SYS/#",
        ] {
            assert!(is_valid_filter(f), "{f:?} should be valid");
        }
    }

    #[test]
    fn valid_filters_reject_typos_that_would_silently_match_nothing() {
        for f in [
            "",
            "zigbee2mqtt#",      // missing slash: one literal level, no match
            "zigbee2mqtt/",      // trailing empty level
            "/zigbee2mqtt",      // leading empty level
            "zigbee2mqtt//#",    // doubled separator
            "zigbee2mqtt/#/set", // `#` must be the last level
            "zigbee2mqtt/+x",    // `+` must be a whole level
            "zigbee2mqtt/x+",    // `+` must be a whole level
        ] {
            assert!(!is_valid_filter(f), "{f:?} should be rejected");
        }
    }

    #[test]
    fn every_rejected_filter_matches_nothing_useful() {
        // Guards the reason for the check: the old matcher accepted these without
        // complaint and then dropped every topic, which is indistinguishable from a
        // broken Z2M bridge.
        assert!(!allows_topic(
            &["zigbee2mqtt#".to_string()],
            "zigbee2mqtt/bridge/state"
        ));
    }
}
