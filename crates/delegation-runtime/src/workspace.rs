//! Isolated worker workspaces (v1.4 §PR3).
//!
//! Two modifying workers must never operate on the same writable tree. This
//! module is the only thing in v1.4 that creates one, and it creates exactly
//! two shapes:
//!
//! * a **writer** gets its own git worktree, checked out at the parent's base
//!   commit and then seeded-and-committed with the parent's uncommitted work,
//!   so its `HEAD` *is* the parent snapshot and its eventual diff is its own
//!   contribution and nothing else;
//! * a **reader** (reviewer, investigator, validator) gets a read-only view of
//!   the parent worktree and no worktree of its own. Creating one anyway because
//!   it is convenient would hand a reviewer write authority nobody delegated.
//!
//! ## Why worker worktrees are siblings, not children
//!
//! The PRD draws worker worktrees nested under the parent's. Physically nesting
//! them would put worker files *inside* the parent's working tree, where git
//! would report them as untracked changes belonging to the parent — the parent's
//! own diff would silently include every worker's work. They are therefore
//! created as siblings under `<repo>/.purrcode/worktrees/<worker-uuid>`, which
//! is the same layout session worktrees already use, and the parent/child
//! relationship is carried in [`WorkerWorkspaceRecord`] instead of in the path.

use crate::DelegationRuntimeError;
use purrcode_repository_engine::{RepositoryEngine, SessionWorktree};
use purrcode_runtime_core::SessionId;
use purrcode_runtime_core::delegation::{
    Delegation, WorkerId, WorkerWorkspaceRecord, WorkspaceAccess,
};
use std::path::{Path, PathBuf};

/// A provisioned workspace: the durable record plus, for a writer, the live
/// worktree handle the runtime needs in order to read its effects later.
#[derive(Clone, Debug)]
pub struct ProvisionedWorkspace {
    pub record: WorkerWorkspaceRecord,
    /// `None` for a read-only worker — it has no worktree of its own.
    pub worktree: Option<SessionWorktree>,
}

impl ProvisionedWorkspace {
    pub fn working_directory(&self) -> &Path {
        self.record.working_directory()
    }
}

/// One worker's proposed change set, read out of its worktree.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct WorkerPatch {
    pub changed_paths: Vec<PathBuf>,
    pub patch: Vec<u8>,
    pub patch_digest: String,
}

impl WorkerPatch {
    pub fn is_empty(&self) -> bool {
        self.patch.is_empty() && self.changed_paths.is_empty()
    }
}

pub struct WorkerWorkspaceManager;

impl WorkerWorkspaceManager {
    /// Provision the workspace one delegation will execute in.
    ///
    /// The access mode is taken from the *delegation*, never from a caller
    /// argument: [`Delegation`] already resolved it through the authority
    /// intersection, and letting a caller pass its own would be a second,
    /// unchecked path to write authority.
    pub async fn provision(
        parent: &SessionWorktree,
        delegation: &Delegation,
        worker_id: WorkerId,
    ) -> Result<ProvisionedWorkspace, DelegationRuntimeError> {
        let parent_effects = RepositoryEngine::effects(parent).await?;
        // The snapshot digest binds both the commit the parent sits on and the
        // uncommitted work on top of it. Either changing means a worker's patch
        // was computed against lines the parent no longer has.
        let base_snapshot_digest = snapshot_digest(&parent.base_head, &parent_effects.binary_patch);

        match delegation.access() {
            WorkspaceAccess::ReadOnly => Ok(ProvisionedWorkspace {
                record: WorkerWorkspaceRecord {
                    parent_worktree: parent.path.clone(),
                    worker_worktree: None,
                    base_commit: parent.base_head.clone(),
                    base_snapshot_digest,
                    access: WorkspaceAccess::ReadOnly,
                },
                worktree: None,
            }),
            WorkspaceAccess::Writable => {
                // A worker worktree is keyed by the worker's own id, so two
                // writers can never resolve to the same directory even if they
                // are delegated identical objectives.
                let worktree = RepositoryEngine::create_worktree_at(
                    &parent.source_repository,
                    SessionId(worker_id.0),
                    Some(&parent.base_head),
                )
                .await?;
                let base_commit = RepositoryEngine::seed_worker_worktree(
                    &worktree,
                    &parent_effects.binary_patch,
                    &worker_id.short(),
                )
                .await?;
                let record = WorkerWorkspaceRecord {
                    parent_worktree: parent.path.clone(),
                    worker_worktree: Some(worktree.path.clone()),
                    base_commit,
                    base_snapshot_digest,
                    access: WorkspaceAccess::Writable,
                };
                debug_assert!(record.is_coherent());
                Ok(ProvisionedWorkspace {
                    record,
                    worktree: Some(worktree),
                })
            }
        }
    }

    /// Read a writer's contribution out of its worktree.
    ///
    /// Because the seed was committed, `effects` here is the worker's delta
    /// against the parent snapshot — not the parent's work plus the worker's.
    pub async fn collect_patch(
        worktree: &SessionWorktree,
    ) -> Result<WorkerPatch, DelegationRuntimeError> {
        let effects = RepositoryEngine::effects(worktree).await?;
        Ok(WorkerPatch {
            patch_digest: blake3::hash(&effects.binary_patch).to_hex().to_string(),
            changed_paths: effects.changed_files,
            patch: effects.binary_patch,
        })
    }

    /// Re-open a persisted worker worktree after a daemon restart.
    ///
    /// Returns `None` when the directory is gone, which is the honest answer:
    /// the caller must then mark the worker unrecoverable rather than silently
    /// creating a fresh worktree and rerunning it.
    pub fn reopen(
        source_repository: &Path,
        record: &WorkerWorkspaceRecord,
        worker_id: WorkerId,
    ) -> Option<SessionWorktree> {
        let path = record.worker_worktree.as_ref()?;
        if !path.exists() {
            return None;
        }
        Some(SessionWorktree {
            session_id: SessionId(worker_id.0),
            source_repository: source_repository.to_path_buf(),
            path: path.clone(),
            base_head: record.base_commit.clone(),
            source_was_dirty: false,
            initialized_submodules: Vec::new(),
            unavailable_submodules: Vec::new(),
        })
    }

    /// Release a worker's worktree once its result is resolved.
    ///
    /// `retain` comes from [`purrcode_runtime_core::delegation::DelegationRecord::retained_worktree`]:
    /// a worker whose patch is still pending integration keeps its worktree,
    /// because deleting it would destroy an unresolved proposal. Removal is
    /// therefore always an explicit decision by the caller.
    pub async fn release(
        worktree: &SessionWorktree,
        retain: bool,
    ) -> Result<bool, DelegationRuntimeError> {
        if retain {
            return Ok(false);
        }
        RepositoryEngine::remove_worktree(worktree).await?;
        Ok(true)
    }
}

/// Digest binding a base commit and the uncommitted work on top of it.
pub fn snapshot_digest(base_head: &str, patch: &[u8]) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(base_head.as_bytes());
    hasher.update(b"\0");
    hasher.update(patch);
    hasher.finalize().to_hex().to_string()
}

/// The parent's snapshot digest right now. Integration compares this against
/// the digest a worker branched from to detect drift.
pub async fn current_snapshot_digest(
    parent: &SessionWorktree,
) -> Result<String, DelegationRuntimeError> {
    let effects = RepositoryEngine::effects(parent).await?;
    Ok(snapshot_digest(&parent.base_head, &effects.binary_patch))
}
