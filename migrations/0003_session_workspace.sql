-- v1.2 session workspace: session metadata, restorable checkpoints,
-- project memory, and live session search.

-- Session organization metadata. This is presentation/workspace state, not
-- replay-audit state, so it lives outside the event log. parent_id links a
-- forked session to its source. deleted is a soft delete; the event log is
-- preserved for audit.
CREATE TABLE IF NOT EXISTS session_meta (
    session_id TEXT PRIMARY KEY,
    title TEXT,
    archived INTEGER NOT NULL DEFAULT 0 CHECK(archived IN (0, 1)),
    pinned INTEGER NOT NULL DEFAULT 0 CHECK(pinned IN (0, 1)),
    parent_id TEXT,
    deleted INTEGER NOT NULL DEFAULT 0 CHECK(deleted IN (0, 1)),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_session_meta_deleted
ON session_meta(deleted, archived);

-- Restorable worktree checkpoints. The patch blob is captured at checkpoint
-- time so a later "restore here" can reverse-apply it. The CheckpointCreated
-- event remains the durable audit record.
CREATE TABLE IF NOT EXISTS session_checkpoints (
    id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL,
    sequence INTEGER NOT NULL,
    label TEXT NOT NULL,
    head TEXT NOT NULL,
    patch BLOB NOT NULL,
    patch_digest TEXT NOT NULL,
    created_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_session_checkpoints_session
ON session_checkpoints(session_id, sequence);

-- Durable project knowledge, keyed by repository. Entries are user-authored
-- and auditable: every row carries its source, confidence, and scope.
CREATE TABLE IF NOT EXISTS project_memory (
    id TEXT PRIMARY KEY,
    repository TEXT NOT NULL,
    kind TEXT NOT NULL,
    content TEXT NOT NULL,
    source TEXT NOT NULL,
    confidence TEXT NOT NULL DEFAULT 'unverified',
    scope TEXT NOT NULL DEFAULT 'repository',
    created_at TEXT NOT NULL,
    last_used_at TEXT
);

CREATE INDEX IF NOT EXISTS idx_project_memory_repo
ON project_memory(repository, kind);

-- Full-text search over the durable event log. Backfilled from existing
-- events so every session becomes searchable immediately.
CREATE VIRTUAL TABLE IF NOT EXISTS session_search USING fts5(
    session_id UNINDEXED,
    event_type,
    payload
);

INSERT INTO session_search(session_id, event_type, payload)
SELECT session_id, event_type, payload FROM session_events
WHERE NOT EXISTS (
    SELECT 1 FROM session_search
    WHERE session_search.session_id = session_events.session_id
);

INSERT OR IGNORE INTO schema_migrations(version, applied_at)
VALUES (3, datetime('now'));
