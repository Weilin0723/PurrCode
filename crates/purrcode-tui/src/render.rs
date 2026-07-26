//! Ratatui rendering for all TUI modes.

use crate::app::{App, AppMode};
use crate::timeline::{action_summary, CardKind, TimelineCard};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph, Wrap};
use ratatui::Frame;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

pub fn draw(frame: &mut Frame<'_>, app: &App) {
    match app.mode {
        AppMode::SecretReview => draw_secret_review(frame, app),
        AppMode::ProviderSetup => draw_setup(frame, app),
        AppMode::SkillBrowse => draw_skills(frame, app),
        AppMode::DiffView => draw_diff(frame, app),
        AppMode::Conversation => draw_conversation(frame, app),
    }
}

fn draw_secret_review(frame: &mut Frame<'_>, app: &App) {
    let Some(review) = &app.secret_review else {
        return;
    };
    let import = if review.provider_candidate {
        "\nProvider configuration signals were detected. Press I to review as a provider import."
    } else {
        ""
    };
    let text = format!(
        "Sensitive content detected\n\n{} secret-like value(s) are hidden. Raw values have not been added to conversation history or sent to the daemon.{}\n\nR  Redact and send\nI  Import as provider configuration\nEsc  Cancel and keep draft",
        review.finding_count, import
    );
    frame.render_widget(
        Paragraph::new(text)
            .wrap(Wrap { trim: false })
            .block(Block::default().title("Secret guard").borders(Borders::ALL)),
        frame.area(),
    );
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum WorkspaceLayout {
    Wide,
    Compact,
    Narrow,
}

fn workspace_layout(width: u16) -> WorkspaceLayout {
    if width >= 120 {
        WorkspaceLayout::Wide
    } else if width >= 80 {
        WorkspaceLayout::Compact
    } else {
        WorkspaceLayout::Narrow
    }
}

fn layout_full(frame: &Frame<'_>, app: &App) -> [Rect; 5] {
    let area = frame.area();
    let composer_height = (app.composer.line_count().clamp(3, 10) as u16).saturating_add(2);
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(5),
            Constraint::Length(4),
            Constraint::Length(composer_height),
            Constraint::Length(1),
        ])
        .split(area);
    [rows[0], rows[1], rows[2], rows[3], rows[4]]
}

// ── Conversation mode ────────────────────────────────────────────

fn draw_conversation(frame: &mut Frame<'_>, app: &App) {
    let [header, body, action_area, input_area, footer] = layout_full(frame, app);

    draw_header(frame, header, app);
    let layout = workspace_layout(frame.area().width);
    let show_files = layout == WorkspaceLayout::Wide || app.workspace.file_panel_visible;
    if layout == WorkspaceLayout::Narrow && show_files {
        draw_workspace_panel(frame, body, app);
    } else if show_files {
        let columns = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(30), Constraint::Min(40)])
            .split(body);
        draw_workspace_panel(frame, columns[0], app);
        draw_messages(frame, columns[1], app);
    } else {
        draw_messages(frame, body, app);
    }
    draw_action_panel(frame, action_area, app);
    draw_composer(frame, input_area, app);
    draw_footer(frame, footer, app);
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

    let local_indicator = if app.status_bar.local {
        "local"
    } else {
        "remote"
    };

    let repository = format!("{}/{}", app.workspace.repository_name, app.workspace.branch);
    let session = app
        .session_id
        .as_deref()
        .map(|id| id.get(..8).unwrap_or(id))
        .unwrap_or("new");
    let title = Line::from(vec![
        Span::styled(
            concat!("PurrCode ", env!("CARGO_PKG_VERSION")),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" · "),
        Span::styled(repository, Style::default().fg(Color::Blue)),
        Span::raw(" · "),
        Span::styled(&app.status_bar.model, Style::default().fg(Color::Yellow)),
        Span::raw(format!(
            " · {mode_str} · {local_indicator} {privacy_indicator} · sandbox:{} · session:{session} {}",
            app.workspace.sandbox, app.workspace.session_phase
        )),
    ]);
    frame.render_widget(
        Paragraph::new(title).block(Block::default().borders(Borders::BOTTOM)),
        area,
    );
}

fn draw_messages(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let mut items: Vec<ListItem> = if app.conversation.timeline.is_empty() {
        app.conversation
            .messages
            .iter()
            .skip(app.conversation.scroll)
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
                ListItem::new(render_markdown(prefix, &msg.content, style))
            })
            .collect()
    } else {
        app.conversation
            .timeline
            .iter()
            .enumerate()
            .skip(app.conversation.scroll)
            .map(|(index, card)| {
                timeline_item(
                    card,
                    index,
                    app.conversation.selected_card == Some(index),
                    app.conversation.expanded_card == Some(index),
                )
            })
            .collect()
    };

    if let Some(ref msg) = app.conversation.streaming_message {
        let text = format!("PurrCode: {}", msg.content);
        items.push(ListItem::new(text).style(Style::default().fg(Color::White)));
    }

    if items.is_empty() {
        items.push(ListItem::new(Text::from(vec![
            Line::from(Span::styled(
                "PurrCode is ready.",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from("Describe a repository task, paste code or logs, or type /connect."),
            Line::from("PawGate judges → Claw executes → evidence verifies."),
            Line::from(""),
            Line::from(Span::styled(
                "Ctrl+Enter sends · Enter adds a line · Ctrl+B opens files",
                Style::default().fg(Color::DarkGray),
            )),
        ])));
    }

    frame.render_widget(
        List::new(items).block(Block::default().title("Timeline").borders(Borders::ALL)),
        area,
    );
}

fn timeline_item<'a>(
    card: &'a TimelineCard,
    index: usize,
    selected: bool,
    expanded: bool,
) -> ListItem<'a> {
    let (glyph, color) = match card.kind {
        CardKind::Conversation => ("●", Color::White),
        CardKind::Plan => ("◆", Color::Blue),
        CardKind::Action => ("▶", Color::Yellow),
        CardKind::PawGate => ("◆", Color::Magenta),
        CardKind::Claw => ("▶", Color::Cyan),
        CardKind::Output => ("│", Color::DarkGray),
        CardKind::Validation => ("✓", Color::Green),
        CardKind::Checkpoint => ("●", Color::Blue),
        CardKind::Recovery => ("!", Color::Red),
        CardKind::Completion => ("✓", Color::Green),
        CardKind::Skill => ("◇", Color::Cyan),
        CardKind::Context => ("·", Color::DarkGray),
    };
    let marker = if selected { "›" } else { " " };
    let mut lines = vec![Line::from(vec![
        Span::styled(format!("{marker}{glyph} "), Style::default().fg(color)),
        Span::styled(
            &card.title,
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ),
    ])];
    lines.extend(render_markdown("  ", &card.summary, Style::default().fg(Color::White)).lines);
    if expanded {
        for detail in &card.details {
            lines.extend(
                render_markdown("    ", detail, Style::default().fg(Color::DarkGray)).lines,
            );
        }
    } else if !card.details.is_empty() {
        lines.push(Line::from(Span::styled(
            format!(
                "    {} detail line(s) · Ctrl+Space to expand",
                card.details.len()
            ),
            Style::default().fg(Color::DarkGray),
        )));
    }
    lines.push(Line::from(""));
    let _ = index;
    ListItem::new(Text::from(lines))
}

fn draw_workspace_panel(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let mut lines = vec![
        Line::from(vec![
            Span::styled("Repository  ", Style::default().fg(Color::DarkGray)),
            Span::raw(&app.workspace.repository_name),
        ]),
        Line::from(vec![
            Span::styled("Branch      ", Style::default().fg(Color::DarkGray)),
            Span::raw(&app.workspace.branch),
        ]),
        Line::from(vec![
            Span::styled("Source      ", Style::default().fg(Color::DarkGray)),
            Span::raw(&app.workspace.source_state),
        ]),
        Line::from(vec![
            Span::styled("Daemon      ", Style::default().fg(Color::DarkGray)),
            Span::raw(&app.workspace.daemon_health),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            "Files",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )),
    ];
    let available = area.height.saturating_sub(8) as usize;
    for path in app.workspace.paths.iter().take(available) {
        let kind = if path.directory { "▸" } else { " " };
        let sensitive = if path.sensitive { " 🔒" } else { "" };
        lines.push(Line::from(format!("{kind} {}{sensitive}", path.display)));
    }
    if app.workspace.paths.len() > available {
        lines.push(Line::from(Span::styled(
            format!("… {} more paths", app.workspace.paths.len() - available),
            Style::default().fg(Color::DarkGray),
        )));
    }
    frame.render_widget(
        Paragraph::new(lines)
            .block(Block::default().title("Workspace").borders(Borders::ALL))
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn draw_footer(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let text = if app.conversation.pending_action.is_some() {
        "A Approve  R Reject  Ctrl+D Full diff  /approve and /deny remain exact-action bound"
    } else if app.workspace.file_panel_visible
        && workspace_layout(frame.area().width) != WorkspaceLayout::Wide
    {
        "Ctrl+B Timeline  @file mention  Ctrl+D Diff  ? Help"
    } else {
        "Ctrl+Enter Send  Enter Newline  Ctrl+P Commands  Ctrl+B Files  Ctrl+D Diff  ? Help"
    };
    frame.render_widget(
        Paragraph::new(text).style(Style::default().fg(Color::DarkGray)),
        area,
    );
}

fn render_markdown<'a>(prefix: &str, content: &'a str, base: Style) -> Text<'a> {
    let mut in_code = false;
    let mut lines = Vec::new();
    for (index, raw) in content.lines().enumerate() {
        if raw.trim_start().starts_with("```") {
            in_code = !in_code;
            continue;
        }
        let prefix = if index == 0 { prefix } else { "  " };
        let (text, style) = if in_code {
            (raw, Style::default().fg(Color::Green))
        } else if raw.starts_with('#') {
            (
                raw.trim_start_matches('#').trim_start(),
                base.add_modifier(Modifier::BOLD),
            )
        } else if raw.starts_with("- ") || raw.starts_with("* ") {
            (raw, base.fg(Color::Cyan))
        } else {
            (raw, base)
        };
        lines.push(Line::from(vec![
            Span::styled(prefix.to_owned(), base.add_modifier(Modifier::BOLD)),
            Span::styled(text.to_owned(), style),
        ]));
    }
    if lines.is_empty() {
        lines.push(Line::from(Span::styled(prefix.to_owned(), base)));
    }
    Text::from(lines)
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
    if let Some(action) = &app.conversation.pending_action {
        text.push_str(&format!(
            "\nPending action: {}\nA approve · R reject · Ctrl+D full diff",
            action_summary(action)
        ));
    }
    for evidence in &app.conversation.evidence {
        text.push('\n');
        text.push_str(evidence);
    }

    frame.render_widget(
        Paragraph::new(text)
            .wrap(Wrap { trim: true })
            .block(Block::default().title("Actions").borders(Borders::ALL)),
        area,
    );
}

fn draw_composer(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let style = if app.composer.is_command() {
        Style::default().fg(Color::Green)
    } else {
        Style::default()
    };

    let content_height = area.height.saturating_sub(2).max(1) as usize;
    let content_width = area.width.saturating_sub(2).max(1) as usize;
    let (cursor_line, cursor_column) = app.composer.cursor_line_column();
    let first_line = cursor_line.saturating_sub(content_height.saturating_sub(1));
    let first_column = cursor_column.saturating_sub(content_width.saturating_sub(1));
    let lines: Vec<Line<'_>> = app
        .composer
        .buffer
        .split('\n')
        .skip(first_line)
        .take(content_height)
        .map(|line| {
            let visible: String = line
                .graphemes(true)
                .skip(first_column)
                .take(content_width)
                .collect();
            Line::from(Span::styled(visible, style))
        })
        .collect();
    let paste_badge = if app.composer.pasted_since_submit() {
        " · pasted"
    } else {
        ""
    };
    let title = format!(
        "Composer · {} lines · {} chars{} · Ctrl+Enter send · Enter newline",
        app.composer.line_count(),
        app.composer.grapheme_count(),
        paste_badge
    );
    frame.render_widget(
        Paragraph::new(lines).block(Block::default().title(title).borders(Borders::ALL)),
        area,
    );

    let current_line = app
        .composer
        .buffer
        .split('\n')
        .nth(cursor_line)
        .unwrap_or("");
    let visible_prefix: String = current_line
        .graphemes(true)
        .skip(first_column)
        .take(cursor_column.saturating_sub(first_column))
        .collect();
    let x = area
        .x
        .saturating_add(1)
        .saturating_add(UnicodeWidthStr::width(visible_prefix.as_str()) as u16)
        .min(area.right().saturating_sub(2));
    let y = area
        .y
        .saturating_add(1)
        .saturating_add(cursor_line.saturating_sub(first_line) as u16)
        .min(area.bottom().saturating_sub(2));
    frame.set_cursor_position((x, y));
}

// ── Provider Setup mode ──────────────────────────────────────────

fn draw_setup(frame: &mut Frame<'_>, app: &App) {
    let area = frame.area();
    let Some(ref setup) = app.provider_setup else {
        return;
    };

    let content = match setup.screen {
        crate::provider_setup::SetupScreen::Discovery => setup_discovery_text(setup),
        crate::provider_setup::SetupScreen::ImportSource => format!(
            "Import provider configuration\n\nPaste a Python, JavaScript, cURL, dotenv, JSON, YAML, or TOML example.\nThe source is parsed locally and is never executed. Raw secret values are never rendered here.\n\nCaptured: {} bytes · {} lines\n\nCtrl+Enter  Parse and review    Esc  Cancel{}",
            setup.import_source.len(),
            setup.import_source.lines().count().max(1),
            setup.error.as_ref().map_or_else(String::new, |error| format!("\n\nError: {error}"))
        ),
        crate::provider_setup::SetupScreen::Form
        | crate::provider_setup::SetupScreen::ImportReview => setup_form_text(setup),
    };

    frame.render_widget(
        Paragraph::new(content)
            .block(
                Block::default()
                    .title("Connect Provider")
                    .borders(Borders::ALL),
            )
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn setup_discovery_text(setup: &crate::provider_setup::ProviderSetup) -> String {
    let entries = [
        (
            "Ollama",
            "http://127.0.0.1:11434",
            "local · model discovery",
        ),
        (
            "LM Studio",
            "http://127.0.0.1:1234",
            "local · model discovery",
        ),
        ("OpenAI", "https://api.openai.com", "remote"),
        ("OpenAI-compatible", "custom endpoint", "local or remote"),
        (
            "Enterprise gateway",
            "organization policy",
            "remote · advanced",
        ),
        (
            "Import from script/config",
            "Python · JS · cURL · env · JSON · YAML · TOML",
            "parse only",
        ),
    ];
    let cards = entries
        .iter()
        .enumerate()
        .map(|(index, (name, endpoint, detail))| {
            let marker = if index == setup.selected { "▶" } else { " " };
            format!("{marker} {name:<26} {endpoint:<42} {detail}")
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    format!("Connect a model provider\n\n{cards}\n\n↑↓  Select    Enter  Continue    Esc  Cancel")
}

fn setup_form_text(setup: &crate::provider_setup::ProviderSetup) -> String {
    let marker = |field| {
        if setup.active_field == field {
            "▶"
        } else {
            " "
        }
    };
    let provider = setup
        .provider_type
        .map_or("Unknown", |provider| match provider {
            crate::provider_setup::ProviderType::Ollama => "Ollama",
            crate::provider_setup::ProviderType::LmStudio => "LM Studio",
            crate::provider_setup::ProviderType::Openai => "OpenAI",
            crate::provider_setup::ProviderType::OpenaiCompatible => "OpenAI-compatible",
            crate::provider_setup::ProviderType::EnterpriseGateway => "Enterprise gateway",
        });
    let import = setup
        .import_candidate
        .as_ref()
        .map_or_else(String::new, |candidate| {
            let warnings = candidate
                .warnings
                .iter()
                .map(|warning| format!("  ! {}", warning.message))
                .collect::<Vec<_>>()
                .join("\n");
            format!(
                "\nImported candidate · API mode {:?} · {} warning(s)\n{}",
                candidate.api_mode.as_ref().map(|mode| mode.value),
                candidate.warnings.len(),
                warnings
            )
        });
    let diagnostics = if let Some(error) = &setup.error {
        format!("\n\nError: {error}")
    } else if let Some(result) = &setup.test_result {
        format!("\n\n{result}")
    } else {
        String::new()
    };
    format!(
        "Review provider\n\nProvider: {provider}\n\n{} Profile name  [{}]\n{} Base URL      [{}]\n{} API key       [{}]\n{} Model          [{}]\n{} Role           [{}]\n\nDiscovered models: {}\nNetwork: {}{}{}\n\nTab/Shift+Tab  Field    Ctrl+Enter  Save and run real connection test    Esc  Cancel",
        marker(0), setup.profile_name,
        marker(1), setup.base_url,
        marker(2), if setup.api_key.is_empty() { "not set" } else { "••••••••" },
        marker(3), setup.model_id,
        marker(4), setup.role,
        if setup.discovered_models.is_empty() { "none".into() } else { setup.discovered_models.join(", ") },
        if setup.local { "local" } else { "remote" },
        import,
        diagnostics,
    )
}

// ── Skill Browser mode ───────────────────────────────────────────

fn draw_skills(frame: &mut Frame<'_>, app: &App) {
    let area = frame.area();
    let Some(ref browser) = app.skill_browser else {
        return;
    };

    let mut items: Vec<ListItem> = browser
        .skills
        .iter()
        .enumerate()
        .map(|(i, skill)| {
            let marker = if i == browser.selected { "▶" } else { " " };
            let status = if skill.installed { " [installed]" } else { "" };
            ListItem::new(format!(
                "{marker} {name} v{ver}{status}\n  Publisher: {pub} · {sig} · Risk: {risk}\n  Source: {source} · Permissions: {permissions} · Network: {network}",
                name = skill.skill_id,
                ver = skill.version,
                pub = skill.publisher,
                sig = skill.signature,
                risk = skill.risk,
                source = skill.source,
                permissions = skill.permissions,
                network = skill.network,
            ))
        })
        .collect();
    if items.is_empty() {
        items.push(ListItem::new(
            browser
                .error
                .as_deref()
                .unwrap_or("No matching skills were returned."),
        ));
    }

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
            .block(
                Block::default()
                    .title("Diff (Esc closes)")
                    .borders(Borders::ALL),
            )
            .wrap(Wrap { trim: false }),
        area,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_breakpoints_match_the_product_contract() {
        assert_eq!(workspace_layout(160), WorkspaceLayout::Wide);
        assert_eq!(workspace_layout(120), WorkspaceLayout::Wide);
        assert_eq!(workspace_layout(119), WorkspaceLayout::Compact);
        assert_eq!(workspace_layout(80), WorkspaceLayout::Compact);
        assert_eq!(workspace_layout(79), WorkspaceLayout::Narrow);
        assert_eq!(workspace_layout(40), WorkspaceLayout::Narrow);
    }
}
