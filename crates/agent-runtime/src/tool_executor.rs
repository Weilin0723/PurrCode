//! The provider-dispatch seam for v1.3 registry tools (PR B).
//!
//! `agent-runtime` cannot depend on `mcp-host`/`skill-store`/`extension-config`
//! without a cycle, so the dispatch contract lives here as a trait and the
//! daemon implements it with the real `McpHost` + skill runtime + claw. Native
//! tools keep flowing through `ToolRuntime::execute`; only `Mcp`/`Skill`
//! providers reach the executor.

use std::collections::BTreeSet;

use async_trait::async_trait;

use purrcode_ninelives::SessionStore;
use purrcode_runtime_core::{
    ActionConstraints, ActionId, EvidenceInitiator, ExecutionEvidence, SessionId, ToolInvocation,
    TurnId,
};

use crate::errors::AgentError;

/// Who asked for this invocation, carried into [`ExecutionEvidence`].
///
/// The executor cannot infer this. Every caller used to be recorded as
/// `EvidenceInitiator::Model`, which made a `before_write` security hook's own
/// tool call read, in the audit trail, as something the model decided to do.
#[derive(Clone, Debug)]
pub struct ToolExecutionContext {
    pub initiator: EvidenceInitiator,
}

impl ToolExecutionContext {
    /// The model proposed this action during `turn_id`.
    pub fn model(turn_id: TurnId) -> Self {
        Self {
            initiator: EvidenceInitiator::Model { turn_id },
        }
    }

    /// A person approved this action directly.
    pub fn human() -> Self {
        Self {
            initiator: EvidenceInitiator::Human,
        }
    }

    /// A governed hook fired this action.
    pub fn hook(hook_id: impl Into<String>, trigger: purrcode_runtime_core::HookTrigger) -> Self {
        Self {
            initiator: EvidenceInitiator::Hook {
                hook_id: hook_id.into(),
                trigger,
            },
        }
    }
}

/// The result of a registry tool execution, plus the durable evidence the
/// caller should attach to the session.
#[derive(Clone, Debug)]
pub struct ToolExecutionOutcome {
    /// stdout (UTF-8 lossy), matching the `ExecutionResult` shape the turn loop
    /// already appends via `ActionOutputRecorded`.
    pub stdout: String,
    /// stderr (UTF-8 lossy).
    pub stderr: String,
    pub exit_code: Option<i32>,
    pub truncated: bool,
    pub affected_paths: Vec<std::path::PathBuf>,
    /// Structured output validated against the descriptor/skill `output_schema`,
    /// when the tool declares one.
    pub structured_output: Option<serde_json::Value>,
    /// Evidence for this invocation, with everything the executor can know
    /// filled in (authority, initiator, scopes, structured output) and the
    /// outcome left provisional.
    ///
    /// It is deliberately NOT persisted by the executor: the real exit status
    /// and the real `affected_paths` are only known after the caller has taken
    /// the post-execution repository snapshot and validated the effect delta.
    /// Persisting here is what produced evidence claiming
    /// `Succeeded { affected_paths: [] }` for a skill that rewrote three files.
    pub evidence: Option<Box<ExecutionEvidence>>,
}

/// Executes a `ProposedAction::Tool` by provider. Implemented by the daemon,
/// which owns `McpHost` and the skill store.
#[async_trait]
#[allow(clippy::too_many_arguments)]
pub trait ToolExecutor: Send + Sync {
    async fn execute_tool(
        &self,
        store: &mut SessionStore,
        session_id: SessionId,
        turn_id: Option<TurnId>,
        action_id: ActionId,
        invocation: &ToolInvocation,
        constraints: &ActionConstraints,
        context: &ToolExecutionContext,
    ) -> Result<ToolExecutionOutcome, AgentError>;
}

/// What came back from a delegation the agent proposed (v1.4 §PR6).
///
/// This — never a worker's transcript — is what re-enters the parent's context.
/// It is deliberately small: the agent needs to know what was decided, what
/// landed, and what still needs a human, and can ask for a specific diff if it
/// needs more.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DelegationHandoff {
    /// `single` | `parallel` | `sequential` | `review`.
    pub classification: String,
    /// Why the runtime decided that, in the classifier's own words.
    pub reason: String,
    /// True when workers actually ran.
    pub delegated: bool,
    /// One line per delegation: specialist, outcome, and integration state.
    pub outcomes: Vec<String>,
    /// Units the runtime refused, with the reason. Surfaced so the agent is not
    /// left waiting for a specialist that was never started.
    pub refusals: Vec<String>,
    /// Delegations whose changes are waiting on a human decision.
    pub awaiting_decision: usize,
}

impl DelegationHandoff {
    /// The message injected into the parent's context. Structured prose, not a
    /// dump: every line is traceable to a delegation the user can inspect.
    pub fn as_context_message(&self) -> String {
        let mut message = String::from("Delegation outcome\n");
        message.push_str(&format!(
            "Decision: {} — {}\n",
            self.classification, self.reason
        ));
        if !self.delegated {
            message.push_str(
                "No specialists were started; continue the work yourself in this session.\n",
            );
            return message;
        }
        for outcome in &self.outcomes {
            message.push_str(&format!("- {outcome}\n"));
        }
        for refusal in &self.refusals {
            message.push_str(&format!("- refused: {refusal}\n"));
        }
        if self.awaiting_decision > 0 {
            message.push_str(&format!(
                "{} worker change set(s) are waiting on a human decision in the agent \
                 workspace; they are NOT in this worktree yet. Do not re-implement them.\n",
                self.awaiting_decision
            ));
        }
        message
    }
}

/// Runs a delegation the main agent proposed (v1.4 §PR2, §PR4).
///
/// The agent loop owns *when* to ask; this owns *whether and how*. The
/// implementation classifies the proposal — and routinely answers "single
/// agent, here is why" — then admits, schedules and integrates. It lives
/// behind a trait for the same reason [`ToolExecutor`] does: the daemon holds
/// the provider router, the worktrees and the store, and a second execution
/// path inside `agent-runtime` would be a second unaudited one.
#[async_trait]
pub trait DelegationPlanner: Send + Sync {
    async fn delegate(
        &self,
        store: &mut SessionStore,
        session_id: SessionId,
        turn_id: TurnId,
        objective: &str,
        units: &[purrcode_runtime_core::delegation::DelegationUnitProposal],
    ) -> Result<DelegationHandoff, AgentError>;
}

/// Governed-hook lifecycle dispatch (v1.3 §8 PR6). The agent turn loop calls
/// this at before_write/after_write/after_validation/after_agent_complete/
/// before_commit; the daemon implements it with the repository's hook set,
/// PawGate, and the ToolExecutor.
#[async_trait]
pub trait HookEvaluator: Send + Sync {
    /// `completed` names hooks that already ran for the action being dispatched
    /// — the chain a previous, now-approved suspension got through. They are
    /// skipped so that approving a hook resumes the chain instead of restarting
    /// it, which would ask for the same approval forever.
    async fn dispatch(
        &self,
        store: &mut SessionStore,
        session_id: SessionId,
        trigger: purrcode_runtime_core::HookTrigger,
        depth: u8,
        completed: &BTreeSet<String>,
    ) -> Result<HookOutcome, AgentError>;
}

/// A hook chain that stopped, waiting for a person.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HookSuspension {
    pub hook_id: String,
    /// The durable pending action the human approves. The session is in
    /// `AwaitingApproval(hook_action_id)`.
    pub hook_action_id: ActionId,
    pub reason: String,
    /// Every hook in this chain that must not fire again when the deferred
    /// action resumes, including the suspended one itself.
    pub completed_hooks: Vec<String>,
}

/// What a trigger's hook chain did to the turn.
///
/// `Suspended` and `Aborted` both stop the turn, but they are different events
/// and must not be reported as the same thing: an aborted chain failed, while a
/// suspended one is waiting on a person and has a durable pending action they
/// can approve to continue. Collapsing them into one boolean made every hook
/// pause read as "a blocking hook aborted the turn" — and, one layer up, as a
/// `SessionFailed`.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum HookOutcome {
    /// Every hook ran (or none fired). The turn continues.
    #[default]
    Continued,
    /// A hook needs human approval. The session is in `AwaitingApproval` with
    /// the hook's exact invocation pending.
    Suspended(HookSuspension),
    /// A blocking hook was denied or failed. The turn fails with it.
    Aborted,
}

impl HookOutcome {
    /// Whether the turn must stop here, for either reason.
    pub fn stops_turn(&self) -> bool {
        !matches!(self, HookOutcome::Continued)
    }

    /// The suspension, when this chain is waiting on a person.
    pub fn suspension(&self) -> Option<&HookSuspension> {
        match self {
            HookOutcome::Suspended(suspension) => Some(suspension),
            _ => None,
        }
    }
}
