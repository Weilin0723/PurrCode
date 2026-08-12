//! The finding contract (v1.5 §27–§28).
//!
//! Every review in v1.5 — deterministic checks, requirement coverage,
//! independent code review, user-alignment review — returns the same shape.
//! One shape means the delivery gate can count blocking findings without
//! knowing which reviewer produced them, and the correction loop can attribute
//! a repair without a per-reviewer special case.
//!
//! The rule that makes the findings worth anything is in §28: **reviewers
//! judge, repair agents repair.** A reviewer that fixes what it finds has
//! destroyed its own evidence — the finding and the fix become one unverifiable
//! act, and nothing independent ever confirmed the problem was real. So a
//! finding is a *claim with a location*, never a patch.

use std::path::PathBuf;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::work::RequirementId;

macro_rules! review_id {
    ($name:ident) => {
        #[derive(
            Clone,
            Copy,
            Debug,
            Eq,
            Hash,
            schemars::JsonSchema,
            Ord,
            PartialEq,
            PartialOrd,
            serde::Serialize,
            serde::Deserialize,
        )]
        #[serde(transparent)]
        pub struct $name(pub uuid::Uuid);

        impl $name {
            pub fn new() -> Self {
                Self(uuid::Uuid::new_v4())
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }
    };
}
review_id!(FindingId);
review_id!(ReviewId);

/// Which review produced a finding.
///
/// Kept on the finding because the four reviews have different standing. A
/// deterministic check that says the build is broken is not a matter of
/// opinion; an alignment reviewer's reading of "too cluttered" is. Recording
/// which one spoke lets the user weigh them differently instead of receiving
/// one undifferentiated list.
#[derive(
    Clone, Copy, Debug, Eq, JsonSchema, Ord, PartialEq, PartialOrd, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum ReviewKind {
    /// Build, tests, lint, types, formatter — no model involved (§7).
    Deterministic,
    /// Does the diff actually satisfy each requirement, with evidence (§8)?
    RequirementCoverage,
    /// Correctness, regressions, security, architecture — read without the
    /// implementer's transcript (§9).
    IndependentCode,
    /// Does this satisfy what the user asked for, as opposed to what the code
    /// says (§10)?
    UserAlignment,
}

impl ReviewKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Deterministic => "deterministic verification",
            Self::RequirementCoverage => "requirement coverage",
            Self::IndependentCode => "code review",
            Self::UserAlignment => "alignment review",
        }
    }
}

/// How much a finding matters.
#[derive(
    Clone, Copy, Debug, Eq, JsonSchema, Ord, PartialEq, PartialOrd, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    /// Record it; it does not stop anything.
    Info,
    Low,
    /// The agent decides whether it is worth fixing now.
    Medium,
    High,
    Critical,
}

impl Severity {
    /// Whether a finding at this severity stops delivery on its own.
    ///
    /// `High` and `Critical` do. `Medium` deliberately does not: a gate that
    /// blocks on every medium-severity opinion produces an agent that never
    /// finishes, and the user asked for one that stops when the work matches
    /// what they asked for — not one that stops when nothing is left to say.
    pub fn blocks_delivery(self) -> bool {
        matches!(self, Self::High | Self::Critical)
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Info => "info",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Critical => "critical",
        }
    }
}

/// What kind of problem a finding describes.
#[derive(
    Clone, Copy, Debug, Eq, JsonSchema, Ord, PartialEq, PartialOrd, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum FindingCategory {
    Correctness,
    Regression,
    /// The implementation does not cover a requirement it claimed to.
    RequirementGap,
    /// It satisfies the letter of the requirement and not the point of it.
    UxMismatch,
    Security,
    Architecture,
    Testing,
    /// Work nobody asked for (§29).
    ScopeDrift,
}

/// One reviewer's claim about the work.
#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct ReviewFinding {
    pub id: FindingId,
    pub review: ReviewId,
    pub kind: ReviewKind,
    pub severity: Severity,
    pub category: FindingCategory,
    /// Which requirement this bears on, when it bears on one. A `RequirementGap`
    /// without one is a complaint with nowhere to land.
    #[serde(default)]
    pub requirement_id: Option<RequirementId>,
    pub description: String,
    /// What the reviewer actually looked at.
    ///
    /// Required, and not merely conventional: a finding whose evidence is empty
    /// is an assertion, and the correction loop would spend a repair cycle on
    /// something nothing established. The same rule the contract applies to
    /// `Verified` applies here in the other direction.
    pub evidence: Vec<String>,
    #[serde(default)]
    pub affected_paths: Vec<PathBuf>,
    /// What to do about it — advice, never a patch (§28).
    pub recommendation: String,
}

impl ReviewFinding {
    /// Whether this finding stops delivery.
    pub fn blocks_delivery(&self) -> bool {
        self.severity.blocks_delivery()
    }

    /// Whether the correction loop should try to fix this without asking.
    ///
    /// Blocking findings are repaired automatically because leaving them means
    /// not delivering at all. Everything below that is reported: spending model
    /// budget on `Low` opinions is how a task that was finished an hour ago is
    /// still running.
    pub fn invites_automatic_repair(&self) -> bool {
        self.severity.blocks_delivery()
    }

    pub fn validate(&self) -> Result<(), ReviewError> {
        if self.description.trim().is_empty() {
            return Err(ReviewError::Invalid(
                "a finding must say what is wrong".into(),
            ));
        }
        if self.recommendation.trim().is_empty() {
            return Err(ReviewError::Invalid(
                "a finding must say what to do about it".into(),
            ));
        }
        if self.evidence.iter().all(|item| item.trim().is_empty()) {
            return Err(ReviewError::UnevidencedFinding(self.id));
        }
        if self.category == FindingCategory::RequirementGap && self.requirement_id.is_none() {
            return Err(ReviewError::UnattributedRequirementGap(self.id));
        }
        Ok(())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ReviewError {
    #[error("{0}")]
    Invalid(String),
    #[error("finding {0:?} claims a problem with no evidence behind it")]
    UnevidencedFinding(FindingId),
    #[error("finding {0:?} reports a requirement gap without saying which requirement")]
    UnattributedRequirementGap(FindingId),
    #[error("review {id:?} is a {kind:?} review but was given the implementer's transcript")]
    ContaminatedReview { id: ReviewId, kind: ReviewKind },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn finding(severity: Severity, category: FindingCategory) -> ReviewFinding {
        ReviewFinding {
            id: FindingId::new(),
            review: ReviewId::new(),
            kind: ReviewKind::IndependentCode,
            severity,
            category,
            requirement_id: None,
            description: "the settings panel still shows every advanced control".into(),
            evidence: vec!["settings.rs:221".into()],
            affected_paths: vec![PathBuf::from("src/settings.rs")],
            recommendation: "move provider internals behind a disclosure".into(),
        }
    }

    #[test]
    fn a_finding_with_no_evidence_is_an_assertion() {
        // Otherwise the correction loop spends a repair cycle on something
        // nothing established.
        let mut claim = finding(Severity::High, FindingCategory::Correctness);
        claim.evidence = vec!["   ".into()];
        assert!(matches!(
            claim.validate().unwrap_err(),
            ReviewError::UnevidencedFinding(_)
        ));
    }

    #[test]
    fn a_requirement_gap_must_name_the_requirement_it_is_about() {
        let claim = finding(Severity::High, FindingCategory::RequirementGap);
        assert!(matches!(
            claim.validate().unwrap_err(),
            ReviewError::UnattributedRequirementGap(_)
        ));
    }

    #[test]
    fn a_finding_must_say_what_to_do_about_it() {
        let mut claim = finding(Severity::Medium, FindingCategory::Architecture);
        claim.recommendation = "  ".into();
        assert!(matches!(
            claim.validate().unwrap_err(),
            ReviewError::Invalid(_)
        ));
    }

    #[test]
    fn only_high_and_critical_stop_delivery() {
        // A gate that blocks on every medium-severity opinion produces an agent
        // that never finishes.
        assert!(!Severity::Info.blocks_delivery());
        assert!(!Severity::Low.blocks_delivery());
        assert!(!Severity::Medium.blocks_delivery());
        assert!(Severity::High.blocks_delivery());
        assert!(Severity::Critical.blocks_delivery());
    }

    #[test]
    fn automatic_repair_tracks_what_blocks_rather_than_what_is_merely_noted() {
        assert!(
            finding(Severity::Critical, FindingCategory::Correctness).invites_automatic_repair()
        );
        assert!(!finding(Severity::Low, FindingCategory::Architecture).invites_automatic_repair());
    }
}

// ---------------------------------------------------------------------------
// Review records
// ---------------------------------------------------------------------------

/// What a reviewer is allowed to read (v1.5 §9).
///
/// The independent code reviewer and the alignment reviewer are given the
/// contract, the repository rules, the diff and the test results — and *not*
/// the implementer's transcript. This is a containment boundary rather than a
/// prompt-shortening measure: a reviewer that has read "I've made this really
/// clean now" is reviewing a conclusion it has already been handed, and the
/// most common result is that it agrees.
///
/// Deterministic and requirement-coverage reviews are unaffected — they read
/// artefacts, not narrative.
#[derive(Clone, Copy, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewContext {
    /// Contract, repository rules, diff, validation results.
    Fresh,
    /// The full session, including the implementer's reasoning.
    Inherited,
}

impl ReviewKind {
    /// Whether this review must be run without the implementer's transcript.
    ///
    /// Enforced as data rather than left to whoever assembles the prompt: the
    /// failure mode is silent, and a reviewer that quietly inherited the
    /// transcript still produces confident-looking findings.
    pub fn requires_fresh_context(self) -> bool {
        matches!(self, Self::IndependentCode | Self::UserAlignment)
    }
}

/// A review that ran, or is running.
#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct ReviewRecord {
    pub id: ReviewId,
    pub kind: ReviewKind,
    pub context: ReviewContext,
    /// Which correction cycle this review belongs to. Zero is the first review,
    /// before any repair — so a finding's cycle says whether it survived a
    /// repair attempt or was only just discovered.
    #[serde(default)]
    pub cycle: u32,
    #[serde(default)]
    pub completed: bool,
    #[serde(default)]
    pub findings: Vec<FindingId>,
}

impl ReviewRecord {
    pub fn validate(&self) -> Result<(), ReviewError> {
        if self.kind.requires_fresh_context() && self.context != ReviewContext::Fresh {
            return Err(ReviewError::ContaminatedReview {
                id: self.id,
                kind: self.kind,
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod review_record_tests {
    use super::*;

    fn record(kind: ReviewKind, context: ReviewContext) -> ReviewRecord {
        ReviewRecord {
            id: ReviewId::new(),
            kind,
            context,
            cycle: 0,
            completed: false,
            findings: vec![],
        }
    }

    #[test]
    fn an_independent_reviewer_that_read_the_transcript_is_not_independent() {
        // The failure is silent: it still produces confident-looking findings,
        // it just mostly agrees with the implementer.
        for kind in [ReviewKind::IndependentCode, ReviewKind::UserAlignment] {
            let error = record(kind, ReviewContext::Inherited)
                .validate()
                .unwrap_err();
            assert!(
                matches!(error, ReviewError::ContaminatedReview { .. }),
                "{kind:?} must be refused: {error}"
            );
            record(kind, ReviewContext::Fresh)
                .validate()
                .expect("fresh context is what these reviews require");
        }
    }

    #[test]
    fn artefact_reviews_do_not_need_a_fresh_context() {
        // They read the build output and the diff; there is no narrative to be
        // captured by.
        for kind in [ReviewKind::Deterministic, ReviewKind::RequirementCoverage] {
            assert!(!kind.requires_fresh_context());
            record(kind, ReviewContext::Inherited).validate().unwrap();
        }
    }
}
