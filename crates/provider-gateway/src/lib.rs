//! Provider-neutral contracts, secure configuration, routing, and HTTP adapters.

use async_stream::try_stream;
use async_trait::async_trait;
use eventsource_stream::Eventsource;
use futures::{stream::BoxStream, StreamExt};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue, AUTHORIZATION, CONTENT_TYPE};
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

#[async_trait]
pub trait ModelProvider: Send + Sync {
    async fn capabilities(&self, model: &ModelId) -> Result<ModelCapabilities, ProviderError>;
    async fn stream(&self, request: ModelRequest) -> Result<ModelEventStream, ProviderError>;
    async fn structured(
        &self,
        request: ModelRequest,
        schema: RootSchema,
    ) -> Result<Value, ProviderError>;
    async fn count_tokens(&self, request: &ModelRequest) -> Result<TokenEstimate, ProviderError>;
    async fn health_check(&self) -> Result<ProviderHealth, ProviderError>;
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
        if name.trim().is_empty() || model.trim().is_empty() {
            return Err(ProviderError::Configuration(
                "provider name and model are required".into(),
            ));
        }
        let base_url = Url::parse(base_url)
            .map_err(|error| ProviderError::Configuration(format!("invalid base URL: {error}")))?;
        let mut capabilities = BTreeMap::new();
        capabilities.insert(
            model.to_owned(),
            ModelCapabilities::unknown(provider_type != "openai"),
        );
        let provider = match provider_type {
            "ollama" => ProviderConfig::Ollama {
                base_url,
                capabilities,
            },
            "lm-studio" | "openai-compatible" => ProviderConfig::OpenaiCompatible {
                base_url,
                api_key_env: credential_name.map(keychain_reference).transpose()?,
                local: provider_type == "lm-studio",
                headers: BTreeMap::new(),
                capabilities,
            },
            "openai" => ProviderConfig::Openai {
                base_url,
                api_key_env: keychain_reference(credential_name.unwrap_or(name))?,
                capabilities,
            },
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
        let config: Self = toml::from_str(&fs::read_to_string(path)?)?;
        if config.schema_version == 0 {
            return Err(ProviderError::Configuration(
                "legacy configuration schema 0; run `purrcode config migrate`".into(),
            ));
        }
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
    Url::parse("http://127.0.0.1:11434/v1/").expect("static Ollama URL is valid")
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
    chat_completions: bool,
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
            chat_completions,
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
                false,
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
                true,
            ),
            ProviderConfig::Ollama {
                base_url,
                capabilities,
            } => (
                base_url,
                None,
                None,
                true,
                BTreeMap::new(),
                BTreeMap::new(),
                None,
                None,
                None,
                capabilities,
                true,
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
                false,
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
            chat_completions,
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
        let mut last_transport = None;
        for attempt in 0..3_u32 {
            let mut request = self.request(method.clone(), url.clone()).await?;
            if let Some(body) = body {
                request = request.json(body);
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
                        return Err(http_failure(response).await);
                    }
                    let retry_after = response
                        .headers()
                        .get(reqwest::header::RETRY_AFTER)
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
                Err(error) => return Err(error.into()),
            }
        }
        match last_transport {
            Some(error) => Err(error.into()),
            None => Err(ProviderError::Unavailable(
                "retry attempts were exhausted without a response".into(),
            )),
        }
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
        if self.chat_completions {
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
        } else {
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
                body["text"] = json!({
                    "format": {
                        "type": "json_schema",
                        "name": "purrcode_result",
                        "strict": true,
                        "schema": schema.schema
                    }
                });
            }
            body
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
        let endpoint = if self.chat_completions {
            "chat/completions"
        } else {
            "responses"
        };
        let response = self
            .send_with_retry(
                reqwest::Method::POST,
                self.endpoint(endpoint)?,
                Some(&body),
                Some(&idempotency_key),
            )
            .await?;
        let status = response.status();
        if !status.is_success() {
            return Err(http_failure(response).await);
        }
        let events = response.bytes_stream().eventsource();
        let use_chat = self.chat_completions;
        Ok(Box::pin(try_stream! {
            futures::pin_mut!(events);
            while let Some(event) = events.next().await {
                let event = event.map_err(|error| ProviderError::InvalidResponse(error.to_string()))?;
                if event.data == "[DONE]" {
                    break;
                }
                if use_chat {
                    if let Some(parsed) = parse_chat_event(&event.data)? {
                        yield parsed;
                    }
                } else if let Some(parsed) = parse_response_event(&event.data)? {
                    yield parsed;
                }
            }
        }))
    }

    async fn structured(
        &self,
        request: ModelRequest,
        schema: RootSchema,
    ) -> Result<Value, ProviderError> {
        let body = self.response_body(&request, false, Some(schema));
        let idempotency_key = Uuid::new_v4().to_string();
        let endpoint = if self.chat_completions {
            "chat/completions"
        } else {
            "responses"
        };
        let response = self
            .send_with_retry(
                reqwest::Method::POST,
                self.endpoint(endpoint)?,
                Some(&body),
                Some(&idempotency_key),
            )
            .await?;
        if !response.status().is_success() {
            return Err(http_failure(response).await);
        }
        let value: Value = response.json().await?;
        if self.chat_completions {
            extract_chat_output(value)
        } else {
            extract_output_json(value)
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
        let response = self
            .send_with_retry(reqwest::Method::GET, self.endpoint("models")?, None, None)
            .await?;
        Ok(ProviderHealth {
            available: response.status().is_success(),
            detail: format!("{} returned HTTP {}", self.name, response.status()),
        })
    }
}

async fn http_failure(response: reqwest::Response) -> ProviderError {
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    let sanitized = if body.len() > 2048 {
        &body[..2048]
    } else {
        &body
    };
    ProviderError::HttpStatus {
        status: status.as_u16(),
        body: sanitized.to_owned(),
    }
}

fn parse_response_event(data: &str) -> Result<Option<ModelEvent>, ProviderError> {
    let value: Value = serde_json::from_str(data)?;
    let event_type = value["type"].as_str().unwrap_or_default();
    let event = match event_type {
        "response.created" => {
            value["response"]["id"]
                .as_str()
                .map(|id| ModelEvent::ResponseStarted {
                    response_id: id.into(),
                })
        }
        "response.output_text.delta" => value["delta"]
            .as_str()
            .map(|delta| ModelEvent::TextDelta(delta.into())),
        "response.output_item.done" if value["item"]["type"] == "function_call" => {
            Some(ModelEvent::ToolCall {
                call_id: required_string(&value["item"], "call_id")?,
                name: required_string(&value["item"], "name")?,
                arguments: required_string(&value["item"], "arguments")?,
            })
        }
        "response.completed" => {
            if let (Some(input), Some(output)) = (
                value["response"]["usage"]["input_tokens"].as_u64(),
                value["response"]["usage"]["output_tokens"].as_u64(),
            ) {
                Some(ModelEvent::Usage {
                    input_tokens: input,
                    output_tokens: output,
                })
            } else {
                Some(ModelEvent::Finished)
            }
        }
        "response.failed" | "error" => {
            return Err(ProviderError::InvalidResponse(
                value["response"]["error"]["message"]
                    .as_str()
                    .or_else(|| value["error"]["message"].as_str())
                    .unwrap_or("provider reported an unspecified error")
                    .into(),
            ))
        }
        _ => None,
    };
    Ok(event)
}

fn required_string(value: &Value, field: &str) -> Result<String, ProviderError> {
    value[field]
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| ProviderError::InvalidResponse(format!("missing string field `{field}`")))
}

fn parse_chat_event(data: &str) -> Result<Option<ModelEvent>, ProviderError> {
    let value: Value = serde_json::from_str(data)?;
    let choices = value["choices"].as_array().and_then(|c| c.first());
    let delta = choices.and_then(|c| c["delta"].as_object());
    let finish_reason = choices.and_then(|c| c["finish_reason"].as_str());
    if finish_reason.is_some() {
        if let Some(input) = value["usage"]["prompt_tokens"].as_u64() {
            if let Some(output) = value["usage"]["completion_tokens"].as_u64() {
                return Ok(Some(ModelEvent::Usage {
                    input_tokens: input,
                    output_tokens: output,
                }));
            }
        }
        return Ok(Some(ModelEvent::Finished));
    }
    let event = delta.and_then(|d| {
        d.get("content")
            .and_then(|c| c.as_str())
            .map(|text| ModelEvent::TextDelta(text.into()))
            .or_else(|| {
                d.get("tool_calls")
                    .and_then(|tc| tc.as_array())
                    .and_then(|calls| calls.first())
                    .map(|call| ModelEvent::ToolCall {
                        call_id: call["id"].as_str().unwrap_or("").into(),
                        name: call["function"]["name"].as_str().unwrap_or("").into(),
                        arguments: call["function"]["arguments"]
                            .as_str()
                            .unwrap_or("{}")
                            .into(),
                    })
            })
    });
    Ok(event)
}

fn extract_chat_output(response: Value) -> Result<Value, ProviderError> {
    let content = response["choices"]
        .as_array()
        .and_then(|c| c.first())
        .and_then(|c| c["message"]["content"].as_str())
        .ok_or_else(|| {
            ProviderError::InvalidResponse("chat response contained no message content".into())
        })?;
    Ok(serde_json::from_str(content)?)
}

fn extract_output_json(response: Value) -> Result<Value, ProviderError> {
    let text = response["output"]
        .as_array()
        .and_then(|items| items.iter().find(|item| item["type"] == "message"))
        .and_then(|message| message["content"].as_array())
        .and_then(|parts| parts.iter().find(|part| part["type"] == "output_text"))
        .and_then(|part| part["text"].as_str())
        .ok_or_else(|| {
            ProviderError::InvalidResponse("structured response contained no output_text".into())
        })?;
    Ok(serde_json::from_str(text)?)
}

#[derive(Debug, Error)]
pub enum ProviderError {
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
    #[error("HTTP transport failed: {0}")]
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn provider_endpoint_preserves_a_base_path_without_trailing_slash() {
        let provider = HttpProvider::from_config(
            "ollama".into(),
            ProviderConfig::Ollama {
                base_url: Url::parse("http://127.0.0.1:11434/v1").unwrap(),
                capabilities: BTreeMap::new(),
            },
        )
        .unwrap();
        assert_eq!(
            provider.endpoint("chat/completions").unwrap().as_str(),
            "http://127.0.0.1:11434/v1/chat/completions"
        );
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
        assert_eq!(extract_chat_output(response).unwrap(), json!({"ok": true}));
    }

    #[test]
    fn structured_output_is_extracted() {
        let response = json!({
            "output": [{
                "type": "message",
                "content": [{"type": "output_text", "text": "{\"ok\":true}"}]
            }]
        });
        assert_eq!(extract_output_json(response).unwrap(), json!({"ok": true}));
    }

    #[tokio::test]
    async fn health_check_retries_transient_statuses_only() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
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
