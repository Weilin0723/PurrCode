//! Evidence-gated integration (v1.4 §PR7).
//!
//! Worker completion is not merge authority. A finished worker has produced a
//! *proposal*; between that proposal and the parent worktree sit five checks —
//! base, scope, conflicts, evidence, PawGate — and this module owns the pure
//! parts of them.
//!
//! The rule that matters most is the one about same-hunk overlap: two workers
//! that edited the same lines produce an [`IntegrationConflict`], never a
//! last-write-win. Losing one worker's change silently is the failure mode this
//! whole pipeline exists to prevent.

use super::{
    Delegation, DelegationError, DelegationId, FindingSeverity, WorkerId, WorkerResult,
    path_is_contained,
};
use crate::ValidationStatus;
use crate::work::EvidenceId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

/// One hunk of a unified diff, in the coordinates the *old* (base) file uses.
///
/// Overlap is judged on old-file coordinates because that is the shared frame:
/// two patches built from the same base disagree exactly when they rewrite the
/// same base lines, whatever their new-file line numbers turn out to be.
#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct PatchHunk {
    pub path: PathBuf,
    pub old_start: u32,
    pub old_lines: u32,
    pub new_start: u32,
    pub new_lines: u32,
}

impl PatchHunk {
    /// Inclusive `[start, end]` range of base lines this hunk rewrites.
    ///
    /// A pure insertion (`old_lines == 0`) still occupies a position: git
    /// reports it as `@@ -N,0` meaning "after base line N". Treating it as an
    /// empty range would let two workers insert different code at the same
    /// point and call it conflict-free, so an insertion claims the single line
    /// it is anchored to.
    pub fn base_range(&self) -> (u32, u32) {
        if self.old_lines == 0 {
            (self.old_start, self.old_start)
        } else {
            (self.old_start, self.old_start + self.old_lines - 1)
        }
    }

    /// True when two hunks rewrite overlapping base lines.
    pub fn overlaps(&self, other: &PatchHunk) -> bool {
        if self.path != other.path {
            return false;
        }
        let (a_start, a_end) = self.base_range();
        let (b_start, b_end) = other.base_range();
        a_start <= b_end && b_start <= a_end
    }
}

/// Parse the hunk headers out of a unified diff.
///
/// Deliberately a header parser, not a patch applier: git remains the only
/// thing that applies patches. What this needs to answer is "which base lines
/// does this patch touch, in which files", which is exactly what the `---`/
/// `+++`/`@@` headers say.
///
/// Non-UTF-8 bytes in the body are ignored (only headers are read), and a
/// malformed header is skipped rather than guessed at — an unparsed hunk means
/// its file is treated as touched with an unknown range, which
/// [`detect_conflicts`] handles conservatively.
pub fn parse_unified_diff(patch: &[u8]) -> Vec<PatchHunk> {
    let text = String::from_utf8_lossy(patch);
    let mut hunks = Vec::new();
    let mut current_path: Option<PathBuf> = None;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("+++ ") {
            current_path = strip_diff_prefix(rest.trim());
            continue;
        }
        if line.starts_with("--- ") {
            // `--- /dev/null` for a new file; the `+++` line names the path.
            continue;
        }
        if line.starts_with("diff --git ") {
            current_path = None;
            continue;
        }
        let Some(rest) = line.strip_prefix("@@ ") else {
            continue;
        };
        let Some(path) = current_path.clone() else {
            continue;
        };
        let Some((old, new)) = parse_hunk_header(rest) else {
            continue;
        };
        hunks.push(PatchHunk {
            path,
            old_start: old.0,
            old_lines: old.1,
            new_start: new.0,
            new_lines: new.1,
        });
    }
    hunks
}

/// `a/src/lib.rs` / `b/src/lib.rs` → `src/lib.rs`; `/dev/null` → `None`.
fn strip_diff_prefix(raw: &str) -> Option<PathBuf> {
    // Git appends a tab and metadata to the path on some diffs.
    let raw = raw.split('\t').next().unwrap_or(raw).trim();
    if raw.is_empty() || raw == "/dev/null" {
        return None;
    }
    let stripped = raw
        .strip_prefix("b/")
        .or_else(|| raw.strip_prefix("a/"))
        .unwrap_or(raw);
    let path = PathBuf::from(stripped);
    // A patch naming an absolute or traversing path is not something to trust
    // into a conflict decision.
    path_is_contained(&path).then_some(path)
}

/// `-12,7 +12,9 @@ fn context` → `((12, 7), (12, 9))`.
fn parse_hunk_header(rest: &str) -> Option<((u32, u32), (u32, u32))> {
    let body = rest.split("@@").next()?.trim();
    let mut parts = body.split_whitespace();
    let old = parts.next()?.strip_prefix('-')?;
    let new = parts.next()?.strip_prefix('+')?;
    Some((parse_range(old)?, parse_range(new)?))
}

/// `12,7` → `(12, 7)`; a bare `12` means one line.
fn parse_range(raw: &str) -> Option<(u32, u32)> {
    match raw.split_once(',') {
        Some((start, count)) => Some((start.parse().ok()?, count.parse().ok()?)),
        None => Some((raw.parse().ok()?, 1)),
    }
}

/// Why two proposals cannot both be applied as-is.
#[derive(
    Clone, Copy, Debug, Eq, JsonSchema, Ord, PartialEq, PartialOrd, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum IntegrationConflictKind {
    /// Two workers rewrote the same base lines. Never auto-merged.
    SameHunk,
    /// Two workers changed the same file in disjoint places. A deterministic
    /// merge is attempted, then validated.
    SameFileDifferentHunks,
    /// The worker's base no longer matches the parent's current snapshot.
    BaseDrift,
    /// The patch touches a path outside the delegation's scope.
    ScopeViolation,
}

impl IntegrationConflictKind {
    /// True when the parent may attempt an automatic merge. `SameHunk` never
    /// qualifies: that is the last-write-win the release gate forbids.
    pub fn is_auto_mergeable(self) -> bool {
        matches!(self, IntegrationConflictKind::SameFileDifferentHunks)
    }
}

#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct IntegrationConflict {
    pub path: PathBuf,
    pub kind: IntegrationConflictKind,
    /// The delegations involved, always in a stable order so two runs of the
    /// same conflict read identically.
    pub delegations: Vec<DelegationId>,
    pub detail: String,
}

/// One worker's proposal to change the parent workspace (v1.4 §PR7).
#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct IntegrationProposal {
    pub delegation_id: DelegationId,
    pub worker_id: WorkerId,
    pub patch_digest: String,
    pub changed_paths: Vec<PathBuf>,
    /// The parent snapshot the worker branched from.
    pub base_snapshot_digest: String,
    pub evidence_ids: Vec<EvidenceId>,
    pub validation_summary: ValidationSummary,
    #[serde(default)]
    pub conflicts: Vec<IntegrationConflict>,
    /// Set when the user accepted a subset of hunks: the patch that will be
    /// applied is no longer the worker's, so it gets its own digest
    /// (v1.4 §PR12).
    #[serde(default)]
    pub amended_patch_digest: Option<String>,
}

impl IntegrationProposal {
    /// The digest of what will actually be applied — the amended patch when the
    /// user selected hunks, otherwise the worker's own.
    ///
    /// Reusing the worker's digest after an edit would make evidence claim the
    /// reviewed patch and the applied patch were the same bytes.
    pub fn effective_patch_digest(&self) -> &str {
        self.amended_patch_digest
            .as_deref()
            .unwrap_or(&self.patch_digest)
    }

    pub fn was_amended(&self) -> bool {
        self.amended_patch_digest.is_some()
    }
}

/// Rolled-up validation state for one proposal.
#[derive(Clone, Debug, Default, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct ValidationSummary {
    pub passed: u32,
    pub failed: u32,
    pub skipped: u32,
    /// Highest severity among the worker's own findings, if any.
    #[serde(default)]
    pub peak_finding_severity: Option<FindingSeverity>,
}

impl ValidationSummary {
    pub fn from_result(result: &WorkerResult) -> Self {
        let mut summary = ValidationSummary {
            peak_finding_severity: result.peak_severity(),
            ..ValidationSummary::default()
        };
        for validation in &result.validations {
            match validation.status {
                ValidationStatus::Passed => summary.passed += 1,
                ValidationStatus::Failed | ValidationStatus::TimedOut => summary.failed += 1,
                _ => summary.skipped += 1,
            }
        }
        summary
    }

    /// A proposal is evidence-clean when nothing failed and no blocking finding
    /// was raised. "No validations ran" is *not* clean — it is unproven, and the
    /// caller decides whether unproven is acceptable for this delegation.
    pub fn is_clean(&self) -> bool {
        self.failed == 0
            && !self
                .peak_finding_severity
                .is_some_and(FindingSeverity::blocks_integration)
    }

    pub fn has_evidence(&self) -> bool {
        self.passed + self.failed + self.skipped > 0
    }
}

/// What the integration coordinator decided.
#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(tag = "decision", rename_all = "snake_case")]
pub enum IntegrationDecision {
    /// Checks passed; the patch may go to PawGate for human approval.
    ReadyForApproval,
    /// Conflicts must be resolved by the main agent or the user first.
    Conflict { conflicts: Vec<IntegrationConflict> },
    /// Structurally refused — scope escape, base drift, missing evidence.
    Rejected { reason: String },
}

impl IntegrationDecision {
    pub fn is_ready(&self) -> bool {
        matches!(self, IntegrationDecision::ReadyForApproval)
    }
}

/// Detect conflicts among a set of proposals (v1.4 §PR7 "Conflict Rules").
///
/// `hunks` maps each delegation to the hunks its patch contains. Separate files
/// are conflict-free; the same file in disjoint places is auto-mergeable; the
/// same base lines is a hard [`IntegrationConflictKind::SameHunk`].
///
/// A file that appears in `changed_paths` but contributed no parsable hunk (a
/// binary file, or a diff this parser did not understand) is treated as
/// touching the *whole* file: two workers claiming it conflict. Guessing the
/// other way would auto-merge a binary asset two workers both rewrote.
pub fn detect_conflicts(
    hunks: &BTreeMap<DelegationId, Vec<PatchHunk>>,
    opaque_paths: &BTreeMap<DelegationId, Vec<PathBuf>>,
) -> Vec<IntegrationConflict> {
    let mut conflicts: Vec<IntegrationConflict> = Vec::new();
    let ids: Vec<DelegationId> = hunks
        .keys()
        .chain(opaque_paths.keys())
        .copied()
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();

    for (index, left) in ids.iter().enumerate() {
        for right in ids.iter().skip(index + 1) {
            let left_hunks = hunks.get(left).map(Vec::as_slice).unwrap_or(&[]);
            let right_hunks = hunks.get(right).map(Vec::as_slice).unwrap_or(&[]);
            let left_opaque = opaque_paths.get(left).map(Vec::as_slice).unwrap_or(&[]);
            let right_opaque = opaque_paths.get(right).map(Vec::as_slice).unwrap_or(&[]);

            // Same file, unknown extent on either side → conflict on the file.
            let mut recorded: std::collections::BTreeSet<PathBuf> =
                std::collections::BTreeSet::new();
            for path in left_opaque {
                let touched_by_right = right_opaque.contains(path)
                    || right_hunks.iter().any(|hunk| &hunk.path == path);
                if touched_by_right && recorded.insert(path.clone()) {
                    conflicts.push(IntegrationConflict {
                        path: path.clone(),
                        kind: IntegrationConflictKind::SameHunk,
                        delegations: vec![*left, *right],
                        detail: "both workers changed this file and its extent could not be \
                                 determined, so an automatic merge is not safe"
                            .into(),
                    });
                }
            }
            for path in right_opaque {
                let touched_by_left = left_hunks.iter().any(|hunk| &hunk.path == path);
                if touched_by_left && recorded.insert(path.clone()) {
                    conflicts.push(IntegrationConflict {
                        path: path.clone(),
                        kind: IntegrationConflictKind::SameHunk,
                        delegations: vec![*left, *right],
                        detail: "both workers changed this file and its extent could not be \
                                 determined, so an automatic merge is not safe"
                            .into(),
                    });
                }
            }

            // Hunk-level comparison for the files both sides described.
            let mut same_file_paths: std::collections::BTreeSet<PathBuf> =
                std::collections::BTreeSet::new();
            for left_hunk in left_hunks {
                for right_hunk in right_hunks {
                    if left_hunk.path != right_hunk.path {
                        continue;
                    }
                    if recorded.contains(&left_hunk.path) {
                        continue;
                    }
                    if left_hunk.overlaps(right_hunk) {
                        if recorded.insert(left_hunk.path.clone()) {
                            let (start, end) = left_hunk.base_range();
                            let (other_start, other_end) = right_hunk.base_range();
                            conflicts.push(IntegrationConflict {
                                path: left_hunk.path.clone(),
                                kind: IntegrationConflictKind::SameHunk,
                                delegations: vec![*left, *right],
                                detail: format!(
                                    "base lines {start}-{end} and {other_start}-{other_end} \
                                     overlap; the parent must decide"
                                ),
                            });
                        }
                    } else {
                        same_file_paths.insert(left_hunk.path.clone());
                    }
                }
            }
            for path in same_file_paths {
                if recorded.contains(&path) {
                    continue;
                }
                conflicts.push(IntegrationConflict {
                    path: path.clone(),
                    kind: IntegrationConflictKind::SameFileDifferentHunks,
                    delegations: vec![*left, *right],
                    detail: "both workers changed this file in disjoint places; a deterministic \
                             merge may be attempted and then validated"
                        .into(),
                });
            }
        }
    }
    conflicts
        .sort_by(|a, b| (&a.path, a.kind, &a.delegations).cmp(&(&b.path, b.kind, &b.delegations)));
    conflicts
}

/// Build a proposal from a validated worker result (v1.4 §PR7 pipeline steps
/// 1–3).
///
/// `current_base_snapshot_digest` is the parent's snapshot *now*. A worker whose
/// base has drifted cannot have its patch applied blindly, because the lines it
/// rewrote may no longer be the lines it read.
pub fn propose_integration(
    delegation: &Delegation,
    result: &WorkerResult,
    worker_base_snapshot_digest: &str,
    current_base_snapshot_digest: &str,
    hunks: Vec<PatchHunk>,
) -> Result<IntegrationProposal, DelegationError> {
    // Scope and budget first: a result that escaped its scope never becomes a
    // proposal at all.
    result.validate_against(delegation)?;

    let mut conflicts = Vec::new();
    if worker_base_snapshot_digest != current_base_snapshot_digest {
        conflicts.push(IntegrationConflict {
            path: PathBuf::new(),
            kind: IntegrationConflictKind::BaseDrift,
            delegations: vec![delegation.id()],
            detail: format!(
                "worker branched from snapshot {} but the parent is now at {}",
                short_digest(worker_base_snapshot_digest),
                short_digest(current_base_snapshot_digest)
            ),
        });
    }
    // A hunk naming a path outside the delegated scope is a scope violation even
    // when `changed_paths` looked clean — the patch is the authority on what it
    // touches.
    for hunk in &hunks {
        if !delegation.permits_path(&hunk.path) {
            conflicts.push(IntegrationConflict {
                path: hunk.path.clone(),
                kind: IntegrationConflictKind::ScopeViolation,
                delegations: vec![delegation.id()],
                detail: "the patch modifies a path outside the delegated scope".into(),
            });
        }
    }

    Ok(IntegrationProposal {
        delegation_id: delegation.id(),
        worker_id: result.worker_id,
        patch_digest: result.patch_digest.clone().unwrap_or_default(),
        changed_paths: result.changed_paths.clone(),
        base_snapshot_digest: worker_base_snapshot_digest.to_owned(),
        evidence_ids: result.evidence_ids.clone(),
        validation_summary: ValidationSummary::from_result(result),
        conflicts,
        amended_patch_digest: None,
    })
}

/// The gate itself: may this proposal go to PawGate?
///
/// `require_evidence` is the delegation's policy: a code change with no
/// validation at all is unproven, and for a writer delegation that is a
/// refusal, not a warning.
pub fn evaluate_proposal(
    proposal: &IntegrationProposal,
    cross_worker_conflicts: &[IntegrationConflict],
    require_evidence: bool,
) -> IntegrationDecision {
    let mut conflicts = proposal.conflicts.clone();
    conflicts.extend(cross_worker_conflicts.iter().cloned());

    if let Some(violation) = conflicts
        .iter()
        .find(|conflict| conflict.kind == IntegrationConflictKind::ScopeViolation)
    {
        return IntegrationDecision::Rejected {
            reason: format!(
                "the patch modifies `{}`, which is outside the delegated scope",
                violation.path.display()
            ),
        };
    }

    if !conflicts.is_empty() {
        return IntegrationDecision::Conflict { conflicts };
    }

    if require_evidence && !proposal.validation_summary.has_evidence() {
        return IntegrationDecision::Rejected {
            reason: "the worker proposed changes with no validation evidence".into(),
        };
    }

    if !proposal.validation_summary.is_clean() {
        return IntegrationDecision::Rejected {
            reason: "validation failed or a blocking finding was raised".into(),
        };
    }

    IntegrationDecision::ReadyForApproval
}

fn short_digest(digest: &str) -> &str {
    &digest[..digest.len().min(12)]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::delegation::{
        AuthorityInputs, DelegationBudget, DelegationRequest, ExpectedOutput, PathPattern,
        StructuredFinding, UsageSummary, ValidationEvidence, WorkerResultStatus,
    };
    use crate::{
        ApprovalPolicy, CapabilityId, FilesystemScope, NetworkScope, SessionId, SideEffectClass,
        ToolCeiling, TurnId,
    };
    use chrono::Utc;

    const SAMPLE: &str = "\
diff --git a/src/auth/token.rs b/src/auth/token.rs
--- a/src/auth/token.rs
+++ b/src/auth/token.rs
@@ -10,6 +10,9 @@ fn exchange() {
     let a = 1;
+    let b = 2;
@@ -80,3 +83,4 @@ fn refresh() {
     ok()
+    more()
diff --git a/src/auth/store.rs b/src/auth/store.rs
--- /dev/null
+++ b/src/auth/store.rs
@@ -0,0 +1,12 @@
+pub struct Store;
";

    #[test]
    fn unified_diff_headers_parse_into_hunks() {
        let hunks = parse_unified_diff(SAMPLE.as_bytes());
        assert_eq!(hunks.len(), 3);
        assert_eq!(hunks[0].path, PathBuf::from("src/auth/token.rs"));
        assert_eq!(hunks[0].old_start, 10);
        assert_eq!(hunks[0].old_lines, 6);
        assert_eq!(hunks[1].old_start, 80);
        assert_eq!(hunks[2].path, PathBuf::from("src/auth/store.rs"));
        // A new file: no base lines, but it still claims a position.
        assert_eq!(hunks[2].old_lines, 0);
        assert_eq!(hunks[2].base_range(), (0, 0));
    }

    #[test]
    fn a_bare_line_number_means_one_line() {
        let patch = "--- a/x.rs\n+++ b/x.rs\n@@ -5 +5 @@\n-a\n+b\n";
        let hunks = parse_unified_diff(patch.as_bytes());
        assert_eq!(hunks.len(), 1);
        assert_eq!(hunks[0].base_range(), (5, 5));
    }

    #[test]
    fn a_patch_naming_a_traversing_path_is_ignored() {
        let patch = "--- a/../../etc/passwd\n+++ b/../../etc/passwd\n@@ -1 +1 @@\n-a\n+b\n";
        assert!(
            parse_unified_diff(patch.as_bytes()).is_empty(),
            "a traversing path must not become a hunk"
        );
    }

    fn hunk(path: &str, old_start: u32, old_lines: u32) -> PatchHunk {
        PatchHunk {
            path: PathBuf::from(path),
            old_start,
            old_lines,
            new_start: old_start,
            new_lines: old_lines,
        }
    }

    #[test]
    fn separate_files_do_not_conflict() {
        let a = DelegationId::new();
        let b = DelegationId::new();
        let hunks = BTreeMap::from([
            (a, vec![hunk("src/auth/token.rs", 10, 5)]),
            (b, vec![hunk("migrations/0007.sql", 1, 20)]),
        ]);
        assert!(detect_conflicts(&hunks, &BTreeMap::new()).is_empty());
    }

    #[test]
    fn the_same_file_in_disjoint_places_is_auto_mergeable() {
        let a = DelegationId::new();
        let b = DelegationId::new();
        let hunks = BTreeMap::from([
            (a, vec![hunk("src/lib.rs", 10, 5)]),
            (b, vec![hunk("src/lib.rs", 100, 5)]),
        ]);
        let conflicts = detect_conflicts(&hunks, &BTreeMap::new());
        assert_eq!(conflicts.len(), 1);
        assert_eq!(
            conflicts[0].kind,
            IntegrationConflictKind::SameFileDifferentHunks
        );
        assert!(conflicts[0].kind.is_auto_mergeable());
    }

    #[test]
    fn the_same_hunk_is_never_auto_merged() {
        // The v1.4 release gate: two workers editing the same hunk produce an
        // explicit conflict, never a last-write-win.
        let a = DelegationId::new();
        let b = DelegationId::new();
        let hunks = BTreeMap::from([
            (a, vec![hunk("src/lib.rs", 10, 5)]),
            (b, vec![hunk("src/lib.rs", 12, 4)]),
        ]);
        let conflicts = detect_conflicts(&hunks, &BTreeMap::new());
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].kind, IntegrationConflictKind::SameHunk);
        assert!(!conflicts[0].kind.is_auto_mergeable());
        assert!(conflicts[0].detail.contains("overlap"));
    }

    #[test]
    fn two_insertions_at_the_same_anchor_conflict() {
        // Both are `old_lines == 0`, so a naive empty-range check would call
        // them disjoint and silently keep one.
        let a = DelegationId::new();
        let b = DelegationId::new();
        let hunks = BTreeMap::from([
            (a, vec![hunk("src/lib.rs", 42, 0)]),
            (b, vec![hunk("src/lib.rs", 42, 0)]),
        ]);
        let conflicts = detect_conflicts(&hunks, &BTreeMap::new());
        assert_eq!(conflicts[0].kind, IntegrationConflictKind::SameHunk);
    }

    #[test]
    fn a_file_with_an_unknown_extent_conflicts_conservatively() {
        let a = DelegationId::new();
        let b = DelegationId::new();
        let hunks = BTreeMap::from([(b, vec![hunk("assets/logo.png", 1, 1)])]);
        let opaque = BTreeMap::from([(a, vec![PathBuf::from("assets/logo.png")])]);
        let conflicts = detect_conflicts(&hunks, &opaque);
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].kind, IntegrationConflictKind::SameHunk);
    }

    fn permissive() -> ToolCeiling {
        ToolCeiling {
            maximum_side_effect: SideEffectClass::Destructive,
            maximum_network: NetworkScope::Any,
            maximum_filesystem: FilesystemScope::maximum(),
            minimum_approval: ApprovalPolicy::ByClass,
            denied_tool_ids: Default::default(),
        }
    }

    fn delegation(paths: &[&str]) -> Delegation {
        let ceiling = permissive();
        let remaining = DelegationBudget::modest();
        DelegationRequest {
            parent_session_id: SessionId::new(),
            parent_turn_id: TurnId::new(),
            objective: "implement".into(),
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

    fn result(delegation: &Delegation, paths: &[&str], validations: bool) -> WorkerResult {
        WorkerResult {
            delegation_id: delegation.id(),
            worker_id: WorkerId::new(),
            status: WorkerResultStatus::Completed,
            summary: "done".into(),
            changed_paths: paths.iter().map(PathBuf::from).collect(),
            patch_digest: Some("patch-digest".into()),
            findings: Vec::new(),
            validations: if validations {
                vec![ValidationEvidence {
                    name: "cargo test".into(),
                    status: ValidationStatus::Passed,
                    detail: "42 passed".into(),
                    evidence_id: Some(EvidenceId::new()),
                }]
            } else {
                Vec::new()
            },
            unresolved: Vec::new(),
            evidence_ids: vec![EvidenceId::new()],
            usage: UsageSummary::default(),
            completed_at: Utc::now(),
        }
    }

    #[test]
    fn a_clean_proposal_is_ready_for_approval() {
        let delegation = delegation(&["src/auth/**"]);
        let result = result(&delegation, &["src/auth/token.rs"], true);
        let proposal = propose_integration(
            &delegation,
            &result,
            "base-digest",
            "base-digest",
            vec![hunk("src/auth/token.rs", 10, 5)],
        )
        .unwrap();
        assert!(proposal.conflicts.is_empty());
        assert!(evaluate_proposal(&proposal, &[], true).is_ready());
    }

    #[test]
    fn base_drift_is_a_conflict_not_a_silent_apply() {
        let delegation = delegation(&["src/auth/**"]);
        let result = result(&delegation, &["src/auth/token.rs"], true);
        let proposal = propose_integration(
            &delegation,
            &result,
            "base-digest",
            "moved-on-digest",
            vec![hunk("src/auth/token.rs", 10, 5)],
        )
        .unwrap();
        match evaluate_proposal(&proposal, &[], true) {
            IntegrationDecision::Conflict { conflicts } => {
                assert_eq!(conflicts[0].kind, IntegrationConflictKind::BaseDrift);
            }
            other => panic!("expected a base-drift conflict, got {other:?}"),
        }
    }

    #[test]
    fn a_patch_touching_an_unscoped_path_is_rejected() {
        // `changed_paths` looked clean, but the patch itself names a file
        // outside the scope — the patch wins the argument.
        let delegation = delegation(&["src/auth/**"]);
        let result = result(&delegation, &["src/auth/token.rs"], true);
        let proposal = propose_integration(
            &delegation,
            &result,
            "base",
            "base",
            vec![
                hunk("src/auth/token.rs", 10, 5),
                hunk("src/payments/billing.rs", 1, 3),
            ],
        )
        .unwrap();
        match evaluate_proposal(&proposal, &[], true) {
            IntegrationDecision::Rejected { reason } => assert!(reason.contains("billing")),
            other => panic!("expected rejection, got {other:?}"),
        }
    }

    #[test]
    fn changes_without_validation_evidence_are_refused_when_evidence_is_required() {
        let delegation = delegation(&["src/auth/**"]);
        let result = result(&delegation, &["src/auth/token.rs"], false);
        let proposal =
            propose_integration(&delegation, &result, "base", "base", Vec::new()).unwrap();
        assert!(!proposal.validation_summary.has_evidence());
        match evaluate_proposal(&proposal, &[], true) {
            IntegrationDecision::Rejected { reason } => assert!(reason.contains("no validation")),
            other => panic!("expected rejection, got {other:?}"),
        }
        // …and permitted when the delegation does not require it.
        assert!(evaluate_proposal(&proposal, &[], false).is_ready());
    }

    #[test]
    fn a_blocking_finding_stops_integration() {
        let delegation = delegation(&["src/auth/**"]);
        let mut worker_result = result(&delegation, &["src/auth/token.rs"], true);
        worker_result.findings.push(StructuredFinding {
            id: "f1".into(),
            title: "token logged".into(),
            detail: "…".into(),
            severity: FindingSeverity::Critical,
            path: Some(PathBuf::from("src/auth/token.rs")),
            line: Some(12),
            evidence_ids: vec![EvidenceId::new()],
            recommended_action: None,
        });
        let proposal =
            propose_integration(&delegation, &worker_result, "base", "base", Vec::new()).unwrap();
        assert!(!proposal.validation_summary.is_clean());
        assert!(!evaluate_proposal(&proposal, &[], true).is_ready());
    }

    #[test]
    fn an_amended_patch_gets_its_own_digest() {
        // v1.4 §PR12: selected-hunk integration must not reuse the worker's
        // digest — the bytes being applied are different bytes.
        let delegation = delegation(&["src/auth/**"]);
        let result = result(&delegation, &["src/auth/token.rs"], true);
        let mut proposal =
            propose_integration(&delegation, &result, "base", "base", Vec::new()).unwrap();
        assert_eq!(proposal.effective_patch_digest(), "patch-digest");
        assert!(!proposal.was_amended());
        proposal.amended_patch_digest = Some("subset-digest".into());
        assert_eq!(proposal.effective_patch_digest(), "subset-digest");
        assert!(proposal.was_amended());
    }

    #[test]
    fn a_scope_escaping_result_never_becomes_a_proposal() {
        let delegation = delegation(&["src/auth/**"]);
        let result = result(&delegation, &["src/payments/billing.rs"], true);
        assert!(matches!(
            propose_integration(&delegation, &result, "base", "base", Vec::new()),
            Err(DelegationError::ScopeEscape { .. })
        ));
    }
}
