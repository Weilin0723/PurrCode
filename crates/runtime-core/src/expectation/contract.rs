//! What the user actually asked for, written down (v1.5 §3).
//!
//! The failure this module exists to prevent is not a model that writes bad
//! code. It is a model that writes good code for a task nobody asked for:
//! build passes, tests pass, lint passes, and the settings page is still
//! unusable because "don't clutter the UI" was never stored anywhere except in
//! a message that later got summarised into "the user wants better settings".
//!
//! So the contract is written down once, revised explicitly when the user
//! corrects course, and re-injected in full at every planning, coding and
//! review step. It is deliberately small — small enough to survive in every
//! prompt, because a contract too large to re-inject is a contract the model
//! will be asked to remember, and remembering is the thing that failed.
//!
//! The contract is *durable control state*, not conversation. Conversation may
//! be compacted freely. This may not.

use std::collections::BTreeSet;
use std::fmt;

use crate::work::{AcceptanceCriterion, EvidenceId, RequirementId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::ExpectationError;

pub const EXPECTATION_SCHEMA_VERSION: u32 = 1;

// ---------------------------------------------------------------------------
// Provenance
// ---------------------------------------------------------------------------

/// Where a clause came from.
///
/// Provenance is what lets a contract be audited rather than trusted. A
/// requirement with no source is one the agent invented, and inventing
/// requirements is exactly as damaging as forgetting them — it is how "make
/// settings simpler" turns into a redesign nobody sanctioned.
#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct IntentSource {
    /// Which user turn the clause was read from, counting from zero.
    pub message_index: u64,
    /// The user's own words, kept verbatim.
    ///
    /// Paraphrase is where intent quietly drifts: "don't clutter the UI"
    /// becomes "simplify the UI" becomes "remove options", and each step looks
    /// reasonable next to the one before it. Keeping the quotation means a
    /// reviewer can check the requirement against what was actually said
    /// instead of against the previous paraphrase.
    pub quotation: String,
}

// ---------------------------------------------------------------------------
// Strength
// ---------------------------------------------------------------------------

/// How firmly the user asked for something.
///
/// The distinction is load-bearing rather than descriptive: only
/// [`ExpectationStrength::Required`] can block delivery. Treating every stated
/// wish as blocking produces an agent that never finishes; treating none of
/// them as blocking produces the agent this release exists to replace.
#[derive(Clone, Copy, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExpectationStrength {
    /// The task is not done while this is unmet.
    Required,
    /// Genuinely wanted, recorded and reported — but the user would trade it
    /// away before they would trade away a hard requirement, so it does not
    /// hold a finished task hostage.
    Preferred,
}

// ---------------------------------------------------------------------------
// Status
// ---------------------------------------------------------------------------

/// What is currently known about whether a clause holds.
///
/// Five states rather than a boolean, because the three ways of *not* being
/// verified need different responses: `Unverified` means go and check,
/// `Unknown` means the check could not settle it and a human may need to,
/// `Violated` means stop and fix. Collapsing them loses the instruction.
#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "state")]
pub enum RequirementStatus {
    /// Nothing has checked this yet.
    Unverified,
    /// Checked, with evidence that says it holds.
    Verified { evidence: Vec<EvidenceId> },
    /// Checked, and the implementation contradicts it.
    Violated {
        evidence: Vec<EvidenceId>,
        detail: String,
    },
    /// Checked, and the check could not tell.
    ///
    /// Distinct from `Unverified` because "we looked and could not establish
    /// it" is information worth surfacing, and distinct from `Verified`
    /// because it is not a pass. This is the state an honest reviewer reaches
    /// for when it would otherwise be tempted to round up.
    Unknown { detail: String },
    /// A human released the agent from this requirement.
    Waived {
        reason: String,
        source: IntentSource,
    },
}

impl RequirementStatus {
    /// Whether delivery may proceed past this clause.
    ///
    /// Only genuine verification and an explicit human waiver qualify. In
    /// particular `Unknown` does not: an agent that ships on "I could not tell"
    /// is the false-`Done` this release is built to stop.
    pub fn clears_delivery(&self) -> bool {
        matches!(self, Self::Verified { .. } | Self::Waived { .. })
    }

    /// Whether something has actually looked at this clause.
    pub fn was_checked(&self) -> bool {
        !matches!(self, Self::Unverified)
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::Unverified => "unverified",
            Self::Verified { .. } => "verified",
            Self::Violated { .. } => "violated",
            Self::Unknown { .. } => "unknown",
            Self::Waived { .. } => "waived",
        }
    }
}

// ---------------------------------------------------------------------------
// Clause
// ---------------------------------------------------------------------------

/// One thing the user asked for, and what is known about it.
#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct ExpectationClause {
    pub id: RequirementId,
    /// The requirement in one sentence, in the product's terms rather than the
    /// implementation's. "The user can add, edit and remove an MCP server" —
    /// not "wire `McpConfig` into `settings.rs`", which is a plan, and plans
    /// change without the requirement changing.
    pub statement: String,
    pub strength: ExpectationStrength,
    /// How anyone would tell whether the clause holds.
    #[serde(default)]
    pub acceptance_criteria: Vec<AcceptanceCriterion>,
    #[serde(default = "unverified")]
    pub status: RequirementStatus,
    pub source: IntentSource,
}

fn unverified() -> RequirementStatus {
    RequirementStatus::Unverified
}

impl ExpectationClause {
    pub fn is_required(&self) -> bool {
        self.strength == ExpectationStrength::Required
    }

    /// A required clause that has not cleared delivery.
    pub fn blocks_delivery(&self) -> bool {
        self.is_required() && !self.status.clears_delivery()
    }
}

// ---------------------------------------------------------------------------
// Supporting clauses
// ---------------------------------------------------------------------------

/// Something the user explicitly did not ask for.
///
/// Recorded separately from requirements because scope drift is not a failure
/// to do something — it is doing something extra, and you cannot detect that
/// by checking a list of things that must be true.
#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct NonGoal {
    pub statement: String,
    pub source: IntentSource,
    /// The part of the tree this non-goal puts out of bounds, when the user's
    /// words were concrete enough to name one.
    ///
    /// "Don't touch the editor" is checkable by a machine; "don't make it ugly"
    /// is the alignment reviewer's problem. Recording the prefix here rather
    /// than handing it to the gate at call time is what makes the scope check
    /// derivable from durable state — a caller cannot quietly narrow the bounds
    /// by passing a shorter list on the day it matters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path_prefix: Option<std::path::PathBuf>,
}

impl NonGoal {
    pub fn new(statement: impl Into<String>, source: IntentSource) -> Self {
        Self {
            statement: statement.into(),
            source,
            path_prefix: None,
        }
    }

    /// Name the part of the tree this non-goal rules out.
    pub fn within(mut self, prefix: impl Into<std::path::PathBuf>) -> Self {
        self.path_prefix = Some(prefix.into());
        self
    }
}

/// Something the agent decided to believe in the absence of an answer.
///
/// Assumptions are the honest name for the gaps a request always has. Writing
/// them down is what makes a later correction cheap: when the user says "no,
/// not that", the assumption that produced "that" can be named and retired
/// instead of the whole plan being thrown away.
#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct Assumption {
    pub id: AssumptionId,
    pub statement: String,
    /// The contract revision that contradicted this assumption, once one has.
    #[serde(default)]
    pub invalidated_by: Option<u64>,
}

impl Assumption {
    pub fn holds(&self) -> bool {
        self.invalidated_by.is_none()
    }
}

/// A question the agent could not answer from the request or the repository.
#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct OpenQuestion {
    pub id: QuestionId,
    pub question: String,
    /// Whether proceeding without an answer would risk doing the wrong work.
    ///
    /// Most questions are not blocking: a competent colleague picks the obvious
    /// reading and says which one they picked. Reserving `true` for the cases
    /// where any choice might be wrong is what keeps the agent from
    /// interrogating the user about things it could have decided itself.
    pub blocking: bool,
    #[serde(default)]
    pub answer: Option<String>,
}

super::expectation_id!(AssumptionId);
super::expectation_id!(QuestionId);

// ---------------------------------------------------------------------------
// Tally
// ---------------------------------------------------------------------------

/// How much of the contract is settled, as counts.
///
/// Deliberately integers. "Alignment: 94%" is a number nobody can act on and
/// nothing can falsify — it cannot be wrong, so it cannot be informative.
/// "6 of 7 requirements verified" names the gap, and the missing one has an
/// id, a statement and a reason. There is intentionally no percentage
/// accessor on this type; adding one would put the vague number back.
#[derive(Clone, Copy, Debug, Default, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct RequirementTally {
    pub verified: usize,
    pub waived: usize,
    pub violated: usize,
    pub unknown: usize,
    pub unverified: usize,
}

impl RequirementTally {
    pub fn total(&self) -> usize {
        self.verified + self.waived + self.violated + self.unknown + self.unverified
    }

    /// Requirements that have cleared delivery.
    pub fn settled(&self) -> usize {
        self.verified + self.waived
    }
}

impl fmt::Display for RequirementTally {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} / {} requirements verified",
            self.settled(),
            self.total()
        )
    }
}

// ---------------------------------------------------------------------------
// Contract
// ---------------------------------------------------------------------------

/// The durable statement of what this task is for.
#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct ExpectationContract {
    pub schema_version: u32,
    /// Starts at one and increases on every user correction.
    pub revision: u64,
    /// The task in one line, as the user would describe it.
    pub objective: String,
    #[serde(default)]
    pub clauses: Vec<ExpectationClause>,
    #[serde(default)]
    pub non_goals: Vec<NonGoal>,
    #[serde(default)]
    pub assumptions: Vec<Assumption>,
    #[serde(default)]
    pub open_questions: Vec<OpenQuestion>,
}

impl ExpectationContract {
    /// A contract with nothing in it yet, at revision one.
    pub fn new(objective: impl Into<String>) -> Self {
        Self {
            schema_version: EXPECTATION_SCHEMA_VERSION,
            revision: 1,
            objective: objective.into(),
            clauses: Vec::new(),
            non_goals: Vec::new(),
            assumptions: Vec::new(),
            open_questions: Vec::new(),
        }
    }

    /// Structural invariants that must hold before a contract is stored.
    ///
    /// These are the guarantees that cannot be left to prompting, because the
    /// component being constrained is the one writing the prose.
    pub fn validate(&self) -> Result<(), ExpectationError> {
        if self.revision == 0 {
            return Err(ExpectationError::Invalid(
                "contract revision starts at one".into(),
            ));
        }
        require_text("objective", &self.objective)?;

        let mut clause_ids = BTreeSet::new();
        let mut criterion_ids = BTreeSet::new();
        for clause in &self.clauses {
            if !clause_ids.insert(clause.id) {
                return Err(ExpectationError::DuplicateClause(clause.id));
            }
            require_text("requirement statement", &clause.statement)?;
            require_text("requirement source quotation", &clause.source.quotation)?;

            // A hard requirement with no acceptance criterion cannot be checked,
            // and a requirement that cannot be checked will be declared done by
            // whoever is in a hurry.
            if clause.is_required() && clause.acceptance_criteria.is_empty() {
                return Err(ExpectationError::UncheckableRequirement(clause.id));
            }
            for criterion in &clause.acceptance_criteria {
                if !criterion_ids.insert(criterion.id) {
                    return Err(ExpectationError::DuplicateCriterion(criterion.id));
                }
                require_text("acceptance criterion", &criterion.statement)?;
            }

            match &clause.status {
                // Verification without evidence is assertion. The whole release
                // rests on the difference.
                RequirementStatus::Verified { evidence } if evidence.is_empty() => {
                    return Err(ExpectationError::UnevidencedVerification(clause.id));
                }
                // A waiver is a human decision. Letting it carry no reason lets
                // the agent excuse itself from work and leaves no trace saying
                // who agreed.
                RequirementStatus::Waived { reason, .. } if reason.trim().is_empty() => {
                    return Err(ExpectationError::UnreasonedWaiver(clause.id));
                }
                _ => {}
            }
        }

        for non_goal in &self.non_goals {
            require_text("non-goal", &non_goal.statement)?;
        }
        for assumption in &self.assumptions {
            require_text("assumption", &assumption.statement)?;
        }
        for question in &self.open_questions {
            require_text("open question", &question.question)?;
        }
        Ok(())
    }

    pub fn clause(&self, id: RequirementId) -> Option<&ExpectationClause> {
        self.clauses.iter().find(|clause| clause.id == id)
    }

    pub fn clause_mut(&mut self, id: RequirementId) -> Option<&mut ExpectationClause> {
        self.clauses.iter_mut().find(|clause| clause.id == id)
    }

    pub fn required(&self) -> impl Iterator<Item = &ExpectationClause> {
        self.clauses.iter().filter(|clause| clause.is_required())
    }

    pub fn preferred(&self) -> impl Iterator<Item = &ExpectationClause> {
        self.clauses.iter().filter(|clause| !clause.is_required())
    }

    /// Counts over the **hard** requirements only.
    ///
    /// Preferences are reported separately rather than mixed in, so that
    /// "6 / 7" always means six of seven things that actually block delivery.
    /// Averaging preferences into the same number is how a run with an unmet
    /// hard requirement comes to look mostly finished.
    pub fn tally(&self) -> RequirementTally {
        let mut tally = RequirementTally::default();
        for clause in self.required() {
            match clause.status {
                RequirementStatus::Verified { .. } => tally.verified += 1,
                RequirementStatus::Waived { .. } => tally.waived += 1,
                RequirementStatus::Violated { .. } => tally.violated += 1,
                RequirementStatus::Unknown { .. } => tally.unknown += 1,
                RequirementStatus::Unverified => tally.unverified += 1,
            }
        }
        tally
    }

    /// Hard requirements standing between this task and delivery.
    pub fn blocking_gaps(&self) -> Vec<&ExpectationClause> {
        self.clauses
            .iter()
            .filter(|clause| clause.blocks_delivery())
            .collect()
    }

    /// The non-goals a machine can check, as `(prefix, statement)` pairs.
    ///
    /// Derived from the contract rather than supplied to the gate, so the scope
    /// check reads the same bounds on the last turn as on the first.
    pub fn forbidden_prefixes(&self) -> Vec<(std::path::PathBuf, String)> {
        self.non_goals
            .iter()
            .filter_map(|non_goal| {
                non_goal
                    .path_prefix
                    .clone()
                    .map(|prefix| (prefix, non_goal.statement.clone()))
            })
            .collect()
    }

    /// Questions that must be answered before the work can be trusted.
    pub fn blocking_questions(&self) -> Vec<&OpenQuestion> {
        self.open_questions
            .iter()
            .filter(|question| question.blocking && question.answer.is_none())
            .collect()
    }

    /// The small block re-injected into every planning, coding and review
    /// prompt (v1.5 §4).
    ///
    /// This is the whole point of storing the contract: the agent is never
    /// asked to recall what the user wanted forty turns ago, it is told, every
    /// time, in a form short enough to include unconditionally. Verified
    /// clauses are summarised as a count rather than listed — the model needs
    /// the *outstanding* work in front of it, and spending the budget on
    /// finished items is what makes a brief too long to always send.
    pub fn brief(&self) -> String {
        let mut out = String::from("ACTIVE TASK CONTRACT\n\nGoal:\n");
        out.push_str(&self.objective);
        out.push_str("\n\nMust satisfy:\n");
        let mut index = 1;
        for clause in self.required() {
            if clause.status.clears_delivery() {
                continue;
            }
            out.push_str(&format!(
                "{index}. {} [{}]\n",
                clause.statement,
                clause.status.label()
            ));
            index += 1;
        }
        if index == 1 {
            out.push_str("(all hard requirements are satisfied)\n");
        }
        let tally = self.tally();
        out.push_str(&format!("\nProgress: {tally}\n"));

        if !self.non_goals.is_empty() {
            out.push_str("\nMust not:\n");
            for (position, non_goal) in self.non_goals.iter().enumerate() {
                out.push_str(&format!("{}. {}\n", position + 1, non_goal.statement));
            }
        }
        let preferences: Vec<_> = self.preferred().collect();
        if !preferences.is_empty() {
            out.push_str("\nPreferences (do not block delivery):\n");
            for preference in preferences {
                out.push_str(&format!("- {}\n", preference.statement));
            }
        }
        let unanswered: Vec<_> = self
            .open_questions
            .iter()
            .filter(|question| question.answer.is_none())
            .collect();
        if !unanswered.is_empty() {
            out.push_str("\nStill unresolved:\n");
            for question in unanswered {
                out.push_str(&format!(
                    "- {}{}\n",
                    question.question,
                    if question.blocking { " (blocking)" } else { "" }
                ));
            }
        }
        out
    }
}

pub(crate) fn require_text(field: &str, value: &str) -> Result<(), ExpectationError> {
    if value.trim().is_empty() {
        return Err(ExpectationError::Invalid(format!(
            "{field} must not be empty"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::work::CriterionId;

    fn source() -> IntentSource {
        IntentSource {
            message_index: 0,
            quotation: "make settings simpler, MCP must actually work".into(),
        }
    }

    fn criterion(statement: &str) -> AcceptanceCriterion {
        AcceptanceCriterion {
            id: CriterionId::new(),
            statement: statement.into(),
        }
    }

    fn hard(statement: &str) -> ExpectationClause {
        ExpectationClause {
            id: RequirementId::new(),
            statement: statement.into(),
            strength: ExpectationStrength::Required,
            acceptance_criteria: vec![criterion("a user can do it")],
            status: RequirementStatus::Unverified,
            source: source(),
        }
    }

    fn soft(statement: &str) -> ExpectationClause {
        ExpectationClause {
            id: RequirementId::new(),
            statement: statement.into(),
            strength: ExpectationStrength::Preferred,
            acceptance_criteria: Vec::new(),
            status: RequirementStatus::Unverified,
            source: source(),
        }
    }

    fn contract(clauses: Vec<ExpectationClause>) -> ExpectationContract {
        let mut contract = ExpectationContract::new("Improve the Settings experience");
        contract.clauses = clauses;
        contract
    }

    #[test]
    fn a_hard_requirement_nothing_can_check_is_rejected() {
        // The quiet way a task gets declared done: a requirement phrased so
        // that no evidence could ever contradict it.
        let mut clause = hard("settings should be good");
        clause.acceptance_criteria.clear();
        let error = contract(vec![clause]).validate().unwrap_err();
        assert!(
            matches!(error, ExpectationError::UncheckableRequirement(_)),
            "got {error:?}"
        );
    }

    #[test]
    fn a_preference_needs_no_acceptance_criterion() {
        // Preferences do not block delivery, so demanding a test for "less
        // cluttered" would only push the agent to invent one.
        contract(vec![soft("reduce visual clutter")])
            .validate()
            .expect("a preference without criteria is legal");
    }

    #[test]
    fn verification_without_evidence_is_not_verification() {
        let mut clause = hard("MCP configuration works");
        clause.status = RequirementStatus::Verified { evidence: vec![] };
        let error = contract(vec![clause]).validate().unwrap_err();
        assert!(
            matches!(error, ExpectationError::UnevidencedVerification(_)),
            "got {error:?}"
        );
    }

    #[test]
    fn a_waiver_must_say_why() {
        // Otherwise a waiver is just the agent excusing itself from work, with
        // nothing on the record saying who agreed to that.
        let mut clause = hard("model configuration works");
        clause.status = RequirementStatus::Waived {
            reason: "   ".into(),
            source: source(),
        };
        let error = contract(vec![clause]).validate().unwrap_err();
        assert!(
            matches!(error, ExpectationError::UnreasonedWaiver(_)),
            "got {error:?}"
        );
    }

    #[test]
    fn unknown_does_not_clear_delivery() {
        // The single most important line in this module: an agent that ships on
        // "I could not tell" is the false Done v1.5 exists to stop.
        assert!(
            !RequirementStatus::Unknown {
                detail: "could not reach the daemon".into()
            }
            .clears_delivery()
        );
        assert!(!RequirementStatus::Unverified.clears_delivery());
        assert!(
            !RequirementStatus::Violated {
                evidence: vec![],
                detail: "still cluttered".into()
            }
            .clears_delivery()
        );
        assert!(
            RequirementStatus::Verified {
                evidence: vec![EvidenceId::new()]
            }
            .clears_delivery()
        );
        assert!(
            RequirementStatus::Waived {
                reason: "user deferred this to the next task".into(),
                source: source()
            }
            .clears_delivery()
        );
    }

    #[test]
    fn preferences_do_not_dilute_the_hard_requirement_count() {
        // Three hard requirements, one met, and a pile of satisfied
        // preferences. Mixing them would report "4 / 6" and read as nearly
        // finished when two thirds of the actual task is untouched.
        let mut met = hard("MCP configuration works");
        met.status = RequirementStatus::Verified {
            evidence: vec![EvidenceId::new()],
        };
        let contract = contract(vec![
            met,
            hard("model configuration works"),
            hard("settings persist"),
            soft("reduce visual clutter"),
            soft("keep common controls reachable"),
            soft("sensible defaults"),
        ]);
        contract.validate().unwrap();

        let tally = contract.tally();
        assert_eq!(tally.total(), 3, "preferences are not hard requirements");
        assert_eq!(tally.settled(), 1);
        assert_eq!(tally.to_string(), "1 / 3 requirements verified");
        assert_eq!(contract.blocking_gaps().len(), 2);
    }

    #[test]
    fn a_waived_requirement_stops_blocking_but_is_still_counted() {
        let mut waived = hard("existing settings migrate");
        waived.status = RequirementStatus::Waived {
            reason: "the user said to handle migration separately".into(),
            source: source(),
        };
        let contract = contract(vec![waived, hard("MCP configuration works")]);
        contract.validate().unwrap();

        let tally = contract.tally();
        assert_eq!(tally.waived, 1);
        assert_eq!(tally.total(), 2, "a waiver does not vanish from the record");
        assert_eq!(
            contract.blocking_gaps().len(),
            1,
            "only the genuinely outstanding requirement blocks"
        );
    }

    #[test]
    fn the_brief_carries_what_is_outstanding_and_drops_what_is_done() {
        let mut done = hard("MCP configuration works");
        done.status = RequirementStatus::Verified {
            evidence: vec![EvidenceId::new()],
        };
        let mut contract = contract(vec![done, hard("model configuration works")]);
        contract
            .non_goals
            .push(NonGoal::new("redesign the editor", source()).within("src/editor"));
        contract.open_questions.push(OpenQuestion {
            id: QuestionId::new(),
            question: "which provider should be the default?".into(),
            blocking: false,
            answer: None,
        });
        contract.clauses.push(soft("reduce visual clutter"));
        contract.validate().unwrap();

        let brief = contract.brief();
        assert!(brief.contains("model configuration works"));
        assert!(
            !brief.contains("1. MCP configuration works"),
            "a satisfied requirement should not spend room in every prompt:\n{brief}"
        );
        assert!(brief.contains("1 / 2 requirements verified"));
        assert!(brief.contains("Must not:"));
        assert!(brief.contains("redesign the editor"));
        assert!(brief.contains("Preferences (do not block delivery):"));
        assert!(brief.contains("Still unresolved:"));
    }

    #[test]
    fn the_brief_says_so_when_nothing_is_outstanding() {
        let mut done = hard("MCP configuration works");
        done.status = RequirementStatus::Verified {
            evidence: vec![EvidenceId::new()],
        };
        let brief = contract(vec![done]).brief();
        assert!(
            brief.contains("(all hard requirements are satisfied)"),
            "{brief}"
        );
    }

    #[test]
    fn duplicate_ids_are_refused() {
        let clause = hard("MCP configuration works");
        let error = contract(vec![clause.clone(), clause])
            .validate()
            .unwrap_err();
        assert!(matches!(error, ExpectationError::DuplicateClause(_)));
    }

    #[test]
    fn a_contract_round_trips_through_json() {
        // The contract is durable state; a shape that cannot be reloaded is a
        // contract that silently reverts to nothing after a restart.
        let mut clause = hard("MCP configuration works");
        clause.status = RequirementStatus::Verified {
            evidence: vec![EvidenceId::new()],
        };
        let original = contract(vec![clause, soft("reduce visual clutter")]);
        original.validate().unwrap();
        let encoded = serde_json::to_string(&original).unwrap();
        let decoded: ExpectationContract = serde_json::from_str(&encoded).unwrap();
        assert_eq!(original, decoded);
    }
}
