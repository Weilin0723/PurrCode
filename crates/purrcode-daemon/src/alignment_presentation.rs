//! Projecting the v1.5 session state into what a client draws (§18–§22).
//!
//! `ui-contracts::alignment` fixed the vocabulary a year of two clients
//! disagreeing had earned. This is the half that produces it, and it lives here
//! rather than in each client for the same reason the activity labels do: the
//! TUI deriving one reading in Rust and the Studio deriving another in
//! JavaScript is how the same run comes to be described two different ways
//! depending on which window you are looking at.
//!
//! One projection here is doing real work rather than reshaping data. Changes
//! are grouped by the requirement they serve, and a file that serves none ends
//! up in a group that says so. That group is the interesting one: a flat file
//! list makes unrequested work look exactly like requested work, and the user
//! cannot audit scope drift they cannot see.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use purrcode_repository_engine::ChangeSet;
use purrcode_runtime_core::SessionState;
use purrcode_runtime_core::correction::LifecyclePhase;
use purrcode_runtime_core::expectation::{DeliveryState, RequirementStatus};
use purrcode_runtime_core::work::RequirementId;
use purrcode_ui_contracts::alignment::{
    ChangeGroupView, ChangedFileView, ChangesView, ContractClauseView, FindingView, ProgressDetail,
    ProgressPhase, ProgressView, RequirementStatusView, RequirementTraceView, ReviewPanelView,
    TaskContractView, ValidationLineView,
};

/// "What PurrCode understood" (§19).
pub(crate) fn task_contract_view(state: &SessionState) -> Option<TaskContractView> {
    let contract = state.expectation_contract.as_ref()?;
    Some(TaskContractView {
        objective: contract.objective.clone(),
        revision: contract.revision,
        must_satisfy: contract.required().map(clause_view).collect(),
        preferences: contract.preferred().map(clause_view).collect(),
        not_requested: contract
            .non_goals
            .iter()
            .map(|non_goal| non_goal.statement.clone())
            .collect(),
        open_questions: contract
            .open_questions
            .iter()
            .filter(|question| question.answer.is_none())
            .map(|question| {
                if question.blocking {
                    format!("{} (blocking)", question.question)
                } else {
                    question.question.clone()
                }
            })
            .collect(),
    })
}

/// The review panel (§20).
pub(crate) fn review_panel_view(state: &SessionState) -> Option<ReviewPanelView> {
    let contract = state.expectation_contract.as_ref()?;
    let being_corrected: BTreeSet<_> = state.correction_ledger.still_open.iter().copied().collect();
    let repaired: BTreeSet<_> = state.correction_ledger.repaired.iter().copied().collect();

    let findings: Vec<FindingView> = state
        .findings
        .values()
        // A finding the correction loop closed is history, not an open
        // complaint. Leaving it in the panel would tell the user something is
        // wrong with work that was fixed.
        .filter(|finding| !repaired.contains(&finding.id))
        .map(|finding| FindingView {
            id: finding.id.0.to_string(),
            summary: finding.description.clone(),
            blocking: finding.blocks_delivery(),
            being_corrected: being_corrected.contains(&finding.id),
            requirement_id: finding
                .requirement_id
                .map(|requirement| requirement.0.to_string()),
            evidence: finding.evidence.clone(),
        })
        .collect();

    let validations: Vec<ValidationLineView> = state
        .validation_record
        .iter()
        .map(|(name, status)| ValidationLineView {
            name: name.clone(),
            passed: *status == purrcode_runtime_core::ValidationStatus::Passed,
            // "Passed" needs no elaboration; everything else does, because
            // every other status means something different and a client that
            // renders them all as one red mark has thrown that away.
            detail: (*status != purrcode_runtime_core::ValidationStatus::Passed)
                .then(|| format!("{status:?}")),
        })
        .collect();

    let (verdict, still_working_on) = match state.delivery.as_ref() {
        Some(assessment) => (
            assessment.state().label().to_owned(),
            (!assessment.may_report_done()).then(|| assessment.summary()),
        ),
        None => (
            "not yet checked".to_owned(),
            Some("PurrCode has not run the delivery gate on this work yet".to_owned()),
        ),
    };

    Some(ReviewPanelView {
        requirements: contract.required().map(clause_view).collect(),
        findings,
        validations,
        verdict,
        still_working_on,
    })
}

/// The headline while a task runs (§18).
pub(crate) fn progress_view(state: &SessionState) -> Option<ProgressView> {
    let contract = state.expectation_contract.as_ref()?;
    let tally = contract.tally();
    let phase = phase_of(state);
    Some(ProgressView {
        phase: user_facing(phase),
        headline: headline(state, contract.objective.as_str()),
        requirements_settled: tally.settled() as u32,
        requirements_total: tally.total() as u32,
        detail: ProgressDetail {
            workers: state.delegations.len() as u32,
            tool_calls: state.proposed_actions.len() as u32,
            validations: state.validation_record.len() as u32,
            corrections: state.correction_ledger.cycles_used,
            reviews: state.reviews.len() as u32,
        },
    })
}

/// Changes, grouped by the requirement they serve (§21).
///
/// Attribution comes from what the reviewers and the evidence actually said
/// about each file, never from the agent's account of what it was doing. A file
/// nothing points at is unattributed — which is the finding, not a gap in the
/// projection.
pub(crate) fn changes_view(state: &SessionState, changes: &ChangeSet) -> ChangesView {
    let mut titles: BTreeMap<RequirementId, String> = BTreeMap::new();
    if let Some(contract) = state.expectation_contract.as_ref() {
        for clause in &contract.clauses {
            titles.insert(clause.id, clause.statement.clone());
        }
    }

    let mut owner: BTreeMap<String, RequirementId> = BTreeMap::new();
    for finding in state.findings.values() {
        let Some(requirement) = finding.requirement_id else {
            continue;
        };
        for path in &finding.affected_paths {
            owner.insert(path.display().to_string(), requirement);
        }
    }
    for evidence in state.alignment_evidence.values() {
        // Evidence sources are written for a person to follow back —
        // `settings.rs:221`, `cargo test`. The first kind names a file; the
        // second names nothing and is skipped rather than guessed at.
        let source = evidence.source.trim();
        let candidate = source.split(':').next().unwrap_or(source);
        if candidate.contains('/') || candidate.contains('.') {
            owner
                .entry(candidate.to_owned())
                .or_insert(evidence.requirement_id);
        }
    }

    let mut grouped: BTreeMap<Option<RequirementId>, Vec<ChangedFileView>> = BTreeMap::new();
    for file in &changes.scope_files {
        let path = file.path.display().to_string();
        let requirement = attribute(&path, &owner);
        grouped
            .entry(requirement)
            .or_default()
            .push(ChangedFileView {
                path,
                added: file.additions.unwrap_or(0) as u32,
                removed: file.deletions.unwrap_or(0) as u32,
            });
    }

    let groups = grouped
        .into_iter()
        .map(|(requirement, files)| ChangeGroupView {
            requirement_id: requirement.map(|id| id.0.to_string()),
            title: match requirement.and_then(|id| titles.get(&id)) {
                Some(statement) => statement.clone(),
                None => "Changes not tied to anything you asked for".to_owned(),
            },
            files,
        })
        .collect();
    ChangesView { groups }
}

/// One trace per hard requirement (§22).
pub(crate) fn requirement_traces(state: &SessionState) -> Vec<RequirementTraceView> {
    let Some(contract) = state.expectation_contract.as_ref() else {
        return Vec::new();
    };
    contract
        .required()
        .filter_map(|clause| requirement_trace(state, clause.id))
        .collect()
}

/// "Why does PurrCode believe this is done?" (§22).
pub(crate) fn requirement_trace(
    state: &SessionState,
    requirement_id: RequirementId,
) -> Option<RequirementTraceView> {
    let contract = state.expectation_contract.as_ref()?;
    let clause = contract.clause(requirement_id)?;

    let mut implemented_by: BTreeSet<String> = BTreeSet::new();
    let mut validated_by: BTreeSet<String> = BTreeSet::new();
    let mut reviewed_by: BTreeSet<String> = BTreeSet::new();
    for evidence in state.alignment_evidence.values() {
        if evidence.requirement_id != requirement_id {
            continue;
        }
        match evidence.kind {
            purrcode_runtime_core::expectation::AlignmentEvidenceKind::Execution => {
                implemented_by.insert(evidence.source.clone())
            }
            purrcode_runtime_core::expectation::AlignmentEvidenceKind::Validation => {
                validated_by.insert(evidence.source.clone())
            }
            purrcode_runtime_core::expectation::AlignmentEvidenceKind::Review => {
                reviewed_by.insert(evidence.source.clone())
            }
        };
    }
    for review in state.reviews.values() {
        let touches = review.findings.iter().any(|finding| {
            state
                .findings
                .get(finding)
                .and_then(|finding| finding.requirement_id)
                == Some(requirement_id)
        });
        if touches {
            reviewed_by.insert(review.kind.label().to_owned());
        }
    }

    Some(RequirementTraceView {
        requirement_id: requirement_id.0.to_string(),
        statement: clause.statement.clone(),
        status: status_view(&clause.status),
        implemented_by: implemented_by.into_iter().collect(),
        validated_by: validated_by.into_iter().collect(),
        reviewed_by: reviewed_by.into_iter().collect(),
    })
}

fn clause_view(
    clause: &purrcode_runtime_core::expectation::ExpectationClause,
) -> ContractClauseView {
    ContractClauseView {
        id: clause.id.0.to_string(),
        statement: clause.statement.clone(),
        status: status_view(&clause.status),
        detail: match &clause.status {
            RequirementStatus::Violated { detail, .. } => Some(detail.clone()),
            RequirementStatus::Unknown { detail } => Some(detail.clone()),
            RequirementStatus::Waived { reason, .. } => Some(reason.clone()),
            _ => None,
        },
    }
}

fn status_view(status: &RequirementStatus) -> RequirementStatusView {
    match status {
        RequirementStatus::Verified { .. } => RequirementStatusView::Verified,
        RequirementStatus::Violated { .. } => RequirementStatusView::NotSatisfied,
        RequirementStatus::Unknown { .. } => RequirementStatusView::Undetermined,
        RequirementStatus::Unverified => RequirementStatusView::Pending,
        RequirementStatus::Waived { .. } => RequirementStatusView::Waived,
    }
}

/// A path is attributed when something named it, allowing for the reviewer
/// having written a suffix (`settings.rs`) where the diff has the full path.
fn attribute(path: &str, owner: &BTreeMap<String, RequirementId>) -> Option<RequirementId> {
    if let Some(id) = owner.get(path) {
        return Some(*id);
    }
    let name = Path::new(path).file_name()?.to_str()?;
    owner
        .iter()
        .find(|(named, _)| {
            named.as_str() == name || named.ends_with(path) || path.ends_with(named.as_str())
        })
        .map(|(_, id)| *id)
}

/// Where the runtime thinks the task is.
fn phase_of(state: &SessionState) -> LifecyclePhase {
    match state.delivery.as_ref().map(|assessment| assessment.state()) {
        Some(DeliveryState::Ready) => LifecyclePhase::Ready,
        Some(DeliveryState::NeedsDecision) => LifecyclePhase::NeedsAttention,
        Some(DeliveryState::Blocked) | Some(DeliveryState::PartiallyComplete)
            if !state.correction_ledger.still_open.is_empty() =>
        {
            LifecyclePhase::Correcting
        }
        Some(_) => LifecyclePhase::Implementing,
        None if state.reviews.is_empty() && state.expectation_contract.is_some() => {
            if state.proposed_actions.is_empty() {
                LifecyclePhase::Understanding
            } else {
                LifecyclePhase::Implementing
            }
        }
        None => LifecyclePhase::Reviewing,
    }
}

/// The runtime's vocabulary translated into the user's (§18).
fn user_facing(phase: LifecyclePhase) -> ProgressPhase {
    match phase {
        LifecyclePhase::Understanding => ProgressPhase::Understanding,
        LifecyclePhase::Implementing => ProgressPhase::Working,
        LifecyclePhase::Verifying => ProgressPhase::Checking,
        LifecyclePhase::Reviewing => ProgressPhase::Reviewing,
        LifecyclePhase::Correcting => ProgressPhase::Improving,
        LifecyclePhase::Ready => ProgressPhase::Ready,
        LifecyclePhase::NeedsAttention => ProgressPhase::NeedsYou,
    }
}

fn headline(state: &SessionState, objective: &str) -> String {
    if let Some(assessment) = state.delivery.as_ref()
        && !assessment.may_report_done()
        && let Some(blocker) = assessment.blockers().first()
    {
        return blocker.describe();
    }
    objective.to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use purrcode_repository_engine::ChangedFile;
    use purrcode_runtime_core::expectation::{
        ExpectationClause, ExpectationContract, IntentSource,
    };
    use purrcode_runtime_core::review::{
        FindingCategory, FindingId, ReviewFinding, ReviewId, ReviewKind, Severity,
    };
    use purrcode_runtime_core::work::{AcceptanceCriterion, CriterionId};
    use purrcode_runtime_core::{SessionEvent, SessionId};

    fn session() -> (SessionState, RequirementId) {
        let mut state = SessionState::empty(SessionId::new());
        let mut contract = ExpectationContract::new("Simplify Settings without losing capability");
        let clause = ExpectationClause::required(
            "Every setting that existed before is still reachable",
            vec![AcceptanceCriterion {
                id: CriterionId::new(),
                statement: "each previous setting is reachable".into(),
            }],
            IntentSource::new(0, "don't remove functionality"),
        );
        let id = clause.id;
        contract.clauses.push(clause);
        state
            .reduce_event(&SessionEvent::ExpectationContractCreated {
                contract: Box::new(contract),
            })
            .unwrap();
        (state, id)
    }

    fn changed(path: &str, additions: usize, deletions: usize) -> ChangedFile {
        ChangedFile {
            path: path.into(),
            status: 'M',
            additions: Some(additions),
            deletions: Some(deletions),
        }
    }

    #[test]
    fn work_nobody_asked_for_gets_a_group_that_says_so() {
        // The whole point of grouping. In a flat list the editor change looks
        // exactly like the settings change, and the user cannot audit scope
        // drift they cannot see.
        let (mut state, requirement) = session();
        let review = ReviewId::new();
        state
            .reduce_event(&SessionEvent::ReviewStarted {
                record: Box::new(purrcode_runtime_core::review::ReviewRecord {
                    id: review,
                    kind: ReviewKind::UserAlignment,
                    context: purrcode_runtime_core::review::ReviewContext::Fresh,
                    cycle: 0,
                    completed: false,
                    findings: vec![],
                }),
            })
            .unwrap();
        state
            .reduce_event(&SessionEvent::ReviewFindingRecorded {
                finding: Box::new(ReviewFinding {
                    id: FindingId::new(),
                    review,
                    kind: ReviewKind::UserAlignment,
                    severity: Severity::High,
                    category: FindingCategory::UxMismatch,
                    requirement_id: Some(requirement),
                    description: "the advanced section was deleted".into(),
                    evidence: vec!["settings.rs:221".into()],
                    affected_paths: vec!["crates/ide/src/settings.rs".into()],
                    recommendation: "put it behind a disclosure".into(),
                }),
            })
            .unwrap();

        let changes = ChangeSet {
            scope_files: vec![
                changed("crates/ide/src/settings.rs", 82, 13),
                changed("crates/ide/src/editor/layout.rs", 120, 60),
            ],
            additions: 202,
            deletions: 73,
            patch: Vec::new(),
        };
        let view = changes_view(&state, &changes);
        let unattributed = view.unattributed();
        assert_eq!(unattributed.len(), 1);
        assert_eq!(
            unattributed[0].files[0].path,
            "crates/ide/src/editor/layout.rs"
        );
        assert_eq!(view.files_changed(), 2);
    }

    #[test]
    fn a_session_with_no_gate_result_does_not_claim_one() {
        // `PanelState::Empty` and "the gate said fine" are different facts, and
        // a client that renders the second when it has the first has made a
        // safety claim nobody checked.
        let (state, _) = session();
        let panel = review_panel_view(&state).expect("a contract exists");
        assert_eq!(panel.verdict, "not yet checked");
        assert!(panel.still_working_on.is_some());
        assert!(panel.worth_showing());
    }

    #[test]
    fn the_progress_line_is_two_integers_and_never_a_percentage() {
        let (state, _) = session();
        let view = progress_view(&state).unwrap();
        assert_eq!(view.requirements_total, 1);
        assert_eq!(view.requirements_settled, 0);
        assert_eq!(view.phase, ProgressPhase::Understanding);
        let encoded = serde_json::to_string(&view).unwrap();
        assert!(!encoded.contains("percent"), "{encoded}");
    }

    #[test]
    fn a_trace_with_nothing_behind_it_does_not_support_a_checkmark() {
        let (state, requirement) = session();
        let trace = requirement_trace(&state, requirement).unwrap();
        assert_eq!(trace.status, RequirementStatusView::Pending);
        assert!(trace.validated_by.is_empty());
        assert!(trace.supports_its_status(), "a pending row claims nothing");
    }

    #[test]
    fn a_repaired_finding_leaves_the_panel_and_an_open_one_stays() {
        let (mut state, requirement) = session();
        let review = ReviewId::new();
        state
            .reduce_event(&SessionEvent::ReviewStarted {
                record: Box::new(purrcode_runtime_core::review::ReviewRecord {
                    id: review,
                    kind: ReviewKind::IndependentCode,
                    context: purrcode_runtime_core::review::ReviewContext::Fresh,
                    cycle: 0,
                    completed: false,
                    findings: vec![],
                }),
            })
            .unwrap();
        let fixed = FindingId::new();
        for (id, description) in [
            (fixed, "the registry is never reloaded"),
            (FindingId::new(), "the picker is three clicks deep"),
        ] {
            state
                .reduce_event(&SessionEvent::ReviewFindingRecorded {
                    finding: Box::new(ReviewFinding {
                        id,
                        review,
                        kind: ReviewKind::IndependentCode,
                        severity: Severity::High,
                        category: FindingCategory::Correctness,
                        requirement_id: Some(requirement),
                        description: description.into(),
                        evidence: vec!["mcp_host.rs:83".into()],
                        affected_paths: vec![],
                        recommendation: "reload after a config write".into(),
                    }),
                })
                .unwrap();
        }
        state
            .reduce_event(&SessionEvent::CorrectionStarted {
                cycle: 1,
                findings: vec![fixed],
            })
            .unwrap();
        state
            .reduce_event(&SessionEvent::CorrectionCompleted {
                cycle: 1,
                repaired: vec![fixed],
                still_open: vec![],
            })
            .unwrap();

        let panel = review_panel_view(&state).unwrap();
        assert_eq!(panel.findings.len(), 1);
        assert_eq!(panel.findings[0].summary, "the picker is three clicks deep");
    }
}
