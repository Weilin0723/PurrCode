-- v1.3 project intelligence graph. Keyed on the SOURCE repository, exactly
-- like project_memory (0003_session_workspace.sql), because the graph is
-- project identity and must outlive any single session worktree.

CREATE TABLE IF NOT EXISTS graph_nodes (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    project TEXT NOT NULL,
    kind TEXT NOT NULL,
    -- ALWAYS repository-relative. Absolute keys are rejected by the writer.
    key TEXT NOT NULL,
    label TEXT NOT NULL,
    attributes TEXT NOT NULL DEFAULT '{}' CHECK(json_valid(attributes)),
    -- Re-applies whisker's sensitivity classifier at INSERT time. A sensitive
    -- node is never materialized into a prompt.
    sensitive INTEGER NOT NULL DEFAULT 0 CHECK(sensitive IN (0, 1)),
    observed_at TEXT NOT NULL,
    UNIQUE(project, kind, key)
);

CREATE INDEX IF NOT EXISTS idx_graph_nodes_lookup
ON graph_nodes(project, kind, key);

CREATE TABLE IF NOT EXISTS graph_edges (
    project TEXT NOT NULL,
    source_id INTEGER NOT NULL REFERENCES graph_nodes(id) ON DELETE CASCADE,
    target_id INTEGER NOT NULL REFERENCES graph_nodes(id) ON DELETE CASCADE,
    kind TEXT NOT NULL,
    -- 0..=1000. Traversal decays by this factor per hop.
    confidence_millis INTEGER NOT NULL CHECK(confidence_millis BETWEEN 0 AND 1000),
    -- event_log | git_history | static_analysis | heuristic
    edge_source TEXT NOT NULL,
    evidence TEXT NOT NULL DEFAULT '',
    observed_at TEXT NOT NULL,
    PRIMARY KEY(project, source_id, target_id, kind)
);

CREATE INDEX IF NOT EXISTS idx_graph_edges_out
ON graph_edges(project, source_id, kind, confidence_millis DESC);

CREATE INDEX IF NOT EXISTS idx_graph_edges_in
ON graph_edges(project, target_id, kind, confidence_millis DESC);

-- Incremental build state. Without this the graph has no invalidation story
-- and will confidently assert edges for symbols renamed an hour ago.
CREATE TABLE IF NOT EXISTS graph_build_state (
    project TEXT NOT NULL,
    producer TEXT NOT NULL,
    -- git sha, event sequence, or file mtime watermark, per producer.
    watermark TEXT NOT NULL,
    last_run_at TEXT NOT NULL,
    last_error TEXT,
    PRIMARY KEY(project, producer)
);

INSERT OR IGNORE INTO schema_migrations(version, applied_at)
VALUES (5, datetime('now'));
