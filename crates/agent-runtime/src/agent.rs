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
    MAX_PROVIDER_HTTP_BODY_BYTES, MAX_PROVIDER_STREAM_FRAME_BYTES, ModelEvent, ModelId,
    ModelMessage, ModelProvider, ModelRequest, ProviderError, ProviderErrorCategory,
    ProviderStreamEvent, StreamIncrement, StreamTracker,
};
use purrcode_repository_engine::{RepositoryEngine, SessionWorktree};
use purrcode_runtime_core::adaptation::{
    BudgetConstraints, SessionControls, UsageLedger, UsageRecord,
};
use purrcode_runtime_core::work::{
    AcceptanceCriterion, CriterionId, Requirement, RequirementId, SpecBundle, SpecKind, TaskGraph,
    WorkPriority, WorkRisk, WorkTask, WorkTaskId, WorkTaskStatus,
};
use purrcode_runtime_core::{
    ActionConstraints, ActionId, ApprovalAuthority, Authorization, CheckpointDecision,
    CheckpointId, ContextClass, ContextLedgerEntry, ContextLedgerSection, ContextualDecision,
    ContextualJudgment, ConversationMessage, FailedAttempt, JudgmentDecision, ProposedAction,
    RepositoryReadAction, SemanticCheckpoint, SessionEvent, SessionId, SessionState, SessionStatus,
    TestResultSummary, TurnId, ValidationStatus, WhyIncluded,
};
use purrcode_validation_runtime::{
    EvidenceStatus, ValidationDetector, ValidationEvidence, ValidationPlan, ValidationRunner,
    ValidationStage,
};
use purrcode_whisker::RetrievalBudget;
use schemars::schema_for;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::Instant;
use tokio::sync::Notify;
use uuid::Uuid;

use crate::context::{
    AgentContextIndex, AgentContextPolicy, ContextualRequestInput, PlanRevision,
    bounded_terminal_text, build_contextual_request, build_messages, build_outcome_request,
    build_plan_messages, completed_outcome, session_worktree, task_related_paths,
};
use crate::errors::AgentError;
use crate::normalize::{
    apply_permission_mode, decision_constraints, normalize_action, requires_contextual_judgment,
    successful_duplicate_action,
};
use crate::schema::{
    AgentPlan, AgentTurn, completion_needs_repair_with_events, objective_requests_advice_only,
    validate_plan, validate_turn,
};
use crate::stream::{AgentStreamEvent, AgentStreamObserver, RationaleStreamExtractor};

const MAX_AUTONOMOUS_ITERATIONS: usize = 32;
const MAX_CONSECUTIVE_POLICY_REJECTIONS: usize = 3;
const MAX_ACTIONS_IN_PROMPT: usize = 12;
/// How many of the most recent proposed actions survive a compaction.
/// `pub(crate)` (re-exported from `lib.rs`, matching `compaction_window`) so
/// the daemon's manual `/compact` uses the exact same retention bound as
/// automatic compaction rather than a second, independently-drifting number.
pub(crate) const RETAINED_ACTIONS_AFTER_COMPACTION: usize = 6;
/// P0-10: Target token budget for the retained conversation window after
/// compaction — keep roughly the last 8K tokens of complete turns, aligned
/// at user-message boundaries. The window always includes the checkpoint,
/// which is accounted for separately. `pub(crate)` for the same reason as
/// [`RETAINED_ACTIONS_AFTER_COMPACTION`].
pub(crate) const COMPACTION_RETAINED_TOKEN_BUDGET: u64 = 8_192;
/// Maximum number of compaction cycles per `run_until_pause` iteration.
/// One preflight compaction is the norm; a second is only allowed as a
/// provider-overflow recovery (P0-5). More than that fails closed.
const MAX_COMPACTION_CYCLES_PER_ITERATION: u8 = 2;
/// When the prompt's estimated tokens exceed this fraction of the input budget,
/// compaction fires even if the action count is below `MAX_ACTIONS_IN_PROMPT`
/// (PRD v1.1 §7.3).
const COMPACTION_INPUT_TOKEN_THRESHOLD_RATIO: f64 = 0.7;
const MAX_REJECTED_RESPONSE_PREVIEW_CHARS: usize = 4_096;
const MAX_VALIDATION_REPAIR_CYCLES: usize = 3;
const MAX_COMPLETION_REPAIR_ATTEMPTS: usize = 2;
const MAX_ACTION_REPAIR_ATTEMPTS: usize = 1;
/// Bounded reject+repair opportunities when a Scout turn requests more actions
/// than the remaining exploration budget. One repair is enough for a compliant
/// model; repeating the overflow fails closed instead of truncating silently.
const MAX_SCOUT_TRUNCATE_REPAIRS: usize = 1;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub struct ScoutId(pub Uuid);

#[derive(Clone, Debug)]
pub struct ScoutRequest {
    pub scout_id: ScoutId,
    pub parent_turn_id: TurnId,
    pub objective: String,
    pub max_actions: u32,
    pub max_tokens: u64,
    pub allowed_action_kinds: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ScoutFinding {
    pub scout_id: ScoutId,
    pub evidence: Vec<EvidenceRef>,
    pub conclusions: Vec<String>,
    pub confidence: ScoutConfidence,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EvidenceRef {
    pub path: PathBuf,
    pub line_range: (u32, u32),
    pub excerpt: String,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum ScoutConfidence {
    High,
    Medium,
    Low,
}

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
    /// Per-role model routes: role → (provider, model). The daemon resolves
    /// these from `[models.roles]` so different stages (planner, coder, …) can
    /// use different providers. `coding_worker` is always present.
    models: BTreeMap<String, (Arc<dyn ModelProvider>, ModelId)>,
    /// P0: Cached context window from provider capabilities, resolved once
    /// before the first capacity decision. The outer `None` means not yet
    /// resolved; the inner `None` means the provider returned no
    /// context_window. `OnceLock` keeps `NativeAgent` `Send + Sync` across
    /// `.await` points (a `Cell` would not).
    cached_context_window: OnceLock<Option<usize>>,
    policy: Policy,
    controls: SessionControls,
    usage_ledger: Mutex<UsageLedger>,
    model_calls_started: AtomicU32,
    started_at: Instant,
    contextual_judge: Option<ContextualJudge<'a>>,
    stream_observer: Option<AgentStreamObserver>,
    cancellation: Option<AgentCancellation>,
}

impl<'a> NativeAgent<'a> {
    pub fn new(
        models: BTreeMap<String, (Arc<dyn ModelProvider>, ModelId)>,
        policy: Policy,
    ) -> Self {
        Self {
            models,
            cached_context_window: OnceLock::new(),
            policy,
            controls: SessionControls::default(),
            usage_ledger: Mutex::new(UsageLedger::default()),
            model_calls_started: AtomicU32::new(0),
            started_at: Instant::now(),
            contextual_judge: None,
            stream_observer: None,
            cancellation: None,
        }
    }

    /// The (provider, model) route for a role, falling back to the coding
    /// worker when the role is not configured.
    fn model_for(&self, role: &str) -> &(Arc<dyn ModelProvider>, ModelId) {
        self.models.get(role).unwrap_or_else(|| {
            self.models
                .get("coding_worker")
                .expect("coding_worker is required")
        })
    }

    /// The route's provider, for call sites that only need the provider.
    fn provider_for(&self, role: &str) -> &Arc<dyn ModelProvider> {
        &self.model_for(role).0
    }

    /// Apply the daemon-computed controls to this agent.  Controls are copied
    /// into the runtime so a client cannot change the effective policy while a
    /// request is in flight.
    pub fn with_controls(mut self, controls: SessionControls) -> Self {
        self.controls = controls;
        self
    }

    /// Seed the request ledger from durable session usage before resuming.
    /// This keeps model-call and token ceilings session-wide rather than
    /// resetting them every time the daemon starts a continuation operation.
    pub fn with_usage_records(self, records: Vec<UsageRecord>) -> Self {
        self.model_calls_started.store(
            records
                .iter()
                .filter(|record| record.search_requests == 0 && record.mcp_calls == 0)
                .count()
                .min(u32::MAX as usize) as u32,
            Ordering::Release,
        );
        if let Ok(mut ledger) = self.usage_ledger.lock() {
            *ledger = UsageLedger::from_records(records);
        }
        self
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

    fn budget(&self) -> BudgetConstraints {
        self.controls.effective_budget()
    }

    /// Effective input capacity for the current request: the minimum of the
    /// model's context window (minus output reserve) and the user's input-token
    /// budget. When neither is known, falls back to a generous default.
    fn effective_input_capacity(&self) -> u64 {
        self.model_capability_context_limit()
            .saturating_sub(purrcode_runtime_core::RESERVED_OUTPUT_TOKENS)
    }

    /// P0: Use the model's actual context window when available.
    /// The provider exposes capabilities per model; cached on first resolve.
    /// Falls back to the user budget, then a generous default.
    fn model_capability_context_limit(&self) -> u64 {
        const DEFAULT_CONTEXT_LIMIT: u64 = 200_000;
        // If provider capabilities have been resolved, use the actual window.
        if let Some(window) = self.cached_context_window.get().copied().flatten() {
            let budget_limit = self
                .budget()
                .maximum_input_tokens
                .unwrap_or(DEFAULT_CONTEXT_LIMIT);
            // Respect both: the model's actual limit and the user's budget.
            return (window as u64).min(budget_limit);
        }
        // Provider hasn't been queried yet — fall back to user budget or default.
        self.budget()
            .maximum_input_tokens
            .unwrap_or(DEFAULT_CONTEXT_LIMIT)
    }

    /// P0: Resolve the model's actual context window from provider capabilities.
    /// Called once during request preparation so all subsequent capacity
    /// decisions use the real limit. `OnceLock::get_or_init` is not used
    /// because the provider call is async; the set is idempotent under races
    /// (first write wins).
    async fn ensure_context_capacity(&self, role: &str) {
        if self.cached_context_window.get().is_some() {
            return;
        }
        let (provider, model) = self.model_for(role);
        let context_window = match provider.capabilities(model).await {
            Ok(caps) => caps.context_window,
            Err(_) => None,
        };
        let _ = self.cached_context_window.set(context_window);
    }

    /// P0: Wired provider context-overflow recovery. Called from the
    /// structured-response failure path when the provider returns
    /// ProviderErrorCategory::ContextTooLarge. Compacts once and rebuilds the
    /// request around the CURRENT objective (never the session's original
    /// objective), reloading durable state first so a second compaction never
    /// runs against a stale snapshot. When auto_context is disabled, retrieval
    /// is skipped exactly like the happy path.
    async fn recover_context_overflow(
        &self,
        store: &mut SessionStore,
        session_id: SessionId,
        current_objective: &str,
        turn_id: TurnId,
        compaction_cycles: u8,
    ) -> Result<(Vec<ModelMessage>, ContextLedgerEntry), AgentError> {
        if compaction_cycles >= MAX_COMPACTION_CYCLES_PER_ITERATION {
            return Err(AgentError::ContextOverflow {
                estimated_tokens: 0,
                max_input_tokens: self.effective_input_capacity(),
                after_compaction: true,
            });
        }
        // Compact the CURRENT durable state, never a caller-supplied snapshot
        // that predates this turn's events.
        let state = store.load(session_id)?;
        self.compact_and_checkpoint(store, session_id, &state, turn_id)
            .await?;
        let rebuilt_state = store.load(session_id)?;
        let session_events = store.events(session_id)?;
        let worktree = state
            .worktree
            .as_ref()
            .ok_or_else(|| AgentError::CorruptSession("worktree missing".into()))?;
        let mut context_hits = Vec::new();
        if self.controls.auto_context {
            let context_index =
                AgentContextIndex::open(worktree, &worktree.join(".purrcode").join("context.db"))?;
            context_hits =
                context_index.retrieve(current_objective, &RetrievalBudget::default())?;
        }
        let (messages, ledger) = build_messages(
            turn_id,
            session_id,
            current_objective,
            worktree,
            &rebuilt_state,
            &context_hits,
            &session_events,
        );
        Ok((messages, ledger))
    }

    /// P0-7: Decide whether the current objective should trigger a read-only
    /// Scout exploration before the main coding turn.
    ///
    /// Delegation is appropriate when:
    /// - The objective contains exploration markers ("find", "explore", etc.)
    /// - The session has no plan yet (no `plan_steps`)
    /// - A scout model is configured or `fast_router` is available
    fn should_delegate_to_scout(&self, objective: &str, state: &SessionState) -> bool {
        if !state.plan_steps.is_empty() {
            return false;
        }
        let has_scout_model =
            self.models.contains_key("scout") || self.models.contains_key("fast_router");
        if !has_scout_model {
            return false;
        }
        let lower = objective.to_ascii_lowercase();
        let exploration_markers = [
            "find ",
            "explore",
            "investigate",
            "understand",
            "how does",
            "what is",
            "where is",
            "locate",
            "search for",
            "look for",
            "audit",
            "trace",
            "map out",
            "diagram",
            "overview",
            "summarize the",
            "explain the",
            "describe the",
        ];
        exploration_markers
            .iter()
            .any(|marker| lower.contains(marker))
    }

    /// Convert Scout evidence into Whisker-compatible context hits for injection
    /// into the main turn's context window.
    fn scout_finding_to_hits(finding: &ScoutFinding) -> Vec<purrcode_whisker::ContextHit> {
        finding
            .evidence
            .iter()
            .map(|ev| purrcode_whisker::ContextHit {
                path: ev.path.clone(),
                start_line: ev.line_range.0 as usize,
                end_line: ev.line_range.1 as usize,
                content: ev.excerpt.clone(),
                score_millis: match finding.confidence {
                    ScoutConfidence::High => 900,
                    ScoutConfidence::Medium => 600,
                    ScoutConfidence::Low => 300,
                },
                sensitive: false,
                reason: purrcode_whisker::HitReason::MatchedQuery {
                    term: ev.path.to_string_lossy().to_string(),
                },
            })
            .collect()
    }

    fn remaining_wall_time(&self) -> Result<Option<std::time::Duration>, AgentError> {
        let Some(limit) = self.budget().maximum_wall_time_seconds else {
            return Ok(None);
        };
        let elapsed = self.started_at.elapsed();
        let limit = std::time::Duration::from_secs(limit);
        if elapsed >= limit {
            return Err(AgentError::InvalidModelTurn(format!(
                "agent wall-time budget exhausted after {limit:?}"
            )));
        }
        Ok(Some(limit - elapsed))
    }

    /// Reserve one model call and bound the request before it reaches a
    /// provider.  Input-token estimates are advisory unless the selected
    /// controls actually carry an input/total token ceiling; in that case a
    /// provider that cannot count tokens fails closed rather than pretending
    /// the budget is enforced.
    async fn prepare_model_request(
        &self,
        provider: &Arc<dyn ModelProvider>,
        mut request: ModelRequest,
    ) -> Result<(ModelRequest, Option<u64>), AgentError> {
        let budget = self.budget();
        let _ = self.remaining_wall_time()?;
        let started = self.model_calls_started.load(Ordering::Acquire);
        if budget
            .maximum_model_calls
            .is_some_and(|limit| started >= limit)
        {
            return Err(AgentError::InvalidModelTurn(
                "model-call budget exhausted".into(),
            ));
        }

        let estimate = match provider.count_tokens(&request).await {
            Ok(estimate) => Some(estimate.tokens),
            Err(error)
                if budget.maximum_input_tokens.is_some()
                    || budget.maximum_total_tokens.is_some() =>
            {
                return Err(AgentError::Provider(error));
            }
            Err(_) => None,
        };
        let completed = self
            .usage_ledger
            .lock()
            .map_err(|_| AgentError::CorruptSession("usage ledger lock poisoned".into()))?;
        let input_so_far = completed.input_tokens();
        let output_so_far = completed.output_tokens();
        let total_so_far = input_so_far.saturating_add(output_so_far);
        if budget.maximum_input_tokens.is_some_and(|limit| {
            estimate.is_some_and(|value| input_so_far.saturating_add(value) > limit)
        }) {
            return Err(AgentError::InvalidModelTurn(
                "input-token budget exhausted".into(),
            ));
        }
        let mut output_cap = request.max_output_tokens;
        if let Some(limit) = budget.maximum_output_tokens {
            let remaining = limit.saturating_sub(output_so_far);
            output_cap = Some(output_cap.unwrap_or(remaining).min(remaining));
        }
        if let Some(limit) = budget.maximum_total_tokens {
            let remaining =
                limit.saturating_sub(total_so_far.saturating_add(estimate.unwrap_or(0)));
            output_cap = Some(output_cap.unwrap_or(remaining).min(remaining));
        }
        if output_cap == Some(0) {
            return Err(AgentError::InvalidModelTurn(
                "output-token budget exhausted".into(),
            ));
        }
        drop(completed);
        request.max_output_tokens = output_cap;
        self.model_calls_started.fetch_add(1, Ordering::AcqRel);
        Ok((request, estimate))
    }

    fn append_usage(
        &self,
        store: &mut SessionStore,
        session_id: SessionId,
        role: &str,
        usage: Option<(u64, u64)>,
        estimated_input_tokens: Option<u64>,
        latency_ms: u64,
    ) -> Result<(), AgentError> {
        let (input_tokens, output_tokens) = usage
            .or_else(|| estimated_input_tokens.map(|input| (input, 0)))
            .unwrap_or((0, 0));
        let record = UsageRecord {
            request_id: Default::default(),
            session_id,
            workflow_lane_id: None,
            provider_id: self.model_for(role).1.provider.clone(),
            model_id: self.model_for(role).1.model.clone(),
            // The provider gateway deliberately keeps credentials out of the
            // agent.  This non-secret marker is truthful about that boundary.
            credential_id: "daemon-managed".into(),
            input_tokens,
            output_tokens,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            tool_result_tokens: 0,
            search_requests: 0,
            mcp_calls: 0,
            estimated_cost: None,
            latency_ms,
            recorded_at: Utc::now(),
        };
        let budget = self.budget();
        let over_budget = {
            let mut ledger = self
                .usage_ledger
                .lock()
                .map_err(|_| AgentError::CorruptSession("usage ledger lock poisoned".into()))?;
            // Calls are reserved before the provider request, so avoid
            // counting this completed record a second time for the call cap.
            let mut token_budget = budget.clone();
            token_budget.maximum_model_calls = None;
            let result = ledger.can_record(&token_budget, &record).err();
            // Preserve observed provider usage even when the provider crossed
            // a hard ceiling; future calls then fail closed from the durable
            // totals rather than silently resetting the counter.
            ledger.records.push(record.clone());
            result
        };
        store.append(
            session_id,
            &SessionEvent::UsageRecorded {
                record: record.clone(),
            },
        )?;
        if let Some(exhaustion) = over_budget {
            return Err(AgentError::InvalidModelTurn(format!(
                "{exhaustion} budget exhausted"
            )));
        }
        let _ = role;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    async fn structured_observed<T, V>(
        &self,
        store: &mut SessionStore,
        session_id: SessionId,
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
        self.structured_observed_from_tracker(
            store, session_id, role, attempt, tracker, request, schema, validate,
        )
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

    #[allow(clippy::too_many_arguments)]
    async fn structured_observed_from_tracker<T, V>(
        &self,
        store: &mut SessionStore,
        session_id: SessionId,
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
        let provider = self.provider_for(role);
        let (request, estimated_input_tokens) =
            self.prepare_model_request(provider, request).await?;
        let request_started = Instant::now();
        self.emit_stream_increment(&mut tracker, role, attempt, StreamIncrement::SendingRequest)
            .await?;

        let request_future = async {
            match &self.cancellation {
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
                            Err(AgentError::Cancelled(
                                "cancelled before provider response headers".into(),
                            ))
                        }
                        result = provider.structured_stream(request, schema) => Ok(result),
                    }
                }
                None => Ok(provider.structured_stream(request, schema).await),
            }
        };
        let stream_result = match self.remaining_wall_time()? {
            Some(remaining) => match tokio::time::timeout(remaining, request_future).await {
                Ok(result) => result?,
                Err(_) => {
                    let error = AgentError::InvalidModelTurn(
                        "agent wall-time budget exhausted before provider response".into(),
                    );
                    self.emit_agent_failure(&mut tracker, role, attempt, &error)
                        .await?;
                    return Err(error);
                }
            },
            None => request_future.await?,
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
            let item_future = async {
                match &self.cancellation {
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
                                Err(AgentError::Cancelled(
                                    "cancelled while receiving provider output".into(),
                                ))
                            }
                            item = stream.next() => Ok(item),
                        }
                    }
                    None => Ok(stream.next().await),
                }
            };
            let item = match self.remaining_wall_time()? {
                Some(remaining) => match tokio::time::timeout(remaining, item_future).await {
                    Ok(item) => item?,
                    Err(_) => {
                        let error = AgentError::InvalidModelTurn(
                            "agent wall-time budget exhausted while receiving provider output"
                                .into(),
                        );
                        self.emit_agent_failure(&mut tracker, role, attempt, &error)
                            .await?;
                        return Err(error);
                    }
                },
                None => item_future.await?,
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

        // Charge every provider call, including malformed responses that will
        // enter the bounded repair path.  Otherwise a model could evade the
        // call ceiling by repeatedly returning invalid JSON.
        let observed_usage = usage.or_else(|| {
            estimated_input_tokens.map(|input| {
                let output = output.chars().count().div_ceil(4) as u64;
                (input, output)
            })
        });
        self.append_usage(
            store,
            session_id,
            role,
            observed_usage,
            estimated_input_tokens,
            request_started.elapsed().as_millis().min(u64::MAX as u128) as u64,
        )?;

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
        Ok((parsed, observed_usage))
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
            // Never block on a slow live client: a full observer queue drops
            // the observation instead of stalling the agent loop (and its
            // lease) behind a receiver that is not draining.
            if !observer.try_send(event.clone()) {
                // A concurrently draining capacity-one client may still own
                // the slot from the immediately preceding phase. Give that
                // receiver one scheduling hand-off, then retry once. This is
                // still bounded and non-blocking when a client is abandoned,
                // while avoiding needless loss for an actively draining UI.
                tokio::task::yield_now().await;
                observer.try_send(event);
            }
            // `try_send` has no await point. Yield after each observation so
            // a concurrently draining receiver can make progress even when
            // the provider itself returns a ready stream of events. Without
            // this hand-off a producer can fill a capacity-one queue in one
            // poll, drop the terminal `Completed` phase, and leave a client
            // waiting forever for an event that was never delivered.
            tokio::task::yield_now().await;
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
                authority_mode: Default::default(),
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
        if state.worktree.is_some() || state.status != SessionStatus::Active {
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
        self.run_until_pause(store, session_id, None).await
    }

    pub async fn plan_initialized(
        &self,
        store: &mut SessionStore,
        session_id: SessionId,
    ) -> Result<AgentPlan, AgentError> {
        let state = store.load(session_id)?;
        if state.worktree.is_some() || state.status != SessionStatus::Active {
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
        let plan = self
            .run_planner(
                store,
                session_id,
                TurnId::new(),
                &objective,
                &worktree.path,
                None,
            )
            .await?;
        store.append(
            session_id,
            &SessionEvent::PlanCreated {
                steps: plan.steps.clone(),
            },
        )?;
        record_plan_work(store, session_id, &objective, &plan.steps, &[])?;
        store.append(
            session_id,
            &SessionEvent::SessionPaused {
                reason: purrcode_runtime_core::PLAN_REVIEW_PAUSE.into(),
            },
        )?;
        Ok(plan)
    }

    /// Fold a reviewer's feedback into the plan they are reading (PRD §11).
    ///
    /// A plan-only run pauses asking to be reviewed, and a review that can only
    /// answer yes is not a review. This produces the next revision from the
    /// existing worktree and pauses again, so the exchange can go back and
    /// forth as many times as the person needs before any file is touched.
    pub async fn revise_plan(
        &self,
        store: &mut SessionStore,
        session_id: SessionId,
        feedback: &str,
    ) -> Result<AgentPlan, AgentError> {
        let state = store.load(session_id)?;
        if !matches!(state.status, SessionStatus::Active | SessionStatus::Paused) {
            return Err(AgentError::SessionNotResumable(format!(
                "{:?}",
                state.status
            )));
        }
        if state.plan_steps.is_empty() {
            return Err(AgentError::CorruptSession(
                "this session has no plan to revise".into(),
            ));
        }
        if state.task_graph.as_ref().is_some_and(|graph| {
            graph
                .tasks
                .iter()
                .any(|task| !matches!(task.status, WorkTaskStatus::Pending | WorkTaskStatus::Ready))
        }) || !state.evidence_links.is_empty()
        {
            return Err(AgentError::SessionNotResumable(
                "plan revision requires an impact review after task execution has begun".into(),
            ));
        }
        let objective = state
            .objective
            .clone()
            .ok_or_else(|| AgentError::CorruptSession("objective is missing".into()))?;
        let worktree = state
            .worktree
            .clone()
            .ok_or_else(|| AgentError::CorruptSession("worktree is missing".into()))?;
        let plan = self
            .run_planner(
                store,
                session_id,
                TurnId::new(),
                &objective,
                &worktree,
                Some(PlanRevision {
                    current: &state.plan_steps,
                    feedback,
                }),
            )
            .await?;
        store.append(
            session_id,
            &SessionEvent::PlanRevised {
                revision: state.plan_revision + 1,
                // Why the plan changed is the reviewer's own words, bounded so
                // one pasted essay cannot dominate the durable log.
                reason: feedback.chars().take(512).collect(),
                steps: plan.steps.clone(),
            },
        )?;
        record_plan_work(store, session_id, &objective, &plan.steps, &[])?;
        store.append(
            session_id,
            &SessionEvent::SessionPaused {
                reason: format!("revised {}", purrcode_runtime_core::PLAN_REVIEW_PAUSE),
            },
        )?;
        Ok(plan)
    }

    /// Index, retrieve and ask the planner for one structured plan.
    ///
    /// Shared by the first plan and every revision: the two differ only in the
    /// event that records the result, so letting them differ in how they gather
    /// context or repair a malformed response would be a second planner nobody
    /// maintains.
    async fn run_planner(
        &self,
        store: &mut SessionStore,
        session_id: SessionId,
        // Threaded per PRD v1.1 §6.3 for correlation with the caller's turn.
        // `run_planner` does not itself emit any ProposedAction/
        // ActionOutputRecorded/JudgmentRecorded event to stamp — it issues
        // exactly one retrieve() call and one structured model request — so
        // this is currently unread; it is reserved for `run_scout`'s
        // `parent_turn_id` (Phase 5) and any future planner-side ledger entry.
        _turn_id: TurnId,
        objective: &str,
        worktree: &Path,
        revision: Option<PlanRevision<'_>>,
    ) -> Result<AgentPlan, AgentError> {
        let tracker = self.begin_stream_observation("planner", 1).await?;
        // Feedback usually names code the objective never mentioned, so it
        // joins the retrieval query — otherwise a revision reasons about the
        // same context that produced the plan being complained about.
        let query = match &revision {
            Some(revision) => format!("{objective}\n{}", revision.feedback),
            None => objective.to_owned(),
        };
        let database = worktree.join(".purrcode").join("context.db");
        let mut context_index = AgentContextIndex::open(worktree, &database)?;
        let indexed = context_index.submit_task(&query, &[], &AgentContextPolicy::default())?;
        store.append(
            session_id,
            &SessionEvent::ContextIndexed {
                files: indexed.summary.indexed_files,
                symbols: indexed.summary.symbols,
                sensitive_files: indexed.summary.sensitive_files,
            },
        )?;
        let hits = context_index.retrieve(&query, &RetrievalBudget::default())?;
        let planner = self.model_for("planner");
        store.append(
            session_id,
            &SessionEvent::ModelRequestStarted {
                role: "planner".into(),
                provider: planner.1.provider.clone(),
                model: planner.1.model.clone(),
            },
        )?;
        let request = ModelRequest {
            model: planner.1.clone(),
            messages: build_plan_messages(objective, worktree, &hits, revision),
            tools: Vec::new(),
            max_output_tokens: Some(4096),
            reasoning_effort: None,
        };
        let first = self
            .structured_observed_from_tracker(
                store,
                session_id,
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
                    store,
                    session_id,
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
        Ok(plan)
    }

    /// Run a read-only scout subagent that explores the repository and returns
    /// structured findings (PRD v1.1 Phase 5).
    ///
    /// The scout opens its own context index, retrieves relevant files, and
    /// executes bounded typed-read actions through PawGate + Claw — it does
    /// NOT bypass authorization.
    pub async fn run_scout(
        &self,
        store: &mut SessionStore,
        session_id: SessionId,
        request: ScoutRequest,
    ) -> Result<ScoutFinding, AgentError> {
        let state = store.load(session_id)?;
        let worktree = state
            .worktree
            .clone()
            .ok_or_else(|| AgentError::CorruptSession("worktree is missing".into()))?;
        let role = if self.models.contains_key("scout") {
            "scout"
        } else {
            "fast_router"
        };
        let database = worktree.join(".purrcode").join("context.db");
        let mut context_index = AgentContextIndex::open(&worktree, &database)?;
        let _indexed =
            context_index.submit_task(&request.objective, &[], &AgentContextPolicy::default())?;
        let context_hits =
            context_index.retrieve(&request.objective, &RetrievalBudget::default())?;
        let mut messages =
            crate::context::build_scout_messages(&request.objective, &worktree, &context_hits);
        let mut token_used: u64 = 0;
        let mut action_count: u32 = 0;
        let mut evidence: Vec<EvidenceRef> = Vec::new();
        let mut conclusions: Vec<String> = Vec::new();
        let mut scout_truncate_repairs = 0_usize;

        let model = self.model_for(role).1.clone();

        loop {
            if action_count >= request.max_actions || token_used >= request.max_tokens {
                conclusions.push(format!(
                    "scout halted at limit: {action_count} actions, {token_used} tokens used"
                ));
                break;
            }

            let scout_request = ModelRequest {
                model: model.clone(),
                messages: messages.clone(),
                tools: Vec::new(),
                max_output_tokens: Some(4096),
                reasoning_effort: None,
            };

            let (turn, usage) = self
                .structured_observed(
                    store,
                    session_id,
                    role,
                    1,
                    scout_request,
                    schema_for!(crate::schema::AgentTurn),
                    crate::schema::validate_turn,
                )
                .await?;

            let (input_tokens, output_tokens) = usage.unwrap_or((0, 0));
            token_used = token_used.saturating_add(input_tokens.saturating_add(output_tokens));

            if turn.complete {
                conclusions.push(turn.rationale.clone());
                // Append the final turn as an assistant message so the scout's
                // complete conversation is coherent (P0-5).
                messages.push(ModelMessage {
                    role: "assistant".into(),
                    content: format!(
                        "{{\"rationale\":{},\"complete\":true}}",
                        serde_json::to_string(&turn.rationale).unwrap_or_default()
                    ),
                });
                break;
            }

            // P0-6: Support multi-read actions[] in scout.
            let actions: Vec<crate::schema::AgentAction> = if !turn.actions.is_empty() {
                turn.actions.clone()
            } else {
                let action = turn.action.clone().ok_or_else(|| {
                    AgentError::InvalidModelTurn("scout action is required".into())
                })?;
                vec![action]
            };

            // P0: Enforce Scout max_actions before execution, not just at
            // loop boundary. A model turn that returns a batch larger than the
            // remaining budget is rejected for a bounded repair — the model
            // chooses the highest-value subset — instead of silently dropping
            // its actions.
            let remaining = request.max_actions.saturating_sub(action_count) as usize;
            if actions.len() > remaining {
                if scout_truncate_repairs >= MAX_SCOUT_TRUNCATE_REPAIRS {
                    conclusions.push(format!(
                        "scout halted: model requested {} actions but only {remaining} remain in budget",
                        actions.len()
                    ));
                    break;
                }
                scout_truncate_repairs += 1;
                messages.push(ModelMessage {
                    role: "user".into(),
                    content: format!(
                        "Your previous turn returned {} actions, but only {remaining} exploration \
                        actions remain in budget. Return the SAME incomplete turn with at most \
                        {remaining} highest-value read actions (no repeats). Do not mark complete.",
                        actions.len()
                    ),
                });
                continue;
            }

            // P0-5: Append the model's turn to messages so the scout sees its
            // own reasoning in the conversation history.
            let action_descriptions: Vec<String> =
                actions.iter().map(|a| format!("{a:?}")).collect();
            messages.push(ModelMessage {
                role: "assistant".into(),
                content: format!(
                    "{{\"rationale\":{},\"actions\":{},\"complete\":false}}",
                    serde_json::to_string(&turn.rationale).unwrap_or_default(),
                    serde_json::to_string(&action_descriptions).unwrap_or_default()
                ),
            });

            let mut action_results = Vec::new();
            for action in actions {
                let proposed = crate::normalize::normalize_action(action, &worktree)?;
                // Verify the action is read-only per scout's allowed kinds.
                if !matches!(&proposed, ProposedAction::RepositoryRead(_)) {
                    return Err(AgentError::InvalidModelTurn(
                        "scout may only issue typed read actions".into(),
                    ));
                }

                // Route through PawGate before execution.
                let deterministic = self.policy.evaluate(&proposed, &worktree);
                let constraints = crate::normalize::decision_constraints(&deterministic)
                    .ok_or_else(|| {
                        AgentError::InvalidModelTurn("scout action was denied by PawGate".into())
                    })?;

                let session_worktree = crate::context::session_worktree(&state)?;
                let action_id = ActionId::new();
                store.append(
                    session_id,
                    &SessionEvent::ActionProposed {
                        action_id,
                        action: proposed.clone(),
                        turn_id: Some(request.parent_turn_id),
                    },
                )?;
                // P0: Scout must record durable JudgmentRecorded, same as the
                // main coding-worker path, so the audit trail is complete.
                store.append(
                    session_id,
                    &SessionEvent::JudgmentRecorded {
                        action_id,
                        decision: deterministic.clone(),
                        turn_id: Some(request.parent_turn_id),
                    },
                )?;

                let authorization = Authorization {
                    action_id,
                    session_id,
                    action_digest: proposed.digest(constraints)?,
                    constraints: constraints.clone(),
                    authorized_at: Utc::now(),
                    approved_by: ApprovalAuthority::DeterministicPolicy,
                };
                store.authorize(&authorization)?;

                let execution = execute_and_record(
                    store,
                    session_id,
                    action_id,
                    &proposed,
                    constraints,
                    &session_worktree,
                    Some(request.parent_turn_id),
                )
                .await?;

                // P0: Structured per-action evidence. Grep output is
                // `path:line:content`, find/list output is one path per line —
                // split those into per-file EvidenceRefs with real line ranges
                // instead of stamping the whole stdout onto every searched
                // directory. read_file/git_show know their exact path.
                let stdout_str = String::from_utf8_lossy(&execution.stdout).to_string();
                let mut action_evidence: Vec<EvidenceRef> = Vec::new();
                if let ProposedAction::RepositoryRead(ref read) = proposed {
                    let kind = read_evidence_kind(read);
                    match kind {
                        "grep" | "find" | "list" => {
                            action_evidence.extend(per_file_evidence(
                                kind,
                                &stdout_str,
                                &read_paths(read),
                                16,
                            ));
                        }
                        _ => {
                            let line_count = stdout_str.lines().count().max(1) as u32;
                            for path in read_paths(read) {
                                action_evidence.push(EvidenceRef {
                                    path,
                                    line_range: (1, line_count),
                                    excerpt: stdout_str.chars().take(2048).collect(),
                                });
                            }
                        }
                    }
                }
                if action_evidence.is_empty() {
                    // Parsed output was empty or unparseable — fall back to the
                    // execution result's affected paths.
                    let line_count = stdout_str.lines().count().max(1) as u32;
                    for path in &execution.affected_paths {
                        action_evidence.push(EvidenceRef {
                            path: path.clone(),
                            line_range: (1, line_count),
                            excerpt: stdout_str.chars().take(2048).collect(),
                        });
                    }
                }
                evidence.extend(action_evidence);
                action_results.push(format!(
                    "Action result ({}):\n{}",
                    action_id.0,
                    bounded_terminal_text(&execution.stdout),
                ));
                action_count += 1;
            }

            // P0-5: Feed action results back into messages so the scout sees
            // the outcome of its reads before deciding the next step.
            messages.push(ModelMessage {
                role: "user".into(),
                content: action_results.join("\n---\n"),
            });
        }

        // P0: Confidence reflects evidence quality, not how chatty the
        // conclusions are. High requires at least two distinct files with
        // specific (non-whole-file) line ranges; Medium requires any
        // repository-backed evidence.
        let confidence = if evidence.is_empty() {
            ScoutConfidence::Low
        } else {
            let distinct_files: std::collections::BTreeSet<&PathBuf> =
                evidence.iter().map(|e| &e.path).collect();
            let specific_lines = evidence.iter().any(|e| {
                e.line_range.0 > 1 || (e.line_range.1 != 1 && e.line_range.0 != e.line_range.1)
            });
            if distinct_files.len() >= 2 && specific_lines {
                ScoutConfidence::High
            } else {
                ScoutConfidence::Medium
            }
        };

        Ok(ScoutFinding {
            scout_id: request.scout_id,
            evidence,
            conclusions,
            confidence,
        })
    }

    pub async fn resume(
        &self,
        store: &mut SessionStore,
        session_id: SessionId,
    ) -> Result<AgentOutcome, AgentError> {
        let state = store.load(session_id)?;
        match state.status {
            SessionStatus::Active => self.run_until_pause(store, session_id, None).await,
            SessionStatus::AwaitingApproval(action_id) => {
                let action = state.proposed_actions.get(&action_id).cloned().ok_or(
                    AgentError::CorruptSession("pending action is missing".into()),
                )?;
                let reason = match state.judgments.get(&action_id) {
                    Some(JudgmentDecision::RequireApproval { reason, .. }) => reason.clone(),
                    _ => {
                        return Err(AgentError::CorruptSession(
                            "pending approval judgment is missing".into(),
                        ));
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

    /// Answer a follow-up message using the user's own words.
    ///
    /// The message travels with the call rather than being re-read from the
    /// tail of the conversation, so the agent answers exactly the follow-up it
    /// was given — never silently re-running the original objective. This
    /// mirrors the `RevisePlan` guarantee for an ordinary follow-up.
    ///
    /// A worktree-less session (a greeting, a read-only "explain this
    /// codebase") is initialized lazily here and then answers the follow-up in
    /// the same turn. The lazy initialization mirrors `start_initialized`, but
    /// ends by entering the autonomous loop with the follow-up, not the session
    /// objective. Sessions that already have a worktree simply enter the loop
    /// with the follow-up.
    pub async fn continue_turn(
        &self,
        store: &mut SessionStore,
        session_id: SessionId,
        message: &str,
    ) -> Result<AgentOutcome, AgentError> {
        let state = store.load(session_id)?;
        match state.status {
            SessionStatus::Active
            | SessionStatus::Paused
            | SessionStatus::Completed
            | SessionStatus::Failed
            | SessionStatus::Cancelled => {}
            SessionStatus::AwaitingApproval(_) => {
                return Err(AgentError::SessionNotResumable(
                    "a follow-up cannot be answered while an action awaits approval".into(),
                ));
            }
            SessionStatus::AwaitingReview => {
                return Err(AgentError::SessionNotResumable(
                    "a follow-up cannot be answered while an outcome review is pending".into(),
                ));
            }
            SessionStatus::Executing(_) | SessionStatus::Uncertain => {
                return Err(AgentError::SessionNotResumable(format!(
                    "session is {:?}",
                    state.status
                )));
            }
        }
        if state.worktree.is_none() {
            let repository = state
                .repository
                .clone()
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
        }
        self.run_until_pause(store, session_id, Some(message)).await
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
                ));
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
        let task_id = state
            .task_graph
            .as_ref()
            .and_then(|graph| {
                graph
                    .tasks
                    .iter()
                    .rev()
                    .find(|task| task.status == WorkTaskStatus::Running)
                    .or_else(|| {
                        graph.tasks.iter().find(|task| {
                            matches!(
                                task.status,
                                WorkTaskStatus::Ready | WorkTaskStatus::NeedsAttention
                            )
                        })
                    })
            })
            .map(|task| task.id);
        start_task(
            store,
            session_id,
            task_id,
            "human approval started the current plan task",
        )?;
        let result = execute_and_record(
            store,
            session_id,
            action_id,
            &action,
            &constraints,
            &worktree,
            // Approval happens outside `run_until_pause`'s loop (a human may
            // approve long after the action was proposed), so there is no
            // "current" turn to stamp this execution with.
            None,
        )
        .await;
        let result = match result {
            Ok(result) => {
                let validation = if result.exit_code == Some(0) {
                    ValidationStatus::Passed
                } else {
                    ValidationStatus::Failed
                };
                append_task_evidence(
                    store,
                    session_id,
                    task_id,
                    Some(action_id),
                    validation.clone(),
                    &format!("approved agent action exited {:?}", result.exit_code),
                    false,
                )?;
                if validation != ValidationStatus::Passed {
                    append_task_status(
                        store,
                        session_id,
                        task_id,
                        WorkTaskStatus::NeedsAttention,
                        "approved agent action produced failing evidence; repair may retry",
                    )?;
                }
                result
            }
            Err(error) => {
                append_task_evidence(
                    store,
                    session_id,
                    task_id,
                    Some(action_id),
                    ValidationStatus::Failed,
                    &error.to_string(),
                    false,
                )?;
                append_task_status(
                    store,
                    session_id,
                    task_id,
                    WorkTaskStatus::NeedsAttention,
                    "approved agent action failed; repair may retry",
                )?;
                return Err(error);
            }
        };
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
        let task_id = state.task_graph.as_ref().and_then(|graph| {
            graph
                .tasks
                .iter()
                .rev()
                .find(|task| task.status == WorkTaskStatus::Running)
                .map(|task| task.id)
        });
        append_task_status(
            store,
            session_id,
            task_id,
            WorkTaskStatus::NeedsAttention,
            "the proposed action was rejected; revise this task before retrying",
        )?;
        Ok(())
    }

    /// Inject runtime messages (daemon contract, step-limit warning) into the
    /// final ModelRequest and update the context ledger to match. Internally
    /// recomputes insertion positions so callers never pass a stale index —
    /// critical for correctness after compaction rebuilds messages.
    fn inject_runtime_messages(
        mut messages: Vec<ModelMessage>,
        mut ledger: ContextLedgerEntry,
        contract: &ModelMessage,
        step_limit_warning: Option<&ModelMessage>,
    ) -> (Vec<ModelMessage>, ContextLedgerEntry) {
        // Same cumulative-ceiling accounting build_messages() uses: each
        // injected section's estimated_tokens is the *increment* in
        // ceil(running_chars / 4), not an independent ceil(section_chars /
        // 4). Adding independent ceilings here (as before) could drift the
        // ledger's total_estimated_tokens away from ceil(total_chars / 4) by
        // a token or two, breaking the PRD v1.1 §6.5/§14.1 invariant that
        // the ledger sum exactly equals the final request estimate.
        let mut cumulative_chars: u64 = messages
            .iter()
            .map(|message| message.content.chars().count() as u64)
            .sum();

        // 1. Insert contract before last user message
        let idx = messages
            .iter()
            .rposition(|m| m.role == "user")
            .unwrap_or(messages.len());
        messages.insert(idx, contract.clone());
        cumulative_chars += contract.content.chars().count() as u64;
        let running_total = cumulative_chars.div_ceil(4);
        let contract_tokens = running_total.saturating_sub(ledger.total_estimated_tokens);
        ledger.total_estimated_tokens = running_total;
        ledger.sections.push(ContextLedgerSection {
            class: ContextClass::Instructions,
            label: "daemon_contract".into(),
            estimated_tokens: contract_tokens,
            byte_len: contract.content.len(),
            why_included: WhyIncluded::AlwaysPresent,
        });

        // 2. Insert warning after last user message (recompute idx!)
        if let Some(warning) = step_limit_warning {
            let idx = messages
                .iter()
                .rposition(|m| m.role == "user")
                .unwrap_or(messages.len());
            messages.insert(idx + 1, warning.clone());
            cumulative_chars += warning.content.chars().count() as u64;
            let running_total = cumulative_chars.div_ceil(4);
            let warning_tokens = running_total.saturating_sub(ledger.total_estimated_tokens);
            ledger.total_estimated_tokens = running_total;
            ledger.sections.push(ContextLedgerSection {
                class: ContextClass::Instructions,
                label: "step_limit_warning".into(),
                estimated_tokens: warning_tokens,
                byte_len: warning.content.len(),
                why_included: WhyIncluded::AlwaysPresent,
            });
        }

        (messages, ledger)
    }

    async fn run_until_pause(
        &self,
        store: &mut SessionStore,
        session_id: SessionId,
        follow_up: Option<&str>,
    ) -> Result<AgentOutcome, AgentError> {
        let mut consecutive_policy_rejections = 0_usize;
        let mut validation_repair_cycles = 0_usize;
        let mut focused_repair_stages = BTreeSet::<ValidationStage>::new();
        let mut iteration = 0_usize;
        for _ in 0..MAX_AUTONOMOUS_ITERATIONS {
            iteration += 1;
            let mut state = store.load(session_id)?;
            // P0-9: TurnId originates at daemon user-message admission
            // (ConversationMessage.turn_id now set to Some(…) in the daemon).
            // Read it from the most recent user message so all actions,
            // judgments, and assistant messages in this turn share the same id.
            // Falls back to a new TurnId when replaying sessions recorded
            // before P0-9 shipped (turn_id was None).
            let turn_id = state
                .conversation_messages
                .iter()
                .rev()
                .find(|m| m.role.eq_ignore_ascii_case("user"))
                .and_then(|m| m.turn_id)
                .unwrap_or_else(TurnId::new);
            // ── Preflight compaction (P0-1/P0-2) ─────────────────────────
            // The action-count guard (hard cap) is checked first — it is
            // cheap and does not need a round-trip estimate. The token guard
            // is evaluated AFTER `build_messages()` assembles the current
            // request so it operates on live context, not a stale ledger.
            // P0: Resolve the model's actual context window from provider
            // capabilities before any capacity decision is made, so the
            // token-pressure preflight uses the real limit, not a fallback.
            self.ensure_context_capacity("coding_worker").await;
            let mut compaction_cycles: u8 = 0;
            loop {
                if state.proposed_actions.len() > MAX_ACTIONS_IN_PROMPT {
                    self.compact_and_checkpoint(store, session_id, &state, turn_id)
                        .await?;
                    compaction_cycles += 1;
                    if compaction_cycles >= MAX_COMPACTION_CYCLES_PER_ITERATION {
                        return Err(AgentError::ContextOverflow {
                            estimated_tokens: 0,
                            max_input_tokens: self.effective_input_capacity(),
                            after_compaction: true,
                        });
                    }
                    state = store.load(session_id)?;
                    continue;
                }
                break;
            }
            // The token-pressure guard is evaluated later, inside the
            // per-iteration flow right after `build_messages()` returns the
            // current request's ledger (see below).
            let worktree = state
                .worktree
                .clone()
                .ok_or_else(|| AgentError::CorruptSession("worktree is missing".into()))?;
            let session_worktree = session_worktree(&state)?;
            // The session objective names the conversation, not every turn in
            // it. A follow-up must retrieve and reason against the latest user
            // message; otherwise a session created with "hello" keeps asking
            // the agent to solve "hello" forever. The follow-up text travels
            // with the operation rather than being re-read from the tail, so
            // this branch never silently re-runs the original objective.
            let objective = follow_up
                .map(str::to_owned)
                .or_else(|| {
                    state
                        .conversation_messages
                        .iter()
                        .rev()
                        .find(|message| message.role.eq_ignore_ascii_case("user"))
                        .map(|message| message.content.clone())
                })
                .or_else(|| state.objective.clone())
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
            let mut context_hits =
                context_index.retrieve(&objective, &RetrievalBudget::default())?;
            // P1-12: When auto_context is disabled, skip both Whisker retrieval
            // AND Scout delegation — the user has opted out of automatic context.
            if !self.controls.auto_context {
                context_hits.clear();
            }
            // ── P0-7: Scout delegation ──────────────────────────────────
            let mut scout_context_hits: Vec<purrcode_whisker::ContextHit> = Vec::new();
            if iteration == 1
                && self.controls.auto_context
                && self.should_delegate_to_scout(&objective, &state)
            {
                let scout_request = ScoutRequest {
                    scout_id: ScoutId(Uuid::new_v4()),
                    parent_turn_id: turn_id,
                    objective: objective.clone(),
                    max_actions: 8,
                    max_tokens: 32_768,
                    allowed_action_kinds: vec!["read".into()],
                };
                match self.run_scout(store, session_id, scout_request).await {
                    Ok(finding) => {
                        store.append(
                            session_id,
                            &SessionEvent::ScoutCompleted {
                                scout_id: finding.scout_id.0.to_string(),
                                parent_turn_id: turn_id,
                                evidence_count: finding.evidence.len() as u32,
                                conclusions: finding.conclusions.clone(),
                                confidence: format!("{:?}", finding.confidence),
                            },
                        )?;
                        scout_context_hits = Self::scout_finding_to_hits(&finding);
                    }
                    Err(scout_error) => {
                        store.append(
                            session_id,
                            &SessionEvent::ScoutFailed {
                                reason: scout_error.to_string(),
                            },
                        )?;
                    }
                }
            }
            // Merge scout hits into the context hits for the main turn.
            if !scout_context_hits.is_empty() {
                context_hits = scout_context_hits.into_iter().chain(context_hits).collect();
            }
            store.append(
                session_id,
                &SessionEvent::ModelRequestStarted {
                    role: "coding_worker".into(),
                    provider: self.model_for("coding_worker").1.provider.clone(),
                    model: self.model_for("coding_worker").1.model.clone(),
                },
            )?;
            let session_events = store.events(session_id)?;
            let (mut messages, mut context_ledger_entry) = build_messages(
                turn_id,
                session_id,
                &objective,
                &worktree,
                &state,
                &context_hits,
                &session_events,
            );
            // ── P0: Preflight the FINAL ModelRequest ─────────────────
            // Move contract + warning injection BEFORE compaction so the
            // preflight check operates on exactly what the model will see.
            let contract_content = format!(
                "EFFECTIVE DAEMON CONTRACT (fixed for this request): mode={}; execution_style={}; workflow={}; search={}. Read-only modes may only return repository reads. Standard and Ultra workflows must provide a durable plan before proposing a mutation. Collaborative execution pauses after planning and after each completed action.",
                self.controls.task_mode,
                self.controls.execution_style,
                self.controls.workflow.label(),
                self.controls.effective_search_policy(
                    state
                        .workflow_plan
                        .as_ref()
                        .map(|plan| plan.profile)
                        .or_else(|| self.controls.workflow.forced_profile())
                        .unwrap_or(purrcode_runtime_core::adaptation::WorkflowProfile::Direct)
                )
            );
            let contract = ModelMessage {
                role: "system".into(),
                content: contract_content,
            };

            let step_limit_warning: Option<ModelMessage> =
                if iteration >= MAX_AUTONOMOUS_ITERATIONS - 1 {
                    Some(ModelMessage {
                        role: "user".into(),
                        content:
                            "STEP LIMIT: This is your last action before the autonomous loop ends. \
                If the objective is satisfied, summarize what was accomplished with \
                `complete: true`. If work remains, describe what is incomplete and set \
                `complete: true` with a clear handoff for a human to resume. \
                Do NOT issue another tool call — produce your closing turn NOW."
                                .into(),
                    })
                } else {
                    None
                };

            // P0: Use inject_runtime_messages helper so both the normal path
            // and the compaction-rebuilt path share the same index-recomputation
            // and ledger-update logic — no stale user_idx, no ledger drift.
            let warning_ref = step_limit_warning.as_ref();
            (messages, context_ledger_entry) = Self::inject_runtime_messages(
                messages,
                context_ledger_entry,
                &contract,
                warning_ref,
            );

            // ── Token-pressure preflight (P0-1/P0-2) on FINAL request ─
            let messages_char_count: usize =
                messages.iter().map(|m| m.content.chars().count()).sum();
            let estimated_tokens = (messages_char_count as u64).div_ceil(4);
            let capacity = self.effective_input_capacity();
            if estimated_tokens as f64 > capacity as f64 * COMPACTION_INPUT_TOKEN_THRESHOLD_RATIO
                && compaction_cycles == 0
            {
                self.compact_and_checkpoint(store, session_id, &state, turn_id)
                    .await?;
                compaction_cycles += 1;
                // Reload state, rebuild with contract+warning, re-estimate.
                let rebuilt_state = store.load(session_id)?;
                let rebuilt = build_messages(
                    turn_id,
                    session_id,
                    &objective,
                    &worktree,
                    &rebuilt_state,
                    &context_hits,
                    &session_events,
                );
                let (rebuilt_msgs, rebuilt_ledger) = rebuilt;
                // P0: Re-inject contract+warning with freshly computed
                // indices via inject_runtime_messages — no stale user_idx,
                // and the rebuilt ledger includes both sections.
                let reb_warning_ref = step_limit_warning.as_ref();
                let (rebuilt_msgs, rebuilt_ledger) = Self::inject_runtime_messages(
                    rebuilt_msgs,
                    rebuilt_ledger,
                    &contract,
                    reb_warning_ref,
                );
                // P0: Re-estimate after compaction — fail if still too large.
                let rebuilt_char_count: usize =
                    rebuilt_msgs.iter().map(|m| m.content.chars().count()).sum();
                let rebuilt_est = (rebuilt_char_count as u64).div_ceil(4);
                if rebuilt_est > capacity {
                    return Err(AgentError::ContextOverflow {
                        estimated_tokens: rebuilt_est,
                        max_input_tokens: capacity,
                        after_compaction: true,
                    });
                }
                messages = rebuilt_msgs;
                context_ledger_entry = rebuilt_ledger;
            }
            // Record the ledger AFTER all message injections so it reflects
            // the FINAL ModelRequest (P0-3).
            store.append(
                session_id,
                &SessionEvent::ContextAssembled {
                    entry: context_ledger_entry.clone(),
                },
            )?;

            let mut active_request = ModelRequest {
                model: self.model_for("coding_worker").1.clone(),
                messages,
                tools: Vec::new(),
                max_output_tokens: Some(4096),
                reasoning_effort: None,
            };
            let first = self
                .structured_observed_from_tracker(
                    store,
                    session_id,
                    "coding_worker",
                    1,
                    tracker,
                    active_request.clone(),
                    schema_for!(AgentTurn),
                    validate_turn,
                )
                .await;
            let (mut turn, mut usage) = match first {
                Ok(result) => result,
                Err(first_error) if first_error.is_cancelled() => return Err(first_error),
                Err(first_error) => {
                    // P0: If the provider rejected the request because context
                    // is too large, trigger compaction+rebuild instead of
                    // pushing a repair message that would make it worse.
                    if first_error.is_context_too_large()
                        && compaction_cycles < MAX_COMPACTION_CYCLES_PER_ITERATION
                    {
                        let (rebuilt_msgs, rebuilt_ledger) = self
                            .recover_context_overflow(
                                store,
                                session_id,
                                &objective,
                                turn_id,
                                compaction_cycles,
                            )
                            .await?;
                        // Re-inject contract and warning into rebuilt messages
                        let reb_w = step_limit_warning.as_ref();
                        let (rebuilt_msgs, rebuilt_ledger) = Self::inject_runtime_messages(
                            rebuilt_msgs,
                            rebuilt_ledger,
                            &contract,
                            reb_w,
                        );
                        // Local preflight of the REBUILT final request: if
                        // compaction still cannot fit within the effective
                        // capacity, fail closed instead of sending a retry
                        // that will overflow again.
                        let rebuilt_char_count: usize =
                            rebuilt_msgs.iter().map(|m| m.content.chars().count()).sum();
                        let rebuilt_est = (rebuilt_char_count as u64).div_ceil(4);
                        if rebuilt_est > capacity {
                            return Err(AgentError::ContextOverflow {
                                estimated_tokens: rebuilt_est,
                                max_input_tokens: capacity,
                                after_compaction: true,
                            });
                        }
                        context_ledger_entry = rebuilt_ledger;
                        store.append(
                            session_id,
                            &SessionEvent::ContextAssembled {
                                entry: context_ledger_entry.clone(),
                            },
                        )?;
                        // Swap the active request to the compacted one so every
                        // subsequent repair (schema/completion/action) rebases
                        // on the rebuilt messages, not the oversized original.
                        active_request = ModelRequest {
                            model: self.model_for("coding_worker").1.clone(),
                            messages: rebuilt_msgs,
                            tools: Vec::new(),
                            max_output_tokens: Some(4096),
                            reasoning_effort: None,
                        };
                        self.structured_observed(
                            store,
                            session_id,
                            "coding_worker",
                            1,
                            active_request.clone(),
                            schema_for!(AgentTurn),
                            validate_turn,
                        )
                        .await?
                    } else {
                        let mut repair = active_request.clone();
                        repair.messages.push(ModelMessage {
                            role: "user".into(),
                            content: format!(
                                "Your previous response was rejected: {first_error}\n\n\
                                Return EXACTLY ONE corrected response matching the JSON schema. \
                                No markdown, no extra text outside the JSON. \
                                This is the only schema repair attempt."
                            ),
                        });
                        self.structured_observed(
                            store,
                            session_id,
                            "coding_worker",
                            2,
                            repair,
                            schema_for!(AgentTurn),
                            validate_turn,
                        )
                        .await?
                    }
                }
            };

            // A provider can satisfy the JSON schema while still returning a
            // progress note such as "I have gathered enough evidence" with
            // `complete=true`. That is not a user-facing answer. Give it a
            // bounded semantic repair opportunity with escalating severity;
            // if the provider repeats the meta-completion, fail closed instead
            // of recording a misleading completed turn or spinning indefinitely.
            let mut completion_repair_attempts = 0_usize;
            loop {
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
                if !turn.complete
                    || !completion_needs_repair_with_events(
                        &objective,
                        &turn.rationale,
                        &session_events,
                    )
                {
                    break;
                }
                if completion_repair_attempts >= MAX_COMPLETION_REPAIR_ATTEMPTS {
                    return Err(AgentError::InvalidModelTurn(
                        "provider repeatedly claimed completion without a concrete user-facing answer".into(),
                    ));
                }
                completion_repair_attempts += 1;
                // Record every repair durably so the meta-completion failure
                // rate is measurable rather than anecdotal (PRD §2.3 FR-B5).
                store.append(
                    session_id,
                    &SessionEvent::CompletionRepairRecorded {
                        attempt: completion_repair_attempts as u8,
                        reason: format!(
                            "complete=true rationale failed evidence grounding: {}",
                            turn.rationale.chars().take(160).collect::<String>()
                        ),
                    },
                )?;
                let mut repair = active_request.clone();
                // Escalating severity: first attempt is a polite nudge, second
                // is a direct command to stop meta-commentary and produce output.
                let repair_prompt = if completion_repair_attempts == 1 {
                    format!(
                        "STOP. Your previous response marked complete=true but only contained a \
                        progress report — it said you are ready to answer, without actually \
                        answering. This is a meta-response, not a user-facing answer.\n\n\
                        You MUST produce the REAL answer now. If you have enough evidence, \
                        write the complete concrete explanation/fix/result with `complete: true`. \
                        If you genuinely need more repository evidence, set `complete: false` \
                        and provide exactly one typed read action.\n\n\
                        Do NOT say you are ready. Do NOT describe what you will do. Either DO it \
                        (complete the answer) or READ more (one typed action).\n\n\
                        Repair attempt {completion_repair_attempts} of {MAX_COMPLETION_REPAIR_ATTEMPTS}."
                    )
                } else {
                    format!(
                        "FINAL WARNING — Your previous two responses both marked complete=true \
                        without delivering the actual answer. This is the last repair opportunity.\n\n\
                        The objective is: {objective}\n\n\
                        You MUST respond with EXACTLY ONE of:\n\
                        1. `complete: true` with `rationale` containing the FULL user-facing answer \
                        (concrete findings, real code, real explanations — NOT a progress note), OR\n\
                        2. `complete: false` with exactly one typed read action to gather missing evidence.\n\n\
                        Any other response — including readiness statements, meta-commentary, or \
                        promises of future action — will cause the session to fail.\n\n\
                        Repair attempt {completion_repair_attempts} of {MAX_COMPLETION_REPAIR_ATTEMPTS}."
                    )
                };
                repair.messages.push(ModelMessage {
                    role: "user".into(),
                    content: repair_prompt,
                });
                store.append(
                    session_id,
                    &SessionEvent::ModelRequestStarted {
                        role: "coding_worker".into(),
                        provider: self.model_for("coding_worker").1.provider.clone(),
                        model: self.model_for("coding_worker").1.model.clone(),
                    },
                )?;
                let repaired = self
                    .structured_observed(
                        store,
                        session_id,
                        "coding_worker",
                        (completion_repair_attempts + 2).min(u8::MAX as usize) as u8,
                        repair,
                        schema_for!(AgentTurn),
                        validate_turn,
                    )
                    .await?;
                turn = repaired.0;
                usage = repaired.1;
            }

            // Path confinement and action bounds are semantic checks rather
            // than JSON-schema checks. Providers occasionally use an absolute
            // slash for "repository root" or otherwise produce a malformed
            // path. Keep the boundary strict, but give the provider one
            // bounded opportunity to express the same action with a safe,
            // repository-relative path instead of failing the whole session.
            let mut proposed_action = None;
            if turn.actions.len() > 1 {
                // ── Batch (multi-read) path ──────────────────────────────
                // Normalize every action, then propose, judge, authorize, and
                // execute them as a batch through execute_batch.
                // Only multi-action batches enter here; single actions in
                // actions[] route through the scalar path below so mutations
                // flow through ToolRuntime::execute() instead of execute_batch().
                let mut normalized = Vec::with_capacity(turn.actions.len());
                for a in &turn.actions {
                    normalized.push(normalize_action(a.clone(), &worktree)?);
                }
                let mut action_ids = Vec::with_capacity(turn.actions.len());
                // P0-4: Store each action's PawGate constraints individually
                // instead of sharing a single hardcoded read_only().
                let mut authorized_reads: Vec<(ActionId, ProposedAction, ActionConstraints)> =
                    Vec::with_capacity(turn.actions.len());
                for action in &normalized {
                    let action_id = ActionId::new();
                    action_ids.push(action_id);
                    store.append(
                        session_id,
                        &SessionEvent::ActionProposed {
                            action_id,
                            action: action.clone(),
                            turn_id: Some(turn_id),
                        },
                    )?;
                    let deterministic = self.policy.evaluate(action, &worktree);
                    store.append(
                        session_id,
                        &SessionEvent::JudgmentRecorded {
                            action_id,
                            decision: deterministic.clone(),
                            turn_id: Some(turn_id),
                        },
                    )?;
                    match deterministic {
                        JudgmentDecision::AllowWithConstraints(constraints) => {
                            authorized_reads.push((action_id, action.clone(), constraints.clone()));
                        }
                        _ => {
                            return Ok(AgentOutcome::IterationLimit { session_id });
                        }
                    }
                }
                // Each action is authorized with its own PawGate constraints (P0-4).
                for (action_id, action, constraints) in &authorized_reads {
                    let digest = action.digest(constraints)?;
                    store.authorize(&Authorization {
                        action_id: *action_id,
                        session_id,
                        action_digest: digest,
                        constraints: constraints.clone(),
                        authorized_at: Utc::now(),
                        approved_by: ApprovalAuthority::DeterministicPolicy,
                    })?;
                }
                // P0: Concurrent batch execution with per-action constraints.
                // Each action carries its own PawGate-approved constraints and
                // authorization. execute_batch consumes authorizations serially
                // then executes all reads concurrently.
                let batch: Vec<purrcode_claw::AuthorizedRead> = authorized_reads
                    .iter()
                    .map(|(id, action, constraints)| purrcode_claw::AuthorizedRead {
                        action_id: *id,
                        action: action.clone(),
                        constraints: constraints.clone(),
                    })
                    .collect();
                let results = purrcode_claw::ToolRuntime::execute_batch(store, &batch).await?;
                for (i, result) in results.iter().enumerate() {
                    if let Some((action_id, _, _)) = authorized_reads.get(i) {
                        store.append(
                            session_id,
                            &SessionEvent::ActionOutputRecorded {
                                action_id: *action_id,
                                stdout: bounded_terminal_text(&result.stdout),
                                stderr: bounded_terminal_text(&result.stderr),
                                truncated: result.truncated,
                                turn_id: Some(turn_id),
                            },
                        )?;
                    }
                }
                if self.controls.execution_style.pauses_between_stages() {
                    store.append(
                        session_id,
                        &SessionEvent::SessionPaused {
                            reason: "Collaborative execution paused after the batch read; review its results, then resume to continue".into(),
                        },
                    )?;
                    return Ok(AgentOutcome::IterationLimit { session_id });
                }
                continue;
            }
            // Single action in unified actions[] schema — route through the
            // scalar execution path (ToolRuntime::execute) which handles all
            // action types: read, write, delete, command.
            if !turn.complete {
                let mut action_repair_attempts = 0_usize;
                loop {
                    // Read action from the CURRENT turn (after repair, turn
                    // is updated to the model's corrected response).
                    let action = if turn.actions.len() == 1 {
                        turn.actions.first().cloned()
                    } else {
                        turn.action.clone()
                    }
                    .ok_or_else(|| AgentError::InvalidModelTurn("action is required".into()))?;
                    match normalize_action(action, &worktree) {
                        Ok(action) => {
                            proposed_action = Some(action);
                            break;
                        }
                        Err(error) if action_repair_attempts < MAX_ACTION_REPAIR_ATTEMPTS => {
                            action_repair_attempts += 1;
                            let mut repair = active_request.clone();
                            repair.messages.push(ModelMessage {
                                role: "user".into(),
                                content: format!(
                                    "REJECTED: {error}\n\n\
                                    Fix the action and return the SAME incomplete turn with exactly \
                                    one corrected action. Repository paths must be relative to the \
                                    worktree root. Use `.` (or an empty paths list where the action \
                                    supports its default root) for the repository root. Never use an \
                                    absolute path or a path that escapes through `..`. \
                                    This is the only action repair attempt."
                                ),
                            });
                            store.append(
                                session_id,
                                &SessionEvent::ModelRequestStarted {
                                    role: "coding_worker".into(),
                                    provider: self.model_for("coding_worker").1.provider.clone(),
                                    model: self.model_for("coding_worker").1.model.clone(),
                                },
                            )?;
                            let (repaired_turn, repaired_usage) = self
                                .structured_observed(
                                    store,
                                    session_id,
                                    "coding_worker",
                                    (completion_repair_attempts + action_repair_attempts + 2)
                                        .min(u8::MAX as usize)
                                        as u8,
                                    repair,
                                    schema_for!(AgentTurn),
                                    validate_turn,
                                )
                                .await?;
                            let (input_tokens, output_tokens) = repaired_usage
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
                            if repaired_turn.complete {
                                return Err(AgentError::InvalidModelTurn(
                                    "action repair unexpectedly marked the task complete".into(),
                                ));
                            }
                            turn = repaired_turn;
                        }
                        Err(error) => return Err(error),
                    }
                }
            }

            // Intermediate action rationales are execution trace, not user
            // conversation. They remain available through durable model/action
            // events and the live stream, while only a validated final turn is
            // promoted into the transcript shown as the assistant answer.
            if turn.complete {
                // FR-B6: for an advice-only objective the final turn is the
                // answer itself. The "Proposed plan:" steps are appended only
                // when the rationale already passes the completion gate; a
                // rationale that is itself a progress report must not be
                // rescued into an answer by bolting the plan onto it.
                let grounded = !completion_needs_repair_with_events(
                    &objective,
                    &turn.rationale,
                    &session_events,
                );
                let assistant_content = if grounded
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
                            model: Some(format!(
                                "{}/{}",
                                self.model_for("coding_worker").1.provider,
                                self.model_for("coding_worker").1.model
                            )),
                            turn_id: Some(turn_id),
                        },
                    },
                )?;
            }
            let mut plan_changed = false;
            if let Some(plan) = turn.plan.clone() {
                if state.plan_steps.is_empty() {
                    store.append(session_id, &SessionEvent::PlanCreated { steps: plan })?;
                    plan_changed = true;
                    if matches!(
                        self.controls.task_mode,
                        purrcode_runtime_core::adaptation::TaskMode::Plan
                            | purrcode_runtime_core::adaptation::TaskMode::Build
                    ) {
                        let current = store.load(session_id)?;
                        record_plan_work(
                            store,
                            session_id,
                            &objective,
                            &current.plan_steps,
                            &turn.expected_postconditions,
                        )?;
                    }
                } else if state.plan_steps != plan {
                    let material_work_exists = state.task_graph.as_ref().is_some_and(|graph| {
                        graph.tasks.iter().any(|task| {
                            !matches!(task.status, WorkTaskStatus::Pending | WorkTaskStatus::Ready)
                        })
                    }) || !state.evidence_links.is_empty();
                    if material_work_exists {
                        store.append(
                            session_id,
                            &SessionEvent::SessionPaused {
                                reason: "the model proposed a material plan revision after task execution began; review and explicitly revise instead of silently replacing tasks or evidence".into(),
                            },
                        )?;
                        return Ok(AgentOutcome::IterationLimit { session_id });
                    }
                    store.append(
                        session_id,
                        &SessionEvent::PlanRevised {
                            revision: state.plan_revision + 1,
                            reason: turn.rationale.clone(),
                            steps: plan,
                        },
                    )?;
                    plan_changed = true;
                    if matches!(
                        self.controls.task_mode,
                        purrcode_runtime_core::adaptation::TaskMode::Plan
                            | purrcode_runtime_core::adaptation::TaskMode::Build
                    ) {
                        let current = store.load(session_id)?;
                        record_plan_work(
                            store,
                            session_id,
                            &objective,
                            &current.plan_steps,
                            &turn.expected_postconditions,
                        )?;
                    }
                }
            }
            if plan_changed
                && self.controls.task_mode == purrcode_runtime_core::adaptation::TaskMode::Build
                && self.controls.execution_style.pauses_between_stages()
            {
                store.append(
                    session_id,
                    &SessionEvent::SessionPaused {
                        reason: "Collaborative execution paused after planning; review the durable tasks, then resume to begin implementation".into(),
                    },
                )?;
                return Ok(AgentOutcome::IterationLimit { session_id });
            }
            if turn.complete {
                if self.controls.task_mode.read_only() || objective_requests_advice_only(&objective)
                {
                    store.append(session_id, &SessionEvent::SessionCompleted)?;
                    return completed_outcome(store, session_id);
                }
                let validation = ValidationDetector::detect(&worktree)?;
                if !focused_repair_stages.is_empty() {
                    let focused = ValidationPlan {
                        commands: validation
                            .commands
                            .iter()
                            .filter(|command| focused_repair_stages.contains(&command.stage))
                            .cloned()
                            .collect(),
                        undetected_stages: Vec::new(),
                        required_stages: focused_repair_stages.clone(),
                        accepted_unavailable_stages: BTreeSet::new(),
                    };
                    let focused_report =
                        ValidationRunner::run(store, session_id, &worktree, &focused).await?;
                    if !focused_report.completion_allowed() {
                        validation_repair_cycles += 1;
                        focused_repair_stages = repair_stages(&focused_report);
                        if validation_repair_cycles < MAX_VALIDATION_REPAIR_CYCLES {
                            continue;
                        }
                        return pause_after_validation_budget(store, session_id, &focused_report);
                    }
                    focused_repair_stages.clear();
                }
                let report =
                    ValidationRunner::run(store, session_id, &worktree, &validation).await?;
                if report.completion_allowed() {
                    close_validated_plan_tasks(store, session_id, &report)?;
                    if !required_tasks_passed(store, session_id)? {
                        store.append(
                            session_id,
                            &SessionEvent::SessionPaused {
                                reason: "final validation passed but one or more required plan tasks never reached a running/evidenced state".into(),
                            },
                        )?;
                        return Ok(AgentOutcome::IterationLimit { session_id });
                    }
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
                validation_repair_cycles += 1;
                focused_repair_stages = repair_stages(&report);
                if validation_repair_cycles < MAX_VALIDATION_REPAIR_CYCLES {
                    continue;
                }
                return pause_after_validation_budget(store, session_id, &report);
            }
            let proposed = proposed_action.ok_or_else(|| {
                AgentError::InvalidModelTurn("incomplete turn did not produce an action".into())
            })?;
            if matches!(
                self.controls.workflow,
                purrcode_runtime_core::adaptation::WorkflowControl::Standard
                    | purrcode_runtime_core::adaptation::WorkflowControl::Ultra
            ) && state.plan_steps.is_empty()
            {
                store.append(
                    session_id,
                    &SessionEvent::SessionPaused {
                        reason: format!(
                            "{} workflow requires a durable plan before the first mutation; the model omitted it",
                            self.controls.workflow.label()
                        ),
                    },
                )?;
                return Ok(AgentOutcome::IterationLimit { session_id });
            }
            if self.controls.task_mode.read_only()
                && !matches!(&proposed, ProposedAction::RepositoryRead(_))
            {
                store.append(
                    session_id,
                    &SessionEvent::SessionPaused {
                        reason: format!(
                            "{} mode refused a mutating action; start an explicit Build task to change files",
                            self.controls.task_mode
                        ),
                    },
                )?;
                return Ok(AgentOutcome::IterationLimit { session_id });
            }
            let action_id = ActionId::new();
            store.append(
                session_id,
                &SessionEvent::ActionProposed {
                    action_id,
                    action: proposed.clone(),
                    turn_id: Some(turn_id),
                },
            )?;
            let deterministic = self.policy.evaluate(&proposed, &worktree);
            store.append(
                session_id,
                &SessionEvent::JudgmentRecorded {
                    action_id,
                    decision: deterministic.clone(),
                    turn_id: Some(turn_id),
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
                            turn_id: Some(turn_id),
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
                                turn_id: Some(turn_id),
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
                                turn_id: Some(turn_id),
                            },
                        )?;
                        effective
                    }
                }
            } else {
                deterministic
            };
            // The session's permission mode is the human's standing decision
            // for this run. Auto (the default) lets governed actions through
            // without a prompt; FullAccess additionally overrides even a deny.
            // Both still write a durable authorization record below, so the
            // action remains auditable (AGENTS.md). Ask leaves PawGate's
            // decision untouched.
            let decision = apply_permission_mode(
                self.controls.permission_mode,
                decision,
                &session_worktree.path,
            );
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
                    let task_id = current_plan_task_id(store, session_id, turn.current_step_index)?;
                    start_task(
                        store,
                        session_id,
                        task_id,
                        "native-agent started the current plan task",
                    )?;
                    let execution = execute_and_record(
                        store,
                        session_id,
                        action_id,
                        &proposed,
                        &constraints,
                        &session_worktree,
                        Some(turn_id),
                    )
                    .await;
                    match execution {
                        Ok(result) => {
                            let validation = if result.exit_code == Some(0) {
                                ValidationStatus::Passed
                            } else {
                                ValidationStatus::Failed
                            };
                            append_task_evidence(
                                store,
                                session_id,
                                task_id,
                                Some(action_id),
                                validation.clone(),
                                &format!("agent action exited {:?}", result.exit_code),
                                false,
                            )?;
                            if validation != ValidationStatus::Passed {
                                append_task_status(
                                    store,
                                    session_id,
                                    task_id,
                                    WorkTaskStatus::NeedsAttention,
                                    "native-agent action produced failing evidence; repair may retry",
                                )?;
                            }
                        }
                        Err(error) => {
                            append_task_evidence(
                                store,
                                session_id,
                                task_id,
                                Some(action_id),
                                ValidationStatus::Failed,
                                &error.to_string(),
                                false,
                            )?;
                            append_task_status(
                                store,
                                session_id,
                                task_id,
                                WorkTaskStatus::NeedsAttention,
                                "native-agent action failed; repair may retry",
                            )?;
                            return Err(error);
                        }
                    }
                    if self.controls.execution_style.pauses_between_stages() {
                        store.append(
                            session_id,
                            &SessionEvent::SessionPaused {
                                reason: "Collaborative execution paused after the current action; review its task evidence, then resume to continue".into(),
                            },
                        )?;
                        return Ok(AgentOutcome::IterationLimit { session_id });
                    }
                }
                JudgmentDecision::RequireApproval {
                    reason,
                    constraints: _,
                } => {
                    let task_id = current_plan_task_id(store, session_id, turn.current_step_index)?;
                    start_task(
                        store,
                        session_id,
                        task_id,
                        "current plan task is waiting for human approval",
                    )?;
                    return Ok(AgentOutcome::AwaitingApproval {
                        session_id,
                        action_id,
                        reason,
                        action: proposed,
                    });
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
                    "agent reached its bounded action limit (32 iterations); review progress before explicitly resuming"
                        .into(),
            },
        )?;
        Ok(AgentOutcome::IterationLimit { session_id })
    }

    // ── Compaction helpers (P0-1/P0-2/P0-10) ────────────────────────
    /// Perform one compaction cycle: prune actions, build a semantic checkpoint,
    /// and record durable events. Called from `run_until_pause`'s preflight guard
    /// so that compaction never repeats against a stale ledger.
    async fn compact_and_checkpoint(
        &self,
        store: &mut SessionStore,
        session_id: SessionId,
        state: &SessionState,
        turn_id: TurnId,
    ) -> Result<(), AgentError> {
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
                    Some(JudgmentDecision::Allow | JudgmentDecision::AllowWithConstraints(_))
                )
            })
            .count();
        store.append(
            session_id,
            &SessionEvent::ContextCompacted {
                summary: format!(
                    "Compacted {compacted} older actions. Across the pre-compaction window, {successful} actions had allow-class deterministic/effective judgments. The durable event log remains authoritative."
                ),
                retained_action_ids: retained_action_ids.clone(),
            },
        )?;
        let events = store.events(session_id)?;
        let checkpoint = build_semantic_checkpoint(
            state,
            &events,
            &self.controls,
            turn_id,
            &retained_action_ids,
        );
        // P0-10: Token-based conversation retention instead of fixed message
        // count. Walk backwards through conversation_messages and stop at the
        // last user-message turn boundary within the token budget.
        let conversation_messages_retained_from = compaction_window(
            &state.conversation_messages,
            COMPACTION_RETAINED_TOKEN_BUDGET,
        );
        let retained: BTreeSet<ActionId> = retained_action_ids.iter().copied().collect();
        store.append(
            session_id,
            &SessionEvent::CheckpointCompacted {
                checkpoint: Box::new(checkpoint),
                retained_action_ids: retained,
                conversation_messages_retained_from,
            },
        )?;
        Ok(())
    }
}

/// Build a `SemanticCheckpoint` from durable session state.
///
/// This is the single implementation both automatic context-pressure
/// compaction (`NativeAgent::compact_and_checkpoint` above) and the daemon's
/// manual `POST /v1/sessions/{id}/compact` endpoint call, so a manual
/// compaction can never construct an empty checkpoint that silently discards
/// accumulated semantic memory (PRD v1.1 §7.3) — the two paths cannot drift
/// apart because there is only one path. A pure function of its inputs, with
/// no `SessionStore`/`NativeAgent` access, so the daemon can call it directly
/// without spinning up an agent just to compact.
///
/// `retained_action_ids` is a parameter rather than recomputed internally:
/// every caller already needs the same slice for other purposes (the
/// `ContextCompacted` summary event, and the `CheckpointCompacted` event's
/// own `retained_action_ids` field), and recomputing it here would either
/// duplicate that work or risk silently diverging from it.
pub(crate) fn build_semantic_checkpoint(
    state: &SessionState,
    events: &[SessionEvent],
    controls: &SessionControls,
    turn_id: TurnId,
    retained_action_ids: &[ActionId],
) -> SemanticCheckpoint {
    let objective = state.objective.clone().unwrap_or_default();
    let files_inspected: Vec<PathBuf> = state
        .proposed_actions
        .values()
        .filter_map(|action| match action {
            ProposedAction::RepositoryRead(read) => match read {
                RepositoryReadAction::ReadFile { path, .. } => Some(path.clone()),
                RepositoryReadAction::GitShow { path, .. } => Some(path.clone()),
                RepositoryReadAction::GitDiff { paths }
                | RepositoryReadAction::List { paths, .. }
                | RepositoryReadAction::Find { paths, .. }
                | RepositoryReadAction::RepositoryGrep { paths, .. } => paths.first().cloned(),
                RepositoryReadAction::GitLsFiles { pathspec } => pathspec.first().cloned(),
                _ => None,
            },
            _ => None,
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let files_modified: Vec<PathBuf> = state
        .proposed_actions
        .values()
        .filter_map(|action| match action {
            ProposedAction::WriteFile(write) => Some(write.path.clone()),
            ProposedAction::DeleteFile(delete) => Some(delete.path.clone()),
            _ => None,
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    // P1-3: Derive execution/validation failures from the durable event log
    // so policy-allowed actions that failed at runtime still enter
    // failed_attempts (a judgment-only filter misses them entirely).
    let mut runtime_failures: BTreeMap<ActionId, String> = BTreeMap::new();
    for event in events {
        match event {
            SessionEvent::ExecutionFinished {
                action_id,
                exit_code,
                ..
            } if *exit_code != Some(0) => {
                runtime_failures.insert(
                    *action_id,
                    format!("execution exited with code {exit_code:?}"),
                );
            }
            SessionEvent::ValidationRecorded {
                action_id,
                status: ValidationStatus::Failed,
                evidence,
            } => {
                runtime_failures.insert(
                    *action_id,
                    format!(
                        "validation failed: {}",
                        evidence.chars().take(200).collect::<String>()
                    ),
                );
            }
            _ => {}
        }
    }
    // P1-3: Derive per-stage test results from the same durable event log.
    // Each ValidationRecorded carries a serialized ValidationEvidence with a
    // concrete stage; group pass/fail/skip counts by stage label so the
    // checkpoint's test_results reflect what actually ran instead of being
    // perpetually empty.
    let mut test_counts: BTreeMap<String, [u32; 3]> = BTreeMap::new(); // [passed, failed, skipped]
    for event in events {
        let SessionEvent::ValidationRecorded {
            status, evidence, ..
        } = event
        else {
            continue;
        };
        let parsed = serde_json::from_str::<ValidationEvidence>(evidence).ok();
        let Some(parsed) = parsed else {
            continue;
        };
        if parsed.stage == ValidationStage::CompletionCriteria {
            continue;
        }
        let label = format!("{stage:?}", stage = parsed.stage);
        let counts = test_counts.entry(label).or_insert([0, 0, 0]);
        match status {
            ValidationStatus::Passed => counts[0] += 1,
            ValidationStatus::Failed | ValidationStatus::TimedOut | ValidationStatus::Uncertain => {
                counts[1] += 1
            }
            ValidationStatus::SkippedByConfiguration
            | ValidationStatus::NotDetected
            | ValidationStatus::Unavailable => counts[2] += 1,
        }
    }
    let test_results: Vec<TestResultSummary> = test_counts
        .into_iter()
        .map(|(label, [passed, failed, skipped])| TestResultSummary {
            label,
            passed,
            failed,
            skipped,
        })
        .collect();
    let failed_attempts: Vec<FailedAttempt> = state
        .proposed_actions
        .iter()
        .filter(|(id, _)| !retained_action_ids.contains(id))
        .filter(|(id, _)| {
            let judgment = state.judgments.get(id);
            let is_allow = matches!(
                judgment,
                Some(JudgmentDecision::Allow | JudgmentDecision::AllowWithConstraints(_))
            );
            // Policy denial, OR policy approval that failed at runtime.
            !is_allow || runtime_failures.contains_key(id)
        })
        .map(|(id, action)| {
            // Prefer the concrete runtime failure; fall back to the policy
            // judgment when the action never executed.
            let reason = if let Some(failure) = runtime_failures.get(id) {
                failure.clone()
            } else if let Some(judgment) = state.judgments.get(id) {
                match judgment {
                    JudgmentDecision::Deny { reason }
                    | JudgmentDecision::ModifyAction { reason }
                    | JudgmentDecision::Replan { reason } => reason.clone(),
                    JudgmentDecision::Allow | JudgmentDecision::AllowWithConstraints(_) => {
                        "execution or validation failed after policy approval".into()
                    }
                    JudgmentDecision::RequireApproval { reason, .. } => {
                        format!("required approval but was compacted before resolution: {reason}")
                    }
                }
            } else {
                "judgment missing from state".into()
            };
            FailedAttempt {
                action_id: *id,
                action_summary: format!("{action:?}"),
                reason,
                judgment: state.judgments.get(id).map(|j| format!("{j:?}")),
            }
        })
        .collect();
    let decisions: Vec<CheckpointDecision> = state
        .proposed_actions
        .iter()
        .filter(|(id, _)| retained_action_ids.contains(id))
        .map(|(id, action)| CheckpointDecision {
            summary: format!("{action:?}"),
            action_id: Some(*id),
        })
        .collect();
    SemanticCheckpoint {
        checkpoint_id: CheckpointId::new(),
        turn_id,
        superseded_checkpoint_id: state.checkpoint.as_ref().map(|c| c.checkpoint_id),
        objective,
        // P1-2: Populate from available session state.
        accepted_requirements: state
            .plan_steps
            .iter()
            .map(|s| format!("planned: {s}"))
            .collect(),
        user_constraints: {
            let mut constraints = Vec::new();
            constraints.push(format!("task_mode={}", controls.task_mode));
            constraints.push(format!("execution_style={:?}", controls.execution_style));
            constraints.push(format!("workflow={}", controls.workflow.label()));
            if let Some(ref plan) = state.workflow_plan {
                constraints.push(format!("search_policy={:?}", plan.search_policy));
            }
            constraints
        },
        decisions,
        files_inspected,
        files_modified,
        important_symbols: state
            .proposed_actions
            .values()
            .filter_map(|action| match action {
                ProposedAction::RepositoryRead(read) => match read {
                    RepositoryReadAction::ReadFile { path, .. } => {
                        Some(path.file_name()?.to_string_lossy().to_string())
                    }
                    RepositoryReadAction::GitShow { path, .. } if !path.as_os_str().is_empty() => {
                        Some(path.file_name()?.to_string_lossy().to_string())
                    }
                    _ => None,
                },
                _ => None,
            })
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect(),
        validated_facts: state
            .evidence_links
            .iter()
            .filter(|link| {
                matches!(
                    link.coverage,
                    purrcode_runtime_core::work::EvidenceCoverage::Covered
                )
            })
            .map(|link| format!("{:?} covered by task {}", link.criterion_id, link.task_id.0))
            .collect(),
        failed_attempts,
        test_results,
        unresolved_questions: state
            .task_graph
            .as_ref()
            .map(|graph| {
                graph
                    .tasks
                    .iter()
                    .filter(|t| {
                        matches!(
                            t.status,
                            WorkTaskStatus::NeedsAttention | WorkTaskStatus::Pending
                        )
                    })
                    .map(|t| format!("task[{}]: {}", t.id.0, t.objective))
                    .collect()
            })
            .unwrap_or_default(),
        current_hypothesis: state
            .conversation_messages
            .iter()
            .rev()
            .find(|m| m.role.eq_ignore_ascii_case("assistant"))
            .map(|m| m.content.chars().take(512).collect()),
        next_actions: state
            .task_graph
            .as_ref()
            .map(|graph| {
                graph
                    .tasks
                    .iter()
                    .filter(|t| matches!(t.status, WorkTaskStatus::Ready | WorkTaskStatus::Pending))
                    .take(8)
                    .map(|t| format!("task[{}]: {}", t.id.0, t.objective))
                    .collect()
            })
            .unwrap_or_else(|| state.plan_steps.iter().take(8).cloned().collect()),
        pinned_context: vec![],
    }
}

/// P1-10: Compute the index into `conversation_messages` where the retained
/// window should start. Walks backwards from the most recent message,
/// accumulating estimated tokens, and stops at the last **user** message
/// boundary before the budget is exceeded. Returns `0` (keep everything) when
/// the total is within budget.
pub(crate) fn compaction_window(messages: &[ConversationMessage], max_tokens: u64) -> usize {
    if messages.is_empty() {
        return 0;
    }
    let mut accumulated = 0u64;
    let mut last_user_boundary = 0usize;
    for (i, msg) in messages.iter().enumerate().rev() {
        let token_est = (msg.content.chars().count() as u64).div_ceil(4);
        if accumulated + token_est > max_tokens {
            return if last_user_boundary > 0 {
                last_user_boundary
            } else {
                i.saturating_add(1).min(messages.len())
            };
        }
        accumulated += token_est;
        if msg.role.eq_ignore_ascii_case("user") {
            last_user_boundary = i;
        }
    }
    0
}

/// Extract paths touched by a typed repository read action.
fn read_paths(read: &purrcode_runtime_core::RepositoryReadAction) -> Vec<PathBuf> {
    match read {
        purrcode_runtime_core::RepositoryReadAction::ReadFile { path, .. } => {
            vec![path.clone()]
        }
        purrcode_runtime_core::RepositoryReadAction::List { paths, .. }
        | purrcode_runtime_core::RepositoryReadAction::Find { paths, .. }
        | purrcode_runtime_core::RepositoryReadAction::RepositoryGrep { paths, .. }
        | purrcode_runtime_core::RepositoryReadAction::GitDiff { paths }
        | purrcode_runtime_core::RepositoryReadAction::GitLsFiles { pathspec: paths } => {
            if paths.is_empty() {
                vec![PathBuf::from(".")]
            } else {
                paths.clone()
            }
        }
        purrcode_runtime_core::RepositoryReadAction::GitShow { path, .. } => {
            vec![path.clone()]
        }
        purrcode_runtime_core::RepositoryReadAction::GitStatus
        | purrcode_runtime_core::RepositoryReadAction::GitLog { .. }
        | purrcode_runtime_core::RepositoryReadAction::GitRevParse { .. } => {
            vec![PathBuf::from(".")]
        }
    }
}

/// Classify a typed read action's stdout shape for evidence extraction.
fn read_evidence_kind(read: &purrcode_runtime_core::RepositoryReadAction) -> &'static str {
    match read {
        purrcode_runtime_core::RepositoryReadAction::RepositoryGrep { .. } => "grep",
        purrcode_runtime_core::RepositoryReadAction::Find { .. } => "find",
        purrcode_runtime_core::RepositoryReadAction::List { .. } => "list",
        purrcode_runtime_core::RepositoryReadAction::GitLsFiles { .. } => "list",
        _ => "other",
    }
}

/// Split multi-file line-oriented output (grep's `path:line:content`, find's
/// `path` entries) into per-file [`EvidenceRef`]s with real line ranges, instead
/// of stamping the entire stdout onto every searched directory.
/// Extract the path column from one `list` action output line, formatted by
/// the sandbox as `"{file_type} {size:>8} {name}"` (e.g. `"d      4096
/// src"`). Splitting on the first two whitespace-delimited fields — rather
/// than trimming the whole line — keeps the type/size columns out of the
/// evidence path regardless of how much padding the size field carries.
fn strip_first_whitespace_field(input: &str) -> &str {
    input
        .trim_start()
        .split_once(char::is_whitespace)
        .map(|(_, rest)| rest)
        .unwrap_or("")
}

fn list_entry_path(line: &str) -> &str {
    strip_first_whitespace_field(strip_first_whitespace_field(line)).trim_start()
}

fn per_file_evidence(
    kind: &str,
    stdout: &str,
    fallback_paths: &[PathBuf],
    max_lines: usize,
) -> Vec<EvidenceRef> {
    let mut evidence: Vec<EvidenceRef> = Vec::new();
    if kind == "grep" {
        let mut per_path: BTreeMap<PathBuf, Vec<(u32, String)>> = BTreeMap::new();
        for line in stdout.lines() {
            let mut split = line.splitn(3, ':');
            let Some(path) = split.next() else {
                continue;
            };
            let Some(line_no) = split.next().and_then(|n| n.trim().parse::<u32>().ok()) else {
                continue;
            };
            let content = split.next().unwrap_or("").trim().to_owned();
            per_path
                .entry(PathBuf::from(path))
                .or_default()
                .push((line_no, content));
        }
        for (path, matches) in per_path {
            let (first, last) = (
                matches.first().map(|m| m.0).unwrap_or(1),
                matches.last().map(|m| m.0).unwrap_or(1),
            );
            let excerpt = matches
                .iter()
                .take(max_lines)
                .map(|(n, content)| format!("{n}:{content}"))
                .collect::<Vec<_>>()
                .join("\n");
            evidence.push(EvidenceRef {
                path,
                line_range: (first, last),
                excerpt,
            });
        }
    } else if kind == "find" || kind == "list" {
        let mut seen = std::collections::BTreeSet::new();
        for line in stdout.lines() {
            let path = if kind == "list" {
                list_entry_path(line)
            } else {
                line.trim()
            };
            if path.is_empty() || !seen.insert(path.to_owned()) {
                continue;
            }
            evidence.push(EvidenceRef {
                path: PathBuf::from(path),
                line_range: (1, 1),
                excerpt: String::new(),
            });
            if evidence.len() >= max_lines {
                break;
            }
        }
        if evidence.is_empty() {
            for path in fallback_paths {
                evidence.push(EvidenceRef {
                    path: path.clone(),
                    line_range: (1, 1),
                    excerpt: String::new(),
                });
            }
        }
    }
    evidence
}

fn repair_stages(
    report: &purrcode_validation_runtime::ValidationReport,
) -> BTreeSet<ValidationStage> {
    let mut stages: BTreeSet<_> = report
        .repair_routes()
        .into_iter()
        .map(|route| route.focused_stage)
        .filter(|stage| *stage != ValidationStage::CompletionCriteria)
        .collect();
    for stage in &report.required_stages {
        let satisfied = report
            .evidence
            .iter()
            .any(|evidence| evidence.stage == *stage && evidence.status == EvidenceStatus::Passed);
        if !satisfied {
            stages.insert(*stage);
        }
    }
    stages
}

/// Persist the smallest faithful requirement/spec and task graph derived from
/// a model plan.  The model schema does not carry design decisions, so this
/// helper records none; objective text and plan/turn postconditions are the
/// only accepted requirements we can truthfully claim.
fn record_plan_work(
    store: &mut SessionStore,
    session_id: SessionId,
    objective: &str,
    steps: &[String],
    expected_postconditions: &[String],
) -> Result<(), AgentError> {
    let state = store.load(session_id)?;
    let revision = state
        .spec_bundle
        .as_ref()
        .map_or(1, |bundle| bundle.revision.saturating_add(1));
    let graph_revision = state
        .task_graph
        .as_ref()
        .map_or(1, |graph| graph.revision.saturating_add(1));
    let mut criteria = expected_postconditions
        .iter()
        .chain(steps.iter())
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    if criteria.is_empty() {
        criteria.push(format!(
            "the requested objective is addressed: {}",
            objective.trim()
        ));
    }
    criteria.truncate(64);
    let requirement_id = RequirementId::new();
    let acceptance_criteria = criteria
        .iter()
        .map(|statement| AcceptanceCriterion {
            id: CriterionId::new(),
            statement: statement.clone(),
        })
        .collect::<Vec<_>>();
    let requirement = Requirement {
        id: requirement_id,
        statement: objective.trim().chars().take(2_048).collect(),
        priority: WorkPriority::Required,
        acceptance_criteria: acceptance_criteria.clone(),
    };
    let bundle = SpecBundle {
        revision,
        kind: SpecKind::Direct,
        title: objective.trim().chars().take(256).collect(),
        requirements: vec![requirement],
        non_goals: Vec::new(),
        design_decisions: Vec::new(),
    };
    let criterion_ids = acceptance_criteria
        .iter()
        .map(|criterion| criterion.id)
        .collect::<Vec<_>>();
    let task_steps = if steps.is_empty() {
        vec![objective.trim().to_owned()]
    } else {
        steps.iter().take(64).cloned().collect::<Vec<_>>()
    };
    let mut tasks = Vec::with_capacity(task_steps.len());
    for (index, step) in task_steps.iter().enumerate() {
        let task_id = WorkTaskId::new();
        // The model schema does not declare dependencies.  Keep steps
        // independent rather than inventing a serial graph that would make a
        // later task appear blocked without evidence.
        let dependencies = Vec::new();
        let task_count = task_steps.len().max(1);
        let start = index * criterion_ids.len() / task_count;
        let end = ((index + 1) * criterion_ids.len() / task_count).max(start + 1);
        let acceptance = criterion_ids
            [start.min(criterion_ids.len() - 1)..end.min(criterion_ids.len())]
            .to_vec();
        tasks.push(WorkTask {
            id: task_id,
            objective: step.trim().to_owned(),
            dependencies,
            priority: WorkPriority::Required,
            risk: WorkRisk::Medium,
            acceptance_criteria: acceptance,
            scope: Vec::new(),
            owner: Some("native-agent".into()),
            status: WorkTaskStatus::Ready,
            retry_count: 0,
            evidence_obligations: Vec::new(),
        });
    }
    let graph = TaskGraph {
        revision: graph_revision,
        tasks,
    };
    store.append(
        session_id,
        &SessionEvent::SpecBundleRecorded {
            bundle,
            reason: "native-agent plan recorded from the accepted objective and plan criteria"
                .into(),
        },
    )?;
    store.append(
        session_id,
        &SessionEvent::TaskGraphRecorded {
            graph,
            reason: "native-agent task graph follows the accepted plan step order".into(),
        },
    )?;
    Ok(())
}

fn current_plan_task_id(
    store: &SessionStore,
    session_id: SessionId,
    current_step_index: Option<usize>,
) -> Result<Option<WorkTaskId>, AgentError> {
    let index = current_step_index.unwrap_or(0);
    Ok(store
        .load(session_id)?
        .task_graph
        .and_then(|graph| graph.tasks.get(index).map(|task| task.id)))
}

fn append_task_status(
    store: &mut SessionStore,
    session_id: SessionId,
    task_id: Option<WorkTaskId>,
    status: WorkTaskStatus,
    reason: &str,
) -> Result<(), AgentError> {
    if let Some(task_id) = task_id {
        store.append(
            session_id,
            &SessionEvent::TaskStatusChanged {
                task_id,
                status,
                reason: reason.into(),
            },
        )?;
    }
    Ok(())
}

fn task_status(
    store: &SessionStore,
    session_id: SessionId,
    task_id: WorkTaskId,
) -> Result<Option<WorkTaskStatus>, AgentError> {
    Ok(store
        .load(session_id)?
        .task_graph
        .and_then(|graph| graph.task(task_id).map(|task| task.status)))
}

fn start_task(
    store: &mut SessionStore,
    session_id: SessionId,
    task_id: Option<WorkTaskId>,
    reason: &str,
) -> Result<(), AgentError> {
    let Some(task_id) = task_id else {
        return Ok(());
    };
    match task_status(store, session_id, task_id)? {
        Some(WorkTaskStatus::NeedsAttention) => {
            append_task_status(
                store,
                session_id,
                Some(task_id),
                WorkTaskStatus::Ready,
                "retrying the task after repair evidence",
            )?;
            append_task_status(
                store,
                session_id,
                Some(task_id),
                WorkTaskStatus::Running,
                reason,
            )?;
        }
        Some(WorkTaskStatus::Ready) => append_task_status(
            store,
            session_id,
            Some(task_id),
            WorkTaskStatus::Running,
            reason,
        )?,
        _ => {}
    }
    Ok(())
}

fn append_task_evidence(
    store: &mut SessionStore,
    session_id: SessionId,
    task_id: Option<WorkTaskId>,
    action_id: Option<ActionId>,
    status: ValidationStatus,
    summary: &str,
    closes_criterion: bool,
) -> Result<(), AgentError> {
    let Some(task_id) = task_id else {
        return Ok(());
    };
    let state = store.load(session_id)?;
    let Some(spec) = state.spec_bundle else {
        return Ok(());
    };
    let Some(graph) = state.task_graph else {
        return Ok(());
    };
    let Some(task) = graph.task(task_id) else {
        return Ok(());
    };
    let requirement_id = spec
        .requirements
        .first()
        .map(|requirement| requirement.id)
        .ok_or_else(|| AgentError::CorruptSession("spec has no requirement".into()))?;
    for criterion_id in &task.acceptance_criteria {
        let coverage = if closes_criterion && status == ValidationStatus::Passed {
            purrcode_runtime_core::work::EvidenceCoverage::Covered
        } else if status == ValidationStatus::Passed {
            purrcode_runtime_core::work::EvidenceCoverage::NotRun
        } else {
            purrcode_runtime_core::work::EvidenceCoverage::Failed
        };
        let digest = blake3::hash(summary.as_bytes()).to_hex().to_string();
        store.append(
            session_id,
            &SessionEvent::EvidenceLinked {
                evidence: purrcode_runtime_core::work::EvidenceLink {
                    id: purrcode_runtime_core::work::EvidenceId::new(),
                    requirement_id,
                    criterion_id: *criterion_id,
                    task_id,
                    action_id,
                    coverage,
                    validation_status: Some(status.clone()),
                    source: "native-agent execution result".into(),
                    summary: summary.chars().take(2_048).collect(),
                    digest,
                    recorded_at: Utc::now(),
                },
            },
        )?;
    }
    Ok(())
}

fn close_validated_plan_tasks(
    store: &mut SessionStore,
    session_id: SessionId,
    report: &purrcode_validation_runtime::ValidationReport,
) -> Result<(), AgentError> {
    let state = store.load(session_id)?;
    let Some(graph) = state.task_graph else {
        return Ok(());
    };
    let report_summary = report
        .evidence
        .iter()
        .filter(|evidence| evidence.status == EvidenceStatus::Passed)
        .map(|evidence| format!("{:?}: {}", evidence.stage, evidence.detail))
        .take(8)
        .collect::<Vec<_>>()
        .join("; ");
    if report_summary.is_empty() {
        return Ok(());
    }
    for task in graph.tasks {
        let validation_task =
            task.status == WorkTaskStatus::Ready && task_is_validation_only(&task.objective);
        let executed_task = task.status == WorkTaskStatus::Running
            && state.evidence_links.iter().any(|evidence| {
                evidence.task_id == task.id
                    && evidence.validation_status == Some(ValidationStatus::Passed)
            });
        if !validation_task && !executed_task {
            continue;
        }
        if validation_task {
            append_task_status(
                store,
                session_id,
                Some(task.id),
                WorkTaskStatus::Running,
                "the validation runtime started this validation-only plan task",
            )?;
        }
        append_task_evidence(
            store,
            session_id,
            Some(task.id),
            None,
            ValidationStatus::Passed,
            &format!("validation report passed linked criterion evidence: {report_summary}"),
            true,
        )?;
        append_task_status(
            store,
            session_id,
            Some(task.id),
            WorkTaskStatus::Passed,
            "final validation closed the plan task",
        )?;
    }
    Ok(())
}

fn task_is_validation_only(objective: &str) -> bool {
    let objective = objective.trim().to_ascii_lowercase();
    objective == "validate"
        || objective == "verify"
        || objective == "test"
        || objective.starts_with("validate ")
        || objective.starts_with("verify ")
        || objective.starts_with("test ")
        || objective.starts_with("run test")
        || objective.starts_with("run check")
}

fn required_tasks_passed(store: &SessionStore, session_id: SessionId) -> Result<bool, AgentError> {
    let state = store.load(session_id)?;
    Ok(state.task_graph.is_none_or(|graph| {
        graph.tasks.iter().all(|task| {
            task.priority != WorkPriority::Required || task.status == WorkTaskStatus::Passed
        })
    }))
}

fn pause_after_validation_budget(
    store: &mut SessionStore,
    session_id: SessionId,
    report: &purrcode_validation_runtime::ValidationReport,
) -> Result<AgentOutcome, AgentError> {
    let failed = report
        .evidence
        .iter()
        .filter(|item| {
            matches!(
                item.status,
                EvidenceStatus::Failed | EvidenceStatus::TimedOut | EvidenceStatus::Uncertain
            ) && item.stage != ValidationStage::CompletionCriteria
        })
        .count()
        .max(1);
    let routes = report
        .repair_routes()
        .into_iter()
        .map(|route| format!("{:?} → {}", route.class, route.specialist_role))
        .collect::<Vec<_>>()
        .join(", ");
    store.append(
        session_id,
        &SessionEvent::SessionPaused {
            reason: format!(
                "validation repair budget exhausted after {MAX_VALIDATION_REPAIR_CYCLES} focused cycles ({failed} unresolved check(s)); routes: {}",
                if routes.is_empty() { "diagnostic review required" } else { &routes }
            ),
        },
    )?;
    Ok(AgentOutcome::ValidationFailed { session_id, failed })
}

async fn execute_and_record(
    store: &mut SessionStore,
    session_id: SessionId,
    action_id: ActionId,
    action: &ProposedAction,
    constraints: &purrcode_runtime_core::ActionConstraints,
    worktree: &SessionWorktree,
    turn_id: Option<TurnId>,
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
                    turn_id,
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
