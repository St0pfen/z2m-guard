//! `z2m-guard` binary — wiring of the crates: config, MQTT, HA discovery,
//! metrics, restart signals.

#![forbid(unsafe_code)]

mod config;
mod http;
mod metrics;
mod runtime;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use clap::Parser;
use guard_ha::HaMeta;
use tokio::sync::mpsc;
use tracing_subscriber::EnvFilter;

use crate::config::Config;
use crate::http::{AppState, Command};
use crate::metrics::Metrics;

#[derive(Debug, Parser)]
#[command(
    name = "z2m-guard",
    version,
    about = "Separates planned Zigbee2MQTT restarts from real outages"
)]
struct Cli {
    /// Path to the TOML config. Defaults to environment only.
    #[arg(long)]
    config: Option<PathBuf>,

    /// Print the Home Assistant discovery payloads and exit (no broker needed).
    #[arg(long)]
    print_discovery: bool,

    /// Read `docker events --format '{{json .}}'` from stdin for attribution
    /// (build with --features stdin-events).
    #[arg(long)]
    docker_events_stdin: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    if cli.print_discovery {
        for d in guard_ha::all(&HaMeta::default()) {
            println!("=== {}\n{}", d.topic, d.payload);
        }
        return Ok(());
    }

    // Checked before anything binds: failing after the listener is up produced
    // "Address already in use" when a second instance was started, which hid the
    // actual reason.
    #[cfg(not(feature = "stdin-events"))]
    if cli.docker_events_stdin {
        anyhow::bail!("--docker-events-stdin requires a build with --features stdin-events");
    }

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_target(false)
        .init();

    let cfg = Config::load(cli.config.as_deref())?;
    cfg.validate()?;
    let metrics = Arc::new(Metrics::new());
    let (tx, rx) = mpsc::channel::<Command>(16);

    // ---- optional proxy mode (M5) ---------------------------------------------
    // Feature-gated *and* config-gated: a binary built without `--features proxy`
    // must refuse `proxy.enabled = true` loudly instead of silently running the
    // guard without the proxy a deployment believes is there.
    #[cfg(not(feature = "proxy"))]
    if cfg.proxy.enabled {
        anyhow::bail!("proxy.enabled = true requires a build with --features proxy");
    }

    #[cfg(feature = "proxy")]
    if cfg.proxy.enabled {
        let listen: SocketAddr = cfg
            .proxy
            .listen
            .parse()
            .with_context(|| format!("invalid proxy.listen: {}", cfg.proxy.listen))?;
        let upstream_url = cfg
            .proxy
            .upstream
            .clone()
            .unwrap_or_else(|| cfg.mqtt.url.clone());
        let hp = guard_mqtt::parse_broker_url(&upstream_url)
            .map_err(|e| anyhow::anyhow!("proxy upstream: {e}"))?;
        let upstream: SocketAddr = tokio::net::lookup_host((hp.host.as_str(), hp.port))
            .await
            .with_context(|| format!("resolving proxy upstream {upstream_url}"))?
            .next()
            .ok_or_else(|| anyhow::anyhow!("proxy upstream {upstream_url} resolved to nothing"))?;

        let mut settings = guard_proxy::RelaySettings::new(upstream);
        settings.allow = cfg.proxy.allow.clone();
        settings.dedupe_window = std::time::Duration::from_millis(cfg.proxy.dedupe_window_ms);
        settings.client_prefix = cfg.proxy.client_prefix.clone();
        settings.username = Some(cfg.mqtt.username.clone());
        settings.password = cfg.mqtt_password();

        let pstats = Arc::new(guard_proxy::Stats::default());
        // The proxy owns its counters; /metrics reads a snapshot once a second
        // rather than sharing an atomic across crates.
        let mirror = Arc::clone(&pstats);
        let m = Arc::clone(&metrics);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                m.set_discovery_deduped(mirror.deduped());
            }
        });

        let serve_stats = Arc::clone(&pstats);
        tokio::spawn(async move {
            if let Err(e) = guard_proxy::serve(listen, settings, serve_stats).await {
                tracing::error!(error = %e, "proxy listener stopped");
            }
        });
        tracing::info!(%listen, %upstream, "proxy mode enabled");
    }

    let bind: SocketAddr = cfg
        .http
        .bind
        .parse()
        .with_context(|| format!("invalid http.bind: {}", cfg.http.bind))?;
    let state = AppState {
        metrics: Arc::clone(&metrics),
        tx: tx.clone(),
        token: cfg.http_token(),
    };
    let router = http::router(state);
    let listener = tokio::net::TcpListener::bind(bind)
        .await
        .with_context(|| format!("binding {bind}"))?;
    tracing::info!(%bind, "http listening");
    let server = tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, router).await {
            tracing::error!(error = %e, "http server stopped");
        }
    });

    let shutdown_tx = tx.clone();
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            let _ = shutdown_tx.send(Command::Shutdown).await;
        }
    });

    #[cfg(feature = "stdin-events")]
    if cli.docker_events_stdin {
        let events_tx = tx.clone();
        tokio::spawn(async move {
            let stdin = tokio::io::stdin();
            guard_docker::read_events(tokio::io::BufReader::new(stdin), |event| {
                if let guard_docker::ContainerEvent::Die { exit_code, .. } = event {
                    let tx = events_tx.clone();
                    tokio::spawn(async move {
                        let _ = tx.send(Command::ContainerDie { exit_code }).await;
                    });
                }
            })
            .await;
        });
    }
    #[cfg(not(feature = "stdin-events"))]
    if cli.docker_events_stdin {
        anyhow::bail!("--docker-events-stdin requires a build with --features stdin-events");
    }

    runtime::run(cfg, metrics, rx).await?;
    server.abort();
    Ok(())
}
