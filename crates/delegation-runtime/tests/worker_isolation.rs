//! End-to-end isolation and integration tests against real git worktrees
//! (v1.4 §PR3, §PR7, §9).
//!
//! The unit tests prove the decisions; these prove the mechanism. Every one of
//! them creates a real repository, provisions real worker worktrees and lets git
//! be the judge of what actually happened on disk.

use purrcode_delegation_runtime::DelegationRuntimeError;
use purrcode_delegation_runtime::integrate::{
    PendingPatch, apply_to_parent, conflicts_among, select_hunks,
};
use purrcode_delegation_runtime::workspace::{WorkerWorkspaceManager, current_snapshot_digest};
use purrcode_repository_engine::{RepositoryEngine, SessionWorktree};
use purrcode_runtime_core::delegation::integration::{
    IntegrationConflictKind, IntegrationDecision, evaluate_proposal, propose_integration,
};
use purrcode_runtime_core::delegation::{
    AuthorityInputs, Delegation, DelegationBudget, DelegationRequest, ExpectedOutput, PathPattern,
    UsageSummary, WorkerId, WorkerResult, WorkerResultStatus, WorkspaceAccess,
};
use purrcode_runtime_core::{
    ApprovalPolicy, CapabilityId, FilesystemScope, NetworkScope, SessionId, SideEffectClass,
    ToolCeiling, TurnId, ValidationStatus,
};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

fn git(repository: &Path, arguments: &[&str]) {
    let status = std::process::Command::new("git")
        .args(arguments)
        .current_dir(repository)
        .status()
        .expect("git is available");
    assert!(status.success(), "git {arguments:?} failed");
}

/// A repository with two source files, so two workers can edit different ones.
fn repository() -> tempfile::TempDir {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    git(root, &["init", "--initial-branch=main"]);
    git(root, &["config", "user.email", "fixture@example.test"]);
    git(root, &["config", "user.name", "Fixture"]);
    git(root, &["config", "commit.gpgsign", "false"]);
    std::fs::create_dir_all(root.join("src/auth")).unwrap();
    std::fs::create_dir_all(root.join("migrations")).unwrap();
    std::fs::write(
        root.join("src/auth/token.rs"),
        "pub fn exchange() {\n    todo!()\n}\n",
    )
    .unwrap();
    std::fs::write(root.join("migrations/0001.sql"), "-- base\n").unwrap();
    std::fs::write(root.join("README.md"), "base\n").unwrap();
    git(root, &["add", "--all"]);
    git(root, &["commit", "-m", "base"]);
    directory
}

fn permissive() -> ToolCeiling {
    ToolCeiling {
        maximum_side_effect: SideEffectClass::Destructive,
        maximum_network: NetworkScope::Any,
        maximum_filesystem: FilesystemScope::maximum(),
        minimum_approval: ApprovalPolicy::ByClass,
        denied_tool_ids: BTreeSet::new(),
    }
}

fn delegation(paths: &[&str], expected: ExpectedOutput) -> Delegation {
    let ceiling = permissive();
    let remaining = DelegationBudget::modest();
    DelegationRequest {
        parent_session_id: SessionId::new(),
        parent_turn_id: TurnId::new(),
        objective: "delegated unit".into(),
        capability: CapabilityId::parse("implement_backend").unwrap(),
        acceptance_criteria: Vec::new(),
        context_refs: Vec::new(),
        allowed_paths: paths
            .iter()
            .map(|p| PathPattern::parse(p).unwrap())
            .collect(),
        expected_output: expected,
        dependencies: Vec::new(),
        budget: DelegationBudget::modest(),
    }
    .admit(AuthorityInputs {
        workspace: &ceiling,
        parent: &ceiling,
        profile: &ceiling,
        parent_remaining_budget: &remaining,
        depth: 1,
    })
    .unwrap()
}

fn result_for(
    delegation: &Delegation,
    worker_id: WorkerId,
    changed: &[PathBuf],
    digest: &str,
) -> WorkerResult {
    WorkerResult {
        delegation_id: delegation.id(),
        worker_id,
        status: WorkerResultStatus::Completed,
        summary: "done".into(),
        changed_paths: changed.to_vec(),
        patch_digest: Some(digest.to_owned()),
        findings: Vec::new(),
        validations: vec![purrcode_runtime_core::delegation::ValidationEvidence {
            name: "cargo check".into(),
            status: ValidationStatus::Passed,
            detail: "ok".into(),
            evidence_id: None,
        }],
        unresolved: Vec::new(),
        evidence_ids: Vec::new(),
        usage: UsageSummary::default(),
        completed_at: chrono::Utc::now(),
    }
}

async fn parent_worktree(root: &Path) -> SessionWorktree {
    RepositoryEngine::create_worktree(root, SessionId::new())
        .await
        .unwrap()
}

#[tokio::test]
async fn two_writer_workers_edit_independently_in_their_own_worktrees() {
    // §PR3 acceptance: two parallel writer workers, no filesystem race, and the
    // parent worktree untouched until integration.
    let repository = repository();
    let parent = parent_worktree(repository.path()).await;

    let backend = delegation(&["src/auth/**"], ExpectedOutput::Patch);
    let migration = delegation(&["migrations/**"], ExpectedOutput::Patch);
    let backend_worker = WorkerId::new();
    let migration_worker = WorkerId::new();

    let (backend_workspace, migration_workspace) = tokio::join!(
        WorkerWorkspaceManager::provision(&parent, &backend, backend_worker),
        WorkerWorkspaceManager::provision(&parent, &migration, migration_worker),
    );
    let backend_workspace = backend_workspace.unwrap();
    let migration_workspace = migration_workspace.unwrap();

    // Distinct directories, neither of which is the parent's.
    let backend_path = backend_workspace.record.worker_worktree.clone().unwrap();
    let migration_path = migration_workspace.record.worker_worktree.clone().unwrap();
    assert_ne!(backend_path, migration_path);
    assert_ne!(backend_path, parent.path);
    assert_ne!(migration_path, parent.path);
    assert!(backend_workspace.record.is_coherent());

    // Both workers write at the same time.
    tokio::join!(
        tokio::fs::write(
            backend_path.join("src/auth/token.rs"),
            "pub fn exchange() {\n    Ok(())\n}\n",
        ),
        tokio::fs::write(migration_path.join("migrations/0002.sql"), "-- oauth\n"),
    )
    .0
    .unwrap();

    let backend_patch =
        WorkerWorkspaceManager::collect_patch(backend_workspace.worktree.as_ref().unwrap())
            .await
            .unwrap();
    let migration_patch =
        WorkerWorkspaceManager::collect_patch(migration_workspace.worktree.as_ref().unwrap())
            .await
            .unwrap();

    // Each worker's patch contains only its own work.
    assert_eq!(
        backend_patch.changed_paths,
        [PathBuf::from("src/auth/token.rs")]
    );
    assert_eq!(
        migration_patch.changed_paths,
        [PathBuf::from("migrations/0002.sql")]
    );
    assert_ne!(backend_patch.patch_digest, migration_patch.patch_digest);

    // The parent worktree saw nothing: a worker's completion is a proposal.
    let parent_effects = RepositoryEngine::effects(&parent).await.unwrap();
    assert!(
        parent_effects.changed_files.is_empty(),
        "the parent must be untouched, found {:?}",
        parent_effects.changed_files
    );
}

#[tokio::test]
async fn a_worker_starts_from_the_parent_snapshot_not_the_repository_head() {
    // A worker seeded from the repository HEAD would re-propose the parent's own
    // uncommitted work as if it were the worker's contribution.
    let repository = repository();
    let parent = parent_worktree(repository.path()).await;
    // The parent has uncommitted work of its own.
    tokio::fs::write(parent.path.join("README.md"), "base\nparent edit\n")
        .await
        .unwrap();

    let backend = delegation(&["src/auth/**"], ExpectedOutput::Patch);
    let workspace = WorkerWorkspaceManager::provision(&parent, &backend, WorkerId::new())
        .await
        .unwrap();
    let worker_path = workspace.record.worker_worktree.clone().unwrap();

    // The worker can see the parent's edit…
    let seen = tokio::fs::read_to_string(worker_path.join("README.md"))
        .await
        .unwrap();
    assert!(seen.contains("parent edit"));

    // …but it is committed as the worker's base, so the worker's own patch is
    // empty until it does something.
    let before = WorkerWorkspaceManager::collect_patch(workspace.worktree.as_ref().unwrap())
        .await
        .unwrap();
    assert!(
        before.is_empty(),
        "a freshly seeded worker proposes nothing, found {:?}",
        before.changed_paths
    );

    tokio::fs::write(
        worker_path.join("src/auth/token.rs"),
        "pub fn exchange() {\n    Ok(())\n}\n",
    )
    .await
    .unwrap();
    let after = WorkerWorkspaceManager::collect_patch(workspace.worktree.as_ref().unwrap())
        .await
        .unwrap();
    assert_eq!(
        after.changed_paths,
        [PathBuf::from("src/auth/token.rs")],
        "the worker's patch is its own delta, not the parent's work as well"
    );
}

#[tokio::test]
async fn a_read_only_worker_gets_no_worktree_of_its_own() {
    let repository = repository();
    let parent = parent_worktree(repository.path()).await;
    let review = delegation(&["src/**"], ExpectedOutput::Review);
    let workspace = WorkerWorkspaceManager::provision(&parent, &review, WorkerId::new())
        .await
        .unwrap();
    assert_eq!(workspace.record.access, WorkspaceAccess::ReadOnly);
    assert!(workspace.record.worker_worktree.is_none());
    assert!(workspace.worktree.is_none());
    assert_eq!(workspace.working_directory(), parent.path);
    assert!(workspace.record.is_coherent());
}

#[tokio::test]
async fn independent_patches_integrate_and_conflicting_ones_do_not() {
    // §PR7: separate files integrate after validation; the same hunk never does.
    let repository = repository();
    let parent = parent_worktree(repository.path()).await;
    let base_digest = current_snapshot_digest(&parent).await.unwrap();

    let backend = delegation(&["src/auth/**"], ExpectedOutput::Patch);
    let migration = delegation(&["migrations/**"], ExpectedOutput::Patch);
    let backend_worker = WorkerId::new();
    let migration_worker = WorkerId::new();
    let backend_workspace = WorkerWorkspaceManager::provision(&parent, &backend, backend_worker)
        .await
        .unwrap();
    let migration_workspace =
        WorkerWorkspaceManager::provision(&parent, &migration, migration_worker)
            .await
            .unwrap();

    tokio::fs::write(
        backend_workspace
            .record
            .worker_worktree
            .as_ref()
            .unwrap()
            .join("src/auth/token.rs"),
        "pub fn exchange() {\n    Ok(())\n}\n",
    )
    .await
    .unwrap();
    tokio::fs::write(
        migration_workspace
            .record
            .worker_worktree
            .as_ref()
            .unwrap()
            .join("migrations/0002.sql"),
        "-- oauth\n",
    )
    .await
    .unwrap();

    let backend_patch =
        WorkerWorkspaceManager::collect_patch(backend_workspace.worktree.as_ref().unwrap())
            .await
            .unwrap();
    let migration_patch =
        WorkerWorkspaceManager::collect_patch(migration_workspace.worktree.as_ref().unwrap())
            .await
            .unwrap();

    // Disjoint files: no conflict.
    let pending = vec![
        PendingPatch::from_worker_patch(backend.id(), &backend_patch),
        PendingPatch::from_worker_patch(migration.id(), &migration_patch),
    ];
    assert!(conflicts_among(&pending).is_empty());

    // Both proposals evaluate clean and apply to the parent.
    for (delegation, worker, patch) in [
        (&backend, backend_worker, &backend_patch),
        (&migration, migration_worker, &migration_patch),
    ] {
        let result = result_for(
            delegation,
            worker,
            &patch.changed_paths,
            &patch.patch_digest,
        );
        let proposal = propose_integration(
            delegation,
            &result,
            &backend_workspace.record.base_snapshot_digest,
            &base_digest,
            purrcode_delegation_runtime::integrate::hunks_of(&patch.patch),
        )
        .unwrap();
        assert!(
            evaluate_proposal(&proposal, &[], true).is_ready(),
            "proposal should be ready: {proposal:?}"
        );
        apply_to_parent(&parent, &patch.patch, &patch.patch_digest)
            .await
            .unwrap();
    }

    let parent_effects = RepositoryEngine::effects(&parent).await.unwrap();
    let changed: BTreeSet<PathBuf> = parent_effects.changed_files.into_iter().collect();
    assert!(changed.contains(&PathBuf::from("src/auth/token.rs")));
    assert!(changed.contains(&PathBuf::from("migrations/0002.sql")));
}

#[tokio::test]
async fn two_workers_editing_the_same_hunk_produce_an_explicit_conflict() {
    // §9 "Conflict": no last-write-win, ever.
    let repository = repository();
    let parent = parent_worktree(repository.path()).await;

    let first = delegation(&["src/auth/**"], ExpectedOutput::Patch);
    let second = delegation(&["src/auth/**"], ExpectedOutput::Patch);
    let first_workspace = WorkerWorkspaceManager::provision(&parent, &first, WorkerId::new())
        .await
        .unwrap();
    let second_workspace = WorkerWorkspaceManager::provision(&parent, &second, WorkerId::new())
        .await
        .unwrap();

    // Both rewrite the same function body.
    tokio::fs::write(
        first_workspace
            .record
            .worker_worktree
            .as_ref()
            .unwrap()
            .join("src/auth/token.rs"),
        "pub fn exchange() {\n    Ok(\"first\")\n}\n",
    )
    .await
    .unwrap();
    tokio::fs::write(
        second_workspace
            .record
            .worker_worktree
            .as_ref()
            .unwrap()
            .join("src/auth/token.rs"),
        "pub fn exchange() {\n    Ok(\"second\")\n}\n",
    )
    .await
    .unwrap();

    let first_patch =
        WorkerWorkspaceManager::collect_patch(first_workspace.worktree.as_ref().unwrap())
            .await
            .unwrap();
    let second_patch =
        WorkerWorkspaceManager::collect_patch(second_workspace.worktree.as_ref().unwrap())
            .await
            .unwrap();

    let conflicts = conflicts_among(&[
        PendingPatch::from_worker_patch(first.id(), &first_patch),
        PendingPatch::from_worker_patch(second.id(), &second_patch),
    ]);
    assert_eq!(conflicts.len(), 1, "expected one conflict: {conflicts:?}");
    assert_eq!(conflicts[0].kind, IntegrationConflictKind::SameHunk);
    assert!(!conflicts[0].kind.is_auto_mergeable());

    // …and the coordinator refuses to call either proposal ready.
    let result = result_for(
        &first,
        WorkerId::new(),
        &first_patch.changed_paths,
        &first_patch.patch_digest,
    );
    let proposal = propose_integration(
        &first,
        &result,
        &first_workspace.record.base_snapshot_digest,
        &first_workspace.record.base_snapshot_digest,
        purrcode_delegation_runtime::integrate::hunks_of(&first_patch.patch),
    )
    .unwrap();
    match evaluate_proposal(&proposal, &conflicts, true) {
        IntegrationDecision::Conflict { conflicts } => {
            assert_eq!(conflicts[0].kind, IntegrationConflictKind::SameHunk);
        }
        other => panic!("expected a conflict, got {other:?}"),
    }
}

#[tokio::test]
async fn a_worker_writing_outside_its_scope_is_refused_before_integration() {
    // §9 "Scope Escape": delegated `src/auth/**`, wrote `migrations/`.
    let repository = repository();
    let parent = parent_worktree(repository.path()).await;
    let scoped = delegation(&["src/auth/**"], ExpectedOutput::Patch);
    let workspace = WorkerWorkspaceManager::provision(&parent, &scoped, WorkerId::new())
        .await
        .unwrap();
    let worker_path = workspace.record.worker_worktree.clone().unwrap();

    // Nothing stops a worker from writing to its own worktree; what stops the
    // change is that it can never become an integration.
    tokio::fs::write(worker_path.join("migrations/0002.sql"), "-- sneaky\n")
        .await
        .unwrap();
    let patch = WorkerWorkspaceManager::collect_patch(workspace.worktree.as_ref().unwrap())
        .await
        .unwrap();
    let result = result_for(
        &scoped,
        WorkerId::new(),
        &patch.changed_paths,
        &patch.patch_digest,
    );
    let error = result
        .validate_against(&scoped)
        .expect_err("writing outside the delegated scope must be refused");
    assert!(error.to_string().contains("outside its delegated scope"));

    // And the parent never saw it.
    let parent_effects = RepositoryEngine::effects(&parent).await.unwrap();
    assert!(parent_effects.changed_files.is_empty());
}

#[tokio::test]
async fn an_approval_cannot_be_spent_on_a_different_patch() {
    let repository = repository();
    let parent = parent_worktree(repository.path()).await;
    let scoped = delegation(&["src/auth/**"], ExpectedOutput::Patch);
    let workspace = WorkerWorkspaceManager::provision(&parent, &scoped, WorkerId::new())
        .await
        .unwrap();
    let worker_path = workspace.record.worker_worktree.clone().unwrap();
    tokio::fs::write(
        worker_path.join("src/auth/token.rs"),
        "pub fn exchange() {\n    Ok(())\n}\n",
    )
    .await
    .unwrap();
    let approved = WorkerWorkspaceManager::collect_patch(workspace.worktree.as_ref().unwrap())
        .await
        .unwrap();

    // The worker keeps working after the approval was granted.
    tokio::fs::write(
        worker_path.join("src/auth/token.rs"),
        "pub fn exchange() {\n    Ok(())\n}\n// and something else\n",
    )
    .await
    .unwrap();
    let drifted = WorkerWorkspaceManager::collect_patch(workspace.worktree.as_ref().unwrap())
        .await
        .unwrap();
    assert_ne!(approved.patch_digest, drifted.patch_digest);

    let error = apply_to_parent(&parent, &drifted.patch, &approved.patch_digest)
        .await
        .expect_err("the approved digest is not the drifted one");
    assert!(matches!(
        error,
        DelegationRuntimeError::PatchDigestMismatch { .. }
    ));
    let parent_effects = RepositoryEngine::effects(&parent).await.unwrap();
    assert!(
        parent_effects.changed_files.is_empty(),
        "nothing may be applied when the digest does not match"
    );
}

#[tokio::test]
async fn selected_hunks_apply_as_a_new_patch_with_a_new_digest() {
    // §PR12: accepting a subset must produce a different digest and apply only
    // the selected part.
    let repository = repository();
    let parent = parent_worktree(repository.path()).await;
    let scoped = delegation(&["src/**", "migrations/**"], ExpectedOutput::Patch);
    let workspace = WorkerWorkspaceManager::provision(&parent, &scoped, WorkerId::new())
        .await
        .unwrap();
    let worker_path = workspace.record.worker_worktree.clone().unwrap();
    tokio::fs::write(
        worker_path.join("src/auth/token.rs"),
        "pub fn exchange() {\n    Ok(())\n}\n",
    )
    .await
    .unwrap();
    tokio::fs::write(
        worker_path.join("migrations/0001.sql"),
        "-- base\n-- more\n",
    )
    .await
    .unwrap();
    let full = WorkerWorkspaceManager::collect_patch(workspace.worktree.as_ref().unwrap())
        .await
        .unwrap();
    assert_eq!(full.changed_paths.len(), 2);

    let subset = select_hunks(&full.patch, &[0]).unwrap();
    let subset_digest = blake3::hash(&subset).to_hex().to_string();
    assert_ne!(subset_digest, full.patch_digest);

    apply_to_parent(&parent, &subset, &subset_digest)
        .await
        .unwrap();
    let parent_effects = RepositoryEngine::effects(&parent).await.unwrap();
    assert_eq!(
        parent_effects.changed_files.len(),
        1,
        "only the selected hunk's file should have landed, got {:?}",
        parent_effects.changed_files
    );
}

#[tokio::test]
async fn an_unresolved_worker_worktree_is_retained_and_reopenable() {
    // §PR3: never silently delete an unresolved worker patch, and survive a
    // restart by reopening what is on disk.
    let repository = repository();
    let parent = parent_worktree(repository.path()).await;
    let scoped = delegation(&["src/auth/**"], ExpectedOutput::Patch);
    let worker_id = WorkerId::new();
    let workspace = WorkerWorkspaceManager::provision(&parent, &scoped, worker_id)
        .await
        .unwrap();
    let worker_path = workspace.record.worker_worktree.clone().unwrap();
    tokio::fs::write(
        worker_path.join("src/auth/token.rs"),
        "pub fn exchange() {\n    Ok(())\n}\n",
    )
    .await
    .unwrap();

    // Pending integration: retained.
    let removed = WorkerWorkspaceManager::release(workspace.worktree.as_ref().unwrap(), true)
        .await
        .unwrap();
    assert!(!removed);
    assert!(worker_path.exists());

    // A "restart" reopens the same worktree, and the patch is still there.
    let reopened =
        WorkerWorkspaceManager::reopen(&parent.source_repository, &workspace.record, worker_id)
            .expect("the worktree is still on disk");
    let patch = WorkerWorkspaceManager::collect_patch(&reopened)
        .await
        .unwrap();
    assert_eq!(patch.changed_paths, [PathBuf::from("src/auth/token.rs")]);

    // Resolved: released.
    let removed = WorkerWorkspaceManager::release(&reopened, false)
        .await
        .unwrap();
    assert!(removed);
    assert!(!worker_path.exists());
    // …and a reopen after removal reports the honest answer rather than
    // silently creating a fresh one.
    assert!(
        WorkerWorkspaceManager::reopen(&parent.source_repository, &workspace.record, worker_id)
            .is_none()
    );
}

#[tokio::test]
async fn base_drift_is_detected_against_the_parent_snapshot() {
    let repository = repository();
    let parent = parent_worktree(repository.path()).await;
    let scoped = delegation(&["src/auth/**"], ExpectedOutput::Patch);
    let workspace = WorkerWorkspaceManager::provision(&parent, &scoped, WorkerId::new())
        .await
        .unwrap();
    let worker_path = workspace.record.worker_worktree.clone().unwrap();
    tokio::fs::write(
        worker_path.join("src/auth/token.rs"),
        "pub fn exchange() {\n    Ok(())\n}\n",
    )
    .await
    .unwrap();
    let patch = WorkerWorkspaceManager::collect_patch(workspace.worktree.as_ref().unwrap())
        .await
        .unwrap();

    // The parent moves on while the worker was busy.
    tokio::fs::write(parent.path.join("README.md"), "base\nparent moved on\n")
        .await
        .unwrap();
    let now = current_snapshot_digest(&parent).await.unwrap();
    assert_ne!(now, workspace.record.base_snapshot_digest);

    let result = result_for(
        &scoped,
        WorkerId::new(),
        &patch.changed_paths,
        &patch.patch_digest,
    );
    let proposal = propose_integration(
        &scoped,
        &result,
        &workspace.record.base_snapshot_digest,
        &now,
        purrcode_delegation_runtime::integrate::hunks_of(&patch.patch),
    )
    .unwrap();
    match evaluate_proposal(&proposal, &[], true) {
        IntegrationDecision::Conflict { conflicts } => {
            assert_eq!(conflicts[0].kind, IntegrationConflictKind::BaseDrift);
        }
        other => panic!("expected base drift, got {other:?}"),
    }
}
