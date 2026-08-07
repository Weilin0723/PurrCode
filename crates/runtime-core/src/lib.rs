//! Provider-independent domain contracts for the trusted runtime.

pub mod adaptation;
pub mod authority;
pub mod product_state;
pub mod terminal;
pub mod work;

pub use authority::{
    AuthenticationChannel, AuthorityMode, GrantCapability, GrantId, HumanAuthorityGrant,
    HumanIdentity,
};
pub use product_state::{InputDisposition, ProductState, ProductStateView, StateColor};
pub use terminal::{
    OwnershipGeneration, OwnershipTransition, ResizeTerminalAction, SendTerminalInputAction,
    StartTerminalAction, StopProcessAction, TerminalAction, TerminalDimensions, TerminalId,
    TerminalInput, TerminalOwner, TerminalSessionRecord, TerminalStatus, TranscriptPolicy,
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
        }
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
            _ => {}
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
            _ => {}
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
