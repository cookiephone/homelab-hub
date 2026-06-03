//! Core domain types shared across monitoring, storage and the web layer.

use serde::Serialize;

/// The health status of a single check or an aggregated service.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Up,
    Degraded,
    Down,
    Unknown,
}

impl Status {
    /// CSS class / machine-readable token for this status.
    pub fn css(self) -> &'static str {
        match self {
            Status::Up => "up",
            Status::Degraded => "degraded",
            Status::Down => "down",
            Status::Unknown => "unknown",
        }
    }

    /// Parse a status from its `css()` token (used when reading from storage).
    pub fn from_css(s: &str) -> Status {
        match s {
            "up" => Status::Up,
            "degraded" => Status::Degraded,
            "down" => Status::Down,
            _ => Status::Unknown,
        }
    }

    /// Human-readable label.
    pub fn label(self) -> &'static str {
        match self {
            Status::Up => "Up",
            Status::Degraded => "Degraded",
            Status::Down => "Down",
            Status::Unknown => "Unknown",
        }
    }

    /// Severity ordering used when aggregating several checks into one status.
    /// Higher wins, so a single `Down` makes the whole service `Down`.
    fn severity(self) -> u8 {
        match self {
            Status::Unknown => 0,
            Status::Up => 1,
            Status::Degraded => 2,
            Status::Down => 3,
        }
    }

    /// Returns the more severe of two statuses.
    pub fn worst(a: Status, b: Status) -> Status {
        if b.severity() > a.severity() {
            b
        } else {
            a
        }
    }
}

/// The result of running a single health check once. Also used as the cached
/// "current state" of a check.
#[derive(Debug, Clone, Serialize)]
pub struct CheckOutcome {
    pub status: Status,
    /// Round-trip latency in milliseconds, when measurable.
    pub latency_ms: Option<u64>,
    /// HTTP status code, for http checks.
    pub http_code: Option<u16>,
    /// Human-readable error / reason, when not fully `Up`.
    pub error: Option<String>,
    /// When this check last ran (unix epoch ms). Set by the scheduler.
    pub checked_at: Option<i64>,
}

impl CheckOutcome {
    pub fn down(error: impl Into<String>) -> Self {
        Self {
            status: Status::Down,
            latency_ms: None,
            http_code: None,
            error: Some(error.into()),
            checked_at: None,
        }
    }
}

/// A single historical data point read back from storage.
#[derive(Debug, Clone)]
pub struct HistPoint {
    /// Unix epoch milliseconds.
    pub ts: i64,
    pub status: Status,
    pub latency_ms: Option<u64>,
}
