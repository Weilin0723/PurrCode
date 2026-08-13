//! The model-facing half of v1.5.
//!
//! `runtime-core` holds the durable state: what the user asked for, what the
//! reviewers found, whether the gate cleared. None of that runs anything. This
//! crate is the behaviour on the other side of that line — the calls that turn
//! a user's message into a contract, a diff into findings, and a finding into
//! repair work — following the split `delegation.rs` and `delegation-runtime`
//! already use.
//!
//! Three invariants are carried by types here rather than by prompts, because
//! each of them fails silently when it is left to whoever assembles the
//! request:
//!
//! - **A reviewer cannot be handed the implementer's transcript.**
//!   [`FreshReviewInput`] has no field it could arrive in. `ReviewContext::Fresh`
//!   in the log then records something that was true by construction rather
//!   than something a caller wrote down.
//! - **A requirement cannot be invented.** [`intent::compile`] refuses a clause
//!   whose quotation does not appear in a message the user actually sent, so
//!   "don't clutter the UI" cannot become "remove the advanced settings" over
//!   two paraphrases that each looked reasonable.
//! - **A reviewer cannot repair.** The reviewers here return findings and
//!   verdicts and have no way to express an edit; repair is
//!   [`correction::next_step`] handing work to a different role.

pub mod correction;
pub mod fresh;
pub mod intent;
pub mod review;

pub use correction::{CorrectionStep, RepairAssignment};
pub use fresh::{FreshReviewInput, ReviewSubject};
pub use intent::{CompiledContract, IntentCompiler};
pub use review::{AlignmentReviewer, IndependentCodeReviewer, RequirementVerdict, ReviewOutcome};

use futures::StreamExt;
use purrcode_provider_gateway::{
    ModelEvent, ModelId, ModelMessage, ModelProvider, ModelRequest, ProviderError,
    ProviderStreamEvent,
};
use schemars::schema::RootSchema;
use std::sync::Arc;

/// Input and output tokens one call cost, when the provider reported them.
///
/// `None` means the provider did not say — which the benchmark reports as
/// unmeasured spend rather than as zero. A review whose cost is unknown is not
/// a free review.
pub type Usage = Option<(u64, u64)>;

/// Everything that can go wrong on this side of the line.
#[derive(Debug, thiserror::Error)]
pub enum AlignmentError {
    #[error("provider call failed: {0}")]
    Provider(#[from] ProviderError),
    #[error("the model returned a shape that did not decode: {0}")]
    Decode(String),
    /// A clause the user never said. Refused rather than repaired, because the
    /// repair would be a paraphrase, which is the failure itself.
    #[error("{0}")]
    Unfaithful(String),
    #[error("{0}")]
    Invalid(String),
}

/// Which model runs a stage, and where.
///
/// Held as a pair rather than resolved from a global registry so a deployment
/// can point the reviewer at a *different* model from the one that wrote the
/// code (v1.5 §14). A model reviewing its own output shares its blind spots,
/// and the review it produces is correlated with the mistakes it exists to
/// catch.
#[derive(Clone)]
pub struct ModelRoute {
    pub provider: Arc<dyn ModelProvider>,
    pub model: ModelId,
}

impl ModelRoute {
    pub fn new(provider: Arc<dyn ModelProvider>, model: ModelId) -> Self {
        Self { provider, model }
    }

    /// One schema-constrained call, with what it cost.
    ///
    /// Streamed rather than fetched whole, for one reason: the streaming path
    /// is where providers report token usage, and a review whose cost nobody
    /// measured is a review that looks free. The v1.5 release bar fails a run
    /// that cannot account for its review overhead, and it should — an
    /// unbounded reviewer that nobody is billing for is exactly the failure
    /// mode the paired gates exist to catch.
    pub(crate) async fn structured<T: serde::de::DeserializeOwned>(
        &self,
        messages: Vec<ModelMessage>,
        schema: RootSchema,
    ) -> Result<(T, Usage), AlignmentError> {
        eprintln!(
            "DEBUG_ALIGNMENT_CALL thread={:?} model={} messages={}",
            std::thread::current().name(),
            self.model.model,
            messages.len()
        );
        let request = ModelRequest {
            model: self.model.clone(),
            messages,
            tools: Vec::new(),
            max_output_tokens: Some(4096),
            reasoning_effort: None,
        };
        let mut stream = self.provider.structured_stream(request, schema).await?;
        let mut output = String::new();
        let mut usage = None;
        while let Some(event) = stream.next().await {
            match event? {
                ProviderStreamEvent::Model(ModelEvent::TextDelta(delta)) => output.push_str(&delta),
                ProviderStreamEvent::Model(ModelEvent::Usage {
                    input_tokens,
                    output_tokens,
                }) => usage = Some((input_tokens, output_tokens)),
                _ => {}
            }
        }
        let value: T = serde_json::from_str(&output)
            .map_err(|error| AlignmentError::Decode(format!("{error}")))?;
        Ok((value, usage))
    }
}

impl std::fmt::Debug for ModelRoute {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ModelRoute")
            .field("model", &self.model)
            .finish_non_exhaustive()
    }
}

/// Normalise text for the provenance check.
///
/// Whitespace and case are the two things a model changes without meaning to
/// change the meaning. Everything else — a dropped "don't", a swapped noun — is
/// exactly what the check exists to catch, so nothing else is normalised away.
pub(crate) fn normalise(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::*;
    use async_trait::async_trait;
    use purrcode_provider_gateway::{
        ModelCapabilities, ModelEventStream, ProviderHealth, TokenEstimate,
    };
    use serde_json::Value;
    use std::sync::Mutex;

    /// A provider that returns whatever the test told it to, and remembers what
    /// it was asked. The second half is what the containment tests read.
    pub struct ScriptedProvider {
        replies: Mutex<Vec<Value>>,
        pub seen: Mutex<Vec<Vec<ModelMessage>>>,
    }

    impl ScriptedProvider {
        pub fn new(replies: Vec<Value>) -> Arc<Self> {
            Arc::new(Self {
                replies: Mutex::new(replies.into_iter().rev().collect()),
                seen: Mutex::new(Vec::new()),
            })
        }

        /// Everything the provider was ever sent, flattened.
        pub fn transcript(&self) -> String {
            self.seen
                .lock()
                .unwrap()
                .iter()
                .flat_map(|messages| messages.iter().map(|message| message.content.clone()))
                .collect::<Vec<_>>()
                .join("\n")
        }
    }

    #[async_trait]
    impl ModelProvider for ScriptedProvider {
        async fn capabilities(&self, _model: &ModelId) -> Result<ModelCapabilities, ProviderError> {
            Ok(ModelCapabilities::unknown(false))
        }

        async fn stream(&self, _request: ModelRequest) -> Result<ModelEventStream, ProviderError> {
            Err(ProviderError::InvalidResponse("not used".into()))
        }

        async fn structured(
            &self,
            request: ModelRequest,
            _schema: RootSchema,
        ) -> Result<Value, ProviderError> {
            self.seen.lock().unwrap().push(request.messages);
            self.replies
                .lock()
                .unwrap()
                .pop()
                .ok_or_else(|| ProviderError::InvalidResponse("the script ran out".into()))
        }

        async fn count_tokens(
            &self,
            _request: &ModelRequest,
        ) -> Result<TokenEstimate, ProviderError> {
            Ok(TokenEstimate {
                tokens: 0,
                exact: false,
            })
        }

        async fn health_check(&self) -> Result<ProviderHealth, ProviderError> {
            Ok(ProviderHealth {
                available: true,
                detail: "scripted".into(),
            })
        }
    }

    pub fn route(provider: Arc<ScriptedProvider>) -> ModelRoute {
        ModelRoute::new(
            provider,
            ModelId {
                provider: "test".into(),
                model: "scripted".into(),
            },
        )
    }
}
