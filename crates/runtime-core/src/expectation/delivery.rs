//! The delivery gate (v1.5 §12).
//!
//! The behaviour this module exists to produce is one sentence long:
//!
//! > **Completion is a state transition, not something the model declares.**
//!
//! An agent asked whether it is finished will usually say yes. It is the most
//! natural continuation of a transcript full of work, and no amount of
//! instruction reliably beats that — which is why "please don't say you're done
//! unless you really are" has never worked in any coding agent. So the model is
//! not asked. The gate reads the contract, the findings, the validations and
//! the diff, and computes an answer the model has no way to overrule.
//!
//! The four outcomes are deliberately not one boolean. "Not ready" collapses
//! three situations that need different responses from the *user*: work still
//! to do, a decision only they can make, and something actively broken.

use std::collections::BTreeSet;
use std::path::PathBuf;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::contract::{ExpectationContract, QuestionId, RequirementStatus, RequirementTally};
use crate::ValidationStatus;
use crate::review::{FindingId, ReviewFinding};
use crate::work::RequirementId;

/// What the gate concluded.
#[derive(
    Clone, Copy, Debug, Eq, JsonSchema, Ord, PartialEq, PartialOrd, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryState {
    /// Every hard requirement is verified or waived, nothing blocking is
    /// outstanding, and the work is inside the scope it was given.
    Ready,
    /// Nothing is wrong; there is simply work left.
    PartiallyComplete,
    /// Progress needs a human: a blocking question, or a requirement the
    /// reviewers checked and could not settle.
    NeedsDecision,
    /// Something is broken or contradicts what was asked for.
    Blocked,
}

impl DeliveryState {
    pub fn label(self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::PartiallyComplete => "partially complete",
            Self::NeedsDecision => "needs decision",
            Self::Blocked => "blocked",
        }
    }

    /// Whether the agent may present the work as finished.
    pub fn may_report_done(self) -> bool {
        self == Self::Ready
    }
}

/// One specific reason delivery did not clear.
#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "blocker")]
pub enum DeliveryBlocker {
    /// A hard requirement nothing has verified yet.
    RequirementOutstanding {
        id: RequirementId,
        statement: String,
    },
    /// A hard requirement the implementation contradicts.
    RequirementViolated {
        id: RequirementId,
        statement: String,
        detail: String,
    },
    /// A hard requirement that was checked and could not be settled.
    RequirementUnknown {
        id: RequirementId,
        statement: String,
        detail: String,
    },
    /// A review finding at a severity that stops delivery.
    BlockingFinding { id: FindingId, description: String },
    /// A validation the task requires that did not pass.
    ValidationFailed { name: String, status: String },
    /// The diff touched something the contract ruled out (§29).
    ScopeEscape { path: PathBuf, non_goal: String },
    /// A worker integration that never resolved (v1.4 carried forward).
    UnresolvedConflict { detail: String },
    /// A question the agent could not answer for itself.
    BlockingQuestion { id: QuestionId, question: String },
}

impl DeliveryBlocker {
    /// Which outcome this blocker implies on its own.
    ///
    /// The gate takes the worst across all of them, so a single broken thing
    /// cannot be averaged away by a long list of merely-outstanding work.
    pub fn implies(&self) -> DeliveryState {
        match self {
            Self::RequirementOutstanding { .. } => DeliveryState::PartiallyComplete,
            Self::RequirementUnknown { .. } | Self::BlockingQuestion { .. } => {
                DeliveryState::NeedsDecision
            }
            Self::RequirementViolated { .. }
            | Self::BlockingFinding { .. }
            | Self::ValidationFailed { .. }
            | Self::ScopeEscape { .. }
            | Self::UnresolvedConflict { .. } => DeliveryState::Blocked,
        }
    }

    pub fn describe(&self) -> String {
        match self {
            Self::RequirementOutstanding { statement, .. } => {
                format!("not yet verified: {statement}")
            }
            Self::RequirementViolated {
                statement, detail, ..
            } => format!("not satisfied: {statement} — {detail}"),
            Self::RequirementUnknown {
                statement, detail, ..
            } => format!("could not be established: {statement} — {detail}"),
            Self::BlockingFinding { description, .. } => description.clone(),
            Self::ValidationFailed { name, status } => format!("{name} did not pass ({status})"),
            Self::ScopeEscape { path, non_goal } => format!(
                "{} was changed, and the task rules out: {non_goal}",
                path.display()
            ),
            Self::UnresolvedConflict { detail } => format!("unresolved integration: {detail}"),
            Self::BlockingQuestion { question, .. } => format!("unanswered: {question}"),
        }
    }
}

/// A validation the task required, and how it went.
#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct RequiredValidation {
    pub name: String,
    pub status: ValidationStatus,
}

impl RequiredValidation {
    /// Only an outright pass clears.
    ///
    /// `Unavailable`, `NotDetected` and `Uncertain` are not passes. A gate that
    /// treats "we could not run the tests" as "the tests are fine" is the false
    /// `Done` wearing a different hat.
    pub fn cleared(&self) -> bool {
        self.status == ValidationStatus::Passed
    }
}

/// Everything the gate reads. Nothing here is the model's opinion of itself.
#[derive(Clone, Copy, Debug)]
pub struct DeliveryInputs<'a> {
    pub contract: &'a ExpectationContract,
    /// Findings that are **still open**.
    ///
    /// Pass `SessionState::outstanding_findings()`, not the whole `findings`
    /// map — that is the full history and never shrinks, so a finding the
    /// correction loop already repaired would block delivery forever.
    pub findings: &'a [ReviewFinding],
    pub validations: &'a [RequiredValidation],
    /// Paths the diff touched, for the scope check.
    pub changed_paths: &'a [PathBuf],
    /// Non-goals expressed as path prefixes, when the contract's non-goals were
    /// concrete enough to be mechanical. Prose non-goals are the alignment
    /// reviewer's job; these are the ones a machine can settle.
    pub forbidden_prefixes: &'a [(PathBuf, String)],
    pub unresolved_conflicts: &'a [String],
}

/// The gate's answer, with every reason it reached it.
#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct DeliveryAssessment {
    pub state: DeliveryState,
    pub blockers: Vec<DeliveryBlocker>,
    pub tally: RequirementTally,
    /// Findings recorded but not blocking, so the summary can mention them
    /// without holding the work.
    pub advisory_findings: usize,
}

impl DeliveryAssessment {
    /// The sentence the user reads (§34).
    pub fn summary(&self) -> String {
        let mut out = format!("{} — {}", self.state.label(), self.tally);
        if !self.blockers.is_empty() {
            out.push_str(&format!("\n{} outstanding:", self.blockers.len()));
            for blocker in &self.blockers {
                out.push_str(&format!("\n  · {}", blocker.describe()));
            }
        }
        if self.advisory_findings > 0 {
            out.push_str(&format!(
                "\n{} advisory finding(s) recorded, not blocking.",
                self.advisory_findings
            ));
        }
        out
    }
}

/// Evaluate whether this task may be reported as done.
///
/// Every blocker is collected rather than short-circuiting on the first, because
/// a user who fixes the one thing they were told about only to be handed another
/// has been given a worse experience than one who saw the list.
pub fn evaluate(inputs: DeliveryInputs<'_>) -> DeliveryAssessment {
    let mut blockers = Vec::new();

    for clause in inputs.contract.required() {
        match &clause.status {
            RequirementStatus::Verified { .. } | RequirementStatus::Waived { .. } => {}
            RequirementStatus::Unverified => {
                blockers.push(DeliveryBlocker::RequirementOutstanding {
                    id: clause.id,
                    statement: clause.statement.clone(),
                })
            }
            RequirementStatus::Violated { detail, .. } => {
                blockers.push(DeliveryBlocker::RequirementViolated {
                    id: clause.id,
                    statement: clause.statement.clone(),
                    detail: detail.clone(),
                })
            }
            RequirementStatus::Unknown { detail } => {
                blockers.push(DeliveryBlocker::RequirementUnknown {
                    id: clause.id,
                    statement: clause.statement.clone(),
                    detail: detail.clone(),
                })
            }
        }
    }

    let mut advisory_findings = 0;
    for finding in inputs.findings {
        if finding.blocks_delivery() {
            blockers.push(DeliveryBlocker::BlockingFinding {
                id: finding.id,
                description: finding.description.clone(),
            });
        } else {
            advisory_findings += 1;
        }
    }

    for validation in inputs.validations {
        if !validation.cleared() {
            blockers.push(DeliveryBlocker::ValidationFailed {
                name: validation.name.clone(),
                status: format!("{:?}", validation.status),
            });
        }
    }

    // Scope is checked against what the diff actually touched, not against what
    // the agent said it did.
    let mut reported: BTreeSet<&PathBuf> = BTreeSet::new();
    for path in inputs.changed_paths {
        for (prefix, non_goal) in inputs.forbidden_prefixes {
            if path.starts_with(prefix) && reported.insert(path) {
                blockers.push(DeliveryBlocker::ScopeEscape {
                    path: path.clone(),
                    non_goal: non_goal.clone(),
                });
            }
        }
    }

    for conflict in inputs.unresolved_conflicts {
        blockers.push(DeliveryBlocker::UnresolvedConflict {
            detail: conflict.clone(),
        });
    }

    for question in inputs.contract.blocking_questions() {
        blockers.push(DeliveryBlocker::BlockingQuestion {
            id: question.id,
            question: question.question.clone(),
        });
    }

    let state = blockers
        .iter()
        .map(DeliveryBlocker::implies)
        .max()
        .unwrap_or(DeliveryState::Ready);

    DeliveryAssessment {
        state,
        blockers,
        tally: inputs.contract.tally(),
        advisory_findings,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::expectation::{ExpectationClause, IntentSource, OpenQuestion};
    use crate::review::{FindingCategory, ReviewFinding, ReviewId, ReviewKind, Severity};
    use crate::work::{AcceptanceCriterion, CriterionId, EvidenceId};

    fn criterion() -> AcceptanceCriterion {
        AcceptanceCriterion {
            id: CriterionId::new(),
            statement: "a user can do it".into(),
        }
    }

    fn source() -> IntentSource {
        IntentSource::new(0, "make MCP configuration actually work")
    }

    fn verified_clause(statement: &str) -> ExpectationClause {
        let mut clause = ExpectationClause::required(statement, vec![criterion()], source());
        clause.status = RequirementStatus::Verified {
            evidence: vec![EvidenceId::new()],
        };
        clause
    }

    fn satisfied_contract() -> ExpectationContract {
        let mut contract = ExpectationContract::new("Improve the Settings experience");
        contract
            .clauses
            .push(verified_clause("MCP configuration works"));
        contract
            .clauses
            .push(verified_clause("model configuration works"));
        contract.validate().unwrap();
        contract
    }

    fn finding(severity: Severity) -> ReviewFinding {
        ReviewFinding {
            id: FindingId::new(),
            review: ReviewId::new(),
            kind: ReviewKind::UserAlignment,
            severity,
            category: FindingCategory::UxMismatch,
            requirement_id: None,
            description: "advanced controls are still visible by default".into(),
            evidence: vec!["settings.rs:221".into()],
            affected_paths: vec![],
            recommendation: "move them behind a disclosure".into(),
        }
    }

    fn inputs<'a>(
        contract: &'a ExpectationContract,
        findings: &'a [ReviewFinding],
        validations: &'a [RequiredValidation],
    ) -> DeliveryInputs<'a> {
        DeliveryInputs {
            contract,
            findings,
            validations,
            changed_paths: &[],
            forbidden_prefixes: &[],
            unresolved_conflicts: &[],
        }
    }

    fn passing() -> Vec<RequiredValidation> {
        vec![RequiredValidation {
            name: "cargo test".into(),
            status: ValidationStatus::Passed,
        }]
    }

    #[test]
    fn everything_verified_and_nothing_blocking_is_ready() {
        let contract = satisfied_contract();
        let validations = passing();
        let assessment = evaluate(inputs(&contract, &[], &validations));
        assert_eq!(assessment.state, DeliveryState::Ready);
        assert!(assessment.state.may_report_done());
        assert!(assessment.blockers.is_empty());
        assert_eq!(assessment.tally.to_string(), "2 / 2 requirements verified");
    }

    #[test]
    fn a_blocking_finding_stops_a_fully_verified_contract() {
        // The case that matters most: every requirement is verified and the
        // agent would happily report success. The reviewer disagreed, and the
        // reviewer is not overrulable by the thing being reviewed.
        let contract = satisfied_contract();
        let findings = vec![finding(Severity::High)];
        let validations = passing();
        let assessment = evaluate(inputs(&contract, &findings, &validations));
        assert_eq!(assessment.state, DeliveryState::Blocked);
        assert!(!assessment.state.may_report_done());
        assert_eq!(assessment.blockers.len(), 1);
    }

    #[test]
    fn advisory_findings_are_recorded_without_holding_the_work() {
        let contract = satisfied_contract();
        let findings = vec![finding(Severity::Low), finding(Severity::Medium)];
        let validations = passing();
        let assessment = evaluate(inputs(&contract, &findings, &validations));
        assert_eq!(assessment.state, DeliveryState::Ready);
        assert_eq!(assessment.advisory_findings, 2);
        assert!(assessment.summary().contains("2 advisory finding(s)"));
    }

    #[test]
    fn a_validation_that_could_not_run_is_not_a_pass() {
        // "We could not run the tests" must not read as "the tests are fine".
        for status in [
            ValidationStatus::Unavailable,
            ValidationStatus::NotDetected,
            ValidationStatus::Uncertain,
            ValidationStatus::TimedOut,
            ValidationStatus::Failed,
        ] {
            let contract = satisfied_contract();
            let validations = vec![RequiredValidation {
                name: "cargo test".into(),
                status: status.clone(),
            }];
            let assessment = evaluate(inputs(&contract, &[], &validations));
            assert_eq!(
                assessment.state,
                DeliveryState::Blocked,
                "{status:?} must not clear the gate"
            );
        }
    }

    #[test]
    fn an_unknown_requirement_asks_a_human_rather_than_shipping_or_failing() {
        let mut contract = satisfied_contract();
        contract.clauses[0].status = RequirementStatus::Unknown {
            detail: "could not tell whether the panel feels cluttered".into(),
        };
        let validations = passing();
        let assessment = evaluate(inputs(&contract, &[], &validations));
        assert_eq!(assessment.state, DeliveryState::NeedsDecision);
        assert!(!assessment.state.may_report_done());
    }

    #[test]
    fn outstanding_work_is_partial_rather_than_broken() {
        let mut contract = satisfied_contract();
        contract.clauses[0].status = RequirementStatus::Unverified;
        let validations = passing();
        let assessment = evaluate(inputs(&contract, &[], &validations));
        assert_eq!(assessment.state, DeliveryState::PartiallyComplete);
        assert_eq!(assessment.tally.to_string(), "1 / 2 requirements verified");
    }

    #[test]
    fn the_worst_blocker_decides_and_the_rest_are_still_listed() {
        // A user who fixes the one thing they were told about, only to be handed
        // another, has had a worse experience than one who saw the list.
        let mut contract = satisfied_contract();
        contract.clauses[0].status = RequirementStatus::Unverified;
        contract.clauses[1].status = RequirementStatus::Violated {
            evidence: vec![EvidenceId::new()],
            detail: "the picker is still three clicks deep".into(),
        };
        let validations = passing();
        let assessment = evaluate(inputs(&contract, &[], &validations));
        assert_eq!(assessment.state, DeliveryState::Blocked);
        assert_eq!(assessment.blockers.len(), 2, "both are reported");
    }

    #[test]
    fn touching_a_forbidden_path_blocks_however_good_the_work_was() {
        let contract = satisfied_contract();
        let validations = passing();
        let changed = vec![PathBuf::from("src/editor/layout.rs")];
        let forbidden = vec![(
            PathBuf::from("src/editor"),
            "redesign the editor".to_string(),
        )];
        let assessment = evaluate(DeliveryInputs {
            contract: &contract,
            findings: &[],
            validations: &validations,
            changed_paths: &changed,
            forbidden_prefixes: &forbidden,
            unresolved_conflicts: &[],
        });
        assert_eq!(assessment.state, DeliveryState::Blocked);
        assert!(assessment.summary().contains("redesign the editor"));
    }

    #[test]
    fn a_blocking_question_needs_the_user_not_another_attempt() {
        let mut contract = satisfied_contract();
        contract
            .open_questions
            .push(OpenQuestion::new("which provider comes first?", true));
        let validations = passing();
        let assessment = evaluate(inputs(&contract, &[], &validations));
        assert_eq!(assessment.state, DeliveryState::NeedsDecision);
    }

    #[test]
    fn a_non_blocking_question_does_not_hold_delivery() {
        // Most questions are not blocking; a competent colleague picks the
        // obvious reading and says which one they picked.
        let mut contract = satisfied_contract();
        contract
            .open_questions
            .push(OpenQuestion::new("should the icon be blue?", false));
        let validations = passing();
        assert_eq!(
            evaluate(inputs(&contract, &[], &validations)).state,
            DeliveryState::Ready
        );
    }

    #[test]
    fn an_empty_contract_with_nothing_to_check_is_ready() {
        // Degenerate but worth pinning: the gate must not invent a blocker, and
        // must not pretend a task with no hard requirements is unfinished.
        let contract = ExpectationContract::new("look into something");
        let assessment = evaluate(inputs(&contract, &[], &[]));
        assert_eq!(assessment.state, DeliveryState::Ready);
        assert_eq!(assessment.tally.total(), 0);
    }
}
