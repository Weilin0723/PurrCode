//! Delegation orchestration in the daemon (v1.4 §PR4, §PR7, §PR11, §PR12).
//!
//! The domain decisions live in `purrcode-runtime-core` and the git mechanics
//! in `purrcode-delegation-runtime`. What lives here is the part that only the
//! daemon can do: own the durable store, run the provider, and turn a scheduler
//! step into events, worktrees and model calls.
//!
//! Three properties this module is responsible for keeping true:
//!
//! * **Every UI action is a daemon command.** The views below are projections of
//!   durable state, and accept/reject/stop append events. Nothing in the client
//!   may hold delegation state of its own (§PR11).
//! * **A worker's authority is enforced while it runs**, not only checked
//!   afterwards. Every action a worker proposes goes through
//!   `Policy::evaluate_delegated`, so a scope escape is denied at proposal time.
//! * **Integration is a separate trust boundary.** Applying a patch is a
//!   distinct, human-gated step; nothing here applies one without an
//!   `IntegrationApproved` event that names the same digest.

use crate::ApiError;
use chrono::Utc;
use purrcode_claw::ToolRuntime;
use purrcode_delegation_runtime::context::{ContextInputs, DependencySummary, WorkerContext};
use purrcode_delegation_runtime::integrate::{
    PendingPatch, apply_to_parent, conflicts_among, hunks_of, select_hunks,
};
use purrcode_delegation_runtime::review::{
    RepairPlan, RepairTrigger, attribute_failure, plan_repair,
};
use purrcode_delegation_runtime::routing::RoutingPolicy;
use purrcode_delegation_runtime::workspace::{
    ProvisionedWorkspace, WorkerWorkspaceManager, current_snapshot_digest,
};
use purrcode_delegation_runtime::{PlannedDelegation, PlanningContext, admit_plan};
use purrcode_ninelives::SessionStore;
use purrcode_pawgate::Policy;
use purrcode_repository_engine::SessionWorktree;
use purrcode_runtime_core::delegation::decision::classify;
use purrcode_runtime_core::delegation::integration::{evaluate_proposal, propose_integration};
use purrcode_runtime_core::delegation::{
    Delegation, DelegationBudget, DelegationGovernance, DelegationId, DelegationPlan,
    DelegationRecord, DelegationSignals, DelegationStatus, ExpectedOutput, IntegrationDecision,
    IntegrationState, PathPattern, PlannedUnit, UsageSummary, WorkerAssignment, WorkerId,
    WorkerResult, WorkerResultStatus, WorkspaceAccess,
};
use purrcode_runtime_core::{
    ActionId, ApprovalAuthority, Authorization, CapabilityId, CapabilityRegistry, JudgmentDecision,
    ProposedAction, SessionEvent, SessionId, SessionState, ToolCeiling, TurnId,
};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

// ---------------------------------------------------------------------------
// Request / response contracts.
// ---------------------------------------------------------------------------

/// A unit the caller proposes. The daemon still decides whether to delegate at
/// all — submitting units is a request, not an instruction.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct DelegationUnitRequest {
    pub key: String,
    pub objective: String,
    pub capability: String,
    #[serde(default)]
    pub allowed_paths: Vec<String>,
    #[serde(default)]
    pub expected_output: Option<String>,
    #[serde(default)]
    pub depends_on: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct DelegationPlanRequest {
    /// Deterministic signals for the classifier. Absent fields default to the
    /// conservative value, which biases toward a single agent.
    #[serde(default)]
    pub signals: DelegationSignals,
    #[serde(default)]
    pub units: Vec<DelegationUnitRequest>,
}

/// What the caller gets back — including "we did not delegate, and here is
/// why", which is a first-class answer rather than an error.
#[derive(Clone, Debug, Serialize)]
pub struct DelegationPlanView {
    pub classification: String,
    pub reason: String,
    pub expected_benefit: i32,
    pub coordination_cost: i32,
    pub delegations: Vec<DelegationView>,
    /// Units that could not be admitted, with the reason.
    pub refused: Vec<RefusedUnit>,
}

#[derive(Clone, Debug, Serialize)]
pub struct RefusedUnit {
    pub key: String,
    pub reason: String,
}

/// One row of the agent workspace (§PR11).
#[derive(Clone, Debug, Serialize)]
pub struct DelegationView {
    pub delegation_id: String,
    pub objective: String,
    pub capability: String,
    pub status: String,
    pub integration_state: String,
    pub access: String,
    pub agent_profile: Option<String>,
    pub model_role: Option<String>,
    pub routing_reason: Option<String>,
    pub allowed_paths: Vec<String>,
    pub dependencies: Vec<String>,
    pub blocked_by: Option<String>,
    pub repair_cycles: u8,
    pub budget: DelegationBudget,
    pub usage: UsageSummary,
    pub changed_paths: Vec<String>,
    pub tool_calls: u32,
    pub validations: Vec<ValidationView>,
    pub findings: Vec<FindingView>,
    pub conflicts: Vec<ConflictView>,
    pub evidence_ids: Vec<String>,
    pub summary: Option<String>,
    pub worker_worktree: Option<String>,
    pub paused_reason: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ValidationView {
    pub name: String,
    pub status: String,
    pub detail: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct FindingView {
    pub id: String,
    pub title: String,
    pub detail: String,
    pub severity: String,
    pub path: Option<String>,
    pub line: Option<u32>,
    pub evidence_ids: Vec<String>,
    pub recommended_action: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ConflictView {
    pub path: String,
    pub kind: String,
    pub detail: String,
    pub auto_mergeable: bool,
}

/// The agent workspace: the parent plus every delegation under it (§PR11).
#[derive(Clone, Debug, Serialize)]
pub struct AgentWorkspaceView {
    pub session_id: String,
    pub objective: Option<String>,
    pub classification: Option<String>,
    pub plan_reason: Option<String>,
    pub running: u32,
    pub awaiting_decision: u32,
    pub workers_started: u32,
    pub workers_completed: u32,
    pub total_worker_usage: UsageSummary,
    pub governance: DelegationGovernance,
    pub delegations: Vec<DelegationView>,
}

/// The integration review for one worker's proposal (§PR12).
#[derive(Clone, Debug, Serialize)]
pub struct IntegrationReviewView {
    pub delegation: DelegationView,
    pub patch_digest: String,
    pub effective_patch_digest: String,
    pub base_snapshot_digest: String,
    pub base_drifted: bool,
    /// The unified diff the worker proposes, as text where it is text.
    pub patch: Option<String>,
    pub hunks: Vec<HunkView>,
    pub decision: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct HunkView {
    pub index: usize,
    pub path: String,
    pub old_start: u32,
    pub old_lines: u32,
    pub new_start: u32,
    pub new_lines: u32,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct AcceptIntegrationRequest {
    /// Hunk indices to accept. Empty (or absent) accepts the whole patch.
    #[serde(default)]
    pub hunks: Vec<usize>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct RejectIntegrationRequest {
    #[serde(default = "default_rejection")]
    pub reason: String,
}

fn default_rejection() -> String {
    "rejected by the user".to_owned()
}

// ---------------------------------------------------------------------------
// Views.
// ---------------------------------------------------------------------------

pub fn workspace_view(
    state: &SessionState,
    governance: DelegationGovernance,
) -> AgentWorkspaceView {
    AgentWorkspaceView {
        session_id: state.id.0.to_string(),
        objective: state.objective.clone(),
        classification: state
            .delegation_plan
            .as_ref()
            .map(|plan| plan.classification.label().to_owned()),
        plan_reason: state
            .delegation_plan
            .as_ref()
            .map(|plan| plan.reason.clone()),
        running: state.running_worker_count(),
        awaiting_decision: state.delegations_awaiting_decision().count() as u32,
        workers_started: state.delegation_ledger.workers_started,
        workers_completed: state.delegation_ledger.workers_completed,
        total_worker_usage: state.delegation_ledger.total_worker_usage,
        governance,
        delegations: state.delegations.values().map(delegation_view).collect(),
    }
}

pub fn delegation_view(record: &DelegationRecord) -> DelegationView {
    let delegation = &record.delegation;
    let result = record.result.as_ref();
    DelegationView {
        delegation_id: delegation.id().to_string(),
        objective: delegation.objective().to_owned(),
        capability: delegation.capability().to_string(),
        status: status_label(delegation.status()),
        integration_state: integration_label(record.integration),
        access: match delegation.access() {
            WorkspaceAccess::ReadOnly => "read_only".to_owned(),
            WorkspaceAccess::Writable => "writable".to_owned(),
        },
        agent_profile: record
            .assignment
            .as_ref()
            .map(|assignment| assignment.agent_profile.clone()),
        model_role: record
            .assignment
            .as_ref()
            .and_then(|assignment| assignment.model_role.as_ref())
            .map(ToString::to_string),
        routing_reason: record
            .routing
            .as_ref()
            .map(|routing| routing.reason.clone()),
        allowed_paths: delegation
            .allowed_paths()
            .iter()
            .map(|pattern| pattern.as_str().to_owned())
            .collect(),
        dependencies: delegation
            .dependencies()
            .iter()
            .map(ToString::to_string)
            .collect(),
        blocked_by: record.blocked_by.map(|id| id.to_string()),
        repair_cycles: record.repair_cycles,
        budget: *delegation.budget(),
        usage: result.map(|result| result.usage).unwrap_or_default(),
        changed_paths: result
            .map(|result| {
                result
                    .changed_paths
                    .iter()
                    .map(|path| path.display().to_string())
                    .collect()
            })
            .unwrap_or_default(),
        tool_calls: result.map(|result| result.usage.tool_calls).unwrap_or(0),
        validations: result
            .map(|result| {
                result
                    .validations
                    .iter()
                    .map(|validation| ValidationView {
                        name: validation.name.clone(),
                        status: format!("{:?}", validation.status).to_lowercase(),
                        detail: validation.detail.clone(),
                    })
                    .collect()
            })
            .unwrap_or_default(),
        findings: result
            .map(|result| {
                result
                    .findings
                    .iter()
                    .map(|finding| FindingView {
                        id: finding.id.clone(),
                        title: finding.title.clone(),
                        detail: finding.detail.clone(),
                        severity: format!("{:?}", finding.severity).to_lowercase(),
                        path: finding.path.as_ref().map(|p| p.display().to_string()),
                        line: finding.line,
                        evidence_ids: finding
                            .evidence_ids
                            .iter()
                            .map(|id| id.0.to_string())
                            .collect(),
                        recommended_action: finding.recommended_action.clone(),
                    })
                    .collect()
            })
            .unwrap_or_default(),
        conflicts: record
            .conflicts
            .iter()
            .map(|conflict| ConflictView {
                path: conflict.path.display().to_string(),
                kind: format!("{:?}", conflict.kind),
                detail: conflict.detail.clone(),
                auto_mergeable: conflict.kind.is_auto_mergeable(),
            })
            .collect(),
        evidence_ids: result
            .map(|result| {
                result
                    .evidence_ids
                    .iter()
                    .map(|id| id.0.to_string())
                    .collect()
            })
            .unwrap_or_default(),
        summary: result.map(|result| result.summary.clone()),
        worker_worktree: record
            .assignment
            .as_ref()
            .and_then(|assignment| assignment.workspace.worker_worktree.as_ref())
            .map(|path| path.display().to_string()),
        paused_reason: record.paused_reason.clone(),
    }
}

pub fn status_label(status: DelegationStatus) -> String {
    serde_json::to_value(status)
        .ok()
        .and_then(|value| value.as_str().map(ToOwned::to_owned))
        .unwrap_or_else(|| "unknown".into())
}

pub fn integration_label(state: IntegrationState) -> String {
    serde_json::to_value(state)
        .ok()
        .and_then(|value| value.as_str().map(ToOwned::to_owned))
        .unwrap_or_else(|| "unknown".into())
}

// ---------------------------------------------------------------------------
// Planning.
// ---------------------------------------------------------------------------

/// Translate a request into a classifier plan.
///
/// The units the caller proposed are only *used* when the classifier decides to
/// delegate. A request carrying four units for a one-line fix still comes back
/// as `Single`, which is the whole point of §PR2.
pub fn build_plan(request: &DelegationPlanRequest) -> Result<DelegationPlan, ApiError> {
    let mut signals = request.signals.clone();
    // The number of units the caller proposed is itself a signal, and a more
    // reliable one than a self-reported count.
    if signals.independent_components == 0 {
        signals.independent_components = request.units.len().max(1) as u32;
    }
    let mut plan = classify(&signals);
    if !plan.classification.delegates() {
        return Ok(plan);
    }
    plan.units = request
        .units
        .iter()
        .map(unit_from_request)
        .collect::<Result<Vec<_>, _>>()?;
    plan.validate()
        .map_err(|error| ApiError::BadRequest(error.to_string()))?;
    Ok(plan)
}

fn unit_from_request(request: &DelegationUnitRequest) -> Result<PlannedUnit, ApiError> {
    let capability = CapabilityId::parse(&request.capability)
        .map_err(|error| ApiError::BadRequest(error.to_string()))?;
    let expected_output = match request.expected_output.as_deref() {
        None | Some("patch") => ExpectedOutput::Patch,
        Some("review") => ExpectedOutput::Review,
        Some("investigation") => ExpectedOutput::Investigation,
        Some("validation") => ExpectedOutput::Validation,
        Some(other) => {
            return Err(ApiError::BadRequest(format!(
                "unknown expected_output `{other}`"
            )));
        }
    };
    let allowed_paths = request
        .allowed_paths
        .iter()
        .map(|raw| PathPattern::parse(raw))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| ApiError::BadRequest(error.to_string()))?;
    let budget = match expected_output {
        ExpectedOutput::Patch => DelegationBudget::modest(),
        _ => DelegationBudget::review(),
    };
    Ok(PlannedUnit {
        key: request.key.clone(),
        objective: request.objective.clone(),
        capability,
        expected_output,
        allowed_paths,
        depends_on: request.depends_on.clone(),
        budget,
    })
}

/// Admit a plan and record it durably.
///
/// The `DelegationPlanned` event is recorded even when the answer is `Single`:
/// a decision *not* to delegate is one the user is entitled to see the
/// reasoning for, and a plan that only appears in the log when it spawned
/// workers cannot answer "why did this run as one agent?".
#[allow(clippy::too_many_arguments)]
pub fn record_plan(
    store: &mut SessionStore,
    session_id: SessionId,
    turn_id: TurnId,
    plan: &DelegationPlan,
    registry: &CapabilityRegistry,
    workspace_ceiling: &ToolCeiling,
    parent_ceiling: &ToolCeiling,
    governance: &DelegationGovernance,
    state: &SessionState,
) -> Result<(Vec<PlannedDelegation>, Vec<RefusedUnit>), ApiError> {
    store.append(
        session_id,
        &SessionEvent::DelegationPlanned {
            plan: Box::new(plan.clone()),
        },
    )?;
    if !plan.delegates() {
        return Ok((Vec::new(), Vec::new()));
    }

    let (admitted, refusals) = admit_plan(
        plan,
        registry,
        &RoutingPolicy::default(),
        PlanningContext {
            parent_session_id: session_id,
            parent_turn_id: turn_id,
            workspace_ceiling,
            parent_ceiling,
            governance,
            ledger: &state.delegation_ledger,
        },
    );

    for planned in &admitted {
        store.append(
            session_id,
            &SessionEvent::DelegationCreated {
                delegation: Box::new(planned.delegation.clone()),
            },
        )?;
        store.append(
            session_id,
            &SessionEvent::DelegationRoutingRecorded {
                delegation_id: planned.delegation.id(),
                decision: planned.specialist.decision.clone(),
            },
        )?;
    }

    Ok((
        admitted,
        refusals
            .into_iter()
            .map(|(key, error)| RefusedUnit {
                key,
                reason: error.to_string(),
            })
            .collect(),
    ))
}

// ---------------------------------------------------------------------------
// Execution.
// ---------------------------------------------------------------------------

/// Provision a workspace, record the assignment, and start the worker.
///
/// Returns the provisioned workspace so the caller can collect its patch later.
pub async fn start_worker(
    store: &mut SessionStore,
    session_id: SessionId,
    parent: &SessionWorktree,
    delegation: &Delegation,
    profile: &str,
    profile_digest: &str,
    model_role: Option<purrcode_runtime_core::ModelRoleName>,
) -> Result<(WorkerId, ProvisionedWorkspace), ApiError> {
    let worker_id = WorkerId::new();
    let workspace = WorkerWorkspaceManager::provision(parent, delegation, worker_id)
        .await
        .map_err(|error| ApiError::Conflict(error.to_string()))?;
    store.append(
        session_id,
        &SessionEvent::DelegationWorkerAssigned {
            assignment: Box::new(WorkerAssignment {
                worker_id,
                delegation_id: delegation.id(),
                agent_profile: profile.to_owned(),
                profile_digest: profile_digest.to_owned(),
                model_role,
                workspace: workspace.record.clone(),
                assigned_at: Utc::now(),
            }),
        },
    )?;
    store.append(
        session_id,
        &SessionEvent::DelegationWorkerStarted {
            delegation_id: delegation.id(),
            worker_id,
        },
    )?;
    Ok((worker_id, workspace))
}

/// Record a finished worker and, when it proposes changes, open an integration
/// proposal for it.
///
/// This is the seam where "the worker finished" becomes "the parent has a
/// decision to make". It never applies anything.
pub async fn finish_worker(
    store: &mut SessionStore,
    session_id: SessionId,
    parent: &SessionWorktree,
    delegation: &Delegation,
    workspace: &ProvisionedWorkspace,
    mut result: WorkerResult,
    other_pending: &[PendingPatch],
) -> Result<IntegrationDecision, ApiError> {
    // The worker's *claimed* changed paths are not trusted: git is asked.
    let patch = match workspace.worktree.as_ref() {
        Some(worktree) => WorkerWorkspaceManager::collect_patch(worktree)
            .await
            .map_err(|error| ApiError::Conflict(error.to_string()))?,
        None => Default::default(),
    };
    result.changed_paths = patch.changed_paths.clone();
    result.patch_digest = (!patch.is_empty()).then(|| patch.patch_digest.clone());
    result.usage.changed_files = patch.changed_paths.len();

    // A result that escaped its scope is recorded as a failure rather than
    // silently dropped: the user needs to see that a worker did something it
    // was not permitted to, and the worktree is retained for inspection.
    if let Err(error) = result.validate_against(delegation) {
        store.append(
            session_id,
            &SessionEvent::DelegationWorkerFailed {
                delegation_id: delegation.id(),
                worker_id: result.worker_id,
                reason: error.to_string(),
            },
        )?;
        return Ok(IntegrationDecision::Rejected {
            reason: error.to_string(),
        });
    }

    let succeeded = result.status.is_success();
    let worker_id = result.worker_id;
    let proposes = result.proposes_changes();
    store.append(
        session_id,
        &SessionEvent::DelegationResultRecorded {
            result: Box::new(result.clone()),
        },
    )?;
    if succeeded {
        store.append(
            session_id,
            &SessionEvent::DelegationWorkerCompleted {
                delegation_id: delegation.id(),
                worker_id,
            },
        )?;
    } else {
        store.append(
            session_id,
            &SessionEvent::DelegationWorkerFailed {
                delegation_id: delegation.id(),
                worker_id,
                reason: result.summary.clone(),
            },
        )?;
        return Ok(IntegrationDecision::Rejected {
            reason: result.summary.clone(),
        });
    }

    if !proposes {
        // A review or investigation has nothing to integrate; its findings are
        // the deliverable.
        return Ok(IntegrationDecision::ReadyForApproval);
    }

    let current = current_snapshot_digest(parent)
        .await
        .map_err(|error| ApiError::Conflict(error.to_string()))?;
    let proposal = propose_integration(
        delegation,
        &result,
        &workspace.record.base_snapshot_digest,
        &current,
        hunks_of(&patch.patch),
    )
    .map_err(|error| ApiError::Conflict(error.to_string()))?;

    // Cross-worker conflicts are computed against every other patch still
    // waiting to land, not just the ones already applied.
    let mut pending = other_pending.to_vec();
    pending.push(PendingPatch::from_worker_patch(delegation.id(), &patch));
    let cross = conflicts_among(&pending);
    let mine: Vec<_> = cross
        .into_iter()
        .filter(|conflict| conflict.delegations.contains(&delegation.id()))
        .collect();

    let decision = evaluate_proposal(&proposal, &mine, delegation.access().is_writable());
    let mut proposal = proposal;
    proposal.conflicts.extend(mine.iter().cloned());
    store.append(
        session_id,
        &SessionEvent::IntegrationProposed {
            proposal: Box::new(proposal),
        },
    )?;
    if let IntegrationDecision::Conflict { conflicts } = &decision {
        store.append(
            session_id,
            &SessionEvent::IntegrationConflictDetected {
                delegation_id: delegation.id(),
                conflicts: conflicts.clone(),
            },
        )?;
    }
    Ok(decision)
}

/// Apply an approved integration to the parent worktree.
///
/// `hunks` empty means "the whole patch". A subset produces a *new* patch with
/// its own digest, and that digest — not the worker's — is what the approval
/// and the applied event both carry.
pub async fn accept_integration(
    store: &mut SessionStore,
    session_id: SessionId,
    parent: &SessionWorktree,
    record: &DelegationRecord,
    hunks: &[usize],
) -> Result<Vec<PathBuf>, ApiError> {
    // Proposal first: a delegation that never produced one has nothing to
    // accept, and saying "no worker" would send the user looking for the wrong
    // problem.
    let proposal = record
        .proposal
        .as_ref()
        .ok_or_else(|| ApiError::Conflict("there is no integration proposal to accept".into()))?;
    let assignment = record
        .assignment
        .as_ref()
        .ok_or_else(|| ApiError::Conflict("this delegation has no worker".into()))?;
    if record.integration == IntegrationState::Applied {
        return Err(ApiError::Conflict(
            "this delegation's patch has already been applied".into(),
        ));
    }
    if proposal
        .conflicts
        .iter()
        .chain(record.conflicts.iter())
        .any(|conflict| !conflict.kind.is_auto_mergeable())
    {
        return Err(ApiError::Conflict(
            "unresolved conflicts must be resolved before this patch can be accepted".into(),
        ));
    }

    let worktree = WorkerWorkspaceManager::reopen(
        &parent.source_repository,
        &assignment.workspace,
        assignment.worker_id,
    )
    .ok_or_else(|| {
        ApiError::Conflict(
            "the worker's worktree is no longer on disk; its patch cannot be applied".into(),
        )
    })?;
    let patch = WorkerWorkspaceManager::collect_patch(&worktree)
        .await
        .map_err(|error| ApiError::Conflict(error.to_string()))?;

    // The worker may have kept working after the proposal was shown. Applying
    // bytes the user never saw is exactly what the digest check exists to stop.
    if patch.patch_digest != proposal.patch_digest {
        return Err(ApiError::Conflict(
            "the worker's patch changed since it was proposed; re-review before accepting".into(),
        ));
    }

    let (bytes, digest) = if hunks.is_empty() {
        (patch.patch.clone(), patch.patch_digest.clone())
    } else {
        let subset = select_hunks(&patch.patch, hunks)
            .map_err(|error| ApiError::BadRequest(error.to_string()))?;
        let digest = blake3::hash(&subset).to_hex().to_string();
        // A selected-hunk integration is a different patch, so the proposal is
        // re-recorded with the amended digest before it is approved. The
        // reducer then requires the applied digest to match this one.
        let mut amended = proposal.clone();
        amended.amended_patch_digest = Some(digest.clone());
        store.append(
            session_id,
            &SessionEvent::IntegrationProposed {
                proposal: Box::new(amended),
            },
        )?;
        (subset, digest)
    };

    store.append(
        session_id,
        &SessionEvent::IntegrationApproved {
            delegation_id: record.delegation.id(),
            patch_digest: digest.clone(),
            authority: ApprovalAuthority::Human,
        },
    )?;
    let applied = apply_to_parent(parent, &bytes, &digest)
        .await
        .map_err(|error| ApiError::Conflict(error.to_string()))?;
    store.append(
        session_id,
        &SessionEvent::IntegrationApplied {
            delegation_id: record.delegation.id(),
            patch_digest: digest,
            changed_paths: applied.newly_changed_paths.clone(),
        },
    )?;

    // The patch has landed; the worktree is no longer holding anything
    // unresolved, so it is released.
    let _ = WorkerWorkspaceManager::release(&worktree, false).await;
    Ok(applied.newly_changed_paths)
}

/// Reject a worker's proposal. The worktree is released — the decision is
/// recorded, so the patch is reconstructible from evidence if anyone asks.
pub async fn reject_integration(
    store: &mut SessionStore,
    session_id: SessionId,
    parent: &SessionWorktree,
    record: &DelegationRecord,
    reason: &str,
) -> Result<(), ApiError> {
    if record.proposal.is_none() {
        return Err(ApiError::Conflict(
            "there is no integration proposal to reject".into(),
        ));
    }
    store.append(
        session_id,
        &SessionEvent::IntegrationRejected {
            delegation_id: record.delegation.id(),
            reason: reason.to_owned(),
        },
    )?;
    if let Some(assignment) = record.assignment.as_ref()
        && let Some(worktree) = WorkerWorkspaceManager::reopen(
            &parent.source_repository,
            &assignment.workspace,
            assignment.worker_id,
        )
    {
        let _ = WorkerWorkspaceManager::release(&worktree, false).await;
    }
    Ok(())
}

/// Build the integration review for one delegation (§PR12).
pub async fn integration_review(
    parent: &SessionWorktree,
    record: &DelegationRecord,
) -> Result<IntegrationReviewView, ApiError> {
    let proposal = record
        .proposal
        .as_ref()
        .ok_or_else(|| ApiError::Conflict("this delegation has no proposal".into()))?;
    let assignment = record
        .assignment
        .as_ref()
        .ok_or_else(|| ApiError::Conflict("this delegation has no worker".into()))?;

    let patch = match WorkerWorkspaceManager::reopen(
        &parent.source_repository,
        &assignment.workspace,
        assignment.worker_id,
    ) {
        Some(worktree) => WorkerWorkspaceManager::collect_patch(&worktree).await.ok(),
        None => None,
    };
    let hunks = patch
        .as_ref()
        .map(|patch| hunks_of(&patch.patch))
        .unwrap_or_default();
    let current = current_snapshot_digest(parent).await.ok();

    Ok(IntegrationReviewView {
        delegation: delegation_view(record),
        patch_digest: proposal.patch_digest.clone(),
        effective_patch_digest: proposal.effective_patch_digest().to_owned(),
        base_snapshot_digest: proposal.base_snapshot_digest.clone(),
        base_drifted: current
            .map(|current| current != proposal.base_snapshot_digest)
            .unwrap_or(false),
        patch: patch
            .as_ref()
            .and_then(|patch| String::from_utf8(patch.patch.clone()).ok()),
        hunks: hunks
            .into_iter()
            .enumerate()
            .map(|(index, hunk)| HunkView {
                index,
                path: hunk.path.display().to_string(),
                old_start: hunk.old_start,
                old_lines: hunk.old_lines,
                new_start: hunk.new_start,
                new_lines: hunk.new_lines,
            })
            .collect(),
        decision: match record.integration {
            IntegrationState::Conflicted => "conflict".into(),
            IntegrationState::Applied => "applied".into(),
            IntegrationState::Rejected => "rejected".into(),
            IntegrationState::Approved => "approved".into(),
            _ => "ready_for_approval".into(),
        },
    })
}

/// Patches still waiting to land, for cross-worker conflict detection.
pub async fn pending_patches(
    parent: &SessionWorktree,
    state: &SessionState,
    excluding: Option<DelegationId>,
) -> Vec<PendingPatch> {
    let mut pending = Vec::new();
    for record in state.delegations.values() {
        if Some(record.delegation.id()) == excluding {
            continue;
        }
        if !matches!(
            record.integration,
            IntegrationState::Proposed | IntegrationState::Conflicted
        ) {
            continue;
        }
        let Some(assignment) = record.assignment.as_ref() else {
            continue;
        };
        let Some(worktree) = WorkerWorkspaceManager::reopen(
            &parent.source_repository,
            &assignment.workspace,
            assignment.worker_id,
        ) else {
            continue;
        };
        if let Ok(patch) = WorkerWorkspaceManager::collect_patch(&worktree).await {
            pending.push(PendingPatch::from_worker_patch(
                record.delegation.id(),
                &patch,
            ));
        }
    }
    pending
}

/// Assemble the brief for one worker (§PR6).
pub fn worker_brief(
    delegation: &Delegation,
    state: &SessionState,
    tool_manifest: Vec<String>,
) -> WorkerContext {
    // Dependencies contribute a summary, never their transcript.
    let dependencies: Vec<DependencySummary> = delegation
        .dependencies()
        .iter()
        .filter_map(|id| state.delegations.get(id))
        .filter_map(|record| {
            record.result.as_ref().map(|result| {
                DependencySummary::from_result(result, &record.delegation.capability().to_string())
            })
        })
        .collect();

    purrcode_delegation_runtime::context::assemble(
        delegation,
        ContextInputs {
            // The parent's *objective*, not the parent's conversation. Cloning
            // the main agent's history into every worker is how a multi-agent
            // run costs five times a single-agent one (§PR6).
            parent_summary: state.objective.as_deref().unwrap_or_default(),
            dependencies,
            findings: Vec::new(),
            memory_excerpts: Vec::new(),
            graph_context: Vec::new(),
            tool_manifest,
        },
    )
}

/// Judge and execute one action a delegated worker proposed.
///
/// Every action goes through `Policy::evaluate_delegated`, so the delegation's
/// scope is enforced *while the worker runs* rather than only checked when its
/// result comes back.
pub async fn execute_worker_action(
    store: &mut SessionStore,
    worker_session: SessionId,
    delegation: &Delegation,
    policy: &Policy,
    worktree: &std::path::Path,
    action: ProposedAction,
) -> Result<purrcode_claw::ExecutionResult, String> {
    let action_id = ActionId::new();
    store
        .append(
            worker_session,
            &SessionEvent::ActionProposed {
                action_id,
                action: action.clone(),
                turn_id: None,
            },
        )
        .map_err(|error| error.to_string())?;
    let decision = policy.evaluate_delegated(&action, worktree, delegation);
    store
        .append(
            worker_session,
            &SessionEvent::JudgmentRecorded {
                action_id,
                decision: decision.clone(),
                turn_id: None,
            },
        )
        .map_err(|error| error.to_string())?;
    let JudgmentDecision::AllowWithConstraints(constraints) = decision else {
        // A delegated worker cannot approve its own action, and there is no
        // human attached to a worker: anything short of a deterministic allow
        // ends the worker's attempt.
        return Err(format!(
            "action was not auto-authorized for delegation {}",
            delegation.id().short()
        ));
    };
    let digest = action
        .digest(&constraints)
        .map_err(|error| error.to_string())?;
    store
        .authorize(&Authorization {
            action_id,
            session_id: worker_session,
            action_digest: digest,
            constraints: constraints.clone(),
            authorized_at: Utc::now(),
            approved_by: ApprovalAuthority::DeterministicPolicy,
        })
        .map_err(|error| error.to_string())?;
    store
        .append(
            worker_session,
            &SessionEvent::ExecutionStarted { action_id },
        )
        .map_err(|error| error.to_string())?;
    let result = ToolRuntime::execute(store, action_id, &action, &constraints)
        .await
        .map_err(|error| error.to_string())?;
    store
        .append(
            worker_session,
            &SessionEvent::ExecutionFinished {
                action_id,
                exit_code: result.exit_code,
                truncated: result.truncated,
                sandbox_level: Some(format!("{:?}", result.sandbox_level)),
                sandbox_backend: Some(result.sandbox_backend.clone()),
            },
        )
        .map_err(|error| error.to_string())?;
    Ok(result)
}

/// Route a failing validation back to the worker that caused it (§PR9).
///
/// Returns `None` when nobody is responsible or the repair bound is reached —
/// in both cases the main agent, not another worker, takes it from here.
pub fn repair_for_failure(
    state: &SessionState,
    validation_name: &str,
    detail: &str,
) -> Option<(DelegationId, RepairPlan)> {
    let candidates: Vec<&WorkerResult> = state
        .delegations
        .values()
        .filter(|record| record.integration == IntegrationState::Applied)
        .filter_map(|record| record.result.as_ref())
        .collect();
    let responsible = attribute_failure(detail, candidates.into_iter())?;
    let record = state.delegations.get(&responsible.delegation_id)?;
    let trigger = RepairTrigger::ValidationFailed {
        name: validation_name.to_owned(),
        detail: detail.to_owned(),
    };
    Some((
        responsible.delegation_id,
        plan_repair(
            responsible.delegation_id,
            responsible.worker_id,
            record.repair_cycles,
            &trigger,
        ),
    ))
}

/// Build the repair delegation for a failure routed back to a worker (§PR9).
///
/// A repair is a **new** delegation with the same scope and specialist, not a
/// restart of the finished one. Restarting would mean reopening a terminal
/// record, which is precisely what the at-most-once rule forbids; the original
/// keeps its result and carries the cycle counter that bounds how many repairs
/// it may spawn.
pub fn repair_delegation(
    original: &Delegation,
    trigger_summary: &str,
    workspace_ceiling: &ToolCeiling,
    parent_ceiling: &ToolCeiling,
    profile_ceiling: &ToolCeiling,
    remaining: &DelegationBudget,
) -> Result<Delegation, ApiError> {
    use purrcode_runtime_core::delegation::{AuthorityInputs, DelegationRequest};
    use purrcode_runtime_core::work::{AcceptanceCriterion, CriterionId};

    DelegationRequest {
        parent_session_id: original.parent_session_id(),
        parent_turn_id: original.parent_turn_id(),
        objective: format!(
            "Repair the failure your previous change caused, without widening scope.\n\n{trigger_summary}"
        ),
        capability: original.capability().clone(),
        acceptance_criteria: vec![AcceptanceCriterion {
            id: CriterionId::new(),
            statement: "the reported failure no longer reproduces".into(),
        }],
        context_refs: original.context_refs().to_vec(),
        allowed_paths: original.allowed_paths().to_vec(),
        expected_output: original.expected_output(),
        dependencies: Vec::new(),
        budget: *original.budget(),
    }
    .admit(AuthorityInputs {
        workspace: workspace_ceiling,
        parent: parent_ceiling,
        profile: profile_ceiling,
        parent_remaining_budget: remaining,
        depth: 1,
    })
    .map_err(|error| ApiError::Conflict(error.to_string()))
}

/// A worker result for a run that failed before producing anything.
pub fn failed_result(
    delegation: &Delegation,
    worker_id: WorkerId,
    reason: String,
    usage: UsageSummary,
) -> WorkerResult {
    WorkerResult {
        delegation_id: delegation.id(),
        worker_id,
        status: WorkerResultStatus::Failed {
            reason: reason.clone(),
        },
        summary: reason,
        changed_paths: Vec::new(),
        patch_digest: None,
        findings: Vec::new(),
        validations: Vec::new(),
        unresolved: Vec::new(),
        evidence_ids: Vec::new(),
        usage,
        completed_at: Utc::now(),
    }
}
