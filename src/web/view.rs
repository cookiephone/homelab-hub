//! View models: plain, template-friendly data assembled from the config, the
//! live status cache and the stored history. Templates only see
//! `String`/`Vec`/`bool` (no `Option`), which keeps the Askama templates simple.
//!
//! The same [`DashboardData`] backs both the full page ([`DashboardPage`]) and
//! the partial returned for live refreshes / window changes ([`DashboardFragment`]).

use askama::Template;

use crate::config::Theme;
use crate::model::{HistPoint, Status};
use crate::state::AppState;
use crate::store::now_ms;

/// Selectable uptime windows, in display order.
const WINDOWS: &[&str] = &["1h", "24h", "7d", "30d"];
/// Default window when none/invalid is requested.
const DEFAULT_WINDOW: &str = "24h";
/// Number of segments in an uptime bar.
const SEGMENTS: usize = 40;
/// Window shown on the per-service detail page.
const DETAIL_WINDOW_MS: i64 = 30 * 86_400_000;

fn window_ms(value: &str) -> i64 {
    match value {
        "1h" => 3_600_000,
        "24h" => 86_400_000,
        "7d" => 7 * 86_400_000,
        "30d" => 30 * 86_400_000,
        _ => 86_400_000,
    }
}

/// The full dashboard HTML page.
#[derive(Template)]
#[template(path = "dashboard.html")]
pub struct DashboardPage {
    pub data: DashboardData,
}

/// The inner content only, for live refresh / window switching.
#[derive(Template)]
#[template(path = "fragment.html")]
pub struct DashboardFragment {
    pub data: DashboardData,
}

impl DashboardPage {
    pub async fn build(state: &AppState, window: &str) -> Self {
        Self {
            data: build_data(state, window).await,
        }
    }
}

impl DashboardFragment {
    pub async fn build(state: &AppState, window: &str) -> Self {
        Self {
            data: build_data(state, window).await,
        }
    }
}

pub struct DashboardData {
    pub title: String,
    pub subtitle: String,
    /// `data-theme` attribute value; empty for "auto" (defer to OS preference).
    pub theme_attr: String,
    pub refresh_interval: u64,
    pub window_value: String,
    pub windows: Vec<WindowOption>,
    pub hub_uptime: String,
    pub summary: Summary,
    pub groups: Vec<GroupView>,
}

pub struct WindowOption {
    pub value: String,
    pub label: String,
    pub active: bool,
}

pub struct Summary {
    pub up: usize,
    pub degraded: usize,
    pub down: usize,
    pub unknown: usize,
    pub total: usize,
}

pub struct GroupView {
    pub name: String,
    pub icon: String,
    pub up: usize,
    pub total: usize,
    pub services: Vec<ServiceView>,
}

pub struct ServiceView {
    pub id: String,
    pub name: String,
    pub description: String,
    pub icon_url: String,
    pub initial: String,
    pub status_css: String,
    pub status_label: String,
    pub tags: Vec<String>,
    /// Host/IP (and port where given) endpoints, derived from links + checks.
    pub addresses: Vec<String>,
    /// Lowercased name + tags + addresses, used for client-side filtering.
    pub filter_text: String,
    pub links: Vec<LinkView>,
    pub checks: Vec<CheckView>,
    pub has_checks: bool,
}

pub struct LinkView {
    pub label: String,
    pub url: String,
    pub primary: bool,
}

pub struct CheckView {
    pub name: String,
    pub kind: String,
    pub target: String,
    pub status_css: String,
    pub status_label: String,
    /// Failure / degraded reason from the last run; empty when fine.
    pub reason: String,
    /// e.g. "12s ago"; empty if never run.
    pub checked_text: String,
    pub uptime: UptimeView,
}

pub struct UptimeView {
    pub percent_text: String,
    pub latency_text: String,
    pub segments: Vec<SegmentView>,
}

pub struct SegmentView {
    pub css: String,
    pub title: String,
}

/// Assemble the dashboard data for the given window.
async fn build_data(state: &AppState, window: &str) -> DashboardData {
    let window_value = if WINDOWS.contains(&window) {
        window
    } else {
        DEFAULT_WINDOW
    };
    let cfg = state.config.load();
    let now = now_ms();
    let since = now - window_ms(window_value);

    let mut summary = Summary {
        up: 0,
        degraded: 0,
        down: 0,
        unknown: 0,
        total: 0,
    };

    let mut groups = Vec::with_capacity(cfg.groups.len());
    for g in &cfg.groups {
        let mut services = Vec::with_capacity(g.services.len());
        let mut group_up = 0usize;
        let mut group_total = 0usize;
        for s in &g.services {
            let sid = s.id_or_slug();

            // Build each check's view and aggregate the service status
            // (worst of all checks; Unknown if there are none).
            let mut svc_status = Status::Unknown;
            let mut checks = Vec::with_capacity(s.checks.len());
            for c in &s.checks {
                let cid = format!("{}::{}", sid, c.name);
                let outcome = state.outcome_for(&cid);
                let st = outcome
                    .as_ref()
                    .map(|o| o.status)
                    .unwrap_or(Status::Unknown);
                svc_status = Status::worst(svc_status, st);

                let points = state.store.history(&cid, since).await.unwrap_or_default();
                let uptime = build_uptime(
                    &points,
                    since,
                    now,
                    outcome.as_ref().and_then(|o| o.latency_ms),
                );

                let reason = outcome
                    .as_ref()
                    .and_then(|o| o.error.clone())
                    .unwrap_or_default();
                let checked_text = outcome
                    .as_ref()
                    .and_then(|o| o.checked_at)
                    .map(|t| rel_time(now - t))
                    .unwrap_or_default();

                checks.push(CheckView {
                    name: c.name.clone(),
                    kind: c.kind.label().to_string(),
                    target: c.target.clone(),
                    status_css: st.css().to_string(),
                    status_label: st.label().to_string(),
                    reason,
                    checked_text,
                    uptime,
                });
            }

            summary.total += 1;
            group_total += 1;
            let svc_up = svc_status == Status::Up;
            if svc_up {
                group_up += 1;
            }
            match svc_status {
                Status::Up => summary.up += 1,
                Status::Degraded => summary.degraded += 1,
                Status::Down => summary.down += 1,
                Status::Unknown => summary.unknown += 1,
            }

            let addresses = service_addresses(s);
            let filter_text =
                format!("{} {} {}", s.name, s.tags.join(" "), addresses.join(" ")).to_lowercase();

            services.push(ServiceView {
                id: sid,
                name: s.name.clone(),
                description: s.description.clone().unwrap_or_default(),
                icon_url: icon_url(s.icon.as_deref()),
                initial: initial(&s.name),
                status_css: svc_status.css().to_string(),
                status_label: svc_status.label().to_string(),
                filter_text,
                tags: s.tags.clone(),
                addresses,
                links: s
                    .links
                    .iter()
                    .map(|l| LinkView {
                        label: l.label.clone(),
                        url: l.url.clone(),
                        primary: l.primary,
                    })
                    .collect(),
                has_checks: !s.checks.is_empty(),
                checks,
            });
        }

        groups.push(GroupView {
            name: g.name.clone(),
            icon: g.icon.clone().unwrap_or_default(),
            up: group_up,
            total: group_total,
            services,
        });
    }

    let theme_attr = match cfg.theme {
        Theme::Auto => "",
        Theme::Light => "light",
        Theme::Dark => "dark",
    }
    .to_string();

    let windows = WINDOWS
        .iter()
        .map(|w| WindowOption {
            value: (*w).to_string(),
            label: (*w).to_string(),
            active: *w == window_value,
        })
        .collect();

    DashboardData {
        title: cfg.title.clone(),
        subtitle: cfg.subtitle.clone().unwrap_or_default(),
        theme_attr,
        refresh_interval: cfg.refresh_interval,
        window_value: window_value.to_string(),
        windows,
        hub_uptime: format_uptime(state.started_at.elapsed()),
        summary,
        groups,
    }
}

/// The per-service detail page.
#[derive(Template)]
#[template(path = "detail.html")]
pub struct DetailPage {
    pub page_title: String,
    pub theme_attr: String,
    pub service_id: String,
    pub name: String,
    pub description: String,
    pub icon_url: String,
    pub initial: String,
    pub status_css: String,
    pub status_label: String,
    pub addresses: Vec<String>,
    pub has_checks: bool,
    pub links: Vec<LinkView>,
    pub checks: Vec<DetailCheck>,
}

pub struct DetailCheck {
    pub name: String,
    pub kind: String,
    pub target: String,
    pub status_css: String,
    pub status_label: String,
    pub reason: String,
    pub checked_text: String,
    pub uptime: UptimeView,
    pub events: Vec<EventView>,
}

pub struct EventView {
    pub time: String,
    pub status_css: String,
    pub status_label: String,
    pub latency_text: String,
    pub error: String,
}

impl DetailPage {
    /// Build the detail page for a service id, or `None` if it doesn't exist.
    pub async fn build(state: &AppState, id: &str) -> Option<Self> {
        let cfg = state.config.load();
        let service = cfg
            .groups
            .iter()
            .flat_map(|g| &g.services)
            .find(|s| s.id_or_slug() == id)?;

        let now = now_ms();
        let since = now - DETAIL_WINDOW_MS;

        let mut svc_status = Status::Unknown;
        let mut checks = Vec::with_capacity(service.checks.len());
        for c in &service.checks {
            let cid = format!("{}::{}", id, c.name);
            let outcome = state.outcome_for(&cid);
            let st = outcome
                .as_ref()
                .map(|o| o.status)
                .unwrap_or(Status::Unknown);
            svc_status = Status::worst(svc_status, st);

            let points = state.store.history(&cid, since).await.unwrap_or_default();
            let uptime = build_uptime(
                &points,
                since,
                now,
                outcome.as_ref().and_then(|o| o.latency_ms),
            );

            let reason = outcome
                .as_ref()
                .and_then(|o| o.error.clone())
                .unwrap_or_default();
            let checked_text = outcome
                .as_ref()
                .and_then(|o| o.checked_at)
                .map(|t| rel_time(now - t))
                .unwrap_or_default();

            let events = state
                .store
                .recent_events(&cid, 40)
                .await
                .unwrap_or_default()
                .into_iter()
                .map(|e| EventView {
                    time: rel_time(now - e.ts),
                    status_css: e.status.css().to_string(),
                    status_label: e.status.label().to_string(),
                    latency_text: e
                        .latency_ms
                        .map(|ms| format!("{ms} ms"))
                        .unwrap_or_else(|| "—".to_string()),
                    error: e.error.unwrap_or_default(),
                })
                .collect();

            checks.push(DetailCheck {
                name: c.name.clone(),
                kind: c.kind.label().to_string(),
                target: c.target.clone(),
                status_css: st.css().to_string(),
                status_label: st.label().to_string(),
                reason,
                checked_text,
                uptime,
                events,
            });
        }

        let theme_attr = match cfg.theme {
            Theme::Auto => "",
            Theme::Light => "light",
            Theme::Dark => "dark",
        }
        .to_string();

        Some(DetailPage {
            page_title: format!("{} · {}", service.name, cfg.title),
            theme_attr,
            service_id: id.to_string(),
            name: service.name.clone(),
            description: service.description.clone().unwrap_or_default(),
            icon_url: icon_url(service.icon.as_deref()),
            initial: initial(&service.name),
            status_css: svc_status.css().to_string(),
            status_label: svc_status.label().to_string(),
            addresses: service_addresses(service),
            has_checks: !service.checks.is_empty(),
            links: service
                .links
                .iter()
                .map(|l| LinkView {
                    label: l.label.clone(),
                    url: l.url.clone(),
                    primary: l.primary,
                })
                .collect(),
            checks,
        })
    }
}

/// Compact "running for" string, e.g. `3d 4h`, `5h 12m`, `7m`.
fn format_uptime(d: std::time::Duration) -> String {
    let secs = d.as_secs();
    let (days, hours, mins) = (secs / 86_400, (secs % 86_400) / 3600, (secs % 3600) / 60);
    if days > 0 {
        format!("{days}d {hours}h")
    } else if hours > 0 {
        format!("{hours}h {mins}m")
    } else {
        format!("{mins}m")
    }
}

/// Turn raw history points into an uptime bar (segments + uptime %).
fn build_uptime(
    points: &[HistPoint],
    since_ms: i64,
    now_ms: i64,
    current_latency: Option<u64>,
) -> UptimeView {
    let total = points.len();
    let available = points.iter().filter(|p| p.status != Status::Down).count();
    let percent_text = if total == 0 {
        "—".to_string()
    } else {
        format!("{:.1}%", available as f64 / total as f64 * 100.0)
    };

    // Bucket points across the window; each segment shows the worst status seen.
    let span = (now_ms - since_ms).max(1);
    let mut buckets = vec![Status::Unknown; SEGMENTS];
    for p in points {
        let rel = (p.ts - since_ms).clamp(0, span);
        let mut idx = ((rel as f64 / span as f64) * SEGMENTS as f64) as usize;
        if idx >= SEGMENTS {
            idx = SEGMENTS - 1;
        }
        buckets[idx] = Status::worst(buckets[idx], p.status);
    }

    let segments = buckets
        .into_iter()
        .map(|st| SegmentView {
            css: st.css().to_string(),
            title: if st == Status::Unknown {
                "No data".to_string()
            } else {
                st.label().to_string()
            },
        })
        .collect();

    let latency_text = current_latency
        .map(|ms| format!("{ms} ms"))
        .unwrap_or_else(|| "—".to_string());

    UptimeView {
        percent_text,
        latency_text,
        segments,
    }
}

/// Resolve a service icon to an image URL. Absolute `http(s)` URLs are used as-is;
/// a bare name (e.g. "jellyfin") resolves to the selfh.st icon CDN. The template
/// falls back to a letter avatar if the resulting image fails to load.
fn icon_url(icon: Option<&str>) -> String {
    match icon.map(str::trim) {
        None | Some("") => String::new(),
        Some(s) if s.starts_with("http://") || s.starts_with("https://") => s.to_string(),
        Some(name) => format!("https://cdn.jsdelivr.net/gh/selfhst/icons/svg/{name}.svg"),
    }
}

/// Compact "time ago" string for a millisecond delta, e.g. "just now", "12s ago".
fn rel_time(ms_ago: i64) -> String {
    let secs = ms_ago.max(0) / 1000;
    if secs < 5 {
        "just now".to_string()
    } else if secs < 60 {
        format!("{secs}s ago")
    } else if secs < 3600 {
        format!("{}m ago", secs / 60)
    } else if secs < 86_400 {
        format!("{}h ago", secs / 3600)
    } else {
        format!("{}d ago", secs / 86_400)
    }
}

/// The deduped set of address endpoints a service exposes, taken from its links
/// (the URL authority) and its checks (the target), in that order. Whatever the
/// user wrote (IP or domain, with or without a port) is shown verbatim.
fn service_addresses(s: &crate::config::Service) -> Vec<String> {
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut add = |raw: &str| {
        if let Some(a) = endpoint_label(raw) {
            if seen.insert(a.clone()) {
                out.push(a);
            }
        }
    };
    for l in &s.links {
        add(&l.url);
    }
    for c in &s.checks {
        add(&c.target);
    }
    out
}

/// Extract the `host[:port]` part from a URL, a `host:port`, or a bare host.
fn endpoint_label(s: &str) -> Option<String> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    // For URLs, take the authority (between "://" and the next '/', '?' or '#')
    // and drop any userinfo. For bare targets, use the string as-is.
    let authority = match s.split_once("://") {
        Some((_, rest)) => {
            let host = rest.split(['/', '?', '#']).next().unwrap_or(rest);
            host.rsplit('@').next().unwrap_or(host)
        }
        None => s,
    };
    if authority.is_empty() {
        None
    } else {
        Some(authority.to_string())
    }
}

/// First character of the name, uppercased, for the letter-avatar fallback.
fn initial(name: &str) -> String {
    name.trim()
        .chars()
        .next()
        .map(|c| c.to_uppercase().to_string())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pt(ts: i64, status: Status) -> HistPoint {
        HistPoint {
            ts,
            status,
            latency_ms: None,
        }
    }

    #[test]
    fn uptime_percent_and_bucketing() {
        let points = vec![
            pt(10, Status::Up),
            pt(50_000, Status::Down),
            pt(99_000, Status::Up),
        ];
        let u = build_uptime(&points, 0, 100_000, Some(7));

        assert_eq!(u.segments.len(), SEGMENTS);
        assert_eq!(u.percent_text, "66.7%"); // 2 of 3 not-down
        assert_eq!(u.latency_text, "7 ms");
        assert_eq!(u.segments[0].css, "up"); // ts=10 -> bucket 0
        assert_eq!(u.segments[20].css, "down"); // ts=50_000 -> bucket 20
    }

    #[test]
    fn endpoint_label_extracts_host_port() {
        assert_eq!(
            endpoint_label("https://192.168.1.10:8006"),
            Some("192.168.1.10:8006".to_string())
        );
        assert_eq!(
            endpoint_label("https://jellyfin.home.lab/health"),
            Some("jellyfin.home.lab".to_string())
        );
        assert_eq!(
            endpoint_label("jellyfin.home.lab:8096"),
            Some("jellyfin.home.lab:8096".to_string())
        );
        assert_eq!(
            endpoint_label("http://user:pw@host:81/x"),
            Some("host:81".to_string())
        );
        assert_eq!(endpoint_label("  "), None);
    }

    #[test]
    fn uptime_empty_is_all_unknown() {
        let u = build_uptime(&[], 0, 100, None);
        assert_eq!(u.percent_text, "—");
        assert_eq!(u.latency_text, "—");
        assert!(u.segments.iter().all(|s| s.css == "unknown"));
    }
}
