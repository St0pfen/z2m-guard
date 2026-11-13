//! HTTP surface: `/healthz`, `/metrics` and `POST /v1/restart-intent`.
//!
//! The intent endpoint is how a compose wrapper or Watchtower hook tells the
//! guard "a restart is coming". It is authenticated with a shared token because
//! a forged signal would silence real alarms.

use std::sync::Arc;

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use crate::metrics::Metrics;

/// Work the HTTP layer hands to the runtime task.
#[derive(Debug)]
pub enum Command {
    PriorSignal {
        reason: String,
        window_ms: u64,
    },
    /// Optional Docker attribution (host mode, events piped on stdin).
    /// Only constructed by the `stdin-events` feature — part of the contract
    /// either way, so it stays visible in the default build.
    #[allow(dead_code)]
    ContainerDie {
        exit_code: i32,
    },
    /// Internal: stop the runtime cleanly (SIGINT).
    Shutdown,
}

#[derive(Debug, Clone)]
pub struct AppState {
    pub metrics: Arc<Metrics>,
    pub tx: mpsc::Sender<Command>,
    pub token: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
pub struct IntentBody {
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub window_s: Option<u64>,
}

#[derive(Debug, Serialize)]
struct OkResponse {
    accepted: bool,
    window_s: u64,
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/metrics", get(metrics))
        .route("/v1/restart-intent", post(restart_intent))
        .with_state(state)
}

async fn healthz() -> impl IntoResponse {
    (StatusCode::OK, "ok\n")
}

async fn metrics(State(state): State<AppState>) -> impl IntoResponse {
    (
        StatusCode::OK,
        [("content-type", "text/plain; version=0.0.4")],
        state.metrics.render(),
    )
}

async fn restart_intent(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Option<Json<IntentBody>>,
) -> impl IntoResponse {
    let Some(expected) = state.token.as_deref() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "no token configured — refusing restart intents\n".to_string(),
        );
    };

    let provided = headers
        .get("x-guard-token")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();

    if !constant_time_eq(provided.as_bytes(), expected.as_bytes()) {
        return (
            StatusCode::UNAUTHORIZED,
            "missing or wrong X-Guard-Token\n".to_string(),
        );
    }

    let body = body.map(|Json(b)| b).unwrap_or_default();
    let window_s = body.window_s.unwrap_or(120).clamp(1, 3600);
    let reason = body.reason.unwrap_or_else(|| "manual".to_string());

    if state
        .tx
        .send(Command::PriorSignal {
            reason,
            window_ms: window_s * 1_000,
        })
        .await
        .is_err()
    {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "guard runtime is shutting down\n".to_string(),
        );
    }

    (
        StatusCode::ACCEPTED,
        serde_json::to_string(&OkResponse {
            accepted: true,
            window_s,
        })
        .unwrap_or_else(|_| "{\"accepted\":true}".to_string()),
    )
}

/// Length-safe comparison that does not bail out on the first differing byte.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constant_time_eq_matches_expected_semantics() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"ab"));
        assert!(constant_time_eq(b"", b""));
    }
}
