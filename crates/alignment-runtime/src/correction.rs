//! Turning findings into repair work (v1.5 §11, §28).
//!
//! `CorrectionLedger` in `runtime-core` decides *whether* another cycle may
//! run. This decides what that cycle is: which findings it addresses, what the
//! repair agent is told, and — the part that is easy to get wrong — that the
//! repair agent is not the reviewer.
//!
//! The separation is not tidiness. A reviewer that fixes what it finds produces
//! one act with no independent confirmation inside it, so the loop cannot tell
//! a real problem that was solved from an imagined problem that was
//! "solved" by rewording. Repair therefore goes to a different role
//! (`ModelRole::RepairWorker`), and whether it worked is decided by a
//! *re-review*, never by the repairer's own account.

use purrcode_runtime_core::correction::{CorrectionAllowance, CorrectionLedger};
use purrcode_runtime_core::expectation::ExpectationContract;
use purrcode_runtime_core::review::{FindingId, ReviewFinding};

/// Work handed to the repair agent for one cycle.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepairAssignment {
    pub cycle: u32,
    pub findings: Vec<FindingId>,
    /// What the repair agent is told, contract included.
    pub brief: String,
}

/// What the loop should do next.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CorrectionStep {
    Repair(Box<RepairAssignment>),
    /// Nothing blocking is outstanding.
    NothingToFix,
    /// The budget is gone. The task goes to the user with what was tried — it
    /// does not silently pass and does not silently fail.
    Exhausted {
        cycles_used: u32,
        allowed: u32,
        abandoned: Vec<FindingId>,
    },
}

/// Decide the next correction step.
pub fn next_step(
    ledger: &CorrectionLedger,
    outstanding: &[&ReviewFinding],
    contract: &ExpectationContract,
) -> CorrectionStep {
    match ledger.may_correct(outstanding) {
        CorrectionAllowance::NothingToFix => CorrectionStep::NothingToFix,
        CorrectionAllowance::Exhausted {
            cycles_used,
            allowed,
        } => CorrectionStep::Exhausted {
            cycles_used,
            allowed,
            abandoned: ledger.abandoned(outstanding),
        },
        CorrectionAllowance::Allowed { cycle } => {
            let blocking: Vec<&ReviewFinding> = outstanding
                .iter()
                .filter(|finding| finding.invites_automatic_repair())
                .copied()
                .collect();
            CorrectionStep::Repair(Box::new(RepairAssignment {
                cycle,
                findings: blocking.iter().map(|finding| finding.id).collect(),
                brief: brief(cycle, &blocking, contract),
            }))
        }
    }
}

/// The instruction a repair agent receives.
fn brief(cycle: u32, blocking: &[&ReviewFinding], contract: &ExpectationContract) -> String {
    let mut out = String::from(
        "A review found problems with work already done. Fix them. This is \
         correction, not a new task: change what the findings name and leave the \
         rest of the branch alone.\n\n",
    );
    out.push_str(&contract.brief());
    out.push_str(&format!("\n\nFINDINGS TO REPAIR (cycle {cycle})\n"));
    for (index, finding) in blocking.iter().enumerate() {
        out.push_str(&format!(
            "\n{}. [{}] {}\n   Evidence: {}\n   Recommended: {}\n",
            index + 1,
            finding.severity.label(),
            finding.description,
            finding.evidence.join("; "),
            finding.recommendation
        ));
        if !finding.affected_paths.is_empty() {
            out.push_str(&format!(
                "   Affects: {}\n",
                finding
                    .affected_paths
                    .iter()
                    .map(|path| path.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
    }
    out.push_str(
        "\nA recommendation is advice from somebody who could not see the whole \
         picture. If it is wrong, fix the problem a better way and say so — but \
         the problem itself was found by someone who read this branch without \
         your reasoning, so do not dismiss it because you remember why the code \
         is that way.\n\n\
         Whether this worked is decided by re-review, not by you. Saying it is \
         fixed does not close a finding.",
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use purrcode_runtime_core::expectation::{ExpectationClause, IntentSource};
    use purrcode_runtime_core::review::{FindingCategory, ReviewId, ReviewKind, Severity};
    use purrcode_runtime_core::work::{AcceptanceCriterion, CriterionId};

    fn contract() -> ExpectationContract {
        let mut contract = ExpectationContract::new("Simplify Settings without losing capability");
        contract.clauses.push(ExpectationClause::required(
            "Every setting that existed before is still reachable",
            vec![AcceptanceCriterion {
                id: CriterionId::new(),
                statement: "each previous setting is reachable from the window".into(),
            }],
            IntentSource::new(0, "don't remove functionality"),
        ));
        contract
    }

    fn finding(kind: ReviewKind, severity: Severity) -> ReviewFinding {
        ReviewFinding {
            id: FindingId::new(),
            review: ReviewId::new(),
            kind,
            severity,
            category: FindingCategory::UxMismatch,
            requirement_id: None,
            description: "the advanced section was deleted rather than collapsed".into(),
            evidence: vec!["settings.rs:221".into()],
            affected_paths: vec!["settings.rs".into()],
            recommendation: "put the controls behind a disclosure".into(),
        }
    }

    #[test]
    fn a_blocking_finding_becomes_a_repair_assignment_carrying_the_contract() {
        let blocker = finding(ReviewKind::UserAlignment, Severity::High);
        let step = next_step(&CorrectionLedger::default(), &[&blocker], &contract());
        let CorrectionStep::Repair(assignment) = step else {
            panic!("a blocking finding must produce repair work: {step:?}");
        };
        assert_eq!(assignment.cycle, 1);
        assert_eq!(assignment.findings, vec![blocker.id]);
        assert!(assignment.brief.contains("ACTIVE TASK CONTRACT"));
        assert!(assignment.brief.contains("settings.rs:221"));
        assert!(
            assignment.brief.contains("decided by re-review"),
            "the repair agent must not be able to close its own finding"
        );
    }

    #[test]
    fn advisory_findings_do_not_start_a_cycle() {
        // Spending model budget on Low opinions is how a task that was finished
        // an hour ago is still running.
        let low = finding(ReviewKind::IndependentCode, Severity::Low);
        assert_eq!(
            next_step(&CorrectionLedger::default(), &[&low], &contract()),
            CorrectionStep::NothingToFix
        );
    }

    #[test]
    fn only_the_blocking_findings_reach_the_repair_agent() {
        // The advisory ones are reported to the user, not fixed automatically —
        // and a repair brief listing them would invite the agent to spend the
        // cycle on them.
        let blocker = finding(ReviewKind::UserAlignment, Severity::High);
        let advisory = finding(ReviewKind::IndependentCode, Severity::Medium);
        let step = next_step(
            &CorrectionLedger::default(),
            &[&blocker, &advisory],
            &contract(),
        );
        let CorrectionStep::Repair(assignment) = step else {
            panic!("expected repair work");
        };
        assert_eq!(assignment.findings, vec![blocker.id]);
    }

    #[test]
    fn a_spent_budget_hands_the_task_back_with_what_was_tried() {
        let blocker = finding(ReviewKind::UserAlignment, Severity::High);
        let mut ledger = CorrectionLedger::default();
        for _ in 0..2 {
            ledger.record_cycle(vec![], vec![blocker.id]);
        }
        let step = next_step(&ledger, &[&blocker], &contract());
        assert_eq!(
            step,
            CorrectionStep::Exhausted {
                cycles_used: 2,
                allowed: 2,
                abandoned: vec![blocker.id],
            }
        );
    }

    #[test]
    fn deterministic_failures_get_the_third_attempt_that_usually_works() {
        let blocker = finding(ReviewKind::Deterministic, Severity::Critical);
        let mut ledger = CorrectionLedger::default();
        for _ in 0..2 {
            ledger.record_cycle(vec![], vec![blocker.id]);
        }
        let step = next_step(&ledger, &[&blocker], &contract());
        let CorrectionStep::Repair(assignment) = step else {
            panic!("a failing test converges; the third try is often the one that works");
        };
        assert_eq!(assignment.cycle, 3);
    }
}
