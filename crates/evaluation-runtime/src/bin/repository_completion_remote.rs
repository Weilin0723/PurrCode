//! Run a frozen, schema-constrained research request bundle through a remote
//! provider.
//!
//! The bundle is the trust boundary: this runner validates every prompt hash
//! and schedule index before it can make a provider call.  The JSONL event log
//! is the recovery boundary: a request is started only after its start event
//! is durable, and a started request with no terminal event is marked uncertain
//! on resume instead of being replayed.

use futures::StreamExt;
use purrcode_provider_gateway::{
    AppConfig, CREDENTIALS_FILE_NAME, ModelEvent, ModelId, ModelMessage, ModelProvider,
    ModelRequest, ProviderError, ProviderRouter, ProviderStreamEvent,
};
use schemars::JsonSchema;
use serde::de::{self, MapAccess, Visitor};
use serde::{Deserialize, Deserializer};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use thiserror::Error;

const BUNDLE_SCHEMA_VERSION: u32 = 1;
const MAX_BUNDLE_REQUESTS: usize = 100_000;
const MAX_PROMPT_BYTES: usize = 8 * 1024 * 1024;
const MAX_RESPONSE_BYTES: usize = 4 * 1024 * 1024;
const MAX_REASON_BYTES: usize = 16 * 1024;
const MAX_REASONS: usize = 256;
const MAX_OUTPUT_TOKENS: u64 = 65_536;

#[derive(Debug, Error)]
enum RunnerError {
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid request bundle: {0}")]
    Bundle(String),
    #[error("event log error: {0}")]
    EventLog(String),
    #[error("configuration error: {0}")]
    Configuration(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct RequestSpec {
    provider: String,
    model: String,
    schedule_index: u64,
    task_id: String,
    candidate: String,
    condition: String,
    seed: i64,
    prompt: String,
    prompt_sha256: String,
    payload_sha256: String,
    max_output_tokens: u64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawBundle {
    schema_version: u32,
    #[serde(default)]
    provider: Option<String>,
    #[serde(default)]
    model: Option<String>,
    requests: Vec<RawRequest>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRequest {
    #[serde(default)]
    provider: Option<String>,
    #[serde(default)]
    model: Option<String>,
    schedule_index: u64,
    prompt: String,
    prompt_sha256: String,
    max_output_tokens: u64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct JsonlRequest {
    schedule_index: u64,
    task_id: String,
    candidate: String,
    condition: String,
    seed: i64,
    model: String,
    prompt_sha256: String,
    payload_sha256: String,
    payload: serde_json::Value,
}

fn load_bundle(path: &Path, provider_override: &str) -> Result<Vec<RequestSpec>, RunnerError> {
    let content = fs::read_to_string(path)?;
    let requests = if let Ok(raw) = serde_json::from_str::<RawBundle>(&content) {
        load_json_bundle(raw, provider_override)?
    } else {
        load_jsonl_bundle(&content, provider_override)?
    };
    validate_schedule(requests)
}

fn load_json_bundle(
    raw: RawBundle,
    provider_override: &str,
) -> Result<Vec<RequestSpec>, RunnerError> {
    if raw.schema_version != BUNDLE_SCHEMA_VERSION {
        return Err(RunnerError::Bundle(format!(
            "schema_version {} is unsupported; expected {}",
            raw.schema_version, BUNDLE_SCHEMA_VERSION
        )));
    }
    if raw.requests.is_empty() || raw.requests.len() > MAX_BUNDLE_REQUESTS {
        return Err(RunnerError::Bundle(format!(
            "requests must contain 1..={MAX_BUNDLE_REQUESTS} items"
        )));
    }
    let default_provider = raw.provider;
    let default_model = raw.model;
    let mut requests = Vec::with_capacity(raw.requests.len());
    for (position, item) in raw.requests.into_iter().enumerate() {
        let provider = item
            .provider
            .or_else(|| default_provider.clone())
            .unwrap_or_else(|| provider_override.to_owned());
        let model = item
            .model
            .or_else(|| default_model.clone())
            .ok_or_else(|| RunnerError::Bundle(format!("requests[{position}] has no model")))?;
        if provider != provider_override {
            return Err(RunnerError::Bundle(format!(
                "requests[{position}] provider does not match --provider"
            )));
        }
        validate_name(&provider, "provider", position)?;
        validate_name(&model, "model", position)?;
        if item.prompt.is_empty() || item.prompt.len() > MAX_PROMPT_BYTES {
            return Err(RunnerError::Bundle(format!(
                "requests[{position}].prompt must be non-empty and at most {MAX_PROMPT_BYTES} bytes"
            )));
        }
        if item.max_output_tokens == 0 || item.max_output_tokens > MAX_OUTPUT_TOKENS {
            return Err(RunnerError::Bundle(format!(
                "requests[{position}].max_output_tokens must be in 1..={MAX_OUTPUT_TOKENS}"
            )));
        }
        let expected_hash = sha256_hex(item.prompt.as_bytes());
        if item.prompt_sha256 != expected_hash {
            return Err(RunnerError::Bundle(format!(
                "requests[{position}].prompt_sha256 does not match prompt"
            )));
        }
        requests.push(RequestSpec {
            provider,
            model,
            schedule_index: item.schedule_index,
            task_id: String::new(),
            candidate: String::new(),
            condition: String::new(),
            seed: 0,
            prompt: item.prompt,
            prompt_sha256: item.prompt_sha256,
            payload_sha256: String::new(),
            max_output_tokens: item.max_output_tokens,
        });
    }
    Ok(requests)
}

fn load_jsonl_bundle(
    content: &str,
    provider_override: &str,
) -> Result<Vec<RequestSpec>, RunnerError> {
    let mut requests = Vec::new();
    for (position, line) in content.lines().enumerate() {
        if line.trim().is_empty() {
            return Err(RunnerError::Bundle(format!(
                "blank JSONL request at line {}",
                position + 1
            )));
        }
        let item: JsonlRequest = serde_json::from_str(line).map_err(|error| {
            RunnerError::Bundle(format!(
                "invalid JSONL request at line {}: {}",
                position + 1,
                redact_error(&error.to_string())
            ))
        })?;
        if requests.len() >= MAX_BUNDLE_REQUESTS {
            return Err(RunnerError::Bundle(format!(
                "requests must contain at most {MAX_BUNDLE_REQUESTS} items"
            )));
        }
        validate_name(&item.task_id, "task_id", position)?;
        validate_name(&item.candidate, "candidate", position)?;
        validate_name(&item.condition, "condition", position)?;
        validate_name(&item.model, "model", position)?;
        let prompt = item
            .payload
            .get("messages")
            .and_then(serde_json::Value::as_array)
            .and_then(|messages| messages.first())
            .and_then(|message| message.get("content"))
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| {
                RunnerError::Bundle(format!(
                    "JSONL line {} payload.messages[0].content must be a string",
                    position + 1
                ))
            })?
            .to_owned();
        if prompt.is_empty() || prompt.len() > MAX_PROMPT_BYTES {
            return Err(RunnerError::Bundle(format!(
                "JSONL line {} prompt must be non-empty and at most {MAX_PROMPT_BYTES} bytes",
                position + 1
            )));
        }
        let max_output_tokens = item
            .payload
            .get("max_tokens")
            .or_else(|| item.payload.get("max_output_tokens"))
            .or_else(|| {
                item.payload
                    .get("options")
                    .and_then(|options| options.get("num_predict"))
            })
            .and_then(serde_json::Value::as_u64)
            .ok_or_else(|| {
                RunnerError::Bundle(format!(
                    "JSONL line {} payload.max_tokens must be a positive integer",
                    position + 1
                ))
            })?;
        if item
            .payload
            .get("model")
            .and_then(serde_json::Value::as_str)
            != Some(item.model.as_str())
        {
            return Err(RunnerError::Bundle(format!(
                "JSONL line {} model does not match payload.model",
                position + 1
            )));
        }
        if item.prompt_sha256 != sha256_hex(prompt.as_bytes()) {
            return Err(RunnerError::Bundle(format!(
                "JSONL line {} prompt_sha256 does not match prompt",
                position + 1
            )));
        }
        if item.payload_sha256 != sha256_json(&item.payload) {
            return Err(RunnerError::Bundle(format!(
                "JSONL line {} payload_sha256 does not match payload",
                position + 1
            )));
        }
        if max_output_tokens == 0 || max_output_tokens > MAX_OUTPUT_TOKENS {
            return Err(RunnerError::Bundle(format!(
                "JSONL line {} max_tokens must be in 1..={MAX_OUTPUT_TOKENS}",
                position + 1
            )));
        }
        requests.push(RequestSpec {
            provider: provider_override.to_owned(),
            model: item.model,
            schedule_index: item.schedule_index,
            task_id: item.task_id,
            candidate: item.candidate,
            condition: item.condition,
            seed: item.seed,
            prompt,
            prompt_sha256: item.prompt_sha256,
            payload_sha256: item.payload_sha256,
            max_output_tokens,
        });
    }
    if requests.is_empty() {
        return Err(RunnerError::Bundle("request JSONL is empty".into()));
    }
    Ok(requests)
}

fn validate_schedule(mut requests: Vec<RequestSpec>) -> Result<Vec<RequestSpec>, RunnerError> {
    if requests.is_empty() || requests.len() > MAX_BUNDLE_REQUESTS {
        return Err(RunnerError::Bundle(format!(
            "requests must contain 1..={MAX_BUNDLE_REQUESTS} items"
        )));
    }
    requests.sort_by_key(|request| request.schedule_index);
    let expected_indices: BTreeSet<u64> = (0..requests.len() as u64).collect();
    let observed_indices: BTreeSet<u64> = requests
        .iter()
        .map(|request| request.schedule_index)
        .collect();
    if observed_indices != expected_indices {
        return Err(RunnerError::Bundle(
            "schedule_index must be a unique complete range 0..requests.len()".into(),
        ));
    }
    Ok(requests)
}

fn validate_name(value: &str, field: &str, position: usize) -> Result<(), RunnerError> {
    if value.is_empty()
        || value.len() > 512
        || value.chars().any(|character| character.is_control())
    {
        return Err(RunnerError::Bundle(format!(
            "requests[{position}].{field} must be a non-empty bounded name without control characters"
        )));
    }
    Ok(())
}

#[derive(Clone, Debug, Default)]
struct EventState {
    started: bool,
    terminal: bool,
}

struct EventLog {
    file: File,
    states: BTreeMap<u64, EventState>,
}

#[derive(Debug, Deserialize)]
struct EventHeader {
    event: String,
    schedule_index: u64,
    task_id: String,
    candidate: String,
    condition: String,
    seed: i64,
    provider: String,
    model: String,
    prompt_sha256: String,
    payload_sha256: String,
}

impl EventLog {
    fn open(path: &Path, requests: &[RequestSpec]) -> Result<Self, RunnerError> {
        let expected: BTreeMap<u64, &RequestSpec> = requests
            .iter()
            .map(|request| (request.schedule_index, request))
            .collect();
        let mut states = BTreeMap::new();
        if path.exists() {
            let file = File::open(path)?;
            for (line_number, line) in BufReader::new(file).lines().enumerate() {
                let line = line?;
                if line.trim().is_empty() {
                    return Err(RunnerError::EventLog(format!(
                        "blank event at line {}",
                        line_number + 1
                    )));
                }
                let header: EventHeader = serde_json::from_str(&line).map_err(|error| {
                    RunnerError::EventLog(format!(
                        "invalid event at line {}: {}",
                        line_number + 1,
                        redact_error(&error.to_string())
                    ))
                })?;
                let request = expected.get(&header.schedule_index).ok_or_else(|| {
                    RunnerError::EventLog(format!(
                        "event at line {} references unknown schedule_index {}",
                        line_number + 1,
                        header.schedule_index
                    ))
                })?;
                if header.task_id != request.task_id
                    || header.candidate != request.candidate
                    || header.condition != request.condition
                    || header.seed != request.seed
                    || header.provider != request.provider
                    || header.model != request.model
                    || header.prompt_sha256 != request.prompt_sha256
                    || header.payload_sha256 != request.payload_sha256
                {
                    return Err(RunnerError::EventLog(format!(
                        "event at line {} does not match the frozen request identity",
                        line_number + 1
                    )));
                }
                let state: &mut EventState = states.entry(header.schedule_index).or_default();
                match header.event.as_str() {
                    "request_started" => {
                        if state.started || state.terminal {
                            return Err(RunnerError::EventLog(format!(
                                "duplicate request_started for schedule_index {}",
                                header.schedule_index
                            )));
                        }
                        state.started = true;
                    }
                    "response" | "error" | "uncertain" => {
                        if !state.started || state.terminal {
                            return Err(RunnerError::EventLog(format!(
                                "invalid terminal sequence for schedule_index {}",
                                header.schedule_index
                            )));
                        }
                        state.terminal = true;
                    }
                    other => {
                        return Err(RunnerError::EventLog(format!(
                            "unsupported event `{other}` at line {}",
                            line_number + 1
                        )));
                    }
                }
            }
        }
        let file = OpenOptions::new().create(true).append(true).open(path)?;
        Ok(Self { file, states })
    }

    fn append(&mut self, event: &serde_json::Value) -> Result<(), RunnerError> {
        serde_json::to_writer(&mut self.file, event)?;
        self.file.write_all(b"\n")?;
        // This is deliberately per event: a process crash after the provider
        // call must leave either a durable terminal event or an uncertain one.
        self.file.sync_data()?;
        Ok(())
    }

    fn state(&self, schedule_index: u64) -> EventState {
        self.states
            .get(&schedule_index)
            .cloned()
            .unwrap_or_default()
    }

    fn mark_started(&mut self, schedule_index: u64) {
        self.states.entry(schedule_index).or_default().started = true;
    }

    fn mark_terminal(&mut self, schedule_index: u64) {
        self.states.entry(schedule_index).or_default().terminal = true;
    }
}

#[derive(Clone, Debug, JsonSchema)]
struct VerdictPayload {
    verdict: String,
    reasons: Vec<String>,
}

impl<'de> Deserialize<'de> for VerdictPayload {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct VerdictVisitor;

        impl<'de> Visitor<'de> for VerdictVisitor {
            type Value = VerdictPayload;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("an object with exactly verdict and reasons fields")
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                let mut verdict = None;
                let mut reasons = None;
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "verdict" => {
                            if verdict.is_some() {
                                return Err(de::Error::duplicate_field("verdict"));
                            }
                            verdict = Some(map.next_value::<String>()?);
                        }
                        "reasons" => {
                            if reasons.is_some() {
                                return Err(de::Error::duplicate_field("reasons"));
                            }
                            reasons = Some(map.next_value::<Vec<String>>()?);
                        }
                        _ => {
                            return Err(de::Error::unknown_field(&key, &["verdict", "reasons"]));
                        }
                    }
                }
                Ok(VerdictPayload {
                    verdict: verdict.ok_or_else(|| de::Error::missing_field("verdict"))?,
                    reasons: reasons.ok_or_else(|| de::Error::missing_field("reasons"))?,
                })
            }
        }

        deserializer.deserialize_map(VerdictVisitor)
    }
}

#[derive(Clone, Debug)]
struct CollectedResponse {
    verdict: String,
    reasons: Vec<String>,
    output_sha256: String,
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    latency_ms: u64,
}

#[derive(Debug)]
struct ResponseFailure {
    message: String,
    output_sha256: Option<String>,
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    latency_ms: u64,
}

fn response_failure(
    started: Instant,
    output: &str,
    usage: Option<(u64, u64)>,
    message: String,
) -> ResponseFailure {
    ResponseFailure {
        message,
        output_sha256: (!output.trim().is_empty()).then(|| sha256_hex(output.trim().as_bytes())),
        input_tokens: usage.map(|value| value.0),
        output_tokens: usage.map(|value| value.1),
        latency_ms: started.elapsed().as_millis().min(u64::MAX as u128) as u64,
    }
}

async fn collect_response(
    provider: &dyn ModelProvider,
    request: ModelRequest,
) -> Result<CollectedResponse, ResponseFailure> {
    let started = Instant::now();
    let mut output = String::new();
    let mut usage: Option<(u64, u64)> = None;
    macro_rules! failure {
        ($message:expr $(,)?) => {
            response_failure(started, &output, usage, $message)
        };
    }
    let mut stream = match provider
        .structured_stream(request, schemars::schema_for!(VerdictPayload))
        .await
    {
        Ok(stream) => stream,
        Err(error) => return Err(failure!(redact_error(&error.to_string()))),
    };
    let mut finished = false;
    while let Some(event) = stream.next().await {
        let event = match event {
            Ok(event) => event,
            Err(error) => return Err(failure!(redact_error(&error.to_string()))),
        };
        match event {
            ProviderStreamEvent::Connected | ProviderStreamEvent::BytesReceived { .. } => {}
            ProviderStreamEvent::Model(ModelEvent::ResponseStarted { .. }) => {
                if finished {
                    return Err(failure!("response_started arrived after finished".into()));
                }
            }
            ProviderStreamEvent::Model(ModelEvent::TextDelta(delta)) => {
                if finished {
                    return Err(failure!("text arrived after finished".into()));
                }
                if output.len().saturating_add(delta.len()) > MAX_RESPONSE_BYTES {
                    return Err(failure!(format!(
                        "structured response exceeded {MAX_RESPONSE_BYTES} bytes"
                    )));
                }
                output.push_str(&delta);
            }
            ProviderStreamEvent::Model(ModelEvent::Usage {
                input_tokens,
                output_tokens,
            }) => {
                if finished {
                    return Err(failure!("usage arrived after finished".into()));
                }
                usage = Some((input_tokens, output_tokens));
            }
            ProviderStreamEvent::Model(ModelEvent::ToolCall { .. }) => {
                return Err(failure!(
                    "structured response emitted an unexpected tool call".into(),
                ));
            }
            ProviderStreamEvent::Model(ModelEvent::Finished) => {
                if finished {
                    return Err(failure!(
                        "structured response emitted duplicate finished events".into(),
                    ));
                }
                finished = true;
            }
        }
    }
    if !finished {
        return Err(failure!(
            "structured response ended without finished event".into(),
        ));
    }
    if output.trim().is_empty() {
        return Err(failure!("structured response was empty".into()));
    }
    let mut deserializer = serde_json::Deserializer::from_str(output.trim());
    let payload = match VerdictPayload::deserialize(&mut deserializer) {
        Ok(payload) => payload,
        Err(error) => {
            return Err(failure!(format!(
                "invalid verdict JSON: {}",
                redact_error(&error.to_string())
            )));
        }
    };
    if let Err(error) = deserializer.end() {
        return Err(failure!(format!(
            "trailing data after verdict JSON: {}",
            redact_error(&error.to_string())
        )));
    }
    if payload.verdict != "READY" && payload.verdict != "BLOCKED" {
        return Err(failure!("verdict must be exactly READY or BLOCKED".into()));
    }
    if payload.reasons.len() > MAX_REASONS
        || payload
            .reasons
            .iter()
            .any(|reason| reason.len() > MAX_REASON_BYTES)
    {
        return Err(failure!(format!(
            "reasons must contain at most {MAX_REASONS} strings of at most {MAX_REASON_BYTES} bytes"
        )));
    }
    let latency_ms = started.elapsed().as_millis().min(u64::MAX as u128) as u64;
    Ok(CollectedResponse {
        verdict: payload.verdict,
        reasons: payload.reasons,
        output_sha256: sha256_hex(output.trim().as_bytes()),
        input_tokens: usage.map(|value| value.0),
        output_tokens: usage.map(|value| value.1),
        latency_ms,
    })
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct RunSummary {
    completed: usize,
    skipped_terminal: usize,
    uncertain: usize,
    errors: usize,
}

type ProviderResolver<'a> = dyn Fn(&ModelId) -> Result<Arc<dyn ModelProvider>, ProviderError> + 'a;

async fn run_bundle(
    requests: &[RequestSpec],
    event_log: &mut EventLog,
    resolver: &ProviderResolver<'_>,
) -> Result<RunSummary, RunnerError> {
    let mut summary = RunSummary::default();
    for request in requests {
        let state = event_log.state(request.schedule_index);
        if state.terminal {
            summary.skipped_terminal += 1;
            continue;
        }
        if state.started {
            event_log.append(&serde_json::json!({
                "event": "uncertain",
                "schedule_index": request.schedule_index,
                "task_id": request.task_id,
                "candidate": request.candidate,
                "condition": request.condition,
                "seed": request.seed,
                "provider": request.provider,
                "model": request.model,
                "prompt_sha256": request.prompt_sha256,
                "payload_sha256": request.payload_sha256,
                "reason": "request_started_without_terminal",
            }))?;
            event_log.mark_terminal(request.schedule_index);
            summary.uncertain += 1;
            continue;
        }
        event_log.append(&serde_json::json!({
            "event": "request_started",
            "schedule_index": request.schedule_index,
            "task_id": request.task_id,
            "candidate": request.candidate,
            "condition": request.condition,
            "seed": request.seed,
            "provider": request.provider,
            "model": request.model,
            "prompt_sha256": request.prompt_sha256,
            "payload_sha256": request.payload_sha256,
            "max_output_tokens": request.max_output_tokens,
            "started_at_ms": unix_time_ms(),
        }))?;
        event_log.mark_started(request.schedule_index);

        let model_id = ModelId {
            provider: request.provider.clone(),
            model: request.model.clone(),
        };
        let provider = match resolver(&model_id) {
            Ok(provider) => provider,
            Err(error) => {
                let message = redact_error(&error.to_string());
                event_log.append(&serde_json::json!({
                    "event": "error",
                    "schedule_index": request.schedule_index,
                    "task_id": request.task_id,
                    "candidate": request.candidate,
                    "condition": request.condition,
                    "seed": request.seed,
                    "provider": request.provider,
                    "model": request.model,
                    "prompt_sha256": request.prompt_sha256,
                    "payload_sha256": request.payload_sha256,
                    "kind": "provider_resolution",
                    "message": message,
                    "latency_ms": 0_u64,
                    "input_tokens": null,
                    "output_tokens": null,
                }))?;
                event_log.mark_terminal(request.schedule_index);
                summary.errors += 1;
                continue;
            }
        };
        let model_request = ModelRequest {
            model: model_id,
            messages: vec![ModelMessage {
                role: "user".into(),
                content: request.prompt.clone(),
            }],
            tools: Vec::new(),
            max_output_tokens: Some(request.max_output_tokens),
            reasoning_effort: None,
        };
        match collect_response(provider.as_ref(), model_request).await {
            Ok(response) => {
                event_log.append(&serde_json::json!({
                    "event": "response",
                    "schedule_index": request.schedule_index,
                    "task_id": request.task_id,
                    "candidate": request.candidate,
                    "condition": request.condition,
                    "seed": request.seed,
                    "provider": request.provider,
                    "model": request.model,
                    "prompt_sha256": request.prompt_sha256,
                    "payload_sha256": request.payload_sha256,
                    "verdict": response.verdict,
                    "reasons": response
                        .reasons
                        .iter()
                        .map(|reason| redact_error(reason))
                        .collect::<Vec<_>>(),
                    "output_sha256": response.output_sha256,
                    "latency_ms": response.latency_ms,
                    "input_tokens": response.input_tokens,
                    "output_tokens": response.output_tokens,
                }))?;
                event_log.mark_terminal(request.schedule_index);
                summary.completed += 1;
            }
            Err(error) => {
                event_log.append(&serde_json::json!({
                    "event": "error",
                    "schedule_index": request.schedule_index,
                    "task_id": request.task_id,
                    "candidate": request.candidate,
                    "condition": request.condition,
                    "seed": request.seed,
                    "provider": request.provider,
                    "model": request.model,
                    "prompt_sha256": request.prompt_sha256,
                    "payload_sha256": request.payload_sha256,
                    "kind": "request",
                    "message": redact_error(&error.message),
                    "output_sha256": error.output_sha256,
                    "latency_ms": error.latency_ms,
                    "input_tokens": error.input_tokens,
                    "output_tokens": error.output_tokens,
                }))?;
                event_log.mark_terminal(request.schedule_index);
                summary.errors += 1;
            }
        }
    }
    Ok(summary)
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn sha256_json(value: &serde_json::Value) -> String {
    sha256_hex(&serde_json::to_vec(value).expect("JSON values serialize infallibly"))
}

fn unix_time_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_millis()
}

/// Error text can contain provider excerpts.  Replace common API-key forms and
/// long opaque tokens before an error reaches stderr or the durable event log.
fn redact_error(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut token = String::new();
    let flush = |output: &mut String, token: &mut String| {
        if token.len() >= 32 {
            output.push_str("<redacted>");
        } else {
            output.push_str(token);
        }
        token.clear();
    };
    for character in input.chars() {
        if character.is_ascii_alphanumeric()
            || matches!(character, '-' | '_' | '.' | '~' | '+' | '/' | '=')
        {
            token.push(character);
        } else {
            flush(&mut output, &mut token);
            output.push(character);
        }
    }
    flush(&mut output, &mut token);
    for marker in ["sk-", "sk_", "Bearer ", "api_key=", "api-key="] {
        loop {
            let lower = output.to_ascii_lowercase();
            let Some(start) = lower.find(&marker.to_ascii_lowercase()) else {
                break;
            };
            let value_start = start + marker.len();
            let value_end = output[value_start..]
                .char_indices()
                .find(|(_, character)| {
                    !character.is_ascii_alphanumeric()
                        && !matches!(character, '-' | '_' | '.' | '~' | '+' | '/' | '=')
                })
                .map(|(offset, _)| value_start + offset)
                .unwrap_or(output.len());
            if value_end == value_start {
                break;
            }
            output.replace_range(value_start..value_end, "<redacted>");
        }
    }
    output
}

struct Options {
    bundle: PathBuf,
    events: PathBuf,
    config: PathBuf,
    provider: String,
}

fn usage() -> ! {
    eprintln!(
        "usage: repository_completion_remote --bundle FILE --events FILE --config FILE --provider NAME"
    );
    std::process::exit(2)
}

fn parse_options() -> Options {
    let mut bundle = None;
    let mut events = None;
    let mut config = None;
    let mut provider = None;
    let mut arguments = std::env::args().skip(1);
    while let Some(flag) = arguments.next() {
        let mut value = || arguments.next().unwrap_or_else(|| usage());
        match flag.as_str() {
            "--bundle" => bundle = Some(PathBuf::from(value())),
            "--events" => events = Some(PathBuf::from(value())),
            "--config" => config = Some(PathBuf::from(value())),
            "--provider" => provider = Some(value()),
            "-h" | "--help" => usage(),
            _ => usage(),
        }
    }
    Options {
        bundle: bundle.unwrap_or_else(|| usage()),
        events: events.unwrap_or_else(|| usage()),
        config: config.unwrap_or_else(|| usage()),
        provider: provider.unwrap_or_else(|| usage()),
    }
}

async fn run() -> Result<(), RunnerError> {
    let options = parse_options();
    let requests = load_bundle(&options.bundle, &options.provider)?;
    let config = AppConfig::load(&options.config)
        .map_err(|error| RunnerError::Configuration(redact_error(&error.to_string())))?;
    let credentials = options.config.with_file_name(CREDENTIALS_FILE_NAME);
    let router = ProviderRouter::from_config(&config, Some(&credentials))
        .map_err(|error| RunnerError::Configuration(redact_error(&error.to_string())))?;
    let mut event_log = EventLog::open(&options.events, &requests)?;
    let summary = run_bundle(&requests, &mut event_log, &|model| router.provider(model)).await?;
    eprintln!(
        "remote research complete: completed={} skipped_terminal={} uncertain={} errors={}",
        summary.completed, summary.skipped_terminal, summary.uncertain, summary.errors
    );
    Ok(())
}

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!(
            "remote research failed: {}",
            redact_error(&error.to_string())
        );
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use futures::stream;
    use purrcode_provider_gateway::{
        ModelCapabilities, ModelEventStream, ProviderEventStream, ProviderHealth, TokenEstimate,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tempfile::tempdir;

    fn bundle_json(requests: &[serde_json::Value]) -> serde_json::Value {
        serde_json::json!({
            "schema_version": BUNDLE_SCHEMA_VERSION,
            "requests": requests,
        })
    }

    fn raw_request(index: u64, prompt: &str) -> serde_json::Value {
        serde_json::json!({
            "provider": "deepseek",
            "model": "deepseek-v4-pro",
            "schedule_index": index,
            "prompt": prompt,
            "prompt_sha256": sha256_hex(prompt.as_bytes()),
            "max_output_tokens": 128,
        })
    }

    fn jsonl_request(index: u64, prompt: &str) -> serde_json::Value {
        let payload = serde_json::json!({
            "model": "deepseek-v4-pro",
            "messages": [{"role": "user", "content": prompt}],
            "max_tokens": 128,
            "stream": true,
        });
        serde_json::json!({
            "schedule_index": index,
            "task_id": "task-1",
            "candidate": "gold",
            "condition": "revision_rubric_json",
            "seed": 17,
            "model": "deepseek-v4-pro",
            "prompt_sha256": sha256_hex(prompt.as_bytes()),
            "payload_sha256": sha256_json(&payload),
            "payload": payload,
        })
    }

    fn load_value(value: serde_json::Value) -> Result<Vec<RequestSpec>, RunnerError> {
        let directory = tempdir().unwrap();
        let path = directory.path().join("bundle.json");
        fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
        load_bundle(&path, "deepseek")
    }

    #[test]
    fn bundle_validation_rejects_duplicate_and_non_contiguous_schedule() {
        let duplicate = load_value(bundle_json(&[raw_request(0, "a"), raw_request(0, "b")]));
        assert!(
            duplicate
                .unwrap_err()
                .to_string()
                .contains("complete range")
        );
        let gap = load_value(bundle_json(&[raw_request(0, "a"), raw_request(2, "b")]));
        assert!(gap.unwrap_err().to_string().contains("complete range"));
    }

    #[test]
    fn bundle_validation_rejects_prompt_hash_tampering() {
        let mut request = raw_request(0, "frozen prompt");
        request["prompt_sha256"] = serde_json::json!("00");
        let error = load_value(bundle_json(&[request])).unwrap_err();
        assert!(error.to_string().contains("prompt_sha256"));
    }

    #[test]
    fn jsonl_bundle_extracts_prompt_and_max_tokens_from_payload() {
        let requests = load_value(jsonl_request(0, "jsonl prompt")).unwrap();
        assert_eq!(requests[0].provider, "deepseek");
        assert_eq!(requests[0].prompt, "jsonl prompt");
        assert_eq!(requests[0].max_output_tokens, 128);
        assert_eq!(requests[0].task_id, "task-1");
    }

    #[test]
    fn strict_verdict_parser_rejects_duplicate_and_unknown_fields() {
        let duplicate = r#"{"verdict":"READY","verdict":"BLOCKED","reasons":[]}"#;
        let mut deserializer = serde_json::Deserializer::from_str(duplicate);
        assert!(VerdictPayload::deserialize(&mut deserializer).is_err());
        let unknown = r#"{"verdict":"READY","reasons":[],"extra":1}"#;
        let mut deserializer = serde_json::Deserializer::from_str(unknown);
        assert!(VerdictPayload::deserialize(&mut deserializer).is_err());
    }

    #[test]
    fn redaction_covers_deepseek_and_long_opaque_tokens() {
        let key = "provider_test_token_0123456789abcdef0123456789";
        let long = "0123456789012345678901234567890123456789";
        let redacted = redact_error(&format!("bad key {key}; token={long}"));
        assert!(!redacted.contains(key));
        assert!(!redacted.contains(long));
        assert!(redacted.contains("<redacted>"));
    }

    struct MockProvider {
        calls: AtomicUsize,
        events: Vec<ProviderStreamEvent>,
    }

    impl MockProvider {
        fn new(events: Vec<ProviderStreamEvent>) -> Arc<Self> {
            Arc::new(Self {
                calls: AtomicUsize::new(0),
                events,
            })
        }
    }

    #[async_trait]
    impl ModelProvider for MockProvider {
        async fn capabilities(&self, _model: &ModelId) -> Result<ModelCapabilities, ProviderError> {
            Ok(ModelCapabilities::unknown(false))
        }

        async fn stream(&self, _request: ModelRequest) -> Result<ModelEventStream, ProviderError> {
            Ok(Box::pin(stream::empty()))
        }

        async fn structured(
            &self,
            _request: ModelRequest,
            _schema: schemars::schema::RootSchema,
        ) -> Result<serde_json::Value, ProviderError> {
            Err(ProviderError::Unavailable(
                "mock structured path unused".into(),
            ))
        }

        async fn structured_stream(
            &self,
            _request: ModelRequest,
            _schema: schemars::schema::RootSchema,
        ) -> Result<ProviderEventStream, ProviderError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(Box::pin(stream::iter(
                self.events.clone().into_iter().map(Ok),
            )))
        }

        async fn count_tokens(
            &self,
            _request: &ModelRequest,
        ) -> Result<TokenEstimate, ProviderError> {
            Ok(TokenEstimate {
                tokens: 1,
                exact: true,
            })
        }

        async fn health_check(&self) -> Result<ProviderHealth, ProviderError> {
            Ok(ProviderHealth {
                available: true,
                detail: "mock".into(),
            })
        }
    }

    fn success_events() -> Vec<ProviderStreamEvent> {
        vec![
            ProviderStreamEvent::Connected,
            ProviderStreamEvent::Model(ModelEvent::TextDelta(
                r#"{"verdict":"READY","reasons":["evidence"]}"#.into(),
            )),
            ProviderStreamEvent::Model(ModelEvent::Usage {
                input_tokens: 10,
                output_tokens: 5,
            }),
            ProviderStreamEvent::Model(ModelEvent::Finished),
        ]
    }

    #[tokio::test]
    async fn resume_marks_started_uncertain_and_does_not_call_provider() {
        let requests = load_value(bundle_json(&[raw_request(0, "resume me")])).unwrap();
        let directory = tempdir().unwrap();
        let events_path = directory.path().join("events.jsonl");
        let mut log = EventLog::open(&events_path, &requests).unwrap();
        log.append(&serde_json::json!({
            "event": "request_started",
            "schedule_index": 0,
            "task_id": requests[0].task_id,
            "candidate": requests[0].candidate,
            "condition": requests[0].condition,
            "seed": requests[0].seed,
            "provider": "deepseek",
            "model": "deepseek-v4-pro",
            "prompt_sha256": requests[0].prompt_sha256,
            "payload_sha256": requests[0].payload_sha256,
        }))
        .unwrap();
        let provider = MockProvider::new(success_events());
        log.mark_started(0);
        let summary = run_bundle(&requests, &mut log, &|_| {
            Ok(provider.clone() as Arc<dyn ModelProvider>)
        })
        .await
        .unwrap();
        assert_eq!(summary.uncertain, 1);
        assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
        let content = fs::read_to_string(events_path).unwrap();
        assert!(content.contains("request_started"));
        assert!(content.contains("request_started_without_terminal"));
    }

    #[tokio::test]
    async fn completed_request_is_terminal_and_resume_skips_it() {
        let requests = load_value(bundle_json(&[raw_request(0, "complete me")])).unwrap();
        let directory = tempdir().unwrap();
        let events_path = directory.path().join("events.jsonl");
        let provider = MockProvider::new(success_events());
        let resolver = |_: &ModelId| Ok(provider.clone() as Arc<dyn ModelProvider>);
        let mut log = EventLog::open(&events_path, &requests).unwrap();
        let first = run_bundle(&requests, &mut log, &resolver).await.unwrap();
        assert_eq!(first.completed, 1);
        assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
        drop(log);
        let mut resumed = EventLog::open(&events_path, &requests).unwrap();
        let second = run_bundle(&requests, &mut resumed, &resolver)
            .await
            .unwrap();
        assert_eq!(second.skipped_terminal, 1);
        assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
        let content = fs::read_to_string(events_path).unwrap();
        assert!(content.contains("\"event\":\"response\""));
        assert!(content.contains("\"input_tokens\":10"));
    }
}
