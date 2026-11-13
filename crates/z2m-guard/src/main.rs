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

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_target(false)
        .init();

    let cfg = Config::load(cli.config.as_deref())?;
    let metrics = Arc::new(Metrics::new());
    let (tx, rx) = mpsc::channel::<Command>(16);

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
