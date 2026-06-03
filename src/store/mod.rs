//! SQLite-backed persistence of check history.
//!
//! Given the very low write rate (one insert per check per interval), a shared
//! pool with WAL journaling and a busy-timeout is more than enough and keeps the
//! code simple: at homelab scale no dedicated writer task is needed.

pub mod retention;

use std::path::Path;
use std::time::Duration;

use anyhow::Context;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};
use sqlx::{Row, SqlitePool};

use crate::model::{CheckOutcome, HistPoint, Status};

/// A historical check row including its error text (for the detail page).
pub struct EventRow {
    pub ts: i64,
    pub status: Status,
    pub latency_ms: Option<u64>,
    pub error: Option<String>,
}

/// Current unix time in milliseconds.
pub fn now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[derive(Clone)]
pub struct Store {
    pool: SqlitePool,
}

impl Store {
    /// Open (creating if needed) the SQLite database at `path` and run migrations.
    pub async fn open(path: &Path) -> anyhow::Result<Self> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).ok();
            }
        }

        let opts = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Normal)
            .busy_timeout(Duration::from_secs(5));

        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect_with(opts)
            .await
            .with_context(|| format!("opening database {}", path.display()))?;

        sqlx::migrate!("./migrations")
            .run(&pool)
            .await
            .context("running database migrations")?;

        Ok(Self { pool })
    }

    /// Append one check result.
    pub async fn record(&self, check_id: &str, o: &CheckOutcome, ts_ms: i64) -> anyhow::Result<()> {
        sqlx::query(
            "INSERT INTO checks (check_id, ts, status, latency_ms, http_code, error) \
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(check_id)
        .bind(ts_ms)
        .bind(o.status.css())
        .bind(o.latency_ms.map(|v| v as i64))
        .bind(o.http_code.map(|v| v as i64))
        .bind(o.error.as_deref())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Append many results for one check in a single transaction. Used by the
    /// demo seeder, which writes thousands of synthetic points at once; doing it
    /// per-row through the pool would be needlessly slow.
    pub async fn record_many(
        &self,
        check_id: &str,
        rows: &[(i64, CheckOutcome)],
    ) -> anyhow::Result<()> {
        let mut tx = self.pool.begin().await?;
        for (ts, o) in rows {
            sqlx::query(
                "INSERT INTO checks (check_id, ts, status, latency_ms, http_code, error) \
                 VALUES (?, ?, ?, ?, ?, ?)",
            )
            .bind(check_id)
            .bind(ts)
            .bind(o.status.css())
            .bind(o.latency_ms.map(|v| v as i64))
            .bind(o.http_code.map(|v| v as i64))
            .bind(o.error.as_deref())
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// Read history for one check since `since_ms`, oldest first.
    pub async fn history(&self, check_id: &str, since_ms: i64) -> anyhow::Result<Vec<HistPoint>> {
        let rows = sqlx::query(
            "SELECT ts, status, latency_ms FROM checks \
             WHERE check_id = ? AND ts >= ? ORDER BY ts ASC",
        )
        .bind(check_id)
        .bind(since_ms)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .iter()
            .map(|r| HistPoint {
                ts: r.get::<i64, _>("ts"),
                status: Status::from_css(&r.get::<String, _>("status")),
                latency_ms: r.get::<Option<i64>, _>("latency_ms").map(|v| v as u64),
            })
            .collect())
    }

    /// Most recent check rows (newest first), including the error text. Backs the
    /// per-service detail page's events table.
    pub async fn recent_events(&self, check_id: &str, limit: i64) -> anyhow::Result<Vec<EventRow>> {
        let rows = sqlx::query(
            "SELECT ts, status, latency_ms, error FROM checks \
             WHERE check_id = ? ORDER BY ts DESC LIMIT ?",
        )
        .bind(check_id)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .iter()
            .map(|r| EventRow {
                ts: r.get::<i64, _>("ts"),
                status: Status::from_css(&r.get::<String, _>("status")),
                latency_ms: r.get::<Option<i64>, _>("latency_ms").map(|v| v as u64),
                error: r.get::<Option<String>, _>("error"),
            })
            .collect())
    }

    /// Latest outcome per check id, used to warm the in-memory cache on boot.
    pub async fn latest_all(&self) -> anyhow::Result<Vec<(String, CheckOutcome)>> {
        let rows = sqlx::query(
            "SELECT c.check_id, c.ts, c.status, c.latency_ms, c.http_code, c.error \
             FROM checks c \
             JOIN (SELECT check_id, MAX(ts) AS m FROM checks GROUP BY check_id) t \
               ON c.check_id = t.check_id AND c.ts = t.m",
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .iter()
            .map(|r| {
                let outcome = CheckOutcome {
                    status: Status::from_css(&r.get::<String, _>("status")),
                    latency_ms: r.get::<Option<i64>, _>("latency_ms").map(|v| v as u64),
                    http_code: r.get::<Option<i64>, _>("http_code").map(|v| v as u16),
                    error: r.get::<Option<String>, _>("error"),
                    checked_at: Some(r.get::<i64, _>("ts")),
                };
                (r.get::<String, _>("check_id"), outcome)
            })
            .collect())
    }

    /// Delete rows older than `older_than_ms`. Returns the number removed.
    pub async fn prune(&self, older_than_ms: i64) -> anyhow::Result<u64> {
        let res = sqlx::query("DELETE FROM checks WHERE ts < ?")
            .bind(older_than_ms)
            .execute(&self.pool)
            .await?;
        Ok(res.rows_affected())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    /// A unique temp DB path; the file is removed at the end of each test.
    async fn temp_store() -> (Store, std::path::PathBuf) {
        let nanos = now_ms() as u64;
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let mut path = std::env::temp_dir();
        path.push(format!(
            "hub-test-{}-{}-{}.db",
            std::process::id(),
            nanos,
            n
        ));
        let store = Store::open(&path).await.unwrap();
        (store, path)
    }

    fn outcome(status: Status, latency: Option<u64>) -> CheckOutcome {
        CheckOutcome {
            status,
            latency_ms: latency,
            http_code: None,
            error: None,
            checked_at: None,
        }
    }

    #[tokio::test]
    async fn record_history_latest_and_prune() {
        let (store, path) = temp_store().await;

        store
            .record("svc::web", &outcome(Status::Up, Some(10)), 1000)
            .await
            .unwrap();
        store
            .record("svc::web", &CheckOutcome::down("boom"), 2000)
            .await
            .unwrap();

        // Full history, oldest first.
        let pts = store.history("svc::web", 0).await.unwrap();
        assert_eq!(pts.len(), 2);
        assert_eq!(pts[0].status, Status::Up);
        assert_eq!(pts[1].status, Status::Down);

        // Window filter excludes the older row.
        let recent = store.history("svc::web", 1500).await.unwrap();
        assert_eq!(recent.len(), 1);

        // latest_all returns the most recent row per check id.
        let latest = store.latest_all().await.unwrap();
        assert_eq!(latest.len(), 1);
        assert_eq!(latest[0].0, "svc::web");
        assert_eq!(latest[0].1.status, Status::Down);

        // Prune removes rows older than the cutoff.
        let removed = store.prune(1500).await.unwrap();
        assert_eq!(removed, 1);
        assert_eq!(store.history("svc::web", 0).await.unwrap().len(), 1);

        drop(store);
        let _ = std::fs::remove_file(&path);
    }
}
