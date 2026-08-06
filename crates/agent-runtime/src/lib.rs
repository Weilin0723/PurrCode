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
