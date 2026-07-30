//! Ratatui rendering for all TUI modes.

use crate::app::{App, AppMode};
use crate::theme::Theme;
use crate::trace_inspector::TraceInspector;
use ratatui::layout::Rect;
use ratatui::style::Color;
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph, Wrap};
use ratatui::Frame;

pub fn draw(frame: &mut Frame<'_>, app: &App) {
    match app.mode {
        AppMode::SecretReview => draw_secret_review(frame, app),
        AppMode::ProviderSetup => draw_setup(frame, app),
        AppMode::SkillBrowse => draw_skills(frame, app),
        AppMode::Review => crate::screens::review::draw(frame, app),
        AppMode::Approval => crate::screens::approval::draw(frame, app),
        AppMode::Help => draw_help(frame, app),
        AppMode::ModelBrowse => draw_model_browser(frame, app),
        AppMode::LeaseConflict => {
            crate::screens::recovery::draw(frame, app, &crate::screens::recovery::lease_conflict());
        }
        AppMode::SessionChoice => {
            let surface = crate::screens::recovery::session_choice(app);
            crate::screens::recovery::draw(frame, app, &surface);
        }
        AppMode::Conversation => crate::screens::workbench::draw(frame, app),
    }
    apply_canvas_background(frame, &app.theme);
    render_trace_inspector_modal(frame, app);
}

fn draw_model_browser(frame: &mut Frame<'_>, app: &App) {
    let mut text = String::from("Select a model\n\n");
    for (index, model) in app.model_choices.iter().enumerate() {
        let marker = if index == app.model_selected {
            ">"
        } else {
            " "
        };
        let active = if model == &app.status_bar.model {
            "  (current)"
        } else {
            ""
        };
        text.push_str(&format!("{marker} {model}{active}\n"));
    }
    if app.model_choices.is_empty() {
        text.push_str("No configured models were reported.\n");
    }
    text.push_str("\nUp/Down select · Enter switch · Esc cancel");
    frame.render_widget(
        Paragraph::new(text)
            .wrap(Wrap { trim: false })
            .block(Block::default().title("Switch model").borders(Borders::ALL)),
        centered(frame.area(), 80, 24),
    );
}

/// Paint the canvas background using the palette's `canvas` color so the
/// "Calm terminal workbench" stays consistent across dark/light/monochrome
/// themes. Cells with no explicit foreground get the palette's `text_primary`
/// color, which preserves legibility regardless of the terminal's profile.
fn apply_canvas_background(frame: &mut Frame<'_>, theme: &Theme) {
    let bg = theme.palette.canvas;
    let fg = theme.palette.text_primary;
    for cell in frame.buffer_mut().content.iter_mut() {
        cell.set_bg(bg);
        if !theme.colors_enabled || cell.fg == Color::Reset || cell.fg == Color::Black {
            cell.set_fg(fg);
        }
    }
}

fn render_trace_inspector_modal(frame: &mut Frame<'_>, app: &App) {
    if app.mode != AppMode::Conversation {
        return;
    }
    let inspector = TraceInspector {
        visible: app.trace_inspector_visible,
        event_index: app.trace_event_index,
        event_type: app.trace_event_type.clone(),
        event_detail: app.trace_event_detail.clone(),
        total_events: app.trace_total_events,
    };
    inspector.render(frame, frame.area(), &app.theme);
}

/// The command palette, generated from the action registry.
fn draw_help(frame: &mut Frame<'_>, app: &App) {
    let theme = app.theme.clone();
    let tokens = crate::design::Tokens::new(&theme);
    let actions = crate::command_palette::filtered_actions(&app.palette_query);
    crate::components::palette::CommandPaletteView::new(
        &app.palette_query,
        &actions,
        app.palette_selected,
        app.ui_context(),
    )
    .render(frame, frame.area(), &tokens);
}

fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    Rect::new(
        area.x + area.width.saturating_sub(width) / 2,
        area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    )
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

// ── Conversation mode ────────────────────────────────────────────

// ── Provider Setup mode ──────────────────────────────────────────

fn draw_setup(frame: &mut Frame<'_>, app: &App) {
    let area = frame.area();
    let Some(ref setup) = app.provider_setup else {
        return;
    };

    let content = match setup.screen {
        crate::provider_setup::SetupScreen::Discovery => setup_discovery_text(setup),
        crate::provider_setup::SetupScreen::ImportSource => format!(
            "Import provider configuration\n\nPaste a Python, JavaScript, cURL, dotenv, JSON, YAML, or TOML example.\nThe source is parsed locally and is never executed. Raw secret values are never rendered here.\n\nCaptured: {} / {} bytes · {} lines\n\nCtrl+G  Parse and review    M  Enter Base URL, API key, and model manually    Esc  Cancel{}",
            setup.import_source.len(),
            purrcode_provider_import::DEFAULT_MAX_INPUT_BYTES,
            setup.import_source.lines().count().max(1),
            setup.error.as_ref().map_or_else(String::new, |error| format!("\n\nError: {error}"))
        ),
        crate::provider_setup::SetupScreen::ImportAuthChoice => {
            setup_import_auth_choice_text(setup)
        }
        crate::provider_setup::SetupScreen::ImportKeychainConfirm => format!(
            "Confirm OS keychain storage\n\nPurrCode detected one static credential. Its value remains in a zeroizing transient buffer and is not shown.\n\nStore it under credential name `{}`?\nOnly `keychain:{}` will be written to provider configuration.\n\nY  Confirm storage    N/Esc  Back",
            setup.profile_name, setup.profile_name
        ),
        crate::provider_setup::SetupScreen::ImportEnvironment => format!(
            "Use an environment-variable reference\n\nVariable: [{}]\n\nPurrCode will save only this variable name. Ensure the variable contains the credential before the real connection test.\n\nCtrl+G/Enter  Confirm reference    Esc  Back{}",
            setup.environment_reference,
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

fn setup_import_auth_choice_text(setup: &crate::provider_setup::ProviderSetup) -> String {
    let choices = [
        (
            "Store in OS keychain",
            "recommended · saves only a keychain reference",
        ),
        (
            "Use existing environment variable",
            "reference only · does not create or set it",
        ),
        ("Discard and enter another", "zeroizes the detected value"),
    ];
    let cards = choices
        .iter()
        .enumerate()
        .map(|(index, (label, detail))| {
            let marker = if index == setup.import_auth_choice {
                "▶"
            } else {
                " "
            };
            format!("{marker} {label:<30} {detail}")
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    format!(
        "Resolve detected authentication\n\nA static credential was extracted before the source entered chat history.\nIts value is hidden and held only in zeroizing memory.\n\n{cards}\n\n↑↓  Select    Enter/Ctrl+G  Continue    K/E/D  Shortcut    Esc  Cancel"
    )
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
        "Review provider\n\nProvider: {provider}\n\n{} Profile name  [{}]\n{} Base URL *    [{}]\n{} API key/auth  [{}]\n{} Model ID *    [{}]\n{} Role           [{}]\n\nDiscovered models: {}\nNetwork: {}{}{}\n\nType to edit · Ctrl+U clear field · ↑↓ choose discovered model\nTab/Shift+Tab field · Ctrl+G save and run real connection test · Esc cancel",
        marker(0), setup.profile_name,
        marker(1), setup.base_url,
        marker(2), setup.auth_status(),
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
        let theme = app.theme.clone();
        let tokens = crate::design::Tokens::new(&theme);
        crate::components::inspector::Inspector::new(
            crate::components::inspector::InspectorSubject::Unavailable("The skill registry"),
        )
        .render(frame, area, &tokens);
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

    let title = if browser.pending_search_action.is_some() {
        "Skills (Esc to close, a to approve the pending search)"
    } else {
        "Skills (Esc to close)"
    };
    frame.render_widget(
        List::new(items).block(Block::default().title(title).borders(Borders::ALL)),
        area,
    );
}

// ── Diff view mode ───────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_fixtures::{monochrome_theme, offline_app, test_terminal};

    fn snapshot(app: &App, width: u16, height: u16) -> String {
        let mut terminal = test_terminal(width, height);
        terminal.draw(|frame| draw(frame, app)).unwrap();
        terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }

    #[test]
    fn every_mode_renders_at_every_supported_size() {
        let modes = [
            AppMode::Conversation,
            AppMode::Help,
            AppMode::ModelBrowse,
            AppMode::Review,
            AppMode::Approval,
            AppMode::LeaseConflict,
            AppMode::SessionChoice,
            AppMode::SkillBrowse,
        ];
        for mode in modes {
            for (width, height) in [(60, 24), (80, 24), (120, 30), (160, 40)] {
                let mut app = offline_app();
                app.mode = mode;
                // Rendering must not panic in any mode at any supported size.
                let rendered = snapshot(&app, width, height);
                assert!(
                    !rendered.trim().is_empty(),
                    "{mode:?} rendered an empty frame at {width}x{height}"
                );
            }
        }
    }

    #[test]
    fn the_whole_ui_uses_the_palette_canvas_with_legible_text() {
        let app = offline_app();
        let mut terminal = test_terminal(120, 30);
        terminal.draw(|frame| draw(frame, &app)).unwrap();
        let canvas = app.theme.palette.canvas;
        for cell in &terminal.backend().buffer().content {
            assert_eq!(cell.bg, canvas, "every cell must use the palette canvas");
            assert_ne!(cell.fg, Color::Reset);
        }
    }

    #[test]
    fn the_no_colour_fallback_keeps_one_high_contrast_foreground() {
        let mut app = offline_app();
        app.theme = monochrome_theme();
        let mut terminal = test_terminal(120, 30);
        terminal.draw(|frame| draw(frame, &app)).unwrap();
        let expected = app.theme.palette.text_primary;
        for cell in &terminal.backend().buffer().content {
            assert_eq!(cell.bg, app.theme.palette.canvas);
            assert_eq!(cell.fg, expected);
        }
    }

    #[test]
    fn rendering_is_deterministic_at_every_supported_width() {
        for width in [60, 80, 120, 160] {
            let render = || {
                let app = offline_app();
                let mut terminal = test_terminal(width, 24);
                terminal.draw(|frame| draw(frame, &app)).unwrap();
                terminal
                    .backend()
                    .buffer()
                    .content
                    .iter()
                    .map(|cell| format!("{}|{:?}|{:?}", cell.symbol(), cell.fg, cell.bg))
                    .collect::<Vec<_>>()
            };
            assert_eq!(render(), render(), "frame changed at width {width}");
        }
    }

    #[test]
    fn the_palette_screen_is_generated_from_the_registry() {
        let mut app = offline_app();
        app.mode = AppMode::Help;
        app.workspace.daemon_health = "connected".into();
        let rendered = snapshot(&app, 120, 40);
        assert!(rendered.contains("Commands"));
        assert!(rendered.contains("TASK"), "categories must be grouped");
        assert!(rendered.contains("SESSION"), "categories must be grouped");
        assert!(
            rendered.contains("unavailable"),
            "unavailable actions must explain themselves"
        );
    }

    /// A secret must never survive into anything PurrCode emits: conversation
    /// history, activity, the inspector, or the restored draft. The composer
    /// legitimately echoes what the user is typing right now, which is why this
    /// exercises the paths that outlive the keystroke instead.
    #[test]
    fn no_emitted_surface_renders_a_secret_like_value() {
        const SECRET: &str = "sk-test-abcdefghijklmnopqrstuvwxyz012345";
        let mut app = offline_app();
        app.conversation.add_user_message(
            &purrcode_provider_import::redact_source(&format!("OPENAI_API_KEY={SECRET}"))
                .expect("redaction must succeed")
                .display
                .to_string(),
        );
        app.conversation.reconcile(
            None,
            vec![serde_json::json!({
                "event": "provider_configured",
                "data": {"credential_reference": "keychain:default"}
            })],
        );
        for mode in [AppMode::Conversation, AppMode::Help, AppMode::Approval] {
            app.mode = mode;
            let rendered = snapshot(&app, 160, 40);
            assert!(
                !rendered.contains(SECRET),
                "{mode:?} rendered a secret-like value"
            );
        }
    }
}
