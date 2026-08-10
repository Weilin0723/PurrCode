//! The provider-dispatch seam for v1.3 registry tools (PR B).
//!
//! `agent-runtime` cannot depend on `mcp-host`/`skill-store`/`extension-config`
//! without a cycle, so the dispatch contract lives here as a trait and the
//! daemon implements it with the real `McpHost` + skill runtime + claw. Native
//! tools keep flowing through `ToolRuntime::execute`; only `Mcp`/`Skill`
//! providers reach the executor.

use async_trait::async_trait;

use purrcode_ninelives::SessionStore;
use purrcode_runtime_core::{ActionConstraints, ActionId, SessionId, ToolInvocation, TurnId};

use crate::errors::AgentError;

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
}

/// Executes a `ProposedAction::Tool` by provider. Implemented by the daemon,
/// which owns `McpHost` and the skill store.
#[async_trait]
pub trait ToolExecutor: Send + Sync {
    async fn execute_tool(
        &self,
        store: &mut SessionStore,
        session_id: SessionId,
        turn_id: Option<TurnId>,
        action_id: ActionId,
        invocation: &ToolInvocation,
        constraints: &ActionConstraints,
    ) -> Result<ToolExecutionOutcome, AgentError>;
}

/// Governed-hook lifecycle dispatch (v1.3 §8 PR6). The agent turn loop calls
/// this at before_write/after_write/after_validation/after_agent_complete/
/// before_commit; the daemon implements it with the repository's hook set,
/// PawGate, and the ToolExecutor. Returns `true` when a blocking hook aborted
/// the turn (the caller must fail the turn).
#[async_trait]
pub trait HookEvaluator: Send + Sync {
    async fn dispatch(
        &self,
        store: &mut SessionStore,
        session_id: SessionId,
        trigger: purrcode_runtime_core::HookTrigger,
        depth: u8,
    ) -> Result<HookOutcome, AgentError>;
}

/// What a trigger's hook chain did to the turn.
///
/// `Suspended` and `Aborted` both stop the turn, but they are different events
/// and must not be reported as the same thing: an aborted chain failed, while a
/// suspended one is waiting on a person and has a durable pending action they
/// can approve to continue. Collapsing them into one boolean made every hook
/// pause read as "a blocking hook aborted the turn".
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum HookOutcome {
    /// Every hook ran (or none fired). The turn continues.
    #[default]
    Continued,
    /// A hook needs human approval. The session is in `AwaitingApproval` with
    /// the hook's exact invocation pending.
    Suspended,
    /// A blocking hook was denied or failed. The turn fails with it.
    Aborted,
}

impl HookOutcome {
    /// Whether the turn must stop here, for either reason.
    pub fn stops_turn(self) -> bool {
        !matches!(self, HookOutcome::Continued)
    }
}
