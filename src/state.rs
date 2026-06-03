//! Shared application state passed to every request handler and background task.

use std::sync::Arc;
use std::sync::Mutex;
use std::time::Instant;

use arc_swap::ArcSwap;
use dashmap::DashMap;
use tokio::sync::{broadcast, Notify};
use tokio::task::AbortHandle;

use crate::config::Config;
use crate::model::CheckOutcome;
use crate::store::Store;

/// Application state. Cheap to clone behind an `Arc`.
///
/// `config` is wrapped in an `ArcSwap` so it can be hot-swapped atomically when
/// the config file changes, without blocking readers.
pub struct AppState {
    pub config: ArcSwap<Config>,
    /// Latest outcome per check id (`"<service-id>::<check-name>"`).
    pub statuses: DashMap<String, CheckOutcome>,
    pub store: Store,
    /// Notifies connected SSE clients when any check's status changes.
    pub events: broadcast::Sender<()>,
    /// Abort handles for the currently running monitor tasks, so they can be
    /// cancelled and respawned when the config is hot-reloaded.
    pub monitors: Mutex<Vec<AbortHandle>>,
    /// Per-check "run now" signals, used by the manual "Check now" button.
    pub triggers: DashMap<String, Arc<Notify>>,
    pub started_at: Instant,
}

impl AppState {
    pub fn new(config: Config, store: Store) -> Self {
        let (events, _) = broadcast::channel(64);
        Self {
            config: ArcSwap::from_pointee(config),
            statuses: DashMap::new(),
            store,
            events,
            monitors: Mutex::new(Vec::new()),
            triggers: DashMap::new(),
            started_at: Instant::now(),
        }
    }

    /// Get (creating if needed) the "run now" signal for a check.
    pub fn trigger_for(&self, check_id: &str) -> Arc<Notify> {
        self.triggers
            .entry(check_id.to_string())
            .or_insert_with(|| Arc::new(Notify::new()))
            .clone()
    }

    /// Signal a check to run immediately, if it is being monitored.
    pub fn fire_trigger(&self, check_id: &str) {
        if let Some(n) = self.triggers.get(check_id) {
            n.notify_one();
        }
    }

    /// Record the latest outcome for a check, notifying SSE clients if the
    /// status (not just latency) changed.
    pub fn set_status(&self, check_id: &str, outcome: CheckOutcome) {
        let changed = self.statuses.get(check_id).map(|o| o.status) != Some(outcome.status);
        self.statuses.insert(check_id.to_string(), outcome);
        if changed {
            let _ = self.events.send(());
        }
    }

    /// Full latest outcome for a check id, if any.
    pub fn outcome_for(&self, check_id: &str) -> Option<CheckOutcome> {
        self.statuses.get(check_id).map(|o| o.value().clone())
    }
}
