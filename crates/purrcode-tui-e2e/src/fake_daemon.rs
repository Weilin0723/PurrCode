//! A deterministic stand-in for `purrcoded`.
//!
//! Speaks the same HTTP surface the workbench calls, with durable state a test
//! scripts up front. Determinism is the point: a real daemon driving a real model
//! cannot produce the same event sequence twice, so an acceptance test built on
//! one could never distinguish a UI regression from provider variance.
//!
//! What this deliberately does **not** do is relax any rule the real daemon
//! enforces. Approval requires an `awaiting_approval` boundary; a decision for a
//! non-pending action is refused with the same 409 the real daemon returns. A
//! test that passes here would pass against the real approval endpoint.

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use serde_json::{json, Value};

/// Everything a test can script before the workbench starts.
#[derive(Clone, Debug, Default)]
pub struct DaemonScript {
    /// Provider profiles the daemon reports.
    pub providers: Vec<Value>,
    /// Models the daemon reports; exactly one should carry `"default": true`.
    pub models: Vec<Value>,
    /// Durable sessions, keyed by session id.
    pub sessions: BTreeMap<String, ScriptedSession>,
    /// Returned by `POST /v1/repository/inspect`.
    pub repository_dirty: bool,
    /// When true every request fails, simulating an unreachable daemon.
    pub unreachable: bool,
    /// Patch and porcelain returned by the diff endpoint.
    pub diff: Option<(String, String)>,
    /// Terminals the daemon reports, and the raw PTY bytes each one has
    /// produced. Bytes are served incrementally through
    /// `/v1/terminals/{id}/output?since=`, exactly as the real daemon does, so
    /// a test can prove the workbench interprets escape sequences rather than
    /// printing them.
    pub terminals: Vec<ScriptedTerminal>,
    /// Models a discovery request reports.
    pub discovered_models: Vec<String>,
    /// When set, provider save and test fail with this message.
    pub provider_failure: Option<String>,
    /// Frames the SSE stream emits for the next task, in order.
    ///
    /// Written as already-encoded frames so a test can express content deltas,
    /// phase changes and durable audits exactly as the daemon would.
    pub stream_frames: Vec<StreamFrame>,
    /// When true the stream endpoint fails, exercising the transport-error path.
    pub stream_fails: bool,
    /// When true the stream stays open after emitting its frames.
    ///
    /// Cancellation is only meaningful while a response is genuinely in flight,
    /// so a test that cancels mid-stream needs the connection held open rather
    /// than closed as soon as the last frame is written.
    pub stream_stays_open: bool,
    /// Skills the registry reports.
    pub skills: Vec<Value>,
    /// When set, skill download and install proposals fail with this message.
    pub skill_failure: Option<String>,
    /// Publishers blocked by local policy.
    pub blocked_publishers: Vec<String>,
    /// Capability search candidates, in the same manifest+score shape the
    /// real daemon returns.
    pub search_candidates: Vec<Value>,
    /// When true, a capability search parks on an approval boundary before
    /// returning candidates, exactly like an MCP tool discovery would.
    pub search_requires_approval: bool,
    /// Model-pull lifecycle, so a start can be refused without an approval.
    pub pull_approved: bool,
    pub pull_running: bool,
    pub pull_cancelled: bool,
    /// Scripted response for `GET /v1/local-models`. `None` uses a plain
    /// "nothing loaded" default.
    pub local_models: Option<Value>,
    /// Scripted response for `GET /v1/local-models/recommendations`. `None`
    /// uses a "no recommendation" default.
    pub recommendations: Option<Value>,
    /// Scripted response for `POST /v1/local-models/qualify`. `None` uses a
    /// passing default.
    pub qualification: Option<Value>,
    /// When set, removing this named provider is refused because a session
    /// still depends on it.
    pub provider_in_use: Option<String>,
}

/// One server-sent frame.
#[derive(Clone, Debug)]
pub enum StreamFrame {
    /// Assistant text arriving incrementally.
    ContentDelta(String),
    /// A durable event, which also lands in the session's event list.
    DurableAudit(Value),
    /// A runtime phase change.
    Phase { phase: String, role: String },
}

impl StreamFrame {
    fn encode(&self, sequence: u64) -> String {
        match self {
            Self::ContentDelta(delta) => format!(
                "id: {sequence}\nevent: content_delta\ndata: {}\n\n",
                json!({"kind": "content_delta", "delta": delta, "role": "coder"})
            ),
            Self::DurableAudit(event) => format!(
                "id: {sequence}\nevent: durable_audit\ndata: {}\n\n",
                json!({"kind": "durable_audit", "sequence": sequence, "event": event})
            ),
            Self::Phase { phase, role } => format!(
                "id: {sequence}\nevent: phase\ndata: {}\n\n",
                json!({"kind": "phase", "phase": phase, "role": role})
            ),
        }
    }
}

/// One terminal the fake daemon serves.
#[derive(Clone, Debug)]
pub struct ScriptedTerminal {
    pub terminal_id: String,
    pub alive: bool,
    pub generation: u64,
    /// Everything the PTY has produced, including escape sequences.
    pub output: Vec<u8>,
    /// Set once the workbench asks for control.
    pub owner_is_human: bool,
}

impl ScriptedTerminal {
    pub fn new(terminal_id: &str, output: impl AsRef<[u8]>) -> Self {
        Self {
            terminal_id: terminal_id.to_owned(),
            alive: true,
            generation: 0,
            output: output.as_ref().to_vec(),
            owner_is_human: false,
        }
    }

    fn snapshot(&self) -> Value {
        json!({
            "terminal_id": self.terminal_id,
            "alive": self.alive,
            "generation": self.generation,
            "owner": if self.owner_is_human {
                json!({"kind": "human"})
            } else {
                json!({"kind": "agent", "data": {"role": "Coding Agent"}})
            },
            "transcript_tail": [],
        })
    }
}

#[derive(Clone, Debug, Default)]
pub struct ScriptedSession {
    pub status_code: String,
    pub lease_active: bool,
    pub events: Vec<Value>,
    pub messages: Vec<Value>,
    /// Present when the session sits on an approval boundary.
    pub awaiting_action: Option<String>,
}

impl ScriptedSession {
    pub fn new(status_code: &str) -> Self {
        Self {
            status_code: status_code.to_owned(),
            ..Self::default()
        }
    }

    pub fn with_events(mut self, events: Vec<Value>) -> Self {
        self.events = events;
        self
    }

    pub fn with_messages(mut self, messages: Vec<Value>) -> Self {
        self.messages = messages;
        self
    }

    pub fn awaiting(mut self, action_id: &str) -> Self {
        self.status_code = "awaiting_approval".to_owned();
        self.awaiting_action = Some(action_id.to_owned());
        self
    }
}

/// Requests the workbench made, so a test can assert that pressing a key really
/// reached the daemon endpoint rather than only changing the screen.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecordedRequest {
    pub method: String,
    pub path: String,
    pub body: Option<String>,
}

#[derive(Clone)]
struct DaemonState {
    script: Arc<Mutex<DaemonScript>>,
    requests: Arc<Mutex<Vec<RecordedRequest>>>,
    token: String,
}

pub struct FakeDaemon {
    address: SocketAddr,
    script: Arc<Mutex<DaemonScript>>,
    requests: Arc<Mutex<Vec<RecordedRequest>>>,
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
    runtime: Option<std::thread::JoinHandle<()>>,
}

impl FakeDaemon {
    /// Bind on an ephemeral port and serve until dropped.
    pub fn start(script: DaemonScript, token: &str) -> Result<Self> {
        let script = Arc::new(Mutex::new(script));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let state = DaemonState {
            script: Arc::clone(&script),
            requests: Arc::clone(&requests),
            token: token.to_owned(),
        };
        let (address_tx, address_rx) = std::sync::mpsc::channel();
        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();

        // The daemon runs on its own runtime thread so the harness stays a plain
        // synchronous API: tests drive keys and assert on screens, not futures.
        let runtime = std::thread::spawn(move || {
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(_) => return,
            };
            runtime.block_on(async move {
                let listener = match tokio::net::TcpListener::bind("127.0.0.1:0").await {
                    Ok(listener) => listener,
                    Err(_) => return,
                };
                let Ok(address) = listener.local_addr() else {
                    return;
                };
                let _ = address_tx.send(address);
                let app = router(state);
                let _ = axum::serve(listener, app)
                    .with_graceful_shutdown(async {
                        let _ = shutdown_rx.await;
                    })
                    .await;
            });
        });

        let address = address_rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .context("fake daemon did not report its address")?;
        Ok(Self {
            address,
            script,
            requests,
            shutdown: Some(shutdown_tx),
            runtime: Some(runtime),
        })
    }

    pub fn url(&self) -> String {
        format!("http://{}", self.address)
    }

    pub fn port(&self) -> u16 {
        self.address.port()
    }

    /// Mutate the scripted state mid-test, e.g. to advance a session.
    pub fn update(&self, change: impl FnOnce(&mut DaemonScript)) {
        let mut script = self.script.lock().expect("script mutex");
        change(&mut script);
    }

    pub fn requests(&self) -> Vec<RecordedRequest> {
        self.requests.lock().expect("requests mutex").clone()
    }

    /// True when a request matching `method` and a path containing `fragment`
    /// was received.
    pub fn saw(&self, method: &str, fragment: &str) -> bool {
        self.requests()
            .iter()
            .any(|request| request.method == method && request.path.contains(fragment))
    }

    pub fn request_log(&self) -> String {
        self.requests()
            .iter()
            .map(|request| {
                format!(
                    "{} {}{}",
                    request.method,
                    request.path,
                    request
                        .body
                        .as_deref()
                        .map(|body| format!(" {body}"))
                        .unwrap_or_default()
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

impl Drop for FakeDaemon {
    fn drop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        if let Some(runtime) = self.runtime.take() {
            let _ = runtime.join();
        }
    }
}

fn router(state: DaemonState) -> Router {
    Router::new()
        .route("/v1/providers", get(providers).post(save_provider))
        .route("/v1/providers/discover", post(discover))
        .route("/v1/providers/test", post(test_provider))
        .route(
            "/v1/providers/{name}",
            get(provider).delete(remove_provider),
        )
        .route("/v1/models", get(models))
        .route("/v1/models/roles", post(assign_role))
        .route("/v1/credentials", post(store_credential))
        .route("/v1/repository/inspect", post(inspect_repository))
        .route("/v1/sessions", get(sessions).post(create_session))
        .route("/v1/sessions/{id}", get(session))
        .route("/v1/sessions/{id}/events", get(events))
        .route("/v1/sessions/{id}/events/stream", get(event_stream))
        .route(
            "/v1/sessions/{id}/messages",
            get(messages).post(add_message),
        )
        .route("/v1/sessions/{id}/diff", get(diff))
        .route("/v1/sessions/{id}/approve", post(approve))
        .route("/v1/sessions/{id}/deny", post(deny))
        .route("/v1/sessions/{id}/cancel", post(cancel))
        .route("/v1/sessions/{id}/pause", post(pause))
        .route("/v1/sessions/{id}/resume", post(resume))
        .route("/v1/sessions/{id}/rollback", post(rollback))
        .route("/v1/terminals", get(terminals).post(start_terminal))
        .route("/v1/terminals/{id}/output", get(terminal_output))
        .route("/v1/terminals/{id}/input", post(terminal_input))
        .route("/v1/terminals/{id}/owner", post(terminal_owner))
        .route("/v1/local-models", get(local_models))
        .route("/v1/local-models/recommendations", get(recommendations))
        // Present so a request never 404s into a confusing error; the tests that
        // exercise these paths script their own responses.
        .route("/v1/skills", get(skills))
        .route("/v1/skills/search", post(skill_search))
        .route("/v1/skills/download", post(skill_download))
        .route("/v1/skills/install/propose", post(skill_install_propose))
        .route(
            "/v1/skills/install/{action}/approve",
            post(skill_install_approve),
        )
        .route("/v1/skills/install", post(skill_install_finalize))
        .route("/v1/skills/publishers/block", post(block_publisher))
        .route("/v1/research/fetch", post(research_fetch))
        .route("/v1/sessions/{id}/compact", post(compact))
        .route("/v1/sessions/{id}/model", post(select_session_model))
        .route("/v1/local-models/qualify", post(qualify))
        .route("/v1/local-models/unload", post(unload))
        .route("/v1/local-models/pull/propose", post(pull_propose))
        .route("/v1/local-models/pull/{action}/approve", post(pull_approve))
        .route("/v1/local-models/pull/{action}/start", post(pull_start))
        .route("/v1/local-models/pull/{action}/cancel", post(pull_cancel))
        .route("/v1/local-models/pull/{action}", get(pull_progress))
        .route("/v1/providers/{name}/edit", delete(accepted))
        .with_state(state)
}

type ApiResult = Result<Json<Value>, (StatusCode, Json<Value>)>;

/// Record a request, redacting any secret-bearing field first.
///
/// Redaction happens here rather than in each handler because the request log
/// becomes a CI artifact: a handler that rejects a raw secret would still have
/// written it to disk if it recorded the body first.
fn record(state: &DaemonState, method: &str, path: &str, body: Option<&Value>) {
    state
        .requests
        .lock()
        .expect("requests mutex")
        .push(RecordedRequest {
            method: method.to_owned(),
            path: path.to_owned(),
            body: body.map(|body| redact(body).to_string()),
        });
}

/// Field names whose values are never written to an artifact.
const SECRET_FIELDS: &[&str] = &["api_key", "secret", "token", "password", "credential"];

fn redact(body: &Value) -> Value {
    match body {
        Value::Object(fields) => Value::Object(
            fields
                .iter()
                .map(|(key, value)| {
                    let redacted = if SECRET_FIELDS
                        .iter()
                        .any(|field| key.to_ascii_lowercase().contains(field))
                    {
                        Value::String("[redacted]".to_owned())
                    } else {
                        redact(value)
                    };
                    (key.clone(), redacted)
                })
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(redact).collect()),
        other => other.clone(),
    }
}

fn authorize(state: &DaemonState, headers: &HeaderMap) -> Result<(), (StatusCode, Json<Value>)> {
    let presented = headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .unwrap_or_default();
    if presented == state.token {
        return Ok(());
    }
    Err((
        StatusCode::UNAUTHORIZED,
        Json(json!({"error": "invalid token"})),
    ))
}

/// Reject every request when the script says the daemon is unreachable.
fn availability(state: &DaemonState) -> Result<(), (StatusCode, Json<Value>)> {
    if state.script.lock().expect("script mutex").unreachable {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"error": "daemon unavailable"})),
        ));
    }
    Ok(())
}

macro_rules! guard {
    ($state:expr, $headers:expr) => {{
        authorize(&$state, &$headers)?;
        availability(&$state)?;
    }};
}

async fn providers(State(state): State<DaemonState>, headers: HeaderMap) -> ApiResult {
    guard!(state, headers);
    record(&state, "GET", "/v1/providers", None);
    let script = state.script.lock().expect("script mutex");
    Ok(Json(Value::Array(script.providers.clone())))
}

async fn provider(
    State(state): State<DaemonState>,
    headers: HeaderMap,
    Path(name): Path<String>,
) -> ApiResult {
    guard!(state, headers);
    record(&state, "GET", &format!("/v1/providers/{name}"), None);
    let script = state.script.lock().expect("script mutex");
    let entry = script
        .providers
        .iter()
        .find(|provider| provider["name"].as_str() == Some(name.as_str()))
        .ok_or((StatusCode::NOT_FOUND, Json(json!({"error": "not found"}))))?;
    // `ProviderSetup::from_saved` expects a `configuration` object wrapping the
    // provider type and endpoint, plus a top-level `models` list — a different
    // shape than what gets POSTed to save one. This mirrors that real shape so
    // `/provider edit` exercises the same parsing path production code does.
    let credential = entry
        .get("credential_reference")
        .and_then(Value::as_str)
        .or_else(|| entry.get("credential_name").and_then(Value::as_str));
    Ok(Json(json!({
        "name": entry["name"],
        "configuration": {
            "type": entry.get("provider_type").cloned().unwrap_or(json!("openai-compatible")),
            "base_url": entry.get("base_url").cloned().unwrap_or(json!("")),
            "api_key_env": credential,
            "local": entry.get("local").cloned().unwrap_or(json!(false)),
            "headers": {},
            "capabilities": {},
        },
        "models": entry
            .get("model")
            .and_then(Value::as_str)
            .map(|model| vec![model.to_owned()])
            .unwrap_or_default(),
    })))
}

async fn save_provider(
    State(state): State<DaemonState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> ApiResult {
    guard!(state, headers);
    record(&state, "POST", "/v1/providers", Some(&body));
    let mut script = state.script.lock().expect("script mutex");
    if let Some(failure) = script.provider_failure.clone() {
        return Err((
            StatusCode::BAD_GATEWAY,
            Json(json!({"error": failure, "detail": failure})),
        ));
    }
    // Refuse to record a secret: the real daemon stores a reference, and a test
    // that could smuggle a raw key through here would not be proving anything.
    if body.get("api_key").is_some() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "raw secrets are never accepted"})),
        ));
    }
    script.providers.push(body.clone());
    Ok(Json(json!({
        "name": body["name"],
        "latency_ms": 12,
        "detail": "healthy response",
    })))
}

async fn remove_provider(
    State(state): State<DaemonState>,
    headers: HeaderMap,
    Path(name): Path<String>,
) -> ApiResult {
    guard!(state, headers);
    record(&state, "DELETE", &format!("/v1/providers/{name}"), None);
    let mut script = state.script.lock().expect("script mutex");
    if script.provider_in_use.as_deref() == Some(name.as_str()) {
        return Err((
            StatusCode::CONFLICT,
            Json(json!({
                "error": format!("provider `{name}` is in use by an active session and cannot be removed")
            })),
        ));
    }
    script
        .providers
        .retain(|provider| provider["name"].as_str() != Some(name.as_str()));
    Ok(Json(json!({"removed": name})))
}

async fn test_provider(
    State(state): State<DaemonState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> ApiResult {
    guard!(state, headers);
    record(&state, "POST", "/v1/providers/test", Some(&body));
    let script = state.script.lock().expect("script mutex");
    match script.provider_failure.clone() {
        Some(failure) => Err((
            StatusCode::BAD_GATEWAY,
            Json(json!({"error": failure, "detail": failure})),
        )),
        None => Ok(Json(json!({"latency_ms": 9, "detail": "healthy response"}))),
    }
}

async fn discover(
    State(state): State<DaemonState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> ApiResult {
    guard!(state, headers);
    record(&state, "POST", "/v1/providers/discover", Some(&body));
    let script = state.script.lock().expect("script mutex");
    Ok(Json(json!({"models": script.discovered_models.clone()})))
}

async fn models(State(state): State<DaemonState>, headers: HeaderMap) -> ApiResult {
    guard!(state, headers);
    record(&state, "GET", "/v1/models", None);
    let script = state.script.lock().expect("script mutex");
    Ok(Json(Value::Array(script.models.clone())))
}

async fn assign_role(
    State(state): State<DaemonState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> ApiResult {
    guard!(state, headers);
    record(&state, "POST", "/v1/models/roles", Some(&body));
    Ok(Json(json!({"assigned": body})))
}

async fn store_credential(
    State(state): State<DaemonState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> ApiResult {
    guard!(state, headers);
    record(&state, "POST", "/v1/credentials", Some(&body));
    Ok(Json(json!({"stored": body["name"]})))
}

async fn inspect_repository(
    State(state): State<DaemonState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> ApiResult {
    guard!(state, headers);
    record(&state, "POST", "/v1/repository/inspect", Some(&body));
    let script = state.script.lock().expect("script mutex");
    Ok(Json(json!({"dirty": script.repository_dirty})))
}

async fn sessions(State(state): State<DaemonState>, headers: HeaderMap) -> ApiResult {
    guard!(state, headers);
    record(&state, "GET", "/v1/sessions", None);
    let script = state.script.lock().expect("script mutex");
    Ok(Json(Value::Array(
        script
            .sessions
            .iter()
            .map(|(id, session)| session_view(id, session))
            .collect(),
    )))
}

async fn create_session(
    State(state): State<DaemonState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> ApiResult {
    guard!(state, headers);
    record(&state, "POST", "/v1/sessions", Some(&body));
    let id = uuid::Uuid::new_v4().to_string();
    let objective = body["objective"].as_str().unwrap_or_default().to_owned();
    let mut script = state.script.lock().expect("script mutex");
    script.sessions.insert(
        id.clone(),
        ScriptedSession::new("active").with_events(vec![json!({
            "event": "session_created",
            "data": {"objective": objective, "repository": body["repository"]}
        })]),
    );
    Ok(Json(json!({"id": id, "status": "active"})))
}

async fn session(
    State(state): State<DaemonState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> ApiResult {
    guard!(state, headers);
    record(&state, "GET", &format!("/v1/sessions/{id}"), None);
    let script = state.script.lock().expect("script mutex");
    script
        .sessions
        .get(&id)
        .map(|session| Json(session_view(&id, session)))
        .ok_or((StatusCode::NOT_FOUND, Json(json!({"error": "not found"}))))
}

fn session_view(id: &str, session: &ScriptedSession) -> Value {
    json!({
        "id": id,
        "status_code": session.status_code,
        "event_count": session.events.len(),
        "lease_active": session.lease_active,
        "repository": null,
        "worktree": null,
        "selected_model": null,
    })
}

async fn events(
    State(state): State<DaemonState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> ApiResult {
    guard!(state, headers);
    record(&state, "GET", &format!("/v1/sessions/{id}/events"), None);
    let script = state.script.lock().expect("script mutex");
    script
        .sessions
        .get(&id)
        .map(|session| Json(Value::Array(session.events.clone())))
        .ok_or((StatusCode::NOT_FOUND, Json(json!({"error": "not found"}))))
}

/// The live event stream, emitted as one response body.
///
/// Frames are written in order and the stream then closes, which is exactly what
/// the workbench sees when a turn finishes. Durable audits are also appended to
/// the session so a later reconciliation agrees with what streamed.
async fn event_stream(
    State(state): State<DaemonState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<axum::response::Response, (StatusCode, Json<Value>)> {
    guard!(state, headers);
    record(
        &state,
        "GET",
        &format!("/v1/sessions/{id}/events/stream"),
        None,
    );
    let (frames, fails) = {
        let mut script = state.script.lock().expect("script mutex");
        if script.stream_fails {
            (Vec::new(), true)
        } else {
            let frames = std::mem::take(&mut script.stream_frames);
            // Durable audits must survive the stream, or a reconnect would lose
            // them and the interface would appear to forget what happened.
            if let Some(session) = script.sessions.get_mut(&id) {
                for frame in &frames {
                    if let StreamFrame::DurableAudit(event) = frame {
                        session.events.push(event.clone());
                    }
                    if let StreamFrame::DurableAudit(event) = frame {
                        match event["event"].as_str() {
                            Some("session_completed") => {
                                session.status_code = "completed".to_owned()
                            }
                            Some("session_failed") => session.status_code = "failed".to_owned(),
                            Some("session_cancelled") => {
                                session.status_code = "cancelled".to_owned()
                            }
                            Some("approval_requested") => {
                                session.status_code = "awaiting_approval".to_owned();
                                if let Some(action) =
                                    event.pointer("/data/action_id").and_then(Value::as_str)
                                {
                                    session.awaiting_action = Some(action.to_owned());
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
            (frames, false)
        }
    };
    if fails {
        return Err((
            StatusCode::BAD_GATEWAY,
            Json(json!({"error": "provider stream failed"})),
        ));
    }
    let body: String = frames
        .iter()
        .enumerate()
        .map(|(index, frame)| frame.encode(index as u64 + 1))
        .collect();
    let stays_open = state.script.lock().expect("script mutex").stream_stays_open;
    let body = if stays_open {
        // Emit the frames, then hold the connection with SSE comments. The
        // workbench sees a live, unfinished response — the only state in which
        // cancelling means anything.
        let initial = Some(body);
        axum::body::Body::from_stream(futures::stream::unfold(
            (initial, 0usize),
            |(initial, ticks)| async move {
                if let Some(initial) = initial {
                    return Some((
                        Ok::<_, std::io::Error>(axum::body::Bytes::from(initial)),
                        (None, ticks),
                    ));
                }
                // Bounded well under the harness timeout so a held stream cannot
                // starve the rest of the suite.
                if ticks > 150 {
                    return None;
                }
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                Some((
                    Ok(axum::body::Bytes::from_static(b": keep-alive\n\n")),
                    (None, ticks + 1),
                ))
            },
        ))
    } else {
        axum::body::Body::from(body)
    };
    Ok(axum::response::Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "text/event-stream")
        .body(body)
        .expect("build stream response"))
}

async fn messages(
    State(state): State<DaemonState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> ApiResult {
    guard!(state, headers);
    record(&state, "GET", &format!("/v1/sessions/{id}/messages"), None);
    let script = state.script.lock().expect("script mutex");
    script
        .sessions
        .get(&id)
        .map(|session| Json(Value::Array(session.messages.clone())))
        .ok_or((StatusCode::NOT_FOUND, Json(json!({"error": "not found"}))))
}

async fn add_message(
    State(state): State<DaemonState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> ApiResult {
    guard!(state, headers);
    record(
        &state,
        "POST",
        &format!("/v1/sessions/{id}/messages"),
        Some(&body),
    );
    let mut script = state.script.lock().expect("script mutex");
    let Some(session) = script.sessions.get_mut(&id) else {
        return Err((StatusCode::NOT_FOUND, Json(json!({"error": "not found"}))));
    };
    // A session another client holds the lease on cannot accept new work: this is
    // the 409 the workbench turns into its lease-conflict surface.
    if session.lease_active {
        return Err((
            StatusCode::CONFLICT,
            Json(json!({"error": "session lease is held by another client"})),
        ));
    }
    session.messages.push(json!({
        "id": uuid::Uuid::new_v4().to_string(),
        "role": "user",
        "content": body["content"],
        "timestamp": "2026-01-01T00:00:00Z",
        "model": null,
    }));
    Ok(Json(json!({"accepted": true})))
}

async fn terminals(State(state): State<DaemonState>, headers: HeaderMap) -> ApiResult {
    guard!(state, headers);
    record(&state, "GET", "/v1/terminals", None);
    let script = state.script.lock().expect("script mutex");
    Ok(Json(json!({
        "terminals": script.terminals.iter().map(ScriptedTerminal::snapshot).collect::<Vec<_>>(),
    })))
}

async fn start_terminal(
    State(state): State<DaemonState>,
    headers: HeaderMap,
    body: String,
) -> ApiResult {
    guard!(state, headers);
    let parsed: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
    record(&state, "POST", "/v1/terminals", Some(&parsed));
    let mut script = state.script.lock().expect("script mutex");
    let terminal =
        ScriptedTerminal::new(&format!("terminal-{}", script.terminals.len() + 1), b"$ ");
    script.terminals.push(terminal.clone());
    Ok(Json(json!({ "terminal": terminal.snapshot() })))
}

#[derive(serde::Deserialize)]
struct SinceQuery {
    #[serde(default)]
    since: u64,
}

async fn terminal_output(
    State(state): State<DaemonState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    axum::extract::Query(query): axum::extract::Query<SinceQuery>,
) -> ApiResult {
    guard!(state, headers);
    let script = state.script.lock().expect("script mutex");
    let Some(terminal) = script.terminals.iter().find(|t| t.terminal_id == id) else {
        return Err((
            StatusCode::NOT_FOUND,
            Json(json!({"error": format!("terminal {id} not found")})),
        ));
    };
    let start = (query.since as usize).min(terminal.output.len());
    Ok(Json(json!({
        "chunk": {
            "bytes": terminal.output[start..],
            "next_offset": terminal.output.len(),
            "truncated": false,
        },
        "alive": terminal.alive,
        "generation": terminal.generation,
        "owner": terminal.snapshot()["owner"],
    })))
}

async fn terminal_input(
    State(state): State<DaemonState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: String,
) -> ApiResult {
    guard!(state, headers);
    let parsed: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
    record(
        &state,
        "POST",
        &format!("/v1/terminals/{id}/input"),
        Some(&parsed),
    );
    let mut script = state.script.lock().expect("script mutex");
    let Some(terminal) = script.terminals.iter_mut().find(|t| t.terminal_id == id) else {
        return Err((
            StatusCode::NOT_FOUND,
            Json(json!({"error": format!("terminal {id} not found")})),
        ));
    };
    // Echo the typed bytes, exactly as a PTY in cooked mode does, so a test can
    // prove input reached the process.
    if let Some(input) = serde_json::from_str::<Value>(&body)
        .ok()
        .and_then(|value| value["input"].as_str().map(str::to_owned))
    {
        terminal.output.extend_from_slice(input.as_bytes());
    }
    Ok(Json(json!({"accepted": true})))
}

async fn terminal_owner(
    State(state): State<DaemonState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: String,
) -> ApiResult {
    guard!(state, headers);
    let parsed: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
    record(
        &state,
        "POST",
        &format!("/v1/terminals/{id}/owner"),
        Some(&parsed),
    );
    let human = body.contains("human");
    let mut script = state.script.lock().expect("script mutex");
    let Some(terminal) = script.terminals.iter_mut().find(|t| t.terminal_id == id) else {
        return Err((
            StatusCode::NOT_FOUND,
            Json(json!({"error": format!("terminal {id} not found")})),
        ));
    };
    if terminal.owner_is_human != human {
        terminal.owner_is_human = human;
        // Ownership transfer bumps the generation, so stale input is rejected.
        terminal.generation += 1;
    }
    Ok(Json(json!({ "terminal": terminal.snapshot() })))
}

async fn diff(
    State(state): State<DaemonState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> ApiResult {
    guard!(state, headers);
    record(&state, "GET", &format!("/v1/sessions/{id}/diff"), None);
    let script = state.script.lock().expect("script mutex");
    match &script.diff {
        Some((patch, porcelain)) => Ok(Json(json!({
            "patch": patch,
            "changed_files": [],
            "status": porcelain,
        }))),
        // No recorded worktree is a conflict, exactly as the real daemon reports
        // it — the workbench must render "unavailable", not "no changes".
        None => Err((
            StatusCode::CONFLICT,
            Json(json!({"error": "session has no isolated worktree"})),
        )),
    }
}

/// Approval, with the real daemon's boundary rule.
async fn approve(
    State(state): State<DaemonState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> ApiResult {
    guard!(state, headers);
    record(&state, "POST", &format!("/v1/sessions/{id}/approve"), None);
    decide(&state, &id, true)
}

async fn deny(
    State(state): State<DaemonState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: Option<Json<Value>>,
) -> ApiResult {
    guard!(state, headers);
    record(
        &state,
        "POST",
        &format!("/v1/sessions/{id}/deny"),
        body.as_ref().map(|Json(body)| body),
    );
    decide(&state, &id, false)
}

fn decide(state: &DaemonState, id: &str, approve: bool) -> ApiResult {
    let mut script = state.script.lock().expect("script mutex");
    let Some(session) = script.sessions.get_mut(id) else {
        return Err((StatusCode::NOT_FOUND, Json(json!({"error": "not found"}))));
    };
    let Some(action_id) = session.awaiting_action.clone() else {
        return Err((
            StatusCode::CONFLICT,
            Json(json!({
                "error": "no action is awaiting approval; approve/deny are available only when an approval card is visible"
            })),
        ));
    };
    session.awaiting_action = None;
    if approve {
        session.status_code = "executing".to_owned();
        session.events.push(json!({
            "event": "approval_recorded",
            "data": {"action_id": action_id, "action_digest": "recorded-by-daemon"}
        }));
        session.events.push(json!({
            "event": "execution_started",
            "data": {"action_id": action_id}
        }));
    } else {
        session.status_code = "active".to_owned();
        session.events.push(json!({
            "event": "approval_rejected",
            "data": {"action_id": action_id, "reason": "rejected by the user"}
        }));
    }
    Ok(Json(json!({
        "id": id,
        "status": if approve { "approval accepted" } else { "rejection accepted" }
    })))
}

async fn cancel(
    State(state): State<DaemonState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> ApiResult {
    guard!(state, headers);
    record(&state, "POST", &format!("/v1/sessions/{id}/cancel"), None);
    let mut script = state.script.lock().expect("script mutex");
    if let Some(session) = script.sessions.get_mut(&id) {
        session.status_code = "cancelled".to_owned();
        session.events.push(json!({
            "event": "session_cancelled",
            "data": {"reason": "cancelled by the user"}
        }));
    }
    Ok(Json(json!({"id": id, "status": "cancelled"})))
}

async fn pause(
    State(state): State<DaemonState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: Option<Json<Value>>,
) -> ApiResult {
    guard!(state, headers);
    record(
        &state,
        "POST",
        &format!("/v1/sessions/{id}/pause"),
        body.as_ref().map(|Json(body)| body),
    );
    let mut script = state.script.lock().expect("script mutex");
    if let Some(session) = script.sessions.get_mut(&id) {
        session.status_code = "paused".to_owned();
    }
    Ok(Json(json!({"id": id, "status": "paused"})))
}

async fn resume(
    State(state): State<DaemonState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> ApiResult {
    guard!(state, headers);
    record(&state, "POST", &format!("/v1/sessions/{id}/resume"), None);
    let mut script = state.script.lock().expect("script mutex");
    if let Some(session) = script.sessions.get_mut(&id) {
        session.status_code = "active".to_owned();
    }
    Ok(Json(json!({"id": id, "status": "resumed"})))
}

async fn rollback(
    State(state): State<DaemonState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> ApiResult {
    guard!(state, headers);
    record(&state, "POST", &format!("/v1/sessions/{id}/rollback"), None);
    let script = state.script.lock().expect("script mutex");
    if script.diff.is_none() {
        return Err((
            StatusCode::CONFLICT,
            Json(json!({"error": "session has no agent-owned worktree to roll back"})),
        ));
    }
    Ok(Json(json!({"id": id, "status": "rolled back"})))
}

async fn local_models(State(state): State<DaemonState>, headers: HeaderMap) -> ApiResult {
    guard!(state, headers);
    record(&state, "GET", "/v1/local-models", None);
    let script = state.script.lock().expect("script mutex");
    Ok(Json(script.local_models.clone().unwrap_or_else(|| {
        json!({
            "loaded": [],
            "resources": {"memory_pressure": "low", "maximum_local_inference_requests": 1},
        })
    })))
}

/// The two call sites in the real client read different shapes from this one
/// response: the startup safety check reads `cards`/`outcome` at the top
/// level, while `/model recommend` reads the richer report nested under
/// `report`. Both must be present for the fixture to be faithful.
async fn recommendations(State(state): State<DaemonState>, headers: HeaderMap) -> ApiResult {
    guard!(state, headers);
    record(&state, "GET", "/v1/local-models/recommendations", None);
    let script = state.script.lock().expect("script mutex");
    Ok(Json(script.recommendations.clone().unwrap_or_else(|| {
        json!({
            "outcome": {"status": "no_recommendation"},
            "cards": [],
            "report": {
                "outcome": {"status": "no_recommendation", "reasons": []},
                "cards": [],
                "constraints": {},
                "resource_risks": [],
            },
        })
    })))
}

async fn skills(State(state): State<DaemonState>, headers: HeaderMap) -> ApiResult {
    guard!(state, headers);
    record(&state, "GET", "/v1/skills", None);
    let script = state.script.lock().expect("script mutex");
    Ok(Json(Value::Array(script.skills.clone())))
}

/// A capability search: candidates directly, or a `requires_approval`
/// boundary first when the script asks for one — the same shape an MCP
/// server discovery search uses, since both normalize to this one endpoint.
async fn skill_search(
    State(state): State<DaemonState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> ApiResult {
    guard!(state, headers);
    record(&state, "POST", "/v1/skills/search", Some(&body));
    let script = state.script.lock().expect("script mutex");
    if let Some(failure) = script.skill_failure.clone() {
        return Err((StatusCode::BAD_REQUEST, Json(json!({"error": failure}))));
    }
    if script.search_requires_approval && body["approved"].as_bool() != Some(true) {
        return Ok(Json(json!({
            "requires_approval": true,
            "action_id": "skill-search-action",
        })));
    }
    Ok(Json(Value::Array(script.search_candidates.clone())))
}

/// A skill download always requires approval first, as the real daemon does.
async fn skill_download(
    State(state): State<DaemonState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> ApiResult {
    guard!(state, headers);
    record(&state, "POST", "/v1/skills/download", Some(&body));
    let script = state.script.lock().expect("script mutex");
    if let Some(failure) = script.skill_failure.clone() {
        return Err((StatusCode::BAD_REQUEST, Json(json!({"error": failure}))));
    }
    if body["approved"].as_bool() == Some(true) {
        // The second call, carrying the exact action id the first call issued:
        // this is the download the client actually inspects and offers to
        // install.
        return Ok(Json(json!({
            "name": body["candidate_id"],
            "version": "1.0.0",
            "source_path": "/tmp/purrcode-skills/example",
            "content_digest": "sha256:downloaded-skill-digest",
            "publisher": "example-publisher",
        })));
    }
    Ok(Json(json!({
        "requires_approval": true,
        "action_id": "skill-download-action",
        "candidate_id": body["candidate_id"],
        "commit": "0123456789abcdef0123456789abcdef01234567",
    })))
}

async fn skill_install_propose(
    State(state): State<DaemonState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> ApiResult {
    guard!(state, headers);
    record(&state, "POST", "/v1/skills/install/propose", Some(&body));
    let script = state.script.lock().expect("script mutex");
    if let Some(failure) = script.skill_failure.clone() {
        return Err((StatusCode::BAD_REQUEST, Json(json!({"error": failure}))));
    }
    Ok(Json(json!({
        "action_id": "skill-install-action",
        "requires_approval": true,
    })))
}

async fn skill_install_approve(
    State(state): State<DaemonState>,
    headers: HeaderMap,
    Path(action): Path<String>,
    Json(body): Json<Value>,
) -> ApiResult {
    guard!(state, headers);
    record(
        &state,
        "POST",
        &format!("/v1/skills/install/{action}/approve"),
        Some(&body),
    );
    // Approval authorizes the exact digest; it does not run qualification, so
    // it is not where `skill_failure` (a content/qualification failure) applies
    // — that is `skill_install_finalize`'s concern, below.
    Ok(Json(json!({"approved": action})))
}

/// The install a client runs only after the exact action digest was approved.
/// A qualification failure blocks this step even after approval, exactly as a
/// failed structured-output/coding check would in the real daemon.
async fn skill_install_finalize(
    State(state): State<DaemonState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> ApiResult {
    guard!(state, headers);
    record(&state, "POST", "/v1/skills/install", Some(&body));
    let script = state.script.lock().expect("script mutex");
    if let Some(failure) = script.skill_failure.clone() {
        return Err((StatusCode::BAD_REQUEST, Json(json!({"error": failure}))));
    }
    Ok(Json(json!({
        "skill_id": "example-skill",
        "version": "1.0.0",
    })))
}

async fn block_publisher(
    State(state): State<DaemonState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> ApiResult {
    guard!(state, headers);
    record(&state, "POST", "/v1/skills/publishers/block", Some(&body));
    let mut script = state.script.lock().expect("script mutex");
    if let Some(publisher) = body["publisher"].as_str() {
        script.blocked_publishers.push(publisher.to_owned());
        script.skill_failure = Some(format!("publisher {publisher} is blocked by local policy"));
    }
    Ok(Json(json!({"blocked": body["publisher"]})))
}

/// A web fetch is proposed, never performed without approval.
async fn research_fetch(
    State(state): State<DaemonState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> ApiResult {
    guard!(state, headers);
    record(&state, "POST", "/v1/research/fetch", Some(&body));
    Ok(Json(json!({
        "requires_approval": true,
        "action_id": "research-action",
        "url": body["url"],
    })))
}

async fn compact(
    State(state): State<DaemonState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> ApiResult {
    guard!(state, headers);
    record(&state, "POST", &format!("/v1/sessions/{id}/compact"), None);
    let mut script = state.script.lock().expect("script mutex");
    if let Some(session) = script.sessions.get_mut(&id) {
        session.events.push(json!({
            "event": "context_compacted",
            "data": {"summary": "9 earlier turns were summarized"}
        }));
    }
    Ok(Json(json!({"compacted": true})))
}

async fn select_session_model(
    State(state): State<DaemonState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> ApiResult {
    guard!(state, headers);
    record(
        &state,
        "POST",
        &format!("/v1/sessions/{id}/model"),
        Some(&body),
    );
    Ok(Json(json!({"model": body["model"]})))
}

async fn qualify(
    State(state): State<DaemonState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> ApiResult {
    guard!(state, headers);
    record(&state, "POST", "/v1/local-models/qualify", Some(&body));
    let script = state.script.lock().expect("script mutex");
    Ok(Json(script.qualification.clone().unwrap_or_else(|| {
        json!({
            "qualification": {
                "model": {"provider": body["provider"], "model": body["model"]},
                "accuracy": 0.92,
                "mean_latency_ms": 480.0,
                "estimated_tokens_per_second": 24.5,
                "maximum_reliable_context_tokens": 8192,
                "cases": [
                    {"name": "structured output", "passed": true},
                    {"name": "tool calling", "passed": true},
                ],
            }
        })
    })))
}

async fn unload(
    State(state): State<DaemonState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> ApiResult {
    guard!(state, headers);
    record(&state, "POST", "/v1/local-models/unload", Some(&body));
    Ok(Json(json!({"unloaded": body})))
}

async fn pull_propose(
    State(state): State<DaemonState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> ApiResult {
    guard!(state, headers);
    record(&state, "POST", "/v1/local-models/pull/propose", Some(&body));
    let mut script = state.script.lock().expect("script mutex");
    script.pull_approved = false;
    Ok(Json(json!({
        "action_id": "pull-action",
        "action_digest": "digest-of-the-exact-pull",
        "session_id": "pull-session",
        "model": body["model"],
    })))
}

async fn pull_approve(
    State(state): State<DaemonState>,
    headers: HeaderMap,
    Path(action): Path<String>,
    Json(body): Json<Value>,
) -> ApiResult {
    guard!(state, headers);
    record(
        &state,
        "POST",
        &format!("/v1/local-models/pull/{action}/approve"),
        Some(&body),
    );
    let mut script = state.script.lock().expect("script mutex");
    // Refuse an approval for an action that was never proposed, as the real
    // daemon does; a client must not be able to authorize a phantom download.
    if action != "pull-action" {
        return Err((
            StatusCode::CONFLICT,
            Json(json!({"error": "no such pull action is awaiting approval"})),
        ));
    }
    script.pull_approved = true;
    // The approve response must echo the exact digest `pull_propose` issued:
    // the client checks it matches the action it displayed before treating
    // the pull as authorized, exactly like the write/command approval flow.
    Ok(Json(json!({
        "approved": action,
        "action_digest": "digest-of-the-exact-pull",
    })))
}

async fn pull_start(
    State(state): State<DaemonState>,
    headers: HeaderMap,
    Path(action): Path<String>,
    Json(body): Json<Value>,
) -> ApiResult {
    guard!(state, headers);
    record(
        &state,
        "POST",
        &format!("/v1/local-models/pull/{action}/start"),
        Some(&body),
    );
    let mut script = state.script.lock().expect("script mutex");
    if !script.pull_approved {
        return Err((
            StatusCode::CONFLICT,
            Json(json!({"error": "this pull action has not been approved"})),
        ));
    }
    script.pull_running = true;
    Ok(Json(
        json!({"phase": "running", "message": "downloading", "captured_output_bytes": 0}),
    ))
}

async fn pull_cancel(
    State(state): State<DaemonState>,
    headers: HeaderMap,
    Path(action): Path<String>,
    Json(body): Json<Value>,
) -> ApiResult {
    guard!(state, headers);
    record(
        &state,
        "POST",
        &format!("/v1/local-models/pull/{action}/cancel"),
        Some(&body),
    );
    let mut script = state.script.lock().expect("script mutex");
    script.pull_running = false;
    script.pull_cancelled = true;
    Ok(Json(json!({"cancelled": action})))
}

async fn pull_progress(
    State(state): State<DaemonState>,
    headers: HeaderMap,
    Path(action): Path<String>,
) -> ApiResult {
    guard!(state, headers);
    record(
        &state,
        "GET",
        &format!("/v1/local-models/pull/{action}"),
        None,
    );
    let script = state.script.lock().expect("script mutex");
    let (phase, message) = if script.pull_cancelled {
        ("cancelled", "the download was cancelled before completing")
    } else if script.pull_running {
        ("running", "downloading model layers")
    } else {
        ("pending", "waiting for approval")
    };
    Ok(Json(json!({
        "phase": phase,
        "message": message,
        "captured_output_bytes": 128,
    })))
}

async fn accepted(State(state): State<DaemonState>, headers: HeaderMap) -> impl IntoResponse {
    if authorize(&state, &headers).is_err() {
        return StatusCode::UNAUTHORIZED;
    }
    StatusCode::ACCEPTED
}

#[cfg(test)]
mod tests {
    use super::*;

    fn client() -> reqwest_lite::Client {
        reqwest_lite::Client
    }

    /// The harness has no async client dependency, so tests here use a tiny
    /// blocking helper over `std::net`. It speaks just enough HTTP/1.1 to assert
    /// the daemon's contract.
    mod reqwest_lite {
        use std::io::{Read, Write};

        pub struct Client;

        pub struct Response {
            pub status: u16,
            pub body: String,
        }

        impl Client {
            pub fn request(
                &self,
                url: &str,
                method: &str,
                token: Option<&str>,
                body: Option<&str>,
            ) -> Response {
                let without_scheme = url.trim_start_matches("http://");
                let (authority, path) = without_scheme
                    .split_once('/')
                    .map(|(authority, path)| (authority, format!("/{path}")))
                    .unwrap_or((without_scheme, "/".to_owned()));
                let mut stream =
                    std::net::TcpStream::connect(authority).expect("connect to fake daemon");
                let mut request = format!(
                    "{method} {path} HTTP/1.1\r\nHost: {authority}\r\nConnection: close\r\n"
                );
                if let Some(token) = token {
                    request.push_str(&format!("Authorization: Bearer {token}\r\n"));
                }
                match body {
                    Some(body) => {
                        request.push_str("Content-Type: application/json\r\n");
                        request.push_str(&format!("Content-Length: {}\r\n\r\n", body.len()));
                        request.push_str(body);
                    }
                    None => request.push_str("Content-Length: 0\r\n\r\n"),
                }
                stream.write_all(request.as_bytes()).expect("write request");
                let mut raw = String::new();
                let _ = stream.read_to_string(&mut raw);
                let status = raw
                    .split_whitespace()
                    .nth(1)
                    .and_then(|code| code.parse().ok())
                    .unwrap_or(0);
                let body = raw
                    .split_once("\r\n\r\n")
                    .map(|(_, body)| body.to_owned())
                    .unwrap_or_default();
                Response { status, body }
            }
        }
    }

    fn daemon(script: DaemonScript) -> FakeDaemon {
        FakeDaemon::start(script, "test-token").expect("start fake daemon")
    }

    #[test]
    fn an_unauthenticated_request_is_refused() {
        let daemon = daemon(DaemonScript::default());
        let response =
            client().request(&format!("{}/v1/providers", daemon.url()), "GET", None, None);
        assert_eq!(response.status, 401);
    }

    #[test]
    fn scripted_providers_are_reported() {
        let daemon = daemon(DaemonScript {
            providers: vec![json!({"name": "local", "provider_type": "ollama"})],
            ..DaemonScript::default()
        });
        let response = client().request(
            &format!("{}/v1/providers", daemon.url()),
            "GET",
            Some("test-token"),
            None,
        );
        assert_eq!(response.status, 200);
        assert!(response.body.contains("\"local\""));
        assert!(daemon.saw("GET", "/v1/providers"));
    }

    #[test]
    fn an_unreachable_daemon_fails_every_request() {
        let daemon = daemon(DaemonScript {
            unreachable: true,
            ..DaemonScript::default()
        });
        let response = client().request(
            &format!("{}/v1/providers", daemon.url()),
            "GET",
            Some("test-token"),
            None,
        );
        assert_eq!(response.status, 503);
    }

    #[test]
    fn approval_requires_an_awaiting_boundary() {
        let mut sessions = BTreeMap::new();
        sessions.insert("s1".to_owned(), ScriptedSession::new("active"));
        let daemon = daemon(DaemonScript {
            sessions,
            ..DaemonScript::default()
        });
        let response = client().request(
            &format!("{}/v1/sessions/s1/approve", daemon.url()),
            "POST",
            Some("test-token"),
            None,
        );
        assert_eq!(
            response.status, 409,
            "approving with no pending action must be refused: {}",
            response.body
        );
        assert!(response.body.contains("no action is awaiting approval"));
    }

    #[test]
    fn approving_a_pending_action_records_the_authorization() {
        let mut sessions = BTreeMap::new();
        sessions.insert(
            "s1".to_owned(),
            ScriptedSession::new("active").awaiting("a1"),
        );
        let daemon = daemon(DaemonScript {
            sessions,
            ..DaemonScript::default()
        });
        let response = client().request(
            &format!("{}/v1/sessions/s1/approve", daemon.url()),
            "POST",
            Some("test-token"),
            None,
        );
        assert_eq!(response.status, 200);
        let events = client().request(
            &format!("{}/v1/sessions/s1/events", daemon.url()),
            "GET",
            Some("test-token"),
            None,
        );
        assert!(events.body.contains("approval_recorded"));
        assert!(events.body.contains("execution_started"));
    }

    #[test]
    fn rejecting_a_pending_action_records_no_execution() {
        let mut sessions = BTreeMap::new();
        sessions.insert(
            "s1".to_owned(),
            ScriptedSession::new("active").awaiting("a1"),
        );
        let daemon = daemon(DaemonScript {
            sessions,
            ..DaemonScript::default()
        });
        client().request(
            &format!("{}/v1/sessions/s1/deny", daemon.url()),
            "POST",
            Some("test-token"),
            Some("{\"reason\":\"no\"}"),
        );
        let events = client().request(
            &format!("{}/v1/sessions/s1/events", daemon.url()),
            "GET",
            Some("test-token"),
            None,
        );
        assert!(events.body.contains("approval_rejected"));
        assert!(
            !events.body.contains("execution_started"),
            "a rejection must never produce an execution: {}",
            events.body
        );
    }

    #[test]
    fn a_second_decision_for_the_same_action_is_refused() {
        let mut sessions = BTreeMap::new();
        sessions.insert(
            "s1".to_owned(),
            ScriptedSession::new("active").awaiting("a1"),
        );
        let daemon = daemon(DaemonScript {
            sessions,
            ..DaemonScript::default()
        });
        let url = format!("{}/v1/sessions/s1/approve", daemon.url());
        assert_eq!(
            client()
                .request(&url, "POST", Some("test-token"), None)
                .status,
            200
        );
        assert_eq!(
            client()
                .request(&url, "POST", Some("test-token"), None)
                .status,
            409,
            "the boundary is consumed by the first decision"
        );
    }

    #[test]
    fn an_active_lease_makes_new_work_conflict() {
        let mut sessions = BTreeMap::new();
        let mut held = ScriptedSession::new("executing");
        held.lease_active = true;
        sessions.insert("s1".to_owned(), held);
        let daemon = daemon(DaemonScript {
            sessions,
            ..DaemonScript::default()
        });
        let response = client().request(
            &format!("{}/v1/sessions/s1/messages", daemon.url()),
            "POST",
            Some("test-token"),
            Some("{\"content\":\"more work\"}"),
        );
        assert_eq!(response.status, 409);
    }

    #[test]
    fn a_missing_worktree_makes_the_diff_a_conflict_not_an_empty_result() {
        let mut sessions = BTreeMap::new();
        sessions.insert("s1".to_owned(), ScriptedSession::new("completed"));
        let daemon = daemon(DaemonScript {
            sessions,
            diff: None,
            ..DaemonScript::default()
        });
        let response = client().request(
            &format!("{}/v1/sessions/s1/diff", daemon.url()),
            "GET",
            Some("test-token"),
            None,
        );
        assert_eq!(
            response.status, 409,
            "an unavailable diff must not look like an empty diff"
        );
    }

    #[test]
    fn nested_secret_fields_are_redacted_in_the_log() {
        let daemon = daemon(DaemonScript::default());
        client().request(
            &format!("{}/v1/providers", daemon.url()),
            "POST",
            Some("test-token"),
            Some("{\"name\":\"p\",\"auth\":{\"api_key\":\"sk-live-nested\"}}"),
        );
        let log = daemon.request_log();
        assert!(log.contains("[redacted]"), "{log}");
        assert!(!log.contains("sk-live-nested"), "{log}");
    }

    #[test]
    fn a_raw_secret_in_a_provider_save_is_refused() {
        let daemon = daemon(DaemonScript::default());
        let response = client().request(
            &format!("{}/v1/providers", daemon.url()),
            "POST",
            Some("test-token"),
            Some("{\"name\":\"remote\",\"api_key\":\"sk-live-abcdef\"}"),
        );
        assert_eq!(response.status, 400);
        assert!(
            !daemon.request_log().contains("sk-live-abcdef"),
            "a secret must never reach the request log: {}",
            daemon.request_log()
        );
    }

    #[test]
    fn a_stored_credential_logs_only_its_name() {
        let daemon = daemon(DaemonScript::default());
        client().request(
            &format!("{}/v1/credentials", daemon.url()),
            "POST",
            Some("test-token"),
            Some("{\"name\":\"remote\",\"secret\":\"sk-live-zzzzzz\"}"),
        );
        let log = daemon.request_log();
        assert!(log.contains("remote"));
        assert!(
            !log.contains("sk-live-zzzzzz"),
            "the artifact log leaked a secret: {log}"
        );
    }

    #[test]
    fn scripted_state_can_be_advanced_mid_test() {
        let daemon = daemon(DaemonScript::default());
        daemon.update(|script| {
            script.sessions.insert(
                "later".to_owned(),
                ScriptedSession::new("completed").with_events(vec![json!({
                    "event": "session_completed", "data": {}
                })]),
            );
        });
        let response = client().request(
            &format!("{}/v1/sessions/later/events", daemon.url()),
            "GET",
            Some("test-token"),
            None,
        );
        assert_eq!(response.status, 200);
        assert!(response.body.contains("session_completed"));
    }

    #[test]
    fn each_daemon_binds_its_own_port() {
        let first = daemon(DaemonScript::default());
        let second = daemon(DaemonScript::default());
        assert_ne!(first.port(), second.port());
    }
}
