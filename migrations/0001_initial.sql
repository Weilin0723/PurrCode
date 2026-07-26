CREATE TABLE IF NOT EXISTS schema_migrations (
    version INTEGER PRIMARY KEY,
    applied_at TEXT NOT NULL
);

INSERT OR IGNORE INTO schema_migrations(version, applied_at)
VALUES (1, datetime('now'));

CREATE TABLE IF NOT EXISTS session_events (
    session_id TEXT NOT NULL,
    sequence INTEGER NOT NULL,
    event_type TEXT NOT NULL,
    payload TEXT NOT NULL CHECK(json_valid(payload)),
    occurred_at TEXT NOT NULL,
    PRIMARY KEY(session_id, sequence)
);

CREATE INDEX IF NOT EXISTS idx_session_events_type
ON session_events(session_id, event_type);

CREATE TABLE IF NOT EXISTS authorizations (
    action_id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL,
    action_digest TEXT NOT NULL,
    constraints TEXT NOT NULL CHECK(json_valid(constraints)),
    authorized_at TEXT NOT NULL,
    approved_by TEXT NOT NULL CHECK(json_valid(approved_by)),
    consumed_at TEXT
);

