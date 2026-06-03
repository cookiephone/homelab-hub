-- Raw check history. One row per check execution.
CREATE TABLE IF NOT EXISTS checks (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    check_id   TEXT    NOT NULL,        -- "<service-id>::<check-name>"
    ts         INTEGER NOT NULL,        -- unix epoch milliseconds
    status     TEXT    NOT NULL,        -- up | degraded | down
    latency_ms INTEGER,
    http_code  INTEGER,
    error      TEXT
);

CREATE INDEX IF NOT EXISTS idx_checks_check_ts ON checks (check_id, ts);
