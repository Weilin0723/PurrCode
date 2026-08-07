//! Resumable native agent orchestration.

#![allow(clippy::collapsible_if)]

mod agent;
mod context;
mod errors;
mod normalize;
mod schema;
mod stream;

pub use purrcode_whisker::{
    ContextIndexSummary, IndexLifecycleStage, IndexPauseReason, IndexStopReason, IndexingSignals,
    MemoryPressure, Tier0Budget, Tier0Preparation, Tier0Snapshot, Tier1Budget, Tier1Report,
    Tier2Policy, Tier2Status, Tier2StepReport, Tier2Work,
};

pub use crate::agent::{
    AgentCancellation, AgentOutcome, CapabilityResolution, NativeAgent, SkillResolver,
};
pub use crate::context::{AgentContextIndex, AgentContextPolicy};
pub use crate::errors::AgentError;
pub use crate::schema::{AgentAction, AgentPlan, AgentTurn};
pub use crate::stream::{
    AgentStreamEvent, AgentStreamObserver, AgentStreamObserverError, AgentStreamReceiver,
    MAX_STREAM_OBSERVER_CAPACITY, bounded_agent_stream_channel,
};

// P1-10: Re-export NativeAgent::compaction_window for daemon use in /compact.
// The function is a pub(crate) associated fn on NativeAgent; re-export via a
// thin wrapper since associated fns can't be re-exported directly.
pub fn compaction_window(
    messages: &[purrcode_runtime_core::ConversationMessage],
    max_tokens: u64,
) -> usize {
    crate::agent::compaction_window(messages, max_tokens)
}

/// Re-export of `agent::build_semantic_checkpoint`: the one canonical
/// `SemanticCheckpoint` builder, shared by `NativeAgent`'s automatic
/// context-pressure compaction and the daemon's manual
/// `POST /v1/sessions/{id}/compact`. The daemon has no `NativeAgent`
/// instance to compact through, so this is exposed as a free function
/// rather than an associated one — same reasoning as `compaction_window`
/// above.
pub fn build_semantic_checkpoint(
    state: &purrcode_runtime_core::SessionState,
    events: &[purrcode_runtime_core::SessionEvent],
    controls: &purrcode_runtime_core::adaptation::SessionControls,
    turn_id: purrcode_runtime_core::TurnId,
    retained_action_ids: &[purrcode_runtime_core::ActionId],
) -> purrcode_runtime_core::SemanticCheckpoint {
    crate::agent::build_semantic_checkpoint(state, events, controls, turn_id, retained_action_ids)
}

/// How many of the most recent proposed actions a compaction retains.
/// Re-exported so the daemon's manual `/compact` uses the exact bound
/// automatic compaction does, rather than an independently-drifting number.
pub const RETAINED_ACTIONS_AFTER_COMPACTION: usize =
    crate::agent::RETAINED_ACTIONS_AFTER_COMPACTION;

/// Target token budget for the retained conversation window after
/// compaction, re-exported for the same reason as
/// [`RETAINED_ACTIONS_AFTER_COMPACTION`].
pub const COMPACTION_RETAINED_TOKEN_BUDGET: u64 = crate::agent::COMPACTION_RETAINED_TOKEN_BUDGET;
