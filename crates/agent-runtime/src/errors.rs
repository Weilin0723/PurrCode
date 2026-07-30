use thiserror::Error;

use purrcode_claw::ExecutionError;
use purrcode_ninelives::StoreError;
use purrcode_provider_gateway::{ProviderError, ProviderErrorCategory, StreamStateError};
use purrcode_repository_engine::RepositoryError;
use purrcode_test_orchestrator::ValidationError;
use purrcode_whisker::ContextError;

use crate::context::AgentContextIndexError;

#[derive(Debug, Error)]
pub enum AgentError {
    #[error("session storage failed: {0}")]
    Store(#[from] StoreError),
    #[error("repository isolation failed: {0}")]
    Repository(#[from] RepositoryError),
    #[error("model provider failed: {0}")]
    Provider(#[from] ProviderError),
    #[error("provider stream state failed: {0}")]
    StreamState(#[from] StreamStateError),
    #[error("tool execution failed: {0}")]
    Execution(#[from] ExecutionError),
    #[error("validation discovery failed: {0}")]
    Validation(#[from] ValidationError),
    #[error("repository context failed: {0}")]
    Context(#[from] ContextError),
    #[error("tiered repository context failed: {0}")]
    TieredContext(#[from] AgentContextIndexError),
    #[error("domain operation failed: {0}")]
    Domain(#[from] purrcode_runtime_core::DomainError),
    #[error("filesystem operation failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("model returned invalid structured data: {0}")]
    Structured(#[from] serde_json::Error),
    #[error("model turn is invalid: {0}")]
    InvalidModelTurn(String),
    #[error("agent request was cancelled: {0}")]
    Cancelled(String),
    #[error("session is corrupt: {0}")]
    CorruptSession(String),
    #[error("session cannot be resumed from state {0}")]
    SessionNotResumable(String),
    #[error("session is not waiting for approval")]
    SessionNotAwaitingApproval,
    #[error("unconstrained allow is forbidden")]
    UnsafeUnconstrainedAllow,
}

impl AgentError {
    pub fn is_cancelled(&self) -> bool {
        matches!(
            self,
            Self::Provider(error)
                if error.category() == Some(ProviderErrorCategory::Cancelled)
        ) || matches!(self, Self::Cancelled(_))
    }
}
