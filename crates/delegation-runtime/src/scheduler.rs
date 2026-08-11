//! The bounded worker scheduler (v1.4 §PR4).
//!
//! This is deliberately **not** another orchestration framework. It is a pure
//! function from the durable projection — the delegation records replayed out of
//! the event log — to "what should happen next". That shape is what makes
//! restart recovery a non-event: after a crash the daemon rebuilds the
//! projection, calls [`next_step`] again, and gets an answer that already knows
//! which workers finished. There is no scheduler state to lose, so there is
//! nothing to reconcile incorrectly.
//!
//! Failure semantics follow the PRD exactly: if `B` depends on `A` and `A`
//! fails, `B` is *blocked*, not run against a dependency that never landed.

use purrcode_runtime_core::delegation::{
    DelegationGovernance, DelegationId, DelegationLedger, DelegationRecord, DelegationStatus,
};
use std::collections::BTreeMap;

/// What the scheduler wants the runtime to do next.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SchedulerStep {
    /// Mark these delegations ready — their dependencies all completed.
    Release { delegations: Vec<DelegationId> },
    /// Start these delegations now. Bounded by parallelism and by the ledger.
    Start { delegations: Vec<DelegationId> },
    /// Block this delegation: a dependency will never complete.
    Block {
        delegation: DelegationId,
        blocking_dependency: DelegationId,
    },
    /// Workers are running and nothing new may start yet.
    Wait { running: u32 },
    /// Governance refused to start more work, with the reason to show the user.
    Refused { reason: String },
    /// Nothing is live; the delegation tree is finished.
    Done,
}

/// Decide the next step from the durable projection.
///
/// Order matters and is not arbitrary: blocking is resolved before releasing,
/// and releasing before starting, so a single call never both blocks a
/// delegation and starts work that depends on it.
pub fn next_step(
    records: &BTreeMap<DelegationId, DelegationRecord>,
    governance: &DelegationGovernance,
    ledger: &DelegationLedger,
) -> SchedulerStep {
    let running: u32 = records
        .values()
        .filter(|record| record.is_running())
        .count() as u32;

    // 1. Anything whose dependency failed, was cancelled or was itself blocked
    //    must be blocked. A dependent that runs anyway would be working against
    //    a change that does not exist.
    for record in records.values() {
        if !matches!(
            record.status(),
            DelegationStatus::Planned | DelegationStatus::Ready
        ) {
            continue;
        }
        if let Some(blocking) = failed_dependency(record, records) {
            return SchedulerStep::Block {
                delegation: record.delegation.id(),
                blocking_dependency: blocking,
            };
        }
    }

    // 2. Planned delegations whose dependencies all completed become ready.
    let releasable: Vec<DelegationId> = records
        .values()
        .filter(|record| record.status() == DelegationStatus::Planned)
        .filter(|record| dependencies_satisfied(record, records))
        .map(|record| record.delegation.id())
        .collect();
    if !releasable.is_empty() {
        return SchedulerStep::Release {
            delegations: releasable,
        };
    }

    // 3. Ready delegations start, up to the parallelism bound and whatever the
    //    resource ledger still permits.
    let ready: Vec<DelegationId> = records
        .values()
        .filter(|record| record.status() == DelegationStatus::Ready)
        .map(|record| record.delegation.id())
        .collect();
    if !ready.is_empty() {
        if let Err(reason) = ledger.admits_another_worker(governance, running) {
            // A refusal while nothing is running is terminal for this tree; a
            // refusal while workers are in flight just means "not yet".
            return if running == 0 {
                SchedulerStep::Refused { reason }
            } else {
                SchedulerStep::Wait { running }
            };
        }
        let parallelism = governance.effective_parallelism(ready.len());
        let slots = parallelism.saturating_sub(running as usize);
        // Also respect the total-worker ceiling: never start more than the
        // session is allowed to have started overall.
        let remaining_total = governance
            .maximum_workers
            .saturating_sub(ledger.workers_started) as usize;
        let slots = slots.min(remaining_total);
        if slots == 0 {
            return SchedulerStep::Wait { running };
        }
        return SchedulerStep::Start {
            delegations: ready.into_iter().take(slots).collect(),
        };
    }

    if running > 0 {
        return SchedulerStep::Wait { running };
    }
    if records.values().any(|record| record.status().is_live()) {
        // Live but neither ready nor running: something is awaiting approval.
        return SchedulerStep::Wait { running };
    }
    SchedulerStep::Done
}

/// The first dependency that will never complete, if any.
fn failed_dependency(
    record: &DelegationRecord,
    records: &BTreeMap<DelegationId, DelegationRecord>,
) -> Option<DelegationId> {
    record
        .delegation
        .dependencies()
        .iter()
        .find(|dependency| {
            records.get(*dependency).is_some_and(|other| {
                matches!(
                    other.status(),
                    DelegationStatus::Failed
                        | DelegationStatus::Cancelled
                        | DelegationStatus::Blocked
                        | DelegationStatus::Superseded
                )
            })
        })
        .copied()
}

fn dependencies_satisfied(
    record: &DelegationRecord,
    records: &BTreeMap<DelegationId, DelegationRecord>,
) -> bool {
    record.delegation.dependencies().iter().all(|dependency| {
        records
            .get(dependency)
            .is_some_and(|other| other.status() == DelegationStatus::Completed)
    })
}

/// Whether a delegation may be *resumed* after a restart, rather than restarted.
///
/// v1.4 §PR10 in one predicate: a delegation with a recorded result is finished
/// and must never run again; one that was `Running` when the daemon died is
/// reconciled; anything else is scheduled normally.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RecoveryAction {
    /// Completed with a result — leave it exactly as it is.
    LeaveCompleted,
    /// Was running when the process died. Its worktree may hold a partial
    /// patch; reconcile process and worktree state, do not rerun.
    Reconcile,
    /// Awaiting a human decision. Stays awaiting.
    KeepAwaitingDecision,
    /// Not started yet; the scheduler may pick it up normally.
    Schedulable,
    /// Terminal without a result (failed, cancelled, blocked).
    LeaveTerminal,
}

/// Classify one delegation for restart recovery.
pub fn recovery_action(record: &DelegationRecord) -> RecoveryAction {
    if record.awaits_decision() {
        return RecoveryAction::KeepAwaitingDecision;
    }
    if record.is_finished() {
        return RecoveryAction::LeaveCompleted;
    }
    match record.status() {
        DelegationStatus::Running => RecoveryAction::Reconcile,
        DelegationStatus::Planned | DelegationStatus::Ready => RecoveryAction::Schedulable,
        DelegationStatus::AwaitingApproval => RecoveryAction::KeepAwaitingDecision,
        DelegationStatus::Completed
        | DelegationStatus::Failed
        | DelegationStatus::Cancelled
        | DelegationStatus::Blocked
        | DelegationStatus::Superseded => RecoveryAction::LeaveTerminal,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{admitted_delegation, record_with_result};
    use purrcode_runtime_core::delegation::{DelegationBudget, ExpectedOutput, UsageSummary};

    fn records(list: Vec<DelegationRecord>) -> BTreeMap<DelegationId, DelegationRecord> {
        list.into_iter()
            .map(|record| (record.delegation.id(), record))
            .collect()
    }

    #[test]
    fn independent_delegations_start_up_to_the_parallelism_bound() {
        let governance = DelegationGovernance::default();
        let ledger = DelegationLedger::default();
        let mut list = Vec::new();
        for _ in 0..4 {
            let mut record =
                DelegationRecord::new(admitted_delegation(&["src/**"], ExpectedOutput::Patch, &[]));
            record
                .delegation
                .transition_to(DelegationStatus::Ready)
                .unwrap();
            list.push(record);
        }
        let records = records(list);
        match next_step(&records, &governance, &ledger) {
            SchedulerStep::Start { delegations } => {
                assert_eq!(
                    delegations.len(),
                    governance.maximum_parallel_workers as usize,
                    "never more than the configured parallelism"
                );
            }
            other => panic!("expected Start, got {other:?}"),
        }
    }

    #[test]
    fn a_dependent_is_released_only_after_its_dependency_completes() {
        let governance = DelegationGovernance::default();
        let ledger = DelegationLedger::default();
        let first = admitted_delegation(&["src/**"], ExpectedOutput::Patch, &[]);
        let second = admitted_delegation(&["tests/**"], ExpectedOutput::Patch, &[first.id()]);

        let mut running_first = DelegationRecord::new(first.clone());
        running_first
            .delegation
            .transition_to(DelegationStatus::Ready)
            .unwrap();
        running_first
            .delegation
            .transition_to(DelegationStatus::Running)
            .unwrap();
        let map = records(vec![
            running_first.clone(),
            DelegationRecord::new(second.clone()),
        ]);
        // The dependent is not released while its dependency runs.
        assert_eq!(
            next_step(&map, &governance, &ledger),
            SchedulerStep::Wait { running: 1 }
        );

        let completed_first = record_with_result(first, &["src/a.rs"]);
        let map = records(vec![completed_first, DelegationRecord::new(second.clone())]);
        match next_step(&map, &governance, &ledger) {
            SchedulerStep::Release { delegations } => assert_eq!(delegations, vec![second.id()]),
            other => panic!("expected Release, got {other:?}"),
        }
    }

    #[test]
    fn a_failed_dependency_blocks_its_dependent_rather_than_running_it() {
        // v1.4 §PR4: "B does not run and pretend the dependency passed."
        let governance = DelegationGovernance::default();
        let ledger = DelegationLedger::default();
        let first = admitted_delegation(&["src/**"], ExpectedOutput::Patch, &[]);
        let second = admitted_delegation(&["tests/**"], ExpectedOutput::Patch, &[first.id()]);
        let mut failed = DelegationRecord::new(first.clone());
        failed
            .delegation
            .transition_to(DelegationStatus::Ready)
            .unwrap();
        failed
            .delegation
            .transition_to(DelegationStatus::Running)
            .unwrap();
        failed
            .delegation
            .transition_to(DelegationStatus::Failed)
            .unwrap();
        let map = records(vec![failed, DelegationRecord::new(second.clone())]);
        assert_eq!(
            next_step(&map, &governance, &ledger),
            SchedulerStep::Block {
                delegation: second.id(),
                blocking_dependency: first.id(),
            }
        );
    }

    #[test]
    fn an_exhausted_ledger_refuses_rather_than_silently_stalling() {
        let governance = DelegationGovernance::default();
        let mut ledger = DelegationLedger::default();
        ledger.record_usage(&UsageSummary {
            input_tokens: governance.maximum_total_worker_input_tokens,
            ..UsageSummary::default()
        });
        let mut record =
            DelegationRecord::new(admitted_delegation(&["src/**"], ExpectedOutput::Patch, &[]));
        record
            .delegation
            .transition_to(DelegationStatus::Ready)
            .unwrap();
        match next_step(&records(vec![record]), &governance, &ledger) {
            SchedulerStep::Refused { reason } => assert!(reason.contains("input-token")),
            other => panic!("expected Refused, got {other:?}"),
        }
    }

    #[test]
    fn the_total_worker_ceiling_bounds_starts_even_with_free_parallelism() {
        let governance = DelegationGovernance {
            maximum_workers: 2,
            ..DelegationGovernance::default()
        };
        let ledger = DelegationLedger {
            workers_started: 2,
            ..DelegationLedger::default()
        };
        let mut record =
            DelegationRecord::new(admitted_delegation(&["src/**"], ExpectedOutput::Patch, &[]));
        record
            .delegation
            .transition_to(DelegationStatus::Ready)
            .unwrap();
        match next_step(&records(vec![record]), &governance, &ledger) {
            SchedulerStep::Refused { reason } => assert!(reason.contains("already started")),
            other => panic!("expected Refused, got {other:?}"),
        }
    }

    #[test]
    fn an_empty_tree_is_done() {
        assert_eq!(
            next_step(
                &BTreeMap::new(),
                &DelegationGovernance::default(),
                &DelegationLedger::default()
            ),
            SchedulerStep::Done
        );
    }

    #[test]
    fn recovery_never_reruns_a_completed_worker() {
        let completed = record_with_result(
            admitted_delegation(&["src/**"], ExpectedOutput::Patch, &[]),
            &["src/a.rs"],
        );
        assert_eq!(recovery_action(&completed), RecoveryAction::LeaveCompleted);

        let mut running =
            DelegationRecord::new(admitted_delegation(&["src/**"], ExpectedOutput::Patch, &[]));
        running
            .delegation
            .transition_to(DelegationStatus::Ready)
            .unwrap();
        running
            .delegation
            .transition_to(DelegationStatus::Running)
            .unwrap();
        assert_eq!(recovery_action(&running), RecoveryAction::Reconcile);

        let planned =
            DelegationRecord::new(admitted_delegation(&["src/**"], ExpectedOutput::Patch, &[]));
        assert_eq!(recovery_action(&planned), RecoveryAction::Schedulable);
    }

    #[test]
    fn the_scheduler_is_a_pure_function_of_the_projection() {
        // Restart safety in one assertion: the same projection always yields the
        // same step, so a daemon that died mid-run resumes identically.
        let governance = DelegationGovernance::default();
        let ledger = DelegationLedger {
            workers_started: 1,
            ..DelegationLedger::default()
        };
        let first = admitted_delegation(&["src/**"], ExpectedOutput::Patch, &[]);
        let second = admitted_delegation(&["tests/**"], ExpectedOutput::Patch, &[first.id()]);
        let map = records(vec![
            record_with_result(first, &["src/a.rs"]),
            DelegationRecord::new(second),
        ]);
        let once = next_step(&map, &governance, &ledger);
        let twice = next_step(&map, &governance, &ledger);
        assert_eq!(once, twice);
    }

    #[test]
    fn a_budget_left_with_nothing_to_spend_is_exhausted() {
        let empty = DelegationBudget {
            maximum_input_tokens: 0,
            ..DelegationBudget::modest()
        };
        assert!(empty.is_exhausted());
    }
}
