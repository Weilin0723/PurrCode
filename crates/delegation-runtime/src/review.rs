//! Review workers and the bounded repair loop (v1.4 §PR8, §PR9).
//!
//! A reviewer reports; it does not rewrite. The main agent decides which
//! findings to act on, and when it decides to act, this module turns a
//! validation failure or a blocking finding into a *bounded* repair — at most
//! one cycle, or two when the failure is a compile/test error whose output names
//! what broke. After that the answer is `NeedsAttention` and a human is asked.
//!
//! The bound is the point. Two agents passing a failing test back and forth is
//! not collaboration, it is a loop with a token meter attached.

use crate::context::CarriedFinding;
use chrono::Utc;
use purrcode_runtime_core::ValidationStatus;
use purrcode_runtime_core::delegation::{
    DelegationId, DelegationOrigin, FindingSeverity, RepairDecision, ReviewResult,
    StructuredFinding, ValidationEvidence, WorkerId, WorkerResult, repair_decision,
};

/// Build a [`ReviewResult`] from a reviewer's worker result.
///
/// `pass` is *computed* from the findings, never taken from the model's claim: a
/// reviewer that reports two critical findings and declares success is
/// contradicting itself, and the contradiction should not reach the parent as a
/// pass.
pub fn review_result_from(result: &WorkerResult) -> ReviewResult {
    let review = ReviewResult {
        delegation_id: result.delegation_id,
        worker_id: result.worker_id,
        findings: result.findings.clone(),
        recommended_actions: result
            .findings
            .iter()
            .filter_map(|finding| finding.recommended_action.clone())
            .collect(),
        pass: false,
        reviewed_at: result.completed_at,
    };
    ReviewResult {
        pass: review.computed_pass(),
        ..review
    }
}

/// Why a repair is being attempted.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RepairTrigger {
    /// A validation the parent ran against the integrated state failed.
    ValidationFailed { name: String, detail: String },
    /// A reviewer raised a finding at or above `High`.
    BlockingFinding { finding: Box<StructuredFinding> },
}

impl RepairTrigger {
    /// True for a failure whose output names what broke — a compile error, a
    /// named failing test. Only these earn a second repair cycle.
    pub fn is_bounded(&self) -> bool {
        match self {
            RepairTrigger::ValidationFailed { detail, .. } => {
                let detail = detail.to_ascii_lowercase();
                detail.contains("error[")
                    || detail.contains("test result: failed")
                    || detail.contains("assertion")
                    || detail.contains("panicked at")
                    || detail.contains("failures:")
            }
            // A reviewer finding is a judgement, not a mechanical failure. One
            // attempt, then a human decides.
            RepairTrigger::BlockingFinding { .. } => false,
        }
    }

    pub fn summary(&self) -> String {
        match self {
            RepairTrigger::ValidationFailed { name, detail } => {
                let detail = detail.lines().take(20).collect::<Vec<_>>().join("\n");
                format!("`{name}` failed:\n{detail}")
            }
            RepairTrigger::BlockingFinding { finding } => {
                format!("{:?}: {}", finding.severity, finding.title)
            }
        }
    }
}

/// What to do about a failure attributed to one worker.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RepairPlan {
    /// Send the failure back to the responsible worker with the evidence.
    Retry {
        cycle: u8,
        delegation_id: DelegationId,
        worker_id: WorkerId,
        findings: Vec<CarriedFinding>,
    },
    /// The bound is reached. Control returns to the main agent and the user.
    NeedsAttention { reason: String },
}

/// Decide whether to route a failure back to its worker (v1.4 §PR9).
pub fn plan_repair(
    delegation_id: DelegationId,
    worker_id: WorkerId,
    cycles_used: u8,
    trigger: &RepairTrigger,
) -> RepairPlan {
    match repair_decision(cycles_used, trigger.is_bounded()) {
        RepairDecision::Retry { cycle } => RepairPlan::Retry {
            cycle,
            delegation_id,
            worker_id,
            findings: vec![carried_finding(delegation_id, worker_id, trigger)],
        },
        RepairDecision::NeedsAttention => RepairPlan::NeedsAttention {
            reason: format!(
                "{} repair cycle(s) did not resolve: {}",
                cycles_used,
                trigger.summary()
            ),
        },
    }
}

/// Turn a trigger into a finding the worker can act on, carrying the provenance
/// that says where it came from.
fn carried_finding(
    delegation_id: DelegationId,
    worker_id: WorkerId,
    trigger: &RepairTrigger,
) -> CarriedFinding {
    match trigger {
        RepairTrigger::ValidationFailed { name, detail } => CarriedFinding {
            finding: StructuredFinding {
                id: format!("validation:{name}"),
                title: format!("`{name}` failed after your change was integrated"),
                detail: detail.clone(),
                severity: FindingSeverity::High,
                path: None,
                line: None,
                evidence_ids: Vec::new(),
                recommended_action: Some(
                    "fix the failure inside your delegated scope; do not widen the scope".into(),
                ),
            },
            origin: DelegationOrigin::IntegrationOutcome {
                delegation_id,
                applied: true,
            },
        },
        RepairTrigger::BlockingFinding { finding } => CarriedFinding {
            finding: (**finding).clone(),
            origin: DelegationOrigin::WorkerFinding {
                delegation_id,
                worker_id,
                finding_id: finding.id.clone(),
                evidence_ids: finding.evidence_ids.clone(),
            },
        },
    }
}

/// Which worker is responsible for a failing validation.
///
/// Attribution is by changed path: the worker whose patch touched a file the
/// failure names. When nothing matches — a failure in a file nobody touched, or
/// a cross-cutting break — the answer is `None`, and the main agent handles it
/// rather than a worker being blamed for something it did not do.
pub fn attribute_failure<'a>(
    detail: &str,
    candidates: impl IntoIterator<Item = &'a WorkerResult>,
) -> Option<&'a WorkerResult> {
    let detail = detail.replace('\\', "/");
    candidates.into_iter().find(|result| {
        result.changed_paths.iter().any(|path| {
            path.to_str()
                .is_some_and(|path| !path.is_empty() && detail.contains(path))
        })
    })
}

/// A validation the parent ran, as evidence attached to a worker result.
pub fn validation_evidence(
    name: &str,
    status: ValidationStatus,
    detail: &str,
) -> ValidationEvidence {
    ValidationEvidence {
        name: name.to_owned(),
        status,
        detail: detail.to_owned(),
        evidence_id: None,
    }
}

/// A worker result for a repair attempt that produced nothing, so the parent
/// still records a structured handoff rather than silence.
pub fn empty_repair_result(
    delegation_id: DelegationId,
    worker_id: WorkerId,
    reason: &str,
) -> WorkerResult {
    WorkerResult {
        delegation_id,
        worker_id,
        status: purrcode_runtime_core::delegation::WorkerResultStatus::NeedsAttention {
            reason: reason.to_owned(),
        },
        summary: reason.to_owned(),
        changed_paths: Vec::new(),
        patch_digest: None,
        findings: Vec::new(),
        validations: Vec::new(),
        unresolved: vec![purrcode_runtime_core::delegation::OpenIssue {
            summary: reason.to_owned(),
            detail: String::new(),
        }],
        evidence_ids: Vec::new(),
        usage: Default::default(),
        completed_at: Utc::now(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{admitted_delegation, worker_result};
    use purrcode_runtime_core::delegation::ExpectedOutput;
    use purrcode_runtime_core::work::EvidenceId;
    use std::path::PathBuf;

    fn finding(severity: FindingSeverity) -> StructuredFinding {
        StructuredFinding {
            id: "f1".into(),
            title: "token logged in plaintext".into(),
            detail: "redact before logging".into(),
            severity,
            path: Some(PathBuf::from("src/auth/token.rs")),
            line: Some(42),
            evidence_ids: vec![EvidenceId::new()],
            recommended_action: Some("redact the token".into()),
        }
    }

    #[test]
    fn a_reviewer_claiming_success_with_a_critical_finding_does_not_pass() {
        let delegation = admitted_delegation(&["src/**"], ExpectedOutput::Review, &[]);
        let mut result = worker_result(&delegation, WorkerId::new(), &[]);
        result.findings = vec![finding(FindingSeverity::Critical)];
        let review = review_result_from(&result);
        assert!(!review.pass);
        assert_eq!(review.blocking_findings().count(), 1);
        assert!(review.findings_have_provenance());
        assert_eq!(review.recommended_actions, ["redact the token"]);
    }

    #[test]
    fn a_clean_review_passes() {
        let delegation = admitted_delegation(&["src/**"], ExpectedOutput::Review, &[]);
        let mut result = worker_result(&delegation, WorkerId::new(), &[]);
        result.findings = vec![finding(FindingSeverity::Low)];
        assert!(review_result_from(&result).pass);
    }

    #[test]
    fn a_test_failure_gets_at_most_two_cycles() {
        let delegation = DelegationId::new();
        let worker = WorkerId::new();
        let trigger = RepairTrigger::ValidationFailed {
            name: "cargo test".into(),
            detail: "failures:\n    auth::token::exchange_refreshes".into(),
        };
        assert!(trigger.is_bounded());
        assert!(matches!(
            plan_repair(delegation, worker, 0, &trigger),
            RepairPlan::Retry { cycle: 1, .. }
        ));
        assert!(matches!(
            plan_repair(delegation, worker, 1, &trigger),
            RepairPlan::Retry { cycle: 2, .. }
        ));
        // …and then it stops. No infinite multi-agent ping-pong.
        assert!(matches!(
            plan_repair(delegation, worker, 2, &trigger),
            RepairPlan::NeedsAttention { .. }
        ));
    }

    #[test]
    fn a_reviewer_finding_gets_exactly_one_cycle() {
        let delegation = DelegationId::new();
        let worker = WorkerId::new();
        let trigger = RepairTrigger::BlockingFinding {
            finding: Box::new(finding(FindingSeverity::High)),
        };
        assert!(!trigger.is_bounded());
        match plan_repair(delegation, worker, 0, &trigger) {
            RepairPlan::Retry {
                cycle, findings, ..
            } => {
                assert_eq!(cycle, 1);
                // The finding reaches the worker with its provenance intact.
                assert!(findings[0].origin.why_included().contains("finding"));
            }
            other => panic!("expected a retry, got {other:?}"),
        }
        assert!(matches!(
            plan_repair(delegation, worker, 1, &trigger),
            RepairPlan::NeedsAttention { .. }
        ));
    }

    #[test]
    fn an_unbounded_failure_is_not_retried_twice() {
        let trigger = RepairTrigger::ValidationFailed {
            name: "deploy".into(),
            detail: "something went wrong".into(),
        };
        assert!(!trigger.is_bounded());
        assert!(matches!(
            plan_repair(DelegationId::new(), WorkerId::new(), 1, &trigger),
            RepairPlan::NeedsAttention { .. }
        ));
    }

    #[test]
    fn failures_are_attributed_by_changed_path() {
        let backend = admitted_delegation(&["src/auth/**"], ExpectedOutput::Patch, &[]);
        let migration = admitted_delegation(&["migrations/**"], ExpectedOutput::Patch, &[]);
        let backend_result = worker_result(&backend, WorkerId::new(), &["src/auth/token.rs"]);
        let migration_result = worker_result(&migration, WorkerId::new(), &["migrations/0007.sql"]);
        let candidates = [&backend_result, &migration_result];

        let attributed = attribute_failure(
            "error[E0425]: cannot find value `token` in src/auth/token.rs:42",
            candidates.iter().copied(),
        )
        .expect("the backend worker touched that file");
        assert_eq!(attributed.worker_id, backend_result.worker_id);

        // A failure in a file nobody touched is nobody's repair task.
        assert!(
            attribute_failure(
                "error: something broke in src/unrelated.rs",
                candidates.iter().copied()
            )
            .is_none()
        );
    }
}
