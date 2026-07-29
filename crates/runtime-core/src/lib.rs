//! Provider-independent domain contracts for the trusted runtime.
//!
//! These types are shared across PawGate, Claw, and NineLives so the trusted
//! path stays provider-independent.

use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use thiserror::Error;
use uuid::Uuid;

#[derive(
    Clone, Copy, Debug, Eq, Hash, JsonSchema, Ord, PartialEq, PartialOrd, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct SessionId(pub Uuid);

impl SessionId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for SessionId {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(
    Clone, Copy, Debug, Eq, Hash, JsonSchema, Ord, PartialEq, PartialOrd, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct ActionId(pub Uuid);

impl ActionId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for ActionId {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct CommandAction {
    pub program: PathBuf,
    pub arguments: Vec<String>,
    pub working_directory: PathBuf,
    #[serde(default)]
    pub environment: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct WriteFileAction {
    /// Repository-relative path. Absolute paths and parent traversal are invalid.
    pub path: PathBuf,
    pub content: String,
    /// When present, the current file must match this BLAKE3 digest.
    pub expected_digest: Option<String>,
}

#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct DeleteFileAction {
    /// Repository-relative path. Absolute paths and parent traversal are invalid.
    pub path: PathBuf,
    /// Deletion always requires an exact current-content digest.
    pub expected_digest: String,
}

#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct ExternalToolAction {
    pub server_id: String,
    pub tool_name: String,
    pub arguments: serde_json::Value,
    pub working_directory: PathBuf,
}

#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ProposedAction {
    Command(CommandAction),
    WriteFile(WriteFileAction),
    DeleteFile(DeleteFileAction),
    ExternalTool(ExternalToolAction),
}

impl ProposedAction {
    pub fn digest(&self, constraints: &ActionConstraints) -> Result<String, DomainError> {
        let canonical = serde_json::to_vec(&(self, constraints))?;
        Ok(blake3::hash(&canonical).to_hex().to_string())
    }
}

#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct ActionConstraints {
    pub working_directory: PathBuf,
    pub network: bool,
    pub timeout_seconds: u64,
    pub maximum_output_bytes: usize,
    #[serde(default)]
    pub allowed_write_globs: Vec<String>,
    pub maximum_changed_files: usize,
}

impl ActionConstraints {
    pub fn read_only(working_directory: PathBuf) -> Self {
        Self {
            working_directory,
            network: false,
            timeout_seconds: 120,
            maximum_output_bytes: 1_048_576,
            allowed_write_globs: Vec::new(),
            maximum_changed_files: 0,
        }
    }
}

#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(tag = "decision", content = "details", rename_all = "snake_case")]
pub enum JudgmentDecision {
    Allow,
    AllowWithConstraints(ActionConstraints),
    RequireApproval {
        reason: String,
        constraints: ActionConstraints,
    },
    ModifyAction {
        reason: String,
    },
    Replan {
        reason: String,
    },
    Deny {
        reason: String,
    },
}

#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct TaskIntent {
    pub objective: String,
    #[serde(default)]
    pub accepted_requirements: Vec<String>,
}

#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct PlanSnapshot {
    pub revision: u64,
    pub steps: Vec<PlanStep>,
}

#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct PlanStep {
    pub id: String,
    pub objective: String,
    #[serde(default)]
    pub preconditions: Vec<String>,
    #[serde(default)]
    pub expected_postconditions: Vec<String>,
}

#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct JudgmentEvidence {
    pub id: String,
    pub kind: String,
    pub source: String,
    pub excerpt: String,
    pub digest: String,
}

#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct PriorActionResult {
    pub action_id: ActionId,
    pub summary: String,
    pub successful: bool,
    #[serde(default)]
    pub affected_paths: Vec<PathBuf>,
}

#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct DiffSummary {
    #[serde(default)]
    pub changed_paths: Vec<PathBuf>,
    pub patch_digest: String,
    pub additions: usize,
    pub deletions: usize,
}

#[derive(
    Clone, Copy, Debug, Eq, JsonSchema, Ord, PartialEq, PartialOrd, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum RiskClass {
    Low,
    Medium,
    High,
    Critical,
}

#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct ContextualJudgmentRequest {
    pub task: TaskIntent,
    pub plan: PlanSnapshot,
    pub current_step: PlanStep,
    pub proposed_action: ProposedAction,
    pub constraints: ActionConstraints,
    #[serde(default)]
    pub repository_evidence: Vec<JudgmentEvidence>,
    #[serde(default)]
    pub prior_results: Vec<PriorActionResult>,
    pub current_diff: DiffSummary,
    pub risk_class: RiskClass,
}

#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextualDecision {
    Allow,
    RequireApproval,
    Replan,
    Deny,
}

#[derive(Clone, Debug, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct ContextualJudgment {
    pub decision: ContextualDecision,
    pub confidence: f32,
    pub reasons: Vec<String>,
    #[serde(default)]
    pub cited_evidence_ids: Vec<String>,
    #[serde(default)]
    pub required_changes: Vec<String>,
    pub escalation: Option<String>,
}

#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct OutcomeEvidence {
    pub id: String,
    pub stage: String,
    pub status: ValidationStatus,
    pub detail: String,
}

#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct OutcomeJudgmentRequest {
    pub task: TaskIntent,
    pub plan: PlanSnapshot,
    pub final_diff: DiffSummary,
    pub validation_evidence: Vec<OutcomeEvidence>,
    pub risk_class: RiskClass,
}

#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct Authorization {
    pub action_id: ActionId,
    pub session_id: SessionId,
    pub action_digest: String,
    pub constraints: ActionConstraints,
    pub authorized_at: DateTime<Utc>,
    pub approved_by: ApprovalAuthority,
}

#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalAuthority {
    DeterministicPolicy,
    Human,
    SignedPolicy { policy_id: String },
}

#[derive(Clone, Debug, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", content = "data", rename_all = "snake_case")]
pub enum SessionEvent {
    SessionCreated {
        objective: String,
        repository: PathBuf,
    },
    ConversationMessageAdded {
        message: ConversationMessage,
    },
    WorktreeCreated {
        path: PathBuf,
        base_head: String,
        source_was_dirty: bool,
    },
    SubmodulesPrepared {
        initialized: Vec<PathBuf>,
        unavailable: Vec<PathBuf>,
    },
    PlanCreated {
        steps: Vec<String>,
    },
    PlanRevised {
        revision: u64,
        reason: String,
        steps: Vec<String>,
    },
    ContextCompacted {
        summary: String,
        retained_action_ids: Vec<ActionId>,
    },
    SessionPaused {
        reason: String,
    },
    SessionResumed,
    ModelSelected {
        model: String,
    },
    SupervisorStarted {
        workers: usize,
    },
    WorkerFinished {
        worker_id: String,
        status: String,
        changed_paths: Vec<PathBuf>,
    },
    SupervisorReviewRequired {
        conflicts: Vec<PathBuf>,
    },
    ContextIndexed {
        files: usize,
        symbols: usize,
        sensitive_files: usize,
    },
    ModelRequestStarted {
        role: String,
        provider: String,
        model: String,
    },
    ModelRequestFinished {
        role: String,
        input_tokens: Option<u64>,
        output_tokens: Option<u64>,
    },
    ActionProposed {
        action_id: ActionId,
        action: ProposedAction,
    },
    ActionSuperseded {
        previous_action_id: ActionId,
        replacement_action_id: ActionId,
        reason: String,
    },
    JudgmentRecorded {
        action_id: ActionId,
        decision: JudgmentDecision,
    },
    ContextualJudgmentRecorded {
        action_id: ActionId,
        judgment: ContextualJudgment,
    },
    OutcomeJudgmentRecorded {
        judgment: ContextualJudgment,
    },
    OutcomeReviewRequired {
        reason: String,
    },
    OutcomeReviewApproved {
        authority: ApprovalAuthority,
    },
    ApprovalRecorded {
        action_id: ActionId,
        authority: ApprovalAuthority,
        action_digest: String,
    },
    ApprovalRejected {
        action_id: ActionId,
        reason: String,
    },
    AuthorizationPersisted {
        authorization: Authorization,
    },
    ExecutionStarted {
        action_id: ActionId,
    },
    ExecutionFinished {
        action_id: ActionId,
        exit_code: Option<i32>,
        truncated: bool,
        #[serde(default)]
        sandbox_level: Option<String>,
        #[serde(default)]
        sandbox_backend: Option<String>,
    },
    ActionOutputRecorded {
        action_id: ActionId,
        stdout: String,
        stderr: String,
        truncated: bool,
    },
    ValidationRecorded {
        action_id: ActionId,
        status: ValidationStatus,
        evidence: String,
    },
    CheckpointCreated {
        label: String,
        head: String,
        patch_digest: String,
    },
    WorktreeDispositionRecorded {
        strategy: String,
        detail: String,
    },
    SessionCancelled {
        reason: String,
    },
    RecoveryRequired {
        reason: String,
    },
    SessionCompleted,
    SessionFailed {
        reason: String,
    },
    // ── Research / skill lifecycle events ─────────────────
    CapabilityGapDetected {
        gap_description: String,
        task_context: String,
    },
    SkillSearchStarted {
        query: String,
        sources: Vec<String>,
    },
    SkillCandidateDiscovered {
        candidate_id: String,
        source: String,
        rank: u32,
    },
    SkillCandidateRanked {
        candidate_id: String,
        rank: u32,
        signals: serde_json::Value,
    },
    SkillInspectionOpened {
        skill_id: String,
        duration_ms: u64,
    },
    SkillInstallApproved {
        skill_id: String,
        scope: String,
    },
    SkillInstallRejected {
        skill_id: String,
        reason: String,
    },
    SkillQualified {
        skill_id: String,
        status: QualificationStatus,
        latency_ms: u64,
    },
    SkillQualificationStarted {
        skill_id: String,
    },
    SkillQualificationFailed {
        skill_id: String,
        failures: Vec<String>,
    },
    SkillInvoked {
        skill_id: String,
        tool_name: String,
    },
    SkillInvocationSucceeded {
        skill_id: String,
        latency_ms: u64,
    },
    SkillInvocationFailed {
        skill_id: String,
        error: String,
    },
    InstalledSkillReused {
        skill_id: String,
        previous_uses: u32,
    },
    InstalledSkillMatched {
        skill_id: String,
        matched_capability: String,
    },
    ExternalSearchAvoided {
        skill_id: String,
        matched_capability: String,
    },
    SkillUpdated {
        skill_id: String,
        old_version: String,
        new_version: String,
    },
    SkillRemoved {
        skill_id: String,
        reason: String,
    },
    ResearchSearchPerformed {
        query: String,
        url: String,
        content_digest: String,
        excerpt: String,
    },
}

#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValidationStatus {
    Passed,
    Failed,
    SkippedByConfiguration,
    Unavailable,
    NotDetected,
    TimedOut,
    Uncertain,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SessionStatus {
    Active,
    Paused,
    AwaitingApproval(ActionId),
    AwaitingReview,
    Executing(ActionId),
    Cancelled,
    Completed,
    Failed,
    Uncertain,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SessionState {
    pub id: SessionId,
    pub objective: Option<String>,
    pub repository: Option<PathBuf>,
    pub worktree: Option<PathBuf>,
    pub base_head: Option<String>,
    pub status: SessionStatus,
    pub event_count: u64,
    pub plan_revision: u64,
    pub plan_steps: Vec<String>,
    pub context_summary: Option<String>,
    pub selected_model: Option<String>,
    pub conversation_messages: Vec<ConversationMessage>,
    pub proposed_actions: BTreeMap<ActionId, ProposedAction>,
    pub judgments: BTreeMap<ActionId, JudgmentDecision>,
    pub contextual_judgments: BTreeMap<ActionId, ContextualJudgment>,
}

impl SessionState {
    pub fn empty(id: SessionId) -> Self {
        Self {
            id,
            objective: None,
            repository: None,
            worktree: None,
            base_head: None,
            status: SessionStatus::Active,
            event_count: 0,
            plan_revision: 0,
            plan_steps: Vec::new(),
            context_summary: None,
            selected_model: None,
            conversation_messages: Vec::new(),
            proposed_actions: BTreeMap::new(),
            judgments: BTreeMap::new(),
            contextual_judgments: BTreeMap::new(),
        }
    }

    pub fn apply(&mut self, event: &SessionEvent) {
        self.event_count += 1;
        match event {
            SessionEvent::SessionCreated {
                objective,
                repository,
            } => {
                self.objective = Some(objective.clone());
                self.repository = Some(repository.clone());
            }
            SessionEvent::WorktreeCreated {
                path, base_head, ..
            } => {
                self.worktree = Some(path.clone());
                self.base_head = Some(base_head.clone());
            }
            SessionEvent::ActionProposed { action_id, action } => {
                self.proposed_actions.insert(*action_id, action.clone());
            }
            SessionEvent::PlanCreated { steps } => {
                self.plan_revision += 1;
                self.plan_steps = steps.clone();
            }
            SessionEvent::PlanRevised {
                revision, steps, ..
            } => {
                self.plan_revision = *revision;
                self.plan_steps = steps.clone();
            }
            SessionEvent::ContextCompacted {
                summary,
                retained_action_ids,
            } => {
                let retained: BTreeSet<_> = retained_action_ids.iter().copied().collect();
                self.context_summary = Some(summary.clone());
                self.proposed_actions.retain(|id, _| retained.contains(id));
                self.judgments.retain(|id, _| retained.contains(id));
                self.contextual_judgments
                    .retain(|id, _| retained.contains(id));
            }
            SessionEvent::SessionPaused { .. } => self.status = SessionStatus::Paused,
            SessionEvent::SessionResumed => self.status = SessionStatus::Active,
            SessionEvent::ModelSelected { model } => self.selected_model = Some(model.clone()),
            SessionEvent::ConversationMessageAdded { message } => {
                self.conversation_messages.push(message.clone());
            }
            SessionEvent::JudgmentRecorded {
                action_id,
                decision,
            } => {
                self.judgments.insert(*action_id, decision.clone());
                if matches!(decision, JudgmentDecision::RequireApproval { .. }) {
                    self.status = SessionStatus::AwaitingApproval(*action_id);
                }
            }
            SessionEvent::ContextualJudgmentRecorded {
                action_id,
                judgment,
            } => {
                self.contextual_judgments
                    .insert(*action_id, judgment.clone());
            }
            SessionEvent::OutcomeReviewRequired { .. } => {
                self.status = SessionStatus::AwaitingReview
            }
            SessionEvent::OutcomeReviewApproved { .. } => self.status = SessionStatus::Active,
            SessionEvent::ApprovalRecorded { .. } => self.status = SessionStatus::Active,
            SessionEvent::ApprovalRejected { .. } => self.status = SessionStatus::Active,
            SessionEvent::ExecutionStarted { action_id } => {
                self.status = SessionStatus::Executing(*action_id)
            }
            SessionEvent::ExecutionFinished { .. } => self.status = SessionStatus::Active,
            SessionEvent::ValidationRecorded {
                status: ValidationStatus::Uncertain,
                ..
            } => self.status = SessionStatus::Uncertain,
            SessionEvent::SessionCompleted => self.status = SessionStatus::Completed,
            SessionEvent::SessionCancelled { .. } => self.status = SessionStatus::Cancelled,
            SessionEvent::RecoveryRequired { .. } => self.status = SessionStatus::Uncertain,
            SessionEvent::SessionFailed { .. } => self.status = SessionStatus::Failed,
            _ => {}
        }
    }
}

// ── Conversation types ────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ConversationMessage {
    pub id: String,
    pub role: String,
    pub content: String,
    pub timestamp: DateTime<Utc>,
    #[serde(default)]
    pub tool_calls: Vec<serde_json::Value>,
    #[serde(default)]
    pub tool_results: Vec<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct ConversationState {
    pub session_id: SessionId,
    pub messages: Vec<ConversationMessage>,
    pub mode: ConversationMode,
    pub selected_model: Option<String>,
    /// "local_only" or "mixed" — matches provider-gateway PrivacyMode but avoids a cross-crate dependency.
    pub privacy: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum ConversationMode {
    Plan,
    #[default]
    Build,
    Review,
    Ask,
}

// ── Qualification types ───────────────────────────────────────────

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum QualificationStatus {
    Qualified,
    QualifiedWithConstraints,
    Unverified,
    Failed,
    Blocked,
    Outdated,
    Incompatible,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct QualificationReport {
    pub skill_id: String,
    pub version: String,
    pub status: QualificationStatus,
    pub cases: Vec<QualificationCaseResult>,
    pub overall_latency_ms: u64,
    pub constraints: Option<ActionConstraints>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct QualificationCaseResult {
    pub name: String,
    pub passed: bool,
    pub latency_ms: u64,
    pub detail: String,
}

// ── Research / evidence types ─────────────────────────────────────

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct ResearchEvent {
    pub event_type: String,
    pub timestamp: DateTime<Utc>,
    pub session_id: SessionId,
    pub data: serde_json::Value,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct ResearchExport {
    pub exported_at: DateTime<Utc>,
    pub session_count: usize,
    pub events: Vec<ResearchEvent>,
    pub metrics: ResearchMetrics,
    pub redacted: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct ResearchMetrics {
    pub skill_discovery_success_rate: Option<f64>,
    pub skill_reuse_rate: Option<f64>,
    pub external_search_avoidance: Option<f64>,
    pub total_skill_invocations: u64,
    pub total_skill_installations: u64,
    pub total_capability_gaps: u64,
    pub total_external_searches: u64,
    pub skill_acquisition_overhead_ms: u64,
    pub qualification_failures: u64,
    pub invocation_denials: u64,
}

#[derive(Debug, Error)]
pub enum DomainError {
    #[error("could not serialize action authorization: {0}")]
    Serialization(#[from] serde_json::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_revision_and_context_compaction_replay_deterministically() {
        let mut state = SessionState::empty(SessionId::new());
        state.apply(&SessionEvent::PlanCreated {
            steps: vec!["inspect".into(), "fix".into()],
        });
        state.apply(&SessionEvent::PlanRevised {
            revision: 2,
            reason: "new evidence".into(),
            steps: vec!["inspect".into(), "fix safely".into()],
        });
        let retained = ActionId::new();
        let removed = ActionId::new();
        for id in [retained, removed] {
            state.apply(&SessionEvent::ActionProposed {
                action_id: id,
                action: ProposedAction::WriteFile(WriteFileAction {
                    path: PathBuf::from("file.txt"),
                    content: "value".into(),
                    expected_digest: None,
                }),
            });
        }
        state.apply(&SessionEvent::ContextCompacted {
            summary: "older evidence summarized".into(),
            retained_action_ids: vec![retained],
        });
        assert_eq!(state.plan_revision, 2);
        assert_eq!(state.plan_steps[1], "fix safely");
        assert!(state.proposed_actions.contains_key(&retained));
        assert!(!state.proposed_actions.contains_key(&removed));
        assert_eq!(
            state.context_summary.as_deref(),
            Some("older evidence summarized")
        );
    }

    #[test]
    fn digest_binds_constraints_and_arguments() {
        let root = PathBuf::from("/repo");
        let action = ProposedAction::Command(CommandAction {
            program: "git".into(),
            arguments: vec!["status".into()],
            working_directory: root.clone(),
            environment: BTreeMap::new(),
        });
        let original = action
            .digest(&ActionConstraints::read_only(root.clone()))
            .unwrap();
        let changed = ProposedAction::Command(CommandAction {
            program: "git".into(),
            arguments: vec!["reset".into(), "--hard".into()],
            working_directory: root.clone(),
            environment: BTreeMap::new(),
        });
        assert_ne!(
            original,
            changed.digest(&ActionConstraints::read_only(root)).unwrap()
        );
    }
}
