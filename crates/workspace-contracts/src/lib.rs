//! Workspace-level domain contracts for PurrCode v0.8.
//!
//! A [`Workspace`] is the durable unit of remote and local work introduced by
//! the Studio and Forever Runtime product surfaces (PRD §11). It owns a
//! repository reference, an isolated worktree path, an environment profile, an
//! optional active run, and an optional authority grant. Workspaces are plain
//! serializable data — they carry no I/O and no tokio dependency — so the same
//! contract is used by the local daemon, the remote VM control plane, and the
//! headless scheduler.
//!
//! This crate also owns the shared newtype identifiers
//! ([`WorkspaceId`], [`RunId`], [`EnvironmentProfileId`], [`GrantId`]) that the
//! terminal-runtime and authority-contracts crates reference. Keeping them here
//! avoids a circular dependency: workspace → authority → terminal all reference
//! the same `WorkspaceId`/`GrantId` rather than redefining them.

use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use thiserror::Error;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Shared newtype identifiers (re-exported by sibling contract crates).
// ---------------------------------------------------------------------------

/// Stable identifier for a workspace. A workspace survives UI closure, browser
/// refresh, client disconnection, and (on persistent storage) VM reboot, so the
/// id is the durable handle clients reconnect with.
#[derive(
    Clone, Copy, Debug, Eq, Hash, JsonSchema, Ord, PartialEq, PartialOrd, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct WorkspaceId(pub Uuid);

impl WorkspaceId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for WorkspaceId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for WorkspaceId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Stable identifier for a run. A run is a single goal-to-completion attempt
/// inside a workspace; the HTTP API returns it immediately from `POST /v1/runs`
/// and the run continues independently of the connection (PRD §2.3, §18.3).
#[derive(
    Clone, Copy, Debug, Eq, Hash, JsonSchema, Ord, PartialEq, PartialOrd, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct RunId(pub Uuid);

impl RunId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for RunId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for RunId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Stable identifier for a recorded environment profile. A profile is the set
/// of toolchain versions and provisioned tools associated with a workspace so
/// that reconnecting (or restarting) can reproduce the same build environment.
#[derive(
    Clone, Copy, Debug, Eq, Hash, JsonSchema, Ord, PartialEq, PartialOrd, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct EnvironmentProfileId(pub Uuid);

impl EnvironmentProfileId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for EnvironmentProfileId {
    fn default() -> Self {
        Self::new()
    }
}

/// Stable identifier for a PurrCode-managed terminal session. Terminal ids are
/// introduced by the terminal runtime (PRD §12) and referenced by the REST
/// terminal endpoints.
#[derive(
    Clone, Copy, Debug, Eq, Hash, JsonSchema, Ord, PartialEq, PartialOrd, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct TerminalId(pub Uuid);

impl TerminalId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for TerminalId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for TerminalId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Stable identifier for an authority grant (PRD §15.2). A grant is persisted
/// once and reused so the user is not repeatedly asked to approve identical
/// operations within the chosen scope.
#[derive(
    Clone, Copy, Debug, Eq, Hash, JsonSchema, Ord, PartialEq, PartialOrd, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct GrantId(pub Uuid);

impl GrantId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for GrantId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for GrantId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

// ---------------------------------------------------------------------------
// Repository reference.
// ---------------------------------------------------------------------------

/// How a workspace resolves its source repository.
///
/// `Local` points at an existing path on the hosting machine (the local
/// Workbench). `Remote` names a Git URL and branch that the runtime clones into
/// an isolated worktree (the remote VM Workbench, PRD §11).
#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "data", rename_all = "snake_case")]
pub enum RepositoryReference {
    Local { path: PathBuf },
    Remote { url: String, branch: String },
}

impl RepositoryReference {
    /// Branch name used when the reference does not carry one (local repos read
    /// the current branch from the working copy).
    pub const DEFAULT_BRANCH: &'static str = "main";

    pub fn is_local(&self) -> bool {
        matches!(self, RepositoryReference::Local { .. })
    }

    pub fn is_remote(&self) -> bool {
        matches!(self, RepositoryReference::Remote { .. })
    }
}

// ---------------------------------------------------------------------------
// Workspace.
// ---------------------------------------------------------------------------

/// Lifecycle of a workspace.
///
/// Mirrors the run lifecycle but is coarser: a workspace may be created and
/// idle, actively executing a run, paused for human input, or in a terminal
/// state (`Completed`/`Failed`/`Discarded`). `Disconnected` is a *runtime*
/// observation that the owning client dropped the connection without ending
/// the run — it does not stop work (PRD §11.3).
#[derive(
    Clone, Copy, Debug, Eq, JsonSchema, Ord, PartialEq, PartialOrd, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceStatus {
    /// Created but no run submitted yet.
    Created,
    /// A run is active on the workspace.
    Running,
    /// Paused awaiting human input (approval or takeover).
    Paused,
    /// Reconciling durable state after a daemon or VM restart.
    Reconciling,
    /// The owning client disconnected while work continues.
    Disconnected,
    /// The active run completed with verified evidence.
    Completed,
    /// The active run reached a terminal failure.
    Failed,
    /// The workspace and its worktree were discarded by the user.
    Discarded,
}

/// The complete durable workspace record (PRD §11.2).
///
/// This is intentionally a value type: the daemon/scheduler persists it, the UI
/// reads it over the authenticated API, and the remote client reconnects to it
/// by id. It does not own open file handles or live processes — those live in
/// the runtime adapters keyed by the contained ids.
#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct Workspace {
    pub workspace_id: WorkspaceId,
    pub repository: RepositoryReference,
    pub source_revision: String,
    pub worktree_path: PathBuf,
    pub environment_profile: EnvironmentProfileId,
    pub active_run: Option<RunId>,
    pub authority_grant: Option<GrantId>,
    pub status: WorkspaceStatus,
    /// Record of the last client connection so reconnect can replay a bounded
    /// transcript (PRD §11.3). Wall-clock observed by the hosting process; not a
    /// monotonic guarantee.
    #[serde(default)]
    pub last_seen_at: Option<DateTime<Utc>>,
}

impl Workspace {
    /// Minimal construction for callers that only need the id-bearing fields.
    pub fn new(
        workspace_id: WorkspaceId,
        repository: RepositoryReference,
        worktree_path: PathBuf,
        environment_profile: EnvironmentProfileId,
    ) -> Self {
        Self {
            workspace_id,
            repository,
            source_revision: String::new(),
            worktree_path,
            environment_profile,
            active_run: None,
            authority_grant: None,
            status: WorkspaceStatus::Created,
            last_seen_at: None,
        }
    }

    /// True when the workspace may still produce effects (created, running,
    /// paused, reconciling, or disconnected-but-alive).
    pub fn is_live(&self) -> bool {
        matches!(
            self.status,
            WorkspaceStatus::Created
                | WorkspaceStatus::Running
                | WorkspaceStatus::Paused
                | WorkspaceStatus::Reconciling
                | WorkspaceStatus::Disconnected
        )
    }

    /// True when the workspace has reached a final state and will not resume.
    pub fn is_terminal(&self) -> bool {
        matches!(
            self.status,
            WorkspaceStatus::Completed | WorkspaceStatus::Failed | WorkspaceStatus::Discarded
        )
    }
}

// ---------------------------------------------------------------------------
// Run lifecycle (lightweight contract used by POST /v1/runs).
// ---------------------------------------------------------------------------

/// Lifecycle of a run (PRD §2.3, §18.3).
///
/// `Created`/`Queued`/`Running`/`AwaitingInput`/`Repairing`/`Paused` are live;
/// `Completed`/`Failed`/`Cancelled` are terminal. A run outlives the HTTP
/// request that created it and outlives the UI connection, so these states are
/// persisted and re-read on reconnect (PRD §21).
#[derive(
    Clone, Copy, Debug, Eq, JsonSchema, Ord, PartialEq, PartialOrd, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Created,
    Queued,
    Running,
    AwaitingInput,
    Repairing,
    Paused,
    Completed,
    Failed,
    Cancelled,
}

impl RunStatus {
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            RunStatus::Completed | RunStatus::Failed | RunStatus::Cancelled
        )
    }

    pub fn is_live(self) -> bool {
        !self.is_terminal()
    }
}

/// Enough run state to satisfy `GET /v1/runs/{run_id}` without loading the full
/// session event log. The heavy detail (actions, evidence, validation) lives in
/// the existing durable session store keyed by [`SessionId`]; this is the
/// lightweight index used by the run list and run card.
#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct RunSummary {
    pub run_id: RunId,
    pub workspace_id: WorkspaceId,
    pub objective: String,
    pub status: RunStatus,
    #[serde(default)]
    pub created_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub updated_at: Option<DateTime<Utc>>,
}

// ---------------------------------------------------------------------------
// Errors.
// ---------------------------------------------------------------------------

/// Workspace-domain errors. Kept narrow: the trust-bearing execution errors
/// still come from `purrcode-runtime-core::DomainError`; these are the
/// bookkeeping failures a control plane can return to a UI client.
#[derive(Clone, Debug, Error, JsonSchema, PartialEq, Serialize, Deserialize)]
pub enum WorkspaceError {
    #[error("workspace {id} not found")]
    NotFound { id: WorkspaceId },
    #[error("workspace {id} is already {status:?}")]
    Conflict {
        id: WorkspaceId,
        status: WorkspaceStatus,
    },
    #[error("run {id} not found")]
    RunNotFound { id: RunId },
    #[error("repository reference is invalid: {reason}")]
    InvalidRepository { reason: String },
    #[error("workspace path is not absolute: {0}")]
    RelativeWorkspacePath(String),
    #[error("workspace {id} is in terminal state {status:?}")]
    Terminal {
        id: WorkspaceId,
        status: WorkspaceStatus,
    },
}

/// Validate that a workspace's worktree path is absolute and non-empty.
///
/// Worktree paths are managed by `repository-engine` and are always absolute
/// under PurrCode-managed storage; a relative or empty path indicates a bug or a
/// tampered record and must fail closed before it reaches any adapter.
pub fn validate_worktree_path(path: &std::path::Path) -> Result<(), WorkspaceError> {
    if path.as_os_str().is_empty() {
        return Err(WorkspaceError::RelativeWorkspacePath(String::new()));
    }
    if !path.is_absolute() {
        return Err(WorkspaceError::RelativeWorkspacePath(
            path.display().to_string(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newtype_ids_serialize_transparently_as_uuid_strings() {
        let id = WorkspaceId::new();
        let json = serde_json::to_string(&id).unwrap();
        // Transparent newtype serializes as the inner uuid string.
        assert!(Uuid::parse_str(json.trim_matches('"')).is_ok());
        let back: WorkspaceId = serde_json::from_str(&json).unwrap();
        assert_eq!(id, back);
    }

    #[test]
    fn repository_reference_round_trips_tagged() {
        let local = RepositoryReference::Local {
            path: PathBuf::from("/repo"),
        };
        let j = serde_json::to_string(&local).unwrap();
        assert!(j.contains("\"kind\":\"local\""));
        let back: RepositoryReference = serde_json::from_str(&j).unwrap();
        assert_eq!(local, back);

        let remote = RepositoryReference::Remote {
            url: "https://x/y".into(),
            branch: "dev".into(),
        };
        let j = serde_json::to_string(&remote).unwrap();
        assert!(j.contains("\"kind\":\"remote\""));
        assert!(remote.is_remote());
        assert!(!local.is_remote());
    }

    #[test]
    fn workspace_new_starts_created_and_is_live() {
        let ws = Workspace::new(
            WorkspaceId::new(),
            RepositoryReference::Local {
                path: PathBuf::from("/r"),
            },
            PathBuf::from("/r/.wk"),
            EnvironmentProfileId::new(),
        );
        assert_eq!(ws.status, WorkspaceStatus::Created);
        assert!(ws.is_live());
        assert!(!ws.is_terminal());
        assert!(ws.active_run.is_none());
        assert!(ws.authority_grant.is_none());
    }

    #[test]
    fn terminal_statuses_are_terminal_and_not_live() {
        for s in [
            WorkspaceStatus::Completed,
            WorkspaceStatus::Failed,
            WorkspaceStatus::Discarded,
        ] {
            assert!(s != WorkspaceStatus::Disconnected);
            let mut ws = Workspace::new(
                WorkspaceId::new(),
                RepositoryReference::Local {
                    path: PathBuf::from("/r"),
                },
                PathBuf::from("/r/.wk"),
                EnvironmentProfileId::new(),
            );
            ws.status = s;
            assert!(ws.is_terminal());
            assert!(!ws.is_live());
        }
    }

    #[test]
    fn disconnected_is_live_so_work_continues() {
        let mut ws = Workspace::new(
            WorkspaceId::new(),
            RepositoryReference::Local {
                path: PathBuf::from("/r"),
            },
            PathBuf::from("/r/.wk"),
            EnvironmentProfileId::new(),
        );
        ws.status = WorkspaceStatus::Disconnected;
        // PRD §11.3: disconnecting the user must not terminate builds/tests.
        assert!(ws.is_live());
        assert!(!ws.is_terminal());
    }

    #[test]
    fn run_status_terminal_predicate() {
        assert!(RunStatus::Completed.is_terminal());
        assert!(RunStatus::Cancelled.is_terminal());
        assert!(!RunStatus::Running.is_terminal());
        assert!(RunStatus::Running.is_live());
    }

    #[test]
    fn validate_worktree_path_rejects_relative_and_empty() {
        assert!(validate_worktree_path(std::path::Path::new("rel/x")).is_err());
        assert!(validate_worktree_path(std::path::Path::new("")).is_err());
        assert!(validate_worktree_path(&std::env::temp_dir().join("purrcode-absolute")).is_ok());
    }
}
