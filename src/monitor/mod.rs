//! The monitoring core: a `Checker` abstraction plus a scheduler that runs one
//! polling task per check and feeds results into the shared status cache.

mod http;
#[cfg(feature = "ping")]
mod ping;
mod tcp;
mod tls;

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::Notify;

use crate::config::{Check, CheckType, Config, Defaults};
use crate::model::CheckOutcome;
use crate::state::AppState;

/// Something that can probe a target and report a health outcome.
///
/// New check types implement this trait and get registered in [`build_checker`].
#[async_trait]
pub trait Checker: Send + Sync {
    async fn check(&self) -> CheckOutcome;
}

/// Build a checker for a check definition, applying `defaults` for any unset
/// timing fields. Returns `None` for unsupported types (e.g. ping when not
/// compiled in), which leaves the check showing as `Unknown`.
fn build_checker(check: &Check, defaults: &Defaults) -> Option<Box<dyn Checker>> {
    let timeout = Duration::from_secs(check.timeout(defaults).max(1));
    let warn_ms = check.warn_ms(defaults);
    match check.kind {
        CheckType::Http => match http::HttpChecker::new(check, timeout, warn_ms) {
            Ok(c) => Some(Box::new(c)),
            Err(e) => {
                tracing::error!("failed to build http checker for '{}': {e}", check.name);
                None
            }
        },
        CheckType::Tcp => Some(Box::new(tcp::TcpChecker::new(check, timeout, warn_ms))),
        CheckType::Tls => {
            match tls::TlsChecker::new(check, timeout, check.warn_cert_days(defaults)) {
                Ok(c) => Some(Box::new(c)),
                Err(e) => {
                    tracing::error!("failed to build tls checker for '{}': {e}", check.name);
                    None
                }
            }
        }
        CheckType::Ping => {
            #[cfg(feature = "ping")]
            {
                Some(Box::new(ping::PingChecker::new(check, timeout, warn_ms)))
            }
            #[cfg(not(feature = "ping"))]
            {
                tracing::warn!(
                    "ping checks require the 'ping' build feature (check '{}'); skipping",
                    check.name
                );
                None
            }
        }
    }
}

/// (Re)spawn one polling task per supported check in the current config.
///
/// Any previously spawned monitor tasks are aborted first, so this is safe to
/// call again after a config hot-reload.
pub fn spawn(state: Arc<AppState>) {
    let cfg: Arc<Config> = state.config.load_full();

    let mut handles = state.monitors.lock().unwrap();
    for h in handles.drain(..) {
        h.abort();
    }

    let mut count = 0usize;
    for group in &cfg.groups {
        for service in &group.services {
            let sid = service.id_or_slug();
            for check in &service.checks {
                let check_id = format!("{}::{}", sid, check.name);
                if let Some(checker) = build_checker(check, &cfg.defaults) {
                    let interval = Duration::from_secs(check.interval(&cfg.defaults).max(1));
                    let trigger = state.trigger_for(&check_id);
                    let handle = tokio::spawn(run_loop(
                        state.clone(),
                        check_id,
                        checker,
                        interval,
                        trigger,
                    ));
                    handles.push(handle.abort_handle());
                    count += 1;
                }
            }
        }
    }
    tracing::info!("monitoring {count} checks");
}

/// Poll a single check forever at its interval. The first tick fires
/// immediately so the dashboard populates on startup.
async fn run_loop(
    state: Arc<AppState>,
    check_id: String,
    checker: Box<dyn Checker>,
    interval: Duration,
    trigger: Arc<Notify>,
) {
    let mut ticker = tokio::time::interval(interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        // Run on the regular cadence, or immediately when "Check now" fires.
        tokio::select! {
            _ = ticker.tick() => {}
            _ = trigger.notified() => {}
        }
        let mut outcome = checker.check().await;
        let ts = crate::store::now_ms();
        outcome.checked_at = Some(ts);
        tracing::debug!(
            check = %check_id,
            status = ?outcome.status,
            latency_ms = ?outcome.latency_ms,
            "check completed"
        );
        if let Err(e) = state.store.record(&check_id, &outcome, ts).await {
            tracing::warn!("failed to persist result for {check_id}: {e}");
        }
        state.set_status(&check_id, outcome);
    }
}
