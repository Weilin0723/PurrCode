//! Transparent provider failover.
//!
//! [`FailoverProvider`] wraps one primary provider plus an ordered list of
//! fallbacks and tries them in sequence. A provider that is temporarily out of
//! quota (HTTP 429), over its payment balance (HTTP 402), unreachable, or
//! timing out is skipped in favour of the next compatible provider in the
//! chain, so a single exhausted API cannot break a session. The winning
//! provider is reported to the caller so usage accounting stays truthful.

use std::sync::Arc;

use schemars::schema::RootSchema;
use serde_json::Value;

use crate::{
    ModelCapabilities, ModelEventStream, ModelId, ModelProvider, ModelRequest, ProviderError,
    ProviderErrorCategory, ProviderEventStream, ProviderHealth, TokenEstimate,
};

/// Whether a provider error should advance the failover chain.
///
/// Quota/payment (429/402), timeouts, and unreachable providers are transient
/// capacity problems another provider can absorb. Everything else (schema,
/// credentials, content-type, model-not-found) is a configuration problem that
/// retrying a different provider would not fix, so it is returned as-is.
pub fn should_failover(error: &ProviderError) -> bool {
    match error {
        ProviderError::HttpStatus { status, .. } => matches!(*status, 402 | 429),
        ProviderError::Diagnostic(diagnostic) => {
            matches!(
                diagnostic.category,
                ProviderErrorCategory::Timeout | ProviderErrorCategory::Unreachable
            ) || matches!(diagnostic.http_status, Some(402 | 429))
        }
        ProviderError::Unavailable(_) => true,
        _ => false,
    }
}

/// A `ModelProvider` that retries the same request across an ordered chain of
/// providers. The primary is tried first; on a failover-worthy error the next
/// compatible provider is tried, until the chain is exhausted.
#[derive(Clone)]
pub struct FailoverProvider {
    primary: Arc<dyn ModelProvider>,
    fallbacks: Vec<Arc<dyn ModelProvider>>,
}

impl FailoverProvider {
    pub fn new(primary: Arc<dyn ModelProvider>, fallbacks: Vec<Arc<dyn ModelProvider>>) -> Self {
        Self { primary, fallbacks }
    }

    /// The full chain, primary first.
    pub fn chain(&self) -> Vec<&Arc<dyn ModelProvider>> {
        std::iter::once(&self.primary)
            .chain(self.fallbacks.iter())
            .collect()
    }

    /// Run `call` against each provider in turn, returning the first success.
    /// Errors that are not failover-worthy are returned immediately; a
    /// failover-worthy error advances to the next provider. If every provider
    /// fails, the last error is returned.
    async fn try_chain<'f, T, F>(&self, call: F) -> Result<T, ProviderError>
    where
        F: Fn(Arc<dyn ModelProvider>) -> futures::future::BoxFuture<'f, Result<T, ProviderError>>,
    {
        let mut last_error: Option<ProviderError> = None;
        for provider in self.chain() {
            match call(provider.clone()).await {
                Ok(value) => return Ok(value),
                Err(error) if should_failover(&error) => last_error = Some(error),
                Err(error) => return Err(error),
            }
        }
        Err(last_error.unwrap_or_else(|| {
            ProviderError::Unavailable("all providers in the failover chain failed".into())
        }))
    }
}

#[async_trait::async_trait]
impl ModelProvider for FailoverProvider {
    async fn capabilities(&self, model: &ModelId) -> Result<ModelCapabilities, ProviderError> {
        self.try_chain(|provider| Box::pin(async move { provider.capabilities(model).await }))
            .await
    }

    async fn stream(&self, request: ModelRequest) -> Result<ModelEventStream, ProviderError> {
        self.try_chain(|provider| {
            let request = request.clone();
            Box::pin(async move { provider.stream(request).await })
        })
        .await
    }

    async fn structured(
        &self,
        request: ModelRequest,
        schema: RootSchema,
    ) -> Result<Value, ProviderError> {
        self.try_chain(|provider| {
            let request = request.clone();
            let schema = schema.clone();
            Box::pin(async move { provider.structured(request, schema).await })
        })
        .await
    }

    async fn structured_stream(
        &self,
        request: ModelRequest,
        schema: RootSchema,
    ) -> Result<ProviderEventStream, ProviderError> {
        self.try_chain(|provider| {
            let request = request.clone();
            let schema = schema.clone();
            Box::pin(async move { provider.structured_stream(request, schema).await })
        })
        .await
    }

    async fn count_tokens(&self, request: &ModelRequest) -> Result<TokenEstimate, ProviderError> {
        self.try_chain(|provider| {
            let request = request.clone();
            Box::pin(async move { provider.count_tokens(&request).await })
        })
        .await
    }

    async fn health_check(&self) -> Result<ProviderHealth, ProviderError> {
        self.try_chain(|provider| Box::pin(async move { provider.health_check().await }))
            .await
    }
}
