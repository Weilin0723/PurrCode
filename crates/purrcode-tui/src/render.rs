//! Ratatui rendering for all TUI modes.

use crate::app::{App, AppMode};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph, Wrap};
use ratatui::Frame;

pub fn draw(frame: &mut Frame<'_>, app: &App) {
    match app.mode {
        AppMode::ProviderSetup => draw_setup(frame, app),
        AppMode::SkillBrowse => draw_skills(frame, app),
        AppMode::DiffView => draw_diff(frame, app),
        AppMode::Conversation => draw_conversation(frame, app),
        AppMode::ModelPicker => draw_conversation(frame, app),
    }
}

fn layout_full(frame: &Frame<'_>) -> [Rect; 4] {
    let area = frame.area();
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(5),
            Constraint::Length(4),
            Constraint::Length(3),
        ])
        .split(area);
    [rows[0], rows[1], rows[2], rows[3]]
}

// ── Conversation mode ────────────────────────────────────────────

fn draw_conversation(frame: &mut Frame<'_>, app: &App) {
    let [header, body, action_area, input_area] = layout_full(frame);

    draw_header(frame, header, app);
    draw_messages(frame, body, app);
    draw_action_panel(frame, action_area, app);
    draw_composer(frame, input_area, app);
}

fn draw_header(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let mode_str = match app.conversation.mode {
        purrcode_runtime_core::ConversationMode::Plan => "Plan",
        purrcode_runtime_core::ConversationMode::Build => "Build",
        purrcode_runtime_core::ConversationMode::Review => "Review",
        purrcode_runtime_core::ConversationMode::Ask => "Ask",
    };

    let privacy_indicator = if app.status_bar.privacy == "local-only" {
        "🔒"
    } else {
        "🌐"
    };

    let local_indicator = if app.status_bar.local { "local" } else { "remote" };

    let title = Line::from(vec![
        Span::styled("PurrCode", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
        Span::raw(" · "),
        Span::styled(mode_str, Style::default().fg(Color::Green)),
        Span::raw(" · "),
        Span::styled(&app.status_bar.model, Style::default().fg(Color::Yellow)),
        Span::raw(format!(" · {local_indicator} · {privacy_indicator}")),
    ]);
    frame.render_widget(
        Paragraph::new(title).block(Block::default().borders(Borders::BOTTOM)),
        area,
    );
}

fn draw_messages(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let mut items: Vec<ListItem> = app
        .conversation
        .messages
        .iter()
        .map(|msg| {
            let prefix = match msg.role.as_str() {
                "user" => "You: ",
                "assistant" => "PurrCode: ",
                _ => "System: ",
            };
            let style = match msg.role.as_str() {
                "user" => Style::default().fg(Color::Cyan),
                "assistant" => Style::default().fg(Color::White),
                _ => Style::default().fg(Color::DarkGray),
            };
            let text = format!("{prefix}{}", msg.content);
            ListItem::new(text).style(style)
        })
        .collect();

    if let Some(ref msg) = app.conversation.streaming_message {
        let text = format!("PurrCode: {}", msg.content);
        items.push(ListItem::new(text).style(Style::default().fg(Color::White)));
    }

    if items.is_empty() {
        items.push(ListItem::new(
            "No messages yet. Type a message or /connect to set up a provider.",
        ));
    }

    frame.render_widget(
        List::new(items).block(
            Block::default()
                .title("Conversation")
                .borders(Borders::ALL),
        ),
        area,
    );
}

fn draw_action_panel(frame: &mut Frame<'_>, area: Rect, app: &App) {

    let mut text = String::new();
    if !app.message_bar.is_empty() {
        text.push_str(&app.message_bar);
        text.push('\n');
    }
    if app.stream.active {
        text.push_str("● Streaming...");
    }

    frame.render_widget(
        Paragraph::new(text)
            .wrap(Wrap { trim: true })
            .block(Block::default().title("Actions").borders(Borders::ALL)),
        area,
    );
}

fn draw_composer(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let prefix = if app.composer.is_command() { "" } else { "> " };
    let display = format!("{}{}", prefix, app.composer.buffer);

    let style = if app.composer.is_command() {
        Style::default().fg(Color::Green)
    } else {
        Style::default()
    };

    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(display, style)))
            .block(Block::default().borders(Borders::ALL)),
        area,
    );
}

// ── Provider Setup mode ──────────────────────────────────────────

fn draw_setup(frame: &mut Frame<'_>, app: &App) {
    let area = frame.area();
    let Some(ref setup) = app.provider_setup else {
        return;
    };

    let content: String = match setup.provider_type {
        None => {
            r#"Choose a provider:

  1. Ollama (local)
  2. LM Studio (local)
  3. OpenAI
  4. OpenAI-compatible endpoint
  5. Enterprise gateway

  Press 1-5 to select, Esc to cancel."#
                .to_string()
        }
        Some(ref pt) => setup_text(setup, pt),
    };

    frame.render_widget(
        Paragraph::new(content)
            .block(Block::default().title("Connect Provider").borders(Borders::ALL))
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn setup_text(setup: &crate::provider_setup::ProviderSetup, pt: &crate::provider_setup::ProviderType) -> String {
    let base = match pt {
        crate::provider_setup::ProviderType::Ollama => format!("Provider: Ollama\nBase URL: {}\n\nPress Enter to discover models, Esc to cancel.", setup.base_url),
        crate::provider_setup::ProviderType::LmStudio => format!("Provider: LM Studio\nBase URL: {}\n\nPress Enter to discover models, Esc to cancel.", setup.base_url),
        crate::provider_setup::ProviderType::Openai => {
            let key_status = if setup.api_key.is_empty() { "not set" } else { "✓ set" };
            format!("Provider: OpenAI\nAPI key: {key_status}\n\nEnter API key, then press Enter to test connection.")
        }
        crate::provider_setup::ProviderType::OpenaiCompatible => format!("Provider: OpenAI-compatible\nBase URL: {}\nAPI key: {}\nModel: {}\n\nPress Enter to test connection.", setup.base_url, if setup.api_key.is_empty() { "not set" } else { "✓ set" }, setup.model_id),
        crate::provider_setup::ProviderType::EnterpriseGateway => format!("Provider: Enterprise Gateway\nBase URL: {}\n\nPress Enter to configure.", setup.base_url),
    };

    if let Some(ref result) = setup.test_result {
        format!("{base}\n\n{result}")
    } else {
        base
    }
}

// ── Skill Browser mode ───────────────────────────────────────────

fn draw_skills(frame: &mut Frame<'_>, app: &App) {
    let area = frame.area();
    let Some(ref browser) = app.skill_browser else {
        return;
    };

    let items: Vec<ListItem> = browser
        .skills
        .iter()
        .enumerate()
        .map(|(i, skill)| {
            let marker = if i == browser.selected { "▶" } else { " " };
            let status = if skill.installed { " [installed]" } else { "" };
            ListItem::new(format!(
                "{marker} {name} v{ver}{status}\n  Publisher: {pub} · {sig} · Risk: {risk}",
                name = skill.skill_id,
                ver = skill.version,
                pub = skill.publisher,
                sig = skill.signature,
                risk = skill.risk,
            ))
        })
        .collect();

    frame.render_widget(
        List::new(items).block(
            Block::default()
                .title("Skills (Esc to close, i to install)")
                .borders(Borders::ALL),
        ),
        area,
    );
}

// ── Diff view mode ───────────────────────────────────────────────

fn draw_diff(frame: &mut Frame<'_>, app: &App) {
    let area = frame.area();
    let content = app
        .diff_view
        .as_ref()
        .map(|d| d.content.as_str())
        .unwrap_or("No diff available.");

    frame.render_widget(
        Paragraph::new(content)
            .block(Block::default().title("Diff (Esc closes)").borders(Borders::ALL))
            .wrap(Wrap { trim: false }),
        area,
    );
}
