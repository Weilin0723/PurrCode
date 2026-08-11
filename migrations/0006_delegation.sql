-- v1.4 collaborative agent development.
--
-- The session event log stays authoritative: every row here is derivable by
-- replaying `SessionEvent::Delegation*` / `Integration*`. These tables exist so
-- the agent workspace and the integration review can answer "what is every
-- worker doing right now?" and "which worktrees must not be deleted?" without
-- replaying every session's whole log on each render.
--
-- The one genuinely load-bearing column is `worker_worktree`: a worker's
-- worktree holds an unresolved patch, and a daemon that restarted needs to know
-- which directories to reconcile rather than reap. Losing that mapping means
-- either deleting a patch nobody reviewed, or leaking worktrees forever.

CREATE TABLE IF NOT EXISTS delegations (
    delegation_id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL,
    parent_turn_id TEXT NOT NULL,
    objective TEXT NOT NULL,
    capability TEXT NOT NULL,
    expected_output TEXT NOT NULL,
    access TEXT NOT NULL CHECK(access IN ('read_only','writable')),
    -- The admitted authority, verbatim. Replay must not have to re-derive it
    -- from a policy file that may since have changed.
    effective_ceiling TEXT NOT NULL CHECK(json_valid(effective_ceiling)),
    allowed_paths TEXT NOT NULL CHECK(json_valid(allowed_paths)),
    budget TEXT NOT NULL CHECK(json_valid(budget)),
    dependencies TEXT NOT NULL DEFAULT '[]' CHECK(json_valid(dependencies)),
    depth INTEGER NOT NULL CHECK(depth >= 0 AND depth <= 1),
    -- blake3 over the delegation's authority fields. A row whose digest does
    -- not match refuses to load (see `Delegation`'s Deserialize impl).
    digest TEXT NOT NULL,
    status TEXT NOT NULL CHECK(status IN (
        'planned','ready','running','awaiting_approval','blocked',
        'completed','failed','cancelled','superseded'
    )),
    integration_state TEXT NOT NULL DEFAULT 'not_proposed' CHECK(integration_state IN (
        'not_proposed','proposed','conflicted','approved','rejected','applied'
    )),
    repair_cycles INTEGER NOT NULL DEFAULT 0 CHECK(repair_cycles >= 0 AND repair_cycles <= 2),
    blocked_by TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_delegations_session
ON delegations(session_id, created_at);

CREATE INDEX IF NOT EXISTS idx_delegations_live
ON delegations(session_id, status);

-- Which specialist ran which delegation, where, and under what profile digest.
CREATE TABLE IF NOT EXISTS delegation_workers (
    worker_id TEXT PRIMARY KEY,
    delegation_id TEXT NOT NULL REFERENCES delegations(delegation_id) ON DELETE CASCADE,
    session_id TEXT NOT NULL,
    agent_profile TEXT NOT NULL,
    profile_digest TEXT NOT NULL,
    model_role TEXT,
    parent_worktree TEXT NOT NULL,
    -- NULL for a read-only worker: it never had one, and inventing a path here
    -- would make cleanup delete something that belongs to the parent.
    worker_worktree TEXT,
    base_commit TEXT NOT NULL,
    base_snapshot_digest TEXT NOT NULL,
    assigned_at TEXT NOT NULL,
    started_at TEXT,
    finished_at TEXT,
    paused_reason TEXT
);

CREATE INDEX IF NOT EXISTS idx_delegation_workers_delegation
ON delegation_workers(delegation_id);

CREATE INDEX IF NOT EXISTS idx_delegation_workers_session
ON delegation_workers(session_id, assigned_at);

-- Worktrees that must survive a restart because their patch is unresolved.
CREATE INDEX IF NOT EXISTS idx_delegation_workers_retained
ON delegation_workers(session_id, worker_worktree)
WHERE worker_worktree IS NOT NULL AND finished_at IS NULL;

-- The structured handoff. One row per delegation, enforced by the primary key:
-- recording a second result for the same delegation is the duplicate the
-- reducer already refuses, and the schema refuses it too.
CREATE TABLE IF NOT EXISTS delegation_results (
    delegation_id TEXT PRIMARY KEY REFERENCES delegations(delegation_id) ON DELETE CASCADE,
    worker_id TEXT NOT NULL,
    session_id TEXT NOT NULL,
    status TEXT NOT NULL,
    summary TEXT NOT NULL,
    changed_paths TEXT NOT NULL DEFAULT '[]' CHECK(json_valid(changed_paths)),
    patch_digest TEXT,
    findings TEXT NOT NULL DEFAULT '[]' CHECK(json_valid(findings)),
    validations TEXT NOT NULL DEFAULT '[]' CHECK(json_valid(validations)),
    unresolved TEXT NOT NULL DEFAULT '[]' CHECK(json_valid(unresolved)),
    evidence_ids TEXT NOT NULL DEFAULT '[]' CHECK(json_valid(evidence_ids)),
    usage TEXT NOT NULL CHECK(json_valid(usage)),
    completed_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_delegation_results_session
ON delegation_results(session_id, completed_at);

-- Integration proposals and their outcomes.
CREATE TABLE IF NOT EXISTS integration_proposals (
    delegation_id TEXT PRIMARY KEY REFERENCES delegations(delegation_id) ON DELETE CASCADE,
    session_id TEXT NOT NULL,
    worker_id TEXT NOT NULL,
    patch_digest TEXT NOT NULL,
    -- Set when the user accepted a subset of hunks. The applied bytes are then
    -- these bytes, and `IntegrationApplied` must carry this digest, not the
    -- worker's.
    amended_patch_digest TEXT,
    base_snapshot_digest TEXT NOT NULL,
    changed_paths TEXT NOT NULL DEFAULT '[]' CHECK(json_valid(changed_paths)),
    conflicts TEXT NOT NULL DEFAULT '[]' CHECK(json_valid(conflicts)),
    validation_summary TEXT NOT NULL CHECK(json_valid(validation_summary)),
    evidence_ids TEXT NOT NULL DEFAULT '[]' CHECK(json_valid(evidence_ids)),
    state TEXT NOT NULL CHECK(state IN ('proposed','conflicted','approved','rejected','applied')),
    decided_by TEXT,
    decided_at TEXT,
    proposed_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_integration_proposals_session
ON integration_proposals(session_id, proposed_at);

-- Why a capability resolved to the specialist it did. `capability_resolutions`
-- (0004) records the same question for tool-level resolution; this one is
-- keyed by delegation so the agent workspace can show it next to the worker.
CREATE TABLE IF NOT EXISTS delegation_routing (
    delegation_id TEXT PRIMARY KEY REFERENCES delegations(delegation_id) ON DELETE CASCADE,
    session_id TEXT NOT NULL,
    capability TEXT NOT NULL,
    chosen_profile TEXT NOT NULL,
    profile_digest TEXT NOT NULL,
    model_role TEXT,
    alternatives TEXT NOT NULL DEFAULT '[]' CHECK(json_valid(alternatives)),
    reason TEXT NOT NULL,
    resolved_at TEXT NOT NULL
);

INSERT OR IGNORE INTO schema_migrations(version, applied_at)
VALUES (6, datetime('now'));
