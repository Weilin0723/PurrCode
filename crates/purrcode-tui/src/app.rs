//! Application state, event loop, and mode management.

use crate::command_palette::CommandPalette;
use crate::composer::Composer;
use crate::conversation::{Conversation, Message};
use crate::diff_view::DiffView;
use crate::keybindings::{handle_key, handle_mouse};
use crate::provider_setup::ProviderSetup;
use crate::render::draw;
use crate::skill_browser::SkillBrowser;
use crate::status_bar::StatusBar;
use crate::streaming::{
    SseDecoder, StreamController, StreamEvent, StreamOutput, VerifiedStreamEnd,
};
use crate::theme::Theme;
use crate::ui_state::UiState;
use crate::workspace::WorkspaceContext;
use anyhow::Result;
use crossterm::event::{
    self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
    Event, KeyEventKind,
};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use futures::StreamExt;
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use serde_json::Value;
use std::io;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use zeroize::Zeroize;

#[derive(Clone, Debug)]
pub struct TuiConfig {
    pub daemon_url: String,
    pub token_file: PathBuf,
    pub repository: PathBuf,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AppMode {
    Conversation,
    SecretReview,
    ProviderSetup,
    SkillBrowse,
    DiffView,
    Help,
    LeaseConflict,
    /// An existing durable session was found at startup. Require an explicit
    /// resume-or-new choice before accepting conversation input.
    SessionChoice,
}

#[derive(Clone, Debug)]
pub struct SecretReview {
    pub redacted_source: String,
    pub finding_count: usize,
    pub provider_candidate: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PendingModelPull {
    pub action_id: String,
    pub action_digest: String,
    pub session_id: String,
    pub model: String,
    pub approved: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PendingResearch {
    pub action_id: String,
    pub url: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PendingSkillDownload {
    pub action_id: String,
    pub candidate_id: String,
    pub commit: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionChoice {
    pub session_id: String,
    pub status: String,
    pub resumable: bool,
    pub lease_active: bool,
}

#[derive(serde::Serialize)]
struct StoreCredentialPayload<'a> {
    name: &'a str,
    secret: &'a str,
}

pub struct App {
    pub config: TuiConfig,
    pub client: reqwest::Client,
    pub token: String,
    pub mode: AppMode,
    pub conversation: Conversation,
    pub composer: Composer,
    pub secret_review: Option<SecretReview>,
    pub status_bar: StatusBar,
    pub workspace: WorkspaceContext,
    pub provider_setup: Option<ProviderSetup>,
    pub skill_browser: Option<SkillBrowser>,
    pub diff_view: Option<DiffView>,
    pub stream: StreamController,
    pub(crate) reconciliation: Option<tokio::task::JoinHandle<ReconciliationSnapshot>>,
    pub last_refresh: Instant,
    pub message_bar: String,
    pub session_id: Option<String>,
    /// Startup gate for a repository-scoped session recovered from durable UI state.
    /// The composer must not accept a task until the user explicitly attaches to
    /// this session or starts a new one.
    pub session_choice: Option<SessionChoice>,
    pub session_read_only: bool,
    pub has_provider: bool,
    pub pending_command: Option<String>,
    pub pending_user_message: bool,
    pub running_command: bool,
    pub quit_requested: bool,
    pub downloaded_skill: Option<Value>,
    pub pending_skill_install_action: Option<String>,
    pub pending_research: Option<PendingResearch>,
    pub pending_skill_download: Option<PendingSkillDownload>,
    pub pending_model_pull: Option<PendingModelPull>,
    pub active_pull_action: Option<String>,
    pub active_pull_session: Option<String>,
    pub theme: Theme,
    pub palette_query: String,
    pub palette_selected: usize,
}

pub(crate) struct ReconciliationSnapshot {
    messages: Option<Vec<Message>>,
    events: Option<Vec<Value>>,
    pull_progress: Option<Result<Value, String>>,
}

pub async fn run(config: TuiConfig) -> Result<()> {
    let token = std::fs::read_to_string(&config.token_file)?;
    let token = token.trim().to_string();

    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(3))
        .timeout(Duration::from_secs(60))
        .build()?;

    let workspace = WorkspaceContext::inspect(&config.repository);
    let recovery = UiState::load(&config.repository);
    let mut app = App {
        config,
        client,
        token,
        mode: AppMode::Conversation,
        conversation: Conversation::new(),
        composer: Composer::new(),
        secret_review: None,
        status_bar: StatusBar::new(),
        workspace,
        provider_setup: None,
        skill_browser: None,
        diff_view: None,
        stream: StreamController::new(),
        reconciliation: None,
        last_refresh: Instant::now(),
        message_bar: String::new(),
        session_id: None,
        session_choice: None,
        session_read_only: false,
        has_provider: false,
        pending_command: None,
        pending_user_message: false,
        running_command: false,
        quit_requested: false,
        downloaded_skill: None,
        pending_skill_install_action: None,
        pending_research: None,
        pending_skill_download: None,
        pending_model_pull: None,
        active_pull_action: None,
        active_pull_session: None,
        theme: Theme::detect(),
        palette_query: String::new(),
        palette_selected: 0,
    };
    app.session_id = recovery.restore(&mut app.composer);

    app.check_provider().await;
    app.check_workspace().await;
    app.inspect_recovered_session().await;

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(
        stdout,
        EnterAlternateScreen,
        EnableBracketedPaste,
        EnableMouseCapture
    )?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    let result = event_loop(&mut terminal, &mut app).await;
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        DisableMouseCapture,
        DisableBracketedPaste,
        LeaveAlternateScreen
    )?;
    terminal.show_cursor()?;
    result
}

async fn event_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
) -> Result<()> {
    loop {
        if app
            .provider_setup
            .as_ref()
            .is_some_and(|setup| setup.discovery_requested)
        {
            app.discover_local_models().await;
        }
        if app
            .provider_setup
            .as_ref()
            .is_some_and(|setup| setup.complete)
        {
            app.finish_provider_setup().await;
        }
        // Reconciliation stays off the input/render tick. While SSE is active, its durable-audit
        // increments replace the two legacy polling GETs.
        app.poll_reconciliation().await;
        if app.last_refresh.elapsed() >= Duration::from_secs(1) {
            app.start_reconciliation();
            app.last_refresh = Instant::now();
        }

        app.drain_live_stream(Instant::now());
        if let Some(stall) = app.stream.actionable_stall(Instant::now()) {
            app.message_bar = stall.into();
        }

        // Process pending command
        if let Some(cmd) = app.pending_command.take() {
            app.running_command = true;
            CommandPalette::new().execute(app, &cmd).await;
            app.running_command = false;
            if app.quit_requested {
                return Ok(());
            }
        }

        // Process pending user message
        if app.pending_user_message {
            app.pending_user_message = false;
            app.conversation.finalize_streaming();

            let objective = app.conversation.current_objective();
            if objective.is_empty() {
                app.message_bar = "No objective set. Type a task first.".into();
            } else {
                let mut stream_after = 0_u64;
                let existing_session = app.session_id.clone();
                let Some(session_id) = app.ensure_session().await else {
                    continue;
                };
                if existing_session.is_some() {
                    stream_after = app
                        .request(
                            reqwest::Method::GET,
                            &format!("/v1/sessions/{session_id}"),
                            None,
                        )
                        .await
                        .ok()
                        .and_then(|value| value["event_count"].as_u64())
                        .unwrap_or(0);
                    let content = app
                        .conversation
                        .messages
                        .last()
                        .map(|message| message.content.clone())
                        .unwrap_or_default();
                    if let Err(error) = app
                        .request(
                            reqwest::Method::POST,
                            &format!("/v1/sessions/{session_id}/messages"),
                            Some(serde_json::json!({"content": content})),
                        )
                        .await
                    {
                        app.stream.active = false;
                        if error.to_string().contains("409 Conflict") {
                            app.switch_mode(AppMode::LeaseConflict);
                            app.refresh().await;
                            app.message_bar = "Session lease conflict; no new action was started and your draft is safe.".into();
                        } else {
                            app.message_bar = format!("Message error: {error}");
                        }
                        continue;
                    }
                }
                app.cancel_reconciliation();
                let selected_model = app.status_bar.model.clone();
                app.conversation
                    .start_streaming(Some(selected_model.clone()));

                app.start_session_stream(stream_after, Some(selected_model));
                app.message_bar = "Preparing context...".into();
            }
        }

        // Render
        terminal.draw(|frame| draw(frame, app))?;

        // Wait for input
        if !event::poll(Duration::from_millis(16))? {
            continue;
        }

        let input = event::read()?;
        if let Event::Paste(content) = input {
            if app.mode == AppMode::Conversation {
                app.composer.insert_paste(&content);
            } else if app.mode == AppMode::ProviderSetup {
                if let Some(setup) = &mut app.provider_setup {
                    match setup.screen {
                        crate::provider_setup::SetupScreen::ImportSource => {
                            setup.insert_import(&content);
                        }
                        crate::provider_setup::SetupScreen::ImportEnvironment => {
                            setup.insert_environment_reference(&content);
                        }
                        crate::provider_setup::SetupScreen::Form
                        | crate::provider_setup::SetupScreen::ImportReview => {
                            setup.insert_active_paste(&content);
                        }
                        crate::provider_setup::SetupScreen::Discovery
                        | crate::provider_setup::SetupScreen::ImportAuthChoice
                        | crate::provider_setup::SetupScreen::ImportKeychainConfirm => {}
                    }
                }
            }
            app.persist_ui_state();
            continue;
        }
        if let Event::Mouse(mouse) = input {
            let size = terminal.size()?;
            handle_mouse(
                app,
                mouse,
                ratatui::layout::Rect::new(0, 0, size.width, size.height),
            );
            app.persist_ui_state();
            continue;
        }
        let Event::Key(key) = input else { continue };
        if key.kind != KeyEventKind::Press {
            continue;
        }

        if !handle_key(app, key) {
            app.persist_ui_state();
            return Ok(());
        }
        app.persist_ui_state();
    }
}

async fn daemon_get_json(
    client: &reqwest::Client,
    token: &str,
    url: &str,
) -> Result<Value, String> {
    let response = client
        .get(url)
        .bearer_auth(token)
        .send()
        .await
        .map_err(|error| error.to_string())?;
    let status = response.status();
    if !status.is_success() {
        return Err(format!("daemon HTTP {status}"));
    }
    response
        .json::<Value>()
        .await
        .map_err(|error| error.to_string())
}

impl App {
    /// Resolve repository-scoped recovery state before the composer becomes
    /// interactive. This is deliberately GET-only: merely opening PurrCode
    /// never resumes a daemon operation or creates a replacement session.
    async fn inspect_recovered_session(&mut self) {
        let Some(session_id) = self.session_id.clone() else {
            return;
        };
        let session = match self
            .request(
                reqwest::Method::GET,
                &format!("/v1/sessions/{session_id}"),
                None,
            )
            .await
        {
            Ok(session) => session,
            Err(error) => {
                self.session_id = None;
                self.message_bar =
                    format!("Previous session is unavailable; starting a new session: {error}");
                return;
            }
        };
        let status = session["status_code"]
            .as_str()
            .unwrap_or("unknown")
            .to_owned();
        let lease_active = session["lease_active"].as_bool().unwrap_or(false);
        let resumable = recovered_session_is_resumable(&status, lease_active);
        self.session_choice = Some(SessionChoice {
            session_id,
            status,
            resumable,
            lease_active,
        });
        self.mode = AppMode::SessionChoice;
    }

    pub(crate) fn start_session_stream(&mut self, after: u64, model: Option<String>) {
        let Some(session_id) = self.session_id.clone() else {
            self.message_bar = "No active session.".into();
            return;
        };
        self.cancel_reconciliation();
        let tx = self.stream.start(model, Instant::now());
        let daemon_url = self.daemon_url().to_string();
        let token = self.token.clone();
        tokio::spawn(async move {
            let url = format!(
                "{}/v1/sessions/{session_id}/events/stream?after={after}",
                daemon_url.trim_end_matches('/')
            );
            let client = match reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(3))
                .build()
            {
                Ok(client) => client,
                Err(error) => {
                    let _ = tx
                        .send(StreamEvent::TransportError(error.to_string()))
                        .await;
                    return;
                }
            };
            match client.get(&url).bearer_auth(&token).send().await {
                Ok(response) if response.status().is_success() => {
                    let mut stream = response.bytes_stream();
                    let mut decoder = SseDecoder::default();
                    while let Some(chunk) = stream.next().await {
                        match chunk {
                            Ok(bytes) => match decoder.push(&bytes) {
                                Ok(events) => {
                                    for event in events {
                                        if tx.send(event).await.is_err() {
                                            return;
                                        }
                                    }
                                }
                                Err(error) => {
                                    let _ = tx.send(StreamEvent::TransportError(error)).await;
                                    return;
                                }
                            },
                            Err(error) => {
                                let _ = tx
                                    .send(StreamEvent::TransportError(error.to_string()))
                                    .await;
                                return;
                            }
                        }
                    }
                    let _ = tx.send(StreamEvent::TransportClosed).await;
                }
                Ok(response) => {
                    let _ = tx
                        .send(StreamEvent::TransportError(format!(
                            "daemon stream returned HTTP {}",
                            response.status()
                        )))
                        .await;
                }
                Err(error) => {
                    let _ = tx
                        .send(StreamEvent::TransportError(error.to_string()))
                        .await;
                }
            }
        });
    }

    pub fn persist_ui_state(&self) {
        UiState::save(
            &self.config.repository,
            self.session_id.as_deref(),
            &self.composer,
        );
    }
    pub fn daemon_url(&self) -> &str {
        self.config.daemon_url.trim_end_matches('/')
    }

    pub async fn request(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<Value> {
        self.request_with_timeout(method, path, body, Duration::from_secs(60))
            .await
    }

    pub async fn request_with_timeout(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
        timeout: Duration,
    ) -> Result<Value> {
        let url = format!("{}{}", self.daemon_url(), path);
        let mut req = self
            .client
            .request(method, &url)
            .bearer_auth(&self.token)
            .timeout(timeout);
        if let Some(body) = body {
            req = req.json(&body);
        }
        let resp = req.send().await?;
        let status = resp.status();
        let value: Value = resp.json().await?;
        if !status.is_success() {
            anyhow::bail!("daemon HTTP {status}: {value}");
        }
        Ok(value)
    }

    async fn store_credential(&self, name: &str, secret: &str) -> Result<Value> {
        let url = format!("{}/v1/credentials", self.daemon_url());
        let response = self
            .client
            .post(url)
            .bearer_auth(&self.token)
            .json(&StoreCredentialPayload { name, secret })
            .send()
            .await?;
        let status = response.status();
        let value: Value = response.json().await?;
        if !status.is_success() {
            let detail = value
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("credential storage was rejected");
            anyhow::bail!("daemon HTTP {status}: {detail}");
        }
        Ok(value)
    }

    pub async fn check_provider(&mut self) {
        match self
            .request(reqwest::Method::GET, "/v1/providers", None)
            .await
        {
            Ok(val) => {
                self.workspace.daemon_health = "connected".into();
                let arr = val.as_array();
                self.has_provider = arr.is_some_and(|a| !a.is_empty());
                if !self.has_provider {
                    self.message_bar =
                        "No provider configured. Type /connect to set one up.".into();
                    self.workspace.model = "none".into();
                } else if let Ok(models) =
                    self.request(reqwest::Method::GET, "/v1/models", None).await
                {
                    if let Some(selected) = models
                        .as_array()
                        .and_then(|models| {
                            models
                                .iter()
                                .find(|model| model["default"].as_bool() == Some(true))
                        })
                        .and_then(|model| model["id"].as_str())
                    {
                        self.status_bar.set_model(selected);
                        self.workspace.model = selected.to_owned();
                        self.status_bar.local = models
                            .as_array()
                            .and_then(|models| models.iter().find(|model| model["id"] == selected))
                            .and_then(|model| model["local"].as_bool())
                            .unwrap_or(false);
                    }
                }
            }
            Err(_) => {
                self.workspace.daemon_health = "unreachable".into();
                self.has_provider = false;
                self.workspace.model = "unavailable".into();
                self.message_bar = "Daemon unreachable. Type /connect to set up.".into();
            }
        }
    }

    pub async fn check_workspace(&mut self) {
        match self
            .request(
                reqwest::Method::POST,
                "/v1/repository/inspect",
                Some(serde_json::json!({"repository": self.config.repository})),
            )
            .await
        {
            Ok(value) => {
                self.workspace.source_state = if value["dirty"].as_bool() == Some(true) {
                    "dirty · preserved"
                } else {
                    "clean"
                }
                .into();
            }
            Err(error) => {
                self.workspace.source_state = "unknown · preserved".into();
                if self.message_bar.is_empty() {
                    self.message_bar = format!("Repository inspection unavailable: {error}");
                }
            }
        }
    }

    fn drain_live_stream(&mut self, now: Instant) {
        for output in self.stream.drain(now) {
            match output {
                StreamOutput::PhaseChanged(update) => {
                    self.workspace.session_phase = update.phase.label().to_lowercase();
                    let role = update
                        .role
                        .as_deref()
                        .map(|role| format!(" · {role}"))
                        .unwrap_or_default();
                    let attempt = update
                        .attempt
                        .map(|attempt| format!(" · attempt {attempt}"))
                        .unwrap_or_default();
                    self.message_bar = format!("{}{}{}", update.phase.label(), role, attempt);
                    if let Some(first_token_ms) = update.timing.first_semantic_delta_ms {
                        self.status_bar.context_info = format!("first token {first_token_ms} ms");
                    }
                }
                StreamOutput::Content { text, replace, .. } => {
                    if replace {
                        self.conversation.replace_streaming(&text);
                    } else {
                        self.conversation.append_streaming(&text);
                    }
                }
                StreamOutput::DurableAudit { sequence, event } => {
                    self.conversation.apply_durable_audit(sequence, event);
                }
                StreamOutput::Diagnostic(message) => {
                    self.message_bar = message;
                }
                StreamOutput::TransportError(error) => {
                    self.conversation.cancel_streaming();
                    self.stream.stop();
                    self.message_bar =
                        format!("Live stream interrupted; partial output was preserved: {error}");
                }
                StreamOutput::VerifiedEnd(end) => {
                    self.conversation.finalize_streaming();
                    self.message_bar = match end {
                        VerifiedStreamEnd::Completed => "Done.".into(),
                        VerifiedStreamEnd::Failed => {
                            "Session failed; partial output was preserved.".into()
                        }
                        VerifiedStreamEnd::Cancelled => {
                            "Cancelled; partial output was preserved.".into()
                        }
                        VerifiedStreamEnd::AwaitingApproval => {
                            "Paused for exact-action approval.".into()
                        }
                        VerifiedStreamEnd::AwaitingReview => self
                            .conversation
                            .timeline
                            .iter()
                            .rev()
                            .find(|card| card.title == "Outcome review required")
                            .map(|card| {
                                format!(
                                    "Paused: {} · click the outcome or press E to review details.",
                                    card.summary
                                )
                            })
                            .unwrap_or_else(|| {
                                "Paused for outcome review; click the latest validation card or press E to inspect evidence.".into()
                            }),
                    };
                }
            }
        }
    }

    fn cancel_reconciliation(&mut self) {
        if let Some(handle) = self.reconciliation.take() {
            handle.abort();
        }
    }

    fn start_reconciliation(&mut self) {
        if self.reconciliation.is_some() || self.stream.active {
            return;
        }
        if self.mode != AppMode::Conversation && self.active_pull_action.is_none() {
            return;
        }
        let client = self.client.clone();
        let daemon_url = self.daemon_url().trim_end_matches('/').to_owned();
        let token = self.token.clone();
        let session_id = self.session_id.clone();
        let pull_action = self.active_pull_action.clone();
        self.reconciliation = Some(tokio::spawn(async move {
            let session = async {
                let Some(session_id) = session_id else {
                    return (None, None);
                };
                let messages_url = format!("{daemon_url}/v1/sessions/{session_id}/messages");
                let events_url = format!("{daemon_url}/v1/sessions/{session_id}/events");
                let (messages, events) = tokio::join!(
                    daemon_get_json(&client, &token, &messages_url),
                    daemon_get_json(&client, &token, &events_url),
                );
                (
                    messages
                        .ok()
                        .and_then(|value| serde_json::from_value::<Vec<Message>>(value).ok()),
                    events
                        .ok()
                        .and_then(|value| serde_json::from_value::<Vec<Value>>(value).ok()),
                )
            };
            let pull = async {
                let action_id = pull_action?;
                let url = format!("{daemon_url}/v1/local-models/pull/{action_id}");
                Some(daemon_get_json(&client, &token, &url).await)
            };
            let ((messages, events), pull_progress) = tokio::join!(session, pull);
            ReconciliationSnapshot {
                messages,
                events,
                pull_progress,
            }
        }));
    }

    async fn poll_reconciliation(&mut self) {
        if !self
            .reconciliation
            .as_ref()
            .is_some_and(tokio::task::JoinHandle::is_finished)
        {
            return;
        }
        let Some(handle) = self.reconciliation.take() else {
            return;
        };
        let Ok(snapshot) = handle.await else {
            return;
        };
        if !self.stream.active {
            if let Some(events) = snapshot.events {
                self.conversation.reconcile(snapshot.messages, events);
                self.workspace.session_phase = self.conversation.phase.clone();
            }
        }
        if let Some(progress) = snapshot.pull_progress {
            match progress {
                Ok(progress) => self.apply_pull_progress(&progress),
                Err(error) => {
                    self.message_bar = format!("Model pull status failed: {error}");
                    self.active_pull_action = None;
                    self.active_pull_session = None;
                }
            }
        }
    }

    fn apply_pull_progress(&mut self, progress: &Value) {
        let phase = progress["phase"].as_str().unwrap_or("unknown");
        let message = progress["message"]
            .as_str()
            .unwrap_or("Ollama pull state unavailable");
        let captured = progress["captured_output_bytes"]
            .as_u64()
            .unwrap_or_default();
        self.message_bar = format!("Model pull · {phase} · {captured} bytes\n{message}");
        if matches!(phase, "completed" | "failed" | "cancelled") {
            self.active_pull_action = None;
            self.active_pull_session = None;
        }
    }

    pub async fn refresh(&mut self) {
        if self.mode == AppMode::Conversation && !self.stream.active {
            let token = self.token.clone();
            let url = self.daemon_url().to_string();
            let session_id = self.session_id.clone();
            self.conversation
                .refresh_events(&url, &token, session_id)
                .await;
            self.workspace.session_phase = self.conversation.phase.clone();
        }
        if let Some(action_id) = self.active_pull_action.clone() {
            match self
                .request(
                    reqwest::Method::GET,
                    &format!("/v1/local-models/pull/{action_id}"),
                    None,
                )
                .await
            {
                Ok(progress) => self.apply_pull_progress(&progress),
                Err(error) => {
                    self.message_bar = format!("Model pull status failed: {error}");
                    self.active_pull_action = None;
                    self.active_pull_session = None;
                }
            }
        }
    }

    pub async fn ensure_session(&mut self) -> Option<String> {
        let objective = self.conversation.latest_user_message();
        if let Some(session_id) = self.session_id.clone() {
            let reusable = self
                .request(
                    reqwest::Method::GET,
                    &format!("/v1/sessions/{session_id}"),
                    None,
                )
                .await
                .ok()
                .and_then(|session| session["status_code"].as_str().map(str::to_owned))
                .is_some_and(|status| status == "active");
            if reusable {
                return Some(session_id);
            }
            self.session_id = None;
            self.conversation = Conversation::new();
            self.conversation.add_user_message(&objective);
            self.workspace.session_phase = "new".into();
        }
        let body = serde_json::json!({
            "objective": objective,
            "repository": self.config.repository,
            "plan_only": explicit_plan_only_intent(&objective),
        });
        match self
            .request(reqwest::Method::POST, "/v1/sessions", Some(body))
            .await
        {
            Ok(val) => {
                let id = val["id"].as_str().map(String::from);
                self.session_id = id.clone();
                id
            }
            Err(e) => {
                self.message_bar = format!("Session error: {e}");
                None
            }
        }
    }

    pub fn switch_mode(&mut self, mode: AppMode) {
        self.mode = mode;
    }

    async fn finish_provider_setup(&mut self) {
        let Some(mut setup) = self.provider_setup.take() else {
            return;
        };
        let Some(provider_type) = setup.provider_type else {
            self.provider_setup = Some(setup);
            return;
        };
        let name = setup.profile_name.trim().to_owned();
        let kind = match provider_type {
            crate::provider_setup::ProviderType::Ollama => "ollama",
            crate::provider_setup::ProviderType::LmStudio => "lm-studio",
            crate::provider_setup::ProviderType::Openai => "openai",
            crate::provider_setup::ProviderType::OpenaiCompatible => "openai-compatible",
            crate::provider_setup::ProviderType::EnterpriseGateway => {
                setup.error =
                    Some("Enterprise gateway setup requires configuration-file policy".into());
                setup.complete = false;
                self.provider_setup = Some(setup);
                return;
            }
        };
        if !setup.editing_existing {
            match self
                .request(reqwest::Method::GET, "/v1/providers", None)
                .await
            {
                Ok(providers)
                    if providers.as_array().is_some_and(|providers| {
                        providers.iter().any(|provider| {
                            provider.get("name").and_then(Value::as_str) == Some(name.as_str())
                        })
                    }) =>
                {
                    setup.editing_existing = true;
                    setup.complete = false;
                    setup.error = Some(format!(
                        "Provider profile `{name}` already exists. Press Ctrl+G again to explicitly replace it and run the real connection test."
                    ));
                    self.provider_setup = Some(setup);
                    return;
                }
                Ok(_) => {}
                Err(error) => {
                    setup.complete = false;
                    setup.error = Some(format!(
                        "Could not check existing provider profiles before saving: {error}"
                    ));
                    self.provider_setup = Some(setup);
                    return;
                }
            }
        }
        let mut credential_name = None;
        let mut credential_reference = setup.credential_reference();
        if let Some(secret) = setup.pending_keychain_secret() {
            let result = self.store_credential(&name, secret).await;
            if let Err(error) = result {
                setup.error = Some(format!("Credential storage failed: {error}"));
                setup.complete = false;
                self.provider_setup = Some(setup);
                return;
            }
            if let Err(error) = setup.confirm_keychain_stored() {
                setup.error = Some(format!("Credential confirmation failed: {error}"));
                setup.complete = false;
                self.provider_setup = Some(setup);
                return;
            }
            credential_reference = setup.credential_reference();
        } else if !setup.api_key.is_empty() {
            let result = self.store_credential(&name, &setup.api_key).await;
            setup.api_key.zeroize();
            if let Err(error) = result {
                setup.error = Some(format!("Credential storage failed: {error}"));
                setup.complete = false;
                self.provider_setup = Some(setup);
                return;
            }
            credential_name = Some(name.clone());
            credential_reference = None;
        }
        let configured = self
            .request(
                reqwest::Method::POST,
                "/v1/providers",
                Some(serde_json::json!({
                    "name": name,
                    "provider_type": kind,
                    "base_url": setup.base_url.clone(),
                    "model": setup.model_id.clone(),
                    "credential_name": credential_name,
                    "credential_reference": credential_reference,
                    "replace": setup.editing_existing,
                })),
            )
            .await;
        let result = match configured {
            Ok(result) => result,
            Err(error) => {
                setup.error = Some(format!(
                    "Provider configuration or connection test failed: {error}"
                ));
                setup.complete = false;
                self.provider_setup = Some(setup);
                return;
            }
        };
        let assigned_model = format!("{}/{}", name, setup.model_id);
        if !setup.role.trim().is_empty() {
            if let Err(error) = self
                .request(
                    reqwest::Method::POST,
                    "/v1/models/roles",
                    Some(serde_json::json!({
                        "role": setup.role,
                        "model": assigned_model,
                    })),
                )
                .await
            {
                setup.error = Some(format!("Role assignment failed: {error}"));
                setup.complete = false;
                self.provider_setup = Some(setup);
                return;
            }
        }
        self.has_provider = true;
        self.mode = AppMode::Conversation;
        self.status_bar.set_model(&assigned_model);
        self.message_bar = format!(
            "Provider {name} verified in {} ms: {}",
            result["latency_ms"].as_u64().unwrap_or_default(),
            result["detail"].as_str().unwrap_or("healthy response")
        );
    }

    async fn discover_local_models(&mut self) {
        let Some(mut setup) = self.provider_setup.take() else {
            return;
        };
        setup.discovery_requested = false;
        let provider_type = match setup.provider_type {
            Some(crate::provider_setup::ProviderType::Ollama) => "ollama",
            Some(crate::provider_setup::ProviderType::LmStudio) => "lm-studio",
            _ => {
                self.provider_setup = Some(setup);
                return;
            }
        };
        match self
            .request(
                reqwest::Method::POST,
                "/v1/providers/discover",
                Some(serde_json::json!({"provider_type": provider_type})),
            )
            .await
        {
            Ok(value) => {
                setup.discovered_models = value["models"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|model| model.as_str().map(str::to_owned))
                    .collect();
                if setup.model_id.is_empty() {
                    setup.model_id = setup.discovered_models.first().cloned().unwrap_or_default();
                }
                if setup.discovered_models.is_empty() {
                    setup.error = Some("Provider is reachable but reported no models".into());
                }
            }
            Err(error) => setup.error = Some(error.to_string()),
        }
        self.provider_setup = Some(setup);
    }
}

fn recovered_session_is_resumable(status: &str, lease_active: bool) -> bool {
    match status {
        "active" | "paused" => true,
        // These restore the durable decision boundary; they do not rerun the
        // model when the user chooses Resume.
        "awaiting_approval" | "awaiting_review" => true,
        // An executing session without its daemon lease cannot safely be
        // replayed. With a lease it may only be attached.
        "executing" => lease_active,
        "cancelled" | "completed" | "failed" => false,
        _ => false,
    }
}

fn explicit_plan_only_intent(objective: &str) -> bool {
    let objective = objective.to_ascii_lowercase();
    objective.contains("plan")
        && [
            "do not revise",
            "don't revise",
            "do not modify",
            "don't modify",
            "do not change",
            "don't change",
            "plan only",
            "only a plan",
            "give me the actual plan",
            "send me the plan",
        ]
        .iter()
        .any(|marker| objective.contains(marker))
}

#[cfg(test)]
mod recovery_tests {
    use super::recovered_session_is_resumable;

    #[test]
    fn terminal_sessions_require_a_new_session() {
        for status in ["completed", "failed", "cancelled"] {
            assert!(!recovered_session_is_resumable(status, false));
            assert!(!recovered_session_is_resumable(status, true));
        }
    }

    #[test]
    fn executing_without_a_lease_is_not_replayed() {
        assert!(!recovered_session_is_resumable("executing", false));
        assert!(recovered_session_is_resumable("executing", true));
    }

    #[test]
    fn paused_and_decision_boundaries_can_be_restored() {
        for status in ["paused", "awaiting_approval", "awaiting_review"] {
            assert!(recovered_session_is_resumable(status, false));
        }
        assert!(!recovered_session_is_resumable("uncertain", false));
    }
}
