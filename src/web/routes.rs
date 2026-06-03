use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::Arc;

use askama::Template;
use axum::extract::{Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{Html, IntoResponse, Response};
use axum::Json;
use rust_embed::RustEmbed;
use serde::Deserialize;
use serde_json::json;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::{Stream, StreamExt};

use crate::model::{CheckOutcome, Status};
use crate::state::AppState;
use crate::store::now_ms;

use super::view::{DashboardFragment, DashboardPage, DetailPage};

/// Static assets (CSS/JS/icons) embedded into the binary at compile time.
#[derive(RustEmbed)]
#[folder = "assets/"]
struct Assets;

#[derive(Deserialize)]
pub struct WindowQuery {
    #[serde(default)]
    window: Option<String>,
}

/// Render the full dashboard page.
pub async fn dashboard(
    State(state): State<Arc<AppState>>,
    Query(q): Query<WindowQuery>,
) -> Response {
    let window = q.window.as_deref().unwrap_or("24h");
    render(DashboardPage::build(&state, window).await)
}

/// Render just the dashboard content, for live refresh / window switching.
pub async fn partial(State(state): State<Arc<AppState>>, Query(q): Query<WindowQuery>) -> Response {
    let window = q.window.as_deref().unwrap_or("24h");
    render(DashboardFragment::build(&state, window).await)
}

/// Server-Sent Events: emits an `update` whenever any check's status changes.
pub async fn events(
    State(state): State<Arc<AppState>>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let rx = state.events.subscribe();
    // Treat lag/recv errors as "something changed" and still nudge clients.
    let stream = BroadcastStream::new(rx).map(|_| Ok(Event::default().data("update")));
    Sse::new(stream).keep_alive(KeepAlive::default())
}

/// Per-service detail page.
pub async fn detail(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Response {
    match DetailPage::build(&state, &id).await {
        Some(page) => render(page),
        None => (StatusCode::NOT_FOUND, "unknown service").into_response(),
    }
}

/// Trigger an immediate re-check of every check on a service.
pub async fn api_check_now(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Response {
    let cfg = state.config.load();
    let service = cfg
        .groups
        .iter()
        .flat_map(|g| &g.services)
        .find(|s| s.id_or_slug() == id);

    let Some(service) = service else {
        return (StatusCode::NOT_FOUND, "unknown service").into_response();
    };

    for c in &service.checks {
        state.fire_trigger(&format!("{}::{}", id, c.name));
    }
    (StatusCode::ACCEPTED, "ok").into_response()
}

/// Liveness probe for the hub itself.
pub async fn healthz() -> &'static str {
    "ok"
}

/// JSON snapshot of the current status of every check.
pub async fn api_status(State(state): State<Arc<AppState>>) -> Json<HashMap<String, CheckOutcome>> {
    let map = state
        .statuses
        .iter()
        .map(|e| (e.key().clone(), e.value().clone()))
        .collect();
    Json(map)
}

#[derive(Deserialize)]
pub struct HistoryQuery {
    #[serde(default)]
    window: Option<String>,
}

/// History for a single service across all its checks within a time window.
pub async fn api_history(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Query(q): Query<HistoryQuery>,
) -> Response {
    let win = window_ms(q.window.as_deref());
    let since = now_ms() - win;

    let cfg = state.config.load();
    let service = cfg
        .groups
        .iter()
        .flat_map(|g| &g.services)
        .find(|s| s.id_or_slug() == id);

    let Some(service) = service else {
        return (StatusCode::NOT_FOUND, "unknown service").into_response();
    };

    let mut checks = serde_json::Map::new();
    for c in &service.checks {
        let cid = format!("{}::{}", id, c.name);
        let points = match state.store.history(&cid, since).await {
            Ok(p) => p,
            Err(e) => {
                tracing::error!("history query failed for {cid}: {e}");
                return (StatusCode::INTERNAL_SERVER_ERROR, "history query failed").into_response();
            }
        };

        let total = points.len();
        let available = points.iter().filter(|p| p.status != Status::Down).count();
        let series: Vec<_> = points
            .iter()
            .map(|p| json!({ "ts": p.ts, "status": p.status.css(), "latencyMs": p.latency_ms }))
            .collect();

        checks.insert(
            c.name.clone(),
            json!({
                "points": series,
                "uptimePercent": if total == 0 {
                    serde_json::Value::Null
                } else {
                    json!(available as f64 / total as f64 * 100.0)
                },
            }),
        );
    }

    Json(json!({
        "service": id,
        "windowMs": win,
        "checks": serde_json::Value::Object(checks),
    }))
    .into_response()
}

/// Serve an embedded static asset.
pub async fn static_asset(Path(path): Path<String>) -> Response {
    match Assets::get(&path) {
        Some(content) => {
            let mime = mime_guess::from_path(&path).first_or_octet_stream();
            (
                [(header::CONTENT_TYPE, mime.to_string())],
                content.data.into_owned(),
            )
                .into_response()
        }
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

/// Render an Askama template, turning errors into a 500.
fn render<T: Template>(template: T) -> Response {
    match template.render() {
        Ok(html) => Html(html).into_response(),
        Err(err) => {
            tracing::error!(%err, "template render failed");
            (StatusCode::INTERNAL_SERVER_ERROR, "template error").into_response()
        }
    }
}

/// Parse a window like `1h`, `24h`, `7d`, `30m`, `2w`. Defaults to 24h.
fn window_ms(window: Option<&str>) -> i64 {
    fn parse(s: &str) -> Option<i64> {
        let s = s.trim();
        let split = s.find(|c: char| c.is_alphabetic())?;
        let (num, unit) = s.split_at(split);
        let n: i64 = num.trim().parse().ok()?;
        let mult = match unit.trim() {
            "m" | "min" => 60_000,
            "h" => 3_600_000,
            "d" => 86_400_000,
            "w" => 604_800_000,
            _ => return None,
        };
        Some(n * mult)
    }
    window.and_then(parse).unwrap_or(24 * 3_600_000)
}
