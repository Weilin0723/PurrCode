//! Provider-independent domain contracts for the trusted runtime.

pub mod adaptation;
pub mod authority;
pub mod capability;
pub mod correction;
pub mod delegation;
pub mod evidence;
pub mod expectation;
pub mod extension;
pub mod graph;
pub mod native_tools;
pub mod product_state;
pub mod review;
pub mod schema_validation;
pub mod terminal;
pub mod tool;
pub mod work;

pub use authority::{
    AuthenticationChannel, AuthorityMode, GrantCapability, GrantId, HumanAuthorityGrant,
    HumanIdentity,
};
pub use capability::{
    AdmissionDiagnostic, CapabilityId, CapabilityProvider, CapabilityRegistry, DiagnosticSeverity,
    ExtensionLayer,
};
pub use delegation::{
    AuthorityInputs, ContextRef, Delegation, DelegationBudget, DelegationClassification,
    DelegationError, DelegationGovernance, DelegationId, DelegationLedger, DelegationOrigin,
    DelegationPlan, DelegationRequest, DelegationSignals, DelegationStatus, DelegationUnitProposal,
    ExpectedOutput, FindingSeverity, IntegrationConflict, IntegrationConflictKind,
    IntegrationDecision, IntegrationProposal, OpenIssue, PathPattern, PlannedUnit, RepairDecision,
    ReviewResult, RoutingDecision, StructuredFinding, UsageSummary, ValidationEvidence,
    WorkerAssignment, WorkerId, WorkerResult, WorkerResultStatus, WorkerWorkspaceRecord,
    WorkspaceAccess,
};
pub use evidence::{EvidenceInitiator, ExecutionEvidence, ExecutionOutcome, RedactionClass};
pub use extension::{
    AgentDescriptor, AgentProfile, CommandDescriptor, CommandExecutionSpec, ContextPolicy,
    ContextRequirement, HookAction, HookDescriptor, HookTrigger, ModelRoleName, PermissionRequest,
    SkillDescriptor, SkillPolicy, SkillValidation, ToolPattern, ToolPolicy, ToolSelection,
    is_safe_default,
};
pub use graph::{GraphEdgeKind, GraphNodeKind};
pub use native_tools::builtin_native_proposals;
pub use product_state::{InputDisposition, ProductState, ProductStateView, StateColor};
pub use schema_validation::{SchemaViolation, validate as validate_against_schema};
pub use terminal::{
    OwnershipGeneration, OwnershipTransition, ResizeTerminalAction, SendTerminalInputAction,
    StartTerminalAction, StopProcessAction, TerminalAction, TerminalDimensions, TerminalId,
    TerminalInput, TerminalOwner, TerminalSessionRecord, TerminalStatus, TranscriptPolicy,
};
pub use tool::{
    ApprovalPolicy, DescriptorOrigin, FilesystemScope, NetworkScope, SideEffectClass, ToolCeiling,
    ToolDescriptor, ToolDescriptorProposal, ToolId, ToolInvocation, ToolProvider,
};
pub use work::{
    AcceptanceCriterion, CriterionId, DesignDecision, DesignDecisionId, EvidenceCoverage,
    EvidenceId, EvidenceLink, EvidenceObligation, Requirement, RequirementId, SpecBundle, SpecKind,
    TaskGraph, WorkModelError, WorkPriority, WorkRisk, WorkTask, WorkTaskId, WorkTaskStatus,
};

use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::{Path, PathBuf};
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

/// Identifies one `run_until_pause`/`run_planner` iteration (PRD v1.1 §6.2).
///
/// A turn may propose several actions (grep, read, judgment, output) that all
/// belong to the same model round-trip. Correlating them by `TurnId` is what
/// lets the IDE Work Log and the context ledger (`ContextLedgerEntry`) show
/// real provenance instead of `work_log_anchor`'s positional guess.
#[derive(
    Clone, Copy, Debug, Eq, Hash, JsonSchema, Ord, PartialEq, PartialOrd, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct TurnId(pub Uuid);

impl TurnId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for TurnId {
    fn default() -> Self {
        Self::new()
    }
}

/// Identifies one bounded unit of work nested inside a turn (reserved for
/// later phases — e.g. one Scout exploration step in Phase 5). Not yet
/// produced by Phase 1, but defined alongside `TurnId`/`ToolCallId` now so
/// later phases do not need another `runtime-core` migration.
#[derive(
    Clone, Copy, Debug, Eq, Hash, JsonSchema, Ord, PartialEq, PartialOrd, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct SpanId(pub Uuid);

impl SpanId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for SpanId {
    fn default() -> Self {
        Self::new()
    }
}

/// Identifies one tool invocation inside a turn (reserved for Phase 3's
/// action-set loop, where a single turn may carry several read-only
/// `ActionId`s and a UI needs to correlate each with its own call).
#[derive(
    Clone, Copy, Debug, Eq, Hash, JsonSchema, Ord, PartialEq, PartialOrd, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct ToolCallId(pub Uuid);

impl ToolCallId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for ToolCallId {
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

/// Default serialized bounds for typed directory/search reads.
pub const DEFAULT_FIND_MAX_DEPTH: u8 = 5;
pub const DEFAULT_LIST_MAX_ENTRIES: u32 = 1024;
pub const DEFAULT_GREP_MAX_RESULTS: u32 = 1_024;
/// Default byte bound for a single bounded file read.
pub const DEFAULT_READ_FILE_MAX_BYTES: usize = 1_048_576;
/// Upper limits enforced on every request so a model cannot amplify the bound.
pub const MAX_READ_FILE_BYTES: usize = 16 * 1_024 * 1_024;
pub const MAX_LIST_MAX_ENTRIES: u32 = 4096;
pub const MAX_GREP_MAX_RESULTS: u32 = 4096;
pub const MAX_GREP_MAX_BYTES: usize = 16 * 1_024 * 1_024;
pub const MAX_GIT_LOG_COUNT: u32 = 4096;

/// A bounded, deterministic repository read.
///
/// Repository reads are a privileged action class. Every variant is allowlisted,
/// network-denied, time-bounded, and confined to the session worktree. Claw
/// executes each variant natively (Rust traversal or `git`) without spawning
/// `find`, `ls`, or `rg`, so the same paths run on Windows without a POSIX
/// shell. Reads never require contextual judgment.
///
/// Every directory/search variant carries serialized bounds (`max_depth`,
/// `max_entries`, `max_results`, `max_bytes`). Clients may omit them and the
/// runtime fills the documented defaults; an explicit zero is rejected during
/// validation. `ReadFile` is the bounded counterpart of `WriteFile`: a single
/// repository-relative file read with a hard byte cap.
#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RepositoryReadAction {
    GitStatus,
    GitRevParse {
        revision: String,
    },
    GitLog {
        #[serde(default)]
        max_count: Option<u32>,
        #[serde(default)]
        oneline: bool,
    },
    GitDiff {
        #[serde(default)]
        paths: Vec<PathBuf>,
    },
    GitShow {
        revision: String,
        path: PathBuf,
    },
    GitLsFiles {
        #[serde(default)]
        pathspec: Vec<PathBuf>,
    },
    RepositoryGrep {
        pattern: String,
        #[serde(default)]
        paths: Vec<PathBuf>,
        #[serde(default)]
        case_insensitive: bool,
        #[serde(default = "default_grep_max_results")]
        max_results: u32,
        #[serde(default = "default_grep_max_bytes")]
        max_bytes: usize,
    },
    Find {
        #[serde(default)]
        paths: Vec<PathBuf>,
        #[serde(default = "default_find_max_depth")]
        max_depth: u8,
        #[serde(default = "default_find_max_entries")]
        max_entries: u32,
    },
    List {
        paths: Vec<PathBuf>,
        #[serde(default = "default_list_max_entries")]
        max_entries: u32,
    },
    /// A bounded, repository-relative single-file content read.
    ///
    /// `path` must be a non-empty repository-relative path that does not
    /// traverse out of the worktree. `max_bytes` defaults to
    /// [`DEFAULT_READ_FILE_MAX_BYTES`] and is clamped to
    /// [`MAX_READ_FILE_BYTES`]; a request for `0` bytes is invalid.
    ReadFile {
        path: PathBuf,
        #[serde(default = "default_read_file_max_bytes")]
        max_bytes: usize,
    },
}

fn default_find_max_depth() -> u8 {
    DEFAULT_FIND_MAX_DEPTH
}
fn default_find_max_entries() -> u32 {
    DEFAULT_LIST_MAX_ENTRIES
}
fn default_list_max_entries() -> u32 {
    DEFAULT_LIST_MAX_ENTRIES
}
fn default_grep_max_results() -> u32 {
    DEFAULT_GREP_MAX_RESULTS
}
fn default_grep_max_bytes() -> usize {
    DEFAULT_READ_FILE_MAX_BYTES
}
fn default_read_file_max_bytes() -> usize {
    DEFAULT_READ_FILE_MAX_BYTES
}

/// Canonicalize a repository-relative path used by typed reads.
///
/// The model may emit `.`, `./`, `./src/../src`, etc. `.`, `./`, and an empty
/// path all map to the repository root (`""`); redundant `CurDir` components
/// are stripped; `ParentDir` is permitted only when it cancels a preceding
/// normal component. Returns `None` when the path is absolute or escapes the
/// worktree. The returned representation is used by the digest so that `.` and
/// `./` produce the same authorization record.
pub fn canonicalize_repository_path(path: &Path) -> Option<PathBuf> {
    use std::path::Component;
    if path.as_os_str().is_empty() {
        return Some(PathBuf::new());
    }
    if path.is_absolute() {
        return None;
    }
    let mut stack: Vec<Component> = Vec::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(name) => stack.push(Component::Normal(name)),
            Component::ParentDir => match stack.last() {
                Some(Component::Normal(_)) => {
                    stack.pop();
                }
                None => return None,
                Some(_) => return None,
            },
            Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    let mut result = PathBuf::new();
    for component in stack {
        result.push(component.as_os_str());
    }
    Some(result)
}

/// True when `path` is a safe repository-relative path produced by
/// [`canonicalize_repository_path`] (i.e. exactly `Normal` components).
pub fn is_canonical_repository_path(path: &Path) -> bool {
    use std::path::Component;
    path.components()
        .all(|component| matches!(component, Component::Normal(_)))
}

impl RepositoryReadAction {
    /// Deterministically synthesize the shell invocation for this read.
    ///
    /// All arguments are constructed from typed fields; no user-supplied shell
    /// string is ever parsed. The working directory is the session worktree.
    ///
    /// Note: paths are canonicalized through [`canonicalize_repository_path`]
    /// first so that `.`, `./`, and equivalent forms all serialize to the same
    /// argument list (and therefore the same action digest).
    pub fn to_command(&self, working_directory: PathBuf) -> CommandAction {
        let mut environment = BTreeMap::new();
        environment.insert("GIT_TERMINAL_PROMPT".to_string(), "0".to_string());
        environment.insert("PAGER".to_string(), "cat".to_string());
        let canon = |p: &std::path::Path| canonicalize_repository_path(p).unwrap_or_default();
        let (program, arguments) = match self {
            Self::GitStatus => (
                PathBuf::from("git"),
                vec!["status".to_string(), "--porcelain".to_string()],
            ),
            Self::GitRevParse { revision } => (
                PathBuf::from("git"),
                vec!["rev-parse".to_string(), revision.clone()],
            ),
            Self::GitLog { max_count, oneline } => {
                let mut args = vec!["log".to_string()];
                if *oneline {
                    args.push("--oneline".to_string());
                }
                if let Some(count) = max_count {
                    args.push(format!("-{count}"));
                }
                (PathBuf::from("git"), args)
            }
            Self::GitDiff { paths } => {
                let mut args = vec!["diff".to_string()];
                for path in paths {
                    args.push("--".to_string());
                    args.push(canon(path).to_string_lossy().to_string());
                }
                (PathBuf::from("git"), args)
            }
            Self::GitShow { revision, path } => {
                let object = if path.as_os_str().is_empty() {
                    revision.clone()
                } else {
                    format!("{revision}:{}", canon(path).display())
                };
                (PathBuf::from("git"), vec!["show".to_string(), object])
            }
            Self::GitLsFiles { pathspec } => {
                let mut args = vec!["ls-files".to_string()];
                for spec in pathspec {
                    args.push(canon(spec).to_string_lossy().to_string());
                }
                (PathBuf::from("git"), args)
            }
            Self::RepositoryGrep {
                pattern,
                paths,
                case_insensitive,
                max_results,
                max_bytes: _,
            } => {
                let mut args = vec![
                    "--no-heading".to_string(),
                    "--line-number".to_string(),
                    format!("--max-count={}", *max_results),
                ];
                if *case_insensitive {
                    args.push("-i".to_string());
                }
                args.push("--".to_string());
                args.push(pattern.clone());
                for path in paths {
                    args.push(canon(path).to_string_lossy().to_string());
                }
                (PathBuf::from("rg"), args)
            }
            Self::Find {
                paths,
                max_depth,
                max_entries: _,
            } => {
                let mut args = Vec::new();
                for path in paths {
                    let canonical = canon(path);
                    if canonical.as_os_str().is_empty() {
                        args.push(".".to_string());
                    } else {
                        args.push(canonical.to_string_lossy().to_string());
                    }
                }
                args.push("-maxdepth".to_string());
                args.push(max_depth.to_string());
                (PathBuf::from("find"), args)
            }
            Self::List {
                paths,
                max_entries: _,
            } => {
                let mut args = vec!["-la".to_string()];
                for path in paths {
                    let canonical = canon(path);
                    if canonical.as_os_str().is_empty() {
                        args.push(".".to_string());
                    } else {
                        args.push(canonical.to_string_lossy().to_string());
                    }
                }
                (PathBuf::from("ls"), args)
            }
            Self::ReadFile { path, max_bytes: _ } => {
                // Single-file reads go through Claw's native cap-std reader;
                // the shell form is only a deterministic fallback for callers
                // that still exercise `to_command` (e.g. tests).
                let mut args = vec!["-c".to_string()];
                args.push(canon(path).to_string_lossy().to_string());
                (PathBuf::from("cat"), args)
            }
        };
        CommandAction {
            program,
            arguments,
            working_directory,
            environment,
        }
    }

    /// Clamp every serialized bound to its safe maximum and reject any
    /// explicit zero. Returns `Ok(())` when the action is internally
    /// well-formed. Path-level validation (containment, traversal, symlink
    /// resolution) lives in PawGate and Claw.
    pub fn validate_bounds(&self) -> Result<(), DomainError> {
        match self {
            Self::Find {
                max_depth,
                max_entries,
                ..
            } => {
                if *max_depth == 0 {
                    return Err(DomainError::InvalidBounds {
                        reason: "find max_depth must be between 1 and 5".into(),
                    });
                }
                if *max_depth > DEFAULT_FIND_MAX_DEPTH {
                    return Err(DomainError::InvalidBounds {
                        reason: format!("find max_depth must be at most {DEFAULT_FIND_MAX_DEPTH}"),
                    });
                }
                if *max_entries == 0 {
                    return Err(DomainError::InvalidBounds {
                        reason: "find max_entries must be greater than zero".into(),
                    });
                }
                if *max_entries > MAX_LIST_MAX_ENTRIES {
                    return Err(DomainError::InvalidBounds {
                        reason: format!("find max_entries must be at most {MAX_LIST_MAX_ENTRIES}"),
                    });
                }
            }
            Self::List { max_entries, .. } => {
                if *max_entries == 0 {
                    return Err(DomainError::InvalidBounds {
                        reason: "list max_entries must be greater than zero".into(),
                    });
                }
                if *max_entries > MAX_LIST_MAX_ENTRIES {
                    return Err(DomainError::InvalidBounds {
                        reason: format!("list max_entries must be at most {MAX_LIST_MAX_ENTRIES}"),
                    });
                }
            }
            Self::RepositoryGrep {
                max_results,
                max_bytes,
                ..
            } => {
                if *max_results == 0 {
                    return Err(DomainError::InvalidBounds {
                        reason: "repository_grep max_results must be greater than zero".into(),
                    });
                }
                if *max_bytes == 0 {
                    return Err(DomainError::InvalidBounds {
                        reason: "repository_grep max_bytes must be greater than zero".into(),
                    });
                }
                if *max_results > MAX_GREP_MAX_RESULTS {
                    return Err(DomainError::InvalidBounds {
                        reason: format!(
                            "repository_grep max_results must be at most {MAX_GREP_MAX_RESULTS}"
                        ),
                    });
                }
                if *max_bytes > MAX_GREP_MAX_BYTES {
                    return Err(DomainError::InvalidBounds {
                        reason: format!(
                            "repository_grep max_bytes must be at most {MAX_GREP_MAX_BYTES}"
                        ),
                    });
                }
            }
            Self::ReadFile { max_bytes, .. } => {
                if *max_bytes == 0 {
                    return Err(DomainError::InvalidBounds {
                        reason: "read_file max_bytes must be greater than zero".into(),
                    });
                }
                if *max_bytes > MAX_READ_FILE_BYTES {
                    return Err(DomainError::InvalidBounds {
                        reason: format!(
                            "read_file max_bytes must be at most {MAX_READ_FILE_BYTES}"
                        ),
                    });
                }
            }
            Self::GitLog {
                max_count: Some(count),
                ..
            } if *count == 0 => {
                return Err(DomainError::InvalidBounds {
                    reason: "git_log max_count must be greater than zero when set".into(),
                });
            }
            Self::GitLog {
                max_count: Some(count),
                ..
            } if *count > MAX_GIT_LOG_COUNT => {
                return Err(DomainError::InvalidBounds {
                    reason: format!("git_log max_count must be at most {MAX_GIT_LOG_COUNT}"),
                });
            }
            _ => {}
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ProposedAction {
    Command(CommandAction),
    RepositoryRead(RepositoryReadAction),
    WriteFile(WriteFileAction),
    DeleteFile(DeleteFileAction),
    ExternalTool(ExternalToolAction), // deprecated; kept for log replay (§10)
    Tool(ToolInvocation),             // NEW v1.3
}

impl ProposedAction {
    pub fn digest(&self, constraints: &ActionConstraints) -> Result<String, DomainError> {
        let canonical = serde_json::to_vec(&(self, constraints))?;
        Ok(blake3::hash(&canonical).to_hex().to_string())
    }

    /// v1.3: the descriptor digest is hashed alongside the action and the
    /// constraints. A descriptor mutated between authorize() and
    /// consume_authorization() no longer matches, so the capability is void.
    pub fn digest_v3(
        &self,
        constraints: &ActionConstraints,
        descriptor_digest: &str,
    ) -> Result<String, DomainError> {
        let canonical = serde_json::to_vec(&(self, constraints, descriptor_digest))?;
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

// ── Context ledger types (PRD v1.1 §6.2, Phase 1) ──────────────────

/// What kind of context a [`ContextLedgerSection`] carries.
///
/// `ToolEvidence` and `Reserve` are not yet produced by `build_messages()` —
/// they are defined now so Phase 3 (batched tool-read evidence kept out of
/// the main transcript) and any future headroom accounting can reuse this
/// enum instead of growing a second one.
#[derive(
    Clone, Copy, Debug, Eq, JsonSchema, Ord, PartialEq, PartialOrd, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum ContextClass {
    Instructions,
    ConversationTail,
    /// Plan, recent actions/results, and validation/repair routing — the
    /// per-turn task-state block `build_messages()` assembles into the final
    /// user message.
    TaskState,
    /// `whisker-context-engine` retrieval hits.
    RetrievedContext,
    /// Phase 2's `SemanticCheckpoint` (today: the flat `context_summary`
    /// string it replaces).
    CompactedCheckpoint,
    ToolEvidence,
    Reserve,
    /// Context a person or the project pinned for this turn, rather than
    /// something retrieval chose: resolved composer references (`@file`,
    /// `#symbol`), project instruction files, and selected project memory.
    /// Classed separately from `RetrievedContext` because the two answer
    /// different questions — "why did the index surface this?" versus "who
    /// asked for this to be here?" — and a context inspector that merges them
    /// cannot tell a user which of their references actually landed.
    PinnedContext,
}

/// Why one [`ContextLedgerSection`] was included in a turn's prompt.
///
/// `RetrievedByScout` is intentionally absent here: Phase 5 introduces
/// `ScoutId`, which does not exist in this codebase yet, so that variant is
/// added alongside `ScoutId` rather than referencing a type Phase 1 cannot
/// define correctly.
#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(tag = "reason", content = "detail", rename_all = "snake_case")]
pub enum WhyIncluded {
    /// Assembled unconditionally on every turn (developer instructions,
    /// conversation tail, plan/recent-actions/validation, the compacted
    /// checkpoint slot).
    AlwaysPresent,
    MatchedQuery {
        term: String,
    },
    RecentEdit,
    Pinned,
    /// A graph-derived hit: the node was reached by traversing a durable
    /// project-graph edge, not by matching the query lexically.
    RelatedByGraph {
        via_edge: GraphEdgeKind,
        from_node: String,
        hops: u8,
    },
}

/// Token/byte accounting for one logical slice of an assembled prompt.
///
/// `estimated_tokens` uses the same `chars().count().div_ceil(4)` heuristic
/// `ProviderRouter`'s default `count_tokens` uses
/// (`crates/provider-gateway/src/lib.rs`), so a ledger's `total_estimated_tokens`
/// is structurally comparable to — not a second, drifting estimate of — the
/// aggregate estimate `prepare_model_request` computes over the same text.
#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct ContextLedgerSection {
    pub class: ContextClass,
    /// Human-readable identity for the section, e.g.
    /// `"conversation_messages[0..7]"` or `"retrieved_context"`.
    pub label: String,
    pub estimated_tokens: u64,
    pub byte_len: usize,
    pub why_included: WhyIncluded,
}

/// One turn's full context-assembly accounting, durably recorded via
/// [`SessionEvent::ContextAssembled`].
///
/// How the token count was computed is tracked in [`TokenEstimator`].
///
/// The enum distinguishes provider-counted (authoritative) from the char/4 heuristic.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum TokenEstimator {
    /// Provider's native tokenizer — the authoritative count.
    ProviderCounted,
    /// chars().count().div_ceil(4) fallback — structurally matches the default
    /// ProviderRouter::count_tokens but may diverge from real tokenizers.
    #[default]
    CharDiv4,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ContextLedgerEntry {
    pub turn_id: TurnId,
    pub session_id: SessionId,
    pub sections: Vec<ContextLedgerSection>,
    pub total_estimated_tokens: u64,
    #[serde(default)]
    pub estimator: TokenEstimator,
    pub recorded_at: DateTime<Utc>,
}

/// Where one [`PinnedSection`]'s content came from.
///
/// The origin is carried into the ledger's section label so a user reading the
/// context inspector can tell "I typed `@src/auth.rs` and it was attached"
/// apart from "the project's AGENTS.md was attached" — both are pinned, but
/// only one of them is something the user asked for in this turn.
#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PinnedOrigin {
    /// A composer reference the user typed (`@file`, `@folder:`, `@diff`,
    /// `@git:`, `#symbol`), resolved against the repository.
    ComposerReference,
    /// A project instruction file in the repository root (AGENTS.md,
    /// CLAUDE.md, .purrcode.md).
    ProjectInstructions,
    /// A durable project-memory entry selected for this turn.
    ProjectMemory,
    /// Structured findings returned by a registered tool (skill validation,
    /// MCP tool result, hook output). Attached as data, not re-parsed prose —
    /// acceptance-test step 12.
    ToolFindings {
        tool_id: ToolId,
        action_id: ActionId,
    },
    /// A file reached by traversing the project-intelligence graph, NOT by a
    /// lexical match and NOT a project instruction file. Carrying the seed,
    /// edge kind and hop count here is what lets the context ledger report
    /// `WhyIncluded::RelatedByGraph` truthfully — pinning graph hits as
    /// `ProjectInstructions` told the user (and the model) that a repository
    /// had declared this file as standing guidance, which it had not.
    GraphRelated {
        from_node: String,
        via_edge: GraphEdgeKind,
        hops: u8,
    },
}

impl PinnedOrigin {
    /// The prefix used to build a ledger section label, so labels group by
    /// origin when the inspector sorts them.
    pub const fn label_prefix(&self) -> &'static str {
        match self {
            Self::ComposerReference => "reference",
            Self::ProjectInstructions => "project_instructions",
            Self::ProjectMemory => "project_memory",
            Self::ToolFindings { .. } => "tool_findings",
            Self::GraphRelated { .. } => "graph_related",
        }
    }

    /// How the section is introduced to the model. Composer references are the
    /// user's own words made concrete; instructions and memory are the
    /// project's standing knowledge, and labelling them as such is what stops
    /// the model from reading a remembered build command as a fresh request.
    pub const fn heading(&self) -> &'static str {
        match self {
            Self::ComposerReference => "ATTACHED REFERENCES (the user pinned these to this turn)",
            Self::ProjectInstructions => "PROJECT INSTRUCTIONS (from the repository)",
            Self::ProjectMemory => "PROJECT MEMORY (durable, auditable project knowledge)",
            Self::ToolFindings { .. } => "TOOL FINDINGS (structured output from a registered tool)",
            Self::GraphRelated { .. } => {
                "RELATED FILES (reached through the project graph, not requested by anyone)"
            }
        }
    }
}

/// One piece of context pinned to a turn by a person or by the project.
#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct PinnedSection {
    pub origin: PinnedOrigin,
    /// What the user or project calls this: `@src/auth.rs`, `AGENTS.md`,
    /// `architecture`.
    pub label: String,
    /// The bounded content itself. Callers truncate before constructing this;
    /// the runtime does not silently shrink it, because a section that claims
    /// to be a file and is half a file is the same lie as a chip that claims
    /// to be attached and is not.
    pub content: String,
    /// A memory entry's id, so the daemon can record that it was actually used.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_id: Option<String>,
}

/// The context a turn had pinned to it, in the order it is presented.
///
/// This is the channel that makes `@file` and project memory real: a section
/// here is assembled into the model request and accounted for in the turn's
/// [`ContextLedgerEntry`] with [`WhyIncluded::Pinned`]. Anything that shows a
/// user an "attached" affordance must put content through here, or the
/// affordance is describing something that did not happen.
#[derive(Clone, Debug, Default, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct PinnedContext {
    pub sections: Vec<PinnedSection>,
}

impl PinnedContext {
    pub fn is_empty(&self) -> bool {
        self.sections.is_empty()
    }

    /// Append sections after the caller's own, preserving their order.
    ///
    /// Used to fold projections — tool findings recovered from the evidence log
    /// — into the caller-supplied pinned set. Deliberately does NOT sort:
    /// the caller's order is meaningful (a user's composer references come
    /// before standing project memory), and `render_parts` emits a heading
    /// whenever the origin changes, so appending groups the new sections
    /// without disturbing what was already there.
    pub fn with_sections(mut self, sections: Vec<PinnedSection>) -> Self {
        self.sections.extend(sections);
        self
    }

    /// The pinned block split into one `(ledger_label, text)` pair per pinned
    /// section, grouped by origin with each group's heading emitted once (on
    /// the first section of the group, so every byte belongs to exactly one
    /// pair).
    ///
    /// This is the single source of truth for both the prompt text and the
    /// ledger: concatenating every `text` yields exactly [`Self::render`], so
    /// the per-section token accounting can never drift from what the model
    /// actually saw — the invariant the context ledger is built on.
    pub fn render_parts(&self) -> Vec<(String, String)> {
        let mut parts = Vec::with_capacity(self.sections.len());
        let mut current: Option<&PinnedOrigin> = None;
        for section in &self.sections {
            let mut text = String::new();
            if current != Some(&section.origin) {
                text.push_str(&format!("## {}\n", section.origin.heading()));
                current = Some(&section.origin);
            }
            text.push_str(&format!("### {}\n{}\n\n", section.label, section.content));
            parts.push((
                format!("{}/{}", section.origin.label_prefix(), section.label),
                text,
            ));
        }
        parts
    }

    /// The whole pinned block as it appears in the prompt. Empty when nothing
    /// is pinned, so the turn carries no empty heading for the model to
    /// interpret.
    pub fn render(&self) -> String {
        self.render_parts()
            .into_iter()
            .map(|(_, text)| text)
            .collect()
    }

    /// The ids of the memory entries pinned into this turn, for recording that
    /// they were actually used rather than merely stored.
    pub fn used_memory_ids(&self) -> Vec<String> {
        self.sections
            .iter()
            .filter(|section| section.origin == PinnedOrigin::ProjectMemory)
            .filter_map(|section| section.memory_id.clone())
            .collect()
    }
}

/// How many of the most recent [`ContextLedgerEntry`] values `SessionState`
/// keeps in memory for the inspector endpoint.
///
/// This is inspector data, not model-facing context — it is bounded
/// independently of Phase 2's compaction, and every entry remains durably
/// replayable from the NineLives event log regardless of this cap.
pub const MAX_RECENT_CONTEXT_LEDGER_ENTRIES: usize = 64;

/// Tokens NativeAgent reserves for model output when computing how much of
/// the context window a turn's prompt may fill (see
/// NativeAgent::effective_input_capacity in agent-runtime). Shared here so
/// the daemon's presentation layer can compute the same "effective capacity"
/// number it shows the user without duplicating the literal.
pub const RESERVED_OUTPUT_TOKENS: u64 = 8192;

// ── Semantic checkpoint types (PRD v1.1 §7.2, Phase 2) ──────────────────

#[derive(
    Clone, Copy, Debug, Eq, Hash, JsonSchema, Ord, PartialEq, PartialOrd, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct CheckpointId(pub Uuid);

impl CheckpointId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for CheckpointId {
    fn default() -> Self {
        Self::new()
    }
}

/// A structured, additive snapshot of the agent's state at compaction.
///
/// Unlike v1.0's flat `context_summary: Option<String>` (overwritten on every
/// compaction), this is chained via `superseded_checkpoint_id` and merged
/// additively by the reducer — `failed_attempts` never fall out of the prompt.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SemanticCheckpoint {
    pub checkpoint_id: CheckpointId,
    pub turn_id: TurnId,
    pub superseded_checkpoint_id: Option<CheckpointId>,
    pub objective: String,
    pub accepted_requirements: Vec<String>,
    pub user_constraints: Vec<String>,
    pub decisions: Vec<CheckpointDecision>,
    pub files_inspected: Vec<PathBuf>,
    pub files_modified: Vec<PathBuf>,
    pub important_symbols: Vec<String>,
    pub validated_facts: Vec<String>,
    /// Must survive every subsequent compaction — the single most important
    /// behavioral change in this phase (§7.5).
    pub failed_attempts: Vec<FailedAttempt>,
    pub test_results: Vec<TestResultSummary>,
    pub unresolved_questions: Vec<String>,
    pub current_hypothesis: Option<String>,
    pub next_actions: Vec<String>,
    pub pinned_context: Vec<PinnedContextRef>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct FailedAttempt {
    pub action_id: ActionId,
    pub action_summary: String,
    pub reason: String,
    pub judgment: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct CheckpointDecision {
    pub summary: String,
    pub action_id: Option<ActionId>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TestResultSummary {
    pub label: String,
    pub passed: u32,
    pub failed: u32,
    pub skipped: u32,
}

/// A reference to context the user pinned in the IDE composer (Phase 5).
///
/// Defined here alongside `SemanticCheckpoint` so the checkpoint can carry
/// pinned-context references before Phase 5's full UI ships; the IDE's chip
/// rendering reads the same `ContextClass`/`WhyIncluded` enum Phase 1 defined.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PinnedContextRef {
    pub label: String,
    pub class: ContextClass,
    pub why_included: WhyIncluded,
    pub estimated_tokens: u64,
}

// ── Session lifecycle / pause constants ────────────────────────────────
///
/// A client has to tell this pause apart from a pause in the middle of the
/// work: one is asking to be read and will take feedback, the other is
/// reporting a problem to fix. The agent writes these reasons and the clients
/// match on them, so the wording lives here instead of being spelled out in
/// three places and drifting.
pub const PLAN_REVIEW_PAUSE: &str = "plan is ready for review";

/// Durable prefix used after an interrupted turn has been reconciled against
/// its isolated worktree. Clients use the resulting state to offer an explicit
/// resume action without automatically replaying an uncertain effect.
pub const RECOVERY_RECONCILED_PAUSE: &str =
    "Recovery reconciled the durable log with the isolated worktree:";

/// True when a [`SessionEvent::SessionPaused`] reason is a plan awaiting review.
pub fn is_plan_review_pause(reason: &str) -> bool {
    reason.ends_with(PLAN_REVIEW_PAUSE)
}

#[derive(Clone, Debug, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", content = "data", rename_all = "snake_case")]
pub enum SessionEvent {
    SessionCreated {
        objective: String,
        repository: PathBuf,
        /// The permission mode an authenticated human chose for this session
        /// (PRD §12). Durable because an authority decision that only lives in
        /// the client that made it cannot be audited afterwards. Defaults to
        /// `Governed` so sessions recorded before v0.9 still load.
        #[serde(default)]
        authority_mode: AuthorityMode,
    },
    /// Human-selected adaptive controls are durable session state, not client
    /// preferences. This keeps TUI, IDE, and CLI attached to one decision.
    #[serde(alias = "session_controls_updated")]
    SessionControlsUpdated {
        controls: adaptation::SessionControls,
    },
    /// The classifier's explainable decision and bounded lane graph.
    WorkflowPlanCreated {
        decision: adaptation::ComplexityDecision,
        plan: adaptation::WorkflowPlan,
    },
    /// Provider usage is recorded as evidence; credentials remain references,
    /// never raw secrets.
    UsageRecorded {
        record: adaptation::UsageRecord,
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
    /// What the user asked for, compiled into a checkable contract (v1.5 §3).
    ExpectationContractCreated {
        contract: Box<expectation::ExpectationContract>,
    },
    /// A user correction, recorded as the correction itself rather than as the
    /// contract it produced (v1.5 §5).
    ///
    /// Storing the delta and re-applying it on replay means the reducer reaches
    /// the same contract the live runtime did, by the same rules — including
    /// resetting the status of work the correction invalidated. Storing the
    /// resulting contract instead would let a buggy writer persist a state the
    /// rules forbid, and replay would faithfully restore it.
    ExpectationContractRevised {
        revision: Box<expectation::ContractRevision>,
    },
    /// A requirement's status changed, with what changed it (v1.5 §8).
    RequirementStatusChanged {
        requirement_id: work::RequirementId,
        status: expectation::RequirementStatus,
        /// Which review or check moved it, for the requirement → evidence trace
        /// the user can open (v1.5 §22).
        source: String,
    },
    /// A review began, with what it was allowed to read (v1.5 §9).
    ReviewStarted {
        record: Box<review::ReviewRecord>,
    },
    /// One reviewer claim (v1.5 §27).
    ReviewFindingRecorded {
        finding: Box<review::ReviewFinding>,
    },
    ReviewCompleted {
        review: review::ReviewId,
    },
    /// A bounded automatic repair attempt began (v1.5 §11).
    CorrectionStarted {
        cycle: u32,
        findings: Vec<review::FindingId>,
    },
    /// A repair attempt finished, with what a *re-review* confirmed — not what
    /// the repair agent claimed.
    CorrectionCompleted {
        cycle: u32,
        repaired: Vec<review::FindingId>,
        still_open: Vec<review::FindingId>,
    },
    /// The delivery gate ran (v1.5 §12).
    DeliveryGateEvaluated {
        assessment: Box<expectation::DeliveryAssessment>,
    },
    /// A durable, reviewable statement of intent. Direct sessions may omit it;
    /// Standard and Rigorous sessions use it as the source for their task graph.
    SpecBundleRecorded {
        bundle: work::SpecBundle,
        reason: String,
    },
    /// The executable work graph derived from the accepted spec revision.
    TaskGraphRecorded {
        graph: work::TaskGraph,
        reason: String,
    },
    /// A single task transition. The reducer independently verifies that the
    /// transition is legal and releases dependants after a passing task.
    TaskStatusChanged {
        task_id: work::WorkTaskId,
        status: work::WorkTaskStatus,
        reason: String,
    },
    /// Evidence tied to an exact requirement, criterion and task.
    EvidenceLinked {
        evidence: work::EvidenceLink,
    },
    ContextCompacted {
        summary: String,
        retained_action_ids: Vec<ActionId>,
    },
    /// One turn's context-assembly accounting (PRD v1.1 §6.2, Phase 1).
    /// Purely additive observability: appended to the bounded
    /// `SessionState.recent_context_ledger`, never consulted by PawGate or
    /// Claw, and replays through the identical `append`/`reduce_event` path
    /// as every other `SessionEvent`.
    ContextAssembled {
        entry: ContextLedgerEntry,
    },
    /// A semantic checkpoint replacing the flat `context_summary: Option<String>`
    /// (PRD v1.1 §7.2, Phase 2). The reducer merges fields additively across
    /// the `superseded_checkpoint_id` chain — `failed_attempts` are unioned,
    /// never dropped — and truncates `conversation_messages` to the window
    /// starting at `conversation_messages_retained_from`.
    CheckpointCompacted {
        checkpoint: Box<SemanticCheckpoint>,
        retained_action_ids: BTreeSet<ActionId>,
        conversation_messages_retained_from: usize,
    },
    SessionPaused {
        reason: String,
    },
    SessionResumed,
    ModelSelected {
        model: String,
    },
    /// The named agent profile this session runs under (v1.3 PR C). Recorded
    /// durably on session creation so resume/continue/approve rebind the same
    /// profile without the client re-supplying it. The name resolves against
    /// the repository's extension set; an unknown name is rejected before the
    /// session starts.
    AgentBound {
        agent: String,
    },
    SupervisorStarted {
        workers: usize,
    },
    WorkerStarted {
        worker_id: String,
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
    /// A read-only Scout subagent began exploring the repository.
    ///
    /// Recorded so a running Scout is visible while it runs. Without a Started
    /// event, the only trace of a subagent is the record of it having finished,
    /// which means the agent workspace can show work only after it is too late
    /// to watch — the same gap `WorkerStarted` was added to close for
    /// supervisor workers.
    ScoutStarted {
        scout_id: String,
        parent_turn_id: TurnId,
    },
    /// A read-only Scout subagent completed its repository exploration and
    /// returned structured evidence (PRD v1.1 §Phase 5, P0-7).
    ScoutCompleted {
        scout_id: String,
        parent_turn_id: TurnId,
        evidence_count: u32,
        conclusions: Vec<String>,
        confidence: String,
    },
    /// A Scout subagent failed — its findings are not available but the main
    /// agent loop continues without them.
    ScoutFailed {
        reason: String,
        /// Which Scout failed, so the workspace can close the entry its
        /// `ScoutStarted` opened rather than leaving it running forever.
        /// Defaulted for logs written before Scouts were identified here.
        #[serde(default)]
        scout_id: Option<String>,
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
        /// The turn that produced this action (PRD v1.1 §6.3). `None` for
        /// events recorded before Phase 1 shipped, or for action proposals
        /// that do not originate from `run_until_pause`'s main loop (e.g.
        /// validation-repair specialists, MCP tool invocations).
        #[serde(default)]
        turn_id: Option<TurnId>,
    },
    ActionSuperseded {
        previous_action_id: ActionId,
        replacement_action_id: ActionId,
        reason: String,
    },
    JudgmentRecorded {
        action_id: ActionId,
        decision: JudgmentDecision,
        /// The turn that produced the judged action; see `ActionProposed`.
        #[serde(default)]
        turn_id: Option<TurnId>,
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
        /// The turn that produced the executed action; see `ActionProposed`.
        #[serde(default)]
        turn_id: Option<TurnId>,
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
    /// A restorable checkpoint was reverse-applied to the isolated worktree.
    /// Audit-only, mirroring `CheckpointCreated`.
    CheckpointRestored {
        checkpoint_id: String,
        head: String,
        patch_digest: String,
    },
    /// A session was forked from a parent at a conversation anchor. Audit-only;
    /// the child session's `SessionCreated` carries the `parent_id`.
    SessionForked {
        parent_id: String,
        anchor_message_id: String,
    },
    WorktreeDispositionRecorded {
        strategy: String,
        detail: String,
    },
    /// An external process (not the agent) changed files in the session
    /// worktree while the session was not executing. This is how the backend
    /// surfaces "someone edited a file outside PurrCode" so clients can stop
    /// polling and refresh on change. Audit-only.
    ExternalChangeDetected {
        changed_files: Vec<PathBuf>,
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
    // ── Terminal actions ──────────────────────────────────
    //
    // Deliberately separate from `ActionProposed`/`JudgmentRecorded`: those
    // events drive the single-slot `SessionStatus::AwaitingApproval`, which
    // represents the primary agent loop's one outstanding boundary. A
    // terminal (a build tab, a test tab, a human shell) is not the primary
    // loop, and several can be pending at once, so terminal approval state
    // lives in its own maps and never touches `status`.
    TerminalActionProposed {
        action_id: ActionId,
        action: TerminalAction,
    },
    TerminalJudgmentRecorded {
        action_id: ActionId,
        decision: JudgmentDecision,
    },
    /// A completion was rejected because it was only a progress report and was
    /// sent back to the provider for a real answer. Durable so the meta-
    /// completion failure rate is measurable rather than anecdotal (PRD §2.3
    /// FR-B5).
    CompletionRepairRecorded {
        /// Which completion-repair attempt this was (1-based).
        attempt: u8,
        reason: String,
    },
    // ── v1.3 extension-platform events (appended at the end so logs written
    //    by v1.2 still deserialize) ───────────────────────────────────────
    /// One authorized tool invocation, recorded as durable evidence. The
    /// canonical projection of [`crate::ExecutionEvidence`] into the event log.
    ToolEvidenceRecorded {
        evidence: Box<ExecutionEvidence>,
    },
    /// A governed hook fired (or was denied before it could). Recorded BEFORE
    /// the action is proposed, so a triggered-but-denied hook is
    /// distinguishable from one that never fired.
    HookTriggered {
        hook_id: String,
        trigger: HookTrigger,
        action_id: Option<ActionId>,
    },
    /// A lifecycle hook needs human approval, so the action that triggered it
    /// is parked rather than executed or failed.
    ///
    /// This is what makes a hook suspension resumable instead of terminal. The
    /// session moves to `AwaitingApproval(hook_action_id)`; when that approval
    /// lands, `action_id` is the work that was waiting on it and
    /// `completed_hooks` is the part of the chain that must not fire a second
    /// time (approving a hook must not re-ask for the same approval).
    ActionDeferredForHook {
        action_id: ActionId,
        hook_action_id: ActionId,
        trigger: HookTrigger,
        completed_hooks: Vec<String>,
    },
    /// A deferred action resumed after its hook approval was granted. Paired
    /// with `ActionDeferredForHook` so the audit trail shows the pause and the
    /// continuation, and so a replayed approval cannot resume it twice.
    ActionResumedAfterHook {
        action_id: ActionId,
        hook_action_id: ActionId,
    },
    // ── v1.4 collaborative-agent events (appended at the end so logs written
    //    by v1.3 still deserialize) ────────────────────────────────────────
    //
    // These are named `Delegation*` rather than the bare `Worker*` of the v1.4
    // PRD because `WorkerStarted`/`WorkerFinished` already exist above for the
    // v1.2 supervisor. Two different things called the same name in one log is
    // how an audit trail starts lying.
    /// The classifier's explainable decision about whether to delegate at all.
    /// Recorded even when the answer is "no": *not* delegating is a decision the
    /// user is entitled to see the reasoning for.
    DelegationPlanned {
        plan: Box<delegation::DelegationPlan>,
    },
    /// One admitted delegation. The payload carries its effective authority, so
    /// replay reconstructs what the worker was allowed to do without consulting
    /// any policy file that may since have changed.
    DelegationCreated {
        delegation: Box<delegation::Delegation>,
    },
    /// Which specialist the capability registry chose, and why.
    DelegationRoutingRecorded {
        delegation_id: delegation::DelegationId,
        decision: delegation::RoutingDecision,
    },
    /// Dependencies satisfied; the delegation may be assigned a worker.
    DelegationReady {
        delegation_id: delegation::DelegationId,
    },
    /// A dependency failed or was cancelled, so this delegation will not run.
    DelegationBlocked {
        delegation_id: delegation::DelegationId,
        blocking_dependency: delegation::DelegationId,
    },
    DelegationWorkerAssigned {
        assignment: Box<delegation::WorkerAssignment>,
    },
    DelegationWorkerStarted {
        delegation_id: delegation::DelegationId,
        worker_id: delegation::WorkerId,
    },
    /// The worker stopped mid-flight without a result — a daemon restart, or a
    /// user pause. Distinct from failure: its worktree and partial patch are
    /// preserved.
    DelegationWorkerPaused {
        delegation_id: delegation::DelegationId,
        worker_id: delegation::WorkerId,
        reason: String,
    },
    DelegationWorkerCompleted {
        delegation_id: delegation::DelegationId,
        worker_id: delegation::WorkerId,
    },
    DelegationWorkerFailed {
        delegation_id: delegation::DelegationId,
        worker_id: delegation::WorkerId,
        reason: String,
    },
    DelegationWorkerCancelled {
        delegation_id: delegation::DelegationId,
        worker_id: delegation::WorkerId,
        reason: String,
    },
    /// The structured handoff. This — never a raw assistant paragraph — is what
    /// the parent integrates from.
    DelegationResultRecorded {
        result: Box<delegation::WorkerResult>,
    },
    /// A bounded repair cycle was routed back to the responsible worker.
    DelegationRepairRequested {
        delegation_id: delegation::DelegationId,
        cycle: u8,
        reason: String,
    },
    IntegrationProposed {
        proposal: Box<delegation::IntegrationProposal>,
    },
    IntegrationConflictDetected {
        delegation_id: delegation::DelegationId,
        conflicts: Vec<delegation::IntegrationConflict>,
    },
    IntegrationApproved {
        delegation_id: delegation::DelegationId,
        /// The digest of what was approved — the amended one when the user
        /// selected a subset of hunks.
        patch_digest: String,
        authority: ApprovalAuthority,
    },
    IntegrationRejected {
        delegation_id: delegation::DelegationId,
        reason: String,
    },
    IntegrationApplied {
        delegation_id: delegation::DelegationId,
        patch_digest: String,
        changed_paths: Vec<PathBuf>,
    },
    DelegationCompleted {
        delegation_id: delegation::DelegationId,
    },
    DelegationCancelled {
        delegation_id: delegation::DelegationId,
        reason: String,
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
    /// The most recent checkpoint from compaction (PRD v1.1 §7.3, Phase 2).
    /// Replaces the flat `context_summary` string that was overwritten on every
    /// compaction; this chains additive merges so `failed_attempts` survive.
    pub checkpoint: Option<SemanticCheckpoint>,
    pub selected_model: Option<String>,
    /// The named agent profile bound to this session (v1.3 PR C). Mirrors
    /// `selected_model`: durable binding metadata, not replay state.
    pub selected_agent: Option<String>,
    pub controls: adaptation::SessionControls,
    pub complexity_decision: Option<adaptation::ComplexityDecision>,
    pub workflow_plan: Option<adaptation::WorkflowPlan>,
    pub spec_bundle: Option<work::SpecBundle>,
    pub task_graph: Option<work::TaskGraph>,
    pub evidence_links: Vec<work::EvidenceLink>,
    pub usage_records: Vec<adaptation::UsageRecord>,
    pub conversation_messages: Vec<ConversationMessage>,
    pub proposed_actions: BTreeMap<ActionId, ProposedAction>,
    pub judgments: BTreeMap<ActionId, JudgmentDecision>,
    pub contextual_judgments: BTreeMap<ActionId, ContextualJudgment>,
    pub proposed_terminal_actions: BTreeMap<ActionId, TerminalAction>,
    pub terminal_judgments: BTreeMap<ActionId, JudgmentDecision>,
    /// The most recent [`ContextLedgerEntry`] values, newest at the back,
    /// bounded by [`MAX_RECENT_CONTEXT_LEDGER_ENTRIES`] (PRD v1.1 §6.3).
    /// Inspector data only — every entry is also durably replayable from the
    /// full NineLives event log regardless of this in-memory cap.
    pub recent_context_ledger: VecDeque<ContextLedgerEntry>,
    /// The delegation classifier's most recent decision, including a decision
    /// *not* to delegate (v1.4 §PR2).
    pub delegation_plan: Option<delegation::DelegationPlan>,
    /// Every delegation this session created, with its worker, result and
    /// integration state (v1.4 §PR10). Rebuilt by replay, so a daemon restart
    /// knows exactly which workers finished and must not be rerun.
    pub delegations: BTreeMap<delegation::DelegationId, delegation::DelegationRecord>,
    /// Running totals across the whole delegation tree (v1.4 §PR14).
    pub delegation_ledger: delegation::DelegationLedger,
    /// What the user actually asked for, at its current revision (v1.5 §3).
    ///
    /// Rebuilt by replay like everything else here, which is the point: a
    /// daemon restart must not lose the requirements, because an agent that
    /// resumes without them resumes without knowing what it is for.
    pub expectation_contract: Option<expectation::ExpectationContract>,
    /// Every review this session ran (v1.5 §7–§10).
    pub reviews: BTreeMap<review::ReviewId, review::ReviewRecord>,
    /// Every finding, keyed so the correction loop can mark them repaired.
    pub findings: BTreeMap<review::FindingId, review::ReviewFinding>,
    /// What automatic correction has cost and achieved (v1.5 §11).
    pub correction_ledger: correction::CorrectionLedger,
    /// The most recent delivery-gate result (v1.5 §12).
    pub delivery: Option<expectation::DeliveryAssessment>,
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
            checkpoint: None,
            selected_model: None,
            selected_agent: None,
            controls: adaptation::SessionControls::default(),
            complexity_decision: None,
            workflow_plan: None,
            spec_bundle: None,
            task_graph: None,
            evidence_links: Vec::new(),
            usage_records: Vec::new(),
            conversation_messages: Vec::new(),
            proposed_actions: BTreeMap::new(),
            judgments: BTreeMap::new(),
            contextual_judgments: BTreeMap::new(),
            proposed_terminal_actions: BTreeMap::new(),
            terminal_judgments: BTreeMap::new(),
            recent_context_ledger: VecDeque::new(),
            delegation_plan: None,
            delegations: BTreeMap::new(),
            delegation_ledger: delegation::DelegationLedger::default(),
            expectation_contract: None,
            reviews: BTreeMap::new(),
            findings: BTreeMap::new(),
            correction_ledger: correction::CorrectionLedger::default(),
            delivery: None,
        }
    }

    /// Findings that are still open: raised, and not since confirmed repaired.
    ///
    /// The correction loop and the delivery gate must both read *this* rather
    /// than `findings`, which is the full history and never shrinks. Counting a
    /// repaired finding as outstanding spends the whole correction budget
    /// re-fixing something already fixed, and then hands the user a task in
    /// `NeedsAttention` with nothing actually wrong with it.
    pub fn outstanding_findings(&self) -> Vec<&review::ReviewFinding> {
        self.findings
            .values()
            .filter(|finding| !self.correction_ledger.repaired.contains(&finding.id))
            .collect()
    }

    /// Open findings that are holding delivery.
    pub fn blocking_findings(&self) -> Vec<&review::ReviewFinding> {
        self.outstanding_findings()
            .into_iter()
            .filter(|finding| finding.blocks_delivery())
            .collect()
    }

    /// Delegations that are still live, in creation order. The scheduler and
    /// the agent workspace both read this rather than tracking their own.
    pub fn live_delegations(&self) -> impl Iterator<Item = &delegation::DelegationRecord> {
        self.delegations
            .values()
            .filter(|record| record.status().is_live())
    }

    /// How many workers are executing right now. Governance clamps against
    /// this, so a restart cannot lose count and over-spawn.
    pub fn running_worker_count(&self) -> u32 {
        self.delegations
            .values()
            .filter(|record| record.is_running())
            .count() as u32
    }

    /// Delegations whose changes are waiting on a human decision.
    pub fn delegations_awaiting_decision(
        &self,
    ) -> impl Iterator<Item = &delegation::DelegationRecord> {
        self.delegations
            .values()
            .filter(|record| record.awaits_decision())
    }

    /// Authoritative state reducer.
    ///
    /// Validates the transition before applying the event. Returns
    /// [`DomainError::InvalidStateTransition`] when the current status forbids
    /// the requested change, [`DomainError::DuplicateEvent`] when the event has
    /// already been applied, and [`DomainError::UnexpectedApproval`] when an
    /// approval references an action that is not awaiting approval.
    ///
    /// `event_count` is incremented **after** all validation succeeds so that
    /// derived state is never mutated by an invalid transition.
    pub fn reduce_event(&mut self, event: &SessionEvent) -> Result<(), DomainError> {
        self.validate_event(event)?;
        self.apply_event(event);
        self.event_count += 1;
        Ok(())
    }

    fn validate_event(&self, event: &SessionEvent) -> Result<(), DomainError> {
        use SessionStatus::*;
        match event {
            SessionEvent::SessionCreated { .. } => {
                if self.event_count > 0 {
                    return Err(DomainError::DuplicateEvent {
                        session: self.id,
                        event: format!("{event:?}"),
                    });
                }
            }
            SessionEvent::ActionProposed { action_id, .. } => {
                if self.proposed_actions.contains_key(action_id) {
                    return Err(DomainError::DuplicateEvent {
                        session: self.id,
                        event: format!("{event:?}"),
                    });
                }
            }
            SessionEvent::TerminalActionProposed { action_id, .. } => {
                if self.proposed_terminal_actions.contains_key(action_id) {
                    return Err(DomainError::DuplicateEvent {
                        session: self.id,
                        event: format!("{event:?}"),
                    });
                }
            }
            SessionEvent::TerminalJudgmentRecorded { action_id, .. } => {
                if !self.proposed_terminal_actions.contains_key(action_id) {
                    return Err(DomainError::InvalidStateTransition {
                        session: self.id,
                        event: format!("{event:?}"),
                        reason: "terminal judgment recorded for unknown action".into(),
                    });
                }
            }
            SessionEvent::ExpectationContractCreated { contract } => {
                contract
                    .validate()
                    .map_err(|error| DomainError::InvalidStateTransition {
                        session: self.id,
                        event: format!("{event:?}"),
                        reason: error.to_string(),
                    })?;
                // A second contract would silently replace the first and take
                // its revision history with it. Corrections are how intent
                // changes; there is no other door.
                if self.expectation_contract.is_some() {
                    return Err(DomainError::InvalidStateTransition {
                        session: self.id,
                        event: format!("{event:?}"),
                        reason: "this session already has an expectation contract; \
                                 record a revision instead"
                            .into(),
                    });
                }
            }
            SessionEvent::ExpectationContractRevised { revision } => {
                let current = self.expectation_contract.as_ref().ok_or_else(|| {
                    DomainError::InvalidStateTransition {
                        session: self.id,
                        event: format!("{event:?}"),
                        reason: "cannot revise an expectation contract that was never created"
                            .into(),
                    }
                })?;
                // Rehearse the correction here so an illegal one is refused at
                // the boundary rather than corrupting state on replay.
                current.revise((**revision).clone()).map_err(|error| {
                    DomainError::InvalidStateTransition {
                        session: self.id,
                        event: format!("{event:?}"),
                        reason: error.to_string(),
                    }
                })?;
            }
            SessionEvent::RequirementStatusChanged {
                requirement_id,
                status,
                source,
            } => {
                require_event_reason(self.id, event, source)?;
                let contract = self.expectation_contract.as_ref().ok_or_else(|| {
                    DomainError::InvalidStateTransition {
                        session: self.id,
                        event: format!("{event:?}"),
                        reason: "no expectation contract holds this requirement".into(),
                    }
                })?;
                if contract.clause(*requirement_id).is_none() {
                    return Err(DomainError::InvalidStateTransition {
                        session: self.id,
                        event: format!("{event:?}"),
                        reason: format!("requirement {requirement_id:?} is not in the contract"),
                    });
                }
                // The same two rules the contract enforces structurally, applied
                // to the transition itself: a status is a claim, and these two
                // claims need something behind them.
                let flaw = match status {
                    expectation::RequirementStatus::Verified { evidence }
                        if evidence.is_empty() =>
                    {
                        Some("a requirement cannot be verified with no evidence")
                    }
                    expectation::RequirementStatus::Waived { reason, .. }
                        if reason.trim().is_empty() =>
                    {
                        Some("a waiver must say why")
                    }
                    _ => None,
                };
                if let Some(flaw) = flaw {
                    return Err(DomainError::InvalidStateTransition {
                        session: self.id,
                        event: format!("{event:?}"),
                        reason: flaw.into(),
                    });
                }
            }
            SessionEvent::ReviewStarted { record } => {
                // Refuses a "fresh-context" review that was handed the
                // implementer's transcript. The failure is silent otherwise:
                // the review still produces confident findings, it just mostly
                // agrees with the work it was shown.
                record
                    .validate()
                    .map_err(|error| DomainError::InvalidStateTransition {
                        session: self.id,
                        event: format!("{event:?}"),
                        reason: error.to_string(),
                    })?;
                if self.reviews.contains_key(&record.id) {
                    return Err(DomainError::InvalidStateTransition {
                        session: self.id,
                        event: format!("{event:?}"),
                        reason: format!("review {:?} already started", record.id),
                    });
                }
            }
            SessionEvent::ReviewFindingRecorded { finding } => {
                finding
                    .validate()
                    .map_err(|error| DomainError::InvalidStateTransition {
                        session: self.id,
                        event: format!("{event:?}"),
                        reason: error.to_string(),
                    })?;
                if !self.reviews.contains_key(&finding.review) {
                    return Err(DomainError::InvalidStateTransition {
                        session: self.id,
                        event: format!("{event:?}"),
                        reason: "a finding must belong to a review that started".into(),
                    });
                }
                if let Some(requirement) = finding.requirement_id
                    && self
                        .expectation_contract
                        .as_ref()
                        .is_none_or(|contract| contract.clause(requirement).is_none())
                {
                    return Err(DomainError::InvalidStateTransition {
                        session: self.id,
                        event: format!("{event:?}"),
                        reason: format!(
                            "finding names requirement {requirement:?}, which is not in the contract"
                        ),
                    });
                }
            }
            SessionEvent::ReviewCompleted { review } => {
                if !self.reviews.contains_key(review) {
                    return Err(DomainError::InvalidStateTransition {
                        session: self.id,
                        event: format!("{event:?}"),
                        reason: format!("review {review:?} never started"),
                    });
                }
            }
            SessionEvent::CorrectionStarted { cycle, .. } => {
                // The budget is the whole point of the loop being bounded; a
                // cycle that skips ahead would spend it without recording it.
                let expected = self.correction_ledger.cycles_used + 1;
                if *cycle != expected {
                    return Err(DomainError::InvalidStateTransition {
                        session: self.id,
                        event: format!("{event:?}"),
                        reason: format!("correction cycle {cycle} does not follow {expected}"),
                    });
                }
            }
            SessionEvent::CorrectionCompleted { cycle, .. } => {
                let expected = self.correction_ledger.cycles_used + 1;
                if *cycle != expected {
                    return Err(DomainError::InvalidStateTransition {
                        session: self.id,
                        event: format!("{event:?}"),
                        reason: format!("correction cycle {cycle} was not the one running"),
                    });
                }
            }
            SessionEvent::DeliveryGateEvaluated { assessment } => {
                // The one invariant that makes completion a state transition
                // rather than a claim: `Ready` with outstanding blockers is not
                // an optimistic assessment, it is a false one, and the log will
                // not hold it however it was produced.
                if assessment.state == expectation::DeliveryState::Ready
                    && !assessment.blockers.is_empty()
                {
                    return Err(DomainError::InvalidStateTransition {
                        session: self.id,
                        event: format!("{event:?}"),
                        reason: format!(
                            "delivery cannot be ready with {} outstanding blocker(s)",
                            assessment.blockers.len()
                        ),
                    });
                }
            }
            SessionEvent::SpecBundleRecorded { bundle, reason } => {
                require_event_reason(self.id, event, reason)?;
                bundle
                    .validate()
                    .map_err(|error| DomainError::InvalidStateTransition {
                        session: self.id,
                        event: format!("{event:?}"),
                        reason: error.to_string(),
                    })?;
                if self
                    .spec_bundle
                    .as_ref()
                    .is_some_and(|current| bundle.revision <= current.revision)
                {
                    return Err(DomainError::InvalidStateTransition {
                        session: self.id,
                        event: format!("{event:?}"),
                        reason: "spec revision must increase".into(),
                    });
                }
            }
            SessionEvent::TaskGraphRecorded { graph, reason } => {
                require_event_reason(self.id, event, reason)?;
                graph.validate(self.spec_bundle.as_ref()).map_err(|error| {
                    DomainError::InvalidStateTransition {
                        session: self.id,
                        event: format!("{event:?}"),
                        reason: error.to_string(),
                    }
                })?;
                if self
                    .task_graph
                    .as_ref()
                    .is_some_and(|current| graph.revision <= current.revision)
                {
                    return Err(DomainError::InvalidStateTransition {
                        session: self.id,
                        event: format!("{event:?}"),
                        reason: "task graph revision must increase".into(),
                    });
                }
            }
            SessionEvent::TaskStatusChanged {
                task_id,
                status,
                reason,
            } => {
                require_event_reason(self.id, event, reason)?;
                if *status == work::WorkTaskStatus::Passed {
                    let task = self
                        .task_graph
                        .as_ref()
                        .and_then(|graph| graph.task(*task_id))
                        .ok_or_else(|| DomainError::InvalidStateTransition {
                            session: self.id,
                            event: format!("{event:?}"),
                            reason: "passing task does not exist in the recorded graph".into(),
                        })?;
                    let uncovered = task.acceptance_criteria.iter().find(|criterion| {
                        !self.evidence_links.iter().any(|evidence| {
                            evidence.task_id == *task_id
                                && evidence.criterion_id == **criterion
                                && matches!(
                                    evidence.coverage,
                                    work::EvidenceCoverage::Covered
                                        | work::EvidenceCoverage::AcceptedException
                                )
                        })
                    });
                    if let Some(criterion) = uncovered {
                        return Err(DomainError::InvalidStateTransition {
                            session: self.id,
                            event: format!("{event:?}"),
                            reason: format!(
                                "task cannot pass before criterion {criterion:?} has closing evidence"
                            ),
                        });
                    }
                }
                let mut graph =
                    self.task_graph
                        .clone()
                        .ok_or_else(|| DomainError::InvalidStateTransition {
                            session: self.id,
                            event: format!("{event:?}"),
                            reason: "task transition requires a recorded graph".into(),
                        })?;
                graph.transition(*task_id, *status).map_err(|error| {
                    DomainError::InvalidStateTransition {
                        session: self.id,
                        event: format!("{event:?}"),
                        reason: error.to_string(),
                    }
                })?;
            }
            SessionEvent::EvidenceLinked { evidence } => {
                let (Some(spec), Some(graph)) =
                    (self.spec_bundle.as_ref(), self.task_graph.as_ref())
                else {
                    return Err(DomainError::InvalidStateTransition {
                        session: self.id,
                        event: format!("{event:?}"),
                        reason: "evidence requires a recorded spec and task graph".into(),
                    });
                };
                evidence.validate(spec, graph).map_err(|error| {
                    DomainError::InvalidStateTransition {
                        session: self.id,
                        event: format!("{event:?}"),
                        reason: error.to_string(),
                    }
                })?;
                if self
                    .evidence_links
                    .iter()
                    .any(|existing| existing.id == evidence.id)
                {
                    return Err(DomainError::DuplicateEvent {
                        session: self.id,
                        event: format!("{event:?}"),
                    });
                }
            }
            SessionEvent::SessionPaused { .. } => {
                self.require_transition(Paused, "session paused", event)?;
            }
            SessionEvent::SessionResumed => {
                self.require_transition(Active, "session resumed", event)?;
            }
            SessionEvent::JudgmentRecorded {
                action_id,
                decision,
                ..
            } => {
                if !self.proposed_actions.contains_key(action_id) {
                    return Err(DomainError::InvalidStateTransition {
                        session: self.id,
                        event: format!("{event:?}"),
                        reason: "judgment recorded for unknown action".into(),
                    });
                }
                if matches!(decision, JudgmentDecision::RequireApproval { .. }) {
                    self.require_transition(
                        AwaitingApproval(*action_id),
                        "judgment required human approval",
                        event,
                    )?;
                }
            }
            SessionEvent::OutcomeReviewRequired { .. } => {
                self.require_transition(AwaitingReview, "outcome review required", event)?;
            }
            SessionEvent::OutcomeReviewApproved { .. } => {
                self.require_transition(Active, "outcome review approved", event)?;
            }
            SessionEvent::ApprovalRecorded { action_id, .. } => {
                self.require_awaiting(action_id, event)?;
                self.require_transition(Active, "approval recorded", event)?;
            }
            SessionEvent::ApprovalRejected { action_id, .. } => {
                self.require_awaiting(action_id, event)?;
                self.require_transition(Active, "approval rejected", event)?;
            }
            SessionEvent::ExecutionStarted { action_id } => {
                self.require_transition(Executing(*action_id), "execution started", event)?;
            }
            SessionEvent::ExecutionFinished { action_id, .. } => {
                let status = self.status.clone();
                if !matches!(&status, Executing(id) if id == action_id) {
                    return Err(DomainError::InvalidStateTransition {
                        session: self.id,
                        event: format!("{event:?}"),
                        reason: format!(
                            "execution finished for action {action_id:?} but currently executing {status:?}"
                        ),
                    });
                }
                self.require_transition(Active, "execution finished", event)?;
            }
            SessionEvent::ValidationRecorded {
                status: ValidationStatus::Uncertain,
                ..
            } => {
                self.require_transition(Uncertain, "validation uncertain", event)?;
            }
            SessionEvent::SessionCompleted => {
                if let Some(graph) = self.task_graph.as_ref()
                    && let Some(incomplete) = graph.tasks.iter().find(|task| {
                        task.priority == work::WorkPriority::Required
                            && task.status != work::WorkTaskStatus::Passed
                    })
                {
                    return Err(DomainError::InvalidStateTransition {
                        session: self.id,
                        event: format!("{event:?}"),
                        reason: format!(
                            "required task {:?} is {:?}, not passed",
                            incomplete.id, incomplete.status
                        ),
                    });
                }
                self.require_transition(Completed, "session completed", event)?;
            }
            SessionEvent::SessionCancelled { .. } => {
                self.require_transition(Cancelled, "session cancelled", event)?;
            }
            SessionEvent::RecoveryRequired { .. } => {
                self.require_transition(Uncertain, "recovery required", event)?;
            }
            SessionEvent::SessionFailed { .. } => {
                self.require_transition(Failed, "session failed", event)?;
            }
            // ── v1.4 delegation lifecycle ────────────────────────────────
            //
            // The whole point of validating here rather than in the daemon is
            // that `SessionStore::append` refuses to persist an event the
            // reducer rejects. A completed worker cannot be restarted, a patch
            // cannot be applied without an approval, and a result cannot be
            // recorded twice — even if a caller tries, and even across a daemon
            // restart, because the durable log is the thing being checked.
            SessionEvent::DelegationCreated { delegation } => {
                if self.delegations.contains_key(&delegation.id()) {
                    return Err(DomainError::DuplicateEvent {
                        session: self.id,
                        event: format!("{event:?}"),
                    });
                }
            }
            SessionEvent::DelegationReady { delegation_id } => {
                self.require_delegation_transition(
                    *delegation_id,
                    delegation::DelegationStatus::Ready,
                    event,
                )?;
            }
            SessionEvent::DelegationBlocked { delegation_id, .. } => {
                self.require_delegation_transition(
                    *delegation_id,
                    delegation::DelegationStatus::Blocked,
                    event,
                )?;
            }
            SessionEvent::DelegationWorkerAssigned { assignment } => {
                let record = self.require_delegation(assignment.delegation_id, event)?;
                if record.assignment.is_some() {
                    return Err(DomainError::DuplicateEvent {
                        session: self.id,
                        event: format!("{event:?}"),
                    });
                }
                if !assignment.workspace.is_coherent() {
                    return Err(DomainError::InvalidStateTransition {
                        session: self.id,
                        event: format!("{event:?}"),
                        reason: "a writer worker must have its own worktree and a read-only \
                                 worker must have none"
                            .into(),
                    });
                }
            }
            SessionEvent::DelegationWorkerStarted { delegation_id, .. } => {
                self.require_delegation_transition(
                    *delegation_id,
                    delegation::DelegationStatus::Running,
                    event,
                )?;
                let record = self.require_delegation(*delegation_id, event)?;
                if record.assignment.is_none() {
                    return Err(DomainError::InvalidStateTransition {
                        session: self.id,
                        event: format!("{event:?}"),
                        reason: "a worker cannot start before it is assigned a workspace".into(),
                    });
                }
            }
            SessionEvent::DelegationWorkerPaused { delegation_id, .. } => {
                let record = self.require_delegation(*delegation_id, event)?;
                if !record.is_running() {
                    return Err(DomainError::InvalidStateTransition {
                        session: self.id,
                        event: format!("{event:?}"),
                        reason: format!(
                            "only a running worker can pause, not {:?}",
                            record.status()
                        ),
                    });
                }
            }
            SessionEvent::DelegationWorkerCompleted { delegation_id, .. } => {
                self.require_delegation_transition(
                    *delegation_id,
                    delegation::DelegationStatus::Completed,
                    event,
                )?;
            }
            SessionEvent::DelegationWorkerFailed { delegation_id, .. } => {
                self.require_delegation_transition(
                    *delegation_id,
                    delegation::DelegationStatus::Failed,
                    event,
                )?;
            }
            SessionEvent::DelegationWorkerCancelled { delegation_id, .. }
            | SessionEvent::DelegationCancelled { delegation_id, .. } => {
                self.require_delegation_transition(
                    *delegation_id,
                    delegation::DelegationStatus::Cancelled,
                    event,
                )?;
            }
            SessionEvent::DelegationResultRecorded { result } => {
                let record = self.require_delegation(result.delegation_id, event)?;
                if record.result.is_some() {
                    // At-most-once: a replayed or duplicated result must not
                    // overwrite the one that was already integrated against.
                    return Err(DomainError::DuplicateEvent {
                        session: self.id,
                        event: format!("{event:?}"),
                    });
                }
                result
                    .validate_against(&record.delegation)
                    .map_err(|error| DomainError::InvalidStateTransition {
                        session: self.id,
                        event: format!("{event:?}"),
                        reason: error.to_string(),
                    })?;
            }
            SessionEvent::DelegationRepairRequested {
                delegation_id,
                cycle,
                ..
            } => {
                let record = self.require_delegation(*delegation_id, event)?;
                if *cycle == 0 || *cycle > delegation::MAXIMUM_REPAIR_CYCLES {
                    return Err(DomainError::InvalidStateTransition {
                        session: self.id,
                        event: format!("{event:?}"),
                        reason: format!(
                            "repair cycle {cycle} is outside 1..={}",
                            delegation::MAXIMUM_REPAIR_CYCLES
                        ),
                    });
                }
                if *cycle != record.repair_cycles + 1 {
                    return Err(DomainError::InvalidStateTransition {
                        session: self.id,
                        event: format!("{event:?}"),
                        reason: format!(
                            "repair cycle {cycle} does not follow {}",
                            record.repair_cycles
                        ),
                    });
                }
            }
            SessionEvent::IntegrationProposed { proposal } => {
                let record = self.require_delegation(proposal.delegation_id, event)?;
                if record.result.is_none() {
                    return Err(DomainError::InvalidStateTransition {
                        session: self.id,
                        event: format!("{event:?}"),
                        reason: "an integration cannot be proposed before the worker's result \
                                 is recorded"
                            .into(),
                    });
                }
                if record.integration == delegation::IntegrationState::Applied {
                    return Err(DomainError::InvalidStateTransition {
                        session: self.id,
                        event: format!("{event:?}"),
                        reason: "this delegation's patch has already been applied".into(),
                    });
                }
            }
            SessionEvent::IntegrationConflictDetected { delegation_id, .. } => {
                self.require_delegation(*delegation_id, event)?;
            }
            SessionEvent::IntegrationApproved { delegation_id, .. } => {
                let record = self.require_delegation(*delegation_id, event)?;
                let Some(proposal) = record.proposal.as_ref() else {
                    return Err(DomainError::InvalidStateTransition {
                        session: self.id,
                        event: format!("{event:?}"),
                        reason: "there is no integration proposal to approve".into(),
                    });
                };
                // Same-hunk overlap is never approvable: it has to be resolved
                // into a different patch first, which produces a new proposal.
                if proposal
                    .conflicts
                    .iter()
                    .chain(record.conflicts.iter())
                    .any(|conflict| !conflict.kind.is_auto_mergeable())
                {
                    return Err(DomainError::InvalidStateTransition {
                        session: self.id,
                        event: format!("{event:?}"),
                        reason: "unresolved conflicts must be resolved before approval".into(),
                    });
                }
            }
            SessionEvent::IntegrationRejected { delegation_id, .. } => {
                let record = self.require_delegation(*delegation_id, event)?;
                if record.proposal.is_none() {
                    return Err(DomainError::InvalidStateTransition {
                        session: self.id,
                        event: format!("{event:?}"),
                        reason: "there is no integration proposal to reject".into(),
                    });
                }
            }
            SessionEvent::IntegrationApplied {
                delegation_id,
                patch_digest,
                ..
            } => {
                let record = self.require_delegation(*delegation_id, event)?;
                if !record.integration.can_apply() {
                    return Err(DomainError::InvalidStateTransition {
                        session: self.id,
                        event: format!("{event:?}"),
                        reason: format!(
                            "a patch may only be applied from Approved, not {:?}",
                            record.integration
                        ),
                    });
                }
                // The applied bytes must be the approved bytes. A selected-hunk
                // integration carries its own digest precisely so this check can
                // catch an apply that drifted from what a human saw.
                let approved = record
                    .proposal
                    .as_ref()
                    .map(|proposal| proposal.effective_patch_digest().to_owned())
                    .unwrap_or_default();
                if &approved != patch_digest {
                    return Err(DomainError::InvalidStateTransition {
                        session: self.id,
                        event: format!("{event:?}"),
                        reason: "the applied patch digest differs from the approved one".into(),
                    });
                }
            }
            SessionEvent::DelegationCompleted { delegation_id } => {
                self.require_delegation(*delegation_id, event)?;
            }
            _ => {}
        }
        Ok(())
    }

    /// Look up a delegation the event refers to, or fail the transition. An
    /// event naming a delegation that was never created is a bug or a tampered
    /// log; either way it must not silently do nothing.
    fn require_delegation(
        &self,
        id: delegation::DelegationId,
        event: &SessionEvent,
    ) -> Result<&delegation::DelegationRecord, DomainError> {
        self.delegations
            .get(&id)
            .ok_or_else(|| DomainError::InvalidStateTransition {
                session: self.id,
                event: format!("{event:?}"),
                reason: format!("delegation {id} was never created"),
            })
    }

    fn require_delegation_transition(
        &self,
        id: delegation::DelegationId,
        next: delegation::DelegationStatus,
        event: &SessionEvent,
    ) -> Result<(), DomainError> {
        let record = self.require_delegation(id, event)?;
        if !record.status().can_transition_to(next) {
            return Err(DomainError::InvalidStateTransition {
                session: self.id,
                event: format!("{event:?}"),
                reason: format!(
                    "delegation {id} cannot move from {:?} to {next:?}",
                    record.status()
                ),
            });
        }
        Ok(())
    }

    fn apply_event(&mut self, event: &SessionEvent) {
        match event {
            SessionEvent::SessionCreated {
                objective,
                repository,
                ..
            } => {
                self.objective = Some(objective.clone());
                self.repository = Some(repository.clone());
            }
            SessionEvent::SessionControlsUpdated { controls } => {
                self.controls = controls.clone();
            }
            SessionEvent::WorkflowPlanCreated { decision, plan } => {
                self.complexity_decision = Some(decision.clone());
                self.workflow_plan = Some(plan.clone());
            }
            SessionEvent::UsageRecorded { record } => {
                self.usage_records.push(record.clone());
            }
            SessionEvent::WorktreeCreated {
                path, base_head, ..
            } => {
                self.worktree = Some(path.clone());
                self.base_head = Some(base_head.clone());
            }
            SessionEvent::ActionProposed {
                action_id, action, ..
            } => {
                self.proposed_actions.insert(*action_id, action.clone());
            }
            SessionEvent::TerminalActionProposed { action_id, action } => {
                self.proposed_terminal_actions
                    .insert(*action_id, action.clone());
            }
            SessionEvent::TerminalJudgmentRecorded {
                action_id,
                decision,
            } => {
                self.terminal_judgments.insert(*action_id, decision.clone());
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
            SessionEvent::ExpectationContractCreated { contract } => {
                self.expectation_contract = Some((**contract).clone());
            }
            SessionEvent::ExpectationContractRevised { revision } => {
                // Re-derived rather than restored: replay reaches the contract
                // by applying the same rules the live runtime applied, so a
                // status the rules would have reset cannot survive a restart.
                if let Some(current) = self.expectation_contract.as_ref()
                    && let Ok(revised) = current.revise((**revision).clone())
                {
                    self.expectation_contract = Some(revised.contract);
                }
            }
            SessionEvent::RequirementStatusChanged {
                requirement_id,
                status,
                ..
            } => {
                if let Some(contract) = self.expectation_contract.as_mut()
                    && let Some(clause) = contract.clause_mut(*requirement_id)
                {
                    clause.status = status.clone();
                }
            }
            SessionEvent::ReviewStarted { record } => {
                self.reviews.insert(record.id, (**record).clone());
            }
            SessionEvent::ReviewFindingRecorded { finding } => {
                if let Some(review) = self.reviews.get_mut(&finding.review) {
                    review.findings.push(finding.id);
                }
                self.findings.insert(finding.id, (**finding).clone());
            }
            SessionEvent::ReviewCompleted { review } => {
                if let Some(record) = self.reviews.get_mut(review) {
                    record.completed = true;
                }
            }
            SessionEvent::CorrectionStarted { .. } => {}
            SessionEvent::CorrectionCompleted {
                repaired,
                still_open,
                ..
            } => {
                self.correction_ledger
                    .record_cycle(repaired.clone(), still_open.clone());
            }
            SessionEvent::DeliveryGateEvaluated { assessment } => {
                self.delivery = Some((**assessment).clone());
            }
            SessionEvent::SpecBundleRecorded { bundle, .. } => {
                self.spec_bundle = Some(bundle.clone());
            }
            SessionEvent::TaskGraphRecorded { graph, .. } => {
                let mut graph = graph.clone();
                graph.refresh_ready();
                self.task_graph = Some(graph);
            }
            SessionEvent::TaskStatusChanged {
                task_id, status, ..
            } => {
                self.task_graph
                    .as_mut()
                    .expect("task transition was validated against a graph")
                    .transition(*task_id, *status)
                    .expect("task transition was validated before apply");
            }
            SessionEvent::EvidenceLinked { evidence } => {
                self.evidence_links.push(evidence.clone());
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
            SessionEvent::CheckpointCompacted {
                checkpoint,
                retained_action_ids,
                conversation_messages_retained_from,
            } => {
                // Chain: merge additively over the superseded checkpoint so
                // failed_attempts, files_inspected, and decisions accumulate
                // across every compaction instead of being dropped (PRD v1.1 §7.3).
                let merged = if let Some(ref prev) = self.checkpoint {
                    Self::merge_checkpoint(prev, checkpoint)
                } else {
                    checkpoint.as_ref().clone()
                };
                self.checkpoint = Some(merged);
                let retained: BTreeSet<_> = retained_action_ids.iter().copied().collect();
                self.proposed_actions.retain(|id, _| retained.contains(id));
                self.judgments.retain(|id, _| retained.contains(id));
                self.contextual_judgments
                    .retain(|id, _| retained.contains(id));
                // conversation_messages now bounded — the oldest messages before
                // the retained window are dropped (PRD v1.1 §7.3, §7.5).
                if *conversation_messages_retained_from < self.conversation_messages.len() {
                    self.conversation_messages = self
                        .conversation_messages
                        .split_off(*conversation_messages_retained_from);
                }
            }
            SessionEvent::ContextAssembled { entry } => {
                self.recent_context_ledger.push_back(entry.clone());
                while self.recent_context_ledger.len() > MAX_RECENT_CONTEXT_LEDGER_ENTRIES {
                    self.recent_context_ledger.pop_front();
                }
            }
            SessionEvent::SessionPaused { .. } => {
                self.status = SessionStatus::Paused;
            }
            SessionEvent::SessionResumed => {
                self.status = SessionStatus::Active;
            }
            SessionEvent::ModelSelected { model } => self.selected_model = Some(model.clone()),
            SessionEvent::AgentBound { agent } => self.selected_agent = Some(agent.clone()),
            SessionEvent::ConversationMessageAdded { message } => {
                self.conversation_messages.push(message.clone());
            }
            SessionEvent::JudgmentRecorded {
                action_id,
                decision,
                ..
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
                self.status = SessionStatus::AwaitingReview;
            }
            SessionEvent::OutcomeReviewApproved { .. } => {
                self.status = SessionStatus::Active;
            }
            SessionEvent::ApprovalRecorded { .. } => {
                self.status = SessionStatus::Active;
            }
            SessionEvent::ApprovalRejected { .. } => {
                self.status = SessionStatus::Active;
            }
            SessionEvent::ExecutionStarted { action_id } => {
                self.status = SessionStatus::Executing(*action_id);
            }
            SessionEvent::ExecutionFinished { .. } => {
                self.status = SessionStatus::Active;
            }
            SessionEvent::ValidationRecorded {
                status: ValidationStatus::Uncertain,
                ..
            } => {
                self.status = SessionStatus::Uncertain;
            }
            SessionEvent::SessionCompleted => {
                self.status = SessionStatus::Completed;
            }
            SessionEvent::SessionCancelled { .. } => {
                self.status = SessionStatus::Cancelled;
            }
            SessionEvent::RecoveryRequired { .. } => {
                self.status = SessionStatus::Uncertain;
            }
            SessionEvent::SessionFailed { .. } => {
                self.status = SessionStatus::Failed;
            }
            // Durable audit of completion-repair attempts; changes no session
            // state.
            SessionEvent::CompletionRepairRecorded { .. } => {}
            // ── v1.4 delegation projection ───────────────────────────────
            SessionEvent::DelegationPlanned { plan } => {
                self.delegation_plan = Some((**plan).clone());
            }
            SessionEvent::DelegationCreated { delegation } => {
                self.delegations.insert(
                    delegation.id(),
                    delegation::DelegationRecord::new((**delegation).clone()),
                );
            }
            SessionEvent::DelegationRoutingRecorded {
                delegation_id,
                decision,
            } => {
                if let Some(record) = self.delegations.get_mut(delegation_id) {
                    record.routing = Some(decision.clone());
                }
            }
            SessionEvent::DelegationReady { delegation_id } => {
                self.set_delegation_status(*delegation_id, delegation::DelegationStatus::Ready);
            }
            SessionEvent::DelegationBlocked {
                delegation_id,
                blocking_dependency,
            } => {
                self.set_delegation_status(*delegation_id, delegation::DelegationStatus::Blocked);
                if let Some(record) = self.delegations.get_mut(delegation_id) {
                    record.blocked_by = Some(*blocking_dependency);
                }
            }
            SessionEvent::DelegationWorkerAssigned { assignment } => {
                if let Some(record) = self.delegations.get_mut(&assignment.delegation_id) {
                    record.assignment = Some((**assignment).clone());
                }
            }
            SessionEvent::DelegationWorkerStarted { delegation_id, .. } => {
                self.set_delegation_status(*delegation_id, delegation::DelegationStatus::Running);
                if let Some(record) = self.delegations.get_mut(delegation_id) {
                    record.paused_reason = None;
                }
                self.delegation_ledger.workers_started += 1;
            }
            SessionEvent::DelegationWorkerPaused {
                delegation_id,
                reason,
                ..
            } => {
                // Status stays `Running`: a paused worker is reconciled on
                // restart, not restarted. Recording it as anything terminal
                // would license a rerun.
                if let Some(record) = self.delegations.get_mut(delegation_id) {
                    record.paused_reason = Some(reason.clone());
                }
            }
            SessionEvent::DelegationWorkerCompleted { delegation_id, .. } => {
                self.set_delegation_status(*delegation_id, delegation::DelegationStatus::Completed);
                self.delegation_ledger.workers_completed += 1;
            }
            SessionEvent::DelegationWorkerFailed { delegation_id, .. } => {
                self.set_delegation_status(*delegation_id, delegation::DelegationStatus::Failed);
            }
            SessionEvent::DelegationWorkerCancelled { delegation_id, .. }
            | SessionEvent::DelegationCancelled { delegation_id, .. } => {
                self.set_delegation_status(*delegation_id, delegation::DelegationStatus::Cancelled);
            }
            SessionEvent::DelegationResultRecorded { result } => {
                // Parent usage includes child usage (§6.3): the ledger is the
                // single place that total is kept, so a worker cannot spend
                // outside the session's accounting.
                self.delegation_ledger.record_usage(&result.usage);
                if let Some(record) = self.delegations.get_mut(&result.delegation_id) {
                    record.result = Some((**result).clone());
                }
            }
            SessionEvent::DelegationRepairRequested {
                delegation_id,
                cycle,
                ..
            } => {
                if let Some(record) = self.delegations.get_mut(delegation_id) {
                    record.repair_cycles = *cycle;
                }
            }
            SessionEvent::IntegrationProposed { proposal } => {
                if let Some(record) = self.delegations.get_mut(&proposal.delegation_id) {
                    record.conflicts = proposal.conflicts.clone();
                    record.integration = if proposal.conflicts.is_empty() {
                        delegation::IntegrationState::Proposed
                    } else {
                        delegation::IntegrationState::Conflicted
                    };
                    record.proposal = Some((**proposal).clone());
                }
            }
            SessionEvent::IntegrationConflictDetected {
                delegation_id,
                conflicts,
            } => {
                if let Some(record) = self.delegations.get_mut(delegation_id) {
                    record.conflicts.extend(conflicts.iter().cloned());
                    record.integration = delegation::IntegrationState::Conflicted;
                }
            }
            SessionEvent::IntegrationApproved { delegation_id, .. } => {
                if let Some(record) = self.delegations.get_mut(delegation_id) {
                    record.integration = delegation::IntegrationState::Approved;
                }
            }
            SessionEvent::IntegrationRejected { delegation_id, .. } => {
                if let Some(record) = self.delegations.get_mut(delegation_id) {
                    record.integration = delegation::IntegrationState::Rejected;
                }
            }
            SessionEvent::IntegrationApplied { delegation_id, .. } => {
                if let Some(record) = self.delegations.get_mut(delegation_id) {
                    record.integration = delegation::IntegrationState::Applied;
                }
            }
            SessionEvent::DelegationCompleted { delegation_id } => {
                // Idempotent: a delegation may already be Completed from its
                // worker event. `set_delegation_status` refuses illegal moves.
                self.set_delegation_status(*delegation_id, delegation::DelegationStatus::Completed);
            }
            _ => {}
        }
    }

    /// Apply a delegation status change, ignoring an illegal one.
    ///
    /// `validate_event` has already refused the transitions that must not
    /// happen; this is the belt to that pair of braces, and it means the
    /// projection can never hold a status the state machine forbids.
    fn set_delegation_status(
        &mut self,
        id: delegation::DelegationId,
        next: delegation::DelegationStatus,
    ) {
        if let Some(record) = self.delegations.get_mut(&id) {
            let _ = record.delegation.transition_to(next);
        }
    }

    // ── SemanticCheckpoint merge (PRD v1.1 §7.3) ──────────────────

    /// Merge a new checkpoint over the previous one, unioning additive fields.
    /// Called from the `CheckpointCompacted` reducer arm — not test-only.
    pub fn merge_checkpoint(
        previous: &SemanticCheckpoint,
        latest: &SemanticCheckpoint,
    ) -> SemanticCheckpoint {
        use std::collections::BTreeSet;
        let mut merged = latest.clone();
        merged.superseded_checkpoint_id = Some(previous.checkpoint_id);
        let mut files_inspected: BTreeSet<PathBuf> =
            previous.files_inspected.iter().cloned().collect();
        files_inspected.extend(latest.files_inspected.iter().cloned());
        merged.files_inspected = files_inspected.into_iter().collect();

        let mut files_modified: BTreeSet<PathBuf> =
            previous.files_modified.iter().cloned().collect();
        files_modified.extend(latest.files_modified.iter().cloned());
        merged.files_modified = files_modified.into_iter().collect();

        let mut decisions = previous.decisions.clone();
        decisions.extend(latest.decisions.iter().cloned());
        merged.decisions = decisions;

        let mut failed_attempts = previous.failed_attempts.clone();
        failed_attempts.extend(latest.failed_attempts.iter().cloned());
        merged.failed_attempts = failed_attempts;

        let mut validated_facts: BTreeSet<String> =
            previous.validated_facts.iter().cloned().collect();
        validated_facts.extend(latest.validated_facts.iter().cloned());
        merged.validated_facts = validated_facts.into_iter().collect();

        let mut test_results = previous.test_results.clone();
        test_results.extend(latest.test_results.iter().cloned());
        merged.test_results = test_results;

        let mut pinned_context = previous.pinned_context.clone();
        pinned_context.extend(latest.pinned_context.iter().cloned());
        merged.pinned_context = pinned_context;

        // Accumulated memory, same as files_inspected/validated_facts above:
        // a requirement once accepted, or a symbol once seen as significant,
        // stays remembered across compactions rather than being forgotten
        // the moment a later checkpoint's snapshot of "what's inspected right
        // now" doesn't happen to include it.
        let mut accepted_requirements: BTreeSet<String> =
            previous.accepted_requirements.iter().cloned().collect();
        accepted_requirements.extend(latest.accepted_requirements.iter().cloned());
        merged.accepted_requirements = accepted_requirements.into_iter().collect();

        let mut important_symbols: BTreeSet<String> =
            previous.important_symbols.iter().cloned().collect();
        important_symbols.extend(latest.important_symbols.iter().cloned());
        merged.important_symbols = important_symbols.into_iter().collect();

        if merged.objective.is_empty() {
            merged.objective = previous.objective.clone();
        }
        if merged.current_hypothesis.is_none() {
            merged.current_hypothesis = previous.current_hypothesis.clone();
        }
        // user_constraints/next_actions describe *current* state (the
        // session's live controls, the plan's still-open work) rather than
        // accumulated history, so unioning them the way files/facts are
        // unioned above would let stale entries (a task_mode that's since
        // changed, a next_action already completed) linger forever next to
        // the correct current one. Only fall back to `previous` when
        // `latest` has nothing to say — the same rule already applied to
        // `objective`/`current_hypothesis` above — never merge the two sets.
        if merged.user_constraints.is_empty() {
            merged.user_constraints = previous.user_constraints.clone();
        }
        if merged.next_actions.is_empty() {
            merged.next_actions = previous.next_actions.clone();
        }
        let mut unresolved: BTreeSet<String> =
            previous.unresolved_questions.iter().cloned().collect();
        unresolved.extend(latest.unresolved_questions.iter().cloned());
        merged.unresolved_questions = unresolved.into_iter().collect();

        merged
    }

    /// Deprecated wrapper around [`Self::reduce_event`].
    ///
    /// Retained for callers that have not yet migrated to the result-returning
    /// API. Errors are swallowed: the previous behaviour was to apply the event
    /// unconditionally. New code must use [`Self::reduce_event`].
    #[deprecated(
        since = "0.6.0",
        note = "use reduce_event; apply silently swallows invalid transitions"
    )]
    pub fn apply(&mut self, event: &SessionEvent) {
        let _ = self.reduce_event(event);
    }

    fn require_transition(
        &self,
        next: SessionStatus,
        reason: &str,
        event: &SessionEvent,
    ) -> Result<(), DomainError> {
        if !is_valid_transition(&self.status, &next) {
            return Err(DomainError::InvalidStateTransition {
                session: self.id,
                event: format!("{event:?}"),
                reason: format!("{reason}: cannot move from {:?} to {:?}", self.status, next),
            });
        }
        Ok(())
    }

    fn require_awaiting(
        &self,
        action_id: &ActionId,
        _event: &SessionEvent,
    ) -> Result<(), DomainError> {
        let expected = match self.status {
            SessionStatus::AwaitingApproval(pending) if pending == *action_id => return Ok(()),
            SessionStatus::AwaitingApproval(pending) => Some(pending),
            _ => None,
        };
        Err(DomainError::UnexpectedApproval {
            session: self.id,
            action_id: *action_id,
            expected,
        })
    }
}

fn require_event_reason(
    session: SessionId,
    event: &SessionEvent,
    reason: &str,
) -> Result<(), DomainError> {
    if reason.trim().is_empty() {
        Err(DomainError::InvalidStateTransition {
            session,
            event: format!("{event:?}"),
            reason: "durable work-model changes require a reason".into(),
        })
    } else {
        Ok(())
    }
}

/// Reconstruct session state by replaying an ordered slice of events.
/// Returns an error if any event is invalid for the derived state.
pub fn reconstruct_state(
    id: SessionId,
    events: &[SessionEvent],
) -> Result<SessionState, DomainError> {
    let mut state = SessionState::empty(id);
    for event in events {
        state.reduce_event(event)?;
    }
    Ok(state)
}

/// Required transition matrix.
///
/// `Completed`, `Cancelled`, and `Failed` close one agent turn, but a person
/// may start another turn in the same conversation. `Uncertain` remains a
/// recovery boundary rather than accepting ordinary chat.
/// `AwaitingApproval` and `Executing` track a single `ActionId`; an
/// approval or execution event must reference that id.
///
/// Returns true only for the explicitly enumerated transitions below. A wildcard
/// `(Active, _)` is intentionally absent so that a transition from Active to
/// Completed, Failed, or Cancelled (for example) requires an explicit pair and
/// is never silently permitted.
fn is_valid_transition(current: &SessionStatus, next: &SessionStatus) -> bool {
    use SessionStatus::*;
    match (current, next) {
        // ── Ended turn: a follow-up starts a new turn ────────────
        (Completed | Cancelled | Failed, Active) => true,
        (Completed | Cancelled | Failed, _) => false,

        // ── Uncertain: can recover to any non-terminal state ─────
        (Uncertain, Active) => true,
        (Uncertain, Paused) => true,
        (Uncertain, AwaitingApproval(_)) => true,
        (Uncertain, Executing(_)) => true,
        (Uncertain, AwaitingReview) => true,
        (Uncertain, Failed) => true,
        (Uncertain, Completed) => true,
        (Uncertain, Cancelled) => true,
        (Uncertain, _) => false,

        // ── Active: running normally ─────────────────────────────
        (Active, Paused) => true,
        (Active, AwaitingApproval(_)) => true,
        (Active, AwaitingReview) => true,
        (Active, Executing(_)) => true,
        (Active, Completed) => true,
        (Active, Failed) => true,
        (Active, Cancelled) => true,
        (Active, Uncertain) => true,
        (Active, _) => false,

        // ── Paused: can resume or fail ───────────────────────────
        (Paused, Active) => true,
        (Paused, AwaitingApproval(_)) => true,
        (Paused, Failed) => true,
        (Paused, Cancelled) => true,
        (Paused, Completed) => true,
        (Paused, _) => false,

        // ── AwaitingApproval: can approve, reject, or pause ──────
        (AwaitingApproval(_), Active) => true,
        (AwaitingApproval(_), AwaitingApproval(_)) => true,
        (AwaitingApproval(_), Paused) => true,
        (AwaitingApproval(_), Executing(_)) => true,
        (AwaitingApproval(_), Failed) => true,
        (AwaitingApproval(_), Cancelled) => true,
        (AwaitingApproval(_), _) => false,

        // ── AwaitingReview: outcome review ───────────────────────
        (AwaitingReview, Active) => true,
        (AwaitingReview, Paused) => true,
        (AwaitingReview, Failed) => true,
        (AwaitingReview, Cancelled) => true,
        (AwaitingReview, Completed) => true,
        (AwaitingReview, _) => false,

        // ── Executing: an action is running ──────────────────────
        (Executing(_), Active) => true,
        (Executing(_), Uncertain) => true,
        (Executing(_), Paused) => true,
        (Executing(_), AwaitingApproval(_)) => true,
        (Executing(_), Failed) => true,
        (Executing(_), Cancelled) => true,
        (Executing(_), _) => false,
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
    /// The turn that produced this message (PRD v1.1 §6.3). `None` for
    /// user-typed messages (created outside `run_until_pause`) and for
    /// messages recorded before Phase 1 shipped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<TurnId>,
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
    #[error("session {session:?} cannot accept event {event}: {reason}")]
    InvalidStateTransition {
        session: SessionId,
        event: String,
        reason: String,
    },
    #[error(
        "session {session:?} received approval event for action {action_id:?} but was awaiting {expected:?}"
    )]
    UnexpectedApproval {
        session: SessionId,
        action_id: ActionId,
        expected: Option<ActionId>,
    },
    #[error("typed read has invalid bounds: {reason}")]
    InvalidBounds { reason: String },
    #[error("session {session:?} received event {event:?} that was already applied")]
    DuplicateEvent { session: SessionId, event: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_task_and_evidence_replay_as_one_durable_work_model() {
        let id = SessionId::new();
        let requirement_id = RequirementId::new();
        let criterion_id = CriterionId::new();
        let task_id = WorkTaskId::new();
        let spec = SpecBundle {
            revision: 1,
            kind: SpecKind::FeatureRequirementsFirst,
            title: "Truthful review".into(),
            requirements: vec![Requirement {
                id: requirement_id,
                statement: "Review distinguishes failed evidence".into(),
                priority: WorkPriority::Required,
                acceptance_criteria: vec![AcceptanceCriterion {
                    id: criterion_id,
                    statement: "A timeout renders Error".into(),
                }],
            }],
            non_goals: vec![],
            design_decisions: vec![],
        };
        let graph = TaskGraph {
            revision: 1,
            tasks: vec![WorkTask {
                id: task_id,
                objective: "Add typed panel state".into(),
                dependencies: vec![],
                priority: WorkPriority::Required,
                risk: WorkRisk::High,
                acceptance_criteria: vec![criterion_id],
                scope: vec![PathBuf::from("crates/purrcode-ide")],
                owner: Some("implementation".into()),
                status: WorkTaskStatus::Pending,
                retry_count: 0,
                evidence_obligations: vec![EvidenceObligation {
                    requirement_id,
                    criterion_id,
                    description: "Inject a timeout".into(),
                    required: true,
                }],
            }],
        };
        let evidence = EvidenceLink {
            id: EvidenceId::new(),
            requirement_id,
            criterion_id,
            task_id,
            action_id: None,
            coverage: EvidenceCoverage::Covered,
            validation_status: Some(ValidationStatus::Passed),
            source: "contract test".into(),
            summary: "timeout remained distinct from empty".into(),
            digest: "evidence-digest".into(),
            recorded_at: Utc::now(),
        };
        let events = vec![
            SessionEvent::SessionCreated {
                objective: "Make review truthful".into(),
                repository: PathBuf::from("/repo"),
                authority_mode: AuthorityMode::Governed,
            },
            SessionEvent::SpecBundleRecorded {
                bundle: spec.clone(),
                reason: "accepted requirements".into(),
            },
            SessionEvent::TaskGraphRecorded {
                graph,
                reason: "derived implementation tasks".into(),
            },
            SessionEvent::TaskStatusChanged {
                task_id,
                status: WorkTaskStatus::Running,
                reason: "worker started".into(),
            },
            SessionEvent::EvidenceLinked {
                evidence: evidence.clone(),
            },
            SessionEvent::TaskStatusChanged {
                task_id,
                status: WorkTaskStatus::Passed,
                reason: "required evidence passed".into(),
            },
        ];

        let state = reconstruct_state(id, &events).unwrap();
        assert_eq!(state.spec_bundle, Some(spec));
        assert_eq!(
            state.task_graph.unwrap().task(task_id).unwrap().status,
            WorkTaskStatus::Passed
        );
        assert_eq!(state.evidence_links, vec![evidence]);
    }

    #[test]
    fn task_transition_without_a_graph_fails_loudly() {
        let mut state = SessionState::empty(SessionId::new());
        let error = state
            .reduce_event(&SessionEvent::TaskStatusChanged {
                task_id: WorkTaskId::new(),
                status: WorkTaskStatus::Running,
                reason: "cannot run missing task".into(),
            })
            .unwrap_err();
        assert!(matches!(error, DomainError::InvalidStateTransition { .. }));
        assert_eq!(state.event_count, 0);
    }

    #[test]
    fn required_task_cannot_pass_without_closing_evidence() {
        let id = SessionId::new();
        let requirement_id = RequirementId::new();
        let criterion_id = CriterionId::new();
        let task_id = WorkTaskId::new();
        let events = [
            SessionEvent::SpecBundleRecorded {
                bundle: SpecBundle {
                    revision: 1,
                    kind: SpecKind::Direct,
                    title: "Evidence gate".into(),
                    requirements: vec![Requirement {
                        id: requirement_id,
                        statement: "Task completion is evidence-derived".into(),
                        priority: WorkPriority::Required,
                        acceptance_criteria: vec![AcceptanceCriterion {
                            id: criterion_id,
                            statement: "Missing evidence blocks pass".into(),
                        }],
                    }],
                    non_goals: vec![],
                    design_decisions: vec![],
                },
                reason: "record direct intent".into(),
            },
            SessionEvent::TaskGraphRecorded {
                graph: TaskGraph {
                    revision: 1,
                    tasks: vec![WorkTask {
                        id: task_id,
                        objective: "Prove the gate".into(),
                        dependencies: vec![],
                        priority: WorkPriority::Required,
                        risk: WorkRisk::High,
                        acceptance_criteria: vec![criterion_id],
                        scope: vec![],
                        owner: None,
                        status: WorkTaskStatus::Pending,
                        retry_count: 0,
                        evidence_obligations: vec![],
                    }],
                },
                reason: "record task".into(),
            },
            SessionEvent::TaskStatusChanged {
                task_id,
                status: WorkTaskStatus::Running,
                reason: "start task".into(),
            },
        ];
        let mut state = SessionState::empty(id);
        for event in events {
            state.reduce_event(&event).unwrap();
        }
        let error = state
            .reduce_event(&SessionEvent::TaskStatusChanged {
                task_id,
                status: WorkTaskStatus::Passed,
                reason: "model said complete".into(),
            })
            .unwrap_err();
        assert!(error.to_string().contains("closing evidence"));
        assert_eq!(
            state.task_graph.unwrap().task(task_id).unwrap().status,
            WorkTaskStatus::Running
        );
    }

    #[test]
    fn plan_revision_and_context_compaction_replay_deterministically() {
        let mut state = SessionState::empty(SessionId::new());
        state
            .reduce_event(&SessionEvent::PlanCreated {
                steps: vec!["inspect".into(), "fix".into()],
            })
            .unwrap();
        state
            .reduce_event(&SessionEvent::PlanRevised {
                revision: 2,
                reason: "new evidence".into(),
                steps: vec!["inspect".into(), "fix safely".into()],
            })
            .unwrap();
        let retained = ActionId::new();
        let removed = ActionId::new();
        for id in [retained, removed] {
            state
                .reduce_event(&SessionEvent::ActionProposed {
                    action_id: id,
                    action: ProposedAction::WriteFile(WriteFileAction {
                        path: PathBuf::from("file.txt"),
                        content: "value".into(),
                        expected_digest: None,
                    }),
                    turn_id: None,
                })
                .unwrap();
        }
        state
            .reduce_event(&SessionEvent::ContextCompacted {
                summary: "older evidence summarized".into(),
                retained_action_ids: vec![retained],
            })
            .unwrap();
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

    #[test]
    fn digest_v3_binds_the_descriptor_digest() {
        // §4.1: a descriptor mutated between authorize() and
        // consume_authorization() must void the capability.
        let root = PathBuf::from("/repo");
        let action = ProposedAction::Tool(ToolInvocation {
            tool_id: ToolId::native("read_file"),
            arguments: serde_json::json!({}),
            working_directory: root.clone(),
            descriptor_digest: "abc".into(),
        });
        let constraints = ActionConstraints::read_only(root.clone());

        let d1 = action.digest_v3(&constraints, "descriptor-v1").unwrap();
        let d2 = action.digest_v3(&constraints, "descriptor-v2").unwrap();
        assert_ne!(
            d1, d2,
            "descriptor refresh invalidates outstanding capabilities"
        );

        let same = action.digest_v3(&constraints, "descriptor-v1").unwrap();
        assert_eq!(d1, same, "deterministic for identical inputs");
    }

    #[test]
    fn repository_read_action_synthesizes_safe_shell_invocation() {
        let root = PathBuf::from("/repo");
        let read = RepositoryReadAction::GitLog {
            max_count: Some(5),
            oneline: true,
        };
        let command = read.to_command(root.clone());
        assert_eq!(command.program, PathBuf::from("git"));
        assert_eq!(command.arguments, vec!["log", "--oneline", "-5"]);
        assert_eq!(command.working_directory, root);
        assert_eq!(
            command
                .environment
                .get("GIT_TERMINAL_PROMPT")
                .map(String::as_str),
            Some("0")
        );
    }

    #[test]
    fn repository_read_round_trip_preserves_payload() {
        let read = RepositoryReadAction::RepositoryGrep {
            pattern: "TODO".into(),
            paths: vec![PathBuf::from("src")],
            case_insensitive: true,
            max_results: 128,
            max_bytes: 4096,
        };
        let json = serde_json::to_string(&read).unwrap();
        let parsed: RepositoryReadAction = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, read);
    }

    #[test]
    fn proposed_action_repository_read_round_trip_preserves_payload() {
        let action = ProposedAction::RepositoryRead(RepositoryReadAction::Find {
            paths: vec![PathBuf::from("crates")],
            max_depth: 3,
            max_entries: 64,
        });
        let json = serde_json::to_string(&action).unwrap();
        let parsed: ProposedAction = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, action);
    }

    #[test]
    fn v1_2_session_event_log_still_deserializes_with_new_variants_present() {
        // §10 backward compatibility: appending variants must not change the
        // serde shape of the pre-existing variants, so a v1.2-written log
        // (which carries exactly these shapes) still loads. Because the derive
        // attributes on the old variants are untouched, round-tripping a typed
        // old-shaped event through the CURRENT serde is a faithful proof: the
        // bytes produced here are byte-identical to what v1.2 wrote.
        let legacy = SessionEvent::ActionProposed {
            action_id: ActionId::new(),
            action: ProposedAction::RepositoryRead(RepositoryReadAction::ReadFile {
                path: PathBuf::from("src/lib.rs"),
                max_bytes: DEFAULT_READ_FILE_MAX_BYTES,
            }),
            turn_id: None,
        };
        let bytes = serde_json::to_vec(&legacy).unwrap();
        let parsed: SessionEvent =
            serde_json::from_slice(&bytes).expect("v1.2 log must still load");
        assert_eq!(parsed, legacy);

        // And the new variants round-trip through the same serde framing.
        let event = SessionEvent::ToolEvidenceRecorded {
            evidence: Box::new(ExecutionEvidence {
                action_id: ActionId::new(),
                session_id: SessionId::new(),
                turn_id: None,
                tool_id: ToolId::native("read_file"),
                provider: ToolProvider::Native,
                descriptor_digest: "abc".into(),
                decision: JudgmentDecision::AllowWithConstraints(ActionConstraints::read_only(
                    PathBuf::from("/repo"),
                )),
                approved_by: ApprovalAuthority::DeterministicPolicy,
                constraints: ActionConstraints::read_only(PathBuf::from("/repo")),
                effective_network_scope: NetworkScope::None,
                effective_filesystem_scope: FilesystemScope::WorktreeRead,
                initiator: EvidenceInitiator::Human,
                outcome: ExecutionOutcome::Succeeded {
                    exit_code: Some(0),
                    truncated: false,
                    affected_paths: Vec::new(),
                },
                structured_output: None,
                redaction_class: RedactionClass::Public,
                started_at: chrono::Utc::now(),
                finished_at: chrono::Utc::now(),
            }),
        };
        let json = serde_json::to_string(&event).unwrap();
        let back: SessionEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(back, event);
    }

    // ── v1.4 delegation lifecycle (§PR1, §PR10) ──────────────────────────

    fn delegation_session() -> SessionState {
        let mut state = SessionState::empty(SessionId::new());
        state
            .reduce_event(&SessionEvent::SessionCreated {
                objective: "add oauth".into(),
                repository: PathBuf::from("/repo"),
                authority_mode: Default::default(),
            })
            .unwrap();
        state
    }

    fn admitted_delegation(paths: &[&str], expected: delegation::ExpectedOutput) -> Delegation {
        let ceiling = ToolCeiling::maximum();
        let remaining = delegation::DelegationBudget::modest();
        delegation::DelegationRequest {
            parent_session_id: SessionId::new(),
            parent_turn_id: TurnId::new(),
            objective: "implement token exchange".into(),
            capability: CapabilityId::parse("implement_backend").unwrap(),
            acceptance_criteria: Vec::new(),
            context_refs: Vec::new(),
            allowed_paths: paths
                .iter()
                .map(|p| delegation::PathPattern::parse(p).unwrap())
                .collect(),
            expected_output: expected,
            dependencies: Vec::new(),
            budget: delegation::DelegationBudget::modest(),
        }
        .admit(delegation::AuthorityInputs {
            workspace: &ceiling,
            parent: &ceiling,
            profile: &ceiling,
            parent_remaining_budget: &remaining,
            depth: 1,
        })
        .unwrap()
    }

    fn assignment(delegation: &Delegation, worker_id: WorkerId) -> delegation::WorkerAssignment {
        delegation::WorkerAssignment {
            worker_id,
            delegation_id: delegation.id(),
            agent_profile: "backend-specialist".into(),
            profile_digest: "profile-digest".into(),
            model_role: None,
            workspace: delegation::WorkerWorkspaceRecord {
                parent_worktree: PathBuf::from("/repo/.purrcode/worktrees/parent"),
                worker_worktree: Some(PathBuf::from("/repo/.purrcode/worktrees/parent/worker-a")),
                base_commit: "abc123".into(),
                base_snapshot_digest: "snapshot".into(),
                access: delegation::WorkspaceAccess::Writable,
            },
            assigned_at: chrono::Utc::now(),
        }
    }

    fn worker_result(
        delegation: &Delegation,
        worker_id: WorkerId,
        paths: &[&str],
    ) -> delegation::WorkerResult {
        delegation::WorkerResult {
            delegation_id: delegation.id(),
            worker_id,
            status: delegation::WorkerResultStatus::Completed,
            summary: "implemented".into(),
            changed_paths: paths.iter().map(PathBuf::from).collect(),
            patch_digest: Some("patch-digest".into()),
            findings: Vec::new(),
            validations: vec![delegation::ValidationEvidence {
                name: "cargo test".into(),
                status: ValidationStatus::Passed,
                detail: "ok".into(),
                evidence_id: None,
            }],
            unresolved: Vec::new(),
            evidence_ids: Vec::new(),
            usage: delegation::UsageSummary {
                input_tokens: 1_000,
                output_tokens: 500,
                tool_calls: 4,
                model_calls: 2,
                duration_seconds: 30,
                changed_files: paths.len(),
            },
            completed_at: chrono::Utc::now(),
        }
    }

    /// Drive one delegation from creation to a recorded result.
    fn run_delegation_to_result(
        state: &mut SessionState,
        delegation: &Delegation,
        worker_id: WorkerId,
        paths: &[&str],
    ) {
        state
            .reduce_event(&SessionEvent::DelegationCreated {
                delegation: Box::new(delegation.clone()),
            })
            .unwrap();
        state
            .reduce_event(&SessionEvent::DelegationReady {
                delegation_id: delegation.id(),
            })
            .unwrap();
        state
            .reduce_event(&SessionEvent::DelegationWorkerAssigned {
                assignment: Box::new(assignment(delegation, worker_id)),
            })
            .unwrap();
        state
            .reduce_event(&SessionEvent::DelegationWorkerStarted {
                delegation_id: delegation.id(),
                worker_id,
            })
            .unwrap();
        state
            .reduce_event(&SessionEvent::DelegationResultRecorded {
                result: Box::new(worker_result(delegation, worker_id, paths)),
            })
            .unwrap();
        state
            .reduce_event(&SessionEvent::DelegationWorkerCompleted {
                delegation_id: delegation.id(),
                worker_id,
            })
            .unwrap();
    }

    #[test]
    fn a_delegation_replays_from_creation_to_integration() {
        let mut state = delegation_session();
        let delegation = admitted_delegation(&["src/auth/**"], delegation::ExpectedOutput::Patch);
        let worker_id = WorkerId::new();
        run_delegation_to_result(&mut state, &delegation, worker_id, &["src/auth/token.rs"]);

        let record = &state.delegations[&delegation.id()];
        assert_eq!(record.status(), delegation::DelegationStatus::Completed);
        assert!(record.result.is_some());
        assert_eq!(state.delegation_ledger.workers_started, 1);
        assert_eq!(state.delegation_ledger.workers_completed, 1);
        // Parent usage includes child usage.
        assert_eq!(
            state.delegation_ledger.total_worker_usage.input_tokens,
            1_000
        );

        let proposal = delegation::IntegrationProposal {
            delegation_id: delegation.id(),
            worker_id,
            patch_digest: "patch-digest".into(),
            changed_paths: vec![PathBuf::from("src/auth/token.rs")],
            base_snapshot_digest: "snapshot".into(),
            evidence_ids: Vec::new(),
            validation_summary: Default::default(),
            conflicts: Vec::new(),
            amended_patch_digest: None,
        };
        state
            .reduce_event(&SessionEvent::IntegrationProposed {
                proposal: Box::new(proposal),
            })
            .unwrap();
        state
            .reduce_event(&SessionEvent::IntegrationApproved {
                delegation_id: delegation.id(),
                patch_digest: "patch-digest".into(),
                authority: ApprovalAuthority::Human,
            })
            .unwrap();
        state
            .reduce_event(&SessionEvent::IntegrationApplied {
                delegation_id: delegation.id(),
                patch_digest: "patch-digest".into(),
                changed_paths: vec![PathBuf::from("src/auth/token.rs")],
            })
            .unwrap();
        assert_eq!(
            state.delegations[&delegation.id()].integration,
            delegation::IntegrationState::Applied
        );
    }

    #[test]
    fn a_completed_worker_cannot_be_restarted() {
        // v1.4 §PR10: completed workers are NEVER rerun. The reducer refuses the
        // event, so `SessionStore::append` refuses to persist it — a restarted
        // daemon cannot double-execute by replaying its own intent.
        let mut state = delegation_session();
        let delegation = admitted_delegation(&["src/auth/**"], delegation::ExpectedOutput::Patch);
        let worker_id = WorkerId::new();
        run_delegation_to_result(&mut state, &delegation, worker_id, &["src/auth/token.rs"]);

        let restart = state.reduce_event(&SessionEvent::DelegationWorkerStarted {
            delegation_id: delegation.id(),
            worker_id,
        });
        assert!(
            matches!(restart, Err(DomainError::InvalidStateTransition { .. })),
            "a completed worker must not be restartable, got {restart:?}"
        );
    }

    #[test]
    fn a_worker_result_cannot_be_recorded_twice() {
        let mut state = delegation_session();
        let delegation = admitted_delegation(&["src/auth/**"], delegation::ExpectedOutput::Patch);
        let worker_id = WorkerId::new();
        run_delegation_to_result(&mut state, &delegation, worker_id, &["src/auth/token.rs"]);
        let duplicate = state.reduce_event(&SessionEvent::DelegationResultRecorded {
            result: Box::new(worker_result(
                &delegation,
                worker_id,
                &["src/auth/token.rs"],
            )),
        });
        assert!(matches!(duplicate, Err(DomainError::DuplicateEvent { .. })));
        // …and the ledger did not double-count.
        assert_eq!(
            state.delegation_ledger.total_worker_usage.input_tokens,
            1_000
        );
    }

    #[test]
    fn a_scope_escaping_result_is_refused_by_the_reducer() {
        let mut state = delegation_session();
        let delegation = admitted_delegation(&["src/auth/**"], delegation::ExpectedOutput::Patch);
        let worker_id = WorkerId::new();
        state
            .reduce_event(&SessionEvent::DelegationCreated {
                delegation: Box::new(delegation.clone()),
            })
            .unwrap();
        state
            .reduce_event(&SessionEvent::DelegationReady {
                delegation_id: delegation.id(),
            })
            .unwrap();
        state
            .reduce_event(&SessionEvent::DelegationWorkerAssigned {
                assignment: Box::new(assignment(&delegation, worker_id)),
            })
            .unwrap();
        state
            .reduce_event(&SessionEvent::DelegationWorkerStarted {
                delegation_id: delegation.id(),
                worker_id,
            })
            .unwrap();
        let escaped = state.reduce_event(&SessionEvent::DelegationResultRecorded {
            result: Box::new(worker_result(
                &delegation,
                worker_id,
                &["src/payments/billing.rs"],
            )),
        });
        assert!(
            matches!(escaped, Err(DomainError::InvalidStateTransition { ref reason, .. })
                if reason.contains("outside its delegated scope")),
            "got {escaped:?}"
        );
    }

    #[test]
    fn a_patch_cannot_be_applied_without_an_approval() {
        // The release gate "all worker changes enter the parent through an
        // IntegrationProposal" is enforced by the log, not by the daemon's
        // control flow.
        let mut state = delegation_session();
        let delegation = admitted_delegation(&["src/auth/**"], delegation::ExpectedOutput::Patch);
        let worker_id = WorkerId::new();
        run_delegation_to_result(&mut state, &delegation, worker_id, &["src/auth/token.rs"]);

        let premature = state.reduce_event(&SessionEvent::IntegrationApplied {
            delegation_id: delegation.id(),
            patch_digest: "patch-digest".into(),
            changed_paths: vec![PathBuf::from("src/auth/token.rs")],
        });
        assert!(matches!(
            premature,
            Err(DomainError::InvalidStateTransition { .. })
        ));
    }

    #[test]
    fn an_unresolved_conflict_cannot_be_approved() {
        let mut state = delegation_session();
        let delegation = admitted_delegation(&["src/auth/**"], delegation::ExpectedOutput::Patch);
        let worker_id = WorkerId::new();
        run_delegation_to_result(&mut state, &delegation, worker_id, &["src/auth/token.rs"]);
        state
            .reduce_event(&SessionEvent::IntegrationProposed {
                proposal: Box::new(delegation::IntegrationProposal {
                    delegation_id: delegation.id(),
                    worker_id,
                    patch_digest: "patch-digest".into(),
                    changed_paths: vec![PathBuf::from("src/auth/token.rs")],
                    base_snapshot_digest: "snapshot".into(),
                    evidence_ids: Vec::new(),
                    validation_summary: Default::default(),
                    conflicts: vec![delegation::IntegrationConflict {
                        path: PathBuf::from("src/auth/token.rs"),
                        kind: delegation::IntegrationConflictKind::SameHunk,
                        delegations: vec![delegation.id()],
                        detail: "overlapping hunks".into(),
                    }],
                    amended_patch_digest: None,
                }),
            })
            .unwrap();
        assert_eq!(
            state.delegations[&delegation.id()].integration,
            delegation::IntegrationState::Conflicted
        );
        let approval = state.reduce_event(&SessionEvent::IntegrationApproved {
            delegation_id: delegation.id(),
            patch_digest: "patch-digest".into(),
            authority: ApprovalAuthority::Human,
        });
        assert!(
            matches!(approval, Err(DomainError::InvalidStateTransition { ref reason, .. })
                if reason.contains("unresolved conflicts")),
            "got {approval:?}"
        );
    }

    #[test]
    fn applying_a_digest_other_than_the_approved_one_is_refused() {
        // A selected-hunk integration carries its own digest; applying anything
        // else means the bytes that landed are not the bytes a human saw.
        let mut state = delegation_session();
        let delegation = admitted_delegation(&["src/auth/**"], delegation::ExpectedOutput::Patch);
        let worker_id = WorkerId::new();
        run_delegation_to_result(&mut state, &delegation, worker_id, &["src/auth/token.rs"]);
        state
            .reduce_event(&SessionEvent::IntegrationProposed {
                proposal: Box::new(delegation::IntegrationProposal {
                    delegation_id: delegation.id(),
                    worker_id,
                    patch_digest: "worker-patch".into(),
                    changed_paths: vec![PathBuf::from("src/auth/token.rs")],
                    base_snapshot_digest: "snapshot".into(),
                    evidence_ids: Vec::new(),
                    validation_summary: Default::default(),
                    conflicts: Vec::new(),
                    amended_patch_digest: Some("selected-hunks".into()),
                }),
            })
            .unwrap();
        state
            .reduce_event(&SessionEvent::IntegrationApproved {
                delegation_id: delegation.id(),
                patch_digest: "selected-hunks".into(),
                authority: ApprovalAuthority::Human,
            })
            .unwrap();
        let wrong = state.reduce_event(&SessionEvent::IntegrationApplied {
            delegation_id: delegation.id(),
            patch_digest: "worker-patch".into(),
            changed_paths: vec![PathBuf::from("src/auth/token.rs")],
        });
        assert!(matches!(
            wrong,
            Err(DomainError::InvalidStateTransition { .. })
        ));
        // The amended digest applies cleanly.
        state
            .reduce_event(&SessionEvent::IntegrationApplied {
                delegation_id: delegation.id(),
                patch_digest: "selected-hunks".into(),
                changed_paths: vec![PathBuf::from("src/auth/token.rs")],
            })
            .unwrap();
    }

    #[test]
    fn a_worker_cannot_start_before_it_has_a_workspace() {
        let mut state = delegation_session();
        let delegation = admitted_delegation(&["src/auth/**"], delegation::ExpectedOutput::Patch);
        state
            .reduce_event(&SessionEvent::DelegationCreated {
                delegation: Box::new(delegation.clone()),
            })
            .unwrap();
        state
            .reduce_event(&SessionEvent::DelegationReady {
                delegation_id: delegation.id(),
            })
            .unwrap();
        let premature = state.reduce_event(&SessionEvent::DelegationWorkerStarted {
            delegation_id: delegation.id(),
            worker_id: WorkerId::new(),
        });
        assert!(matches!(
            premature,
            Err(DomainError::InvalidStateTransition { .. })
        ));
    }

    #[test]
    fn two_writers_cannot_be_assigned_the_same_worktree() {
        let mut state = delegation_session();
        let delegation = admitted_delegation(&["src/auth/**"], delegation::ExpectedOutput::Patch);
        state
            .reduce_event(&SessionEvent::DelegationCreated {
                delegation: Box::new(delegation.clone()),
            })
            .unwrap();
        let mut shared = assignment(&delegation, WorkerId::new());
        shared.workspace.worker_worktree = Some(shared.workspace.parent_worktree.clone());
        let refused = state.reduce_event(&SessionEvent::DelegationWorkerAssigned {
            assignment: Box::new(shared),
        });
        assert!(
            matches!(refused, Err(DomainError::InvalidStateTransition { ref reason, .. })
                if reason.contains("own worktree")),
            "got {refused:?}"
        );
    }

    #[test]
    fn a_paused_worker_stays_running_so_recovery_reconciles_rather_than_reruns() {
        let mut state = delegation_session();
        let delegation = admitted_delegation(&["src/auth/**"], delegation::ExpectedOutput::Patch);
        let worker_id = WorkerId::new();
        state
            .reduce_event(&SessionEvent::DelegationCreated {
                delegation: Box::new(delegation.clone()),
            })
            .unwrap();
        state
            .reduce_event(&SessionEvent::DelegationReady {
                delegation_id: delegation.id(),
            })
            .unwrap();
        state
            .reduce_event(&SessionEvent::DelegationWorkerAssigned {
                assignment: Box::new(assignment(&delegation, worker_id)),
            })
            .unwrap();
        state
            .reduce_event(&SessionEvent::DelegationWorkerStarted {
                delegation_id: delegation.id(),
                worker_id,
            })
            .unwrap();
        state
            .reduce_event(&SessionEvent::DelegationWorkerPaused {
                delegation_id: delegation.id(),
                worker_id,
                reason: "daemon restarted".into(),
            })
            .unwrap();
        let record = &state.delegations[&delegation.id()];
        assert_eq!(record.status(), delegation::DelegationStatus::Running);
        assert_eq!(record.paused_reason.as_deref(), Some("daemon restarted"));
        // Its worktree is retained: an unresolved patch is never silently
        // deleted.
        assert!(record.retained_worktree().is_some());
        assert_eq!(state.running_worker_count(), 1);
    }

    #[test]
    fn repair_cycles_must_be_consecutive_and_bounded() {
        let mut state = delegation_session();
        let delegation = admitted_delegation(&["src/auth/**"], delegation::ExpectedOutput::Patch);
        state
            .reduce_event(&SessionEvent::DelegationCreated {
                delegation: Box::new(delegation.clone()),
            })
            .unwrap();
        // Skipping straight to cycle 2 is refused.
        assert!(
            state
                .reduce_event(&SessionEvent::DelegationRepairRequested {
                    delegation_id: delegation.id(),
                    cycle: 2,
                    reason: "tests failed".into(),
                })
                .is_err()
        );
        state
            .reduce_event(&SessionEvent::DelegationRepairRequested {
                delegation_id: delegation.id(),
                cycle: 1,
                reason: "tests failed".into(),
            })
            .unwrap();
        state
            .reduce_event(&SessionEvent::DelegationRepairRequested {
                delegation_id: delegation.id(),
                cycle: 2,
                reason: "still failing".into(),
            })
            .unwrap();
        // …and there is no third cycle. No infinite multi-agent ping-pong.
        assert!(
            state
                .reduce_event(&SessionEvent::DelegationRepairRequested {
                    delegation_id: delegation.id(),
                    cycle: 3,
                    reason: "again".into(),
                })
                .is_err()
        );
    }

    #[test]
    fn events_for_an_unknown_delegation_are_refused() {
        let mut state = delegation_session();
        let ghost = delegation::DelegationId::new();
        assert!(
            state
                .reduce_event(&SessionEvent::DelegationReady {
                    delegation_id: ghost
                })
                .is_err()
        );
        assert!(
            state
                .reduce_event(&SessionEvent::DelegationWorkerStarted {
                    delegation_id: ghost,
                    worker_id: WorkerId::new(),
                })
                .is_err()
        );
    }

    #[test]
    fn a_v1_4_delegation_event_round_trips_through_serde() {
        let delegation = admitted_delegation(&["src/auth/**"], delegation::ExpectedOutput::Patch);
        let event = SessionEvent::DelegationCreated {
            delegation: Box::new(delegation),
        };
        let json = serde_json::to_string(&event).unwrap();
        let back: SessionEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(back, event);
    }

    #[test]
    fn reducer_accepts_typed_read_proposal_and_executes_through_active() {
        let mut state = SessionState::empty(SessionId::new());
        state
            .reduce_event(&SessionEvent::SessionCreated {
                objective: "scan".into(),
                repository: PathBuf::from("/repo"),
                authority_mode: Default::default(),
            })
            .unwrap();
        state
            .reduce_event(&SessionEvent::WorktreeCreated {
                path: PathBuf::from("/repo/.purrcode/worktrees/session"),
                base_head: "HEAD".into(),
                source_was_dirty: false,
            })
            .unwrap();
        let action_id = ActionId::new();
        state
            .reduce_event(&SessionEvent::ActionProposed {
                action_id,
                action: ProposedAction::RepositoryRead(RepositoryReadAction::GitStatus),
                turn_id: None,
            })
            .unwrap();
        assert!(state.proposed_actions.contains_key(&action_id));
        state
            .reduce_event(&SessionEvent::ExecutionStarted { action_id })
            .unwrap();
        assert_eq!(state.status, SessionStatus::Executing(action_id));
        state
            .reduce_event(&SessionEvent::ExecutionFinished {
                action_id,
                exit_code: Some(0),
                truncated: false,
                sandbox_level: None,
                sandbox_backend: None,
            })
            .unwrap();
        assert_eq!(state.status, SessionStatus::Active);
    }

    #[test]
    fn reducer_rejects_approval_for_unknown_action() {
        let mut state = SessionState::empty(SessionId::new());
        let action_id = ActionId::new();
        let error = state
            .reduce_event(&SessionEvent::ApprovalRecorded {
                action_id,
                authority: ApprovalAuthority::Human,
                action_digest: "digest".into(),
            })
            .unwrap_err();
        assert!(matches!(error, DomainError::UnexpectedApproval { .. }));
    }

    #[test]
    fn reducer_rejects_approval_for_wrong_action_id() {
        let mut state = SessionState::empty(SessionId::new());
        let proposed = ActionId::new();
        let other = ActionId::new();
        state
            .reduce_event(&SessionEvent::ActionProposed {
                action_id: proposed,
                action: ProposedAction::RepositoryRead(RepositoryReadAction::GitStatus),
                turn_id: None,
            })
            .unwrap();
        state
            .reduce_event(&SessionEvent::JudgmentRecorded {
                action_id: proposed,
                decision: JudgmentDecision::RequireApproval {
                    reason: "user review".into(),
                    constraints: ActionConstraints::read_only(PathBuf::from("/repo")),
                },
                turn_id: None,
            })
            .unwrap();
        assert_eq!(state.status, SessionStatus::AwaitingApproval(proposed));
        let error = state
            .reduce_event(&SessionEvent::ApprovalRecorded {
                action_id: other,
                authority: ApprovalAuthority::Human,
                action_digest: "digest".into(),
            })
            .unwrap_err();
        assert!(matches!(error, DomainError::UnexpectedApproval { .. }));
    }

    #[test]
    fn reducer_rejects_transitions_from_terminal_states() {
        let mut state = SessionState::empty(SessionId::new());
        state.reduce_event(&SessionEvent::SessionCompleted).unwrap();
        assert_eq!(state.status, SessionStatus::Completed);
        assert!(
            state
                .reduce_event(&SessionEvent::SessionPaused {
                    reason: "after completed".into(),
                })
                .is_err()
        );
    }

    #[test]
    fn reducer_rejects_execution_started_from_completed() {
        let mut state = SessionState::empty(SessionId::new());
        state.reduce_event(&SessionEvent::SessionCompleted).unwrap();
        assert!(
            state
                .reduce_event(&SessionEvent::ExecutionStarted {
                    action_id: ActionId::new(),
                })
                .is_err()
        );
    }

    #[test]
    fn reducer_rejects_approval_from_active_no_prior_judgment() {
        let mut state = SessionState::empty(SessionId::new());
        let action_id = ActionId::new();
        let error = state
            .reduce_event(&SessionEvent::ApprovalRecorded {
                action_id,
                authority: ApprovalAuthority::Human,
                action_digest: "digest".into(),
            })
            .unwrap_err();
        assert!(matches!(error, DomainError::UnexpectedApproval { .. }));
    }

    #[test]
    fn reducer_approve_after_judgment_requires_approval_returns_to_active() {
        let mut state = SessionState::empty(SessionId::new());
        let action_id = ActionId::new();
        // We need to be in a valid state first
        state
            .reduce_event(&SessionEvent::SessionCreated {
                objective: "test".into(),
                repository: PathBuf::from("/repo"),
                authority_mode: Default::default(),
            })
            .unwrap();
        state
            .reduce_event(&SessionEvent::ActionProposed {
                action_id,
                action: ProposedAction::RepositoryRead(RepositoryReadAction::GitStatus),
                turn_id: None,
            })
            .unwrap();
        state
            .reduce_event(&SessionEvent::JudgmentRecorded {
                action_id,
                decision: JudgmentDecision::RequireApproval {
                    reason: "human review".into(),
                    constraints: ActionConstraints::read_only(PathBuf::from("/repo")),
                },
                turn_id: None,
            })
            .unwrap();
        assert_eq!(state.status, SessionStatus::AwaitingApproval(action_id));
        state
            .reduce_event(&SessionEvent::ApprovalRecorded {
                action_id,
                authority: ApprovalAuthority::Human,
                action_digest: "digest".into(),
            })
            .unwrap();
        assert_eq!(state.status, SessionStatus::Active);
    }

    #[test]
    fn reducer_maintains_event_count_as_exact_replay_position() {
        let mut state = SessionState::empty(SessionId::new());
        assert_eq!(state.event_count, 0);
        state
            .reduce_event(&SessionEvent::SessionCreated {
                objective: "count".into(),
                repository: PathBuf::from("/repo"),
                authority_mode: Default::default(),
            })
            .unwrap();
        assert_eq!(state.event_count, 1);
        state
            .reduce_event(&SessionEvent::SessionCancelled {
                reason: "done".into(),
            })
            .unwrap();
        assert_eq!(state.event_count, 2);
        // After terminal state, further events are rejected and event_count does not advance
        assert!(
            state
                .reduce_event(&SessionEvent::SessionPaused {
                    reason: "after terminal".into(),
                })
                .is_err()
        );
        assert_eq!(
            state.event_count, 2,
            "event_count must not increment on invalid transitions"
        );
    }

    #[test]
    fn agent_bound_reduces_into_selected_agent() {
        let id = SessionId::new();
        let mut state = SessionState::empty(id);
        state
            .reduce_event(&SessionEvent::SessionCreated {
                objective: "review".into(),
                repository: PathBuf::from("/repo"),
                authority_mode: Default::default(),
            })
            .unwrap();
        state
            .reduce_event(&SessionEvent::AgentBound {
                agent: "security-reviewer".into(),
            })
            .unwrap();
        assert_eq!(state.selected_agent.as_deref(), Some("security-reviewer"));
        // Rebinding replaces the previous binding.
        state
            .reduce_event(&SessionEvent::AgentBound {
                agent: "architect".into(),
            })
            .unwrap();
        assert_eq!(state.selected_agent.as_deref(), Some("architect"));
    }

    #[test]
    fn reconstruct_state_from_events_matches_sequential_reduce() {
        let id = SessionId::new();
        let events = vec![
            SessionEvent::SessionCreated {
                objective: "reconstruct".into(),
                repository: PathBuf::from("/repo"),
                authority_mode: Default::default(),
            },
            SessionEvent::ActionProposed {
                action_id: ActionId::new(),
                action: ProposedAction::RepositoryRead(RepositoryReadAction::GitStatus),
                turn_id: None,
            },
            SessionEvent::SessionCompleted,
        ];
        let mut sequential = SessionState::empty(id);
        for event in &events {
            sequential.reduce_event(event).unwrap();
        }
        let reconstructed = reconstruct_state(id, &events).unwrap();
        assert_eq!(sequential.objective, reconstructed.objective);
        assert_eq!(sequential.status, reconstructed.status);
        assert_eq!(sequential.event_count, reconstructed.event_count);
        assert_eq!(
            sequential.proposed_actions.len(),
            reconstructed.proposed_actions.len()
        );
    }

    #[test]
    fn reconstruct_state_rejects_invalid_event_sequence() {
        let id = SessionId::new();
        let events = vec![
            SessionEvent::SessionCreated {
                objective: "reconstruct".into(),
                repository: PathBuf::from("/repo"),
                authority_mode: Default::default(),
            },
            SessionEvent::SessionCompleted,
            SessionEvent::ExecutionStarted {
                action_id: ActionId::new(),
            },
        ];
        assert!(reconstruct_state(id, &events).is_err());
    }

    #[test]
    fn verify_active_transition_matrix_does_not_allow_wildcard() {
        // Active -> Completed requires explicit pair
        assert!(is_valid_transition(
            &SessionStatus::Active,
            &SessionStatus::Completed
        ));
        assert!(is_valid_transition(
            &SessionStatus::Active,
            &SessionStatus::Failed
        ));
        assert!(is_valid_transition(
            &SessionStatus::Active,
            &SessionStatus::Cancelled
        ));
        assert!(is_valid_transition(
            &SessionStatus::Active,
            &SessionStatus::Paused
        ));
        // Active -> Active is not in the matrix (though not practically needed)
        assert!(!is_valid_transition(
            &SessionStatus::Active,
            &SessionStatus::Active
        ));
        assert!(is_valid_transition(
            &SessionStatus::Completed,
            &SessionStatus::Active
        ));
        assert!(!is_valid_transition(
            &SessionStatus::Failed,
            &SessionStatus::Paused
        ));
        assert!(is_valid_transition(
            &SessionStatus::Failed,
            &SessionStatus::Active
        ));
        assert!(is_valid_transition(
            &SessionStatus::Cancelled,
            &SessionStatus::Active
        ));
    }

    #[test]
    fn replay_is_idempotent_across_identical_event_streams() {
        let id = SessionId::new();
        let events = vec![
            SessionEvent::SessionCreated {
                objective: "idempotent".into(),
                repository: PathBuf::from("/repo"),
                authority_mode: Default::default(),
            },
            SessionEvent::WorktreeCreated {
                path: PathBuf::from("/repo/.purrcode/worktrees/test"),
                base_head: "abc123".into(),
                source_was_dirty: false,
            },
            SessionEvent::PlanCreated {
                steps: vec!["step1".into(), "step2".into()],
            },
        ];
        let first = reconstruct_state(id, &events).unwrap();
        let second = reconstruct_state(id, &events).unwrap();
        assert_eq!(first, second);
        assert_eq!(first.event_count, 3);
    }

    #[test]
    fn judgment_recorded_inserts_judgment_and_triggers_status_transition() {
        let mut state = SessionState::empty(SessionId::new());
        let action_id = ActionId::new();
        state
            .reduce_event(&SessionEvent::SessionCreated {
                objective: "test".into(),
                repository: PathBuf::from("/repo"),
                authority_mode: Default::default(),
            })
            .unwrap();
        state
            .reduce_event(&SessionEvent::ActionProposed {
                action_id,
                action: ProposedAction::WriteFile(WriteFileAction {
                    path: PathBuf::from("test.txt"),
                    content: "content".into(),
                    expected_digest: None,
                }),
                turn_id: None,
            })
            .unwrap();
        let decision = JudgmentDecision::RequireApproval {
            reason: "manual approval needed".into(),
            constraints: ActionConstraints::read_only(PathBuf::from("/repo")),
        };
        state
            .reduce_event(&SessionEvent::JudgmentRecorded {
                action_id,
                decision: decision.clone(),
                turn_id: None,
            })
            .unwrap();
        assert!(state.judgments.contains_key(&action_id));
        assert_eq!(state.judgments.get(&action_id), Some(&decision));
        assert_eq!(state.status, SessionStatus::AwaitingApproval(action_id));
    }

    #[test]
    fn execution_finished_from_executing_returns_to_active() {
        let mut state = SessionState::empty(SessionId::new());
        let action_id = ActionId::new();
        state
            .reduce_event(&SessionEvent::SessionCreated {
                objective: "test".into(),
                repository: PathBuf::from("/repo"),
                authority_mode: Default::default(),
            })
            .unwrap();
        state
            .reduce_event(&SessionEvent::ExecutionStarted { action_id })
            .unwrap();
        assert_eq!(state.status, SessionStatus::Executing(action_id));
        state
            .reduce_event(&SessionEvent::ExecutionFinished {
                action_id,
                exit_code: Some(0),
                truncated: false,
                sandbox_level: None,
                sandbox_backend: None,
            })
            .unwrap();
        assert_eq!(state.status, SessionStatus::Active);
    }
}

#[cfg(test)]
mod session_state_tests {
    use super::*;

    #[test]
    fn checkpoint_merge_unions_failed_attempts_for_persistence_across_compactions() {
        let previous = SemanticCheckpoint {
            checkpoint_id: CheckpointId::new(),
            turn_id: TurnId::new(),
            superseded_checkpoint_id: None,
            objective: "fix the parser".into(),
            accepted_requirements: vec![],
            user_constraints: vec![],
            decisions: vec![CheckpointDecision {
                summary: "used regex parser".into(),
                action_id: None,
            }],
            files_inspected: vec![PathBuf::from("src/parser.rs")],
            files_modified: vec![],
            important_symbols: vec![],
            validated_facts: vec!["parser compiles".into()],
            failed_attempts: vec![FailedAttempt {
                action_id: ActionId::new(),
                action_summary: "tried a hand-written parser".into(),
                reason: "too many edge cases".into(),
                judgment: Some("AllowWithConstraints".into()),
            }],
            test_results: vec![],
            unresolved_questions: vec![],
            current_hypothesis: Some("regex covers 90%".into()),
            next_actions: vec![],
            pinned_context: vec![],
        };
        let latest = SemanticCheckpoint {
            checkpoint_id: CheckpointId::new(),
            turn_id: TurnId::new(),
            superseded_checkpoint_id: None,
            objective: String::new(),
            accepted_requirements: vec![],
            user_constraints: vec![],
            decisions: vec![CheckpointDecision {
                summary: "switched to pest".into(),
                action_id: None,
            }],
            files_inspected: vec![PathBuf::from("src/parser.rs"), PathBuf::from("Cargo.toml")],
            files_modified: vec![],
            important_symbols: vec![],
            validated_facts: vec!["pest integration works".into()],
            failed_attempts: vec![FailedAttempt {
                action_id: ActionId::new(),
                action_summary: "tried nom combinator".into(),
                reason: "compile times too high".into(),
                judgment: Some("AllowWithConstraints".into()),
            }],
            test_results: vec![],
            unresolved_questions: vec![],
            current_hypothesis: None,
            next_actions: vec![],
            pinned_context: vec![],
        };
        let merged = SessionState::merge_checkpoint(&previous, &latest);
        // Union
        assert_eq!(
            merged.failed_attempts.len(),
            2,
            "failed_attempts must union across the chain"
        );
        assert!(
            merged
                .failed_attempts
                .iter()
                .any(|f| f.action_summary.contains("hand-written"))
        );
        assert!(
            merged
                .failed_attempts
                .iter()
                .any(|f| f.action_summary.contains("nom"))
        );
        assert_eq!(
            merged.files_inspected.len(),
            2,
            "files_inspected must deduplicate"
        );
        assert_eq!(merged.decisions.len(), 2);
        assert_eq!(merged.validated_facts.len(), 2);
        // Carry-forward
        assert_eq!(merged.objective, "fix the parser");
        assert_eq!(
            merged.current_hypothesis.as_deref(),
            Some("regex covers 90%")
        );
        // Chain identity
        assert_eq!(
            merged.superseded_checkpoint_id,
            Some(previous.checkpoint_id)
        );
    }

    #[test]
    fn checkpoint_merge_unions_accumulated_memory_and_carries_forward_current_state() {
        let previous = SemanticCheckpoint {
            checkpoint_id: CheckpointId::new(),
            turn_id: TurnId::new(),
            superseded_checkpoint_id: None,
            objective: "fix the parser".into(),
            accepted_requirements: vec!["planned: use a real parser combinator".into()],
            user_constraints: vec!["task_mode=build".into()],
            decisions: vec![],
            files_inspected: vec![],
            files_modified: vec![],
            important_symbols: vec!["parser.rs".into()],
            validated_facts: vec![],
            failed_attempts: vec![],
            test_results: vec![],
            unresolved_questions: vec![],
            current_hypothesis: None,
            next_actions: vec!["task[1]: wire the new parser into the CLI".into()],
            pinned_context: vec![],
        };
        let latest = SemanticCheckpoint {
            checkpoint_id: CheckpointId::new(),
            turn_id: TurnId::new(),
            superseded_checkpoint_id: None,
            objective: "fix the parser".into(),
            accepted_requirements: vec!["planned: add a regression test".into()],
            // Manual /compact building through the same path as automatic
            // compaction always populates this from current controls, but
            // the merge rule itself must not assume that — it should carry
            // `previous` forward whenever `latest` genuinely has nothing.
            user_constraints: vec![],
            decisions: vec![],
            files_inspected: vec![],
            files_modified: vec![],
            important_symbols: vec!["cli.rs".into()],
            validated_facts: vec![],
            failed_attempts: vec![],
            test_results: vec![],
            unresolved_questions: vec![],
            current_hypothesis: None,
            // The first next_action is done; the checkpoint now reports a
            // different one. This must NOT accumulate with `previous`'s —
            // next_actions describes what's still open right now, not a
            // log of everything ever queued.
            next_actions: vec!["task[2]: add docs".into()],
            pinned_context: vec![],
        };
        let merged = SessionState::merge_checkpoint(&previous, &latest);

        // Accumulated memory: union, same rule as failed_attempts/files_inspected.
        assert_eq!(merged.accepted_requirements.len(), 2);
        assert!(
            merged
                .accepted_requirements
                .iter()
                .any(|r| r.contains("parser combinator"))
        );
        assert!(
            merged
                .accepted_requirements
                .iter()
                .any(|r| r.contains("regression test"))
        );
        assert_eq!(merged.important_symbols.len(), 2);
        assert!(merged.important_symbols.iter().any(|s| s == "parser.rs"));
        assert!(merged.important_symbols.iter().any(|s| s == "cli.rs"));

        // Current state: latest wins outright when non-empty — no union,
        // no stale entries left sitting next to the fresh one.
        assert_eq!(merged.next_actions, vec!["task[2]: add docs".to_string()]);

        // Current state: falls back to previous only because latest was
        // empty here — this is what a hand-rolled empty manual-compact
        // checkpoint used to wipe permanently before the unified builder.
        assert_eq!(merged.user_constraints, vec!["task_mode=build".to_string()]);
    }
}

#[cfg(test)]
mod expectation_replay_tests {
    use super::*;
    use crate::expectation::{
        ContractChange, ContractRevision, ExpectationClause, ExpectationContract, IntentSource,
        RequirementStatus,
    };
    use crate::work::{AcceptanceCriterion, CriterionId, EvidenceId};

    fn criterion(statement: &str) -> AcceptanceCriterion {
        AcceptanceCriterion {
            id: CriterionId::new(),
            statement: statement.into(),
        }
    }

    /// The agent read "the sidebar is cramped" as "remove the sidebar", did it,
    /// and verified it.
    fn sidebar_session() -> (SessionState, ExpectationContract, work::RequirementId) {
        let mut state = SessionState::empty(SessionId::new());
        let mut contract = ExpectationContract::new("Improve the settings layout");
        let mut clause = ExpectationClause::required(
            "remove the sidebar",
            vec![criterion("the sidebar is gone")],
            IntentSource::new(0, "the sidebar feels really cramped"),
        );
        clause.status = RequirementStatus::Verified {
            evidence: vec![EvidenceId::new()],
        };
        let id = clause.id;
        contract.clauses.push(clause);
        state
            .reduce_event(&SessionEvent::ExpectationContractCreated {
                contract: Box::new(contract.clone()),
            })
            .expect("a valid contract is accepted");
        (state, contract, id)
    }

    #[test]
    fn a_contract_survives_replay() {
        let (state, contract, _) = sidebar_session();
        let replayed = replay(&[SessionEvent::ExpectationContractCreated {
            contract: Box::new(contract),
        }]);
        assert_eq!(state.expectation_contract, replayed.expectation_contract);
        assert!(replayed.expectation_contract.is_some());
    }

    #[test]
    fn a_correction_is_re_derived_on_replay_rather_than_restored() {
        // The crash-recovery half of the sidebar case. After a restart the
        // contract must say the deletion is no longer verified — otherwise the
        // agent resumes believing it already did the right thing.
        let (_, contract, id) = sidebar_session();
        let events = vec![
            SessionEvent::ExpectationContractCreated {
                contract: Box::new(contract),
            },
            SessionEvent::ExpectationContractRevised {
                revision: Box::new(ContractRevision {
                    revision: 2,
                    source: IntentSource::new(1, "no, I didn't mean delete it, it's just cramped"),
                    reason: "the user wants the sidebar kept and made less dense".into(),
                    changes: vec![ContractChange::ClauseRestated {
                        id,
                        from: "remove the sidebar".into(),
                        to: "keep the sidebar and reduce its information density".into(),
                    }],
                }),
            },
        ];
        let replayed = replay(&events);
        let recovered = replayed.expectation_contract.expect("the contract replays");
        assert_eq!(recovered.revision, 2);
        assert_eq!(
            recovered.clause(id).unwrap().statement,
            "keep the sidebar and reduce its information density"
        );
        assert_eq!(
            recovered.clause(id).unwrap().status,
            RequirementStatus::Unverified,
            "a restart must not resurrect evidence the correction invalidated"
        );
        assert_eq!(recovered.tally().settled(), 0);
    }

    #[test]
    fn a_requirement_cannot_be_verified_into_the_log_without_evidence() {
        let (mut state, _, id) = sidebar_session();
        let error = state
            .reduce_event(&SessionEvent::RequirementStatusChanged {
                requirement_id: id,
                status: RequirementStatus::Verified { evidence: vec![] },
                source: "alignment reviewer".into(),
            })
            .unwrap_err();
        assert!(
            format!("{error}").contains("no evidence"),
            "the log must refuse an unevidenced pass: {error}"
        );
    }

    #[test]
    fn a_status_change_for_a_requirement_outside_the_contract_is_refused() {
        let (mut state, _, _) = sidebar_session();
        let error = state
            .reduce_event(&SessionEvent::RequirementStatusChanged {
                requirement_id: work::RequirementId::new(),
                status: RequirementStatus::Unknown {
                    detail: "could not tell".into(),
                },
                source: "alignment reviewer".into(),
            })
            .unwrap_err();
        assert!(
            format!("{error}").contains("not in the contract"),
            "{error}"
        );
    }

    #[test]
    fn a_second_contract_cannot_quietly_replace_the_first() {
        // Intent changes through corrections, which keep the history. A fresh
        // contract would drop it and nothing would show that it had.
        let (mut state, _, _) = sidebar_session();
        let error = state
            .reduce_event(&SessionEvent::ExpectationContractCreated {
                contract: Box::new(ExpectationContract::new("something else entirely")),
            })
            .unwrap_err();
        assert!(
            format!("{error}").contains("already has an expectation contract"),
            "{error}"
        );
    }

    #[test]
    fn a_correction_to_a_session_with_no_contract_is_refused() {
        let mut state = SessionState::empty(SessionId::new());
        let error = state
            .reduce_event(&SessionEvent::ExpectationContractRevised {
                revision: Box::new(ContractRevision {
                    revision: 2,
                    source: IntentSource::new(1, "keep the sidebar"),
                    reason: "correction".into(),
                    changes: vec![],
                }),
            })
            .unwrap_err();
        assert!(format!("{error}").contains("never created"), "{error}");
    }

    #[test]
    fn an_out_of_order_correction_is_refused_by_the_log() {
        let (mut state, _, _) = sidebar_session();
        let error = state
            .reduce_event(&SessionEvent::ExpectationContractRevised {
                revision: Box::new(ContractRevision {
                    revision: 7,
                    source: IntentSource::new(1, "keep the sidebar"),
                    reason: "stale correction".into(),
                    changes: vec![],
                }),
            })
            .unwrap_err();
        assert!(format!("{error}").contains("does not follow"), "{error}");
    }

    fn replay(events: &[SessionEvent]) -> SessionState {
        let mut state = SessionState::empty(SessionId::new());
        for event in events {
            state.reduce_event(event).expect("replay applies");
        }
        state
    }
}

#[cfg(test)]
mod delivery_replay_tests {
    use super::*;
    use crate::correction::{CorrectionAllowance, CorrectionLedger};
    use crate::expectation::{
        DeliveryAssessment, DeliveryBlocker, DeliveryState, ExpectationClause, ExpectationContract,
        IntentSource, RequirementStatus, RequirementTally,
    };
    use crate::review::{
        FindingCategory, FindingId, ReviewContext, ReviewFinding, ReviewId, ReviewKind,
        ReviewRecord, Severity,
    };
    use crate::work::{AcceptanceCriterion, CriterionId, EvidenceId};

    fn contract_session() -> (SessionState, work::RequirementId) {
        let mut state = SessionState::empty(SessionId::new());
        let mut contract = ExpectationContract::new("Improve the Settings experience");
        let clause = ExpectationClause::required(
            "MCP configuration works",
            vec![AcceptanceCriterion {
                id: CriterionId::new(),
                statement: "a user can add and remove a server".into(),
            }],
            IntentSource::new(0, "MCP must actually work"),
        );
        let id = clause.id;
        contract.clauses.push(clause);
        state
            .reduce_event(&SessionEvent::ExpectationContractCreated {
                contract: Box::new(contract),
            })
            .unwrap();
        (state, id)
    }

    fn review(kind: ReviewKind, context: ReviewContext) -> ReviewRecord {
        ReviewRecord {
            id: ReviewId::new(),
            kind,
            context,
            cycle: 0,
            completed: false,
            findings: vec![],
        }
    }

    fn finding(review: ReviewId, severity: Severity) -> ReviewFinding {
        ReviewFinding {
            id: FindingId::new(),
            review,
            kind: ReviewKind::IndependentCode,
            severity,
            category: FindingCategory::Correctness,
            requirement_id: None,
            description: "the daemon never reloads the registry".into(),
            evidence: vec!["mcp_host.rs:83".into()],
            affected_paths: vec![],
            recommendation: "reload after a config write".into(),
        }
    }

    #[test]
    fn the_log_refuses_a_ready_verdict_that_still_has_blockers() {
        // The single guarantee behind "completion is a state transition, not
        // something the model declares". However the assessment was produced,
        // a Ready carrying blockers is a false claim and does not enter the log.
        let (mut state, id) = contract_session();
        let error = state
            .reduce_event(&SessionEvent::DeliveryGateEvaluated {
                assessment: Box::new(DeliveryAssessment {
                    state: DeliveryState::Ready,
                    blockers: vec![DeliveryBlocker::RequirementOutstanding {
                        id,
                        statement: "MCP configuration works".into(),
                    }],
                    tally: RequirementTally::default(),
                    advisory_findings: 0,
                }),
            })
            .unwrap_err();
        assert!(
            format!("{error}").contains("cannot be ready"),
            "got {error}"
        );
    }

    #[test]
    fn a_genuine_ready_verdict_is_recorded_and_replays() {
        let (mut state, _) = contract_session();
        let assessment = DeliveryAssessment {
            state: DeliveryState::Ready,
            blockers: vec![],
            tally: RequirementTally {
                verified: 1,
                ..RequirementTally::default()
            },
            advisory_findings: 2,
        };
        state
            .reduce_event(&SessionEvent::DeliveryGateEvaluated {
                assessment: Box::new(assessment.clone()),
            })
            .unwrap();
        assert_eq!(state.delivery, Some(assessment));
        assert!(state.delivery.unwrap().state.may_report_done());
    }

    #[test]
    fn a_contaminated_review_cannot_enter_the_log() {
        let (mut state, _) = contract_session();
        let error = state
            .reduce_event(&SessionEvent::ReviewStarted {
                record: Box::new(review(ReviewKind::UserAlignment, ReviewContext::Inherited)),
            })
            .unwrap_err();
        assert!(format!("{error}").contains("transcript"), "got {error}");
    }

    #[test]
    fn a_finding_must_belong_to_a_review_that_started() {
        let (mut state, _) = contract_session();
        let error = state
            .reduce_event(&SessionEvent::ReviewFindingRecorded {
                finding: Box::new(finding(ReviewId::new(), Severity::High)),
            })
            .unwrap_err();
        assert!(
            format!("{error}").contains("review that started"),
            "{error}"
        );
    }

    #[test]
    fn a_finding_cannot_name_a_requirement_the_contract_does_not_have() {
        let (mut state, _) = contract_session();
        let record = review(ReviewKind::IndependentCode, ReviewContext::Fresh);
        let review_id = record.id;
        state
            .reduce_event(&SessionEvent::ReviewStarted {
                record: Box::new(record),
            })
            .unwrap();
        let mut claim = finding(review_id, Severity::High);
        claim.category = FindingCategory::RequirementGap;
        claim.requirement_id = Some(work::RequirementId::new());
        let error = state
            .reduce_event(&SessionEvent::ReviewFindingRecorded {
                finding: Box::new(claim),
            })
            .unwrap_err();
        assert!(
            format!("{error}").contains("not in the contract"),
            "{error}"
        );
    }

    #[test]
    fn findings_and_reviews_survive_replay_and_stay_linked() {
        let (mut state, _) = contract_session();
        let record = review(ReviewKind::IndependentCode, ReviewContext::Fresh);
        let review_id = record.id;
        let claim = finding(review_id, Severity::High);
        let finding_id = claim.id;
        for event in [
            SessionEvent::ReviewStarted {
                record: Box::new(record),
            },
            SessionEvent::ReviewFindingRecorded {
                finding: Box::new(claim),
            },
            SessionEvent::ReviewCompleted { review: review_id },
        ] {
            state.reduce_event(&event).unwrap();
        }
        assert!(state.reviews[&review_id].completed);
        assert_eq!(state.reviews[&review_id].findings, vec![finding_id]);
        assert!(state.findings[&finding_id].blocks_delivery());
    }

    #[test]
    fn correction_cycles_cannot_skip_ahead_of_the_budget() {
        // The bound is the whole point; a cycle that jumps the count would
        // spend the budget without recording that it had.
        let (mut state, _) = contract_session();
        let error = state
            .reduce_event(&SessionEvent::CorrectionStarted {
                cycle: 3,
                findings: vec![],
            })
            .unwrap_err();
        assert!(format!("{error}").contains("does not follow"), "{error}");
    }

    #[test]
    fn a_correction_cycle_replays_into_the_ledger_and_the_budget_shrinks() {
        let (mut state, _) = contract_session();
        let record = review(ReviewKind::IndependentCode, ReviewContext::Fresh);
        let review_id = record.id;
        let claim = finding(review_id, Severity::High);
        let finding_id = claim.id;
        state
            .reduce_event(&SessionEvent::ReviewStarted {
                record: Box::new(record),
            })
            .unwrap();
        state
            .reduce_event(&SessionEvent::ReviewFindingRecorded {
                finding: Box::new(claim),
            })
            .unwrap();

        for cycle in 1..=2 {
            state
                .reduce_event(&SessionEvent::CorrectionStarted {
                    cycle,
                    findings: vec![finding_id],
                })
                .unwrap();
            state
                .reduce_event(&SessionEvent::CorrectionCompleted {
                    cycle,
                    repaired: vec![],
                    still_open: vec![finding_id],
                })
                .unwrap();
        }
        assert_eq!(state.correction_ledger.cycles_used, 2);

        // After a restart the budget is still spent — the loop does not get a
        // fresh allowance by crashing.
        let outstanding: Vec<&ReviewFinding> = state.findings.values().collect();
        assert_eq!(
            state.correction_ledger.may_correct(&outstanding),
            CorrectionAllowance::Exhausted {
                cycles_used: 2,
                allowed: 2
            }
        );
        assert_eq!(
            CorrectionLedger::default().may_correct(&outstanding),
            CorrectionAllowance::Allowed { cycle: 1 },
            "a fresh ledger would have allowed one, which is what recovery must not do"
        );
    }

    #[test]
    fn a_repaired_finding_stops_counting_against_the_correction_budget() {
        // Without `outstanding_findings`, the natural thing to write is
        // `state.findings.values()` — which is the full history and never
        // shrinks. The loop would then spend its whole budget re-fixing
        // something already fixed and hand the user a `NeedsAttention` task
        // with nothing wrong with it.
        let (mut state, _) = contract_session();
        let record = review(ReviewKind::Deterministic, ReviewContext::Inherited);
        let review_id = record.id;
        let claim = finding(review_id, Severity::Critical);
        let finding_id = claim.id;
        state
            .reduce_event(&SessionEvent::ReviewStarted {
                record: Box::new(record),
            })
            .unwrap();
        state
            .reduce_event(&SessionEvent::ReviewFindingRecorded {
                finding: Box::new(claim),
            })
            .unwrap();
        assert_eq!(state.outstanding_findings().len(), 1);
        assert_eq!(state.blocking_findings().len(), 1);

        state
            .reduce_event(&SessionEvent::CorrectionStarted {
                cycle: 1,
                findings: vec![finding_id],
            })
            .unwrap();
        state
            .reduce_event(&SessionEvent::CorrectionCompleted {
                cycle: 1,
                repaired: vec![finding_id],
                still_open: vec![],
            })
            .unwrap();

        assert_eq!(
            state.findings.len(),
            1,
            "the finding stays in the record; the history is not rewritten"
        );
        assert!(
            state.outstanding_findings().is_empty(),
            "but it is no longer outstanding"
        );
        assert_eq!(
            state
                .correction_ledger
                .may_correct(&state.outstanding_findings()),
            CorrectionAllowance::NothingToFix,
            "so no further cycle is proposed"
        );
    }

    #[test]
    fn a_verified_requirement_and_a_clean_gate_agree_after_replay() {
        let (mut state, id) = contract_session();
        state
            .reduce_event(&SessionEvent::RequirementStatusChanged {
                requirement_id: id,
                status: RequirementStatus::Verified {
                    evidence: vec![EvidenceId::new()],
                },
                source: "requirement coverage review".into(),
            })
            .unwrap();
        let contract = state.expectation_contract.clone().unwrap();
        let assessment =
            crate::expectation::delivery::evaluate(crate::expectation::DeliveryInputs {
                contract: &contract,
                findings: &[],
                validations: &[],
                changed_paths: &[],
                forbidden_prefixes: &[],
                unresolved_conflicts: &[],
            });
        assert_eq!(assessment.state, DeliveryState::Ready);
        state
            .reduce_event(&SessionEvent::DeliveryGateEvaluated {
                assessment: Box::new(assessment),
            })
            .expect("a gate result derived from the contract is accepted");
    }
}

#[cfg(test)]
mod gate_wiring_tests {
    use super::*;
    use crate::expectation::{
        DeliveryInputs, DeliveryState, ExpectationClause, ExpectationContract, IntentSource,
        RequirementStatus, delivery,
    };
    use crate::review::{
        FindingCategory, FindingId, ReviewContext, ReviewFinding, ReviewId, ReviewKind,
        ReviewRecord, Severity,
    };
    use crate::work::{AcceptanceCriterion, CriterionId, EvidenceId};

    /// A session whose only requirement is verified, with one blocking finding
    /// that a correction cycle then repairs.
    fn repaired_session() -> SessionState {
        let mut state = SessionState::empty(SessionId::new());
        let mut contract = ExpectationContract::new("Improve the Settings experience");
        let mut clause = ExpectationClause::required(
            "MCP configuration works",
            vec![AcceptanceCriterion {
                id: CriterionId::new(),
                statement: "a user can add and remove a server".into(),
            }],
            IntentSource::new(0, "MCP must actually work"),
        );
        clause.status = RequirementStatus::Verified {
            evidence: vec![EvidenceId::new()],
        };
        contract.clauses.push(clause);
        state
            .reduce_event(&SessionEvent::ExpectationContractCreated {
                contract: Box::new(contract),
            })
            .unwrap();

        let record = ReviewRecord {
            id: ReviewId::new(),
            kind: ReviewKind::Deterministic,
            context: ReviewContext::Inherited,
            cycle: 0,
            completed: true,
            findings: vec![],
        };
        let review_id = record.id;
        let claim = ReviewFinding {
            id: FindingId::new(),
            review: review_id,
            kind: ReviewKind::Deterministic,
            severity: Severity::Critical,
            category: FindingCategory::Correctness,
            requirement_id: None,
            description: "the daemon never reloads the registry".into(),
            evidence: vec!["mcp_host.rs:83".into()],
            affected_paths: vec![],
            recommendation: "reload after a config write".into(),
        };
        let finding_id = claim.id;
        state
            .reduce_event(&SessionEvent::ReviewStarted {
                record: Box::new(record),
            })
            .unwrap();
        state
            .reduce_event(&SessionEvent::ReviewFindingRecorded {
                finding: Box::new(claim),
            })
            .unwrap();
        state
            .reduce_event(&SessionEvent::CorrectionStarted {
                cycle: 1,
                findings: vec![finding_id],
            })
            .unwrap();
        state
            .reduce_event(&SessionEvent::CorrectionCompleted {
                cycle: 1,
                repaired: vec![finding_id],
                still_open: vec![],
            })
            .unwrap();
        state
    }

    #[test]
    fn a_repaired_finding_stops_blocking_delivery() {
        // The whole loop, end to end: a blocking finding is raised, repaired,
        // and the gate then clears. Feeding the gate the full findings history
        // instead of the outstanding set would leave this task blocked forever
        // on a problem that was fixed.
        let state = repaired_session();
        let contract = state.expectation_contract.clone().unwrap();
        let outstanding: Vec<ReviewFinding> =
            state.outstanding_findings().into_iter().cloned().collect();
        let assessment = delivery::evaluate(DeliveryInputs {
            contract: &contract,
            findings: &outstanding,
            validations: &[],
            changed_paths: &[],
            forbidden_prefixes: &[],
            unresolved_conflicts: &[],
        });
        assert_eq!(assessment.state, DeliveryState::Ready);
        assert!(assessment.blockers.is_empty());

        // And the history is still there, so the record of what was found and
        // fixed does not disappear from the session.
        assert_eq!(state.findings.len(), 1);
        assert_eq!(state.correction_ledger.repaired.len(), 1);

        // The gate result is consistent enough for the log to accept it.
        let mut state = state;
        state
            .reduce_event(&SessionEvent::DeliveryGateEvaluated {
                assessment: Box::new(assessment),
            })
            .expect("a ready verdict with no blockers is accepted");
        assert!(state.delivery.unwrap().state.may_report_done());
    }

    #[test]
    fn the_whole_history_would_have_blocked_it_forever() {
        // Demonstrates why `outstanding_findings` exists rather than being a
        // convenience: passing `findings` directly is the natural mistake, and
        // it is silent.
        let state = repaired_session();
        let contract = state.expectation_contract.clone().unwrap();
        let everything: Vec<ReviewFinding> = state.findings.values().cloned().collect();
        let assessment = delivery::evaluate(DeliveryInputs {
            contract: &contract,
            findings: &everything,
            validations: &[],
            changed_paths: &[],
            forbidden_prefixes: &[],
            unresolved_conflicts: &[],
        });
        assert_eq!(
            assessment.state,
            DeliveryState::Blocked,
            "the repaired finding still blocks when the caller passes the full history"
        );
    }
}
