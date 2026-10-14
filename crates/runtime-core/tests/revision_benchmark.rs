//! Deterministic mechanism benchmark for revision-bound completion evidence.
//!
//! This is mechanism regression evidence, not a live-agent or A/B/C effect
//! experiment. Each case supplies durable events directly to the real
//! `SessionState::reduce_event` reducer and reads the reducer's delivery gate.
//! The expected decision is a hand-written scenario oracle in this file; it is
//! never obtained from the gate under test.
//!
//! JSON output is opt-in. Set `PURRCODE_REVISION_BENCHMARK_JSON` to an output
//! path to write a report. Existing files are never replaced.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;

use purrcode_runtime_core::expectation::{
    AlignmentEvidence, AlignmentEvidenceKind, ContractChange, ContractRevision, DeliveryState,
    ExpectationClause, ExpectationContract, IntentSource, NonGoal, RequirementStatus,
};
use purrcode_runtime_core::{
    ActionId, JudgmentDecision, ProposedAction, SessionEvent, SessionId, SessionState,
    WriteFileAction,
};
use serde::Serialize;

const BENCHMARK_KIND: &str = "deterministic-mechanism";
const SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum SupportStatus {
    Supported,
    Unsupported,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum CaseOutcome {
    Pass,
    ObservedFailure,
    Unsupported,
}

#[derive(Clone, Debug, Serialize)]
struct CaseResult {
    case_id: &'static str,
    expected: DeliveryState,
    observed: DeliveryState,
    support: SupportStatus,
    outcome: CaseOutcome,
    pass: bool,
    note: &'static str,
}

#[derive(Debug, Serialize)]
struct BenchmarkReport {
    schema_version: u32,
    benchmark_kind: &'static str,
    live: bool,
    oracle: &'static str,
    cases: Vec<CaseResult>,
}

fn source(quotation: &str) -> IntentSource {
    IntentSource::new(0, quotation)
}

fn contract_with(statement: &str) -> (SessionState, purrcode_runtime_core::RequirementId) {
    let mut contract = ExpectationContract::new(statement);
    let clause = ExpectationClause::required(
        statement,
        vec![purrcode_runtime_core::AcceptanceCriterion {
            id: purrcode_runtime_core::CriterionId::new(),
            statement: "an independent deterministic check establishes the requested behavior"
                .into(),
        }],
        source(statement),
    );
    let requirement_id = clause.id;
    contract.clauses.push(clause);

    let mut state = SessionState::empty(SessionId::new());
    state
        .reduce_event(&SessionEvent::ExpectationContractCreated {
            contract: Box::new(contract),
        })
        .expect("the benchmark contract is valid");
    (state, requirement_id)
}

fn record_evidence_and_verify(
    state: &mut SessionState,
    requirement_id: purrcode_runtime_core::RequirementId,
    detail: &str,
) -> purrcode_runtime_core::EvidenceId {
    let evidence = AlignmentEvidence::new_bound(
        AlignmentEvidenceKind::Validation,
        requirement_id,
        *state
            .requirement_revisions
            .get(&requirement_id)
            .expect("the benchmark requirement has a revision"),
        "benchmark-artifact-v1",
        "deterministic mechanism oracle",
        detail,
    );
    let evidence_id = evidence.id;
    state
        .reduce_event(&SessionEvent::AlignmentEvidenceRecorded {
            evidence: Box::new(evidence),
        })
        .expect("evidence must be recorded before it can be cited");
    state
        .reduce_event(&SessionEvent::RequirementStatusChanged {
            requirement_id,
            status: RequirementStatus::Verified {
                evidence: vec![evidence_id],
            },
            source: "deterministic mechanism oracle".into(),
        })
        .expect("recorded evidence should verify its requirement");
    evidence_id
}

fn revise_statement(
    state: &mut SessionState,
    requirement_id: purrcode_runtime_core::RequirementId,
    from: &str,
    to: &str,
) {
    state
        .reduce_event(&SessionEvent::ExpectationContractRevised {
            revision: Box::new(ContractRevision {
                revision: 2,
                source: source("the user revised the requirement"),
                reason: "apply the user's latest requirement wording".into(),
                changes: vec![ContractChange::ClauseRestated {
                    id: requirement_id,
                    from: from.into(),
                    to: to.into(),
                }],
            }),
        })
        .expect("the revision follows the contract revision");
}

fn gate_state(state: &mut SessionState) -> DeliveryState {
    gate_state_for(state, "benchmark-artifact-v1")
}

fn gate_state_for(state: &mut SessionState, artifact_digest: &str) -> DeliveryState {
    state
        .reduce_event(&SessionEvent::DeliveryGateEvaluated {
            validations: Vec::new(),
            artifact_digest: Some(artifact_digest.into()),
        })
        .expect("the delivery gate event is valid");
    state
        .delivery
        .as_ref()
        .expect("the reducer stores its computed delivery assessment")
        .state()
}

fn result(
    case_id: &'static str,
    expected: DeliveryState,
    observed: DeliveryState,
    support: SupportStatus,
    note: &'static str,
) -> CaseResult {
    let outcome = if support == SupportStatus::Unsupported {
        CaseOutcome::Unsupported
    } else if expected == observed {
        CaseOutcome::Pass
    } else {
        CaseOutcome::ObservedFailure
    };
    CaseResult {
        case_id,
        expected,
        observed,
        support,
        outcome,
        pass: outcome == CaseOutcome::Pass,
        note,
    }
}

fn fresh_evidence_after_revision() -> CaseResult {
    let (mut state, requirement_id) = contract_with("retry at most four attempts");
    record_evidence_and_verify(&mut state, requirement_id, "revision 1 behavior passed");
    revise_statement(
        &mut state,
        requirement_id,
        "retry at most four attempts",
        "retry at most four attempts; attempt four must be rejected",
    );
    record_evidence_and_verify(
        &mut state,
        requirement_id,
        "revision 2 behavior passed on the current artifact",
    );
    result(
        "correct-fresh-evidence",
        DeliveryState::Ready,
        gate_state(&mut state),
        SupportStatus::Supported,
        "Fresh evidence recorded after the requirement revision clears delivery.",
    )
}

fn stale_evidence_after_revision() -> CaseResult {
    let (mut state, requirement_id) = contract_with("retry at most three attempts");
    let stale_evidence =
        record_evidence_and_verify(&mut state, requirement_id, "revision 1 behavior passed");
    revise_statement(
        &mut state,
        requirement_id,
        "retry at most three attempts",
        "retry at most four attempts",
    );

    let rejected = state
        .reduce_event(&SessionEvent::RequirementStatusChanged {
            requirement_id,
            status: RequirementStatus::Verified {
                evidence: vec![stale_evidence],
            },
            source: "stale-evidence regression probe".into(),
        })
        .expect_err("evidence from requirement revision 1 must not settle revision 2");
    assert!(rejected.to_string().contains("revision 1"), "{rejected}");
    result(
        "stale-evidence-after-revision",
        DeliveryState::PartiallyComplete,
        gate_state(&mut state),
        SupportStatus::Supported,
        "Revision-bound evidence is rejected before it can re-verify the changed requirement.",
    )
}

fn artifact_edited_after_verification() -> CaseResult {
    let (mut state, requirement_id) = contract_with("the retry limit is four");
    record_evidence_and_verify(
        &mut state,
        requirement_id,
        "retry limit four passed before the later edit",
    );

    let action_id = ActionId::new();
    let artifact = PathBuf::from("src/retry.rs");
    for event in [
        SessionEvent::ActionProposed {
            action_id,
            action: ProposedAction::WriteFile(WriteFileAction {
                path: artifact.clone(),
                content: "const RETRY_LIMIT: usize = 3;\n".into(),
                expected_digest: None,
            }),
            turn_id: None,
        },
        SessionEvent::JudgmentRecorded {
            action_id,
            decision: JudgmentDecision::Allow,
            turn_id: None,
        },
        SessionEvent::ExecutionStarted { action_id },
        SessionEvent::ExecutionFinished {
            action_id,
            exit_code: Some(0),
            truncated: false,
            sandbox_level: None,
            sandbox_backend: None,
            affected_paths: vec![artifact],
        },
    ] {
        state
            .reduce_event(&event)
            .expect("the durable edit lifecycle should reduce");
    }

    let observed = gate_state_for(&mut state, "benchmark-artifact-v2");
    result(
        "artifact-edited-after-verification",
        DeliveryState::PartiallyComplete,
        observed,
        SupportStatus::Supported,
        "The gate re-checks the current artifact digest and returns the requirement to incomplete instead of accepting pre-edit evidence.",
    )
}

fn scope_preservation_violation() -> CaseResult {
    let (mut state, requirement_id) = contract_with("set page size to fifty");
    let non_goal =
        NonGoal::new("do not change auth.py", source("do not change auth.py")).within("auth.py");
    state
        .reduce_event(&SessionEvent::ExpectationContractRevised {
            revision: Box::new(ContractRevision {
                revision: 2,
                source: source("do not change auth.py"),
                reason: "record the protected path".into(),
                changes: vec![ContractChange::NonGoalAdded { non_goal }],
            }),
        })
        .expect("adding a non-goal is a valid contract revision");
    record_evidence_and_verify(&mut state, requirement_id, "page size check passed");

    let action_id = ActionId::new();
    let protected = PathBuf::from("auth.py");
    for event in [
        SessionEvent::ActionProposed {
            action_id,
            action: ProposedAction::WriteFile(WriteFileAction {
                path: protected.clone(),
                content: "# unauthorized edit\n".into(),
                expected_digest: None,
            }),
            turn_id: None,
        },
        SessionEvent::JudgmentRecorded {
            action_id,
            decision: JudgmentDecision::Allow,
            turn_id: None,
        },
        SessionEvent::ExecutionStarted { action_id },
        SessionEvent::ExecutionFinished {
            action_id,
            exit_code: Some(0),
            truncated: false,
            sandbox_level: None,
            sandbox_backend: None,
            affected_paths: vec![protected],
        },
    ] {
        state
            .reduce_event(&event)
            .expect("the durable edit lifecycle should reduce");
    }

    result(
        "scope-preservation-violation",
        DeliveryState::Blocked,
        gate_state(&mut state),
        SupportStatus::Supported,
        "The gate derives scope from changed paths and the contract's forbidden prefix.",
    )
}

fn redundant_correction() -> CaseResult {
    let statement = "page size remains twenty";
    let (mut state, requirement_id) = contract_with(statement);
    record_evidence_and_verify(&mut state, requirement_id, "page size twenty passed");

    // A repeated clarification does not change the requirement's meaning, so
    // its existing revision-bound evidence remains current.
    revise_statement(&mut state, requirement_id, statement, statement);
    let observed = gate_state(&mut state);
    result(
        "redundant-correction",
        DeliveryState::Ready,
        observed,
        SupportStatus::Supported,
        "A same-text restatement is a semantic no-op and preserves the requirement's evidence binding.",
    )
}

fn benign_static_control() -> CaseResult {
    let (mut state, requirement_id) = contract_with("add returns the sum");
    record_evidence_and_verify(&mut state, requirement_id, "add(2, 3) == 5");
    result(
        "benign-static-control",
        DeliveryState::Ready,
        gate_state(&mut state),
        SupportStatus::Supported,
        "A static request with fresh passing evidence is a clean control.",
    )
}

fn report() -> BenchmarkReport {
    BenchmarkReport {
        schema_version: SCHEMA_VERSION,
        benchmark_kind: BENCHMARK_KIND,
        live: false,
        oracle: "hand-written scenario expectations in revision_benchmark.rs",
        cases: vec![
            fresh_evidence_after_revision(),
            stale_evidence_after_revision(),
            artifact_edited_after_verification(),
            scope_preservation_violation(),
            redundant_correction(),
            benign_static_control(),
        ],
    }
}

fn write_report_if_requested(report: &BenchmarkReport) {
    let Ok(path) = std::env::var("PURRCODE_REVISION_BENCHMARK_JSON") else {
        return;
    };
    let path = PathBuf::from(path);
    let bytes = serde_json::to_vec_pretty(report).expect("benchmark report serializes");
    let file = OpenOptions::new().write(true).create_new(true).open(&path);
    match file {
        Ok(mut file) => file
            .write_all(&bytes)
            .expect("benchmark report output is writable"),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            eprintln!(
                "revision mechanism benchmark left existing JSON untouched: {}",
                path.display()
            );
        }
        Err(error) => panic!(
            "cannot create revision mechanism benchmark JSON {}: {error}",
            path.display()
        ),
    }
}

#[test]
fn revision_mechanism_benchmark_is_deterministic_and_opt_in() {
    let report = report();
    assert_eq!(report.benchmark_kind, BENCHMARK_KIND);
    assert!(
        !report.live,
        "this benchmark must never claim live-agent evidence"
    );
    assert_eq!(report.cases.len(), 6);
    assert!(
        report
            .cases
            .iter()
            .all(|case| !case.case_id.is_empty() && !case.note.is_empty())
    );
    let stale = report
        .cases
        .iter()
        .find(|case| case.case_id == "stale-evidence-after-revision")
        .expect("stale evidence case is present");
    assert_eq!(stale.observed, DeliveryState::PartiallyComplete);
    assert_eq!(stale.outcome, CaseOutcome::Pass);
    assert!(stale.pass);

    let artifact = report
        .cases
        .iter()
        .find(|case| case.case_id == "artifact-edited-after-verification")
        .expect("artifact freshness case is present");
    assert_eq!(artifact.support, SupportStatus::Supported);
    assert!(artifact.pass);

    let redundant = report
        .cases
        .iter()
        .find(|case| case.case_id == "redundant-correction")
        .expect("redundant correction case is present");
    assert_eq!(redundant.support, SupportStatus::Supported);
    assert!(redundant.pass);
    write_report_if_requested(&report);
}
