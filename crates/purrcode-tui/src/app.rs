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
use crossterm::event::{self, Event, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use futures::StreamExt;
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use serde_json::Value;
use std::io;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

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
    ModelPicker,
}

pub struct App {
    pub config: TuiConfig,
    pub client: reqwest::Client,
    pub token: String,
    pub mode: AppMode,
    pub conversation: Conversation,
    pub composer: Composer,
    pub status_bar: StatusBar,
    pub command_palette: CommandPalette,
    pub provider_setup: Option<ProviderSetup>,
    pub skill_browser: Option<SkillBrowser>,
    pub diff_view: Option<DiffView>,
    pub model_picker_visible: bool,
    pub stream: StreamController,
    pub last_refresh: Instant,
    pub message_bar: String,
    pub session_id: Option<String>,
    pub has_provider: bool,
    pub pending_command: Option<String>,
    pub pending_user_message: bool,
    pub running_command: bool,
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
        command_palette: CommandPalette::new(),
        provider_setup: None,
        skill_browser: None,
        diff_view: None,
        model_picker_visible: false,
        stream: StreamController::new(),
        last_refresh: Instant::now(),
        message_bar: String::new(),
        session_id: None,
        has_provider: false,
        pending_command: None,
        pending_user_message: false,
        running_command: false,
    };

    app.check_provider().await;

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    let result = event_loop(&mut terminal, &mut app).await;
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    result
}

async fn event_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
) -> Result<()> {
    let mut stream_rx: Option<mpsc::Receiver<StreamEvent>> = None;

    loop {
        // Refresh state
        if app.last_refresh.elapsed() >= Duration::from_millis(100) {
            app.refresh().await;
            app.last_refresh = Instant::now();
        }

        // Poll SSE stream
        if let Some(ref mut rx) = stream_rx {
            match rx.try_recv() {
                Ok(StreamEvent::Delta(text)) => {
                    app.conversation.append_streaming(&text);
                }
                Ok(StreamEvent::ToolCall(tc)) => {
                    app.conversation.pending_action = Some(tc);
                }
                Ok(StreamEvent::Done) => {
                    app.conversation.finalize_streaming();
                    stream_rx = None;
                    app.stream.stop();
                    app.message_bar = "Done.".into();
                }
                Ok(StreamEvent::Error(e)) => {
                    app.conversation.cancel_streaming();
                    stream_rx = None;
                    app.stream.stop();
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
            let cmd_clone = cmd.clone();
            let daemon_url = app.daemon_url().to_string();
            let token = app.token.clone();
            let client = reqwest::Client::new();

            app.running_command = true;
            tokio::spawn(async move {
                let cp = CommandPalette::new();
                // We execute via a separate method that doesn't require &mut App
                cp.execute_detached(&client, &daemon_url, &token, &cmd_clone).await;
            });
            app.message_bar = format!("Running: {cmd}");
        }

        // Process pending user message
        if app.pending_user_message {
            app.pending_user_message = false;
            app.conversation.finalize_streaming();

            let objective = app.conversation.current_objective();
            if objective.is_empty() {
                app.message_bar = "No objective set. Type a task first.".into();
            } else {
                app.ensure_session().await;
                app.conversation
                    .start_streaming(Some(app.status_bar.model.clone()));

                let tx = app.stream.start();
                stream_rx = Some(tx);

                let daemon_url = app.daemon_url().to_string();
                let token = app.token.clone();
                let client = app.client.clone();
                let session_id = app.session_id.clone();

                tokio::spawn(async move {
                    let sid = session_id.unwrap_or_default();
                    let url = format!("{}/v1/sessions/{}/events/stream", daemon_url.trim_end_matches('/'), sid);
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
                                                    if data == "[DONE]" {
                                                        let _ = tx.send(StreamEvent::Done).await;
                                                    } else if let Ok(val) = serde_json::from_str::<serde_json::Value>(data) {
                                                        if let Some(delta) = val.get("delta").and_then(|d| d.as_str()) {
                                                            let _ = tx.send(StreamEvent::Delta(delta.to_string())).await;
                                                        }
                                                        if let Some(tc) = val.get("tool_call") {
                                                            let _ = tx.send(StreamEvent::ToolCall(tc.clone())).await;
                                                        }
                                                        if let Some(usage) = val.get("usage") {
                                                            let input = usage.get("input_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
                                                            let output = usage.get("output_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
                                                            let _ = tx.send(StreamEvent::Usage { input, output }).await;
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
                            // Done if stream closes cleanly after loop
                            let _ = tx.send(StreamEvent::Done).await;
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

        let Event::Key(key) = event::read()? else {
            continue;
        };
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

    pub async fn request(&self, method: reqwest::Method, path: &str, body: Option<Value>) -> Result<Value> {
        let url = format!("{}{}", self.daemon_url(), path);
        let mut req = self.client.request(method, &url).bearer_auth(&self.token);
        if let Some(body) = body {
            req = req.json(&body);
        }
        let resp = req.send().await?;
        let status = resp.status();
        let value: Value = resp.json().await?;
        if !status.is_success() {
            anyhow::bail!("daemon HTTP {}: {}", status, value);
        }
        Ok(value)
    }

    pub async fn check_provider(&mut self) {
        match self.request(reqwest::Method::GET, "/v1/providers", None).await {
            Ok(val) => {
                let arr = val.as_array();
                self.has_provider = arr.is_some_and(|a| !a.is_empty());
                if !self.has_provider {
                    self.message_bar = "No provider configured. Type /connect to set one up.".into();
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
            self.conversation.refresh_events(&url, &token, session_id).await;
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
        match self.request(reqwest::Method::POST, "/v1/sessions", Some(body)).await {
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
}
