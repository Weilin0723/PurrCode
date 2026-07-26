//! Daemon-backed terminal interface for daily PurrCode use.

use anyhow::{bail, Context, Result};
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph, Wrap};
use ratatui::Terminal;
use reqwest::Method;
use serde::Deserialize;
use serde_json::{json, Value};
use std::io::{self, Write};
use std::path::PathBuf;
use std::time::{Duration, Instant};

#[derive(Clone, Debug)]
pub struct TuiConfig {
    pub daemon_url: String,
    pub token_file: PathBuf,
    pub repository: PathBuf,
}

#[derive(Clone, Debug, Deserialize)]
struct SessionView {
    id: String,
    objective: Option<String>,
    status_code: String,
    worktree: Option<PathBuf>,
    event_count: u64,
    #[allow(dead_code)]
    lease_active: bool,
    selected_model: Option<String>,
}

struct App {
    client: DaemonClient,
    repository: PathBuf,
    sessions: Vec<SessionView>,
    selected: usize,
    events: Vec<Value>,
    message: String,
    diff: Option<String>,
    last_refresh: Instant,
}

pub async fn run(config: TuiConfig) -> Result<()> {
    let token = std::fs::read_to_string(&config.token_file).with_context(|| {
        format!(
            "daemon token is unavailable at {}; run `purrcode init` or `purrcode serve`",
            config.token_file.display()
        )
    })?;
    let mut app = App {
        client: DaemonClient::new(config.daemon_url, token.trim().into())?,
        repository: config.repository,
        sessions: Vec::new(),
        selected: 0,
        events: Vec::new(),
        message: "n new · a approve · d deny · e edit command · p pause · r resume · v diff · t checkpoint · z rollback · x compact · m model · c cancel · q quit".into(),
        diff: None,
        last_refresh: Instant::now() - Duration::from_secs(10),
    };
    app.refresh().await?;
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
    loop {
        if app.last_refresh.elapsed() >= Duration::from_millis(500) {
            if let Err(error) = app.refresh().await {
                app.message = format!("refresh failed: {error}");
            }
        }
        terminal.draw(|frame| draw(frame, app))?;
        if !event::poll(Duration::from_millis(100))? {
            continue;
        }
        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        match key.code {
            KeyCode::Char('q') => return Ok(()),
            KeyCode::Down | KeyCode::Char('j') => {
                if app.selected + 1 < app.sessions.len() {
                    app.selected += 1;
                    app.diff = None;
                    app.refresh_events().await?;
                }
            }
            KeyCode::Up | KeyCode::Char('k') => {
                if app.selected > 0 {
                    app.selected -= 1;
                    app.diff = None;
                    app.refresh_events().await?;
                }
            }
            KeyCode::Char('n') => {
                let objective = prompt(terminal, "Task objective")?;
                if !objective.trim().is_empty() {
                    app.client
                        .request(
                            Method::POST,
                            "/v1/sessions",
                            Some(json!({
                                "objective":objective,
                                "repository":app.repository
                            })),
                        )
                        .await?;
                    app.selected = 0;
                    app.refresh().await?;
                }
            }
            KeyCode::Char('a') => app.command("approve", json!({})).await?,
            KeyCode::Char('d') => {
                let reason = prompt(terminal, "Denial reason")?;
                app.command("reject", json!({"reason":reason})).await?;
            }
            KeyCode::Char('e') => {
                let Some(mut action) = app.pending_action() else {
                    app.message = "No pending proposed action is available".into();
                    continue;
                };
                if action["type"] != "command" {
                    app.message = "Only proposed commands can be edited in the TUI".into();
                    continue;
                }
                let current_program = action["program"].as_str().unwrap_or_default();
                let program = prompt(terminal, &format!("Program (current: {current_program})"))?;
                let current_arguments = action["arguments"].clone();
                let arguments = prompt(
                    terminal,
                    &format!("Arguments as JSON array (current: {current_arguments})"),
                )?;
                if !program.trim().is_empty() {
                    action["program"] = Value::String(program);
                }
                if !arguments.trim().is_empty() {
                    let parsed: Value = serde_json::from_str(&arguments)
                        .context("arguments must be a JSON string array")?;
                    if !parsed
                        .as_array()
                        .is_some_and(|items| items.iter().all(Value::is_string))
                    {
                        app.message = "Arguments must be a JSON string array".into();
                        continue;
                    }
                    action["arguments"] = parsed;
                }
                app.command(
                    "replace-action",
                    json!({"action":action,"reason":"command edited from TUI"}),
                )
                .await?;
            }
            KeyCode::Char('r') => app.command("resume", json!({})).await?,
            KeyCode::Char('p') => {
                app.command("pause", json!({"reason":"paused from TUI"}))
                    .await?
            }
            KeyCode::Char('t') => {
                let label = prompt(terminal, "Checkpoint label")?;
                app.command("checkpoint", json!({"label":label})).await?
            }
            KeyCode::Char('z') => app.command("rollback", json!({})).await?,
            KeyCode::Char('x') => app.command("compact", json!({})).await?,
            KeyCode::Char('m') => {
                let model = prompt(terminal, "Model (provider/model)")?;
                if !model.trim().is_empty() {
                    app.command("model", json!({"model":model})).await?;
                }
            }
            KeyCode::Char('c') => {
                app.command("cancel", json!({"reason":"cancelled from TUI"}))
                    .await?
            }
            KeyCode::Char('v') => app.load_diff().await?,
            KeyCode::Esc => app.diff = None,
            _ => {}
        }
    }
}

fn draw(frame: &mut ratatui::Frame<'_>, app: &App) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(10),
            Constraint::Length(3),
        ])
        .split(frame.area());
    let title = Paragraph::new(Line::from(vec![
        Span::styled(
            "PurrCode",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  Models propose · Judgment authorizes · Runtime executes · Evidence verifies"),
    ]))
    .block(Block::default().borders(Borders::ALL));
    frame.render_widget(title, rows[0]);
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(30), Constraint::Percentage(70)])
        .split(rows[1]);
    let items = app.sessions.iter().enumerate().map(|(index, session)| {
        let marker = if index == app.selected { "▶" } else { " " };
        ListItem::new(format!(
            "{marker} {} [{}]\n  {} · {} events",
            session.objective.as_deref().unwrap_or("<untitled>"),
            session.status_code,
            session.selected_model.as_deref().unwrap_or("default model"),
            session.event_count
        ))
    });
    frame.render_widget(
        List::new(items).block(Block::default().title("Sessions").borders(Borders::ALL)),
        columns[0],
    );
    let detail = app
        .diff
        .clone()
        .unwrap_or_else(|| render_event_summary(&app.events));
    frame.render_widget(
        Paragraph::new(detail).wrap(Wrap { trim: false }).block(
            Block::default()
                .title(if app.diff.is_some() {
                    "Diff (Esc closes)"
                } else {
                    "Plan · Judgment · Actions · Validation"
                })
                .borders(Borders::ALL),
        ),
        columns[1],
    );
    frame.render_widget(
        Paragraph::new(app.message.as_str())
            .wrap(Wrap { trim: true })
            .block(Block::default().title("Actions").borders(Borders::ALL)),
        rows[2],
    );
}

fn render_event_summary(events: &[Value]) -> String {
    if events.is_empty() {
        return "No session selected. Press n to start a task.".into();
    }
    events
        .iter()
        .rev()
        .take(30)
        .rev()
        .map(|event| {
            let kind = event["event"].as_str().unwrap_or("unknown");
            let data = &event["data"];
            match kind {
                "plan_created" => format!("PLAN\n{}\n", compact(data)),
                "submodules_prepared" => format!("SUBMODULES\n{}\n", compact(data)),
                "plan_revised" => format!("REPLAN\n{}\n", compact(data)),
                "context_compacted" => format!("CONTEXT COMPACTED\n{}\n", compact(data)),
                "session_paused" => format!("PAUSED\n{}\n", compact(data)),
                "session_resumed" => "RESUMED\n".into(),
                "model_selected" => format!("MODEL\n{}\n", compact(data)),
                "judgment_recorded" => format!("JUDGMENT\n{}\n", compact(data)),
                "contextual_judgment_recorded" | "outcome_judgment_recorded" => {
                    format!("SEMANTIC JUDGMENT\n{}\n", compact(data))
                }
                "action_proposed" => format!("ACTION\n{}\n", compact(data)),
                "action_output_recorded" => format!("TERMINAL\n{}\n", compact(data)),
                "validation_recorded" => format!("VALIDATION\n{}\n", compact(data)),
                "session_completed" => "✓ COMPLETED\n".into(),
                "session_failed" => format!("✗ FAILED {}\n", compact(data)),
                "session_cancelled" => format!("■ CANCELLED {}\n", compact(data)),
                _ => format!("{kind}: {}\n", compact(data)),
            }
        })
        .collect()
}

fn compact(value: &Value) -> String {
    let encoded = serde_json::to_string(value).unwrap_or_else(|_| "<invalid event>".into());
    if encoded.len() > 1200 {
        format!("{}…", encoded.chars().take(1200).collect::<String>())
    } else {
        encoded
    }
}

fn prompt(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>, label: &str) -> Result<String> {
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    print!("{label}: ");
    io::stdout().flush()?;
    let mut value = String::new();
    io::stdin().read_line(&mut value)?;
    execute!(terminal.backend_mut(), EnterAlternateScreen)?;
    enable_raw_mode()?;
    terminal.clear()?;
    Ok(value.trim().into())
}

impl App {
    fn pending_action(&self) -> Option<Value> {
        self.events.iter().rev().find_map(|event| {
            (event["event"] == "action_proposed").then(|| event["data"]["action"].clone())
        })
    }

    async fn refresh(&mut self) -> Result<()> {
        self.sessions = serde_json::from_value(
            self.client
                .request(Method::GET, "/v1/sessions", None)
                .await?,
        )?;
        if self.selected >= self.sessions.len() {
            self.selected = self.sessions.len().saturating_sub(1);
        }
        self.refresh_events().await?;
        self.last_refresh = Instant::now();
        Ok(())
    }

    async fn refresh_events(&mut self) -> Result<()> {
        let Some(session) = self.sessions.get(self.selected) else {
            self.events.clear();
            return Ok(());
        };
        self.events = serde_json::from_value(
            self.client
                .request(
                    Method::GET,
                    &format!("/v1/sessions/{}/events", session.id),
                    None,
                )
                .await?,
        )?;
        Ok(())
    }

    async fn command(&mut self, command: &str, body: Value) -> Result<()> {
        let Some(session) = self.sessions.get(self.selected) else {
            self.message = "No session selected".into();
            return Ok(());
        };
        let id = session.id.clone();
        let response = self
            .client
            .request(
                Method::POST,
                &format!("/v1/sessions/{id}/{command}"),
                Some(body),
            )
            .await?;
        self.message = format!("{command}: {}", compact(&response));
        self.refresh().await
    }

    async fn load_diff(&mut self) -> Result<()> {
        let Some(session) = self.sessions.get(self.selected) else {
            return Ok(());
        };
        let Some(worktree) = &session.worktree else {
            self.message = "Session has no worktree yet".into();
            return Ok(());
        };
        let output = tokio::process::Command::new("git")
            .args(["diff", "--binary", "HEAD", "--", "."])
            .current_dir(worktree)
            .output()
            .await?;
        if !output.status.success() {
            self.message = String::from_utf8_lossy(&output.stderr).into_owned();
        } else {
            self.diff = Some(String::from_utf8_lossy(&output.stdout).into_owned());
        }
        Ok(())
    }
}

struct DaemonClient {
    base_url: String,
    token: String,
    http: reqwest::Client,
}

impl DaemonClient {
    fn new(base_url: String, token: String) -> Result<Self> {
        if token.len() < 32 {
            bail!("daemon token is invalid");
        }
        Ok(Self {
            base_url: base_url.trim_end_matches('/').into(),
            token,
            http: reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(3))
                .timeout(Duration::from_secs(30))
                .build()?,
        })
    }

    async fn request(&self, method: Method, path: &str, body: Option<Value>) -> Result<Value> {
        let mut request = self
            .http
            .request(method, format!("{}{}", self.base_url, path))
            .bearer_auth(&self.token);
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().await.context("connect to PurrCode daemon")?;
        let status = response.status();
        let value: Value = response.json().await.context("decode daemon response")?;
        if !status.is_success() {
            bail!("daemon HTTP {status}: {value}");
        }
        Ok(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_summary_surfaces_semantic_judgment_and_validation() {
        let events = vec![
            json!({"event":"contextual_judgment_recorded","data":{
                "judgment":{"decision":"require_approval","reasons":["auth risk"]}
            }}),
            json!({"event":"validation_recorded","data":{
                "status":"unavailable","evidence":"scanner not installed"
            }}),
        ];
        let rendered = render_event_summary(&events);
        assert!(rendered.contains("SEMANTIC JUDGMENT"));
        assert!(rendered.contains("auth risk"));
        assert!(rendered.contains("VALIDATION"));
        assert!(rendered.contains("scanner not installed"));
    }
}
