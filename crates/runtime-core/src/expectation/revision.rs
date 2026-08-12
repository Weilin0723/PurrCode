//! User corrections revise the contract (v1.5 §5).
//!
//! The common failure is small and expensive. The user says
//!
//! > "no, I didn't mean delete the sidebar, I just think it's cramped"
//!
//! and the agent appends that to the conversation and carries on executing a
//! plan whose first step is deleting the sidebar. Nothing in the system ever
//! recorded that a requirement was withdrawn, so nothing stops the work that
//! served it — and the evidence gathered under the old wording still reads as
//! progress.
//!
//! A correction is therefore a *revision*, not a message. Applying one answers
//! three questions at once: what the contract says now, which of the agent's
//! assumptions just died, and which already-verified work no longer proves
//! anything. The third is what lets the runtime re-plan only the affected part
//! instead of starting over or, worse, continuing.

use crate::work::RequirementId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::ExpectationError;
use super::contract::{
    Assumption, AssumptionId, ExpectationClause, ExpectationContract, ExpectationStrength,
    IntentSource, NonGoal, OpenQuestion, QuestionId, RequirementStatus, require_text,
};

/// One edit a correction makes to the contract.
#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "change")]
pub enum ContractChange {
    /// The user asked for something the contract did not cover.
    ClauseAdded {
        clause: Box<ExpectationClause>,
    },
    /// The user took a requirement back.
    ///
    /// Withdrawn rather than deleted: the clause leaves the active contract,
    /// but the revision keeps its id and statement so the record still explains
    /// why work was done that the final diff does not justify.
    ClauseWithdrawn {
        id: RequirementId,
        statement: String,
        reason: String,
    },
    /// The requirement survives but says something different now.
    ClauseRestated {
        id: RequirementId,
        from: String,
        to: String,
    },
    /// A hard requirement became a preference, or the reverse.
    StrengthChanged {
        id: RequirementId,
        from: ExpectationStrength,
        to: ExpectationStrength,
    },
    /// Give a clause something that can check it.
    ///
    /// Needed whenever a preference is promoted: a preference may have no
    /// acceptance criterion and a hard requirement may not, so promotion on its
    /// own cannot produce a valid contract. Also the honest way to record that
    /// a vague requirement was finally pinned down — and because the bar moved,
    /// evidence gathered against the old one no longer settles it.
    AcceptanceCriteriaSet {
        id: RequirementId,
        criteria: Vec<crate::work::AcceptanceCriterion>,
    },
    NonGoalAdded {
        non_goal: NonGoal,
    },
    /// A belief the agent was working from turned out to be wrong.
    AssumptionInvalidated {
        id: AssumptionId,
    },
    QuestionAnswered {
        id: QuestionId,
        answer: String,
    },
    QuestionRaised {
        question: OpenQuestion,
    },
}

/// A correction, ready to apply.
#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct ContractRevision {
    /// The revision this produces. Must be exactly one past the current one, so
    /// that two corrections racing on a stale contract cannot silently drop one.
    pub revision: u64,
    /// The user's words that prompted the change.
    pub source: IntentSource,
    /// Why the contract is changing, in the agent's words — what it understood
    /// the correction to mean. This is the sentence a user reads to check that
    /// the agent took the right lesson from what they said.
    pub reason: String,
    pub changes: Vec<ContractChange>,
}

/// The result of applying a correction.
#[derive(Clone, Debug)]
pub struct RevisedContract {
    pub contract: ExpectationContract,
    /// Requirements whose evidence no longer proves anything.
    ///
    /// A clause that was `Verified` and then restated is not still verified:
    /// the evidence was gathered against wording that no longer exists. Its
    /// status is reset here rather than left to the next reviewer to notice,
    /// because "notice that this is stale" is exactly the kind of judgement
    /// that quietly does not happen.
    pub invalidated: Vec<RequirementId>,
    /// Requirements that left the contract entirely, with the work they may
    /// have already caused.
    pub withdrawn: Vec<RequirementId>,
    /// Assumptions the correction killed.
    pub retired_assumptions: Vec<AssumptionId>,
}

impl RevisedContract {
    /// Whether this correction changed the direction of work in progress,
    /// rather than only adding to it.
    ///
    /// The distinction decides whether the runtime re-plans: adding a
    /// requirement extends the task, but withdrawing or restating one means
    /// something already done may now be wrong.
    pub fn requires_replanning(&self) -> bool {
        !self.invalidated.is_empty() || !self.withdrawn.is_empty()
    }

    /// A one-paragraph account of what changed, for the user to confirm the
    /// agent understood them (v1.5 §5).
    pub fn summary(&self) -> String {
        let mut out = String::new();
        if !self.withdrawn.is_empty() {
            out.push_str(&format!(
                "{} requirement(s) withdrawn. ",
                self.withdrawn.len()
            ));
        }
        if !self.invalidated.is_empty() {
            out.push_str(&format!(
                "{} previously-verified requirement(s) must be re-checked. ",
                self.invalidated.len()
            ));
        }
        if !self.retired_assumptions.is_empty() {
            out.push_str(&format!(
                "{} assumption(s) retired. ",
                self.retired_assumptions.len()
            ));
        }
        if out.is_empty() {
            out.push_str("Direction unchanged; the contract gained detail.");
        }
        out.trim_end().to_owned()
    }
}

impl ExpectationContract {
    /// Apply a user correction, producing the next revision.
    ///
    /// The contract is consumed and a new one returned rather than mutated in
    /// place: a half-applied correction is a contract that describes neither
    /// what the user asked for before nor what they are asking for now.
    pub fn revise(&self, revision: ContractRevision) -> Result<RevisedContract, ExpectationError> {
        if revision.revision != self.revision + 1 {
            return Err(ExpectationError::RevisionOutOfOrder {
                expected: self.revision + 1,
                found: revision.revision,
            });
        }
        require_text("revision reason", &revision.reason)?;
        require_text("revision source quotation", &revision.source.quotation)?;

        let mut next = self.clone();
        next.revision = revision.revision;
        let mut invalidated = Vec::new();
        let mut withdrawn = Vec::new();
        let mut retired_assumptions = Vec::new();

        for change in revision.changes {
            match change {
                ContractChange::ClauseAdded { clause } => {
                    if next.clause(clause.id).is_some() {
                        return Err(ExpectationError::DuplicateClause(clause.id));
                    }
                    next.clauses.push(*clause);
                }
                ContractChange::ClauseWithdrawn { id, .. } => {
                    let position = next
                        .clauses
                        .iter()
                        .position(|clause| clause.id == id)
                        .ok_or(ExpectationError::UnknownRequirement(id))?;
                    next.clauses.remove(position);
                    withdrawn.push(id);
                }
                ContractChange::ClauseRestated { id, to, .. } => {
                    require_text("restated requirement", &to)?;
                    let clause = next
                        .clause_mut(id)
                        .ok_or(ExpectationError::UnknownRequirement(id))?;
                    clause.statement = to;
                    // Evidence was gathered against the old wording.
                    if clause.status.was_checked() {
                        clause.status = RequirementStatus::Unverified;
                        invalidated.push(id);
                    }
                }
                ContractChange::StrengthChanged { id, to, .. } => {
                    let clause = next
                        .clause_mut(id)
                        .ok_or(ExpectationError::UnknownRequirement(id))?;
                    let promoted = clause.strength == ExpectationStrength::Preferred
                        && to == ExpectationStrength::Required;
                    clause.strength = to;
                    // A preference that becomes a hard requirement is now held
                    // to a standard nothing has checked it against, even if a
                    // reviewer waved it through as a nice-to-have.
                    if promoted && clause.status.was_checked() {
                        clause.status = RequirementStatus::Unverified;
                        invalidated.push(id);
                    }
                }
                ContractChange::AcceptanceCriteriaSet { id, criteria } => {
                    let clause = next
                        .clause_mut(id)
                        .ok_or(ExpectationError::UnknownRequirement(id))?;
                    clause.acceptance_criteria = criteria;
                    // The bar moved, so whatever cleared the old one proves
                    // nothing about the new one.
                    if clause.status.was_checked() {
                        clause.status = RequirementStatus::Unverified;
                        invalidated.push(id);
                    }
                }
                ContractChange::NonGoalAdded { non_goal } => {
                    require_text("non-goal", &non_goal.statement)?;
                    next.non_goals.push(non_goal);
                }
                ContractChange::AssumptionInvalidated { id } => {
                    let assumption = next
                        .assumptions
                        .iter_mut()
                        .find(|assumption| assumption.id == id)
                        .ok_or(ExpectationError::UnknownAssumption(id.0))?;
                    assumption.invalidated_by = Some(revision.revision);
                    retired_assumptions.push(id);
                }
                ContractChange::QuestionAnswered { id, answer } => {
                    require_text("answer", &answer)?;
                    let question = next
                        .open_questions
                        .iter_mut()
                        .find(|question| question.id == id)
                        .ok_or(ExpectationError::UnknownQuestion(id.0))?;
                    question.answer = Some(answer);
                }
                ContractChange::QuestionRaised { question } => {
                    require_text("open question", &question.question)?;
                    next.open_questions.push(question);
                }
            }
        }

        next.validate()?;
        // One correction can invalidate the same clause more than once — a
        // promotion that also restates it and pins down its criteria is three
        // changes to one requirement, not three requirements to re-check.
        invalidated.sort_unstable();
        invalidated.dedup();
        Ok(RevisedContract {
            contract: next,
            invalidated,
            withdrawn,
            retired_assumptions,
        })
    }
}

/// Convenience constructors used by the intent compiler and by tests.
impl ExpectationClause {
    pub fn required(
        statement: impl Into<String>,
        criteria: Vec<crate::work::AcceptanceCriterion>,
        source: IntentSource,
    ) -> Self {
        Self {
            id: RequirementId::new(),
            statement: statement.into(),
            strength: ExpectationStrength::Required,
            acceptance_criteria: criteria,
            status: RequirementStatus::Unverified,
            source,
        }
    }

    pub fn preferred(statement: impl Into<String>, source: IntentSource) -> Self {
        Self {
            id: RequirementId::new(),
            statement: statement.into(),
            strength: ExpectationStrength::Preferred,
            acceptance_criteria: Vec::new(),
            source,
            status: RequirementStatus::Unverified,
        }
    }
}

impl Assumption {
    pub fn new(statement: impl Into<String>) -> Self {
        Self {
            id: AssumptionId::new(),
            statement: statement.into(),
            invalidated_by: None,
        }
    }
}

impl OpenQuestion {
    pub fn new(question: impl Into<String>, blocking: bool) -> Self {
        Self {
            id: QuestionId::new(),
            question: question.into(),
            blocking,
            answer: None,
        }
    }
}

impl IntentSource {
    pub fn new(message_index: u64, quotation: impl Into<String>) -> Self {
        Self {
            message_index,
            quotation: quotation.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::expectation::contract::RequirementTally;
    use crate::work::{AcceptanceCriterion, CriterionId, EvidenceId};

    fn said(turn: u64, words: &str) -> IntentSource {
        IntentSource::new(turn, words)
    }

    fn criterion(statement: &str) -> AcceptanceCriterion {
        AcceptanceCriterion {
            id: CriterionId::new(),
            statement: statement.into(),
        }
    }

    fn verified() -> RequirementStatus {
        RequirementStatus::Verified {
            evidence: vec![EvidenceId::new()],
        }
    }

    /// The scenario from v1.5 §5: the agent understood "the sidebar is cramped"
    /// as "remove the sidebar", did it, and verified it.
    fn sidebar_contract() -> (ExpectationContract, RequirementId) {
        let mut contract = ExpectationContract::new("Improve the settings layout");
        let mut clause = ExpectationClause::required(
            "remove the sidebar",
            vec![criterion("the sidebar is gone")],
            said(0, "the sidebar feels really cramped"),
        );
        clause.status = verified();
        let id = clause.id;
        contract.clauses.push(clause);
        contract.assumptions.push(Assumption::new(
            "the user wants the sidebar gone rather than tidied",
        ));
        contract.validate().unwrap();
        (contract, id)
    }

    #[test]
    fn a_correction_stops_verified_work_from_still_counting_as_progress() {
        // Without this, the user says "no, keep the sidebar" and the contract
        // still reports the deletion as a satisfied requirement.
        let (contract, id) = sidebar_contract();
        let assumption = contract.assumptions[0].id;
        assert_eq!(contract.tally().settled(), 1);

        let revised = contract
            .revise(ContractRevision {
                revision: 2,
                source: said(1, "no, I didn't mean delete it, it's just cramped"),
                reason: "the user wants the sidebar kept and made less dense".into(),
                changes: vec![
                    ContractChange::ClauseRestated {
                        id,
                        from: "remove the sidebar".into(),
                        to: "keep the sidebar and reduce its information density".into(),
                    },
                    ContractChange::AssumptionInvalidated { id: assumption },
                ],
            })
            .expect("the correction applies");

        assert_eq!(revised.contract.revision, 2);
        assert_eq!(
            revised.contract.clause(id).unwrap().statement,
            "keep the sidebar and reduce its information density"
        );
        assert_eq!(
            revised.contract.clause(id).unwrap().status,
            RequirementStatus::Unverified,
            "evidence gathered against the old wording cannot vouch for the new one"
        );
        assert_eq!(revised.invalidated, vec![id]);
        assert!(revised.requires_replanning());
        assert_eq!(revised.retired_assumptions.len(), 1);
        assert!(!revised.contract.assumptions[0].holds());
        assert_eq!(revised.contract.assumptions[0].invalidated_by, Some(2));
        assert_eq!(
            revised.contract.tally(),
            RequirementTally {
                unverified: 1,
                ..RequirementTally::default()
            },
            "the task went back to having nothing verified, which is the truth"
        );
    }

    #[test]
    fn withdrawing_a_requirement_removes_it_and_asks_for_a_replan() {
        let (contract, id) = sidebar_contract();
        let revised = contract
            .revise(ContractRevision {
                revision: 2,
                source: said(1, "actually forget the sidebar entirely"),
                reason: "the user dropped the sidebar work".into(),
                changes: vec![ContractChange::ClauseWithdrawn {
                    id,
                    statement: "remove the sidebar".into(),
                    reason: "the user withdrew it".into(),
                }],
            })
            .unwrap();
        assert!(revised.contract.clause(id).is_none());
        assert_eq!(revised.withdrawn, vec![id]);
        assert!(revised.requires_replanning());
        assert_eq!(revised.contract.tally().total(), 0);
    }

    #[test]
    fn adding_detail_is_not_a_change_of_direction() {
        // Only withdrawal and restatement invalidate work. Adding a requirement
        // extends the task, and re-planning everything on every clarification
        // would make the agent unusable.
        let (contract, _) = sidebar_contract();
        let revised = contract
            .revise(ContractRevision {
                revision: 2,
                source: said(1, "also the model picker should be easy to find"),
                reason: "the user added a requirement".into(),
                changes: vec![ContractChange::ClauseAdded {
                    clause: Box::new(ExpectationClause::required(
                        "model selection is reachable without opening advanced settings",
                        vec![criterion("the picker is in the default view")],
                        said(1, "the model picker should be easy to find"),
                    )),
                }],
            })
            .unwrap();
        assert!(!revised.requires_replanning());
        assert!(revised.summary().contains("Direction unchanged"));
        assert_eq!(revised.contract.tally().total(), 2);
        assert_eq!(
            revised.contract.tally().settled(),
            1,
            "the already-verified requirement is untouched"
        );
    }

    #[test]
    fn promoting_a_preference_to_a_requirement_makes_it_prove_itself_again() {
        // It may have been waved through as a nice-to-have. Against a hard bar,
        // nothing has actually checked it.
        let mut contract = ExpectationContract::new("Improve the settings layout");
        let mut clause = ExpectationClause::preferred(
            "reduce visual clutter",
            said(0, "don't make it cluttered"),
        );
        clause.status = verified();
        let id = clause.id;
        contract.clauses.push(clause);
        contract.validate().unwrap();

        let revised = contract
            .revise(ContractRevision {
                revision: 2,
                source: said(1, "the clutter thing is not optional, it's the whole point"),
                reason: "the user made decluttering a hard requirement".into(),
                changes: vec![
                    ContractChange::StrengthChanged {
                        id,
                        from: ExpectationStrength::Preferred,
                        to: ExpectationStrength::Required,
                    },
                    // Promotion means it now needs something that can check it.
                    ContractChange::ClauseRestated {
                        id,
                        from: "reduce visual clutter".into(),
                        to: "the default settings view shows only common controls".into(),
                    },
                    ContractChange::AcceptanceCriteriaSet {
                        id,
                        criteria: vec![criterion("advanced controls sit behind a disclosure")],
                    },
                ],
            })
            .unwrap();
        assert_eq!(
            revised.contract.clause(id).unwrap().status,
            RequirementStatus::Unverified
        );
        assert!(revised.requires_replanning());
        assert_eq!(
            revised.contract.tally().total(),
            1,
            "it is a hard requirement now"
        );
        assert_eq!(
            revised.invalidated,
            vec![id],
            "three changes to one clause is one requirement to re-check, not three"
        );
    }

    #[test]
    fn a_revision_that_does_not_follow_the_current_one_is_refused() {
        // Two corrections racing on a stale contract would otherwise drop one
        // silently, which is the failure this whole crate is about.
        let (contract, _) = sidebar_contract();
        let error = contract
            .revise(ContractRevision {
                revision: 5,
                source: said(1, "keep the sidebar"),
                reason: "stale".into(),
                changes: vec![],
            })
            .unwrap_err();
        assert!(
            matches!(
                error,
                ExpectationError::RevisionOutOfOrder {
                    expected: 2,
                    found: 5
                }
            ),
            "got {error:?}"
        );
    }

    #[test]
    fn a_revision_cannot_leave_the_contract_invalid() {
        // Promoting to Required without giving it anything to check against
        // would produce a contract that cannot be verified but claims it must
        // be. The revision fails rather than storing that.
        let mut contract = ExpectationContract::new("Improve the settings layout");
        let clause =
            ExpectationClause::preferred("reduce visual clutter", said(0, "keep it simple"));
        let id = clause.id;
        contract.clauses.push(clause);
        contract.validate().unwrap();

        let error = contract
            .revise(ContractRevision {
                revision: 2,
                source: said(1, "decluttering is mandatory"),
                reason: "promote the preference".into(),
                changes: vec![ContractChange::StrengthChanged {
                    id,
                    from: ExpectationStrength::Preferred,
                    to: ExpectationStrength::Required,
                }],
            })
            .unwrap_err();
        assert!(
            matches!(error, ExpectationError::UncheckableRequirement(_)),
            "got {error:?}"
        );
    }

    #[test]
    fn answering_a_blocking_question_unblocks_it() {
        let mut contract = ExpectationContract::new("Add OAuth");
        let question = OpenQuestion::new("which provider should be supported first?", true);
        let id = question.id;
        contract.open_questions.push(question);
        contract.validate().unwrap();
        assert_eq!(contract.blocking_questions().len(), 1);

        let revised = contract
            .revise(ContractRevision {
                revision: 2,
                source: said(1, "start with GitHub"),
                reason: "the user chose a provider".into(),
                changes: vec![ContractChange::QuestionAnswered {
                    id,
                    answer: "GitHub".into(),
                }],
            })
            .unwrap();
        assert!(revised.contract.blocking_questions().is_empty());
        assert!(!revised.requires_replanning());
    }

    #[test]
    fn a_correction_summary_says_what_actually_changed() {
        let (contract, id) = sidebar_contract();
        let revised = contract
            .revise(ContractRevision {
                revision: 2,
                source: said(1, "keep the sidebar"),
                reason: "the user wants the sidebar kept".into(),
                changes: vec![ContractChange::ClauseRestated {
                    id,
                    from: "remove the sidebar".into(),
                    to: "keep the sidebar and reduce its density".into(),
                }],
            })
            .unwrap();
        let summary = revised.summary();
        assert!(summary.contains("re-checked"), "{summary}");
    }
}
