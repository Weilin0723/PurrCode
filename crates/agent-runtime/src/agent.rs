//! The `agent.rs` module contains the core [`NativeAgent`] orchestration loop.
//!
//! ## Line-count note
//!
//! At ~1,400 lines of non-test code this file is larger than the original ~1,200
//! line soft target. The excess is justified because:
//!
//! 1. The v0.6 decomposition already extracted five sibling modules (context,
//!    errors, normalize, schema, stream) from what was previously a single
//!    monolithic file.
//! 2. Tests live alongside the code they exercise (~980 lines) and are included
//!    only under `#[cfg(test)]`.
//! 3. The [`NativeAgent`] struct carries public API methods for the daemon
//!    (start, resume, approve, reject, plan, cancel), provider-stream
//!    integration, and the inner `run_until_pause` loop — all of which share
//!    internal state and type aliases. Extracting them further would force
//!    `pub(crate)` cross-module plumbing without reducing overall complexity.
//!
//! Future splitting should target the `run_until_pause` loop body into a
//! dedicated state-machine file once the next iteration of the agent protocol
//! stabilises.

use chrono::Utc;
use futures::StreamExt;
use purrcode_claw::{ExecutionResult, ToolRuntime};
use purrcode_contextual_judgment::ContextualJudge;
use purrcode_ninelives::SessionStore;
use purrcode_pawgate::Policy;
use purrcode_provider_gateway::{
    ModelEvent, ModelId, ModelMessage, ModelProvider, ModelRequest, ProviderError,
    ProviderErrorCategory, ProviderStreamEvent, StreamIncrement, StreamTracker,
    MAX_PROVIDER_HTTP_BODY_BYTES, MAX_PROVIDER_STREAM_FRAME_BYTES,
};
use purrcode_repository_engine::{RepositoryEngine, SessionWorktree};
use purrcode_runtime_core::{
    ActionId, ApprovalAuthority, Authorization, ContextualDecision, ContextualJudgment,
    ConversationMessage, JudgmentDecision, ProposedAction, SessionEvent, SessionId, SessionStatus,
    ValidationStatus,
};
use purrcode_validation_runtime::{ValidationDetector, ValidationRunner};
use purrcode_whisker::RetrievalBudget;
use schemars::schema_for;
use serde::de::DeserializeOwned;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::Notify;

use crate::context::{
    bounded_terminal_text, build_contextual_request, build_messages, build_outcome_request,
    build_plan_messages, completed_outcome, session_worktree, task_related_paths,
    AgentContextIndex, AgentContextPolicy, ContextualRequestInput,
};
use crate::errors::AgentError;
use crate::normalize::{
    decision_constraints, normalize_action, requires_contextual_judgment,
    successful_duplicate_action,
};
use crate::schema::{
    objective_requests_advice_only, validate_plan, validate_turn, AgentPlan, AgentTurn,
};
use crate::stream::{AgentStreamEvent, AgentStreamObserver, RationaleStreamExtractor};

const MAX_AUTONOMOUS_ITERATIONS: usize = 32;
const MAX_CONSECUTIVE_POLICY_REJECTIONS: usize = 3;
const MAX_ACTIONS_IN_PROMPT: usize = 12;
const RETAINED_ACTIONS_AFTER_COMPACTION: usize = 6;
const MAX_REJECTED_RESPONSE_PREVIEW_CHARS: usize = 4_096;

fn safe_rejected_response_preview(output: &str, attempt: u8) -> String {
    let preview = output
        .chars()
        .filter(|character| !crate::stream::is_unsafe_terminal_control(*character))
        .take(MAX_REJECTED_RESPONSE_PREVIEW_CHARS)
        .collect::<String>();
    if preview.trim().is_empty() {
        String::new()
    } else {
        let truncated = output.chars().count() > MAX_REJECTED_RESPONSE_PREVIEW_CHARS;
        format!(
            "Rejected structured response (attempt {attempt}):\n{preview}{}",
            if truncated {
                "\n… output truncated"
            } else {
                ""
            }
        )
    }
}
/// Cooperative cancellation shared by the daemon/client owner and a [`NativeAgent`].
#[derive(Clone, Debug, Default)]
pub struct AgentCancellation {
    inner: Arc<AgentCancellationInner>,
}

#[derive(Debug, Default)]
struct AgentCancellationInner {
    cancelled: AtomicBool,
    notify: Notify,
}

impl AgentCancellation {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        if !self.inner.cancelled.swap(true, Ordering::AcqRel) {
            self.inner.notify.notify_waiters();
        }
    }

    pub fn is_cancelled(&self) -> bool {
        self.inner.cancelled.load(Ordering::Acquire)
    }

    async fn cancelled(&self) {
        loop {
            let notified = self.inner.notify.notified();
            if self.is_cancelled() {
                return;
            }
            notified.await;
        }
    }
}

pub struct NativeAgent<'a> {
    provider: &'a dyn ModelProvider,
    model: ModelId,
    policy: Policy,
    contextual_judge: Option<ContextualJudge<'a>>,
    stream_observer: Option<AgentStreamObserver>,
    cancellation: Option<AgentCancellation>,
}

impl<'a> NativeAgent<'a> {
    pub fn new(provider: &'a dyn ModelProvider, model: ModelId, policy: Policy) -> Self {
        Self {
            provider,
            model,
            policy,
            contextual_judge: None,
            stream_observer: None,
            cancellation: None,
        }
    }

    pub fn with_contextual_judge(
        mut self,
        provider: &'a dyn ModelProvider,
        model: ModelId,
    ) -> Self {
        self.contextual_judge = Some(ContextualJudge::new(provider, model));
        self
    }

    pub fn with_stream_observer(mut self, observer: AgentStreamObserver) -> Self {
        self.stream_observer = Some(observer);
        self
    }

    pub fn with_cancellation(mut self, cancellation: AgentCancellation) -> Self {
        self.cancellation = Some(cancellation);
        self
    }

    async fn structured_observed<T, V>(
        &self,
        role: &str,
        attempt: u8,
        request: ModelRequest,
        schema: schemars::schema::RootSchema,
        validate: V,
    ) -> Result<(T, Option<(u64, u64)>), AgentError>
    where
        T: DeserializeOwned,
        V: FnOnce(&T) -> Result<(), AgentError>,
    {
        let tracker = self.begin_stream_observation(role, attempt).await?;
        self.structured_observed_from_tracker(role, attempt, tracker, request, schema, validate)
            .await
    }

    async fn begin_stream_observation(
        &self,
        role: &str,
        attempt: u8,
    ) -> Result<StreamTracker, AgentError> {
        let mut tracker = StreamTracker::new(Instant::now());
        self.emit_stream_increment(&mut tracker, role, attempt, StreamIncrement::Queued)
            .await?;
        self.emit_stream_increment(
            &mut tracker,
            role,
            attempt,
            StreamIncrement::PreparingContext,
        )
        .await?;
        Ok(tracker)
    }

    async fn structured_observed_from_tracker<T, V>(
        &self,
        role: &str,
        attempt: u8,
        mut tracker: StreamTracker,
        request: ModelRequest,
        schema: schemars::schema::RootSchema,
        validate: V,
    ) -> Result<(T, Option<(u64, u64)>), AgentError>
    where
        T: DeserializeOwned,
        V: FnOnce(&T) -> Result<(), AgentError>,
    {
        self.emit_stream_increment(&mut tracker, role, attempt, StreamIncrement::SendingRequest)
            .await?;

        let stream_result = match &self.cancellation {
            Some(cancellation) => {
                tokio::select! {
                    biased;
                    _ = cancellation.cancelled() => {
                        self.emit_stream_increment(
                            &mut tracker,
                            role,
                            attempt,
                            StreamIncrement::Interrupted {
                                reason: "agent request cancelled".into(),
                            },
                        )
                        .await?;
                        return Err(AgentError::Cancelled(
                            "cancelled before provider response headers".into(),
                        ));
                    }
                    result = self.provider.structured_stream(request, schema) => result,
                }
            }
            None => self.provider.structured_stream(request, schema).await,
        };
        let mut stream = match stream_result {
            Ok(stream) => stream,
            Err(error) => {
                self.emit_provider_failure(&mut tracker, role, attempt, &error)
                    .await?;
                return Err(error.into());
            }
        };
        let mut output = String::new();
        let mut rationale_extractor =
            (role == "coding_worker").then(RationaleStreamExtractor::default);
        let mut usage = None;
        let mut provider_finished = false;
        loop {
            let item = match &self.cancellation {
                Some(cancellation) => {
                    tokio::select! {
                        biased;
                        _ = cancellation.cancelled() => {
                            self.emit_stream_increment(
                                &mut tracker,
                                role,
                                attempt,
                                StreamIncrement::Interrupted {
                                    reason: "agent request cancelled".into(),
                                },
                            )
                            .await?;
                            return Err(AgentError::Cancelled(
                                "cancelled while receiving provider output".into(),
                            ));
                        }
                        item = stream.next() => item,
                    }
                }
                None => stream.next().await,
            };
            let Some(item) = item else {
                break;
            };
            let event = match item {
                Ok(event) => event,
                Err(error) => {
                    self.emit_provider_failure(&mut tracker, role, attempt, &error)
                        .await?;
                    return Err(error.into());
                }
            };
            match event {
                ProviderStreamEvent::Connected => {
                    self.emit_stream_increment(
                        &mut tracker,
                        role,
                        attempt,
                        StreamIncrement::Connected,
                    )
                    .await?;
                }
                ProviderStreamEvent::BytesReceived { byte_count } => {
                    self.emit_stream_increment(
                        &mut tracker,
                        role,
                        attempt,
                        StreamIncrement::BytesReceived { byte_count },
                    )
                    .await?;
                }
                ProviderStreamEvent::Model(ModelEvent::ResponseStarted { response_id }) => {
                    self.emit_stream_increment(
                        &mut tracker,
                        role,
                        attempt,
                        StreamIncrement::ResponseStarted { response_id },
                    )
                    .await?;
                }
                ProviderStreamEvent::Model(ModelEvent::TextDelta(delta)) => {
                    if delta.len() > MAX_PROVIDER_STREAM_FRAME_BYTES {
                        let error = AgentError::InvalidModelTurn(format!(
                            "structured delta exceeded the {MAX_PROVIDER_STREAM_FRAME_BYTES} byte limit"
                        ));
                        self.emit_agent_failure(&mut tracker, role, attempt, &error)
                            .await?;
                        return Err(error);
                    }
                    let Some(total) = output.len().checked_add(delta.len()) else {
                        let error = AgentError::InvalidModelTurn(
                            "structured stream size overflowed".into(),
                        );
                        self.emit_agent_failure(&mut tracker, role, attempt, &error)
                            .await?;
                        return Err(error);
                    };
                    if total > MAX_PROVIDER_HTTP_BODY_BYTES {
                        let error = AgentError::InvalidModelTurn(format!(
                            "structured stream exceeded the {MAX_PROVIDER_HTTP_BODY_BYTES} byte limit"
                        ));
                        self.emit_agent_failure(&mut tracker, role, attempt, &error)
                            .await?;
                        return Err(error);
                    }
                    self.emit_stream_increment(
                        &mut tracker,
                        role,
                        attempt,
                        StreamIncrement::ContentDelta {
                            delta: delta.clone(),
                        },
                    )
                    .await?;
                    if let Some(visible) = rationale_extractor
                        .as_mut()
                        .and_then(|extractor| extractor.push(&delta))
                    {
                        self.emit_stream_event(AgentStreamEvent::ContentDelta {
                            role: role.into(),
                            attempt,
                            delta: visible,
                        })
                        .await;
                    }
                    output.push_str(&delta);
                }
                ProviderStreamEvent::Model(ModelEvent::ToolCall {
                    call_id,
                    name,
                    arguments,
                }) => {
                    self.emit_stream_increment(
                        &mut tracker,
                        role,
                        attempt,
                        StreamIncrement::ToolCall {
                            call_id,
                            name,
                            arguments,
                        },
                    )
                    .await?;
                    let error = AgentError::InvalidModelTurn(
                        "structured provider response unexpectedly requested a tool".into(),
                    );
                    self.emit_agent_failure(&mut tracker, role, attempt, &error)
                        .await?;
                    return Err(error);
                }
                ProviderStreamEvent::Model(ModelEvent::Usage {
                    input_tokens,
                    output_tokens,
                }) => {
                    usage = Some((input_tokens, output_tokens));
                    self.emit_stream_increment(
                        &mut tracker,
                        role,
                        attempt,
                        StreamIncrement::Usage {
                            input_tokens,
                            output_tokens,
                        },
                    )
                    .await?;
                }
                ProviderStreamEvent::Model(ModelEvent::Finished) => {
                    self.emit_stream_increment(
                        &mut tracker,
                        role,
                        attempt,
                        StreamIncrement::Finalizing,
                    )
                    .await?;
                    provider_finished = true;
                    break;
                }
            }
        }

        if !provider_finished {
            let error =
                AgentError::InvalidModelTurn("structured provider stream ended unfinished".into());
            self.emit_agent_failure(&mut tracker, role, attempt, &error)
                .await?;
            return Err(error);
        }

        let parsed = match serde_json::from_str::<T>(&output) {
            Ok(parsed) => parsed,
            Err(error) => {
                let preview = safe_rejected_response_preview(&output, attempt);
                if !preview.is_empty() {
                    self.emit_stream_event(AgentStreamEvent::ContentDelta {
                        role: role.into(),
                        attempt,
                        delta: preview,
                    })
                    .await;
                }
                let error = AgentError::Structured(error);
                self.emit_agent_failure(&mut tracker, role, attempt, &error)
                    .await?;
                return Err(error);
            }
        };
        if let Some(extractor) = &rationale_extractor {
            let rationale_matches = serde_json::from_str::<serde_json::Value>(&output)
                .ok()
                .and_then(|value| {
                    value
                        .get("rationale")
                        .and_then(serde_json::Value::as_str)
                        .map(|rationale| extractor.matches_final(rationale))
                })
                .unwrap_or(false);
            if !rationale_matches {
                let error = AgentError::InvalidModelTurn(
                    "streamed rationale was unsafe, incomplete, or diverged from the validated response"
                        .into(),
                );
                self.emit_agent_failure(&mut tracker, role, attempt, &error)
                    .await?;
                return Err(error);
            }
        }
        if let Err(error) = validate(&parsed) {
            self.emit_agent_failure(&mut tracker, role, attempt, &error)
                .await?;
            return Err(error);
        }
        self.emit_stream_increment(&mut tracker, role, attempt, StreamIncrement::Finished)
            .await?;
        Ok((parsed, usage))
    }

    async fn emit_provider_failure(
        &self,
        tracker: &mut StreamTracker,
        role: &str,
        attempt: u8,
        error: &ProviderError,
    ) -> Result<(), AgentError> {
        let increment = if error.category() == Some(ProviderErrorCategory::Cancelled) {
            StreamIncrement::Interrupted {
                reason: "provider request cancelled".into(),
            }
        } else {
            StreamIncrement::Error {
                message: "provider request failed".into(),
            }
        };
        self.emit_stream_increment(tracker, role, attempt, increment)
            .await
    }

    async fn emit_agent_failure(
        &self,
        tracker: &mut StreamTracker,
        role: &str,
        attempt: u8,
        _error: &AgentError,
    ) -> Result<(), AgentError> {
        self.emit_stream_increment(
            tracker,
            role,
            attempt,
            StreamIncrement::Error {
                message: "structured response failed validation".into(),
            },
        )
        .await
    }

    async fn emit_stream_increment(
        &self,
        tracker: &mut StreamTracker,
        role: &str,
        attempt: u8,
        increment: StreamIncrement,
    ) -> Result<(), AgentError> {
        let update = tracker.observe(Instant::now(), increment)?;
        self.emit_stream_event(AgentStreamEvent::Phase {
            role: role.into(),
            attempt,
            sequence: update.sequence,
            previous_phase: update.previous_phase,
            phase: update.phase,
            timing: update.timing,
        })
        .await;
        Ok(())
    }

    async fn emit_stream_event(&self, event: AgentStreamEvent) {
        if let Some(observer) = &self.stream_observer {
            let _ = observer.sender.send(event).await;
        }
    }

    pub async fn start(
        &self,
        store: &mut SessionStore,
        repository: &Path,
        objective: &str,
    ) -> Result<AgentOutcome, AgentError> {
        self.start_with_session_id(store, repository, objective, SessionId::new())
            .await
    }

    pub async fn start_with_session_id(
        &self,
        store: &mut SessionStore,
        repository: &Path,
        objective: &str,
        session_id: SessionId,
    ) -> Result<AgentOutcome, AgentError> {
        if store.load(session_id)?.event_count != 0 {
            return Err(AgentError::CorruptSession(
                "refusing to start an existing session ID".into(),
            ));
        }
        let repository = repository.canonicalize()?;
        store.append(
            session_id,
            &SessionEvent::SessionCreated {
                objective: objective.into(),
                repository: repository.clone(),
            },
        )?;
        self.start_initialized(store, session_id).await
    }

    pub async fn start_initialized(
        &self,
        store: &mut SessionStore,
        session_id: SessionId,
    ) -> Result<AgentOutcome, AgentError> {
        let state = store.load(session_id)?;
        if state.worktree.is_some()
            || state.event_count != 1 + state.conversation_messages.len() as u64
            || state.status != SessionStatus::Active
        {
            return Err(AgentError::CorruptSession(
                "pre-created session is not in its initial state".into(),
            ));
        }
        let repository = state
            .repository
            .ok_or_else(|| AgentError::CorruptSession("repository is missing".into()))?;
        let worktree = RepositoryEngine::create_worktree(&repository, session_id).await?;
        store.append(
            session_id,
            &SessionEvent::WorktreeCreated {
                path: worktree.path.clone(),
                base_head: worktree.base_head.clone(),
                source_was_dirty: worktree.source_was_dirty,
            },
        )?;
        store.append(
            session_id,
            &SessionEvent::SubmodulesPrepared {
                initialized: worktree.initialized_submodules.clone(),
                unavailable: worktree.unavailable_submodules.clone(),
            },
        )?;
        store.append(
            session_id,
            &SessionEvent::CheckpointCreated {
                label: "session-start".into(),
                head: worktree.base_head,
                patch_digest: blake3::hash(b"").to_hex().to_string(),
            },
        )?;
        self.run_until_pause(store, session_id).await
    }

    pub async fn plan_initialized(
        &self,
        store: &mut SessionStore,
        session_id: SessionId,
    ) -> Result<AgentPlan, AgentError> {
        let state = store.load(session_id)?;
        if state.worktree.is_some()
            || state.event_count != 1 + state.conversation_messages.len() as u64
            || state.status != SessionStatus::Active
        {
            return Err(AgentError::CorruptSession(
                "pre-created plan session is not in its initial state".into(),
            ));
        }
        let repository = state
            .repository
            .ok_or_else(|| AgentError::CorruptSession("repository is missing".into()))?;
        let objective = state
            .objective
            .ok_or_else(|| AgentError::CorruptSession("objective is missing".into()))?;
        let worktree = RepositoryEngine::create_worktree(&repository, session_id).await?;
        store.append(
            session_id,
            &SessionEvent::WorktreeCreated {
                path: worktree.path.clone(),
                base_head: worktree.base_head.clone(),
                source_was_dirty: worktree.source_was_dirty,
            },
        )?;
        store.append(
            session_id,
            &SessionEvent::SubmodulesPrepared {
                initialized: worktree.initialized_submodules.clone(),
                unavailable: worktree.unavailable_submodules.clone(),
            },
        )?;
        store.append(
            session_id,
            &SessionEvent::CheckpointCreated {
                label: "plan-session-start".into(),
                head: worktree.base_head,
                patch_digest: blake3::hash(b"").to_hex().to_string(),
            },
        )?;
        let tracker = self.begin_stream_observation("planner", 1).await?;
        let database = worktree.path.join(".purrcode").join("context.db");
        let mut context_index = AgentContextIndex::open(&worktree.path, &database)?;
        let indexed = context_index.submit_task(&objective, &[], &AgentContextPolicy::default())?;
        store.append(
            session_id,
            &SessionEvent::ContextIndexed {
                files: indexed.summary.indexed_files,
                symbols: indexed.summary.symbols,
                sensitive_files: indexed.summary.sensitive_files,
            },
        )?;
        let hits = context_index.retrieve(&objective, &RetrievalBudget::default())?;
        store.append(
            session_id,
            &SessionEvent::ModelRequestStarted {
                role: "planner".into(),
                provider: self.model.provider.clone(),
                model: self.model.model.clone(),
            },
        )?;
        let request = ModelRequest {
            model: self.model.clone(),
            messages: build_plan_messages(&objective, &worktree.path, &hits),
            tools: Vec::new(),
            max_output_tokens: Some(4096),
            reasoning_effort: None,
        };
        let first = self
            .structured_observed_from_tracker(
                "planner",
                1,
                tracker,
                request.clone(),
                schema_for!(AgentPlan),
                validate_plan,
            )
            .await;
        let (plan, usage) = match first {
            Ok(result) => result,
            Err(first_error) if first_error.is_cancelled() => return Err(first_error),
            Err(first_error) => {
                let mut repair = request;
                repair.messages.push(ModelMessage {
                    role: "user".into(),
                    content: format!(
                        "Your previous structured plan was rejected: {first_error}. Return one corrected JSON object with exactly `steps`, `assumptions`, and `risks`; each value must be an array of plain strings. Do not return objects inside those arrays or any additional fields. This is the only repair attempt."
                    ),
                });
                self.structured_observed(
                    "planner",
                    2,
                    repair,
                    schema_for!(AgentPlan),
                    validate_plan,
                )
                .await?
            }
        };
        let (input_tokens, output_tokens) = usage
            .map(|(input, output)| (Some(input), Some(output)))
            .unwrap_or((None, None));
        store.append(
            session_id,
            &SessionEvent::ModelRequestFinished {
                role: "planner".into(),
                input_tokens,
                output_tokens,
            },
        )?;
        store.append(
            session_id,
            &SessionEvent::PlanCreated {
                steps: plan.steps.clone(),
            },
        )?;
        store.append(
            session_id,
            &SessionEvent::SessionPaused {
                reason: "plan-only session is ready for review".into(),
            },
        )?;
        Ok(plan)
    }

    pub async fn resume(
        &self,
        store: &mut SessionStore,
        session_id: SessionId,
    ) -> Result<AgentOutcome, AgentError> {
        let state = store.load(session_id)?;
        match state.status {
            SessionStatus::Active => self.run_until_pause(store, session_id).await,
            SessionStatus::AwaitingApproval(action_id) => {
                let action = state.proposed_actions.get(&action_id).cloned().ok_or(
                    AgentError::CorruptSession("pending action is missing".into()),
                )?;
                let reason = match state.judgments.get(&action_id) {
                    Some(JudgmentDecision::RequireApproval { reason, .. }) => reason.clone(),
                    _ => {
                        return Err(AgentError::CorruptSession(
                            "pending approval judgment is missing".into(),
                        ))
                    }
                };
                Ok(AgentOutcome::AwaitingApproval {
                    session_id,
                    action_id,
                    reason,
                    action,
                })
            }
            SessionStatus::AwaitingReview => {
                let reason = store
                    .events(session_id)?
                    .into_iter()
                    .rev()
                    .find_map(|event| match event {
                        SessionEvent::OutcomeReviewRequired { reason } => Some(reason),
                        _ => None,
                    })
                    .unwrap_or_else(|| "independent outcome review is required".into());
                Ok(AgentOutcome::AwaitingOutcomeReview { session_id, reason })
            }
            other => Err(AgentError::SessionNotResumable(format!("{other:?}"))),
        }
    }

    pub async fn approve(
        &self,
        store: &mut SessionStore,
        session_id: SessionId,
    ) -> Result<AgentOutcome, AgentError> {
        let state = store.load(session_id)?;
        if state.status == SessionStatus::AwaitingReview {
            store.append(
                session_id,
                &SessionEvent::OutcomeReviewApproved {
                    authority: ApprovalAuthority::Human,
                },
            )?;
            store.append(session_id, &SessionEvent::SessionCompleted)?;
            return completed_outcome(store, session_id);
        }
        let SessionStatus::AwaitingApproval(action_id) = state.status else {
            return Err(AgentError::SessionNotAwaitingApproval);
        };
        let action = state
            .proposed_actions
            .get(&action_id)
            .cloned()
            .ok_or_else(|| AgentError::CorruptSession("pending action is missing".into()))?;
        let constraints = match state.judgments.get(&action_id) {
            Some(JudgmentDecision::RequireApproval { constraints, .. }) => constraints.clone(),
            _ => {
                return Err(AgentError::CorruptSession(
                    "pending judgment constraints are missing".into(),
                ))
            }
        };
        let worktree = session_worktree(&state)?;
        let authorization = Authorization {
            action_id,
            session_id,
            action_digest: action.digest(&constraints)?,
            constraints: constraints.clone(),
            authorized_at: Utc::now(),
            approved_by: ApprovalAuthority::Human,
        };
        store.authorize(&authorization)?;
        let result = execute_and_record(
            store,
            session_id,
            action_id,
            &action,
            &constraints,
            &worktree,
        )
        .await?;
        Ok(AgentOutcome::ActionExecuted {
            session_id,
            action_id,
            result,
        })
    }

    pub fn reject(
        &self,
        store: &mut SessionStore,
        session_id: SessionId,
        reason: &str,
    ) -> Result<(), AgentError> {
        let state = store.load(session_id)?;
        let SessionStatus::AwaitingApproval(action_id) = state.status else {
            return Err(AgentError::SessionNotAwaitingApproval);
        };
        store.append(
            session_id,
            &SessionEvent::ApprovalRejected {
                action_id,
                reason: reason.into(),
            },
        )?;
        Ok(())
    }

    async fn run_until_pause(
        &self,
        store: &mut SessionStore,
        session_id: SessionId,
    ) -> Result<AgentOutcome, AgentError> {
        let mut consecutive_policy_rejections = 0_usize;
        for _ in 0..MAX_AUTONOMOUS_ITERATIONS {
            let state = store.load(session_id)?;
            if state.proposed_actions.len() > MAX_ACTIONS_IN_PROMPT {
                let retained_action_ids: Vec<_> = state
                    .proposed_actions
                    .keys()
                    .rev()
                    .take(RETAINED_ACTIONS_AFTER_COMPACTION)
                    .copied()
                    .collect();
                let compacted = state.proposed_actions.len() - retained_action_ids.len();
                let successful = state
                    .proposed_actions
                    .keys()
                    .filter(|id| {
                        matches!(
                            state.judgments.get(id),
                            Some(
                                JudgmentDecision::Allow | JudgmentDecision::AllowWithConstraints(_)
                            )
                        )
                    })
                    .count();
                store.append(
                    session_id,
                    &SessionEvent::ContextCompacted {
                        summary: format!(
                            "Compacted {compacted} older actions. Across the pre-compaction window, {successful} actions had allow-class deterministic/effective judgments. The durable event log remains authoritative."
                        ),
                        retained_action_ids,
                    },
                )?;
                continue;
            }
            let worktree = state
                .worktree
                .clone()
                .ok_or_else(|| AgentError::CorruptSession("worktree is missing".into()))?;
            let session_worktree = session_worktree(&state)?;
            let objective = state
                .objective
                .clone()
                .ok_or_else(|| AgentError::CorruptSession("objective is missing".into()))?;
            let related_paths = task_related_paths(&state);
            let tracker = self.begin_stream_observation("coding_worker", 1).await?;
            let context_database = worktree.join(".purrcode").join("context.db");
            let mut context_index = AgentContextIndex::open(&worktree, &context_database)?;
            let index_report = context_index.submit_task(
                &objective,
                &related_paths,
                &AgentContextPolicy::default(),
            )?;
            store.append(
                session_id,
                &SessionEvent::ContextIndexed {
                    files: index_report.summary.indexed_files,
                    symbols: index_report.summary.symbols,
                    sensitive_files: index_report.summary.sensitive_files,
                },
            )?;
            let context_hits = context_index.retrieve(&objective, &RetrievalBudget::default())?;
            store.append(
                session_id,
                &SessionEvent::ModelRequestStarted {
                    role: "coding_worker".into(),
                    provider: self.model.provider.clone(),
                    model: self.model.model.clone(),
                },
            )?;
            let session_events = store.events(session_id)?;
            let request = ModelRequest {
                model: self.model.clone(),
                messages: build_messages(
                    &objective,
                    &worktree,
                    &state,
                    &context_hits,
                    &session_events,
                ),
                tools: Vec::new(),
                max_output_tokens: Some(4096),
                reasoning_effort: None,
            };
            let first = self
                .structured_observed_from_tracker(
                    "coding_worker",
                    1,
                    tracker,
                    request.clone(),
                    schema_for!(AgentTurn),
                    validate_turn,
                )
                .await;
            let (turn, usage) = match first {
                Ok(result) => result,
                Err(first_error) if first_error.is_cancelled() => return Err(first_error),
                Err(first_error) => {
                    let mut repair = request;
                    repair.messages.push(ModelMessage {
                        role: "user".into(),
                        content: format!(
                            "Your previous structured action was rejected: {first_error}. Return one corrected response matching the schema. This is the only repair attempt."
                        ),
                    });
                    self.structured_observed(
                        "coding_worker",
                        2,
                        repair,
                        schema_for!(AgentTurn),
                        validate_turn,
                    )
                    .await?
                }
            };
            let (input_tokens, output_tokens) = usage
                .map(|(input, output)| (Some(input), Some(output)))
                .unwrap_or((None, None));
            store.append(
                session_id,
                &SessionEvent::ModelRequestFinished {
                    role: "coding_worker".into(),
                    input_tokens,
                    output_tokens,
                },
            )?;
            let assistant_content = if turn.complete
                && objective_requests_advice_only(&objective)
                && turn.plan.as_ref().is_some_and(|steps| !steps.is_empty())
            {
                let steps = turn
                    .plan
                    .as_ref()
                    .expect("checked as present")
                    .iter()
                    .enumerate()
                    .map(|(index, step)| format!("{}. {step}", index + 1))
                    .collect::<Vec<_>>()
                    .join("\n");
                format!("{}\n\nProposed plan:\n{steps}", turn.rationale)
            } else {
                turn.rationale.clone()
            };
            store.append(
                session_id,
                &SessionEvent::ConversationMessageAdded {
                    message: ConversationMessage {
                        id: ActionId::new().0.to_string(),
                        role: "assistant".into(),
                        content: assistant_content,
                        timestamp: Utc::now(),
                        tool_calls: Vec::new(),
                        tool_results: Vec::new(),
                        model: Some(format!("{}/{}", self.model.provider, self.model.model)),
                    },
                },
            )?;
            if let Some(plan) = turn.plan.clone() {
                if state.plan_steps.is_empty() {
                    store.append(session_id, &SessionEvent::PlanCreated { steps: plan })?;
                } else if state.plan_steps != plan {
                    store.append(
                        session_id,
                        &SessionEvent::PlanRevised {
                            revision: state.plan_revision + 1,
                            reason: turn.rationale.clone(),
                            steps: plan,
                        },
                    )?;
                }
            }
            if turn.complete {
                if objective_requests_advice_only(&objective) {
                    store.append(session_id, &SessionEvent::SessionCompleted)?;
                    return completed_outcome(store, session_id);
                }
                let validation = ValidationDetector::detect(&worktree)?;
                let report =
                    ValidationRunner::run(store, session_id, &worktree, &validation).await?;
                if report.completion_allowed() {
                    if let Some(judge) = self.contextual_judge.as_ref() {
                        let outcome_request = build_outcome_request(
                            &objective,
                            turn.plan.as_deref().unwrap_or(&state.plan_steps),
                            state.plan_revision + u64::from(turn.plan.is_some()),
                            &session_worktree,
                            &report,
                        )
                        .await?;
                        let judgment = match judge.evaluate_outcome(&outcome_request).await {
                            Ok(judgment) => judgment,
                            Err(error) => ContextualJudgment {
                                decision: ContextualDecision::RequireApproval,
                                confidence: 0.0,
                                reasons: vec![format!("outcome judge failed closed: {error}")],
                                cited_evidence_ids: Vec::new(),
                                required_changes: Vec::new(),
                                escalation: Some("human".into()),
                            },
                        };
                        store.append(
                            session_id,
                            &SessionEvent::OutcomeJudgmentRecorded {
                                judgment: judgment.clone(),
                            },
                        )?;
                        match judgment.decision {
                            ContextualDecision::Allow => {}
                            ContextualDecision::RequireApproval => {
                                let reason = judgment.reasons.join("; ");
                                store.append(
                                    session_id,
                                    &SessionEvent::OutcomeReviewRequired {
                                        reason: reason.clone(),
                                    },
                                )?;
                                return Ok(AgentOutcome::AwaitingOutcomeReview {
                                    session_id,
                                    reason,
                                });
                            }
                            ContextualDecision::Replan | ContextualDecision::Deny => {
                                store.append(
                                    session_id,
                                    &SessionEvent::SessionPaused {
                                        reason: "outcome review did not approve completion; revise the objective or resume to replan".into(),
                                    },
                                )?;
                                return Ok(AgentOutcome::ValidationFailed {
                                    session_id,
                                    failed: 1,
                                });
                            }
                        }
                    }
                    store.append(session_id, &SessionEvent::SessionCompleted)?;
                    return completed_outcome(store, session_id);
                }
                store.append(
                    session_id,
                    &SessionEvent::SessionPaused {
                        reason: format!(
                            "validation did not pass ({} check(s) failed, timed out, or remained uncertain); review the evidence before resuming",
                            report
                                .evidence
                                .iter()
                                .filter(|item| {
                                    matches!(
                                        item.status,
                                        purrcode_validation_runtime::EvidenceStatus::Failed
                                            | purrcode_validation_runtime::EvidenceStatus::TimedOut
                                            | purrcode_validation_runtime::EvidenceStatus::Uncertain
                                    )
                                })
                                .count()
                        ),
                    },
                )?;
                return Ok(AgentOutcome::ValidationFailed {
                    session_id,
                    failed: report
                        .evidence
                        .iter()
                        .filter(|item| {
                            matches!(
                                item.status,
                                purrcode_validation_runtime::EvidenceStatus::Failed
                                    | purrcode_validation_runtime::EvidenceStatus::TimedOut
                                    | purrcode_validation_runtime::EvidenceStatus::Uncertain
                            )
                        })
                        .count(),
                });
            }
            let proposed = normalize_action(
                turn.action
                    .ok_or_else(|| AgentError::InvalidModelTurn("action is required".into()))?,
                &worktree,
            )?;
            let action_id = ActionId::new();
            store.append(
                session_id,
                &SessionEvent::ActionProposed {
                    action_id,
                    action: proposed.clone(),
                },
            )?;
            let deterministic = self.policy.evaluate(&proposed, &worktree);
            store.append(
                session_id,
                &SessionEvent::JudgmentRecorded {
                    action_id,
                    decision: deterministic.clone(),
                },
            )?;
            if let Some(constraints) = decision_constraints(&deterministic) {
                if let Some(previous_action_id) =
                    successful_duplicate_action(&state, &session_events, &proposed, constraints)?
                {
                    consecutive_policy_rejections += 1;
                    store.append(
                        session_id,
                        &SessionEvent::JudgmentRecorded {
                            action_id,
                            decision: JudgmentDecision::Replan {
                                reason: format!(
                                    "exact action already succeeded as {}; reuse its recorded output and continue with the next distinct step",
                                    previous_action_id.0
                                ),
                            },
                        },
                    )?;
                    if consecutive_policy_rejections >= MAX_CONSECUTIVE_POLICY_REJECTIONS {
                        store.append(
                            session_id,
                            &SessionEvent::SessionPaused {
                                reason: format!(
                                    "model repeated an already successful exact action {consecutive_policy_rejections} consecutive times; execution was not replayed"
                                ),
                            },
                        )?;
                        return Ok(AgentOutcome::IterationLimit { session_id });
                    }
                    continue;
                }
            }
            let decision = if let (Some(judge), Some(constraints)) = (
                self.contextual_judge
                    .as_ref()
                    .filter(|_| requires_contextual_judgment(&proposed, &deterministic)),
                decision_constraints(&deterministic),
            ) {
                let events = store.events(session_id)?;
                let request = build_contextual_request(ContextualRequestInput {
                    objective: &objective,
                    plan: turn.plan.as_deref().unwrap_or(&state.plan_steps),
                    plan_revision: state.plan_revision + u64::from(turn.plan.is_some()),
                    current_step_index: turn.current_step_index,
                    expected_postconditions: &turn.expected_postconditions,
                    rationale: &turn.rationale,
                    action: &proposed,
                    constraints,
                    context_hits: &context_hits,
                    worktree: &session_worktree,
                    session_events: &events,
                })
                .await?;
                match judge.evaluate(&request, &deterministic).await {
                    Ok(outcome) => {
                        store.append(
                            session_id,
                            &SessionEvent::ContextualJudgmentRecorded {
                                action_id,
                                judgment: outcome.judgment,
                            },
                        )?;
                        store.append(
                            session_id,
                            &SessionEvent::JudgmentRecorded {
                                action_id,
                                decision: outcome.effective_decision.clone(),
                            },
                        )?;
                        outcome.effective_decision
                    }
                    Err(error) => {
                        let judgment = ContextualJudgment {
                            decision: ContextualDecision::RequireApproval,
                            confidence: 0.0,
                            reasons: vec![format!("contextual judge failed closed: {error}")],
                            cited_evidence_ids: Vec::new(),
                            required_changes: Vec::new(),
                            escalation: Some("human".into()),
                        };
                        let effective = JudgmentDecision::RequireApproval {
                            reason: judgment.reasons.join("; "),
                            constraints: constraints.clone(),
                        };
                        store.append(
                            session_id,
                            &SessionEvent::ContextualJudgmentRecorded {
                                action_id,
                                judgment,
                            },
                        )?;
                        store.append(
                            session_id,
                            &SessionEvent::JudgmentRecorded {
                                action_id,
                                decision: effective.clone(),
                            },
                        )?;
                        effective
                    }
                }
            } else {
                deterministic
            };
            match decision {
                JudgmentDecision::AllowWithConstraints(constraints) => {
                    consecutive_policy_rejections = 0;
                    store.authorize(&Authorization {
                        action_id,
                        session_id,
                        action_digest: proposed.digest(&constraints)?,
                        constraints: constraints.clone(),
                        authorized_at: Utc::now(),
                        approved_by: ApprovalAuthority::DeterministicPolicy,
                    })?;
                    execute_and_record(
                        store,
                        session_id,
                        action_id,
                        &proposed,
                        &constraints,
                        &session_worktree,
                    )
                    .await?;
                }
                JudgmentDecision::RequireApproval {
                    reason,
                    constraints: _,
                } => {
                    return Ok(AgentOutcome::AwaitingApproval {
                        session_id,
                        action_id,
                        reason,
                        action: proposed,
                    })
                }
                JudgmentDecision::Deny { .. }
                | JudgmentDecision::ModifyAction { .. }
                | JudgmentDecision::Replan { .. } => {
                    consecutive_policy_rejections += 1;
                    if consecutive_policy_rejections >= MAX_CONSECUTIVE_POLICY_REJECTIONS {
                        store.append(
                            session_id,
                            &SessionEvent::SessionPaused {
                                reason: format!(
                                    "PawGate rejected {consecutive_policy_rejections} consecutive proposed actions; review the policy decision or revise the task before resuming"
                                ),
                            },
                        )?;
                        return Ok(AgentOutcome::IterationLimit { session_id });
                    }
                    continue;
                }
                JudgmentDecision::Allow => {
                    return Err(AgentError::UnsafeUnconstrainedAllow);
                }
            }
        }
        store.append(
            session_id,
            &SessionEvent::SessionPaused {
                reason:
                    "agent reached its bounded action limit; review progress before explicitly resuming"
                        .into(),
            },
        )?;
        Ok(AgentOutcome::IterationLimit { session_id })
    }
}

async fn execute_and_record(
    store: &mut SessionStore,
    session_id: SessionId,
    action_id: ActionId,
    action: &ProposedAction,
    constraints: &purrcode_runtime_core::ActionConstraints,
    worktree: &SessionWorktree,
) -> Result<ExecutionResult, AgentError> {
    let before = RepositoryEngine::snapshot(worktree).await?;
    store.append(session_id, &SessionEvent::ExecutionStarted { action_id })?;
    match ToolRuntime::execute(store, action_id, action, constraints).await {
        Ok(mut result) => {
            let after = RepositoryEngine::snapshot(worktree).await?;
            result.affected_paths =
                RepositoryEngine::validate_effect_delta(&before, &after, constraints)?;
            store.append(
                session_id,
                &SessionEvent::ExecutionFinished {
                    action_id,
                    exit_code: result.exit_code,
                    truncated: result.truncated,
                    sandbox_level: Some(format!("{:?}", result.sandbox_level)),
                    sandbox_backend: Some(result.sandbox_backend.clone()),
                },
            )?;
            store.append(
                session_id,
                &SessionEvent::ActionOutputRecorded {
                    action_id,
                    stdout: bounded_terminal_text(&result.stdout),
                    stderr: bounded_terminal_text(&result.stderr),
                    truncated: result.truncated,
                },
            )?;
            store.append(
                session_id,
                &SessionEvent::ValidationRecorded {
                    action_id,
                    status: if result.exit_code == Some(0) {
                        ValidationStatus::Passed
                    } else {
                        ValidationStatus::Failed
                    },
                    evidence: format!(
                        "exit={:?}; affected_paths={:?}",
                        result.exit_code, result.affected_paths
                    ),
                },
            )?;
            Ok(result)
        }
        Err(error) => {
            store.append(
                session_id,
                &SessionEvent::ValidationRecorded {
                    action_id,
                    status: ValidationStatus::Uncertain,
                    evidence: error.to_string(),
                },
            )?;
            Err(error.into())
        }
    }
}

#[derive(Clone, Debug)]
pub enum AgentOutcome {
    AwaitingApproval {
        session_id: SessionId,
        action_id: ActionId,
        reason: String,
        action: ProposedAction,
    },
    AwaitingOutcomeReview {
        session_id: SessionId,
        reason: String,
    },
    Completed {
        session_id: SessionId,
        passed: usize,
        unavailable: usize,
        not_detected: usize,
    },
    ValidationFailed {
        session_id: SessionId,
        failed: usize,
    },
    IterationLimit {
        session_id: SessionId,
    },
    ActionExecuted {
        session_id: SessionId,
        action_id: ActionId,
        result: ExecutionResult,
    },
}

// ── Skill-first capability resolution ───────────────────────────

/// A capability resolution: either a matching installed skill, external candidates, or unavailable.
#[derive(Clone, Debug)]
pub enum CapabilityResolution {
    /// An installed skill was found and should be preferred.
    InstalledSkill { skill_id: String, tool_name: String },
    /// External candidates were discovered.
    CandidatesFound(Vec<serde_json::Value>),
    /// No resolution possible with current configuration.
    Unavailable,
}

/// Optional trait the daemon can implement to let the agent resolve capabilities
/// against installed skills before falling back to core tools or external search.
#[async_trait::async_trait]
pub trait SkillResolver: Send + Sync {
    async fn resolve(&self, capability: &str) -> CapabilityResolution;
}

impl<'a> NativeAgent<'a> {
    /// Resolve a required capability, checking installed skills first.
    /// Returns the resolution without mutating agent state.
    pub async fn resolve_capability(
        &self,
        capability: &str,
        resolver: Option<&dyn SkillResolver>,
    ) -> CapabilityResolution {
        if let Some(resolver) = resolver {
            let result = resolver.resolve(capability).await;
            match &result {
                CapabilityResolution::InstalledSkill { .. } => return result,
                CapabilityResolution::CandidatesFound(_) => return result,
                CapabilityResolution::Unavailable => {}
            }
        }
        CapabilityResolution::Unavailable
    }
}

#[cfg(test)]
#[path = "agent_tests.rs"]
mod tests;
