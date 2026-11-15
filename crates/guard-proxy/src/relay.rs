//! One proxied session: the Zigbee2MQTT client on one side, the broker on the other.
//!
//! The proxy terminates the downstream CONNECT, re-issues it upstream with the
//! same protocol level and the **client's will**, and then relays packets in both
//! directions. The will matters: if it were dropped, the broker would never
//! publish `offline` for the bridge when Z2M dies, and the availability topic the
//! whole design rests on would go stale.

use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::io::AsyncWriteExt;
use tokio::net::tcp::OwnedWriteHalf;
use tokio::net::TcpStream;
use tokio::time::timeout;
use tracing::{info, warn};

use crate::allow::{allows_filter, allows_topic};
use crate::dedupe::Dedupe;
use crate::packet::{
    body, build_connack, build_connect, build_puback, packet_type, parse_connect, parse_publish,
    parse_subscribe_at, publish_payload, read_packet, CONNACK, DISCONNECT, PINGREQ, PUBLISH,
    SUBSCRIBE,
};
use crate::{RelaySettings, Stats};

pub async fn run_session(
    down: TcpStream,
    settings: Arc<RelaySettings>,
    stats: Arc<Stats>,
    dedupe: Arc<Mutex<Dedupe>>,
) {
    let peer = down
        .peer_addr()
        .map(|a| a.to_string())
        .unwrap_or_else(|_| "?".into());

    stats.sessions.fetch_add(1, Ordering::Relaxed);
    match session(down, &settings, &stats, &dedupe).await {
        Ok(()) => info!(peer, "proxy session closed"),
        Err(e) => {
            stats.session_errors.fetch_add(1, Ordering::Relaxed);
            warn!(peer, error = %e, "proxy session ended with an error");
        }
    }
}

async fn session(
    mut down: TcpStream,
    settings: &RelaySettings,
    stats: &Stats,
    dedupe: &Mutex<Dedupe>,
) -> std::io::Result<()> {
    // ---- downstream CONNECT ---------------------------------------------------
    let Some(connect_pkt) = read_or_none(&mut down).await? else {
        return Ok(());
    };
    let Some(connect) = parse_connect(&connect_pkt) else {
        warn!("first packet was not a CONNECT, closing");
        return Ok(());
    };
    let level = connect.protocol_level;

    // ---- upstream CONNECT -----------------------------------------------------
    let mut up = match timeout(
        Duration::from_secs(10),
        TcpStream::connect(settings.upstream),
    )
    .await
    {
        Ok(Ok(s)) => s,
        other => {
            let why = match other {
                Ok(Err(e)) => e.to_string(),
                _ => "timeout".to_string(),
            };
            warn!(upstream = %settings.upstream, error = %why, "cannot reach upstream broker");
            // Answer instead of dropping the socket: a silent close looks like a
            // broker restart to Z2M and hides the actual cause.
            down.write_all(&build_connack(level, false, 0x03)).await?; // server unavailable
            return Ok(());
        }
    };

    let n = stats.sessions.load(Ordering::Relaxed);
    let upstream_id = format!("{}-{}", settings.client_prefix, n);
    up.write_all(&build_connect(
        level,
        &upstream_id,
        connect.will.as_ref(),
        settings.username.as_deref(),
        settings.password.as_deref(),
        connect.keep_alive,
    ))
    .await?;

    let Some(connack) = read_or_none(&mut up).await? else {
        warn!("upstream closed before CONNACK");
        down.write_all(&build_connack(level, false, 0x03)).await?;
        return Ok(());
    };
    if packet_type(&connack) != CONNACK || body(&connack).len() < 2 {
        warn!("upstream's first reply was not a CONNACK");
        return Ok(());
    }
    let code = body(&connack)[1];
    down.write_all(&connack).await?;
    if code != 0 {
        warn!(code, "broker refused the upstream connection");
        return Ok(());
    }
    info!(
        client = %connect.client_id,
        level,
        will = connect.will.as_ref().map(|w| w.topic.as_str()).unwrap_or("-"),
        upstream_id,
        "proxy session established"
    );

    // ---- relay ----------------------------------------------------------------
    let (mut dr, mut dw) = down.into_split();
    let (mut ur, mut uw) = up.into_split();

    loop {
        tokio::select! {
            r = read_packet(&mut dr) => {
                let pkt = match r { Ok(Some(p)) => p, Ok(None) => break, Err(e) => return Err(e) };
                if !forward_client(pkt, level, settings, stats, dedupe, &mut uw, &mut dw).await? {
                    break;
                }
            }
            r = read_packet(&mut ur) => {
                let pkt = match r { Ok(Some(p)) => p, Ok(None) => break, Err(e) => return Err(e) };
                let last = packet_type(&pkt) == DISCONNECT;
                dw.write_all(&pkt).await?;
                if last { break; }
            }
        }
    }

    let _ = dw.shutdown().await;
    let _ = uw.shutdown().await;
    Ok(())
}

async fn read_or_none(s: &mut TcpStream) -> std::io::Result<Option<Vec<u8>>> {
    match timeout(Duration::from_secs(10), read_packet(s)).await {
        Ok(r) => r,
        Err(_) => {
            warn!("handshake timed out");
            Ok(None)
        }
    }
}

/// Relays one client → broker packet. Returns `false` when the session must end.
async fn forward_client(
    pkt: Vec<u8>,
    level: u8,
    settings: &RelaySettings,
    stats: &Stats,
    dedupe: &Mutex<Dedupe>,
    upstream: &mut OwnedWriteHalf,
    downstream: &mut OwnedWriteHalf,
) -> std::io::Result<bool> {
    match packet_type(&pkt) {
        PUBLISH => {
            let Some(info) = parse_publish(&pkt) else {
                warn!("malformed PUBLISH, closing");
                return Ok(false);
            };
            if !allows_topic(&settings.allow, &info.topic) {
                stats.denied.fetch_add(1, Ordering::Relaxed);
                warn!(topic = %info.topic, "publish outside the allowlist, closing (fail closed)");
                return Ok(false);
            }

            let payload = publish_payload(&pkt, &info);
            let forward = {
                // A poisoned lock only means another session panicked while holding
                // it; the window is still usable, so recover instead of failing.
                let mut d = dedupe.lock().unwrap_or_else(|e| e.into_inner());
                d.should_forward(&info.topic, payload, info.retain, Instant::now())
            };
            if !forward {
                stats.deduped.fetch_add(1, Ordering::Relaxed);
                // QoS 1 must still be acknowledged, or Z2M retransmits exactly the
                // storm the dedupe just removed.
                if let Some(id) = info.packet_id {
                    // Acknowledge to the *client*, never upstream: the broker has
                    // not seen this packet and its PUBACK state knows nothing of it.
                    downstream.write_all(&build_puback(level, id)).await?;
                }
                return Ok(true);
            }
            upstream.write_all(&pkt).await?;
        }
        SUBSCRIBE => {
            let Some((_, filters)) = parse_subscribe_at(&pkt, level) else {
                warn!("malformed SUBSCRIBE, closing");
                return Ok(false);
            };
            if let Some(f) = filters.iter().find(|f| !allows_filter(&settings.allow, f)) {
                stats.denied.fetch_add(1, Ordering::Relaxed);
                warn!(filter = %f, "subscription outside the allowlist, closing (fail closed)");
                return Ok(false);
            }
            upstream.write_all(&pkt).await?;
        }
        PINGREQ | DISCONNECT => {
            upstream.write_all(&pkt).await?;
            if packet_type(&pkt) == DISCONNECT {
                return Ok(false);
            }
        }
        _ => upstream.write_all(&pkt).await?,
    }
    Ok(true)
}
