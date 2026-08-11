-- v1.3 extension platform: descriptor pinning, hook governance, and the
-- capability resolution audit trail. The session event log remains the
-- authoritative record; these tables are queryable projections plus one
-- genuinely durable trust decision (tool_descriptor_pins).

-- Trust-on-first-use pinning for tool descriptors.
--
-- A descriptor discovered from a remote MCP server is authored by that server.
-- Without a pin, a server that reports side_effect_class='read' on Monday and
-- 'destructive' on Tuesday silently changes what PawGate will auto-allow. The
-- pin records the digest the user (or a signed pack) accepted; a descriptor
-- whose digest differs from an approved pin is admitted as Forbidden until
-- re-approved.
CREATE TABLE IF NOT EXISTS tool_descriptor_pins (
    project TEXT NOT NULL,
    tool_id TEXT NOT NULL,
    descriptor_digest TEXT NOT NULL,
    provider TEXT NOT NULL,
    origin TEXT NOT NULL,
    side_effect_class TEXT NOT NULL,
    network_scope TEXT NOT NULL CHECK(json_valid(network_scope)),
    filesystem_scope TEXT NOT NULL CHECK(json_valid(filesystem_scope)),
    approval_policy TEXT NOT NULL,
    first_seen_at TEXT NOT NULL,
    approved_at TEXT,
    approved_by TEXT CHECK(approved_by IS NULL OR json_valid(approved_by)),
    revoked_at TEXT,
    PRIMARY KEY(project, tool_id)
);

CREATE INDEX IF NOT EXISTS idx_tool_descriptor_pins_digest
ON tool_descriptor_pins(project, descriptor_digest);

-- Hook governance. Every hook firing is recorded BEFORE the action is proposed,
-- so a hook that was triggered but denied is distinguishable from one that
-- never fired. action_id links into authorizations(action_id) when the hook
-- reached PawGate.
CREATE TABLE IF NOT EXISTS hook_runs (
    id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL,
    hook_id TEXT NOT NULL,
    hook_digest TEXT NOT NULL,
    trigger TEXT NOT NULL,
    layer TEXT NOT NULL,
    action_id TEXT,
    status TEXT NOT NULL CHECK(status IN (
        'triggered','denied','awaiting_approval','executing','succeeded','failed','timed_out','skipped'
    )),
    detail TEXT,
    started_at TEXT NOT NULL,
    finished_at TEXT
);

CREATE INDEX IF NOT EXISTS idx_hook_runs_session
ON hook_runs(session_id, started_at);

CREATE INDEX IF NOT EXISTS idx_hook_runs_action
ON hook_runs(action_id);

-- Which provider satisfied which capability, and why that one. Makes
-- "who can satisfy this capability?" answerable after the fact, which is
-- acceptance-test step 15 for the resolution step specifically.
CREATE TABLE IF NOT EXISTS capability_resolutions (
    id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL,
    turn_id TEXT,
    capability TEXT NOT NULL,
    chosen_provider TEXT NOT NULL CHECK(json_valid(chosen_provider)),
    considered TEXT NOT NULL CHECK(json_valid(considered)),
    resolved_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_capability_resolutions_session
ON capability_resolutions(session_id, resolved_at);

-- Structured tool evidence, projected from SessionEvent::ToolEvidenceRecorded
-- so the IDE can list "every external tool this session touched" without
-- replaying the whole log.
CREATE TABLE IF NOT EXISTS tool_evidence (
    action_id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL,
    turn_id TEXT,
    tool_id TEXT NOT NULL,
    provider TEXT NOT NULL,
    descriptor_digest TEXT NOT NULL,
    initiator TEXT NOT NULL CHECK(json_valid(initiator)),
    approved_by TEXT NOT NULL CHECK(json_valid(approved_by)),
    outcome TEXT NOT NULL CHECK(json_valid(outcome)),
    redaction_class TEXT NOT NULL,
    started_at TEXT NOT NULL,
    finished_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_tool_evidence_session
ON tool_evidence(session_id, started_at);

CREATE INDEX IF NOT EXISTS idx_tool_evidence_tool
ON tool_evidence(tool_id);

INSERT OR IGNORE INTO schema_migrations(version, applied_at)
VALUES (4, datetime('now'));
