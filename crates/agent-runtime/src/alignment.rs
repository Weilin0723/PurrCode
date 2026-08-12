//! The alignment loop, as the agent runs it (v1.5).
//!
//! `runtime-core` holds what the user asked for and decides whether it has been
//! delivered. `alignment-runtime` compiles intent and produces findings. This
//! module is the part that makes both of them *happen* on a real session:
//!
//! ```text
//! user message → contract → work → validation
//!   → coverage review → independent code review → alignment review
//!   → delivery gate
//!       ready → complete
//!       blocking findings → bounded correction → re-review
//!       anything else → the user
//! ```
//!
//! Two decisions in here are worth stating, because both could reasonably have
//! gone the other way.
//!
//! **A failed contract compilation does not silently disable the gate.** If the
//! intent compiler cannot produce a contract whose clauses quote the user, the
//! session says so in the conversation and continues on the v1.4 path. That is
//! an honest degradation and not a quiet one: the benchmark counts a completion
//! with no gate as a false `Done`, so this route cannot become the comfortable
//! way to finish.
//!
//! **Whether a repair worked is decided by re-review, matched by complaint.**
//! A repair cycle produces new findings with new identities, so nothing links
//! them to the old ones automatically. A finding is treated as repaired when
//! the re-review does not raise a blocking finding of the same category against
//! the same requirement. That is a heuristic and it is worth being plain about
//! its failure mode: a second, different problem in the same place reads as the
//! first one surviving. It errs toward *not* closing findings, which is the
//! direction this release wants to err in.

use std::collections::BTreeSet;
use std::path::PathBuf;

use purrcode_alignment_runtime::{
    AlignmentError, AlignmentReviewer, CorrectionStep, FreshReviewInput, IndependentCodeReviewer,
    IntentCompiler, ModelRoute, ReviewOutcome, ReviewSubject, correction, intent::IntentRequest,
    review::VerdictKind,
};
use purrcode_ninelives::SessionStore;
use purrcode_repository_engine::{ChangeScope, RepositoryEngine, SessionWorktree};
use purrcode_runtime_core::expectation::{
    AlignmentEvidence, AlignmentEvidenceKind, DeliveryAssessment, DeliveryState,
    RequiredValidation, RequirementStatus,
};
use purrcode_runtime_core::review::{FindingCategory, FindingId, ReviewContext, ReviewRecord};
use purrcode_runtime_core::work::{EvidenceId, RequirementId};
use purrcode_runtime_core::{ConversationMessage, SessionEvent, SessionId, SessionState};

use crate::errors::AgentError;

/// How much of the diff a reviewer is shown.
///
/// A reviewer handed a 400 KB patch reads the first part of it and answers
/// confidently about the whole thing. Truncating visibly is worse than useless
/// only if nothing says it happened, so it says so.
const MAXIMUM_DIFF_CHARACTERS: usize = 60_000;

/// How many files are shown in full alongside the diff.
const MAXIMUM_REVIEW_FILES: usize = 12;
const MAXIMUM_FILE_CHARACTERS: usize = 20_000;

/// The three model-facing reviews, and the compiler that produces the contract
/// they check against.
pub struct AlignmentRuntime {
    compiler: IntentCompiler,
    coverage: AlignmentReviewer,
    code: IndependentCodeReviewer,
    alignment: AlignmentReviewer,
}

impl std::fmt::Debug for AlignmentRuntime {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("AlignmentRuntime")
    }
}

/// What the loop should do after a gate evaluation.
#[derive(Clone, Debug)]
pub enum AlignmentVerdict {
    /// The gate cleared. The session may complete.
    Deliver,
    /// Keep working, with this in front of the model.
    KeepWorking { brief: String },
    /// Repair what a review found.
    Correct {
        cycle: u32,
        findings: Vec<FindingId>,
        brief: String,
    },
    /// The user has to decide. Correction cannot help, or its budget is gone.
    HandBack { reason: String },
}

impl AlignmentRuntime {
    /// Build the three reviewers and the compiler from per-role routes.
    ///
    /// Separate routes rather than one, because the whole point of §14 is that a
    /// deployment can point the reviewer at a different model from the one that
    /// wrote the code. Defaulting them to the same deployment is fine; being
    /// unable to change it is not.
    pub fn new(planner: ModelRoute, reviewer: ModelRoute, alignment_reviewer: ModelRoute) -> Self {
        Self {
            compiler: IntentCompiler::new(planner),
            coverage: AlignmentReviewer::requirement_coverage(reviewer.clone()),
            code: IndependentCodeReviewer::new(reviewer),
            alignment: AlignmentReviewer::user_alignment(alignment_reviewer),
        }
    }

    /// Compile the user's request into a contract and record it (§3).
    ///
    /// Returns the "what PurrCode understood" paragraph when a contract was
    /// established, and `None` when the session already had one.
    pub async fn establish_contract(
        &self,
        store: &mut SessionStore,
        session_id: SessionId,
        state: &SessionState,
        objective: &str,
    ) -> Result<Option<String>, AgentError> {
        if state.expectation_contract.is_some() {
            return Ok(None);
        }
        let request = IntentRequest {
            user_messages: user_messages(state, objective),
            repository_summary: state
                .repository
                .as_ref()
                .map(|path| format!("The task runs against the repository at {}", path.display()))
                .unwrap_or_default(),
        };
        match self.compiler.compile(&request).await {
            Ok(compiled) => {
                store.append(
                    session_id,
                    &SessionEvent::ExpectationContractCreated {
                        contract: Box::new(compiled.contract),
                    },
                )?;
                note(
                    store,
                    session_id,
                    &format!("What I understood: {}", compiled.understanding),
                )?;
                Ok(Some(compiled.understanding))
            }
            Err(error) => {
                // Said out loud rather than swallowed. A session with no
                // contract has no delivery gate, and the user is entitled to
                // know that this run is the old kind.
                note(
                    store,
                    session_id,
                    &format!(
                        "I could not turn this request into a checkable contract, so this task \
                         runs without the delivery gate: {error}"
                    ),
                )?;
                Ok(None)
            }
        }
    }

    /// Record a user correction as a revision of the contract (§5).
    pub async fn record_correction(
        &self,
        store: &mut SessionStore,
        session_id: SessionId,
        state: &SessionState,
        follow_up: &str,
    ) -> Result<Option<String>, AgentError> {
        let Some(contract) = state.expectation_contract.as_ref() else {
            return Ok(None);
        };
        let request = IntentRequest {
            user_messages: user_messages(state, follow_up),
            repository_summary: String::new(),
        };
        match self.compiler.revise(contract, &request).await {
            Ok(revision) => {
                let reason = revision.reason.clone();
                let outcome = contract
                    .revise(revision.clone())
                    .map_err(|error| AgentError::InvalidModelTurn(error.to_string()))?;
                store.append(
                    session_id,
                    &SessionEvent::ExpectationContractRevised {
                        revision: Box::new(revision),
                    },
                )?;
                note(
                    store,
                    session_id,
                    &format!("Contract updated: {reason}. {}", outcome.summary()),
                )?;
                Ok(Some(reason))
            }
            // A follow-up that changes nothing is the common case — "carry on",
            // "thanks", a question. It is not an error and does not deserve a
            // line in the conversation.
            Err(AlignmentError::Invalid(_)) => Ok(None),
            Err(error) => {
                note(
                    store,
                    session_id,
                    &format!("I could not fit that correction to the contract: {error}"),
                )?;
                Ok(None)
            }
        }
    }

    /// Run the three reviews, move the requirement statuses they settle, close
    /// any correction cycle that was running, and evaluate the gate.
    #[allow(clippy::too_many_arguments)]
    pub async fn review_and_gate(
        &self,
        store: &mut SessionStore,
        session_id: SessionId,
        worktree: &SessionWorktree,
        repository_rules: &str,
        validations: &[RequiredValidation],
        cycle: u32,
        pending: Option<(u32, Vec<FindingId>)>,
    ) -> Result<DeliveryAssessment, AgentError> {
        let state = store.load(session_id)?;
        let contract = state
            .expectation_contract
            .clone()
            .ok_or_else(|| AgentError::CorruptSession("the contract vanished mid-run".into()))?;
        let requirement_ids: Vec<RequirementId> =
            contract.required().map(|clause| clause.id).collect();

        let changes = RepositoryEngine::changes(worktree, ChangeScope::Agent).await?;
        let diff = render_diff(&changes.patch);
        let files = review_files(worktree, &changes).await;
        let base = FreshReviewInput::for_contract(&contract)
            .with_repository_rules(repository_rules)
            .with_diff(diff)
            .with_files(files)
            .with_validations(
                validations
                    .iter()
                    .map(|check| (check.name.clone(), format!("{:?}", check.status)))
                    .collect(),
            );

        let mut outcomes = Vec::new();
        // Coverage first: it is the cheapest and the one that catches the
        // requirement everybody forgot, which changes what the other two are
        // looking at.
        for outcome in [
            self.coverage.review(base.clone(), &requirement_ids).await,
            self.code.review(base.clone(), &requirement_ids).await,
            self.alignment.review(base.clone(), &requirement_ids).await,
        ] {
            match outcome {
                Ok(outcome) => outcomes.push(outcome),
                // One reviewer failing must not silently reduce the standard.
                // The failure is recorded, and the requirements that reviewer
                // would have settled stay unsettled — which blocks the gate,
                // which is the correct outcome for "we could not check".
                Err(error) => note(
                    store,
                    session_id,
                    &format!("A review could not be completed: {error}"),
                )?,
            }
        }

        for outcome in &outcomes {
            self.record_review(store, session_id, outcome, cycle)?;
        }

        if let Some((cycle, attempted)) = pending {
            self.close_correction_cycle(store, session_id, cycle, &attempted, &outcomes)?;
        }

        self.settle_requirements(store, session_id, &outcomes)?;

        store.append(
            session_id,
            &SessionEvent::DeliveryGateEvaluated {
                validations: validations.to_vec(),
            },
        )?;
        store
            .load(session_id)?
            .delivery
            .ok_or_else(|| AgentError::CorruptSession("the gate ran and recorded nothing".into()))
    }

    fn record_review(
        &self,
        store: &mut SessionStore,
        session_id: SessionId,
        outcome: &ReviewOutcome,
        cycle: u32,
    ) -> Result<(), AgentError> {
        store.append(
            session_id,
            &SessionEvent::ReviewStarted {
                record: Box::new(ReviewRecord {
                    id: outcome.review,
                    kind: outcome.kind,
                    // True by construction: `FreshReviewInput` has nowhere to
                    // put a transcript, so this is a record of what happened
                    // rather than an assertion about it.
                    context: ReviewContext::Fresh,
                    cycle,
                    completed: false,
                    findings: Vec::new(),
                }),
            },
        )?;
        for finding in &outcome.findings {
            store.append(
                session_id,
                &SessionEvent::ReviewFindingRecorded {
                    finding: Box::new(finding.clone()),
                },
            )?;
        }
        store.append(
            session_id,
            &SessionEvent::ReviewCompleted {
                review: outcome.review,
            },
        )?;
        Ok(())
    }

    /// Decide what a repair cycle achieved, from the re-review rather than from
    /// the repairer.
    fn close_correction_cycle(
        &self,
        store: &mut SessionStore,
        session_id: SessionId,
        cycle: u32,
        attempted: &[FindingId],
        outcomes: &[ReviewOutcome],
    ) -> Result<(), AgentError> {
        let state = store.load(session_id)?;
        let returned: BTreeSet<(FindingCategory, Option<RequirementId>)> = outcomes
            .iter()
            .flat_map(|outcome| outcome.findings.iter())
            .filter(|finding| finding.blocks_delivery())
            .map(|finding| (finding.category, finding.requirement_id))
            .collect();

        let mut repaired = Vec::new();
        let mut still_open = Vec::new();
        for id in attempted {
            let Some(finding) = state.findings.get(id) else {
                continue;
            };
            if returned.contains(&(finding.category, finding.requirement_id)) {
                still_open.push(*id);
            } else {
                repaired.push(*id);
            }
        }
        store.append(
            session_id,
            &SessionEvent::CorrectionCompleted {
                cycle,
                repaired,
                still_open,
            },
        )?;
        Ok(())
    }

    /// Move each requirement to what the reviewers settled, recording the
    /// evidence first so the citation resolves.
    fn settle_requirements(
        &self,
        store: &mut SessionStore,
        session_id: SessionId,
        outcomes: &[ReviewOutcome],
    ) -> Result<(), AgentError> {
        // Worst verdict wins. A requirement one reviewer called satisfied and
        // another called violated is not satisfied, and averaging the two is
        // how a contradicted requirement ships.
        let mut merged: Vec<(RequirementId, VerdictKind, String, Vec<String>)> = Vec::new();
        for outcome in outcomes {
            for verdict in &outcome.verdicts {
                let severity = |kind: VerdictKind| match kind {
                    VerdictKind::Satisfied => 0,
                    VerdictKind::Undetermined => 1,
                    VerdictKind::Violated => 2,
                };
                match merged
                    .iter_mut()
                    .find(|(id, ..)| *id == verdict.requirement_id)
                {
                    Some(existing) if severity(verdict.kind) > severity(existing.1) => {
                        existing.1 = verdict.kind;
                        existing.2 = verdict.detail.clone();
                        existing.3 = verdict.evidence.clone();
                    }
                    Some(existing) => existing.3.extend(verdict.evidence.iter().cloned()),
                    None => merged.push((
                        verdict.requirement_id,
                        verdict.kind,
                        verdict.detail.clone(),
                        verdict.evidence.clone(),
                    )),
                }
            }
        }

        for (requirement_id, kind, detail, evidence) in merged {
            let mut ids: Vec<EvidenceId> = Vec::new();
            for item in &evidence {
                let record = AlignmentEvidence::new(
                    AlignmentEvidenceKind::Review,
                    requirement_id,
                    item.clone(),
                    detail.clone(),
                );
                ids.push(record.id);
                store.append(
                    session_id,
                    &SessionEvent::AlignmentEvidenceRecorded {
                        evidence: Box::new(record),
                    },
                )?;
            }
            let status = purrcode_alignment_runtime::RequirementVerdict {
                requirement_id,
                kind,
                detail,
                evidence,
            }
            .into_status(ids);
            let source = match &status {
                RequirementStatus::Verified { .. } => "alignment review: satisfied",
                RequirementStatus::Violated { .. } => "alignment review: violated",
                _ => "alignment review: could not be established",
            };
            store.append(
                session_id,
                &SessionEvent::RequirementStatusChanged {
                    requirement_id,
                    status,
                    source: source.into(),
                },
            )?;
        }
        Ok(())
    }
}

/// Turn a gate result into the loop's next move.
///
/// Kept separate from the gate itself because the gate answers "may this be
/// delivered" and this answers "what now" — and the second question has answers
/// the first one has no business knowing about, like how much correction budget
/// is left.
pub fn decide(
    state: &SessionState,
    assessment: &DeliveryAssessment,
    rounds: u8,
) -> AlignmentVerdict {
    if assessment.may_report_done() {
        return AlignmentVerdict::Deliver;
    }
    let Some(contract) = state.expectation_contract.as_ref() else {
        return AlignmentVerdict::HandBack {
            reason: assessment.summary(),
        };
    };
    let outstanding = state.outstanding_findings();
    match correction::next_step(&state.correction_ledger, &outstanding, contract) {
        CorrectionStep::Repair(assignment) => AlignmentVerdict::Correct {
            cycle: assignment.cycle,
            findings: assignment.findings.clone(),
            brief: assignment.brief.clone(),
        },
        CorrectionStep::Exhausted {
            cycles_used,
            allowed,
            abandoned,
        } => AlignmentVerdict::HandBack {
            reason: format!(
                "{}\n\nI tried {cycles_used} of {allowed} automatic correction cycles and \
                 {} issue(s) are still open. This needs your decision rather than another \
                 attempt.",
                assessment.summary(),
                abandoned.len()
            ),
        },
        // Nothing to repair, and still not deliverable: the work itself is
        // unfinished. Whether to keep going or hand back depends on whether
        // going round again could plausibly help.
        CorrectionStep::NothingToFix => match assessment.state() {
            DeliveryState::PartiallyComplete if rounds < MAXIMUM_ALIGNMENT_ROUNDS => {
                AlignmentVerdict::KeepWorking {
                    brief: format!(
                        "This is not finished. The delivery gate says:\n\n{}\n\nKeep working on \
                         the outstanding requirements. Do not report completion until they are \
                         satisfied — the gate decides that, and it reads the repository rather \
                         than your summary.",
                        assessment.summary()
                    ),
                }
            }
            // `NeedsDecision` is a question only the user can settle, and going
            // round again would produce the same question with more tokens
            // spent on it.
            _ => AlignmentVerdict::HandBack {
                reason: assessment.summary(),
            },
        },
    }
}

/// How many times the loop will send the agent back to unfinished requirements
/// before handing the task to the user.
///
/// Each round costs three reviews, so this is not free, and an agent that has
/// been told twice that a requirement is unmet and has not met it is usually
/// stuck on something a person can resolve in a sentence.
pub const MAXIMUM_ALIGNMENT_ROUNDS: u8 = 3;

/// The user's own words, in order, with the current turn's message last.
fn user_messages(state: &SessionState, current: &str) -> Vec<String> {
    let mut messages: Vec<String> = state
        .conversation_messages
        .iter()
        .filter(|message| message.role.eq_ignore_ascii_case("user"))
        .map(|message| message.content.clone())
        .collect();
    if messages
        .last()
        .map(|last| last.trim() != current.trim())
        .unwrap_or(true)
        && !current.trim().is_empty()
    {
        messages.push(current.to_owned());
    }
    messages
}

fn render_diff(patch: &[u8]) -> String {
    let text = String::from_utf8_lossy(patch);
    if text.chars().count() <= MAXIMUM_DIFF_CHARACTERS {
        return text.into_owned();
    }
    let kept: String = text.chars().take(MAXIMUM_DIFF_CHARACTERS).collect();
    format!(
        "{kept}\n\n[the diff was truncated here; you have not seen all of it, so do not \
         report the parts you cannot see as correct]"
    )
}

async fn review_files(
    worktree: &SessionWorktree,
    changes: &purrcode_repository_engine::ChangeSet,
) -> Vec<ReviewSubject> {
    let mut files = Vec::new();
    for changed in changes.scope_files.iter().take(MAXIMUM_REVIEW_FILES) {
        if changed.status == 'D' {
            continue;
        }
        let path: PathBuf = changed.path.clone();
        let Ok(contents) = RepositoryEngine::file_diff_contents(worktree, &path).await else {
            continue;
        };
        let Some(after) = contents.after else {
            continue;
        };
        let trimmed: String = after.chars().take(MAXIMUM_FILE_CHARACTERS).collect();
        files.push(ReviewSubject::new(path, trimmed));
    }
    files
}

/// Say something to the user in the conversation, as the runtime rather than as
/// the agent.
fn note(store: &mut SessionStore, session_id: SessionId, content: &str) -> Result<(), AgentError> {
    store.append(
        session_id,
        &SessionEvent::ConversationMessageAdded {
            message: ConversationMessage {
                id: uuid::Uuid::new_v4().to_string(),
                role: "system".into(),
                content: content.to_owned(),
                timestamp: chrono::Utc::now(),
                tool_calls: Vec::new(),
                tool_results: Vec::new(),
                model: None,
                turn_id: None,
            },
        },
    )?;
    Ok(())
}
