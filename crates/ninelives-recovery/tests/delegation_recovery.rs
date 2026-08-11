//! Durable multi-agent recovery (v1.4 §PR10, §9 "Parent Restart").
//!
//! These tests kill and reopen a real on-disk store, because that is the only
//! way to test the claim that matters: after a restart, PurrCode knows which
//! workers finished, does not run them again, keeps pending approvals pending,
//! and still knows which worktrees hold an unresolved patch.

use purrcode_ninelives::SessionStore;
use purrcode_runtime_core::delegation::{
    AuthorityInputs, Delegation, DelegationBudget, DelegationRequest, DelegationStatus,
    ExpectedOutput, IntegrationProposal, IntegrationState, PathPattern, UsageSummary,
    ValidationEvidence, WorkerAssignment, WorkerId, WorkerResult, WorkerResultStatus,
    WorkerWorkspaceRecord, WorkspaceAccess,
};
use purrcode_runtime_core::{
    ApprovalAuthority, ApprovalPolicy, AuthorityMode, CapabilityId, FilesystemScope, NetworkScope,
    SessionEvent, SessionId, SideEffectClass, ToolCeiling, TurnId, ValidationStatus,
};
use std::collections::BTreeSet;
use std::path::PathBuf;

fn permissive() -> ToolCeiling {
    ToolCeiling {
        maximum_side_effect: SideEffectClass::Destructive,
        maximum_network: NetworkScope::Any,
        maximum_filesystem: FilesystemScope::maximum(),
        minimum_approval: ApprovalPolicy::ByClass,
        denied_tool_ids: BTreeSet::new(),
    }
}

fn delegation(session: SessionId, paths: &[&str]) -> Delegation {
    let ceiling = permissive();
    let remaining = DelegationBudget::modest();
    DelegationRequest {
        parent_session_id: session,
        parent_turn_id: TurnId::new(),
        objective: "implement oauth token exchange".into(),
        capability: CapabilityId::parse("implement_backend").unwrap(),
        acceptance_criteria: Vec::new(),
        context_refs: Vec::new(),
        allowed_paths: paths
            .iter()
            .map(|p| PathPattern::parse(p).unwrap())
            .collect(),
        expected_output: ExpectedOutput::Patch,
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

fn assignment(delegation: &Delegation, worker_id: WorkerId, worktree: &str) -> WorkerAssignment {
    WorkerAssignment {
        worker_id,
        delegation_id: delegation.id(),
        agent_profile: "backend-specialist".into(),
        profile_digest: "profile-digest".into(),
        model_role: None,
        workspace: WorkerWorkspaceRecord {
            parent_worktree: PathBuf::from("/repo/.purrcode/worktrees/parent"),
            worker_worktree: Some(PathBuf::from(worktree)),
            base_commit: "abc123".into(),
            base_snapshot_digest: "snapshot-digest".into(),
            access: WorkspaceAccess::Writable,
        },
        assigned_at: chrono::Utc::now(),
    }
}

fn result(delegation: &Delegation, worker_id: WorkerId, paths: &[&str]) -> WorkerResult {
    WorkerResult {
        delegation_id: delegation.id(),
        worker_id,
        status: WorkerResultStatus::Completed,
        summary: "implemented the token exchange".into(),
        changed_paths: paths.iter().map(PathBuf::from).collect(),
        patch_digest: Some("patch-digest".into()),
        findings: Vec::new(),
        validations: vec![ValidationEvidence {
            name: "cargo test".into(),
            status: ValidationStatus::Passed,
            detail: "ok".into(),
            evidence_id: None,
        }],
        unresolved: Vec::new(),
        evidence_ids: Vec::new(),
        usage: UsageSummary {
            input_tokens: 2_000,
            output_tokens: 800,
            tool_calls: 6,
            model_calls: 3,
            duration_seconds: 45,
            changed_files: paths.len(),
        },
        completed_at: chrono::Utc::now(),
    }
}

fn start_session(store: &mut SessionStore, session: SessionId) {
    store
        .append(
            session,
            &SessionEvent::SessionCreated {
                objective: "add oauth".into(),
                repository: PathBuf::from("/repo"),
                authority_mode: AuthorityMode::Governed,
            },
        )
        .unwrap();
}

#[test]
fn a_completed_worker_survives_restart_and_is_never_rerun() {
    let directory = tempfile::tempdir().unwrap();
    let database = directory.path().join("sessions.db");
    let session = SessionId::new();
    let delegation = {
        let mut store = SessionStore::open(&database).unwrap();
        start_session(&mut store, session);
        let delegation = delegation(session, &["src/auth/**"]);
        let worker_id = WorkerId::new();
        store
            .append(
                session,
                &SessionEvent::DelegationCreated {
                    delegation: Box::new(delegation.clone()),
                },
            )
            .unwrap();
        store
            .append(
                session,
                &SessionEvent::DelegationReady {
                    delegation_id: delegation.id(),
                },
            )
            .unwrap();
        store
            .append(
                session,
                &SessionEvent::DelegationWorkerAssigned {
                    assignment: Box::new(assignment(
                        &delegation,
                        worker_id,
                        "/repo/.purrcode/worktrees/worker-a",
                    )),
                },
            )
            .unwrap();
        store
            .append(
                session,
                &SessionEvent::DelegationWorkerStarted {
                    delegation_id: delegation.id(),
                    worker_id,
                },
            )
            .unwrap();
        store
            .append(
                session,
                &SessionEvent::DelegationResultRecorded {
                    result: Box::new(result(&delegation, worker_id, &["src/auth/token.rs"])),
                },
            )
            .unwrap();
        store
            .append(
                session,
                &SessionEvent::DelegationWorkerCompleted {
                    delegation_id: delegation.id(),
                    worker_id,
                },
            )
            .unwrap();
        delegation
    };

    // ── daemon restart ────────────────────────────────────────────────
    let mut store = SessionStore::open(&database).unwrap();
    let state = store.load(session).unwrap();
    let record = &state.delegations[&delegation.id()];
    assert_eq!(record.status(), DelegationStatus::Completed);
    assert!(record.is_finished());
    assert_eq!(state.delegation_ledger.workers_completed, 1);
    // Usage survived: the parent's accounting still includes the child's spend.
    assert_eq!(
        state.delegation_ledger.total_worker_usage.input_tokens,
        2_000
    );

    // Restarting the worker is refused by the durable log itself.
    let rerun = store.append(
        session,
        &SessionEvent::DelegationWorkerStarted {
            delegation_id: delegation.id(),
            worker_id: record.assignment.as_ref().unwrap().worker_id,
        },
    );
    assert!(rerun.is_err(), "a completed worker must never be rerun");

    // Recording its result a second time is refused too, so a replayed intent
    // cannot double-count usage.
    let duplicate = store.append(
        session,
        &SessionEvent::DelegationResultRecorded {
            result: Box::new(result(
                &delegation,
                record.assignment.as_ref().unwrap().worker_id,
                &["src/auth/token.rs"],
            )),
        },
    );
    assert!(duplicate.is_err());
}

#[test]
fn a_pending_approval_and_its_worktree_survive_restart() {
    let directory = tempfile::tempdir().unwrap();
    let database = directory.path().join("sessions.db");
    let session = SessionId::new();
    let worker_id = WorkerId::new();
    let delegation = {
        let mut store = SessionStore::open(&database).unwrap();
        start_session(&mut store, session);
        let delegation = delegation(session, &["src/auth/**"]);
        for event in [
            SessionEvent::DelegationCreated {
                delegation: Box::new(delegation.clone()),
            },
            SessionEvent::DelegationReady {
                delegation_id: delegation.id(),
            },
            SessionEvent::DelegationWorkerAssigned {
                assignment: Box::new(assignment(
                    &delegation,
                    worker_id,
                    "/repo/.purrcode/worktrees/worker-b",
                )),
            },
            SessionEvent::DelegationWorkerStarted {
                delegation_id: delegation.id(),
                worker_id,
            },
            SessionEvent::DelegationResultRecorded {
                result: Box::new(result(&delegation, worker_id, &["src/auth/token.rs"])),
            },
            SessionEvent::IntegrationProposed {
                proposal: Box::new(IntegrationProposal {
                    delegation_id: delegation.id(),
                    worker_id,
                    patch_digest: "patch-digest".into(),
                    changed_paths: vec![PathBuf::from("src/auth/token.rs")],
                    base_snapshot_digest: "snapshot-digest".into(),
                    evidence_ids: Vec::new(),
                    validation_summary: Default::default(),
                    conflicts: Vec::new(),
                    amended_patch_digest: None,
                }),
            },
        ] {
            store.append(session, &event).unwrap();
        }
        delegation
    };

    // ── daemon restart ────────────────────────────────────────────────
    let mut store = SessionStore::open(&database).unwrap();
    let state = store.load(session).unwrap();
    let record = &state.delegations[&delegation.id()];
    assert_eq!(record.integration, IntegrationState::Proposed);
    assert!(record.awaits_decision(), "a pending approval stays pending");
    // The worktree is still marked as holding an unresolved patch.
    let retained = store.retained_worker_worktrees().unwrap();
    assert!(
        retained
            .iter()
            .any(|(_, path)| path == &PathBuf::from("/repo/.purrcode/worktrees/worker-b")),
        "an unresolved worker patch must keep its worktree, got {retained:?}"
    );

    // Applying without an approval is refused even after the restart.
    assert!(
        store
            .append(
                session,
                &SessionEvent::IntegrationApplied {
                    delegation_id: delegation.id(),
                    patch_digest: "patch-digest".into(),
                    changed_paths: vec![PathBuf::from("src/auth/token.rs")],
                },
            )
            .is_err()
    );

    // Approve, apply, and the worktree stops being retained.
    store
        .append(
            session,
            &SessionEvent::IntegrationApproved {
                delegation_id: delegation.id(),
                patch_digest: "patch-digest".into(),
                authority: ApprovalAuthority::Human,
            },
        )
        .unwrap();
    store
        .append(
            session,
            &SessionEvent::IntegrationApplied {
                delegation_id: delegation.id(),
                patch_digest: "patch-digest".into(),
                changed_paths: vec![PathBuf::from("src/auth/token.rs")],
            },
        )
        .unwrap();
    let retained = store.retained_worker_worktrees().unwrap();
    assert!(
        !retained
            .iter()
            .any(|(_, path)| path == &PathBuf::from("/repo/.purrcode/worktrees/worker-b")),
        "an applied patch releases its worktree"
    );
}

#[test]
fn a_worker_running_at_crash_time_is_reconciled_not_restarted() {
    let directory = tempfile::tempdir().unwrap();
    let database = directory.path().join("sessions.db");
    let session = SessionId::new();
    let worker_id = WorkerId::new();
    let delegation = {
        let mut store = SessionStore::open(&database).unwrap();
        start_session(&mut store, session);
        let delegation = delegation(session, &["src/auth/**"]);
        for event in [
            SessionEvent::DelegationCreated {
                delegation: Box::new(delegation.clone()),
            },
            SessionEvent::DelegationReady {
                delegation_id: delegation.id(),
            },
            SessionEvent::DelegationWorkerAssigned {
                assignment: Box::new(assignment(
                    &delegation,
                    worker_id,
                    "/repo/.purrcode/worktrees/worker-c",
                )),
            },
            SessionEvent::DelegationWorkerStarted {
                delegation_id: delegation.id(),
                worker_id,
            },
        ] {
            store.append(session, &event).unwrap();
        }
        delegation
    };

    // ── the daemon dies here, mid-worker ──────────────────────────────
    let mut store = SessionStore::open(&database).unwrap();
    let state = store.load(session).unwrap();
    let record = &state.delegations[&delegation.id()];
    assert_eq!(record.status(), DelegationStatus::Running);
    assert_eq!(state.running_worker_count(), 1);
    assert!(record.retained_worktree().is_some());

    // Reconciliation records the pause; the delegation stays `Running` so
    // nothing treats it as fresh work to schedule.
    store
        .append(
            session,
            &SessionEvent::DelegationWorkerPaused {
                delegation_id: delegation.id(),
                worker_id,
                reason: "daemon restarted while the worker was executing".into(),
            },
        )
        .unwrap();
    let state = store.load(session).unwrap();
    let record = &state.delegations[&delegation.id()];
    assert_eq!(record.status(), DelegationStatus::Running);
    assert_eq!(
        record.paused_reason.as_deref(),
        Some("daemon restarted while the worker was executing")
    );
}

#[test]
fn the_projection_matches_the_replayed_log() {
    // The tables in migration 0006 are derivable from the log. If they can
    // disagree, one of them is lying to the UI.
    let directory = tempfile::tempdir().unwrap();
    let database = directory.path().join("sessions.db");
    let session = SessionId::new();
    let mut store = SessionStore::open(&database).unwrap();
    start_session(&mut store, session);

    let first = delegation(session, &["src/auth/**"]);
    let second = delegation(session, &["migrations/**"]);
    let first_worker = WorkerId::new();
    for event in [
        SessionEvent::DelegationCreated {
            delegation: Box::new(first.clone()),
        },
        SessionEvent::DelegationCreated {
            delegation: Box::new(second.clone()),
        },
        SessionEvent::DelegationReady {
            delegation_id: first.id(),
        },
        SessionEvent::DelegationWorkerAssigned {
            assignment: Box::new(assignment(
                &first,
                first_worker,
                "/repo/.purrcode/worktrees/worker-d",
            )),
        },
        SessionEvent::DelegationWorkerStarted {
            delegation_id: first.id(),
            worker_id: first_worker,
        },
        SessionEvent::DelegationResultRecorded {
            result: Box::new(result(&first, first_worker, &["src/auth/token.rs"])),
        },
        SessionEvent::DelegationWorkerCompleted {
            delegation_id: first.id(),
            worker_id: first_worker,
        },
    ] {
        store.append(session, &event).unwrap();
    }

    let state = store.load(session).unwrap();
    let rows = store.delegation_summaries(session).unwrap();
    assert_eq!(rows.len(), state.delegations.len());
    for row in &rows {
        let id: purrcode_runtime_core::delegation::DelegationId =
            row.delegation_id.parse().unwrap();
        let record = &state.delegations[&id];
        let replayed_status = serde_json::to_value(record.status())
            .unwrap()
            .as_str()
            .unwrap()
            .to_owned();
        assert_eq!(
            row.status, replayed_status,
            "projection and replay disagree about {id}"
        );
    }
    let completed = rows
        .iter()
        .find(|row| row.delegation_id == first.id().to_string())
        .unwrap();
    assert_eq!(completed.status, "completed");
    assert_eq!(
        completed.agent_profile.as_deref(),
        Some("backend-specialist")
    );
    assert!(completed.result_summary.is_some());

    let planned = rows
        .iter()
        .find(|row| row.delegation_id == second.id().to_string())
        .unwrap();
    assert_eq!(planned.status, "planned");
    assert!(planned.agent_profile.is_none());
}
