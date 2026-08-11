//! Shared fixtures for this crate's tests.

use chrono::Utc;
use purrcode_runtime_core::delegation::{
    AuthorityInputs, Delegation, DelegationBudget, DelegationId, DelegationRecord,
    DelegationRequest, DelegationStatus, ExpectedOutput, PathPattern, UsageSummary, WorkerId,
    WorkerResult, WorkerResultStatus,
};
use purrcode_runtime_core::{
    ApprovalPolicy, CapabilityId, FilesystemScope, NetworkScope, SessionId, SideEffectClass,
    ToolCeiling, TurnId, ValidationStatus,
};
use std::collections::BTreeSet;
use std::path::PathBuf;

pub fn permissive_ceiling() -> ToolCeiling {
    ToolCeiling {
        maximum_side_effect: SideEffectClass::Destructive,
        maximum_network: NetworkScope::Any,
        maximum_filesystem: FilesystemScope::maximum(),
        minimum_approval: ApprovalPolicy::ByClass,
        denied_tool_ids: BTreeSet::new(),
    }
}

/// An admitted delegation with the given scope, output shape and dependencies.
pub fn admitted_delegation(
    paths: &[&str],
    expected: ExpectedOutput,
    dependencies: &[DelegationId],
) -> Delegation {
    let ceiling = permissive_ceiling();
    let remaining = DelegationBudget::modest();
    DelegationRequest {
        parent_session_id: SessionId::new(),
        parent_turn_id: TurnId::new(),
        objective: "implement the delegated unit".into(),
        capability: CapabilityId::parse("implement_backend").unwrap(),
        acceptance_criteria: Vec::new(),
        context_refs: Vec::new(),
        allowed_paths: paths
            .iter()
            .map(|p| PathPattern::parse(p).unwrap())
            .collect(),
        expected_output: expected,
        dependencies: dependencies.to_vec(),
        budget: DelegationBudget::modest(),
    }
    .admit(AuthorityInputs {
        workspace: &ceiling,
        parent: &ceiling,
        profile: &ceiling,
        parent_remaining_budget: &remaining,
        depth: 1,
    })
    .expect("fixture delegation is admissible")
}

pub fn worker_result(delegation: &Delegation, worker_id: WorkerId, paths: &[&str]) -> WorkerResult {
    WorkerResult {
        delegation_id: delegation.id(),
        worker_id,
        status: WorkerResultStatus::Completed,
        summary: "completed the delegated unit".into(),
        changed_paths: paths.iter().map(PathBuf::from).collect(),
        patch_digest: (!paths.is_empty()).then(|| "patch-digest".to_owned()),
        findings: Vec::new(),
        validations: vec![purrcode_runtime_core::delegation::ValidationEvidence {
            name: "cargo test".into(),
            status: ValidationStatus::Passed,
            detail: "ok".into(),
            evidence_id: None,
        }],
        unresolved: Vec::new(),
        evidence_ids: Vec::new(),
        usage: UsageSummary {
            input_tokens: 1_000,
            output_tokens: 400,
            tool_calls: 3,
            model_calls: 2,
            duration_seconds: 20,
            changed_files: paths.len(),
        },
        completed_at: Utc::now(),
    }
}

/// A record driven all the way to `Completed` with a recorded result.
pub fn record_with_result(delegation: Delegation, paths: &[&str]) -> DelegationRecord {
    let worker_id = WorkerId::new();
    let result = worker_result(&delegation, worker_id, paths);
    let mut record = DelegationRecord::new(delegation);
    record
        .delegation
        .transition_to(DelegationStatus::Ready)
        .unwrap();
    record
        .delegation
        .transition_to(DelegationStatus::Running)
        .unwrap();
    record
        .delegation
        .transition_to(DelegationStatus::Completed)
        .unwrap();
    record.result = Some(result);
    record
}
