//! The bounded self-correction loop (v1.5 §11, §18).
//!
//! When a review finds something blocking, the agent should fix it rather than
//! stopping to tell the user it found four issues. But an unbounded
//! review → fix → review → fix loop is how a task that was nearly finished
//! spends an afternoon and a budget converging on nothing.
//!
//! So correction is *bounded*, and the bound is not the same for every kind of
//! finding. A failing test is a fact with an unambiguous target: attempts at it
//! converge, and a third try is often the one that works. A reviewer's reading
//! of "this still feels cluttered" is a judgement, and a third attempt at
//! satisfying a judgement mostly produces a differently-shaped disagreement.
//! One budget for both would either cut the tractable case short or let the
//! intractable one run.
//!
//! When the budget is gone the task does not silently pass and does not
//! silently fail. It goes to the user, with what was tried.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::review::{FindingId, ReviewFinding, ReviewKind};

/// Automatic correction cycles allowed for findings that involve judgement.
pub const MAXIMUM_CORRECTION_CYCLES: u32 = 2;

/// Automatic correction cycles allowed when every outstanding blocker is a
/// deterministic failure — a broken build, a failing test, a lint error.
pub const MAXIMUM_DETERMINISTIC_CORRECTION_CYCLES: u32 = 3;

/// Where a task is, in the runtime's terms.
///
/// The user does not see these names (§18). They see "Understanding",
/// "Working", "Checking", "Reviewing", "Improving", "Ready" — because
/// `Correcting` is honest about the machine and alarming about the work, and a
/// person watching their task should not have to learn a state machine to know
/// whether anything is wrong.
#[derive(Clone, Copy, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LifecyclePhase {
    /// Reading the request and the repository, before committing to anything.
    Understanding,
    Implementing,
    /// Running the deterministic checks.
    Verifying,
    Reviewing,
    /// Repairing what a review found.
    Correcting,
    /// The delivery gate cleared.
    Ready,
    /// The correction budget is gone, or something needs a human decision.
    NeedsAttention,
}

impl LifecyclePhase {
    /// What the user sees.
    pub fn user_facing_label(self) -> &'static str {
        match self {
            Self::Understanding => "Understanding",
            Self::Implementing => "Working",
            Self::Verifying => "Checking",
            Self::Reviewing => "Reviewing",
            Self::Correcting => "Improving",
            Self::Ready => "Ready",
            Self::NeedsAttention => "Needs you",
        }
    }

    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Ready | Self::NeedsAttention)
    }
}

/// Whether another automatic correction cycle may start.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CorrectionAllowance {
    /// Go ahead; this is the cycle number that will run.
    Allowed { cycle: u32 },
    /// There is nothing blocking left to fix.
    NothingToFix,
    /// The budget is spent. The task goes to the user rather than looping.
    Exhausted { cycles_used: u32, allowed: u32 },
}

impl CorrectionAllowance {
    pub fn permits_correction(self) -> bool {
        matches!(self, Self::Allowed { .. })
    }
}

/// What correction has cost and achieved so far.
#[derive(Clone, Debug, Default, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct CorrectionLedger {
    pub cycles_used: u32,
    /// Findings a correction cycle resolved, confirmed by re-review rather than
    /// by the repair agent saying so.
    #[serde(default)]
    pub repaired: Vec<FindingId>,
    /// Findings the most recent cycle left open.
    ///
    /// Replaced each cycle rather than accumulated, because a finding that was
    /// open after cycle one and fixed in cycle two is not still open. Only once
    /// [`Self::may_correct`] returns `Exhausted` does this mean "gave up on
    /// these" — see [`Self::abandoned`], which is the question callers actually
    /// want to ask and which this field cannot answer on its own.
    #[serde(default)]
    pub still_open: Vec<FindingId>,
}

impl CorrectionLedger {
    /// How many cycles this set of findings is worth.
    ///
    /// The generous allowance applies only when *every* outstanding blocker is
    /// deterministic. A single judgement-shaped finding in the set pulls the
    /// whole round down to the tighter budget, because the loop cannot converge
    /// faster than its slowest-converging member.
    pub fn allowance_for(outstanding: &[&ReviewFinding]) -> u32 {
        let all_deterministic = !outstanding.is_empty()
            && outstanding
                .iter()
                .all(|finding| finding.kind == ReviewKind::Deterministic);
        if all_deterministic {
            MAXIMUM_DETERMINISTIC_CORRECTION_CYCLES
        } else {
            MAXIMUM_CORRECTION_CYCLES
        }
    }

    /// Decide whether to run another correction cycle.
    pub fn may_correct(&self, outstanding: &[&ReviewFinding]) -> CorrectionAllowance {
        let blocking: Vec<&ReviewFinding> = outstanding
            .iter()
            .filter(|finding| finding.invites_automatic_repair())
            .copied()
            .collect();
        if blocking.is_empty() {
            return CorrectionAllowance::NothingToFix;
        }
        // The allowance looks at what actually enters the repair loop, not
        // every outstanding finding. A non-blocking judgement-kind finding
        // sitting alongside a purely mechanical failure — a lint note next to
        // a build error, say — must not pull a deterministic budget down to
        // the tighter one; it never reaches the repair agent, so it cannot be
        // part of what the loop is waiting to converge on.
        let allowed = Self::allowance_for(&blocking);
        if self.cycles_used >= allowed {
            return CorrectionAllowance::Exhausted {
                cycles_used: self.cycles_used,
                allowed,
            };
        }
        CorrectionAllowance::Allowed {
            cycle: self.cycles_used + 1,
        }
    }

    /// Record the outcome of a finished cycle.
    pub fn record_cycle(&mut self, repaired: Vec<FindingId>, still_open: Vec<FindingId>) {
        self.cycles_used += 1;
        self.repaired.extend(repaired);
        self.still_open = still_open;
    }

    /// Findings the loop gave up on, once it has actually given up.
    ///
    /// Empty while cycles remain: a finding that is open mid-loop has not been
    /// abandoned, it is being worked on, and telling the user otherwise would
    /// report a failure that has not happened yet.
    pub fn abandoned(&self, outstanding: &[&ReviewFinding]) -> Vec<FindingId> {
        match self.may_correct(outstanding) {
            CorrectionAllowance::Exhausted { .. } => self.still_open.clone(),
            _ => Vec::new(),
        }
    }

    /// The phase a task should move to once correction can go no further.
    pub fn exhausted_phase(&self) -> LifecyclePhase {
        LifecyclePhase::NeedsAttention
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::review::{FindingCategory, ReviewFinding, ReviewId, Severity};

    fn finding(kind: ReviewKind, severity: Severity) -> ReviewFinding {
        ReviewFinding {
            id: FindingId::new(),
            review: ReviewId::new(),
            kind,
            severity,
            category: FindingCategory::Correctness,
            requirement_id: None,
            description: "a test fails".into(),
            evidence: vec!["cargo test".into()],
            affected_paths: vec![],
            recommendation: "fix it".into(),
        }
    }

    #[test]
    fn judgement_shaped_findings_get_two_cycles() {
        let blocker = finding(ReviewKind::UserAlignment, Severity::High);
        let outstanding = vec![&blocker];
        let mut ledger = CorrectionLedger::default();

        assert_eq!(
            ledger.may_correct(&outstanding),
            CorrectionAllowance::Allowed { cycle: 1 }
        );
        ledger.record_cycle(vec![], vec![blocker.id]);
        assert_eq!(
            ledger.may_correct(&outstanding),
            CorrectionAllowance::Allowed { cycle: 2 }
        );
        ledger.record_cycle(vec![], vec![blocker.id]);
        assert_eq!(
            ledger.may_correct(&outstanding),
            CorrectionAllowance::Exhausted {
                cycles_used: 2,
                allowed: 2
            },
            "a third attempt at satisfying a judgement mostly restates the disagreement"
        );
        assert_eq!(ledger.exhausted_phase(), LifecyclePhase::NeedsAttention);
    }

    #[test]
    fn deterministic_failures_get_a_third_attempt() {
        // A failing test is a fact with an unambiguous target; attempts at it
        // converge, and the third is often the one that works.
        let blocker = finding(ReviewKind::Deterministic, Severity::Critical);
        let outstanding = vec![&blocker];
        let mut ledger = CorrectionLedger::default();
        for cycle in 1..=3 {
            assert_eq!(
                ledger.may_correct(&outstanding),
                CorrectionAllowance::Allowed { cycle }
            );
            ledger.record_cycle(vec![], vec![blocker.id]);
        }
        assert!(matches!(
            ledger.may_correct(&outstanding),
            CorrectionAllowance::Exhausted { .. }
        ));
    }

    #[test]
    fn one_judgement_finding_pulls_the_whole_round_to_the_tighter_budget() {
        // The loop cannot converge faster than its slowest-converging member.
        let deterministic = finding(ReviewKind::Deterministic, Severity::Critical);
        let judgement = finding(ReviewKind::UserAlignment, Severity::High);
        let outstanding = vec![&deterministic, &judgement];
        assert_eq!(CorrectionLedger::allowance_for(&outstanding), 2);
    }

    #[test]
    fn advisory_findings_never_start_a_correction_cycle() {
        // Spending model budget on Low opinions is how a task that was finished
        // an hour ago is still running.
        let low = finding(ReviewKind::IndependentCode, Severity::Low);
        let medium = finding(ReviewKind::IndependentCode, Severity::Medium);
        let outstanding = vec![&low, &medium];
        assert_eq!(
            CorrectionLedger::default().may_correct(&outstanding),
            CorrectionAllowance::NothingToFix
        );
    }

    #[test]
    fn a_repaired_finding_stops_being_outstanding() {
        let blocker = finding(ReviewKind::Deterministic, Severity::Critical);
        let mut ledger = CorrectionLedger::default();
        ledger.record_cycle(vec![blocker.id], vec![]);
        assert_eq!(ledger.repaired, vec![blocker.id]);
        assert!(ledger.still_open.is_empty());
        assert!(ledger.abandoned(&[]).is_empty());
        // Re-review found nothing blocking, so no further cycle is proposed.
        assert_eq!(ledger.may_correct(&[]), CorrectionAllowance::NothingToFix);
    }

    #[test]
    fn a_finding_open_mid_loop_has_not_been_abandoned() {
        // It is being worked on. Reporting it as given-up-on would tell the
        // user about a failure that has not happened.
        let blocker = finding(ReviewKind::UserAlignment, Severity::High);
        let outstanding = vec![&blocker];
        let mut ledger = CorrectionLedger::default();

        ledger.record_cycle(vec![], vec![blocker.id]);
        assert_eq!(ledger.still_open, vec![blocker.id]);
        assert!(
            ledger.abandoned(&outstanding).is_empty(),
            "one cycle remains, so nothing has been given up on yet"
        );

        ledger.record_cycle(vec![], vec![blocker.id]);
        assert_eq!(
            ledger.abandoned(&outstanding),
            vec![blocker.id],
            "the budget is gone, so now it has"
        );
    }

    #[test]
    fn the_user_never_sees_the_state_machine() {
        // "Correcting" is honest about the machine and alarming about the work.
        assert_eq!(LifecyclePhase::Correcting.user_facing_label(), "Improving");
        assert_eq!(LifecyclePhase::Implementing.user_facing_label(), "Working");
        assert_eq!(LifecyclePhase::Verifying.user_facing_label(), "Checking");
        assert!(LifecyclePhase::Ready.is_terminal());
        assert!(LifecyclePhase::NeedsAttention.is_terminal());
        assert!(!LifecyclePhase::Correcting.is_terminal());
    }
}
