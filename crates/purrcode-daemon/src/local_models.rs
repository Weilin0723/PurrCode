//! Local model discovery and lifecycle control.
//!
//! Inspection is deliberately generation-free: startup and status checks call only Ollama's
//! metadata endpoints. A model is unloaded only after an explicit API request or after a
//! configured lifecycle boundary.

use futures::StreamExt;
use purrcode_provider_gateway::AppConfig;
use reqwest::{Client, Response, Url};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::time::Duration;
use sysinfo::System;

const DEFAULT_OLLAMA_URL: &str = "http://127.0.0.1:11434/";
const MAX_OLLAMA_RESPONSE_BYTES: usize = 8 * 1024 * 1024;
const MIB: u64 = 1024 * 1024;
const GIB: u64 = 1024 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LocalModelLifecycle {
    #[default]
    UnloadAfterRequest,
    IdleTimeout,
    KeepLoaded,
    External,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LocalModelLifecycleSettings {
    #[serde(default)]
    pub policy: LocalModelLifecycle,
    #[serde(default = "default_idle_timeout_seconds")]
    pub idle_timeout_seconds: u64,
}

impl Default for LocalModelLifecycleSettings {
    fn default() -> Self {
        Self {
            policy: LocalModelLifecycle::UnloadAfterRequest,
            idle_timeout_seconds: default_idle_timeout_seconds(),
        }
    }
}

impl LocalModelLifecycleSettings {
    pub fn validate(&self) -> Result<(), String> {
        if self.policy == LocalModelLifecycle::IdleTimeout
            && !(30..=86_400).contains(&self.idle_timeout_seconds)
        {
            return Err("idle_timeout_seconds must be between 30 and 86400".into());
        }
        Ok(())
    }

    pub fn load(config: &AppConfig) -> Result<Self, String> {
        let Some(value) = config.extensions.get("local_model_lifecycle") else {
            return Ok(Self::default());
        };
        let settings: Self = value
            .clone()
            .try_into()
            .map_err(|error| format!("invalid local_model_lifecycle settings: {error}"))?;
        settings.validate()?;
        Ok(settings)
    }

    pub fn save(&self, config: &mut AppConfig, path: &Path) -> Result<(), String> {
        self.validate()?;
        let value = toml::Value::try_from(self)
            .map_err(|error| format!("could not serialize lifecycle settings: {error}"))?;
        config
            .extensions
            .insert("local_model_lifecycle".into(), value);
        config.save(path).map_err(|error| error.to_string())
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UnloadLocalModelRequest {
    pub model: Option<String>,
    #[serde(default)]
    pub all: bool,
}

impl UnloadLocalModelRequest {
    pub fn validate(&self) -> Result<(), String> {
        if self.all == self.model.is_some() {
            return Err("provide exactly one of `model` or `all: true`".into());
        }
        if let Some(model) = &self.model {
            validate_model_name(model)?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct OllamaModel {
    pub name: String,
    #[serde(default)]
    pub size: u64,
    #[serde(default)]
    pub size_vram: u64,
    #[serde(default)]
    pub digest: String,
    #[serde(default)]
    pub details: serde_json::Value,
}

#[derive(Clone, Debug, Serialize)]
pub struct ResourceSnapshot {
    pub total_memory_bytes: u64,
    pub available_memory_bytes: u64,
    pub loaded_model_bytes: u64,
    pub total_swap_bytes: u64,
    pub used_swap_bytes: u64,
    pub low_memory: bool,
    pub memory_pressure: &'static str,
    pub maximum_local_inference_requests: usize,
    pub allow_separate_local_judge: bool,
}

impl ResourceSnapshot {
    pub fn detect(loaded_model_bytes: u64) -> Self {
        let mut system = System::new();
        system.refresh_memory();
        let total = system.total_memory();
        let available = system.available_memory();
        let total_swap = system.total_swap();
        let used_swap = system.used_swap();
        let low_memory = total <= 16 * GIB;
        let pressure_ratio = if total == 0 {
            1.0
        } else {
            total.saturating_sub(available) as f64 / total as f64
        };
        let memory_pressure = if pressure_ratio >= 0.9 || used_swap >= 2 * GIB {
            "high"
        } else if pressure_ratio >= 0.75 || used_swap >= 512 * MIB {
            "elevated"
        } else {
            "normal"
        };
        Self {
            total_memory_bytes: total,
            available_memory_bytes: available,
            loaded_model_bytes,
            total_swap_bytes: total_swap,
            used_swap_bytes: used_swap,
            low_memory,
            memory_pressure,
            // Stabilization default: one local request everywhere. Phase 4 may raise this only
            // after model qualification and an explicit hardware-aware recommendation.
            maximum_local_inference_requests: 1,
            allow_separate_local_judge: !low_memory && memory_pressure == "normal",
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct LocalModelStatus {
    pub provider: &'static str,
    pub reachable: bool,
    pub version: Option<String>,
    pub installed: Vec<OllamaModel>,
    pub loaded: Vec<OllamaModel>,
    pub resources: ResourceSnapshot,
    pub generation_requests_made: u64,
}

#[derive(Clone)]
pub struct LocalModelRuntime {
    client: Client,
    base_url: Url,
}

impl LocalModelRuntime {
    pub fn ollama_default() -> Result<Self, String> {
        Self::new(DEFAULT_OLLAMA_URL)
    }

    pub fn new(base_url: &str) -> Result<Self, String> {
        let mut base_url = Url::parse(base_url).map_err(|error| error.to_string())?;
        if !matches!(base_url.host_str(), Some("127.0.0.1" | "localhost" | "::1")) {
            return Err("local model lifecycle endpoint must be loopback".into());
        }
        if base_url.path().trim_end_matches('/') == "/v1" {
            base_url.set_path("/");
        } else if !base_url.path().ends_with('/') {
            let path = format!("{}/", base_url.path());
            base_url.set_path(&path);
        }
        let client = Client::builder()
            .connect_timeout(Duration::from_secs(2))
            .timeout(Duration::from_secs(5))
            .build()
            .map_err(|error| error.to_string())?;
        Ok(Self { client, base_url })
    }

    fn endpoint(&self, path: &str) -> Result<Url, String> {
        self.base_url.join(path).map_err(|error| error.to_string())
    }

    pub async fn inspect(&self) -> Result<LocalModelStatus, String> {
        let version = self.get_json("api/version").await?;
        let tags = self.get_json("api/tags").await?;
        let running = self.get_json("api/ps").await?;
        let installed = parse_models(&tags)?;
        let loaded = parse_models(&running)?;
        let loaded_bytes = sum_loaded_model_bytes(&loaded)?;
        Ok(LocalModelStatus {
            provider: "ollama",
            reachable: true,
            version: version
                .get("version")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned),
            installed,
            loaded,
            resources: ResourceSnapshot::detect(loaded_bytes),
            // Metadata inspection never sends a prompt and therefore cannot load a model.
            generation_requests_made: 0,
        })
    }

    pub async fn unload(&self, request: &UnloadLocalModelRequest) -> Result<Vec<String>, String> {
        request.validate()?;
        let targets = if request.all {
            self.inspect()
                .await?
                .loaded
                .into_iter()
                .map(|model| model.name)
                .collect::<Vec<_>>()
        } else {
            vec![request.model.clone().expect("validated model is present")]
        };
        for model in &targets {
            let response = self
                .client
                .post(self.endpoint("api/generate")?)
                .json(&serde_json::json!({
                    "model": model,
                    "keep_alive": 0,
                    "stream": false,
                }))
                .send()
                .await
                .map_err(|error| format!("Ollama unload request failed: {error}"))?;
            if !response.status().is_success() {
                return Err(format!(
                    "Ollama unload returned HTTP {} for model `{model}`",
                    response.status()
                ));
            }
            read_bounded_json(response, "api/generate unload").await?;
        }
        let remaining = self.inspect().await?.loaded;
        let still_loaded = remaining
            .iter()
            .filter(|model| targets.contains(&model.name))
            .map(|model| model.name.clone())
            .collect::<Vec<_>>();
        if !still_loaded.is_empty() {
            return Err(format!(
                "Ollama acknowledged unload but models remain loaded: {}",
                still_loaded.join(", ")
            ));
        }
        Ok(targets)
    }

    pub async fn keep_alive(&self, model: &str, keep_alive_seconds: i64) -> Result<(), String> {
        validate_model_name(model)?;
        if keep_alive_seconds != -1 && !(30..=86_400).contains(&keep_alive_seconds) {
            return Err("keep_alive seconds must be -1 or between 30 and 86400".into());
        }
        if !self
            .inspect()
            .await?
            .loaded
            .iter()
            .any(|loaded| loaded.name == model)
        {
            return Ok(());
        }
        let response = self
            .client
            .post(self.endpoint("api/generate")?)
            .json(&serde_json::json!({
                "model": model,
                "keep_alive": keep_alive_seconds,
                "stream": false,
            }))
            .send()
            .await
            .map_err(|error| format!("Ollama keep-alive request failed: {error}"))?;
        if !response.status().is_success() {
            return Err(format!(
                "Ollama keep-alive returned HTTP {} for model `{model}`",
                response.status()
            ));
        }
        read_bounded_json(response, "api/generate keep-alive").await?;
        if !self
            .inspect()
            .await?
            .loaded
            .iter()
            .any(|loaded| loaded.name == model)
        {
            return Err(format!(
                "Ollama acknowledged keep-alive but model `{model}` is not loaded"
            ));
        }
        Ok(())
    }

    async fn get_json(&self, path: &str) -> Result<serde_json::Value, String> {
        let response = self
            .client
            .get(self.endpoint(path)?)
            .send()
            .await
            .map_err(|error| format!("Ollama `{path}` probe failed: {error}"))?;
        if !response.status().is_success() {
            return Err(format!(
                "Ollama `{path}` probe returned HTTP {}",
                response.status()
            ));
        }
        read_bounded_json(response, path).await
    }
}

async fn read_bounded_json(response: Response, context: &str) -> Result<serde_json::Value, String> {
    if response
        .content_length()
        .is_some_and(|length| length > MAX_OLLAMA_RESPONSE_BYTES as u64)
    {
        return Err(format!(
            "Ollama `{context}` response exceeded {MAX_OLLAMA_RESPONSE_BYTES} bytes"
        ));
    }
    let mut stream = response.bytes_stream();
    let mut body = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk =
            chunk.map_err(|error| format!("Ollama `{context}` response read failed: {error}"))?;
        if body.len().saturating_add(chunk.len()) > MAX_OLLAMA_RESPONSE_BYTES {
            return Err(format!(
                "Ollama `{context}` response exceeded {MAX_OLLAMA_RESPONSE_BYTES} bytes"
            ));
        }
        body.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&body)
        .map_err(|error| format!("Ollama `{context}` returned incompatible JSON: {error}"))
}

fn parse_models(value: &serde_json::Value) -> Result<Vec<OllamaModel>, String> {
    value
        .get("models")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| "Ollama response did not contain a `models` array".to_owned())?
        .iter()
        .cloned()
        .map(|value| {
            serde_json::from_value(value)
                .map_err(|error| format!("Ollama returned incompatible model metadata: {error}"))
        })
        .collect()
}

fn sum_loaded_model_bytes(models: &[OllamaModel]) -> Result<u64, String> {
    models.iter().try_fold(0_u64, |total, model| {
        total
            .checked_add(model.size_vram)
            .ok_or_else(|| "Ollama loaded-model memory total overflowed".to_owned())
    })
}

fn validate_model_name(model: &str) -> Result<(), String> {
    if model.is_empty()
        || model.len() > 256
        || !model
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "._:/-".contains(character))
    {
        return Err("model name contains unsupported characters".into());
    }
    Ok(())
}

const fn default_idle_timeout_seconds() -> u64 {
    300
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::extract::State;
    use axum::routing::{get, post};
    use axum::{Json, Router};
    use std::sync::Arc;
    use tokio::net::TcpListener;
    use tokio::sync::Mutex;

    #[derive(Clone, Default)]
    struct FakeState {
        posts: Arc<Mutex<Vec<serde_json::Value>>>,
        loaded: Arc<Mutex<Vec<String>>>,
    }

    async fn version() -> Json<serde_json::Value> {
        Json(serde_json::json!({"version":"test"}))
    }

    async fn tags() -> Json<serde_json::Value> {
        Json(serde_json::json!({
            "models":[{"name":"small:latest","size":123,"digest":"abc","details":{}}]
        }))
    }

    async fn oversized_tags() -> axum::http::Response<Body> {
        axum::http::Response::new(Body::from(vec![b'x'; MAX_OLLAMA_RESPONSE_BYTES + 1]))
    }

    async fn ps(State(state): State<FakeState>) -> Json<serde_json::Value> {
        let models = state
            .loaded
            .lock()
            .await
            .iter()
            .map(|name| {
                serde_json::json!({
                    "name":name,
                    "size":123,
                    "size_vram":123,
                    "digest":"abc",
                    "details":{}
                })
            })
            .collect::<Vec<_>>();
        Json(serde_json::json!({"models":models}))
    }

    async fn generate(
        State(state): State<FakeState>,
        Json(body): Json<serde_json::Value>,
    ) -> Json<serde_json::Value> {
        state.posts.lock().await.push(body.clone());
        if let Some(model) = body.get("model").and_then(serde_json::Value::as_str) {
            let mut loaded = state.loaded.lock().await;
            if body.get("keep_alive").and_then(serde_json::Value::as_i64) == Some(0) {
                loaded.retain(|loaded| loaded != model);
            } else if !loaded.iter().any(|loaded| loaded == model) {
                loaded.push(model.to_owned());
            }
        }
        Json(serde_json::json!({"done":true}))
    }

    async fn fixture() -> (LocalModelRuntime, FakeState) {
        let state = FakeState::default();
        let app = Router::new()
            .route("/api/version", get(version))
            .route("/api/tags", get(tags))
            .route("/api/ps", get(ps))
            .route("/api/generate", post(generate))
            .with_state(state.clone());
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (
            LocalModelRuntime::new(&format!("http://{address}/")).unwrap(),
            state,
        )
    }

    #[tokio::test]
    async fn startup_inspection_never_loads_or_generates() {
        let (runtime, state) = fixture().await;
        let status = runtime.inspect().await.unwrap();
        assert_eq!(status.version.as_deref(), Some("test"));
        assert_eq!(status.installed[0].name, "small:latest");
        assert_eq!(status.generation_requests_made, 0);
        assert!(state.posts.lock().await.is_empty());
    }

    #[tokio::test]
    async fn inspection_rejects_oversized_untrusted_metadata() {
        let state = FakeState::default();
        let app = Router::new()
            .route("/api/version", get(version))
            .route("/api/tags", get(oversized_tags))
            .route("/api/ps", get(ps))
            .with_state(state);
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let runtime = LocalModelRuntime::new(&format!("http://{address}/")).unwrap();
        let error = runtime.inspect().await.unwrap_err();
        assert!(error.contains("response exceeded"));
    }

    #[tokio::test]
    async fn unload_is_explicit_and_verified_through_ps() {
        let (runtime, state) = fixture().await;
        state.loaded.lock().await.push("small:latest".into());
        let unloaded = runtime
            .unload(&UnloadLocalModelRequest {
                model: Some("small:latest".into()),
                all: false,
            })
            .await
            .unwrap();
        assert_eq!(unloaded, vec!["small:latest"]);
        assert!(state.loaded.lock().await.is_empty());
        assert_eq!(
            state.posts.lock().await[0]["keep_alive"],
            serde_json::json!(0)
        );
    }

    #[tokio::test]
    async fn keep_loaded_is_explicit_and_verified_through_ps() {
        let (runtime, state) = fixture().await;
        state.loaded.lock().await.push("small:latest".into());
        runtime.keep_alive("small:latest", -1).await.unwrap();
        assert_eq!(
            state.posts.lock().await[0]["keep_alive"],
            serde_json::json!(-1)
        );
        assert_eq!(
            state.loaded.lock().await.as_slice(),
            &["small:latest".to_owned()]
        );
    }

    #[tokio::test]
    async fn keep_alive_never_loads_an_absent_model() {
        let (runtime, state) = fixture().await;
        runtime.keep_alive("small:latest", -1).await.unwrap();
        assert!(state.posts.lock().await.is_empty());
        assert!(state.loaded.lock().await.is_empty());
    }

    #[test]
    fn low_memory_policy_is_conservative() {
        let snapshot = ResourceSnapshot {
            total_memory_bytes: 8 * GIB,
            available_memory_bytes: 2 * GIB,
            loaded_model_bytes: 4 * GIB,
            total_swap_bytes: 4 * GIB,
            used_swap_bytes: 1,
            low_memory: true,
            memory_pressure: "elevated",
            maximum_local_inference_requests: 1,
            allow_separate_local_judge: false,
        };
        assert_eq!(snapshot.maximum_local_inference_requests, 1);
        assert!(!snapshot.allow_separate_local_judge);
    }

    #[test]
    fn loaded_model_memory_overflow_fails_closed() {
        let model = |size_vram| OllamaModel {
            name: "fixture".into(),
            size: 0,
            size_vram,
            digest: String::new(),
            details: serde_json::Value::Null,
        };
        assert!(sum_loaded_model_bytes(&[model(u64::MAX), model(1)]).is_err());
    }

    #[test]
    fn lifecycle_settings_are_validated_and_persisted() {
        let temporary = tempfile::tempdir().unwrap();
        let path = temporary.path().join("config.toml");
        let mut config: AppConfig = toml::from_str("schema_version = 1\n").unwrap();
        let settings = LocalModelLifecycleSettings {
            policy: LocalModelLifecycle::IdleTimeout,
            idle_timeout_seconds: 120,
        };
        settings.save(&mut config, &path).unwrap();

        let reloaded = AppConfig::load(&path).unwrap();
        assert_eq!(
            LocalModelLifecycleSettings::load(&reloaded).unwrap(),
            settings
        );
        assert!(LocalModelLifecycleSettings {
            policy: LocalModelLifecycle::IdleTimeout,
            idle_timeout_seconds: 5,
        }
        .validate()
        .is_err());

        let invalid: AppConfig = toml::from_str(
            "schema_version = 1\n\n[local_model_lifecycle]\npolicy = \"idle_timeout\"\nidle_timeout_seconds = 5\n",
        )
        .unwrap();
        assert!(LocalModelLifecycleSettings::load(&invalid).is_err());
    }
}
