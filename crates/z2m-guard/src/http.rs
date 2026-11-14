//! HTTP surface: `/healthz`, `/metrics` and `POST /v1/restart-intent`.
//!
//! The intent endpoint is how a compose wrapper or Watchtower hook tells the
//! guard "a restart is coming". It is authenticated with a shared token because
//! a forged signal would silence real alarms.

use std::sync::Arc;
use std::time::Duration;

use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::Router;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tower_http::timeout::TimeoutLayer;

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

/// 5 s cap on a request: a stalled client must not pin a task, and the layer
/// answers 408 (not 500) so a timeout stays distinguishable from a broken handler.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);

pub fn router(state: AppState) -> Router {
    with_limits(
        Router::new()
            .route("/healthz", get(healthz))
            .route("/metrics", get(metrics))
            // The only endpoint with a body, and its body is a few bytes of JSON.
            // Anything larger is a mistake or an attack.
            .route("/v1/restart-intent", post(restart_intent)),
    )
    .with_state(state)
}

fn with_limits<S>(router: Router<S>) -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    with_limits_for(router, REQUEST_TIMEOUT)
}

/// The one place the limits are configured, so the test proving 408 exercises
/// the same layer construction the shipped router uses.
fn with_limits_for<S>(router: Router<S>, timeout: Duration) -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    router
        .layer(DefaultBodyLimit::max(4096))
        .layer(TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            timeout,
        ))
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
    body: Bytes,
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

    // Parsed by hand instead of via `Option<Json<_>>`: an optional extractor turns a
    // rejection (malformed JSON, oversized body) into `None`, so a caller would get
    // 202 while the window silently fell back to the default. Empty body keeps
    // meaning "defaults"; anything else must be valid JSON or it is a 400.
    let body = if body.is_empty() {
        IntentBody::default()
    } else {
        match serde_json::from_slice::<IntentBody>(&body) {
            Ok(b) => b,
            Err(e) => return (StatusCode::BAD_REQUEST, format!("invalid JSON body: {e}\n")),
        }
    };

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
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    #[test]
    fn constant_time_eq_matches_expected_semantics() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"ab"));
        assert!(constant_time_eq(b"", b""));
    }

    fn app_token(token: Option<&str>) -> (Router, mpsc::Receiver<Command>) {
        let (tx, rx) = mpsc::channel(8);
        let state = AppState {
            metrics: Arc::new(Metrics::new()),
            tx,
            token: token.map(str::to_string),
        };
        (router(state), rx)
    }

    async fn post_intent(app: Router, token: &str, body: Body) -> StatusCode {
        let req = Request::post("/v1/restart-intent")
            .header("x-guard-token", token)
            .header("content-type", "application/json")
            .body(body)
            .expect("request");
        app.oneshot(req).await.expect("response").status()
    }

    #[tokio::test]
    async fn empty_body_opens_the_default_window() {
        let (app, mut rx) = app_token(Some("t"));
        let status = post_intent(app, "t", Body::empty()).await;
        assert_eq!(status, StatusCode::ACCEPTED);
        match rx.try_recv() {
            Ok(Command::PriorSignal { window_ms, .. }) => assert_eq!(window_ms, 120_000),
            other => panic!("expected PriorSignal, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn malformed_json_is_a_client_error_not_a_silent_default() {
        let (app, mut rx) = app_token(Some("t"));
        let status = post_intent(app, "t", Body::from("{not json")).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(rx.try_recv().is_err(), "no window may open on a bad body");
    }

    #[tokio::test]
    async fn oversized_body_is_rejected_by_the_limit() {
        let (app, mut rx) = app_token(Some("t"));
        let big = Body::from("a".repeat(8192));
        let status = post_intent(app, "t", big).await;
        assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn wrong_token_is_unauthorized() {
        let (app, mut rx) = app_token(Some("t"));
        let status = post_intent(app, "wrong", Body::empty()).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn missing_token_configuration_refuses_every_intent() {
        let (app, mut rx) = app_token(None);
        let status = post_intent(app, "anything", Body::empty()).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn a_hung_request_answers_408_not_500() {
        async fn slow() -> &'static str {
            tokio::time::sleep(Duration::from_millis(200)).await;
            "late"
        }
        // Same layer construction as `router`, only with a test-sized deadline.
        let app: Router = with_limits_for(
            Router::new().route("/slow", get(slow)),
            Duration::from_millis(20),
        );
        let req = Request::get("/slow").body(Body::empty()).expect("request");
        let status = app.oneshot(req).await.expect("response").status();
        assert_eq!(status, StatusCode::REQUEST_TIMEOUT);
    }
}
