//! Minimal MQTT packet codec — just enough to *relay* packets.
//!
//! The proxy does not implement a broker. It reads whole packets off one socket
//! and writes them to the other, inspecting only what it must: the CONNECT (to
//! re-issue it upstream with the same protocol level and the client's will), the
//! PUBLISH topic/QoS/retain flags (for the allowlist and the discovery dedupe),
//! and the SUBSCRIBE topic filters.
//!
//! Everything else is forwarded as the exact bytes it arrived as. That is a
//! deliberate constraint: the moment the proxy re-encodes a PUBLISH, it has to
//! understand properties, reason codes and the version differences between
//! MQTT 3.1.1 and 5.0 — the very thing this design avoids.

use std::io;

use tokio::io::{AsyncRead, AsyncReadExt};

pub const CONNECT: u8 = 1;
pub const CONNACK: u8 = 2;
pub const PUBLISH: u8 = 3;
pub const PUBACK: u8 = 4;
pub const SUBSCRIBE: u8 = 8;
pub const SUBACK: u8 = 9;
pub const PINGREQ: u8 = 12;
pub const PINGRESP: u8 = 13;
pub const DISCONNECT: u8 = 14;

/// Reads one complete MQTT packet, including its fixed header.
///
/// Returns `Ok(None)` on a clean EOF *before* the first byte, which is how a
/// client that closes between packets looks. A truncated packet is an error.
pub async fn read_packet<R: AsyncRead + Unpin>(r: &mut R) -> io::Result<Option<Vec<u8>>> {
    let mut head = [0u8; 1];
    match r.read_exact(&mut head).await {
        Ok(_) => {}
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    }

    let mut len_bytes = Vec::with_capacity(4);
    let mut remaining: usize = 0;
    for i in 0..4 {
        let mut b = [0u8; 1];
        r.read_exact(&mut b).await?;
        len_bytes.push(b[0]);
        remaining |= ((b[0] & 0x7f) as usize) << (7 * i);
        if b[0] & 0x80 == 0 {
            break;
        }
    }

    let mut body = vec![0u8; remaining];
    r.read_exact(&mut body).await?;

    let mut packet = Vec::with_capacity(1 + len_bytes.len() + remaining);
    packet.push(head[0]);
    packet.extend_from_slice(&len_bytes);
    packet.extend_from_slice(&body);
    Ok(Some(packet))
}

pub fn packet_type(packet: &[u8]) -> u8 {
    packet[0] >> 4
}

/// The variable-length remaining-length field can be 1..=4 bytes.
fn header_len(packet: &[u8]) -> usize {
    let mut i = 1;
    while i < packet.len() && packet[i] & 0x80 != 0 {
        i += 1;
    }
    i + 1
}

pub fn body(packet: &[u8]) -> &[u8] {
    let h = header_len(packet);
    if h > packet.len() {
        &[]
    } else {
        &packet[h..]
    }
}

fn put_string(out: &mut Vec<u8>, s: &str) {
    out.extend_from_slice(&(s.len() as u16).to_be_bytes());
    out.extend_from_slice(s.as_bytes());
}

fn put_remaining_len(out: &mut Vec<u8>, mut len: usize) {
    loop {
        let mut b = (len % 128) as u8;
        len /= 128;
        if len > 0 {
            b |= 0x80;
        }
        out.push(b);
        if len == 0 {
            break;
        }
    }
}

fn read_string(body: &[u8], pos: &mut usize) -> Option<String> {
    if *pos + 2 > body.len() {
        return None;
    }
    let n = u16::from_be_bytes([body[*pos], body[*pos + 1]]) as usize;
    *pos += 2;
    if *pos + n > body.len() {
        return None;
    }
    let s = String::from_utf8(body[*pos..*pos + n].to_vec()).ok()?;
    *pos += n;
    Some(s)
}

fn read_bytes_chunk<'a>(body: &'a [u8], pos: &mut usize) -> Option<&'a [u8]> {
    if *pos + 2 > body.len() {
        return None;
    }
    let n = u16::from_be_bytes([body[*pos], body[*pos + 1]]) as usize;
    *pos += 2;
    if *pos + n > body.len() {
        return None;
    }
    let s = &body[*pos..*pos + n];
    *pos += n;
    Some(s)
}

#[derive(Debug, Clone, PartialEq)]
pub struct Will {
    pub topic: String,
    pub payload: Vec<u8>,
    pub qos: u8,
    pub retain: bool,
}

/// What the proxy needs from a CONNECT: the protocol level it must speak
/// upstream, and the will it must carry so the broker still announces the
/// client's death when the *proxy's* socket is the one that lives on.
#[derive(Debug, Clone, PartialEq)]
pub struct ConnectInfo {
    pub protocol_level: u8,
    pub clean_session: bool,
    pub keep_alive: u16,
    pub client_id: String,
    pub will: Option<Will>,
}

impl ConnectInfo {
    pub fn is_mqtt5(&self) -> bool {
        self.protocol_level == 5
    }
}

pub fn parse_connect(packet: &[u8]) -> Option<ConnectInfo> {
    if packet_type(packet) != CONNECT {
        return None;
    }
    let b = body(packet);
    let mut pos = 0usize;
    let _protocol_name = read_string(b, &mut pos)?;
    if pos >= b.len() {
        return None;
    }
    let protocol_level = b[pos];
    pos += 1;
    if pos + 3 > b.len() {
        return None;
    }
    let flags = b[pos];
    pos += 1;
    let keep_alive = u16::from_be_bytes([b[pos], b[pos + 1]]);
    pos += 2;

    // MQTT 5 has a properties block here; it is length-prefixed, so it can be
    // skipped without knowing any property id.
    if protocol_level == 5 {
        let props_len = read_varint(b, &mut pos)?;
        pos += props_len;
        if pos > b.len() {
            return None;
        }
    }

    let client_id = read_string(b, &mut pos)?;

    let will = if flags & 0x04 != 0 {
        // MQTT 5 puts a length-prefixed property block in front of the will topic.
        // Without this the will payload's length prefix is read from the property
        // bytes and the will arrives corrupted — found by adding an MQTT 5 client
        // after the broker log showed HA negotiating level 5.
        if protocol_level == 5 {
            let props_len = read_varint(b, &mut pos)?;
            pos += props_len;
            if pos > b.len() {
                return None;
            }
        }
        let topic = read_string(b, &mut pos)?;
        let payload = read_bytes_chunk(b, &mut pos)?.to_vec();
        Some(Will {
            topic,
            payload,
            qos: (flags >> 3) & 0x03,
            retain: flags & 0x20 != 0,
        })
    } else {
        None
    };

    Some(ConnectInfo {
        protocol_level,
        clean_session: flags & 0x02 != 0,
        keep_alive,
        client_id,
        will,
    })
}

fn read_varint(b: &[u8], pos: &mut usize) -> Option<usize> {
    let mut value = 0usize;
    for i in 0..4 {
        if *pos >= b.len() {
            return None;
        }
        let byte = b[*pos];
        *pos += 1;
        value |= ((byte & 0x7f) as usize) << (7 * i);
        if byte & 0x80 == 0 {
            return Some(value);
        }
    }
    None
}

/// Rebuilds the CONNECT the proxy sends upstream: same protocol level, same will,
/// but its own client id and clean session.
pub fn build_connect(
    level: u8,
    client_id: &str,
    will: Option<&Will>,
    username: Option<&str>,
    password: Option<&str>,
    keep_alive: u16,
) -> Vec<u8> {
    let mut v = Vec::new();
    put_string(&mut v, "MQTT");
    v.push(level);

    let mut flags = 0x02; // clean session
    if let Some(w) = will {
        flags |= 0x04;
        flags |= (w.qos & 0x03) << 3;
        if w.retain {
            flags |= 0x20;
        }
    }
    if username.is_some() {
        flags |= 0x80;
    }
    if password.is_some() {
        flags |= 0x40;
    }
    v.push(flags);
    v.extend_from_slice(&keep_alive.to_be_bytes());

    if level == 5 {
        v.push(0); // empty properties
    }

    put_string(&mut v, client_id);
    if will.is_some() && level == 5 {
        // Same field order as above: the will's property block comes first.
        v.push(0);
    }
    if let Some(w) = will {
        put_string(&mut v, &w.topic);
        v.extend_from_slice(&(w.payload.len() as u16).to_be_bytes());
        v.extend_from_slice(&w.payload);
    }
    if let Some(u) = username {
        put_string(&mut v, u);
    }
    if let Some(p) = password {
        put_string(&mut v, p);
    }

    let mut out = vec![CONNECT << 4];
    put_remaining_len(&mut out, v.len());
    out.extend_from_slice(&v);
    out
}

/// CONNACK for the downstream client: echoes the broker's verdict.
pub fn build_connack(level: u8, session_present: bool, code: u8) -> Vec<u8> {
    let mut v = Vec::new();
    v.push(u8::from(session_present));
    v.push(code);
    if level == 5 {
        v.push(0); // empty properties
    }
    let mut out = vec![CONNACK << 4];
    put_remaining_len(&mut out, v.len());
    out.extend_from_slice(&v);
    out
}

pub fn build_puback(level: u8, packet_id: u16) -> Vec<u8> {
    let mut v = Vec::new();
    v.extend_from_slice(&packet_id.to_be_bytes());
    if level == 5 {
        v.push(0); // reason code: success
        v.push(0); // empty properties
    }
    let mut out = vec![PUBACK << 4];
    put_remaining_len(&mut out, v.len());
    out.extend_from_slice(&v);
    out
}

pub fn build_suback(level: u8, packet_id: u16, codes: &[u8]) -> Vec<u8> {
    let mut v = Vec::new();
    v.extend_from_slice(&packet_id.to_be_bytes());
    if level == 5 {
        v.push(0);
    }
    v.extend_from_slice(codes);
    let mut out = vec![SUBACK << 4];
    put_remaining_len(&mut out, v.len());
    out.extend_from_slice(&v);
    out
}

#[derive(Debug, Clone, PartialEq)]
pub struct PublishInfo {
    pub topic: String,
    pub qos: u8,
    pub retain: bool,
    pub packet_id: Option<u16>,
    pub payload_offset: usize,
}

pub fn parse_publish(packet: &[u8]) -> Option<PublishInfo> {
    if packet_type(packet) != PUBLISH {
        return None;
    }
    let flags = packet[0] & 0x0f;
    let qos = (flags >> 1) & 0x03;
    let retain = flags & 0x01 != 0;
    let h = header_len(packet);
    if h > packet.len() {
        return None;
    }
    let b = &packet[h..];
    let mut pos = 0usize;
    let topic = read_string(b, &mut pos)?;
    let packet_id = if qos > 0 {
        if pos + 2 > b.len() {
            return None;
        }
        let id = u16::from_be_bytes([b[pos], b[pos + 1]]);
        pos += 2;
        Some(id)
    } else {
        None
    };
    Some(PublishInfo {
        topic,
        qos,
        retain,
        packet_id,
        payload_offset: h + pos,
    })
}

pub fn publish_payload<'a>(packet: &'a [u8], info: &PublishInfo) -> &'a [u8] {
    &packet[info.payload_offset.min(packet.len())..]
}

/// Topic filters of a SUBSCRIBE packet, in order.
pub fn parse_subscribe(packet: &[u8]) -> Option<(u16, Vec<String>)> {
    if packet_type(packet) != SUBSCRIBE {
        return None;
    }
    let b = body(packet);
    if b.len() < 2 {
        return None;
    }
    let packet_id = u16::from_be_bytes([b[0], b[1]]);
    let mut pos = 2usize;
    // MQTT 5 puts properties between the packet id and the first filter.
    if packet[0] & 0x0f == 0x02 {
        // SUBSCRIBE properties exist only in v5 sessions; the caller knows the
        // level, so v5 handling is done there by skipping a varint block.
    }
    let mut filters = Vec::new();
    while pos < b.len() {
        let f = read_string(b, &mut pos)?;
        filters.push(f);
        if pos < b.len() {
            pos += 1; // requested QoS
        }
    }
    Some((packet_id, filters))
}

/// Same as [`parse_subscribe`], but aware of the session's protocol level.
pub fn parse_subscribe_at(packet: &[u8], level: u8) -> Option<(u16, Vec<String>)> {
    if packet_type(packet) != SUBSCRIBE {
        return None;
    }
    let b = body(packet);
    if b.len() < 2 {
        return None;
    }
    let packet_id = u16::from_be_bytes([b[0], b[1]]);
    let mut pos = 2usize;
    if level == 5 {
        let n = read_varint(b, &mut pos)?;
        pos += n;
    }
    let mut filters = Vec::new();
    while pos < b.len() {
        let f = read_string(b, &mut pos)?;
        filters.push(f);
        if pos < b.len() {
            pos += 1;
        }
    }
    Some((packet_id, filters))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_connect_with_will_and_level_4() {
        let will = Will {
            topic: "zigbee2mqtt/bridge/state".into(),
            payload: b"offline".to_vec(),
            qos: 0,
            retain: true,
        };
        let pkt = build_connect(
            4,
            "zigbee2mqtt",
            Some(&will),
            Some("z2mguard"),
            Some("secret"),
            60,
        );
        let info = parse_connect(&pkt).expect("connect");
        assert_eq!(info.protocol_level, 4);
        assert!(info.clean_session);
        assert_eq!(info.client_id, "zigbee2mqtt");
        assert_eq!(info.will, Some(will));
        assert_eq!(info.keep_alive, 60);
    }

    #[test]
    fn parses_connect_with_will_and_level_5() {
        // The will sits behind its own property block in MQTT 5. Both directions are
        // checked: what we parse, and what we re-issue upstream must stay readable.
        let will = Will {
            topic: "zigbee2mqtt/bridge/state".into(),
            payload: br#"{"state":"offline"}"#.to_vec(),
            qos: 1,
            retain: true,
        };
        let pkt = build_connect(
            5,
            "zigbee2mqtt",
            Some(&will),
            Some("z2mguard"),
            Some("secret"),
            45,
        );
        let info = parse_connect(&pkt).expect("connect");
        assert_eq!(info.protocol_level, 5);
        assert!(info.is_mqtt5());
        assert_eq!(info.client_id, "zigbee2mqtt");
        assert_eq!(info.will, Some(will.clone()));
        assert_eq!(info.keep_alive, 45);
    }

    #[test]
    fn parses_a_client_built_mqtt5_connect_with_will_properties() {
        // Hand-assembled the way a real MQTT 5 client sends it: connect properties
        // (session expiry = 0, 1 byte) and will properties (payload format = UTF-8,
        // 2 bytes) in front of the will topic. A parser that ignores the will property
        // block reads the payload length from these bytes and corrupts the will.
        let mut body = Vec::new();
        put_string(&mut body, "MQTT");
        body.push(5);
        body.push(0x26); // clean start + will + will retain
        body.extend_from_slice(&30u16.to_be_bytes());
        body.push(2); // connect properties length
        body.push(0x11); // session expiry interval
        body.push(0x00);
        put_string(&mut body, "ha-client");
        body.push(2); // will properties length
        body.push(0x01); // payload format indicator
        body.push(0x01); // UTF-8
        put_string(&mut body, "homeassistant/status");
        body.extend_from_slice(&5u16.to_be_bytes());
        body.extend_from_slice(b"gone!");

        let mut pkt = vec![CONNECT << 4];
        put_remaining_len(&mut pkt, body.len());
        pkt.extend_from_slice(&body);

        let info = parse_connect(&pkt).expect("connect");
        assert_eq!(info.protocol_level, 5);
        assert_eq!(info.client_id, "ha-client");
        let will = info.will.expect("will");
        assert_eq!(will.topic, "homeassistant/status");
        assert_eq!(will.payload, b"gone!");
        assert!(will.retain);
    }

    #[test]
    fn parses_connect_without_will_and_level_5_properties() {
        let pkt = build_connect(5, "client-1", None, None, None, 30);
        let info = parse_connect(&pkt).expect("connect");
        assert_eq!(info.protocol_level, 5);
        assert!(info.will.is_none());
        assert!(info.is_mqtt5());
    }

    #[test]
    fn parses_publish_qos1_and_extracts_payload() {
        // topic "a/b", packet id 7, payload {"x":1}
        let mut body_pkt = Vec::new();
        put_string(&mut body_pkt, "a/b");
        body_pkt.extend_from_slice(&7u16.to_be_bytes());
        body_pkt.extend_from_slice(b"{\"x\":1}");
        let mut pkt = vec![PUBLISH << 4 | 0b0010];
        put_remaining_len(&mut pkt, body_pkt.len());
        pkt.extend_from_slice(&body_pkt);

        let info = parse_publish(&pkt).expect("publish");
        assert_eq!(info.topic, "a/b");
        assert_eq!(info.qos, 1);
        assert!(!info.retain);
        assert_eq!(info.packet_id, Some(7));
        assert_eq!(publish_payload(&pkt, &info), b"{\"x\":1}");
    }

    #[test]
    fn parses_subscribe_filters() {
        let mut body_pkt = Vec::new();
        body_pkt.extend_from_slice(&3u16.to_be_bytes());
        put_string(&mut body_pkt, "zigbee2mqtt/#");
        body_pkt.push(0);
        put_string(&mut body_pkt, "homeassistant/status");
        body_pkt.push(1);
        let mut pkt = vec![SUBSCRIBE << 4 | 0x02];
        put_remaining_len(&mut pkt, body_pkt.len());
        pkt.extend_from_slice(&body_pkt);

        let (id, filters) = parse_subscribe_at(&pkt, 4).expect("subscribe");
        assert_eq!(id, 3);
        assert_eq!(filters, vec!["zigbee2mqtt/#", "homeassistant/status"]);
    }

    #[test]
    fn connack_shape_differs_by_level() {
        assert_eq!(build_connack(4, false, 0).len(), 4); // hdr, len, flags, code
        assert_eq!(build_connack(5, false, 0).len(), 5); // + properties
        assert_eq!(build_puback(4, 9).len(), 4);
        assert_eq!(build_puback(5, 9).len(), 6);
    }
}
