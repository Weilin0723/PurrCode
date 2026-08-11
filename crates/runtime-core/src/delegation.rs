//! Typed delegation contracts (v1.4 §5.2, §6).
//!
//! v1.3 made PurrCode extensible; v1.4 lets those extensions cooperate without
//! giving up any of the guarantees that made them safe. Two invariants carry
//! the whole release, and both are structural here rather than conventional:
//!
//! 1. **Delegation must never create more authority than the parent already
//!    has.** A [`Delegation`] has no public constructor. The only way to obtain
//!    one is [`DelegationRequest::admit`], which folds
//!    `workspace ∩ parent ∩ profile` through [`ToolCeiling::meet`] and then
//!    *narrows* — never widens — that result to the delegated path set. A
//!    delegation carrying authority nobody granted cannot be built.
//!
//! 2. **Workers propose; evidence proves; the parent integrates.** A
//!    [`WorkerResult`] is a proposal, not a mutation: it names changed paths and
//!    a patch digest, and [`WorkerResult::validate_against`] refuses one that
//!    escaped its path scope or its changed-file budget before any integration
//!    logic runs.
//!
//! Everything in this module is pure value: no I/O, no tokio, no git. The
//! runtime that creates worktrees, schedules workers and applies patches lives
//! in `purrcode-delegation-runtime`; it can only express decisions these types
//! already permit.

pub mod decision;
pub mod integration;

use crate::work::{AcceptanceCriterion, EvidenceId};
use crate::{
    CapabilityId, FilesystemScope, ModelRoleName, SessionId, SideEffectClass, ToolCeiling, TurnId,
    ValidationStatus,
};
use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};
use thiserror::Error;
use uuid::Uuid;

pub use decision::{DelegationClassification, DelegationPlan, DelegationSignals, PlannedUnit};
pub use integration::{
    IntegrationConflict, IntegrationConflictKind, IntegrationDecision, IntegrationProposal,
    PatchHunk, detect_conflicts, parse_unified_diff,
};

macro_rules! delegation_id {
    ($name:ident, $prefix:literal) => {
        #[derive(
            Clone,
            Copy,
            Debug,
            Eq,
            Hash,
            JsonSchema,
            Ord,
            PartialEq,
            PartialOrd,
            Serialize,
            Deserialize,
        )]
        #[serde(transparent)]
        pub struct $name(pub Uuid);

        impl $name {
            pub fn new() -> Self {
                Self(Uuid::new_v4())
            }

            /// Short, stable display form for worker trees and log lines.
            pub fn short(&self) -> String {
                format!("{}-{}", $prefix, &self.0.simple().to_string()[..8])
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "{}", self.0)
            }
        }

        impl std::str::FromStr for $name {
            type Err = uuid::Error;

            fn from_str(raw: &str) -> Result<Self, Self::Err> {
                Ok(Self(Uuid::parse_str(raw)?))
            }
        }
    };
}

delegation_id!(DelegationId, "dlg");
delegation_id!(WorkerId, "wrk");

// ---------------------------------------------------------------------------
// Errors.
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Error, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
pub enum DelegationError {
    #[error("delegation objective must not be empty")]
    EmptyObjective,
    #[error("path pattern `{pattern}` is not repository-relative: {reason}")]
    UnsafePathPattern { pattern: String, reason: String },
    #[error(
        "delegation would need authority the parent does not have on the {axis} axis \
         (requested {requested}, parent ceiling {permitted})"
    )]
    AuthorityEscalation {
        axis: String,
        requested: String,
        permitted: String,
    },
    #[error(
        "child budget exceeds the parent's remaining budget on {axis}: \
         requested {requested}, remaining {remaining}"
    )]
    BudgetExceedsParent {
        axis: String,
        requested: u64,
        remaining: u64,
    },
    #[error("a writer delegation was left with no writable path inside the parent's scope")]
    NoWritablePathRemains,
    #[error(
        "a delegation expecting a patch cannot be created under a read-only parent; \
         the worker would have no write authority to produce one"
    )]
    WriterUnderReadOnlyParent,
    #[error("delegation depth {depth} exceeds the v1.4 maximum of {maximum}")]
    DepthExceeded { depth: u8, maximum: u8 },
    #[error("worker wrote outside its delegated scope: {path}")]
    ScopeEscape { path: String },
    #[error("worker changed {changed} files but its budget allows {permitted}")]
    ChangedFileBudgetExceeded { changed: usize, permitted: usize },
    #[error("a read-only delegation reported {changed} changed paths")]
    ReadOnlyWorkerMutated { changed: usize },
    #[error("finding `{title}` has no evidence provenance")]
    FindingWithoutProvenance { title: String },
    #[error("a completed worker result must carry a patch digest or declare no changes")]
    MissingPatchDigest,
    #[error("worker result belongs to delegation {actual}, not {expected}")]
    ResultDelegationMismatch {
        expected: DelegationId,
        actual: DelegationId,
    },
    #[error("delegation cannot move from {from:?} to {to:?}")]
    IllegalTransition {
        from: DelegationStatus,
        to: DelegationStatus,
    },
    #[error("no provider is registered for capability `{capability}`")]
    CapabilityUnavailable { capability: String },
    #[error("delegation dependency graph is invalid: {reason}")]
    InvalidDependencyGraph { reason: String },
}

// ---------------------------------------------------------------------------
// Path patterns.
// ---------------------------------------------------------------------------

/// A repository-relative write scope for one delegation.
///
/// Validated on construction: absolute paths, `..` traversal, Windows drive
/// prefixes and bare `~` are all rejected, so `allowed_paths` can never name
/// something outside the worker's worktree — one of PR1's acceptance tests.
#[derive(Clone, Debug, Eq, Hash, JsonSchema, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct PathPattern(String);

impl PathPattern {
    pub fn parse(raw: &str) -> Result<Self, DelegationError> {
        let pattern = raw.trim();
        let unsafe_pattern = |reason: &str| DelegationError::UnsafePathPattern {
            pattern: pattern.to_owned(),
            reason: reason.to_owned(),
        };
        if pattern.is_empty() {
            return Err(unsafe_pattern("pattern is empty"));
        }
        // Normalize separators before inspecting so a Windows-style pattern is
        // judged by the same rules as a POSIX one.
        let normalized = pattern.replace('\\', "/");
        if normalized.starts_with('/') {
            return Err(unsafe_pattern("pattern is absolute"));
        }
        if normalized.starts_with('~') {
            return Err(unsafe_pattern("pattern starts at a home directory"));
        }
        if normalized.len() >= 2 && normalized.as_bytes()[1] == b':' {
            return Err(unsafe_pattern("pattern carries a drive prefix"));
        }
        if normalized.split('/').any(|segment| segment == "..") {
            return Err(unsafe_pattern("pattern traverses above the worktree"));
        }
        if normalized.contains('\0') {
            return Err(unsafe_pattern("pattern contains a NUL byte"));
        }
        Ok(Self(normalized))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// True when `path` (repository-relative) falls inside this pattern.
    ///
    /// Matching is the same conservative syntactic rule
    /// [`crate::tool::glob_covers`] uses, so a scope check here and a ceiling
    /// check there can never disagree about the same pair of strings.
    pub fn matches(&self, path: &Path) -> bool {
        let Some(candidate) = path.to_str() else {
            // A non-UTF-8 path cannot be shown to have been permitted.
            return false;
        };
        let candidate = candidate.replace('\\', "/");
        crate::tool::glob_covers(&self.0, &candidate)
    }
}

impl std::fmt::Display for PathPattern {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for PathPattern {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        PathPattern::parse(&raw).map_err(serde::de::Error::custom)
    }
}

/// True when a repository-relative path stays inside the worktree.
pub fn path_is_contained(path: &Path) -> bool {
    !path.is_absolute()
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_) | Component::CurDir))
}

// ---------------------------------------------------------------------------
// Status.
// ---------------------------------------------------------------------------

/// Lifecycle of one delegation (v1.4 §6.1).
#[derive(
    Clone, Copy, Debug, Eq, JsonSchema, Ord, PartialEq, PartialOrd, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum DelegationStatus {
    /// The planner proposed it; nothing has been assigned.
    Planned,
    /// Dependencies satisfied, authority resolved, awaiting a worker slot.
    Ready,
    /// A worker is executing.
    Running,
    /// The worker produced a result that needs a human decision.
    AwaitingApproval,
    /// A dependency failed or was cancelled, so this will not run.
    Blocked,
    Completed,
    Failed,
    Cancelled,
    /// Replaced by a later delegation (a repair, or a re-plan).
    Superseded,
}

impl DelegationStatus {
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            DelegationStatus::Completed
                | DelegationStatus::Failed
                | DelegationStatus::Cancelled
                | DelegationStatus::Superseded
                | DelegationStatus::Blocked
        )
    }

    pub fn is_live(self) -> bool {
        !self.is_terminal()
    }

    /// Legal transitions. The reducer consults this so a replayed or duplicated
    /// event cannot resurrect a finished worker — v1.4 §PR10's "completed
    /// workers are NEVER rerun automatically" is enforced here, not by
    /// convention at the call sites.
    pub fn can_transition_to(self, next: DelegationStatus) -> bool {
        use DelegationStatus::*;
        match (self, next) {
            // Terminal states are terminal, with one exception: anything may be
            // superseded, because superseding is how the parent records that a
            // *new* delegation replaces this record without rewriting history.
            (from, Superseded) => from != Superseded,
            (from, _) if from.is_terminal() => false,
            (Planned, Ready | Blocked | Cancelled | Failed) => true,
            (Ready, Running | Blocked | Cancelled | Failed) => true,
            (Running, AwaitingApproval | Completed | Failed | Cancelled) => true,
            (AwaitingApproval, Completed | Failed | Cancelled | Running) => true,
            _ => false,
        }
    }
}

// ---------------------------------------------------------------------------
// Budget and usage.
// ---------------------------------------------------------------------------

/// One delegation's resource envelope (v1.4 §6.3).
///
/// A worker budget is a *subdivision* of the parent's remaining budget, never an
/// independent one: [`DelegationBudget::clamp_to_remaining`] is the only way a
/// budget reaches an admitted delegation.
#[derive(Clone, Copy, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct DelegationBudget {
    pub maximum_input_tokens: u64,
    pub maximum_output_tokens: u64,
    pub maximum_tool_calls: u32,
    pub maximum_duration_seconds: u64,
    pub maximum_changed_files: usize,
}

impl DelegationBudget {
    /// A conservative default for one specialist: enough for a bounded change,
    /// small enough that three of them cannot silently triple a session's cost.
    pub fn modest() -> Self {
        Self {
            maximum_input_tokens: 120_000,
            maximum_output_tokens: 32_000,
            maximum_tool_calls: 60,
            maximum_duration_seconds: 900,
            maximum_changed_files: 25,
        }
    }

    /// A read-only reviewer needs no write budget and far fewer tool calls.
    pub fn review() -> Self {
        Self {
            maximum_input_tokens: 80_000,
            maximum_output_tokens: 16_000,
            maximum_tool_calls: 40,
            maximum_duration_seconds: 600,
            maximum_changed_files: 0,
        }
    }

    /// Clamp every axis down to what the parent has left. Never raises a value,
    /// so a planner asking for more than the session can afford gets less rather
    /// than an escalation.
    pub fn clamp_to_remaining(self, remaining: &DelegationBudget) -> Self {
        Self {
            maximum_input_tokens: self
                .maximum_input_tokens
                .min(remaining.maximum_input_tokens),
            maximum_output_tokens: self
                .maximum_output_tokens
                .min(remaining.maximum_output_tokens),
            maximum_tool_calls: self.maximum_tool_calls.min(remaining.maximum_tool_calls),
            maximum_duration_seconds: self
                .maximum_duration_seconds
                .min(remaining.maximum_duration_seconds),
            maximum_changed_files: self
                .maximum_changed_files
                .min(remaining.maximum_changed_files),
        }
    }

    /// True when every axis is within `remaining`.
    pub fn fits_within(&self, remaining: &DelegationBudget) -> bool {
        self.maximum_input_tokens <= remaining.maximum_input_tokens
            && self.maximum_output_tokens <= remaining.maximum_output_tokens
            && self.maximum_tool_calls <= remaining.maximum_tool_calls
            && self.maximum_duration_seconds <= remaining.maximum_duration_seconds
            && self.maximum_changed_files <= remaining.maximum_changed_files
    }

    /// True when no axis has room left for useful work. The scheduler refuses to
    /// admit a new delegation once this holds (v1.4 §PR14).
    pub fn is_exhausted(&self) -> bool {
        self.maximum_input_tokens == 0
            || self.maximum_output_tokens == 0
            || self.maximum_tool_calls == 0
            || self.maximum_duration_seconds == 0
    }

    /// What is left of this budget after `spent`. Saturating: a worker that
    /// overran reports zero remaining, never a wrapped-around allowance.
    pub fn remaining_after(&self, spent: &UsageSummary) -> DelegationBudget {
        DelegationBudget {
            maximum_input_tokens: self.maximum_input_tokens.saturating_sub(spent.input_tokens),
            maximum_output_tokens: self
                .maximum_output_tokens
                .saturating_sub(spent.output_tokens),
            maximum_tool_calls: self.maximum_tool_calls.saturating_sub(spent.tool_calls),
            maximum_duration_seconds: self
                .maximum_duration_seconds
                .saturating_sub(spent.duration_seconds),
            maximum_changed_files: self
                .maximum_changed_files
                .saturating_sub(spent.changed_files),
        }
    }
}

/// What a worker actually consumed. Parent usage includes child usage
/// (v1.4 §6.3), so this is summed up the tree, never kept per-worker only.
#[derive(Clone, Copy, Debug, Default, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct UsageSummary {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub tool_calls: u32,
    pub model_calls: u32,
    pub duration_seconds: u64,
    pub changed_files: usize,
}

impl UsageSummary {
    pub fn saturating_add(self, other: &UsageSummary) -> UsageSummary {
        UsageSummary {
            input_tokens: self.input_tokens.saturating_add(other.input_tokens),
            output_tokens: self.output_tokens.saturating_add(other.output_tokens),
            tool_calls: self.tool_calls.saturating_add(other.tool_calls),
            model_calls: self.model_calls.saturating_add(other.model_calls),
            duration_seconds: self.duration_seconds.saturating_add(other.duration_seconds),
            changed_files: self.changed_files.saturating_add(other.changed_files),
        }
    }

    /// True when this usage has already met or passed `budget` on any axis.
    pub fn exceeds(&self, budget: &DelegationBudget) -> bool {
        self.input_tokens > budget.maximum_input_tokens
            || self.output_tokens > budget.maximum_output_tokens
            || self.tool_calls > budget.maximum_tool_calls
            || self.duration_seconds > budget.maximum_duration_seconds
            || self.changed_files > budget.maximum_changed_files
    }
}

// ---------------------------------------------------------------------------
// Workspace access and expected output.
// ---------------------------------------------------------------------------

/// Whether a worker may modify anything at all (v1.4 §PR3).
///
/// Reviewer and investigator workers get [`WorkspaceAccess::ReadOnly`] — a view
/// of the parent snapshot, *not* a worktree of their own. Creating a second
/// worktree because it is convenient would hand a reviewer write authority it
/// was never delegated.
#[derive(
    Clone, Copy, Debug, Eq, JsonSchema, Ord, PartialEq, PartialOrd, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceAccess {
    ReadOnly,
    Writable,
}

impl WorkspaceAccess {
    pub fn is_writable(self) -> bool {
        matches!(self, WorkspaceAccess::Writable)
    }
}

/// What the parent expects back. Drives result validation: an
/// [`ExpectedOutput::Review`] delegation that returns a patch is a contract
/// violation, not a bonus.
#[derive(
    Clone, Copy, Debug, Eq, JsonSchema, Ord, PartialEq, PartialOrd, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum ExpectedOutput {
    /// A patch proposal against the worker's base commit.
    Patch,
    /// Structured findings with file/line/evidence provenance.
    Review,
    /// Structured answers to a question; no code changes.
    Investigation,
    /// Validation evidence (tests, build, lint) against an integrated state.
    Validation,
}

impl ExpectedOutput {
    /// The access a delegation of this shape may hold. Read-only outputs can
    /// never be writable, whatever the profile asked for.
    pub fn required_access(self) -> WorkspaceAccess {
        match self {
            ExpectedOutput::Patch => WorkspaceAccess::Writable,
            ExpectedOutput::Review | ExpectedOutput::Investigation | ExpectedOutput::Validation => {
                WorkspaceAccess::ReadOnly
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Request → admitted delegation.
// ---------------------------------------------------------------------------

/// The unrestricted claim a planner produces (v1.4 §5.2).
///
/// Like [`crate::ToolDescriptorProposal`], this can never be executed: it has to
/// pass [`DelegationRequest::admit`] first, which is where the authority
/// intersection happens.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct DelegationRequest {
    pub parent_session_id: SessionId,
    pub parent_turn_id: TurnId,
    pub objective: String,
    pub capability: CapabilityId,
    #[serde(default)]
    pub acceptance_criteria: Vec<AcceptanceCriterion>,
    #[serde(default)]
    pub context_refs: Vec<ContextRef>,
    #[serde(default)]
    pub allowed_paths: Vec<PathPattern>,
    pub expected_output: ExpectedOutput,
    #[serde(default)]
    pub dependencies: Vec<DelegationId>,
    pub budget: DelegationBudget,
}

/// The context the parent explicitly hands a worker (v1.4 §PR6).
///
/// Structurally mirrors the composer reference grammar in
/// `purrcode-reference-resolver`; kept as its own type so `runtime-core` stays
/// at the bottom of the dependency graph. The daemon converts between them.
#[derive(Clone, Debug, Eq, JsonSchema, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum ContextRef {
    File {
        path: String,
        #[serde(default)]
        range: Option<(u64, u64)>,
    },
    Symbol {
        name: String,
    },
    Folder {
        path: String,
    },
    /// The parent's current change set.
    Diff,
    /// A named project-memory entry. Never "all of project memory".
    Memory {
        key: String,
    },
    /// A finding another worker produced, carried with its provenance.
    WorkerFinding {
        delegation_id: DelegationId,
        finding_id: String,
    },
}

impl ContextRef {
    pub fn display(&self) -> String {
        match self {
            ContextRef::File { path, range } => match range {
                Some((start, end)) => format!("@{path}#L{start}-L{end}"),
                None => format!("@{path}"),
            },
            ContextRef::Symbol { name } => format!("#{name}"),
            ContextRef::Folder { path } => format!("@folder:{path}"),
            ContextRef::Diff => "@diff".into(),
            ContextRef::Memory { key } => format!("@memory:{key}"),
            ContextRef::WorkerFinding {
                delegation_id,
                finding_id,
            } => format!("@finding:{}/{finding_id}", delegation_id.short()),
        }
    }
}

/// Everything the intersection needs, in one argument so a caller cannot forget
/// a term (v1.4 §5.3).
///
/// ```text
/// Effective Worker Authority
///     = Workspace Policy ∩ Parent Agent Ceiling ∩ Specialist Profile ∩ Task
/// ```
#[derive(Clone, Debug)]
pub struct AuthorityInputs<'a> {
    /// The workspace policy ceiling (PawGate).
    pub workspace: &'a ToolCeiling,
    /// The ceiling the *parent* agent is running under this turn.
    pub parent: &'a ToolCeiling,
    /// The selected specialist profile's ceiling.
    pub profile: &'a ToolCeiling,
    /// What the parent has left to spend.
    pub parent_remaining_budget: &'a DelegationBudget,
    /// How deep this delegation would sit. v1.4 permits exactly one level.
    pub depth: u8,
}

/// The maximum delegation depth in v1.4 (§PR14). Main → specialist, and no
/// further: a specialist cannot delegate again.
pub const MAXIMUM_DELEGATION_DEPTH: u8 = 1;

/// One bounded unit of delegated work (v1.4 §6.1).
///
/// **No public constructor and no public authority field.** `effective_ceiling`
/// and `budget` are private and reachable only through accessors, so the only
/// value they can ever hold is the one [`DelegationRequest::admit`] computed.
#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize)]
pub struct Delegation {
    id: DelegationId,
    parent_session_id: SessionId,
    parent_turn_id: TurnId,
    objective: String,
    capability: CapabilityId,
    acceptance_criteria: Vec<AcceptanceCriterion>,
    context_refs: Vec<ContextRef>,
    allowed_paths: Vec<PathPattern>,
    expected_output: ExpectedOutput,
    access: WorkspaceAccess,
    effective_ceiling: ToolCeiling,
    budget: DelegationBudget,
    dependencies: Vec<DelegationId>,
    depth: u8,
    status: DelegationStatus,
    created_at: DateTime<Utc>,
    /// blake3 over every field above. Binds a worker's authority to evidence:
    /// a re-planned delegation is a different delegation.
    digest: String,
}

impl Delegation {
    pub fn id(&self) -> DelegationId {
        self.id
    }
    pub fn parent_session_id(&self) -> SessionId {
        self.parent_session_id
    }
    pub fn parent_turn_id(&self) -> TurnId {
        self.parent_turn_id
    }
    pub fn objective(&self) -> &str {
        &self.objective
    }
    pub fn capability(&self) -> &CapabilityId {
        &self.capability
    }
    pub fn acceptance_criteria(&self) -> &[AcceptanceCriterion] {
        &self.acceptance_criteria
    }
    pub fn context_refs(&self) -> &[ContextRef] {
        &self.context_refs
    }
    pub fn allowed_paths(&self) -> &[PathPattern] {
        &self.allowed_paths
    }
    pub fn expected_output(&self) -> ExpectedOutput {
        self.expected_output
    }
    pub fn access(&self) -> WorkspaceAccess {
        self.access
    }
    pub fn effective_ceiling(&self) -> &ToolCeiling {
        &self.effective_ceiling
    }
    pub fn budget(&self) -> &DelegationBudget {
        &self.budget
    }
    pub fn dependencies(&self) -> &[DelegationId] {
        &self.dependencies
    }
    pub fn depth(&self) -> u8 {
        self.depth
    }
    pub fn status(&self) -> DelegationStatus {
        self.status
    }
    pub fn created_at(&self) -> DateTime<Utc> {
        self.created_at
    }
    pub fn digest(&self) -> &str {
        &self.digest
    }

    /// Apply a lifecycle transition, refusing an illegal one. The digest is not
    /// recomputed: status is mutable lifecycle state, while the digest binds the
    /// delegation's *authority*, which never changes after admission.
    pub fn transition_to(&mut self, next: DelegationStatus) -> Result<(), DelegationError> {
        if !self.status.can_transition_to(next) {
            return Err(DelegationError::IllegalTransition {
                from: self.status,
                to: next,
            });
        }
        self.status = next;
        Ok(())
    }

    /// True when `path` (repository-relative) is inside the delegated scope.
    /// A delegation with no declared paths permits nothing to be written — an
    /// empty allowlist is empty, never "everything".
    pub fn permits_path(&self, path: &Path) -> bool {
        path_is_contained(path)
            && self
                .allowed_paths
                .iter()
                .any(|pattern| pattern.matches(path))
    }

    fn recompute_digest(mut self) -> Self {
        self.digest.clear();
        let canonical = serde_json::to_vec(&self).expect("Delegation is always serializable");
        self.digest = blake3::hash(&canonical).to_hex().to_string();
        self
    }
}

impl DelegationRequest {
    /// THE ONLY MINT for a [`Delegation`] (v1.4 §5.3).
    ///
    /// Folds `workspace ∩ parent ∩ profile` and narrows the result to the
    /// requested paths, then clamps the budget to what the parent has left.
    /// Every axis can only shrink; the function has no branch that widens one.
    pub fn admit(self, authority: AuthorityInputs<'_>) -> Result<Delegation, DelegationError> {
        if self.objective.trim().is_empty() {
            return Err(DelegationError::EmptyObjective);
        }
        if authority.depth > MAXIMUM_DELEGATION_DEPTH {
            return Err(DelegationError::DepthExceeded {
                depth: authority.depth,
                maximum: MAXIMUM_DELEGATION_DEPTH,
            });
        }

        // 1. Fold the three policy ceilings. `meet` is the only combinator, so
        //    the result is bounded by every input by construction.
        let folded = authority
            .workspace
            .meet(authority.parent)
            .meet(authority.profile);

        // 2. The expected output decides whether write authority is coherent at
        //    all. A reviewer is read-only even when its profile asks for more —
        //    this is the "parent read-only agent delegates to a write-enabled
        //    profile" failure test, and the answer is: the worker stays
        //    read-only.
        //
        //    A delegation that is *expected to return a patch* but whose folded
        //    authority permits no writing is incoherent, not merely narrow:
        //    admitting it would produce a worker that cannot do the job it was
        //    given, and whose eventual empty result would read as success. It is
        //    refused here. The invariant the failure test cares about still
        //    holds — the worker never gains write authority the parent lacks —
        //    it is just enforced by refusal rather than by a silent downgrade.
        let requested_access = self.expected_output.required_access();
        if requested_access.is_writable() && !folded.permits_write() {
            return Err(DelegationError::WriterUnderReadOnlyParent);
        }
        let access = if requested_access.is_writable() {
            WorkspaceAccess::Writable
        } else {
            WorkspaceAccess::ReadOnly
        };

        // 3. Narrow the filesystem axis to the delegated paths. Narrowing, not
        //    meeting: `allowed_paths` is a subset request against a ceiling that
        //    already permits them, and exact-string glob intersection would
        //    wrongly collapse `src/auth/**` under a `**` ceiling.
        let patterns: Vec<&str> = self.allowed_paths.iter().map(PathPattern::as_str).collect();
        let filesystem = match access {
            WorkspaceAccess::ReadOnly => match folded.maximum_filesystem {
                FilesystemScope::None => FilesystemScope::None,
                _ => FilesystemScope::WorktreeRead,
            },
            WorkspaceAccess::Writable => folded
                .maximum_filesystem
                .narrow_to_paths(patterns.iter().copied())
                .with_changed_file_limit(self.budget.maximum_changed_files),
        };

        if access.is_writable() && !matches!(filesystem, FilesystemScope::Worktree { .. }) {
            // Every requested path was outside the parent's write scope. Failing
            // here is the point: silently downgrading a writer to a reader would
            // produce a worker that cannot do the job it was delegated.
            return Err(DelegationError::NoWritablePathRemains);
        }

        let effective_ceiling = ToolCeiling {
            maximum_side_effect: match access {
                WorkspaceAccess::ReadOnly => folded.maximum_side_effect.min(SideEffectClass::Read),
                WorkspaceAccess::Writable => folded.maximum_side_effect,
            },
            maximum_network: folded.maximum_network.clone(),
            maximum_filesystem: filesystem,
            minimum_approval: folded.minimum_approval,
            denied_tool_ids: folded.denied_tool_ids.clone(),
        };

        // 4. The result must be within every input. This is belt-and-braces
        //    against a future edit to the fold above: the assertion is cheap and
        //    the failure mode it guards is authority escalation.
        for (axis, bound) in [
            ("workspace", authority.workspace),
            ("parent", authority.parent),
            ("profile", authority.profile),
        ] {
            if !effective_ceiling.is_within(bound) {
                return Err(DelegationError::AuthorityEscalation {
                    axis: axis.to_owned(),
                    requested: format!("{effective_ceiling:?}"),
                    permitted: format!("{bound:?}"),
                });
            }
        }

        // 5. Budgets subdivide the parent's remaining allowance.
        if authority.parent_remaining_budget.is_exhausted() {
            return Err(DelegationError::BudgetExceedsParent {
                axis: "parent budget".into(),
                requested: self.budget.maximum_input_tokens,
                remaining: authority.parent_remaining_budget.maximum_input_tokens,
            });
        }
        let budget = self
            .budget
            .clamp_to_remaining(authority.parent_remaining_budget);
        let budget = match access {
            // A read-only worker's changed-file allowance is zero, whatever the
            // planner asked for.
            WorkspaceAccess::ReadOnly => DelegationBudget {
                maximum_changed_files: 0,
                ..budget
            },
            WorkspaceAccess::Writable => budget,
        };

        Ok(Delegation {
            id: DelegationId::new(),
            parent_session_id: self.parent_session_id,
            parent_turn_id: self.parent_turn_id,
            objective: self.objective.trim().to_owned(),
            capability: self.capability,
            acceptance_criteria: self.acceptance_criteria,
            context_refs: self.context_refs,
            allowed_paths: self.allowed_paths,
            expected_output: self.expected_output,
            access,
            effective_ceiling,
            budget,
            dependencies: self.dependencies,
            depth: authority.depth,
            status: DelegationStatus::Planned,
            created_at: Utc::now(),
            digest: String::new(),
        }
        .recompute_digest())
    }
}

/// Read-back path for a persisted [`Delegation`]. Re-verifies the digest, so a
/// record whose authority fields were edited in the database fails to load
/// rather than executing with authority nobody granted.
impl<'de> Deserialize<'de> for Delegation {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct Wire {
            id: DelegationId,
            parent_session_id: SessionId,
            parent_turn_id: TurnId,
            objective: String,
            capability: CapabilityId,
            #[serde(default)]
            acceptance_criteria: Vec<AcceptanceCriterion>,
            #[serde(default)]
            context_refs: Vec<ContextRef>,
            #[serde(default)]
            allowed_paths: Vec<PathPattern>,
            expected_output: ExpectedOutput,
            access: WorkspaceAccess,
            effective_ceiling: ToolCeiling,
            budget: DelegationBudget,
            #[serde(default)]
            dependencies: Vec<DelegationId>,
            depth: u8,
            status: DelegationStatus,
            created_at: DateTime<Utc>,
            digest: String,
        }
        let wire = Wire::deserialize(deserializer)?;
        let stored_digest = wire.digest.clone();
        let delegation = Delegation {
            id: wire.id,
            parent_session_id: wire.parent_session_id,
            parent_turn_id: wire.parent_turn_id,
            objective: wire.objective,
            capability: wire.capability,
            acceptance_criteria: wire.acceptance_criteria,
            context_refs: wire.context_refs,
            allowed_paths: wire.allowed_paths,
            expected_output: wire.expected_output,
            access: wire.access,
            effective_ceiling: wire.effective_ceiling,
            budget: wire.budget,
            dependencies: wire.dependencies,
            depth: wire.depth,
            // The digest is computed over the delegation as first admitted, so a
            // status change must not invalidate it: hash with the status the
            // digest was taken at, then restore the persisted one.
            status: DelegationStatus::Planned,
            created_at: wire.created_at,
            digest: String::new(),
        }
        .recompute_digest();
        if delegation.digest != stored_digest {
            return Err(serde::de::Error::custom(
                "Delegation digest does not match its authority fields; refusing tampered record",
            ));
        }
        Ok(Delegation {
            status: wire.status,
            ..delegation
        })
    }
}

// ---------------------------------------------------------------------------
// Worker assignment.
// ---------------------------------------------------------------------------

/// Where a worker's files live (v1.4 §PR3).
#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct WorkerWorkspaceRecord {
    pub parent_worktree: PathBuf,
    /// A writable worker's own worktree. `None` for a read-only worker, which
    /// reads the parent snapshot and is given no worktree of its own.
    #[serde(default)]
    pub worker_worktree: Option<PathBuf>,
    pub base_commit: String,
    /// Digest of the parent snapshot the worker started from. Integration
    /// refuses a patch whose base no longer matches.
    pub base_snapshot_digest: String,
    pub access: WorkspaceAccess,
}

impl WorkerWorkspaceRecord {
    /// The directory a worker actually executes in.
    pub fn working_directory(&self) -> &Path {
        self.worker_worktree
            .as_deref()
            .unwrap_or(&self.parent_worktree)
    }

    /// A writable record must name its own worktree, and it must not be the
    /// parent's. Two writers sharing one writable worktree is the exact failure
    /// v1.4 §5.4 forbids.
    pub fn is_coherent(&self) -> bool {
        match (self.access, self.worker_worktree.as_deref()) {
            (WorkspaceAccess::Writable, Some(path)) => path != self.parent_worktree,
            (WorkspaceAccess::Writable, None) => false,
            (WorkspaceAccess::ReadOnly, None) => true,
            (WorkspaceAccess::ReadOnly, Some(_)) => false,
        }
    }
}

/// Which specialist ran which delegation, on what model, in which workspace.
#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct WorkerAssignment {
    pub worker_id: WorkerId,
    pub delegation_id: DelegationId,
    pub agent_profile: String,
    /// Digest of the profile as admitted. A profile edited mid-run produces a
    /// different digest, so evidence never claims a worker ran under a profile
    /// it did not.
    pub profile_digest: String,
    #[serde(default)]
    pub model_role: Option<ModelRoleName>,
    pub workspace: WorkerWorkspaceRecord,
    pub assigned_at: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// Worker result.
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum WorkerResultStatus {
    /// The worker met its acceptance criteria and proposes its changes.
    Completed,
    /// The worker ran but could not finish; the parent decides what happens.
    Failed {
        reason: String,
    },
    TimedOut,
    Cancelled,
    /// The worker stopped because its budget ran out. Bounded failure, not a
    /// silent truncation.
    BudgetExhausted {
        axis: String,
    },
    /// The worker finished but flagged something a human must look at, e.g.
    /// after the repair loop reached its bound.
    NeedsAttention {
        reason: String,
    },
}

impl WorkerResultStatus {
    pub fn is_success(&self) -> bool {
        matches!(self, WorkerResultStatus::Completed)
    }

    pub fn label(&self) -> &'static str {
        match self {
            WorkerResultStatus::Completed => "completed",
            WorkerResultStatus::Failed { .. } => "failed",
            WorkerResultStatus::TimedOut => "timed_out",
            WorkerResultStatus::Cancelled => "cancelled",
            WorkerResultStatus::BudgetExhausted { .. } => "budget_exhausted",
            WorkerResultStatus::NeedsAttention { .. } => "needs_attention",
        }
    }
}

#[derive(
    Clone, Copy, Debug, Eq, JsonSchema, Ord, PartialEq, PartialOrd, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum FindingSeverity {
    Info,
    Low,
    Medium,
    High,
    Critical,
}

impl FindingSeverity {
    /// Findings at or above this level block an automatic integration.
    pub fn blocks_integration(self) -> bool {
        self >= FindingSeverity::High
    }
}

/// A reviewer's structured output. `evidence_ids` is not optional: v1.4 §PR8's
/// acceptance is that a finding can always be traced to what produced it.
#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct StructuredFinding {
    pub id: String,
    pub title: String,
    pub detail: String,
    pub severity: FindingSeverity,
    #[serde(default)]
    pub path: Option<PathBuf>,
    #[serde(default)]
    pub line: Option<u32>,
    pub evidence_ids: Vec<EvidenceId>,
    #[serde(default)]
    pub recommended_action: Option<String>,
}

impl StructuredFinding {
    /// A finding with no evidence and no file location is an assertion, not a
    /// finding. The parent must be able to trace "security issue detected" to
    /// the worker and evidence that produced it (v1.4 §PR13).
    pub fn has_provenance(&self) -> bool {
        !self.evidence_ids.is_empty() || self.path.is_some()
    }
}

#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct ValidationEvidence {
    pub name: String,
    pub status: ValidationStatus,
    pub detail: String,
    #[serde(default)]
    pub evidence_id: Option<EvidenceId>,
}

#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct OpenIssue {
    pub summary: String,
    #[serde(default)]
    pub detail: String,
}

/// The canonical handoff (v1.4 §6.4).
///
/// Never a raw assistant paragraph: the parent integrates from this structure,
/// and [`WorkerResult::validate_against`] is the gate every result passes before
/// the integration pipeline sees it.
#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct WorkerResult {
    pub delegation_id: DelegationId,
    pub worker_id: WorkerId,
    pub status: WorkerResultStatus,
    pub summary: String,
    #[serde(default)]
    pub changed_paths: Vec<PathBuf>,
    #[serde(default)]
    pub patch_digest: Option<String>,
    #[serde(default)]
    pub findings: Vec<StructuredFinding>,
    #[serde(default)]
    pub validations: Vec<ValidationEvidence>,
    #[serde(default)]
    pub unresolved: Vec<OpenIssue>,
    #[serde(default)]
    pub evidence_ids: Vec<EvidenceId>,
    pub usage: UsageSummary,
    pub completed_at: DateTime<Utc>,
}

impl WorkerResult {
    /// Every scope and budget check a result must pass before integration
    /// (v1.4 §PR7 step 2, §9 "Scope Escape").
    ///
    /// This runs *before* any patch is applied and before a human is asked to
    /// approve: a worker that wrote outside its delegated paths is refused here,
    /// so the approval prompt can never be the first place the escape is
    /// noticed.
    pub fn validate_against(&self, delegation: &Delegation) -> Result<(), DelegationError> {
        if self.delegation_id != delegation.id() {
            return Err(DelegationError::ResultDelegationMismatch {
                expected: delegation.id(),
                actual: self.delegation_id,
            });
        }

        if !delegation.access().is_writable() && !self.changed_paths.is_empty() {
            return Err(DelegationError::ReadOnlyWorkerMutated {
                changed: self.changed_paths.len(),
            });
        }

        for path in &self.changed_paths {
            if !delegation.permits_path(path) {
                return Err(DelegationError::ScopeEscape {
                    path: path.display().to_string(),
                });
            }
        }

        if self.changed_paths.len() > delegation.budget().maximum_changed_files {
            return Err(DelegationError::ChangedFileBudgetExceeded {
                changed: self.changed_paths.len(),
                permitted: delegation.budget().maximum_changed_files,
            });
        }

        for finding in &self.findings {
            if !finding.has_provenance() {
                return Err(DelegationError::FindingWithoutProvenance {
                    title: finding.title.clone(),
                });
            }
        }

        if self.status.is_success() && !self.changed_paths.is_empty() && self.patch_digest.is_none()
        {
            return Err(DelegationError::MissingPatchDigest);
        }

        Ok(())
    }

    /// True when this result proposes changes the parent could integrate.
    pub fn proposes_changes(&self) -> bool {
        self.status.is_success() && !self.changed_paths.is_empty()
    }

    /// The highest severity among the findings, if any.
    pub fn peak_severity(&self) -> Option<FindingSeverity> {
        self.findings.iter().map(|f| f.severity).max()
    }
}

/// A review worker's verdict (v1.4 §PR8).
///
/// A reviewer reports; it does not rewrite. `pass` is advisory to the main
/// agent, which decides which findings to act on.
#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct ReviewResult {
    pub delegation_id: DelegationId,
    pub worker_id: WorkerId,
    pub findings: Vec<StructuredFinding>,
    #[serde(default)]
    pub recommended_actions: Vec<String>,
    pub pass: bool,
    pub reviewed_at: DateTime<Utc>,
}

impl ReviewResult {
    /// A review passes only when nothing at or above `High` was found. Computed,
    /// never taken from the model's own `pass` claim — a reviewer that reports
    /// two critical findings and `pass: true` is contradicting itself.
    pub fn computed_pass(&self) -> bool {
        !self
            .findings
            .iter()
            .any(|finding| finding.severity.blocks_integration())
    }

    /// True when every finding can be traced to evidence or a file location.
    pub fn findings_have_provenance(&self) -> bool {
        self.findings.iter().all(StructuredFinding::has_provenance)
    }

    pub fn blocking_findings(&self) -> impl Iterator<Item = &StructuredFinding> {
        self.findings
            .iter()
            .filter(|finding| finding.severity.blocks_integration())
    }
}

// ---------------------------------------------------------------------------
// Parent-level governance (v1.4 §PR14).
// ---------------------------------------------------------------------------

/// Ceilings that apply to the *whole* delegation tree, not to one worker.
#[derive(Clone, Copy, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct DelegationGovernance {
    pub maximum_total_worker_input_tokens: u64,
    pub maximum_total_worker_output_tokens: u64,
    pub maximum_workers: u32,
    pub maximum_parallel_workers: u32,
    pub maximum_worker_model_calls: u32,
    pub maximum_delegation_depth: u8,
}

impl Default for DelegationGovernance {
    /// v1.4 §PR4 recommends parallelism 2, maximum 3, hard maximum 5. The
    /// default sits at the recommended maximum, and
    /// [`DelegationGovernance::validate`] refuses anything past the hard bound.
    fn default() -> Self {
        Self {
            maximum_total_worker_input_tokens: 600_000,
            maximum_total_worker_output_tokens: 150_000,
            maximum_workers: 6,
            maximum_parallel_workers: 3,
            maximum_worker_model_calls: 60,
            maximum_delegation_depth: MAXIMUM_DELEGATION_DEPTH,
        }
    }
}

/// The hard parallelism bound. No configuration may exceed it (v1.4 §PR4).
pub const HARD_MAXIMUM_PARALLEL_WORKERS: u32 = 5;

impl DelegationGovernance {
    pub fn validate(&self) -> Result<(), DelegationError> {
        if self.maximum_parallel_workers == 0
            || self.maximum_parallel_workers > HARD_MAXIMUM_PARALLEL_WORKERS
        {
            return Err(DelegationError::InvalidDependencyGraph {
                reason: format!(
                    "parallel workers must be between 1 and {HARD_MAXIMUM_PARALLEL_WORKERS}"
                ),
            });
        }
        if self.maximum_delegation_depth > MAXIMUM_DELEGATION_DEPTH {
            return Err(DelegationError::DepthExceeded {
                depth: self.maximum_delegation_depth,
                maximum: MAXIMUM_DELEGATION_DEPTH,
            });
        }
        Ok(())
    }

    /// Effective parallelism: the configured value, clamped to the hard bound
    /// and to the number of workers that could possibly run.
    pub fn effective_parallelism(&self, ready_workers: usize) -> usize {
        (self
            .maximum_parallel_workers
            .min(HARD_MAXIMUM_PARALLEL_WORKERS) as usize)
            .min(ready_workers.max(1))
    }
}

/// Running totals for one parent session's delegation tree.
#[derive(Clone, Debug, Default, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct DelegationLedger {
    pub workers_started: u32,
    pub workers_completed: u32,
    pub total_worker_usage: UsageSummary,
    #[serde(default)]
    pub refusals: Vec<String>,
}

impl DelegationLedger {
    /// Whether one more worker may start. Returns the reason when not, so the
    /// refusal is explainable rather than a silent "nothing happened".
    pub fn admits_another_worker(
        &self,
        governance: &DelegationGovernance,
        currently_running: u32,
    ) -> Result<(), String> {
        if self.workers_started >= governance.maximum_workers {
            return Err(format!(
                "the session already started {} workers (maximum {})",
                self.workers_started, governance.maximum_workers
            ));
        }
        if currently_running >= governance.maximum_parallel_workers {
            return Err(format!(
                "{currently_running} workers are already running (maximum {})",
                governance.maximum_parallel_workers
            ));
        }
        if self.total_worker_usage.input_tokens >= governance.maximum_total_worker_input_tokens {
            return Err(format!(
                "worker input-token budget is exhausted ({} of {})",
                self.total_worker_usage.input_tokens, governance.maximum_total_worker_input_tokens
            ));
        }
        if self.total_worker_usage.output_tokens >= governance.maximum_total_worker_output_tokens {
            return Err(format!(
                "worker output-token budget is exhausted ({} of {})",
                self.total_worker_usage.output_tokens,
                governance.maximum_total_worker_output_tokens
            ));
        }
        if self.total_worker_usage.model_calls >= governance.maximum_worker_model_calls {
            return Err(format!(
                "worker model-call budget is exhausted ({} of {})",
                self.total_worker_usage.model_calls, governance.maximum_worker_model_calls
            ));
        }
        Ok(())
    }

    /// What is left for the next worker, expressed as a budget so
    /// [`DelegationRequest::admit`] can clamp against it.
    pub fn remaining_budget(&self, governance: &DelegationGovernance) -> DelegationBudget {
        DelegationBudget {
            maximum_input_tokens: governance
                .maximum_total_worker_input_tokens
                .saturating_sub(self.total_worker_usage.input_tokens),
            maximum_output_tokens: governance
                .maximum_total_worker_output_tokens
                .saturating_sub(self.total_worker_usage.output_tokens),
            maximum_tool_calls: u32::MAX,
            maximum_duration_seconds: u64::MAX,
            maximum_changed_files: usize::MAX,
        }
    }

    pub fn record_usage(&mut self, usage: &UsageSummary) {
        self.total_worker_usage = self.total_worker_usage.saturating_add(usage);
    }
}

/// The maximum number of automatic repair cycles for one delegation
/// (v1.4 §PR9). One by default; two is permitted only for a bounded
/// compilation/test failure, and after that control returns to the parent.
pub const MAXIMUM_REPAIR_CYCLES: u8 = 2;

/// Whether a failed validation may be routed back to its worker for repair.
#[derive(Clone, Copy, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RepairDecision {
    /// Send the failure evidence back to the responsible worker.
    Retry { cycle: u8 },
    /// The bound is reached; a human decides.
    NeedsAttention,
}

/// Decide whether to repair. `bounded_failure` is true for a compile or test
/// failure whose output names what broke — the only case that earns a second
/// cycle. Anything else gets one attempt, then stops.
pub fn repair_decision(cycles_used: u8, bounded_failure: bool) -> RepairDecision {
    let permitted = if bounded_failure {
        MAXIMUM_REPAIR_CYCLES
    } else {
        1
    };
    if cycles_used < permitted {
        RepairDecision::Retry {
            cycle: cycles_used + 1,
        }
    } else {
        RepairDecision::NeedsAttention
    }
}

// ---------------------------------------------------------------------------
// Context provenance (v1.4 §PR13).
// ---------------------------------------------------------------------------

/// Why a section of the parent's context exists, when it came from a worker.
///
/// A model must never receive "security issue detected" without being able to
/// trace which worker and which evidence produced it.
#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DelegationOrigin {
    /// The whole structured result of a completed delegation.
    WorkerResult {
        delegation_id: DelegationId,
        worker_id: WorkerId,
        capability: String,
    },
    /// One finding, carried with the evidence that produced it.
    WorkerFinding {
        delegation_id: DelegationId,
        worker_id: WorkerId,
        finding_id: String,
        evidence_ids: Vec<EvidenceId>,
    },
    /// The integration outcome for one worker's patch.
    IntegrationOutcome {
        delegation_id: DelegationId,
        applied: bool,
    },
}

impl DelegationOrigin {
    /// One line explaining why this content is in the parent's context.
    pub fn why_included(&self) -> String {
        match self {
            DelegationOrigin::WorkerResult { capability, .. } => {
                format!("delegation {capability} completed")
            }
            DelegationOrigin::WorkerFinding {
                delegation_id,
                evidence_ids,
                ..
            } => format!(
                "finding from delegation {} with {} evidence record(s)",
                delegation_id.short(),
                evidence_ids.len()
            ),
            DelegationOrigin::IntegrationOutcome {
                delegation_id,
                applied,
            } => format!(
                "integration of delegation {} was {}",
                delegation_id.short(),
                if *applied { "applied" } else { "not applied" }
            ),
        }
    }

    pub fn delegation_id(&self) -> DelegationId {
        match self {
            DelegationOrigin::WorkerResult { delegation_id, .. }
            | DelegationOrigin::WorkerFinding { delegation_id, .. }
            | DelegationOrigin::IntegrationOutcome { delegation_id, .. } => *delegation_id,
        }
    }
}

// ---------------------------------------------------------------------------
// Durable projection (v1.4 §PR10).
// ---------------------------------------------------------------------------

/// Where one worker's changes are in the integration pipeline.
#[derive(
    Clone, Copy, Debug, Default, Eq, JsonSchema, Ord, PartialEq, PartialOrd, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum IntegrationState {
    /// No proposal yet — the worker has not finished, or produced no changes.
    #[default]
    NotProposed,
    Proposed,
    /// Conflicts were detected; the parent must resolve them.
    Conflicted,
    /// A human approved the patch, but it has not been applied yet.
    Approved,
    Rejected,
    Applied,
}

impl IntegrationState {
    /// A patch may only be applied from `Approved`. This is what makes
    /// "all worker changes enter the parent through an IntegrationProposal"
    /// a property of the event log rather than of the daemon's control flow.
    pub fn can_apply(self) -> bool {
        matches!(self, IntegrationState::Approved)
    }
}

/// Everything durable about one delegation, rebuilt by replaying the event log.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DelegationRecord {
    pub delegation: Delegation,
    #[serde(default)]
    pub routing: Option<RoutingDecision>,
    #[serde(default)]
    pub assignment: Option<WorkerAssignment>,
    #[serde(default)]
    pub result: Option<WorkerResult>,
    #[serde(default)]
    pub proposal: Option<IntegrationProposal>,
    #[serde(default)]
    pub conflicts: Vec<IntegrationConflict>,
    #[serde(default)]
    pub integration: IntegrationState,
    #[serde(default)]
    pub repair_cycles: u8,
    #[serde(default)]
    pub blocked_by: Option<DelegationId>,
    /// Set when a worker paused rather than finished. Its worktree and any
    /// partial patch are preserved; recovery reconciles rather than reruns.
    #[serde(default)]
    pub paused_reason: Option<String>,
}

impl DelegationRecord {
    pub fn new(delegation: Delegation) -> Self {
        Self {
            delegation,
            routing: None,
            assignment: None,
            result: None,
            proposal: None,
            conflicts: Vec::new(),
            integration: IntegrationState::NotProposed,
            repair_cycles: 0,
            blocked_by: None,
            paused_reason: None,
        }
    }

    pub fn status(&self) -> DelegationStatus {
        self.delegation.status()
    }

    pub fn is_running(&self) -> bool {
        self.delegation.status() == DelegationStatus::Running
    }

    /// True when this delegation reached a terminal state with a recorded
    /// result. Recovery must never rerun one of these (v1.4 §PR10).
    pub fn is_finished(&self) -> bool {
        self.delegation.status().is_terminal() && self.result.is_some()
    }

    /// True when the worker's changes are still waiting on a human.
    pub fn awaits_decision(&self) -> bool {
        matches!(
            self.integration,
            IntegrationState::Proposed | IntegrationState::Conflicted
        ) || self.delegation.status() == DelegationStatus::AwaitingApproval
    }

    /// The worktree a paused or pending worker owns, which must not be deleted
    /// while its result is unresolved (v1.4 §PR3 "never silently delete an
    /// unresolved worker patch").
    pub fn retained_worktree(&self) -> Option<&Path> {
        let workspace = &self.assignment.as_ref()?.workspace;
        let unresolved = !matches!(
            self.integration,
            IntegrationState::Applied | IntegrationState::Rejected
        );
        unresolved
            .then_some(workspace.worker_worktree.as_deref())
            .flatten()
    }
}

/// A capability-routing decision, recorded so "why this specialist?" is
/// answerable after the fact (v1.4 §PR5).
#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct RoutingDecision {
    pub capability: String,
    pub chosen_profile: String,
    pub profile_digest: String,
    #[serde(default)]
    pub model_role: Option<ModelRoleName>,
    /// Every other candidate that was considered, best-ranked first.
    #[serde(default)]
    pub alternatives: Vec<String>,
    pub reason: String,
}

/// A `BTreeSet` of capability ids a delegation planner knows how to ask for.
/// Not a closed enum: a user can define `.purrcode/agents/security-reviewer.yaml`
/// declaring any capability, and the planner routes to it without a Rust change.
pub fn well_known_capabilities() -> BTreeSet<&'static str> {
    [
        "implement_backend",
        "implement_frontend",
        "write_tests",
        "security_review",
        "debug_failure",
        "documentation",
        "database_migration",
        "performance_review",
        "code_review",
    ]
    .into_iter()
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ApprovalPolicy, NetworkScope};

    fn capability(raw: &str) -> CapabilityId {
        CapabilityId::parse(raw).unwrap()
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

    fn read_only_ceiling() -> ToolCeiling {
        ToolCeiling {
            maximum_side_effect: SideEffectClass::Read,
            maximum_network: NetworkScope::None,
            maximum_filesystem: FilesystemScope::WorktreeRead,
            minimum_approval: ApprovalPolicy::AlwaysAsk,
            denied_tool_ids: BTreeSet::new(),
        }
    }

    fn request(paths: &[&str], expected: ExpectedOutput) -> DelegationRequest {
        DelegationRequest {
            parent_session_id: SessionId::new(),
            parent_turn_id: TurnId::new(),
            objective: "add oauth token exchange".into(),
            capability: capability("implement_backend"),
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
    }

    fn inputs<'a>(
        workspace: &'a ToolCeiling,
        parent: &'a ToolCeiling,
        profile: &'a ToolCeiling,
        remaining: &'a DelegationBudget,
    ) -> AuthorityInputs<'a> {
        AuthorityInputs {
            workspace,
            parent,
            profile,
            parent_remaining_budget: remaining,
            depth: 1,
        }
    }

    #[test]
    fn admit_narrows_the_filesystem_to_the_delegated_paths() {
        let ceiling = permissive();
        let remaining = DelegationBudget::modest();
        let delegation = request(&["src/auth/**"], ExpectedOutput::Patch)
            .admit(inputs(&ceiling, &ceiling, &ceiling, &remaining))
            .unwrap();
        // The parent could write anywhere; the delegation may only write
        // `src/auth/**` — narrower on the axis, never wider.
        assert_eq!(
            delegation
                .effective_ceiling()
                .maximum_filesystem
                .write_globs(),
            ["src/auth/**"]
        );
        assert!(delegation.permits_path(Path::new("src/auth/token.rs")));
        assert!(!delegation.permits_path(Path::new("src/payments/billing.rs")));
        assert!(delegation.effective_ceiling().is_within(&ceiling));
    }

    #[test]
    fn a_read_only_parent_cannot_delegate_write_authority() {
        // v1.4 §9 "Permission Escalation": a read-only parent delegating to a
        // write-enabled profile must still produce a read-only worker.
        let workspace = permissive();
        let parent = read_only_ceiling();
        let profile = permissive();
        let remaining = DelegationBudget::modest();
        let error = request(&["src/auth/**"], ExpectedOutput::Patch)
            .admit(inputs(&workspace, &parent, &profile, &remaining))
            .expect_err("a writer under a read-only parent must be refused, not downgraded");
        assert!(matches!(error, DelegationError::WriterUnderReadOnlyParent));

        // The same delegation as a review is admitted, and is read-only.
        let review = request(&["src/auth/**"], ExpectedOutput::Review)
            .admit(inputs(&workspace, &parent, &profile, &remaining))
            .unwrap();
        assert_eq!(review.access(), WorkspaceAccess::ReadOnly);
        assert_eq!(
            review.effective_ceiling().maximum_side_effect,
            SideEffectClass::Read
        );
        assert!(!review.effective_ceiling().permits_write());
        assert_eq!(review.budget().maximum_changed_files, 0);
    }

    #[test]
    fn a_reviewer_is_read_only_even_when_its_profile_asks_for_write() {
        let ceiling = permissive();
        let remaining = DelegationBudget::modest();
        let review = request(&["src/**"], ExpectedOutput::Review)
            .admit(inputs(&ceiling, &ceiling, &ceiling, &remaining))
            .unwrap();
        assert_eq!(review.access(), WorkspaceAccess::ReadOnly);
        assert_eq!(
            review.effective_ceiling().maximum_filesystem,
            FilesystemScope::WorktreeRead
        );
    }

    #[test]
    fn admitted_authority_is_within_every_input_over_a_generated_sweep() {
        // The structural claim of §5.3, checked over combinations rather than
        // one hand-picked case: whichever three ceilings go in, what comes out
        // is within all of them.
        let side_effects = [
            SideEffectClass::Read,
            SideEffectClass::Write,
            SideEffectClass::Destructive,
        ];
        let filesystems = [
            FilesystemScope::WorktreeRead,
            FilesystemScope::Worktree {
                write_globs: vec!["src/**".into()],
                maximum_changed_files: 10,
            },
            FilesystemScope::maximum(),
        ];
        let approvals = [
            ApprovalPolicy::PreAuthorized,
            ApprovalPolicy::ByClass,
            ApprovalPolicy::AlwaysAsk,
        ];
        let remaining = DelegationBudget::modest();
        let mut admitted = 0_usize;
        for workspace_side in side_effects {
            for workspace_fs in &filesystems {
                for parent_side in side_effects {
                    for parent_fs in &filesystems {
                        for approval in approvals {
                            let workspace = ToolCeiling {
                                maximum_side_effect: workspace_side,
                                maximum_filesystem: workspace_fs.clone(),
                                minimum_approval: approval,
                                ..permissive()
                            };
                            let parent = ToolCeiling {
                                maximum_side_effect: parent_side,
                                maximum_filesystem: parent_fs.clone(),
                                ..permissive()
                            };
                            let profile = permissive();
                            let outcome = request(&["src/auth/**"], ExpectedOutput::Patch)
                                .admit(inputs(&workspace, &parent, &profile, &remaining));
                            let Ok(delegation) = outcome else {
                                continue;
                            };
                            admitted += 1;
                            let effective = delegation.effective_ceiling();
                            assert!(effective.is_within(&workspace), "escaped workspace");
                            assert!(effective.is_within(&parent), "escaped parent");
                            assert!(effective.is_within(&profile), "escaped profile");
                        }
                    }
                }
            }
        }
        assert!(admitted > 0, "the sweep must admit at least one delegation");
    }

    #[test]
    fn child_budget_cannot_exceed_the_parents_remaining_budget() {
        let ceiling = permissive();
        let remaining = DelegationBudget {
            maximum_input_tokens: 1_000,
            maximum_output_tokens: 500,
            maximum_tool_calls: 5,
            maximum_duration_seconds: 30,
            maximum_changed_files: 2,
        };
        let delegation = request(&["src/**"], ExpectedOutput::Patch)
            .admit(inputs(&ceiling, &ceiling, &ceiling, &remaining))
            .unwrap();
        assert!(delegation.budget().fits_within(&remaining));
        assert_eq!(delegation.budget().maximum_input_tokens, 1_000);
        assert_eq!(delegation.budget().maximum_changed_files, 2);
    }

    #[test]
    fn an_exhausted_parent_budget_refuses_new_delegation() {
        let ceiling = permissive();
        let remaining = DelegationBudget {
            maximum_input_tokens: 0,
            ..DelegationBudget::modest()
        };
        let error = request(&["src/**"], ExpectedOutput::Patch)
            .admit(inputs(&ceiling, &ceiling, &ceiling, &remaining))
            .expect_err("an exhausted parent cannot fund a worker");
        assert!(matches!(error, DelegationError::BudgetExceedsParent { .. }));
    }

    #[test]
    fn delegation_depth_is_bounded_to_one_level() {
        let ceiling = permissive();
        let remaining = DelegationBudget::modest();
        let error = request(&["src/**"], ExpectedOutput::Patch)
            .admit(AuthorityInputs {
                depth: 2,
                ..inputs(&ceiling, &ceiling, &ceiling, &remaining)
            })
            .expect_err("v1.4 permits exactly one delegation level");
        assert!(matches!(error, DelegationError::DepthExceeded { .. }));
    }

    #[test]
    fn path_patterns_cannot_escape_the_worktree() {
        for escape in [
            "/etc/passwd",
            "../../secrets",
            "~/.ssh/id_rsa",
            "C:/Windows/System32",
            "src/../../out",
        ] {
            assert!(
                PathPattern::parse(escape).is_err(),
                "`{escape}` must be refused"
            );
        }
        assert!(PathPattern::parse("src/auth/**").is_ok());
        // Backslashes normalize so a Windows-style pattern is judged the same.
        assert_eq!(
            PathPattern::parse("src\\auth\\**").unwrap().as_str(),
            "src/auth/**"
        );
        assert!(PathPattern::parse("src\\..\\..\\out").is_err());
    }

    #[test]
    fn an_empty_allowlist_permits_nothing() {
        let ceiling = permissive();
        let remaining = DelegationBudget::modest();
        // A writer with no paths has nothing to write to and is refused.
        assert!(matches!(
            request(&[], ExpectedOutput::Patch)
                .admit(inputs(&ceiling, &ceiling, &ceiling, &remaining)),
            Err(DelegationError::NoWritablePathRemains)
        ));
        // ...and a read-only delegation permits no path either.
        let review = request(&[], ExpectedOutput::Review)
            .admit(inputs(&ceiling, &ceiling, &ceiling, &remaining))
            .unwrap();
        assert!(!review.permits_path(Path::new("src/anything.rs")));
    }

    #[test]
    fn a_worker_that_writes_outside_its_scope_is_refused() {
        // v1.4 §9 "Scope Escape".
        let ceiling = permissive();
        let remaining = DelegationBudget::modest();
        let delegation = request(&["src/auth/**"], ExpectedOutput::Patch)
            .admit(inputs(&ceiling, &ceiling, &ceiling, &remaining))
            .unwrap();
        let result = WorkerResult {
            delegation_id: delegation.id(),
            worker_id: WorkerId::new(),
            status: WorkerResultStatus::Completed,
            summary: "did the thing".into(),
            changed_paths: vec![
                PathBuf::from("src/auth/token.rs"),
                PathBuf::from("src/payments/billing.rs"),
            ],
            patch_digest: Some("abc".into()),
            findings: Vec::new(),
            validations: Vec::new(),
            unresolved: Vec::new(),
            evidence_ids: Vec::new(),
            usage: UsageSummary::default(),
            completed_at: Utc::now(),
        };
        let error = result
            .validate_against(&delegation)
            .expect_err("payments is outside src/auth/**");
        assert!(
            matches!(error, DelegationError::ScopeEscape { ref path } if path.contains("billing"))
        );
    }

    #[test]
    fn a_read_only_worker_reporting_changes_is_refused() {
        let ceiling = permissive();
        let remaining = DelegationBudget::modest();
        let review = request(&["src/**"], ExpectedOutput::Review)
            .admit(inputs(&ceiling, &ceiling, &ceiling, &remaining))
            .unwrap();
        let result = WorkerResult {
            delegation_id: review.id(),
            worker_id: WorkerId::new(),
            status: WorkerResultStatus::Completed,
            summary: "reviewed".into(),
            changed_paths: vec![PathBuf::from("src/auth/token.rs")],
            patch_digest: Some("abc".into()),
            findings: Vec::new(),
            validations: Vec::new(),
            unresolved: Vec::new(),
            evidence_ids: Vec::new(),
            usage: UsageSummary::default(),
            completed_at: Utc::now(),
        };
        assert!(matches!(
            result.validate_against(&review),
            Err(DelegationError::ReadOnlyWorkerMutated { changed: 1 })
        ));
    }

    #[test]
    fn a_finding_without_provenance_is_refused() {
        let ceiling = permissive();
        let remaining = DelegationBudget::modest();
        let review = request(&["src/**"], ExpectedOutput::Review)
            .admit(inputs(&ceiling, &ceiling, &ceiling, &remaining))
            .unwrap();
        let finding = StructuredFinding {
            id: "f1".into(),
            title: "security issue detected".into(),
            detail: "trust me".into(),
            severity: FindingSeverity::Critical,
            path: None,
            line: None,
            evidence_ids: Vec::new(),
            recommended_action: None,
        };
        assert!(!finding.has_provenance());
        let result = WorkerResult {
            delegation_id: review.id(),
            worker_id: WorkerId::new(),
            status: WorkerResultStatus::Completed,
            summary: "reviewed".into(),
            changed_paths: Vec::new(),
            patch_digest: None,
            findings: vec![finding],
            validations: Vec::new(),
            unresolved: Vec::new(),
            evidence_ids: Vec::new(),
            usage: UsageSummary::default(),
            completed_at: Utc::now(),
        };
        assert!(matches!(
            result.validate_against(&review),
            Err(DelegationError::FindingWithoutProvenance { .. })
        ));
    }

    #[test]
    fn status_transitions_never_resurrect_a_finished_worker() {
        use DelegationStatus::*;
        assert!(Planned.can_transition_to(Ready));
        assert!(Ready.can_transition_to(Running));
        assert!(Running.can_transition_to(Completed));
        assert!(!Completed.can_transition_to(Running));
        assert!(!Completed.can_transition_to(Ready));
        assert!(!Failed.can_transition_to(Running));
        assert!(!Cancelled.can_transition_to(Running));
        // Superseding is the one way to retire a terminal record.
        assert!(Completed.can_transition_to(Superseded));
        assert!(!Superseded.can_transition_to(Superseded));
        assert!(Blocked.is_terminal());
    }

    #[test]
    fn transition_to_refuses_an_illegal_move() {
        let ceiling = permissive();
        let remaining = DelegationBudget::modest();
        let mut delegation = request(&["src/**"], ExpectedOutput::Patch)
            .admit(inputs(&ceiling, &ceiling, &ceiling, &remaining))
            .unwrap();
        delegation.transition_to(DelegationStatus::Ready).unwrap();
        delegation.transition_to(DelegationStatus::Running).unwrap();
        delegation
            .transition_to(DelegationStatus::Completed)
            .unwrap();
        assert!(matches!(
            delegation.transition_to(DelegationStatus::Running),
            Err(DelegationError::IllegalTransition { .. })
        ));
    }

    #[test]
    fn a_tampered_delegation_record_fails_to_deserialize() {
        let ceiling = permissive();
        let remaining = DelegationBudget::modest();
        let delegation = request(&["src/auth/**"], ExpectedOutput::Patch)
            .admit(inputs(&ceiling, &ceiling, &ceiling, &remaining))
            .unwrap();
        let mut json = serde_json::to_value(&delegation).unwrap();
        // Widen the scope in the persisted record.
        json["allowed_paths"] = serde_json::json!(["**"]);
        assert!(
            serde_json::from_value::<Delegation>(json).is_err(),
            "a widened record must not load"
        );

        // A clean round trip survives, including a status change made after
        // admission (the digest binds authority, not lifecycle).
        let mut advanced = delegation.clone();
        advanced.transition_to(DelegationStatus::Ready).unwrap();
        let bytes = serde_json::to_vec(&advanced).unwrap();
        let back: Delegation = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(back, advanced);
        assert_eq!(back.status(), DelegationStatus::Ready);
    }

    #[test]
    fn workspace_records_refuse_a_shared_writable_worktree() {
        let parent = PathBuf::from("/repo/.purrcode/worktrees/parent");
        let shared = WorkerWorkspaceRecord {
            parent_worktree: parent.clone(),
            worker_worktree: Some(parent.clone()),
            base_commit: "abc".into(),
            base_snapshot_digest: "digest".into(),
            access: WorkspaceAccess::Writable,
        };
        assert!(
            !shared.is_coherent(),
            "a writer sharing the parent worktree violates §5.4"
        );
        let isolated = WorkerWorkspaceRecord {
            worker_worktree: Some(parent.join("worker-a")),
            ..shared.clone()
        };
        assert!(isolated.is_coherent());
        // A reviewer gets no worktree of its own.
        let reviewer = WorkerWorkspaceRecord {
            worker_worktree: None,
            access: WorkspaceAccess::ReadOnly,
            ..shared.clone()
        };
        assert!(reviewer.is_coherent());
        let greedy_reviewer = WorkerWorkspaceRecord {
            worker_worktree: Some(parent.join("worker-b")),
            access: WorkspaceAccess::ReadOnly,
            ..shared
        };
        assert!(
            !greedy_reviewer.is_coherent(),
            "a read-only worker must not be handed a writable worktree"
        );
    }

    #[test]
    fn governance_refuses_parallelism_beyond_the_hard_maximum() {
        let governance = DelegationGovernance {
            maximum_parallel_workers: HARD_MAXIMUM_PARALLEL_WORKERS + 1,
            ..DelegationGovernance::default()
        };
        assert!(governance.validate().is_err());
        assert!(DelegationGovernance::default().validate().is_ok());
        assert_eq!(DelegationGovernance::default().effective_parallelism(10), 3);
        assert_eq!(DelegationGovernance::default().effective_parallelism(1), 1);
    }

    #[test]
    fn the_ledger_refuses_workers_once_a_ceiling_is_reached() {
        let governance = DelegationGovernance::default();
        let mut ledger = DelegationLedger::default();
        assert!(ledger.admits_another_worker(&governance, 0).is_ok());
        assert!(
            ledger
                .admits_another_worker(&governance, governance.maximum_parallel_workers)
                .is_err()
        );
        ledger.record_usage(&UsageSummary {
            input_tokens: governance.maximum_total_worker_input_tokens,
            ..UsageSummary::default()
        });
        let refusal = ledger
            .admits_another_worker(&governance, 0)
            .expect_err("an exhausted token budget must refuse");
        assert!(refusal.contains("input-token budget"));
        assert_eq!(
            ledger.remaining_budget(&governance).maximum_input_tokens,
            0,
            "no input tokens remain"
        );
    }

    #[test]
    fn repair_is_bounded_to_one_cycle_unless_the_failure_is_bounded() {
        assert_eq!(
            repair_decision(0, false),
            RepairDecision::Retry { cycle: 1 }
        );
        assert_eq!(repair_decision(1, false), RepairDecision::NeedsAttention);
        assert_eq!(repair_decision(1, true), RepairDecision::Retry { cycle: 2 });
        assert_eq!(repair_decision(2, true), RepairDecision::NeedsAttention);
    }

    #[test]
    fn review_pass_is_computed_not_claimed() {
        let review = ReviewResult {
            delegation_id: DelegationId::new(),
            worker_id: WorkerId::new(),
            findings: vec![StructuredFinding {
                id: "f1".into(),
                title: "token logged in plaintext".into(),
                detail: "…".into(),
                severity: FindingSeverity::Critical,
                path: Some(PathBuf::from("src/auth/token.rs")),
                line: Some(42),
                evidence_ids: vec![EvidenceId::new()],
                recommended_action: Some("redact".into()),
            }],
            recommended_actions: Vec::new(),
            // The reviewer claims it passed…
            pass: true,
            reviewed_at: Utc::now(),
        };
        // …but a critical finding means it did not.
        assert!(!review.computed_pass());
        assert!(review.findings_have_provenance());
        assert_eq!(review.blocking_findings().count(), 1);
    }

    #[test]
    fn usage_sums_into_the_parent_and_detects_overrun() {
        let budget = DelegationBudget {
            maximum_input_tokens: 100,
            maximum_output_tokens: 100,
            maximum_tool_calls: 2,
            maximum_duration_seconds: 10,
            maximum_changed_files: 1,
        };
        let a = UsageSummary {
            input_tokens: 60,
            tool_calls: 1,
            ..UsageSummary::default()
        };
        let b = UsageSummary {
            input_tokens: 50,
            tool_calls: 1,
            ..UsageSummary::default()
        };
        let total = a.saturating_add(&b);
        assert_eq!(total.input_tokens, 110);
        assert!(total.exceeds(&budget));
        assert!(!a.exceeds(&budget));
        assert_eq!(budget.remaining_after(&a).maximum_input_tokens, 40);
        // Saturating, never wrapping.
        assert_eq!(budget.remaining_after(&total).maximum_input_tokens, 0);
    }

    #[test]
    fn delegation_origin_explains_itself() {
        let origin = DelegationOrigin::WorkerFinding {
            delegation_id: DelegationId::new(),
            worker_id: WorkerId::new(),
            finding_id: "f1".into(),
            evidence_ids: vec![EvidenceId::new()],
        };
        let why = origin.why_included();
        assert!(why.contains("finding from delegation"));
        assert!(why.contains("1 evidence record"));
    }
}
