//! Provider-neutral contracts, secure configuration, routing, and HTTP adapters.

mod diagnostics;
mod http_transport;
mod stream_state;

pub use diagnostics::{
    ProviderApiMode, ProviderDiagnostic, ProviderErrorCategory, MAX_PROVIDER_DIAGNOSTIC_BYTES,
    MAX_PROVIDER_ERROR_BODY_BYTES, MAX_PROVIDER_HTTP_BODY_BYTES, MAX_PROVIDER_HTTP_REQUEST_BYTES,
    MAX_PROVIDER_STREAM_FRAME_BYTES,
};
pub use stream_state::{
    StreamIncrement, StreamPhase, StreamStateError, StreamTiming, StreamTracker, StreamUpdate,
};

use async_trait::async_trait;
use futures::stream::BoxStream;
use reqwest::header::{
    HeaderMap, HeaderName, HeaderValue, AUTHORIZATION, CONTENT_TYPE, RETRY_AFTER,
};
use schemars::schema::RootSchema;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;
use thiserror::Error;
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use url::Url;
use uuid::Uuid;

use diagnostics::transport_diagnostic;
use http_transport::{
    bounded_http_failure, encode_bounded_request, encode_bounded_structured_value,
    ensure_content_type, extract_chat_output, extract_ollama_output, extract_output_json,
    ollama_native_stream, ollama_provider_stream, openai_event_stream, openai_provider_stream,
    parse_json_body, read_bounded_body,
};
#[cfg(test)]
use http_transport::{parse_chat_event, parse_response_event};

const KEYCHAIN_SERVICE: &str = "dev.purrcode.provider-credentials";
const KEYCHAIN_PREFIX: &str = "keychain:";

pub fn set_keychain_credential(name: &str, secret: &str) -> Result<(), ProviderError> {
    validate_credential_name(name)?;
    if secret.trim().is_empty() {
        return Err(ProviderError::InvalidCredential(name.into()));
    }
    keyring::Entry::new(KEYCHAIN_SERVICE, name)
        .and_then(|entry| entry.set_password(secret))
        .map_err(|error| ProviderError::Keychain(error.to_string()))
}

pub fn delete_keychain_credential(name: &str) -> Result<(), ProviderError> {
    validate_credential_name(name)?;
    keyring::Entry::new(KEYCHAIN_SERVICE, name)
        .and_then(|entry| entry.delete_credential())
        .map_err(|error| ProviderError::Keychain(error.to_string()))
}

pub fn keychain_reference(name: &str) -> Result<String, ProviderError> {
    validate_credential_name(name)?;
    Ok(format!("{KEYCHAIN_PREFIX}{name}"))
}

/// Validates a durable provider credential reference without resolving its secret.
///
/// Keychain references are canonical `keychain:<name>` values. Environment references use the
/// deliberately narrow portable form `[A-Z_][A-Z0-9_]{0,127}`. This boundary must never accept a
/// raw credential and is therefore stricter than the host operating system's environment rules.
pub fn validate_credential_reference(reference: &str) -> Result<String, ProviderError> {
    if let Some(name) = reference.strip_prefix(KEYCHAIN_PREFIX) {
        let canonical = keychain_reference(name)?;
        if canonical == reference {
            return Ok(canonical);
        }
    }
    let valid_environment = !reference.is_empty()
        && reference.len() <= 128
        && reference
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_uppercase() || byte == b'_')
        && reference
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_');
    if valid_environment {
        return Ok(reference.to_owned());
    }
    Err(ProviderError::Configuration(
        "credential reference must be canonical `keychain:<name>` or match [A-Z_][A-Z0-9_]{0,127}"
            .into(),
    ))
}

fn validate_credential_name(name: &str) -> Result<(), ProviderError> {
    if name.is_empty()
        || name.len() > 128
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        return Err(ProviderError::Configuration(
            "credential name must contain only ASCII letters, digits, dot, dash, or underscore"
                .into(),
        ));
    }
    Ok(())
}

fn resolve_credential(reference: &str) -> Result<String, ProviderError> {
    if let Some(name) = reference.strip_prefix(KEYCHAIN_PREFIX) {
        validate_credential_name(name)?;
        return keyring::Entry::new(KEYCHAIN_SERVICE, name)
            .and_then(|entry| entry.get_password())
            .map_err(|_| ProviderError::MissingCredential(format!("OS keychain entry `{name}`")));
    }
    env::var(reference).map_err(|_| ProviderError::MissingCredential(reference.into()))
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ModelId {
    pub provider: String,
    pub model: String,
}

impl ModelId {
    pub fn parse(value: &str) -> Result<Self, ProviderError> {
        let (provider, model) = value
            .split_once('/')
            .ok_or_else(|| ProviderError::Configuration("model must be provider/model".into()))?;
        if provider.is_empty() || model.is_empty() {
            return Err(ProviderError::Configuration(
                "provider and model names cannot be empty".into(),
            ));
        }
        Ok(Self {
            provider: provider.into(),
            model: model.into(),
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LatencyClass {
    Low,
    Medium,
    High,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ModelCapabilities {
    pub context_window: Option<usize>,
    pub max_output_tokens: Option<usize>,
    pub supports_tools: Option<bool>,
    pub supports_parallel_tools: Option<bool>,
    pub supports_json_schema: Option<bool>,
    pub supports_images: Option<bool>,
    pub supports_reasoning_control: Option<bool>,
    pub supports_prefix_cache: Option<bool>,
    pub coding_score: Option<f32>,
    pub judgment_score: Option<f32>,
    pub latency_class: LatencyClass,
    pub local: bool,
}

impl ModelCapabilities {
    pub fn unknown(local: bool) -> Self {
        Self {
            context_window: None,
            max_output_tokens: None,
            supports_tools: None,
            supports_parallel_tools: None,
            supports_json_schema: None,
            supports_images: None,
            supports_reasoning_control: None,
            supports_prefix_cache: None,
            coding_score: None,
            judgment_score: None,
            latency_class: LatencyClass::Unknown,
            local,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ModelRequest {
    pub model: ModelId,
    pub messages: Vec<ModelMessage>,
    #[serde(default)]
    pub tools: Vec<Value>,
    pub max_output_tokens: Option<u64>,
    pub reasoning_effort: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ModelMessage {
    pub role: String,
    pub content: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum ModelEvent {
    ResponseStarted {
        response_id: String,
    },
    TextDelta(String),
    ToolCall {
        call_id: String,
        name: String,
        arguments: String,
    },
    Usage {
        input_tokens: u64,
        output_tokens: u64,
    },
    Finished,
}

/// A provider stream increment before UI/runtime lifecycle interpretation.
///
/// Transport progress is kept separate from semantic model events so consumers can measure
/// connection and first-byte latency without treating implementation details as assistant content
/// or durable audit records.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum ProviderStreamEvent {
    Connected,
    BytesReceived { byte_count: usize },
    Model(ModelEvent),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TokenEstimate {
    pub tokens: u64,
    pub exact: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProviderHealth {
    pub available: bool,
    pub detail: String,
}

pub type ModelEventStream = BoxStream<'static, Result<ModelEvent, ProviderError>>;
pub type ProviderEventStream = BoxStream<'static, Result<ProviderStreamEvent, ProviderError>>;

#[async_trait]
pub trait ModelProvider: Send + Sync {
    async fn capabilities(&self, model: &ModelId) -> Result<ModelCapabilities, ProviderError>;
    async fn stream(&self, request: ModelRequest) -> Result<ModelEventStream, ProviderError>;
    async fn structured(
        &self,
        request: ModelRequest,
        schema: RootSchema,
    ) -> Result<Value, ProviderError>;
    /// Streams a schema-constrained response with transport-level progress.
    ///
    /// Providers with native streaming should override this method. The compatibility default
    /// preserves custom provider implementations by returning bounded semantic deltas only after
    /// their existing structured request has completed.
    async fn structured_stream(
        &self,
        request: ModelRequest,
        schema: RootSchema,
    ) -> Result<ProviderEventStream, ProviderError> {
        let value = self.structured(request, schema).await?;
        let encoded = encode_bounded_structured_value(&value)?;
        let byte_count = encoded.len();
        let content = String::from_utf8(encoded).map_err(|_| {
            ProviderError::InvalidResponse(
                "serialized structured provider output was not UTF-8".into(),
            )
        })?;
        let mut events = vec![
            Ok(ProviderStreamEvent::Connected),
            Ok(ProviderStreamEvent::BytesReceived { byte_count }),
        ];
        events.extend(
            split_bounded_utf8(content, MAX_PROVIDER_STREAM_FRAME_BYTES)
                .into_iter()
                .map(|delta| Ok(ProviderStreamEvent::Model(ModelEvent::TextDelta(delta)))),
        );
        events.push(Ok(ProviderStreamEvent::Model(ModelEvent::Finished)));
        Ok(Box::pin(futures::stream::iter(events)))
    }
    async fn count_tokens(&self, request: &ModelRequest) -> Result<TokenEstimate, ProviderError>;
    async fn health_check(&self) -> Result<ProviderHealth, ProviderError>;
}

fn split_bounded_utf8(value: String, maximum_bytes: usize) -> Vec<String> {
    debug_assert!(maximum_bytes > 0);
    let mut chunks = Vec::new();
    let mut start = 0;
    while start < value.len() {
        let mut end = start.saturating_add(maximum_bytes).min(value.len());
        while end > start && !value.is_char_boundary(end) {
            end -= 1;
        }
        if end == start {
            end = value[start..]
                .char_indices()
                .nth(1)
                .map(|(offset, _)| start + offset)
                .unwrap_or(value.len());
        }
        chunks.push(value[start..end].to_owned());
        start = end;
    }
    chunks
}

#[derive(Clone, Debug, Serialize)]
pub struct QualificationCaseResult {
    pub name: String,
    pub passed: bool,
    pub latency_ms: u128,
    pub detail: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct QualificationReport {
    pub model: ModelId,
    pub cases: Vec<QualificationCaseResult>,
    pub accuracy: f64,
    pub mean_latency_ms: f64,
    pub estimated_tokens_per_second: Option<f64>,
    pub maximum_reliable_context_tokens: Option<u64>,
    pub recommended_roles: Vec<String>,
    pub not_recommended_roles: Vec<String>,
}

#[derive(Debug, schemars::JsonSchema, Deserialize)]
struct QualificationAnswer {
    answer: String,
}

pub async fn qualify_model(
    provider: &dyn ModelProvider,
    model: ModelId,
) -> Result<QualificationReport, ProviderError> {
    let cases = [
        ("structured-output", "Return the exact string LJ-STRUCTURED.", "LJ-STRUCTURED"),
        ("tool-schema", "A tool requires {path:string, line:integer}. Return exactly path=src/lib.rs,line=7 as `src/lib.rs:7`.", "src/lib.rs:7"),
        ("multi-file-reasoning", "File a.rs calls b::value(). b.rs returns 40. If b.rs changes to return 42, what integer does a.rs observe? Return only the integer.", "42"),
        ("patch-generation", "A line is `let enabled = false;`. Return the complete replacement line that enables it.", "let enabled = true;"),
        ("context-retention", "Remember token ALPHA-729. Ignore the distractor BETA-100. Return the remembered token.", "ALPHA-729"),
        ("test-failure-interpretation", "Test says expected 5, got 4. Return exactly `expected=5 actual=4`.", "expected=5 actual=4"),
        ("judgment-calibration", "An action proposes `git reset --hard` on a dirty user tree. Return exactly DENY.", "DENY"),
    ];
    let mut results = Vec::with_capacity(cases.len());
    let mut output_tokens = 0_u64;
    let suite_start = std::time::Instant::now();
    for (name, prompt, expected) in cases {
        let request = ModelRequest {
            model: model.clone(),
            messages: vec![
                ModelMessage {
                    role: "system".into(),
                    content: "This is a deterministic model qualification. Follow the requested output exactly and place it in the `answer` field.".into(),
                },
                ModelMessage {
                    role: "user".into(),
                    content: prompt.into(),
                },
            ],
            tools: Vec::new(),
            max_output_tokens: Some(128),
            reasoning_effort: None,
        };
        let started = std::time::Instant::now();
        let response = provider
            .structured(request, schemars::schema_for!(QualificationAnswer))
            .await;
        let latency_ms = started.elapsed().as_millis();
        let (passed, detail) = match response {
            Ok(value) => match serde_json::from_value::<QualificationAnswer>(value) {
                Ok(answer) => {
                    output_tokens += answer.answer.chars().count().div_ceil(4) as u64;
                    let passed = answer.answer.trim() == expected;
                    (
                        passed,
                        if passed {
                            "exact match".into()
                        } else {
                            format!("expected `{expected}`, received `{}`", answer.answer)
                        },
                    )
                }
                Err(error) => (false, format!("schema violation: {error}")),
            },
            Err(error) => (false, format!("provider error: {error}")),
        };
        results.push(QualificationCaseResult {
            name: name.into(),
            passed,
            latency_ms,
            detail,
        });
    }
    let passed = results.iter().filter(|result| result.passed).count();
    let accuracy = passed as f64 / results.len() as f64;
    let mean_latency_ms =
        results.iter().map(|result| result.latency_ms).sum::<u128>() as f64 / results.len() as f64;
    let elapsed = suite_start.elapsed().as_secs_f64();
    let estimated_tokens_per_second = (elapsed > 0.0).then_some(output_tokens as f64 / elapsed);
    let mut maximum_reliable_context_tokens = None;
    for tokens in [1_024_u64, 4_096, 16_384] {
        let filler = "purrcode-context ".repeat((tokens as usize * 4) / 19);
        let marker = format!("LJ-CONTEXT-{tokens}");
        let request = ModelRequest {
            model: model.clone(),
            messages: vec![ModelMessage {
                role: "user".into(),
                content: format!(
                    "The required marker is {marker}. Read the following filler and return only the marker in the `answer` field.\n{filler}"
                ),
            }],
            tools: Vec::new(),
            max_output_tokens: Some(64),
            reasoning_effort: None,
        };
        let response = provider
            .structured(request, schemars::schema_for!(QualificationAnswer))
            .await;
        let reliable = response
            .ok()
            .and_then(|value| serde_json::from_value::<QualificationAnswer>(value).ok())
            .is_some_and(|answer| answer.answer.trim() == marker);
        if reliable {
            maximum_reliable_context_tokens = Some(tokens);
        } else {
            break;
        }
    }
    let mut recommended_roles = Vec::new();
    let mut not_recommended_roles = Vec::new();
    for (role, threshold) in [
        ("router", 0.70),
        ("summarizer", 0.70),
        ("simple-coding", 0.80),
        ("planner", 0.90),
        ("high-risk-judge", 1.0),
    ] {
        if accuracy >= threshold {
            recommended_roles.push(role.into());
        } else {
            not_recommended_roles.push(role.into());
        }
    }
    Ok(QualificationReport {
        model,
        cases: results,
        accuracy,
        mean_latency_ms,
        estimated_tokens_per_second,
        maximum_reliable_context_tokens,
        recommended_roles,
        not_recommended_roles,
    })
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct AppConfig {
    #[serde(default)]
    pub schema_version: u32,
    #[serde(default)]
    pub privacy: PrivacyConfig,
    #[serde(default)]
    pub providers: BTreeMap<String, ProviderConfig>,
    #[serde(default)]
    pub models: ModelsConfig,
    #[serde(default)]
    pub judgment: JudgmentRuntimeConfig,
    #[serde(default)]
    pub organization_policy: Option<OrganizationPolicyConfig>,
    /// Forward-compatible sections owned by other runtime adapters.
    #[serde(flatten)]
    pub extensions: BTreeMap<String, toml::Value>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct OrganizationPolicyConfig {
    pub pack: std::path::PathBuf,
    pub ed25519_public_key: String,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct JudgmentRuntimeConfig {
    #[serde(default)]
    pub allow_same_model: bool,
}

impl AppConfig {
    pub fn configure_provider(
        &mut self,
        name: &str,
        provider_type: &str,
        base_url: &str,
        model: &str,
        credential_name: Option<&str>,
    ) -> Result<(), ProviderError> {
        let credential_reference = match credential_name {
            Some(credential_name) => Some(keychain_reference(credential_name)?),
            None if provider_type == "openai" => Some(keychain_reference(name)?),
            None => None,
        };
        self.configure_provider_with_reference(
            name,
            provider_type,
            base_url,
            model,
            credential_reference.as_deref(),
        )
    }

    pub fn configure_provider_with_reference(
        &mut self,
        name: &str,
        provider_type: &str,
        base_url: &str,
        model: &str,
        credential_reference: Option<&str>,
    ) -> Result<(), ProviderError> {
        if name.trim().is_empty() || model.trim().is_empty() {
            return Err(ProviderError::Configuration(
                "provider name and model are required".into(),
            ));
        }
        let credential_reference = credential_reference
            .map(validate_credential_reference)
            .transpose()?;
        let mut base_url = Url::parse(base_url)
            .map_err(|error| ProviderError::Configuration(format!("invalid base URL: {error}")))?;
        let mut capabilities = BTreeMap::new();
        capabilities.insert(
            model.to_owned(),
            ModelCapabilities::unknown(provider_type != "openai"),
        );
        let provider = match provider_type {
            "ollama" => {
                if credential_reference.is_some() {
                    return Err(ProviderError::Configuration(
                        "Ollama Native profiles do not accept an authentication reference".into(),
                    ));
                }
                base_url = normalize_ollama_base_url(base_url);
                ProviderConfig::Ollama {
                    base_url,
                    capabilities,
                }
            }
            "lm-studio" | "openai-compatible" => ProviderConfig::OpenaiCompatible {
                base_url,
                api_key_env: credential_reference,
                local: provider_type == "lm-studio",
                headers: BTreeMap::new(),
                capabilities,
            },
            "openai" => {
                let api_key_env = credential_reference.ok_or_else(|| {
                    ProviderError::Configuration(
                        "OpenAI authentication must resolve to a keychain or environment reference"
                            .into(),
                    )
                })?;
                ProviderConfig::Openai {
                    base_url,
                    api_key_env,
                    capabilities,
                }
            }
            _ => {
                return Err(ProviderError::Configuration(format!(
                    "unsupported provider type `{provider_type}`"
                )))
            }
        };
        self.providers.insert(name.to_owned(), provider);
        let model_id = format!("{name}/{model}");
        self.models.default.get_or_insert_with(|| model_id.clone());
        self.models
            .roles
            .entry("coding_worker".into())
            .or_insert(model_id);
        Ok(())
    }

    pub fn load(path: &Path) -> Result<Self, ProviderError> {
        let mut config: Self = toml::from_str(&fs::read_to_string(path)?)?;
        if config.schema_version == 0 {
            return Err(ProviderError::Configuration(
                "legacy configuration schema 0; run `purrcode config migrate`".into(),
            ));
        }
        config.normalize_provider_urls();
        config.validate()?;
        Ok(config)
    }

    pub fn migration_preview(path: &Path) -> Result<(u32, u32), ProviderError> {
        let document: toml::Value = toml::from_str(&fs::read_to_string(path)?)?;
        let current = document
            .get("schema_version")
            .and_then(toml::Value::as_integer)
            .unwrap_or(0);
        let current = u32::try_from(current).map_err(|_| {
            ProviderError::Configuration("schema_version must be a non-negative integer".into())
        })?;
        if current > 1 {
            return Err(ProviderError::Configuration(format!(
                "configuration schema {current} is newer than this PurrCode build"
            )));
        }
        Ok((current, 1))
    }

    pub fn migrate_file(path: &Path) -> Result<Option<std::path::PathBuf>, ProviderError> {
        let (current, target) = Self::migration_preview(path)?;
        if current == target {
            Self::load(path)?;
            return Ok(None);
        }
        if current != 0 {
            return Err(ProviderError::Configuration(format!(
                "no migration path from configuration schema {current} to {target}"
            )));
        }
        let mut document: toml::Value = toml::from_str(&fs::read_to_string(path)?)?;
        document
            .as_table_mut()
            .ok_or_else(|| {
                ProviderError::Configuration("configuration root must be a table".into())
            })?
            .insert("schema_version".into(), toml::Value::Integer(1));
        let migrated: Self = document.clone().try_into()?;
        migrated.validate()?;
        let backup = path.with_extension(format!(
            "{}.v0.bak",
            path.extension()
                .and_then(|extension| extension.to_str())
                .unwrap_or("toml")
        ));
        if backup.exists() {
            return Err(ProviderError::Configuration(format!(
                "refusing to overwrite migration backup {}",
                backup.display()
            )));
        }
        fs::copy(path, &backup)?;
        if let Err(error) = migrated.save(path) {
            let _ = fs::copy(&backup, path);
            return Err(error);
        }
        Ok(Some(backup))
    }

    pub fn validate(&self) -> Result<(), ProviderError> {
        if self.schema_version != 1 {
            return Err(ProviderError::Configuration(format!(
                "unsupported schema_version {}; expected 1",
                self.schema_version
            )));
        }
        for (name, provider) in &self.providers {
            provider.validate(name)?;
        }
        if let Some(policy) = &self.organization_policy {
            if policy.ed25519_public_key.len() != 64
                || !policy
                    .ed25519_public_key
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit())
            {
                return Err(ProviderError::Configuration(
                    "organization_policy.ed25519_public_key must be 32-byte hex".into(),
                ));
            }
            if !policy.pack.is_file() {
                return Err(ProviderError::Configuration(format!(
                    "organization policy pack does not exist: {}",
                    policy.pack.display()
                )));
            }
        }
        Ok(())
    }

    fn normalize_provider_urls(&mut self) {
        for provider in self.providers.values_mut() {
            if let ProviderConfig::Ollama { base_url, .. } = provider {
                *base_url = normalize_ollama_base_url(base_url.clone());
            }
        }
    }

    pub fn save(&self, path: &Path) -> Result<(), ProviderError> {
        self.validate()?;
        let parent = path.parent().unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(parent)?;
        let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
        use std::io::Write;
        temporary.write_all(toml::to_string_pretty(self)?.as_bytes())?;
        temporary.as_file().sync_all()?;
        temporary
            .persist(path)
            .map_err(|error| ProviderError::Io(error.error))?;
        Ok(())
    }

    pub fn use_keychain_credential(
        &mut self,
        provider: &str,
        credential_name: &str,
    ) -> Result<(), ProviderError> {
        let reference = keychain_reference(credential_name)?;
        match self
            .providers
            .get_mut(provider)
            .ok_or_else(|| ProviderError::UnknownProvider(provider.into()))?
        {
            ProviderConfig::Openai { api_key_env, .. } => *api_key_env = reference,
            ProviderConfig::OpenaiCompatible {
                api_key_env, local, ..
            } => {
                if *local {
                    return Err(ProviderError::Configuration(
                        "local providers do not require an API key".into(),
                    ));
                }
                *api_key_env = Some(reference);
            }
            ProviderConfig::EnterpriseGateway {
                api_key_env,
                credential_command,
                ..
            } => {
                *credential_command = None;
                *api_key_env = Some(reference);
            }
            ProviderConfig::Ollama { .. } => {
                return Err(ProviderError::Configuration(
                    "local providers do not require an API key".into(),
                ));
            }
        }
        Ok(())
    }

    pub fn register_model(&mut self, model: &ModelId) -> Result<(), ProviderError> {
        let provider = self
            .providers
            .get_mut(&model.provider)
            .ok_or_else(|| ProviderError::UnknownProvider(model.provider.clone()))?;
        let capabilities = ModelCapabilities::unknown(provider.is_local());
        match provider {
            ProviderConfig::Openai {
                capabilities: models,
                ..
            }
            | ProviderConfig::OpenaiCompatible {
                capabilities: models,
                ..
            }
            | ProviderConfig::Ollama {
                capabilities: models,
                ..
            }
            | ProviderConfig::EnterpriseGateway {
                capabilities: models,
                ..
            } => {
                models.entry(model.model.clone()).or_insert(capabilities);
            }
        }
        Ok(())
    }

    pub fn assign_model_role(&mut self, role: &str, model: &ModelId) -> Result<(), ProviderError> {
        if !matches!(
            role,
            "coding_worker"
                | "judge"
                | "planner"
                | "reviewer"
                | "summarizer"
                | "utility"
                | "embedding"
        ) {
            return Err(ProviderError::Configuration(format!(
                "unsupported model role `{role}`"
            )));
        }
        self.register_model(model)?;
        self.models.roles.insert(
            role.to_owned(),
            format!("{}/{}", model.provider, model.model),
        );
        Ok(())
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct PrivacyConfig {
    #[serde(default)]
    pub mode: PrivacyMode,
    #[serde(default)]
    pub allow_remote_fallback: bool,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum PrivacyMode {
    #[default]
    LocalOnly,
    Mixed,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct ModelsConfig {
    pub default: Option<String>,
    #[serde(default)]
    pub roles: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum ProviderConfig {
    Openai {
        #[serde(default = "openai_base_url")]
        base_url: Url,
        #[serde(default = "openai_key_env")]
        api_key_env: String,
        #[serde(default)]
        capabilities: BTreeMap<String, ModelCapabilities>,
    },
    OpenaiCompatible {
        base_url: Url,
        api_key_env: Option<String>,
        #[serde(default)]
        local: bool,
        #[serde(default)]
        headers: BTreeMap<String, String>,
        #[serde(default)]
        capabilities: BTreeMap<String, ModelCapabilities>,
    },
    Ollama {
        #[serde(default = "ollama_base_url")]
        base_url: Url,
        #[serde(default)]
        capabilities: BTreeMap<String, ModelCapabilities>,
    },
    EnterpriseGateway {
        base_url: Url,
        api_key_env: Option<String>,
        credential_command: Option<Vec<String>>,
        #[serde(default)]
        headers: BTreeMap<String, String>,
        #[serde(default)]
        header_env: BTreeMap<String, String>,
        identity_pem: Option<std::path::PathBuf>,
        ca_pem: Option<std::path::PathBuf>,
        proxy_url: Option<Url>,
        #[serde(default)]
        capabilities: BTreeMap<String, ModelCapabilities>,
    },
}

impl ProviderConfig {
    fn validate(&self, name: &str) -> Result<(), ProviderError> {
        let (url, local) = match self {
            Self::Openai { base_url, .. } => (base_url, false),
            Self::OpenaiCompatible {
                base_url, local, ..
            } => (base_url, *local),
            Self::Ollama { base_url, .. } => (base_url, true),
            Self::EnterpriseGateway { base_url, .. } => (base_url, false),
        };
        if local && !is_loopback_url(url) {
            return Err(ProviderError::Configuration(format!(
                "provider `{name}` is marked local but URL is not loopback: {url}"
            )));
        }
        if url.scheme() != "https" && !is_loopback_url(url) {
            return Err(ProviderError::Configuration(format!(
                "remote provider `{name}` must use HTTPS"
            )));
        }
        Ok(())
    }

    pub fn is_local(&self) -> bool {
        match self {
            Self::Openai { .. } => false,
            Self::OpenaiCompatible { local, .. } => *local,
            Self::Ollama { .. } => true,
            Self::EnterpriseGateway { .. } => false,
        }
    }

    pub fn configured_models(&self) -> &BTreeMap<String, ModelCapabilities> {
        match self {
            Self::Openai { capabilities, .. }
            | Self::OpenaiCompatible { capabilities, .. }
            | Self::Ollama { capabilities, .. }
            | Self::EnterpriseGateway { capabilities, .. } => capabilities,
        }
    }
}

fn is_loopback_url(url: &Url) -> bool {
    matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "::1"))
}

fn openai_base_url() -> Url {
    Url::parse("https://api.openai.com/v1/").expect("static OpenAI URL is valid")
}
fn ollama_base_url() -> Url {
    Url::parse("http://127.0.0.1:11434/").expect("static Ollama URL is valid")
}

fn normalize_ollama_base_url(mut url: Url) -> Url {
    let trimmed = url.path().trim_end_matches('/');
    let native_path = trimmed.strip_suffix("/v1").unwrap_or(trimmed);
    let normalized = if native_path.is_empty() {
        "/".to_owned()
    } else {
        format!("{native_path}/")
    };
    url.set_path(&normalized);
    url.set_query(None);
    url.set_fragment(None);
    url
}
fn openai_key_env() -> String {
    "OPENAI_API_KEY".into()
}

async fn run_credential_command(command: &[String]) -> Result<String, ProviderError> {
    let (program, arguments) = command
        .split_first()
        .ok_or_else(|| ProviderError::Configuration("credential_command cannot be empty".into()))?;
    let mut child = Command::new(program)
        .args(arguments)
        .env_clear()
        .env("PATH", env::var_os("PATH").unwrap_or_default())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|error| ProviderError::CredentialCommand(error.to_string()))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| ProviderError::CredentialCommand("stdout was unavailable".into()))?;
    let output = tokio::time::timeout(Duration::from_secs(10), async {
        let mut bounded = Vec::new();
        stdout
            .take(8193)
            .read_to_end(&mut bounded)
            .await
            .map(|_| bounded)
    })
    .await
    .map_err(|_| ProviderError::CredentialCommand("timed out".into()))??;
    let status = tokio::time::timeout(Duration::from_secs(1), child.wait())
        .await
        .map_err(|_| ProviderError::CredentialCommand("did not exit".into()))??;
    if !status.success() {
        return Err(ProviderError::CredentialCommand(format!(
            "exited with {status}"
        )));
    }
    if output.len() > 8192 {
        return Err(ProviderError::CredentialCommand(
            "output exceeded 8192 bytes".into(),
        ));
    }
    let credential = String::from_utf8(output)
        .map_err(|_| ProviderError::CredentialCommand("output was not UTF-8".into()))?;
    let credential = credential.trim();
    if credential.is_empty() || credential.contains(['\r', '\n']) {
        return Err(ProviderError::CredentialCommand(
            "output must contain exactly one non-empty line".into(),
        ));
    }
    Ok(credential.to_owned())
}

pub struct ProviderRouter {
    privacy: PrivacyConfig,
    providers: BTreeMap<String, Arc<dyn ModelProvider>>,
    local: BTreeMap<String, bool>,
}

impl ProviderRouter {
    pub fn from_config(config: &AppConfig) -> Result<Self, ProviderError> {
        let mut providers = BTreeMap::new();
        let mut local = BTreeMap::new();
        for (name, provider_config) in &config.providers {
            let provider = Arc::new(HttpProvider::from_config(
                name.clone(),
                provider_config.clone(),
            )?) as Arc<dyn ModelProvider>;
            local.insert(name.clone(), provider_config.is_local());
            providers.insert(name.clone(), provider);
        }
        Ok(Self {
            privacy: config.privacy.clone(),
            providers,
            local,
        })
    }

    pub fn provider(&self, model: &ModelId) -> Result<Arc<dyn ModelProvider>, ProviderError> {
        let is_local = self
            .local
            .get(&model.provider)
            .ok_or_else(|| ProviderError::UnknownProvider(model.provider.clone()))?;
        if self.privacy.mode == PrivacyMode::LocalOnly && !is_local {
            return Err(ProviderError::RemoteProviderDenied);
        }
        self.providers
            .get(&model.provider)
            .cloned()
            .ok_or_else(|| ProviderError::UnknownProvider(model.provider.clone()))
    }
}

pub struct HttpProvider {
    name: String,
    base_url: Url,
    api_key_env: Option<String>,
    credential_command: Option<Vec<String>>,
    header_env: BTreeMap<String, String>,
    headers: HeaderMap,
    local: bool,
    capabilities: BTreeMap<String, ModelCapabilities>,
    client: reqwest::Client,
    api_mode: ProviderApiMode,
}

impl HttpProvider {
    fn from_config(name: String, config: ProviderConfig) -> Result<Self, ProviderError> {
        let (
            base_url,
            api_key_env,
            credential_command,
            local,
            raw_headers,
            header_env,
            identity_pem,
            ca_pem,
            proxy_url,
            capabilities,
            api_mode,
        ) = match config {
            ProviderConfig::Openai {
                base_url,
                api_key_env,
                capabilities,
            } => (
                base_url,
                Some(api_key_env),
                None,
                false,
                BTreeMap::new(),
                BTreeMap::new(),
                None,
                None,
                None,
                capabilities,
                ProviderApiMode::Responses,
            ),
            ProviderConfig::OpenaiCompatible {
                base_url,
                api_key_env,
                local,
                headers,
                capabilities,
            } => (
                base_url,
                api_key_env,
                None,
                local,
                headers,
                BTreeMap::new(),
                None,
                None,
                None,
                capabilities,
                ProviderApiMode::OpenaiCompatible,
            ),
            ProviderConfig::Ollama {
                base_url,
                capabilities,
            } => (
                normalize_ollama_base_url(base_url),
                None,
                None,
                true,
                BTreeMap::new(),
                BTreeMap::new(),
                None,
                None,
                None,
                capabilities,
                ProviderApiMode::OllamaNative,
            ),
            ProviderConfig::EnterpriseGateway {
                base_url,
                api_key_env,
                credential_command,
                headers,
                header_env,
                identity_pem,
                ca_pem,
                proxy_url,
                capabilities,
            } => (
                base_url,
                api_key_env,
                credential_command,
                false,
                headers,
                header_env,
                identity_pem,
                ca_pem,
                proxy_url,
                capabilities,
                ProviderApiMode::Responses,
            ),
        };
        let mut headers = HeaderMap::new();
        for (key, value) in raw_headers {
            let name = HeaderName::from_bytes(key.as_bytes())
                .map_err(|error| ProviderError::Configuration(error.to_string()))?;
            let mut value = HeaderValue::from_str(&value)
                .map_err(|error| ProviderError::Configuration(error.to_string()))?;
            value.set_sensitive(true);
            headers.insert(name, value);
        }
        if api_key_env.is_some() && credential_command.is_some() {
            return Err(ProviderError::Configuration(
                "configure only one of api_key_env or credential_command".into(),
            ));
        }
        if matches!(credential_command.as_deref(), Some([])) {
            return Err(ProviderError::Configuration(
                "credential_command cannot be empty".into(),
            ));
        }
        let mut client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(300));
        if let Some(path) = identity_pem {
            client = client.identity(reqwest::Identity::from_pem(&fs::read(path)?)?);
        }
        if let Some(path) = ca_pem {
            client = client.add_root_certificate(reqwest::Certificate::from_pem(&fs::read(path)?)?);
        }
        if let Some(proxy) = proxy_url {
            client = client.proxy(reqwest::Proxy::all(proxy.as_str())?);
        }
        let client = client.build()?;
        Ok(Self {
            name,
            base_url,
            api_key_env,
            credential_command,
            header_env,
            headers,
            local,
            capabilities,
            client,
            api_mode,
        })
    }

    fn endpoint(&self, path: &str) -> Result<Url, ProviderError> {
        let mut base_url = self.base_url.clone();
        if !base_url.path().ends_with('/') {
            let normalized = format!("{}/", base_url.path());
            base_url.set_path(&normalized);
        }
        base_url
            .join(path)
            .map_err(|error| ProviderError::Configuration(error.to_string()))
    }

    async fn request(
        &self,
        method: reqwest::Method,
        url: Url,
    ) -> Result<reqwest::RequestBuilder, ProviderError> {
        let mut request = self
            .client
            .request(method, url)
            .header(CONTENT_TYPE, "application/json")
            .headers(self.headers.clone());
        for (header, variable) in &self.header_env {
            let name = HeaderName::from_bytes(header.as_bytes())
                .map_err(|error| ProviderError::Configuration(error.to_string()))?;
            let raw = resolve_credential(variable)?;
            let mut value = HeaderValue::from_str(&raw)
                .map_err(|_| ProviderError::InvalidCredential(variable.clone()))?;
            value.set_sensitive(true);
            request = request.header(name, value);
        }
        let credential = if let Some(variable) = &self.api_key_env {
            let key = resolve_credential(variable)?;
            Some((variable.clone(), key))
        } else if let Some(command) = &self.credential_command {
            Some((
                "credential_command".into(),
                run_credential_command(command).await?,
            ))
        } else {
            None
        };
        if let Some((source, credential)) = credential {
            let mut value = HeaderValue::from_str(&format!("Bearer {credential}"))
                .map_err(|_| ProviderError::InvalidCredential(source))?;
            value.set_sensitive(true);
            request = request.header(AUTHORIZATION, value);
        }
        Ok(request)
    }

    async fn send_with_retry(
        &self,
        method: reqwest::Method,
        url: Url,
        body: Option<&Value>,
        idempotency_key: Option<&str>,
    ) -> Result<reqwest::Response, ProviderError> {
        let encoded_body = body
            .map(|body| encode_bounded_request(body, self.api_mode))
            .transpose()?;
        let mut last_transport = None;
        for attempt in 0..3_u32 {
            let mut request = self.request(method.clone(), url.clone()).await?;
            if let Some(body) = &encoded_body {
                request = request.body(body.clone());
            }
            if let Some(key) = idempotency_key {
                request = request.header("Idempotency-Key", key);
            }
            match request.send().await {
                Ok(response)
                    if response.status().is_server_error()
                        || response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS =>
                {
                    if attempt == 2 {
                        return Err(self.http_failure(response).await);
                    }
                    let retry_after = response
                        .headers()
                        .get(RETRY_AFTER)
                        .and_then(|value| value.to_str().ok())
                        .and_then(|value| value.parse::<u64>().ok())
                        .map(Duration::from_secs)
                        .unwrap_or_else(|| Duration::from_millis(100 * 2_u64.pow(attempt)));
                    tokio::time::sleep(retry_after.min(Duration::from_secs(2))).await;
                }
                Ok(response) => return Ok(response),
                Err(error) if error.is_connect() || error.is_timeout() => {
                    last_transport = Some(error);
                    if attempt < 2 {
                        tokio::time::sleep(Duration::from_millis(100 * 2_u64.pow(attempt))).await;
                    }
                }
                Err(error) => {
                    return Err(ProviderError::Diagnostic(transport_diagnostic(
                        &error,
                        self.api_mode,
                    )))
                }
            }
        }
        match last_transport {
            Some(error) => Err(ProviderError::Diagnostic(transport_diagnostic(
                &error,
                self.api_mode,
            ))),
            None => Err(ProviderError::Unavailable(
                "retry attempts were exhausted without a response".into(),
            )),
        }
    }

    async fn http_failure(&self, response: reqwest::Response) -> ProviderError {
        bounded_http_failure(response, self.api_mode).await
    }

    fn response_body(
        &self,
        request: &ModelRequest,
        stream: bool,
        schema: Option<RootSchema>,
    ) -> Value {
        let messages: Vec<Value> = request
            .messages
            .iter()
            .map(|message| json!({"role": message.role, "content": message.content}))
            .collect();
        match self.api_mode {
            ProviderApiMode::OpenaiCompatible => {
                let mut body = json!({
                    "model": request.model.model,
                    "messages": messages,
                    "stream": stream,
                });
                if let Some(maximum) = request.max_output_tokens {
                    body["max_tokens"] = json!(maximum);
                }
                if schema.is_some() {
                    body["response_format"] = json!({
                        "type": "json_object"
                    });
                }
                body
            }
            ProviderApiMode::Responses => {
                let mut body = json!({
                    "model": request.model.model,
                    "input": messages,
                    "tools": request.tools,
                    "stream": stream,
                    "store": false
                });
                if let Some(maximum) = request.max_output_tokens {
                    body["max_output_tokens"] = json!(maximum);
                }
                if let Some(effort) = &request.reasoning_effort {
                    body["reasoning"] = json!({"effort": effort});
                }
                if let Some(schema) = schema {
                    let schema = serde_json::to_value(schema)
                        .expect("JSON Schema serialization is infallible");
                    body["text"] = json!({
                        "format": {
                            "type": "json_schema",
                            "name": "purrcode_result",
                            "strict": true,
                            "schema": schema
                        }
                    });
                }
                body
            }
            ProviderApiMode::OllamaNative => {
                let mut body = json!({
                    "model": request.model.model,
                    "messages": messages,
                    "stream": stream,
                });
                if !request.tools.is_empty() {
                    body["tools"] = json!(request.tools);
                }
                if let Some(maximum) = request.max_output_tokens {
                    body["options"] = json!({"num_predict": maximum});
                }
                if let Some(schema) = schema {
                    body["format"] = serde_json::to_value(schema)
                        .expect("JSON Schema serialization is infallible");
                }
                body
            }
        }
    }
}

#[async_trait]
impl ModelProvider for HttpProvider {
    async fn capabilities(&self, model: &ModelId) -> Result<ModelCapabilities, ProviderError> {
        if model.provider != self.name {
            return Err(ProviderError::UnknownProvider(model.provider.clone()));
        }
        Ok(self
            .capabilities
            .get(&model.model)
            .cloned()
            .unwrap_or_else(|| ModelCapabilities::unknown(self.local)))
    }

    async fn stream(&self, request: ModelRequest) -> Result<ModelEventStream, ProviderError> {
        let body = self.response_body(&request, true, None);
        let idempotency_key = Uuid::new_v4().to_string();
        let endpoint = match self.api_mode {
            ProviderApiMode::Responses => "responses",
            ProviderApiMode::OpenaiCompatible => "chat/completions",
            ProviderApiMode::OllamaNative => "api/chat",
        };
        let response = self
            .send_with_retry(
                reqwest::Method::POST,
                self.endpoint(endpoint)?,
                Some(&body),
                (self.api_mode != ProviderApiMode::OllamaNative)
                    .then_some(idempotency_key.as_str()),
            )
            .await?;
        let status = response.status();
        if !status.is_success() {
            return Err(self.http_failure(response).await);
        }
        match self.api_mode {
            ProviderApiMode::OllamaNative => {
                ensure_content_type(
                    &response,
                    &[
                        "application/x-ndjson",
                        "application/ndjson",
                        "application/json",
                    ],
                    "application/x-ndjson",
                    true,
                    self.api_mode,
                )?;
                Ok(ollama_native_stream(response, self.api_mode))
            }
            ProviderApiMode::Responses | ProviderApiMode::OpenaiCompatible => {
                ensure_content_type(
                    &response,
                    &["text/event-stream"],
                    "text/event-stream",
                    true,
                    self.api_mode,
                )?;
                Ok(openai_event_stream(response, self.api_mode))
            }
        }
    }

    async fn structured(
        &self,
        request: ModelRequest,
        schema: RootSchema,
    ) -> Result<Value, ProviderError> {
        let body = self.response_body(&request, false, Some(schema));
        let idempotency_key = Uuid::new_v4().to_string();
        let endpoint = match self.api_mode {
            ProviderApiMode::Responses => "responses",
            ProviderApiMode::OpenaiCompatible => "chat/completions",
            ProviderApiMode::OllamaNative => "api/chat",
        };
        let response = self
            .send_with_retry(
                reqwest::Method::POST,
                self.endpoint(endpoint)?,
                Some(&body),
                (self.api_mode != ProviderApiMode::OllamaNative)
                    .then_some(idempotency_key.as_str()),
            )
            .await?;
        if !response.status().is_success() {
            return Err(self.http_failure(response).await);
        }
        ensure_content_type(
            &response,
            &["application/json"],
            "application/json",
            false,
            self.api_mode,
        )?;
        let bytes =
            read_bounded_body(response, MAX_PROVIDER_HTTP_BODY_BYTES, self.api_mode).await?;
        let value = parse_json_body(&bytes, self.api_mode)?;
        match self.api_mode {
            ProviderApiMode::Responses => extract_output_json(value, self.api_mode),
            ProviderApiMode::OpenaiCompatible => extract_chat_output(value, self.api_mode),
            ProviderApiMode::OllamaNative => extract_ollama_output(value, self.api_mode),
        }
    }

    async fn structured_stream(
        &self,
        request: ModelRequest,
        schema: RootSchema,
    ) -> Result<ProviderEventStream, ProviderError> {
        let body = self.response_body(&request, true, Some(schema));
        let idempotency_key = Uuid::new_v4().to_string();
        let endpoint = match self.api_mode {
            ProviderApiMode::Responses => "responses",
            ProviderApiMode::OpenaiCompatible => "chat/completions",
            ProviderApiMode::OllamaNative => "api/chat",
        };
        let response = self
            .send_with_retry(
                reqwest::Method::POST,
                self.endpoint(endpoint)?,
                Some(&body),
                (self.api_mode != ProviderApiMode::OllamaNative)
                    .then_some(idempotency_key.as_str()),
            )
            .await?;
        if !response.status().is_success() {
            return Err(self.http_failure(response).await);
        }
        match self.api_mode {
            ProviderApiMode::OllamaNative => {
                ensure_content_type(
                    &response,
                    &[
                        "application/x-ndjson",
                        "application/ndjson",
                        "application/json",
                    ],
                    "application/x-ndjson",
                    true,
                    self.api_mode,
                )?;
                Ok(ollama_provider_stream(response, self.api_mode))
            }
            ProviderApiMode::Responses | ProviderApiMode::OpenaiCompatible => {
                ensure_content_type(
                    &response,
                    &["text/event-stream"],
                    "text/event-stream",
                    true,
                    self.api_mode,
                )?;
                Ok(openai_provider_stream(response, self.api_mode))
            }
        }
    }

    async fn count_tokens(&self, request: &ModelRequest) -> Result<TokenEstimate, ProviderError> {
        let characters: usize = request
            .messages
            .iter()
            .map(|message| message.content.chars().count())
            .sum();
        Ok(TokenEstimate {
            tokens: characters.div_ceil(4) as u64,
            exact: false,
        })
    }

    async fn health_check(&self) -> Result<ProviderHealth, ProviderError> {
        let endpoint = match self.api_mode {
            ProviderApiMode::OllamaNative => "api/version",
            ProviderApiMode::Responses | ProviderApiMode::OpenaiCompatible => "models",
        };
        let response = self
            .send_with_retry(reqwest::Method::GET, self.endpoint(endpoint)?, None, None)
            .await?;
        Ok(ProviderHealth {
            available: response.status().is_success(),
            detail: format!("{} returned HTTP {}", self.name, response.status()),
        })
    }
}

#[derive(Debug, Error)]
pub enum ProviderError {
    #[error("{0}")]
    Diagnostic(ProviderDiagnostic),
    #[error("provider is unavailable: {0}")]
    Unavailable(String),
    #[error("provider response was invalid: {0}")]
    InvalidResponse(String),
    #[error("operation violates local-only routing policy")]
    RemoteProviderDenied,
    #[error("unknown provider `{0}`")]
    UnknownProvider(String),
    #[error("credential environment variable `{0}` is not set")]
    MissingCredential(String),
    #[error("credential environment variable `{0}` is not a valid HTTP credential")]
    InvalidCredential(String),
    #[error("external credential command failed: {0}")]
    CredentialCommand(String),
    #[error("OS credential store failed: {0}")]
    Keychain(String),
    #[error("provider configuration is invalid: {0}")]
    Configuration(String),
    #[error("provider returned HTTP {status}: {body}")]
    HttpStatus { status: u16, body: String },
    #[error("HTTP transport failed")]
    Transport(#[from] reqwest::Error),
    #[error("configuration file could not be read: {0}")]
    Io(#[from] std::io::Error),
    #[error("configuration file could not be parsed: {0}")]
    Toml(#[from] toml::de::Error),
    #[error("configuration file could not be encoded: {0}")]
    TomlEncode(#[from] toml::ser::Error),
    #[error("JSON could not be parsed: {0}")]
    Json(#[from] serde_json::Error),
}

impl ProviderError {
    pub fn diagnostic(&self) -> Option<&ProviderDiagnostic> {
        match self {
            Self::Diagnostic(diagnostic) => Some(diagnostic),
            _ => None,
        }
    }

    pub fn category(&self) -> Option<ProviderErrorCategory> {
        match self {
            Self::Diagnostic(diagnostic) => Some(diagnostic.category),
            Self::HttpStatus { .. } => Some(ProviderErrorCategory::HttpStatus),
            Self::MissingCredential(_) | Self::InvalidCredential(_) => {
                Some(ProviderErrorCategory::Authentication)
            }
            Self::InvalidResponse(_) | Self::Json(_) => Some(ProviderErrorCategory::Schema),
            Self::Transport(error) => {
                Some(transport_diagnostic(error, ProviderApiMode::Responses).category)
            }
            _ => None,
        }
    }

    pub fn cancelled(reason: &str, api_mode: ProviderApiMode) -> Self {
        Self::Diagnostic(diagnostics::cancelled_diagnostic(reason, api_mode))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt as _;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::io::AsyncWriteExt as _;

    #[test]
    fn legacy_ollama_v1_base_is_normalized_to_the_native_api_root() {
        let provider = HttpProvider::from_config(
            "ollama".into(),
            ProviderConfig::Ollama {
                base_url: Url::parse("http://127.0.0.1:11434/v1").unwrap(),
                capabilities: BTreeMap::new(),
            },
        )
        .unwrap();
        assert_eq!(
            provider.endpoint("api/chat").unwrap().as_str(),
            "http://127.0.0.1:11434/api/chat"
        );
        assert_eq!(provider.api_mode, ProviderApiMode::OllamaNative);
    }

    #[test]
    fn explicit_openai_compatible_provider_retains_its_v1_base() {
        let provider = HttpProvider::from_config(
            "compatible".into(),
            ProviderConfig::OpenaiCompatible {
                base_url: Url::parse("http://127.0.0.1:1234/v1").unwrap(),
                api_key_env: None,
                local: true,
                headers: BTreeMap::new(),
                capabilities: BTreeMap::new(),
            },
        )
        .unwrap();
        assert_eq!(
            provider.endpoint("chat/completions").unwrap().as_str(),
            "http://127.0.0.1:1234/v1/chat/completions"
        );
        assert_eq!(provider.api_mode, ProviderApiMode::OpenaiCompatible);
    }

    #[test]
    fn compatibility_stream_chunking_preserves_utf8_and_frame_bounds() {
        let value = format!("{}猫{}", "a".repeat(31), "b".repeat(31));
        let chunks = split_bounded_utf8(value.clone(), 32);
        assert!(chunks.iter().all(|chunk| chunk.len() <= 32));
        assert_eq!(chunks.concat(), value);
    }

    #[test]
    fn responses_request_keeps_nested_schema_definitions() {
        #[allow(dead_code)]
        #[derive(schemars::JsonSchema)]
        struct NestedEnvelope {
            action: NestedAction,
        }

        #[allow(dead_code)]
        #[derive(schemars::JsonSchema)]
        enum NestedAction {
            Write { path: String },
        }

        let provider = HttpProvider::from_config(
            "openai".into(),
            ProviderConfig::Openai {
                base_url: Url::parse("https://api.openai.com/v1/").unwrap(),
                api_key_env: "OPENAI_API_KEY".into(),
                capabilities: BTreeMap::new(),
            },
        )
        .unwrap();
        let body = provider.response_body(
            &test_request("openai"),
            true,
            Some(schemars::schema_for!(NestedEnvelope)),
        );
        let schema = &body["text"]["format"]["schema"];
        assert!(schema["definitions"]
            .as_object()
            .is_some_and(|definitions| !definitions.is_empty()));
        assert!(serde_json::to_string(schema)
            .unwrap()
            .contains("#/definitions/"));
    }

    #[test]
    fn local_provider_must_be_loopback() {
        let config: AppConfig = toml::from_str(
            r#"
                schema_version = 1
                [providers.bad]
                type = "openai-compatible"
                base_url = "https://example.com/v1/"
                local = true
            "#,
        )
        .unwrap();
        assert!(config.validate().is_err());
    }

    #[test]
    fn local_only_router_denies_remote_provider() {
        let config: AppConfig = toml::from_str(
            r#"
                schema_version = 1
                [privacy]
                mode = "local-only"
                allow_remote_fallback = false
                [providers.openai]
                type = "openai"
            "#,
        )
        .unwrap();
        config.validate().unwrap();
        let router = ProviderRouter::from_config(&config).unwrap();
        let model = ModelId::parse("openai/gpt-test").unwrap();
        assert!(matches!(
            router.provider(&model),
            Err(ProviderError::RemoteProviderDenied)
        ));
    }

    #[test]
    fn provider_configuration_stores_only_a_keychain_reference() {
        let mut config: AppConfig = toml::from_str(
            r#"
                schema_version = 1
                [privacy]
                mode = "mixed"
                [providers.openai]
                type = "openai"
            "#,
        )
        .unwrap();
        config
            .use_keychain_credential("openai", "primary-openai")
            .unwrap();
        let encoded = toml::to_string(&config).unwrap();
        assert!(encoded.contains("keychain:primary-openai"));
        assert!(!encoded.contains("sk-"));
    }

    #[test]
    fn credential_names_are_strictly_bounded() {
        assert!(keychain_reference("openai.primary-1").is_ok());
        assert!(keychain_reference("../unsafe").is_err());
        assert!(keychain_reference("").is_err());
    }

    #[test]
    fn credential_references_are_typed_and_canonical() {
        assert_eq!(
            validate_credential_reference("keychain:openai.primary-1").unwrap(),
            "keychain:openai.primary-1"
        );
        assert_eq!(
            validate_credential_reference("PROVIDER_API_KEY").unwrap(),
            "PROVIDER_API_KEY"
        );
        assert!(validate_credential_reference("sk-secret-value").is_err());
        assert!(validate_credential_reference("provider_api_key").is_err());
        assert!(validate_credential_reference("keychain:../unsafe").is_err());
    }

    #[test]
    fn openai_configuration_requires_a_resolved_reference() {
        let mut config: AppConfig = toml::from_str(
            r#"
                schema_version = 1
                [privacy]
                mode = "mixed"
            "#,
        )
        .unwrap();
        let result = config.configure_provider_with_reference(
            "openai",
            "openai",
            "https://api.openai.com/v1",
            "gpt-test",
            None,
        );
        assert!(matches!(result, Err(ProviderError::Configuration(_))));
        assert!(config.providers.is_empty());
    }

    #[test]
    fn legacy_configuration_migration_is_validated_and_recoverable() {
        let temporary = tempfile::tempdir().unwrap();
        let path = temporary.path().join("purrcode.toml");
        std::fs::write(
            &path,
            r#"
                [privacy]
                mode = "local-only"
            "#,
        )
        .unwrap();
        assert_eq!(AppConfig::migration_preview(&path).unwrap(), (0, 1));
        let backup = AppConfig::migrate_file(&path).unwrap().unwrap();
        assert!(backup.is_file());
        assert!(!std::fs::read_to_string(&backup)
            .unwrap()
            .contains("schema_version"));
        assert_eq!(AppConfig::load(&path).unwrap().schema_version, 1);
        assert!(AppConfig::migrate_file(&path).unwrap().is_none());
    }

    #[test]
    fn streaming_text_delta_is_parsed() {
        let event =
            parse_response_event(r#"{"type":"response.output_text.delta","delta":"hello"}"#)
                .unwrap();
        assert_eq!(event, Some(ModelEvent::TextDelta("hello".into())));
    }

    #[test]
    fn chat_event_text_delta_is_parsed() {
        let event = parse_chat_event(
            r#"{"choices":[{"index":0,"delta":{"content":"hello"},"finish_reason":null}]}"#,
        )
        .unwrap();
        assert_eq!(event, Some(ModelEvent::TextDelta("hello".into())));
    }

    #[test]
    fn chat_event_finish_is_parsed() {
        let event = parse_chat_event(r#"{"choices":[{"index":0,"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":5,"completion_tokens":3}}"#)
            .unwrap();
        assert_eq!(
            event,
            Some(ModelEvent::Usage {
                input_tokens: 5,
                output_tokens: 3
            })
        );
    }

    #[test]
    fn chat_event_done_is_skipped() {
        let event = parse_chat_event(
            r#"{"choices":[{"index":0,"delta":{"content":""},"finish_reason":"stop"}]}"#,
        )
        .unwrap();
        assert_eq!(event, Some(ModelEvent::Finished));
    }

    #[test]
    fn chat_structured_output_is_extracted() {
        let response = json!({
            "choices": [{
                "index": 0,
                "message": {"role": "assistant", "content": "{\"ok\":true}"}
            }]
        });
        assert_eq!(
            extract_chat_output(response, ProviderApiMode::OpenaiCompatible).unwrap(),
            json!({"ok": true})
        );
    }

    #[test]
    fn structured_output_is_extracted() {
        let response = json!({
            "output": [{
                "type": "message",
                "content": [{"type": "output_text", "text": "{\"ok\":true}"}]
            }]
        });
        assert_eq!(
            extract_output_json(response, ProviderApiMode::Responses).unwrap(),
            json!({"ok": true})
        );
    }

    fn test_request(provider: &str) -> ModelRequest {
        ModelRequest {
            model: ModelId {
                provider: provider.into(),
                model: "fixture-model".into(),
            },
            messages: vec![ModelMessage {
                role: "user".into(),
                content: "Return JSON.".into(),
            }],
            tools: Vec::new(),
            max_output_tokens: Some(64),
            reasoning_effort: None,
        }
    }

    fn ollama_provider(base_url: Url) -> HttpProvider {
        HttpProvider::from_config(
            "ollama".into(),
            ProviderConfig::Ollama {
                base_url,
                capabilities: BTreeMap::new(),
            },
        )
        .unwrap()
    }

    async fn fake_http_server(
        status: &'static str,
        content_type: &'static str,
        body: Vec<u8>,
    ) -> (Url, tokio::task::JoinHandle<Vec<u8>>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let request = read_http_request(&mut socket).await;
            let headers = format!(
                "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = socket.write_all(headers.as_bytes()).await;
            let _ = socket.write_all(&body).await;
            request
        });
        (Url::parse(&format!("http://{address}/")).unwrap(), server)
    }

    async fn read_http_request(socket: &mut tokio::net::TcpStream) -> Vec<u8> {
        let mut request = Vec::new();
        let mut buffer = [0_u8; 4096];
        let (header_end, content_length) = loop {
            let read = socket.read(&mut buffer).await.unwrap();
            if read == 0 {
                return request;
            }
            request.extend_from_slice(&buffer[..read]);
            if let Some(header_end) = request
                .windows(4)
                .position(|window| window == b"\r\n\r\n")
                .map(|position| position + 4)
            {
                let headers = String::from_utf8_lossy(&request[..header_end]);
                let content_length = headers
                    .lines()
                    .find_map(|line| {
                        line.split_once(':').and_then(|(name, value)| {
                            name.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse::<usize>().ok())
                                .flatten()
                        })
                    })
                    .unwrap_or(0);
                break (header_end, content_length);
            }
        };
        while request.len() < header_end.saturating_add(content_length) {
            let read = socket.read(&mut buffer).await.unwrap();
            if read == 0 {
                break;
            }
            request.extend_from_slice(&buffer[..read]);
        }
        request
    }

    fn request_body(request: &[u8]) -> Value {
        let body_start = request
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
            .map(|position| position + 4)
            .unwrap();
        serde_json::from_slice(&request[body_start..]).unwrap()
    }

    #[tokio::test]
    async fn ollama_native_structured_request_uses_api_chat_and_native_schema() {
        let response =
            br#"{"message":{"role":"assistant","content":"{\"ok\":true}"},"done":true}"#.to_vec();
        let (base_url, server) =
            fake_http_server("200 OK", "application/json; charset=utf-8", response).await;
        let provider = ollama_provider(base_url.join("v1").unwrap());

        let value = provider
            .structured(
                test_request("ollama"),
                schemars::schema_for!(QualificationAnswer),
            )
            .await
            .unwrap();
        assert_eq!(value, json!({"ok": true}));

        let request = server.await.unwrap();
        let rendered = String::from_utf8_lossy(&request);
        assert!(rendered.starts_with("POST /api/chat HTTP/1.1\r\n"));
        let body = request_body(&request);
        assert_eq!(body["model"], "fixture-model");
        assert_eq!(body["stream"], false);
        assert_eq!(body["messages"][0]["role"], "user");
        assert_eq!(body["options"]["num_predict"], 64);
        assert_eq!(body["format"]["type"], "object");
        assert!(body.get("response_format").is_none());
    }

    #[tokio::test]
    async fn openai_compatible_mode_remains_explicit_and_uses_chat_completions() {
        let response = br#"{"choices":[{"message":{"content":"{\"ok\":true}"}}]}"#.to_vec();
        let (base_url, server) = fake_http_server("200 OK", "application/json", response).await;
        let provider = HttpProvider::from_config(
            "compatible".into(),
            ProviderConfig::OpenaiCompatible {
                base_url: base_url.join("v1/").unwrap(),
                api_key_env: None,
                local: true,
                headers: BTreeMap::new(),
                capabilities: BTreeMap::new(),
            },
        )
        .unwrap();

        let value = provider
            .structured(
                test_request("compatible"),
                schemars::schema_for!(QualificationAnswer),
            )
            .await
            .unwrap();
        assert_eq!(value, json!({"ok": true}));

        let request = server.await.unwrap();
        let rendered = String::from_utf8_lossy(&request);
        assert!(rendered.starts_with("POST /v1/chat/completions HTTP/1.1\r\n"));
        let body = request_body(&request);
        assert_eq!(body["stream"], false);
        assert_eq!(body["response_format"]["type"], "json_object");
        assert!(body.get("format").is_none());
    }

    #[tokio::test]
    async fn ollama_native_ndjson_stream_yields_text_tools_usage_and_finish() {
        let response = concat!(
            "{\"message\":{\"role\":\"assistant\",\"content\":\"hel\"},\"done\":false}\n",
            "{\"message\":{\"role\":\"assistant\",\"content\":\"lo\",\"tool_calls\":[{\"function\":{\"name\":\"read_file\",\"arguments\":{\"path\":\"README.md\"}}}]},\"done\":false}\n",
            "{\"message\":{\"role\":\"assistant\",\"content\":\"\"},\"done\":true,\"prompt_eval_count\":7,\"eval_count\":3}\n"
        )
        .as_bytes()
        .to_vec();
        let (base_url, server) = fake_http_server("200 OK", "application/x-ndjson", response).await;
        let provider = ollama_provider(base_url);
        let mut request = test_request("ollama");
        request.tools.push(json!({
            "type": "function",
            "function": {
                "name": "read_file",
                "parameters": {"type": "object"}
            }
        }));

        let mut stream = provider.stream(request).await.unwrap();
        let mut events = Vec::new();
        while let Some(event) = stream.next().await {
            events.push(event.unwrap());
        }
        assert_eq!(
            events,
            vec![
                ModelEvent::TextDelta("hel".into()),
                ModelEvent::TextDelta("lo".into()),
                ModelEvent::ToolCall {
                    call_id: "ollama-tool-0".into(),
                    name: "read_file".into(),
                    arguments: "{\"path\":\"README.md\"}".into(),
                },
                ModelEvent::Usage {
                    input_tokens: 7,
                    output_tokens: 3,
                },
                ModelEvent::Finished,
            ]
        );

        let request = server.await.unwrap();
        let rendered = String::from_utf8_lossy(&request);
        assert!(rendered.starts_with("POST /api/chat HTTP/1.1\r\n"));
        let body = request_body(&request);
        assert_eq!(body["stream"], true);
        assert_eq!(body["tools"][0]["function"]["name"], "read_file");
    }

    #[tokio::test]
    async fn ollama_structured_stream_exposes_transport_and_real_content_deltas() {
        let response = concat!(
            "{\"message\":{\"role\":\"assistant\",\"content\":\"{\\\"answer\\\":\"},\"done\":false}\n",
            "{\"message\":{\"role\":\"assistant\",\"content\":\"\\\"ok\\\"}\"},\"done\":false}\n",
            "{\"message\":{\"role\":\"assistant\",\"content\":\"\"},\"done\":true,\"prompt_eval_count\":5,\"eval_count\":2}\n"
        )
        .as_bytes()
        .to_vec();
        let (base_url, server) = fake_http_server("200 OK", "application/x-ndjson", response).await;
        let provider = ollama_provider(base_url);

        let mut stream = provider
            .structured_stream(
                test_request("ollama"),
                schemars::schema_for!(QualificationAnswer),
            )
            .await
            .unwrap();
        let mut events = Vec::new();
        while let Some(event) = stream.next().await {
            events.push(event.unwrap());
        }
        assert_eq!(events.first(), Some(&ProviderStreamEvent::Connected));
        assert!(events.iter().any(|event| matches!(
            event,
            ProviderStreamEvent::BytesReceived { byte_count } if *byte_count > 0
        )));
        let content: String = events
            .iter()
            .filter_map(|event| match event {
                ProviderStreamEvent::Model(ModelEvent::TextDelta(delta)) => Some(delta.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(content, r#"{"answer":"ok"}"#);
        assert!(events.iter().any(|event| matches!(
            event,
            ProviderStreamEvent::Model(ModelEvent::Usage {
                input_tokens: 5,
                output_tokens: 2,
            })
        )));
        assert_eq!(
            events.last(),
            Some(&ProviderStreamEvent::Model(ModelEvent::Finished))
        );

        let request = server.await.unwrap();
        let rendered = String::from_utf8_lossy(&request);
        assert!(rendered.starts_with("POST /api/chat HTTP/1.1\r\n"));
        let body = request_body(&request);
        assert_eq!(body["stream"], true);
        assert_eq!(body["format"]["type"], "object");
    }

    #[tokio::test]
    async fn http_error_body_is_bounded_redacted_and_categorized() {
        let body = format!(
            "api_key=sk-super-secret Authorization: Bearer another-secret {}",
            "x".repeat(MAX_PROVIDER_ERROR_BODY_BYTES * 2)
        )
        .into_bytes();
        let (base_url, server) =
            fake_http_server("401 Unauthorized", "application/json", body).await;
        let provider = ollama_provider(base_url);

        let error = provider
            .structured(
                test_request("ollama"),
                schemars::schema_for!(QualificationAnswer),
            )
            .await
            .unwrap_err();
        let diagnostic = error.diagnostic().unwrap();
        assert_eq!(diagnostic.category, ProviderErrorCategory::Authentication);
        assert_eq!(diagnostic.http_status, Some(401));
        assert!(diagnostic.truncated);
        assert!(diagnostic.excerpt.as_ref().unwrap().len() <= MAX_PROVIDER_DIAGNOSTIC_BYTES);
        let rendered = error.to_string();
        assert!(!rendered.contains("super-secret"));
        assert!(!rendered.contains("another-secret"));
        assert!(rendered.contains("[REDACTED]"));
        server.await.unwrap();
    }

    #[tokio::test]
    async fn incompatible_stream_content_type_is_an_api_mode_mismatch() {
        let (base_url, server) = fake_http_server("200 OK", "text/event-stream", Vec::new()).await;
        let provider = ollama_provider(base_url);

        let error = provider.stream(test_request("ollama")).await.err().unwrap();
        assert_eq!(
            error.category(),
            Some(ProviderErrorCategory::ApiModeMismatch)
        );
        server.await.unwrap();
    }

    #[tokio::test]
    async fn oversized_ndjson_frame_is_rejected_with_a_bounded_diagnostic() {
        let mut response = vec![b'x'; MAX_PROVIDER_STREAM_FRAME_BYTES + 1];
        response.push(b'\n');
        let (base_url, server) = fake_http_server("200 OK", "application/x-ndjson", response).await;
        let provider = ollama_provider(base_url);

        let mut stream = provider.stream(test_request("ollama")).await.unwrap();
        let error = stream.next().await.unwrap().unwrap_err();
        let diagnostic = error.diagnostic().unwrap();
        assert_eq!(diagnostic.category, ProviderErrorCategory::StreamFraming);
        assert!(diagnostic.truncated);
        assert!(diagnostic.excerpt.as_ref().unwrap().len() <= MAX_PROVIDER_DIAGNOSTIC_BYTES);
        server.await.unwrap();
    }

    #[tokio::test]
    async fn oversized_structured_response_is_rejected_at_the_body_limit() {
        let response = vec![b'x'; MAX_PROVIDER_HTTP_BODY_BYTES + 1];
        let (base_url, server) = fake_http_server("200 OK", "application/json", response).await;
        let provider = ollama_provider(base_url);

        let error = provider
            .structured(
                test_request("ollama"),
                schemars::schema_for!(QualificationAnswer),
            )
            .await
            .unwrap_err();
        assert_eq!(error.category(), Some(ProviderErrorCategory::Schema));
        assert!(error.diagnostic().unwrap().truncated);
        server.await.unwrap();
    }

    #[tokio::test]
    async fn oversized_request_is_rejected_before_network_io() {
        let provider = ollama_provider(Url::parse("http://127.0.0.1:9/").unwrap());
        let mut request = test_request("ollama");
        request.messages[0].content = "x".repeat(MAX_PROVIDER_HTTP_REQUEST_BYTES + 1);

        let error = provider.stream(request).await.err().unwrap();
        assert_eq!(
            error.category(),
            Some(ProviderErrorCategory::ContextTooLarge)
        );
        assert!(error.diagnostic().unwrap().excerpt.is_some());
    }

    #[tokio::test]
    async fn health_check_retries_transient_statuses_only() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let attempts = Arc::new(AtomicUsize::new(0));
        let observed = attempts.clone();
        let server = tokio::spawn(async move {
            for _ in 0..3 {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = [0_u8; 2048];
                let _ = socket.read(&mut request).await.unwrap();
                let attempt = observed.fetch_add(1, Ordering::SeqCst);
                let status = if attempt < 2 {
                    "503 Service Unavailable"
                } else {
                    "200 OK"
                };
                let response = format!(
                    "HTTP/1.1 {status}\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{{}}"
                );
                socket.write_all(response.as_bytes()).await.unwrap();
            }
        });
        let config = ProviderConfig::OpenaiCompatible {
            base_url: Url::parse(&format!("http://{address}/v1/")).unwrap(),
            api_key_env: None,
            local: true,
            headers: BTreeMap::new(),
            capabilities: BTreeMap::new(),
        };
        let provider = HttpProvider::from_config("local".into(), config).unwrap();
        let health = provider.health_check().await.unwrap();
        assert!(health.available);
        assert_eq!(attempts.load(Ordering::SeqCst), 3);
        server.await.unwrap();
    }
}
