CREATE TABLE IF NOT EXISTS automations (
    id TEXT PRIMARY KEY,
    objective TEXT NOT NULL,
    repository TEXT NOT NULL,
    interval_seconds INTEGER NOT NULL CHECK(interval_seconds >= 60),
    enabled INTEGER NOT NULL CHECK(enabled IN (0, 1)),
    next_run_at TEXT NOT NULL,
    last_session_id TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_automations_due
ON automations(enabled, next_run_at);

INSERT OR IGNORE INTO schema_migrations(version, applied_at)
VALUES (2, datetime('now'));
