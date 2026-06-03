//! Periodic pruning of old check history.

use std::time::Duration;

use super::{now_ms, Store};

/// Spawn a background task that prunes history older than `retention_days`,
/// running once at startup and then hourly.
pub fn spawn(store: Store, retention_days: u64) {
    let retention_ms = retention_days.saturating_mul(24 * 60 * 60 * 1000) as i64;
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_secs(60 * 60));
        loop {
            ticker.tick().await;
            let cutoff = now_ms() - retention_ms;
            match store.prune(cutoff).await {
                Ok(0) => {}
                Ok(n) => tracing::info!("pruned {n} check rows older than {retention_days}d"),
                Err(e) => tracing::warn!("retention prune failed: {e}"),
            }
        }
    });
}
