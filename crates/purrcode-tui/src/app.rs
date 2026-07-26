//! Application state, event loop, and mode management.

use crate::command_palette::CommandPalette;
use crate::composer::Composer;
use crate::conversation::Conversation;
use crate::diff_view::DiffView;
use crate::keybindings::handle_key;
use crate::provider_setup::ProviderSetup;
use crate::render::draw;
use crate::skill_browser::SkillBrowser;
use crate::status_bar::StatusBar;
use crate::streaming::{StreamController, StreamEvent};
use anyhow::Result;
use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyEventKind, MouseEventKind,
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
    ProviderSetup,
    SkillBrowse,
    DiffView,
}

pub struct App {
    pub config: TuiConfig,
    pub client: reqwest::Client,
    pub token: String,
    pub mode: AppMode,
    pub conversation: Conversation,
    pub composer: Composer,
    pub status_bar: StatusBar,
    pub provider_setup: Option<ProviderSetup>,
    pub skill_browser: Option<SkillBrowser>,
    pub diff_view: Option<DiffView>,
    pub stream: StreamController,
    pub last_refresh: Instant,
    pub message_bar: String,
    pub session_id: Option<String>,
    pub has_provider: bool,
    pub pending_command: Option<String>,
    pub pending_user_message: bool,
    pub running_command: bool,
    pub quit_requested: bool,
    pub downloaded_skill: Option<Value>,
    pub pending_skill_install_action: Option<String>,
}

pub async fn run(config: TuiConfig) -> Result<()> {
    let token = std::fs::read_to_string(&config.token_file)?;
    let token = token.trim().to_string();

    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(3))
        .timeout(Duration::from_secs(60))
        .build()?;

    let mut app = App {
        config,
        client,
        token,
        mode: AppMode::Conversation,
        conversation: Conversation::new(),
        composer: Composer::new(),
        status_bar: StatusBar::new(),
        provider_setup: None,
        skill_browser: None,
        diff_view: None,
        stream: StreamController::new(),
        last_refresh: Instant::now(),
        message_bar: String::new(),
        session_id: None,
        has_provider: false,
        pending_command: None,
        pending_user_message: false,
        running_command: false,
        quit_requested: false,
        downloaded_skill: None,
        pending_skill_install_action: None,
    };

    app.check_provider().await;

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    let result = event_loop(&mut terminal, &mut app).await;
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        DisableMouseCapture,
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
        // Refresh state
        if app.last_refresh.elapsed() >= Duration::from_millis(100) {
            app.refresh().await;
            app.last_refresh = Instant::now();
        }

        // Poll SSE stream
        if let Some(ref mut rx) = app.stream.receiver {
            match rx.try_recv() {
                Ok(StreamEvent::Delta(text)) => {
                    app.conversation.append_streaming(&text);
                }
                Ok(StreamEvent::ToolCall(tc)) => {
                    app.conversation.pending_action = Some(tc);
                }
                Ok(StreamEvent::Done) => {
                    app.conversation.finalize_streaming();
                    app.stream.active = false;
                    app.message_bar = "Done.".into();
                }
                Ok(StreamEvent::Error(e)) => {
                    app.conversation.cancel_streaming();
                    app.stream.active = false;
                    app.message_bar = format!("Stream error: {e}");
                }
                Ok(StreamEvent::Usage { input, output }) => {
                    app.status_bar.context_info = format!("{input} in / {output} out");
                }
                Err(_) => {} // no events ready
            }
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
                        app.message_bar = format!("Message error: {error}");
                        continue;
                    }
                }
                app.conversation
                    .start_streaming(Some(app.status_bar.model.clone()));

                let tx = app.stream.start();

                let daemon_url = app.daemon_url().to_string();
                let token = app.token.clone();
                let client = app.client.clone();
                let session_id = app.session_id.clone();

                tokio::spawn(async move {
                    let sid = session_id.unwrap_or_default();
                    let url = format!(
                        "{}/v1/sessions/{}/events/stream?after={stream_after}",
                        daemon_url.trim_end_matches('/'),
                        sid
                    );
                    let resp = client.get(&url).bearer_auth(&token).send().await;
                    match resp {
                        Ok(rsp) => {
                            let mut stream = rsp.bytes_stream();
                            let mut buffer = String::new();
                            while let Some(chunk) = stream.next().await {
                                match chunk {
                                    Ok(bytes) => {
                                        buffer.push_str(&String::from_utf8_lossy(&bytes));
                                        while let Some(pos) = buffer.find("\n\n") {
                                            let event_block = buffer[..pos].to_string();
                                            buffer = buffer[pos + 2..].to_string();
                                            for line in event_block.lines() {
                                                if let Some(data) = line.strip_prefix("data: ") {
                                                    if let Ok(val) =
                                                        serde_json::from_str::<serde_json::Value>(
                                                            data,
                                                        )
                                                    {
                                                        if let Some(delta) = val
                                                            .get("delta")
                                                            .and_then(|value| value.as_str())
                                                        {
                                                            let _ = tx
                                                                .send(StreamEvent::Delta(
                                                                    delta.to_owned(),
                                                                ))
                                                                .await;
                                                        }
                                                        if let Some(tc) =
                                                            val.pointer("/data/action")
                                                        {
                                                            let _ = tx
                                                                .send(StreamEvent::ToolCall(
                                                                    tc.clone(),
                                                                ))
                                                                .await;
                                                        }
                                                        if let Some(usage) = val.get("usage") {
                                                            let input = usage
                                                                .get("input_tokens")
                                                                .and_then(|v| v.as_u64())
                                                                .unwrap_or(0);
                                                            let output = usage
                                                                .get("output_tokens")
                                                                .and_then(|v| v.as_u64())
                                                                .unwrap_or(0);
                                                            let _ = tx
                                                                .send(StreamEvent::Usage {
                                                                    input,
                                                                    output,
                                                                })
                                                                .await;
                                                        }
                                                        if matches!(
                                                            val.get("event")
                                                                .and_then(|v| v.as_str()),
                                                            Some(
                                                                "session_completed"
                                                                    | "session_failed"
                                                                    | "outcome_review_required"
                                                            )
                                                        ) {
                                                            let _ =
                                                                tx.send(StreamEvent::Done).await;
                                                            return;
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        let _ = tx.send(StreamEvent::Error(e.to_string())).await;
                                        return;
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            let _ = tx.send(StreamEvent::Error(e.to_string())).await;
                        }
                    }
                });
                app.message_bar = "Processing...".into();
            }
        }

        // Render
        terminal.draw(|frame| draw(frame, app))?;

        // Wait for input
        if !event::poll(Duration::from_millis(50))? {
            continue;
        }

        let input = event::read()?;
        if let Event::Mouse(mouse) = input {
            match mouse.kind {
                MouseEventKind::ScrollUp => {
                    app.conversation.scroll = app.conversation.scroll.saturating_sub(3)
                }
                MouseEventKind::ScrollDown => {
                    app.conversation.scroll = app.conversation.scroll.saturating_add(3)
                }
                _ => {}
            }
            continue;
        }
        let Event::Key(key) = input else { continue };
        if key.kind != KeyEventKind::Press {
            continue;
        }

        if !handle_key(app, key) {
            return Ok(());
        }
    }
}

impl App {
    pub fn daemon_url(&self) -> &str {
        self.config.daemon_url.trim_end_matches('/')
    }

    pub async fn request(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<Value> {
        let url = format!("{}{}", self.daemon_url(), path);
        let mut req = self.client.request(method, &url).bearer_auth(&self.token);
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

    pub async fn check_provider(&mut self) {
        match self
            .request(reqwest::Method::GET, "/v1/providers", None)
            .await
        {
            Ok(val) => {
                let arr = val.as_array();
                self.has_provider = arr.is_some_and(|a| !a.is_empty());
                if !self.has_provider {
                    self.message_bar =
                        "No provider configured. Type /connect to set one up.".into();
                }
            }
            Err(_) => {
                self.has_provider = false;
                self.message_bar = "Daemon unreachable. Type /connect to set up.".into();
            }
        }
    }

    pub async fn refresh(&mut self) {
        if self.mode == AppMode::Conversation {
            let token = self.token.clone();
            let url = self.daemon_url().to_string();
            let session_id = self.session_id.clone();
            self.conversation
                .refresh_events(&url, &token, session_id)
                .await;
        }
    }

    pub async fn ensure_session(&mut self) -> Option<String> {
        if self.session_id.is_some() {
            return self.session_id.clone();
        }
        let body = serde_json::json!({
            "objective": self.conversation.current_objective(),
            "repository": self.config.repository,
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
        let (name, kind) = match provider_type {
            crate::provider_setup::ProviderType::Ollama => ("ollama", "ollama"),
            crate::provider_setup::ProviderType::LmStudio => ("lm-studio", "lm-studio"),
            crate::provider_setup::ProviderType::Openai => ("openai", "openai"),
            crate::provider_setup::ProviderType::OpenaiCompatible => {
                ("openai-compatible", "openai-compatible")
            }
            crate::provider_setup::ProviderType::EnterpriseGateway => {
                setup.error =
                    Some("Enterprise gateway setup requires configuration-file policy".into());
                setup.complete = false;
                self.provider_setup = Some(setup);
                return;
            }
        };
        let credential_name = if setup.api_key.is_empty() {
            None
        } else {
            let result = self
                .request(
                    reqwest::Method::POST,
                    "/v1/credentials",
                    Some(serde_json::json!({"name": name, "secret": setup.api_key})),
                )
                .await;
            setup.api_key.zeroize();
            if let Err(error) = result {
                setup.error = Some(format!("Credential storage failed: {error}"));
                setup.complete = false;
                self.provider_setup = Some(setup);
                return;
            }
            Some(name)
        };
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
                })),
            )
            .await;
        if let Err(error) = configured {
            setup.error = Some(format!("Provider configuration failed: {error}"));
            setup.complete = false;
            self.provider_setup = Some(setup);
            return;
        }
        match self
            .request(
                reqwest::Method::POST,
                "/v1/providers/test",
                Some(serde_json::json!({"provider": name})),
            )
            .await
        {
            Ok(_) => {
                self.has_provider = true;
                self.mode = AppMode::Conversation;
                self.message_bar = format!("Provider {name} connected and verified.");
            }
            Err(error) => {
                setup.error = Some(format!("Connection test failed: {error}"));
                setup.complete = false;
                self.provider_setup = Some(setup);
            }
        }
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
