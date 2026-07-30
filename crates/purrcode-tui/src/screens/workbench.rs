//! The workbench: conversation, activity, contextual inspector, composer.

use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};
use ratatui::Frame;

use crate::activity::WaitingOn;
use crate::app::{App, AppMode};
use crate::components::activity_rail::ActivityRail;
use crate::components::composer::ComposerView;
use crate::components::empty_state::{EmptyReason, EmptyState};
use crate::components::header::Header;
use crate::components::hints::Hints;
use crate::components::inspector::{Inspector, InspectorSubject, StatusDrawer};
use crate::components::message::{MessageBlock, MessageRole};
use crate::components::status_chip::session_chip;
use crate::design::{Emphasis, Role, Tokens};
use crate::layout::adaptive::{InspectorReason, LayoutRequest};
use crate::layout::{Focus, WorkbenchAreas};
use crate::ui_actions::ShortcutContext;

/// Why the inspector is on screen for the current application state.
pub fn inspector_reason(app: &App) -> InspectorReason {
    if app.conversation.pending_action.is_some() {
        return InspectorReason::ApprovalRequired;
    }
    // A pending pull is digest-bound and approval-gated exactly like a
    // write/command decision, so it gets the same forced, focused treatment —
    // never a bare status line for something that authorizes running a new
    // local model.
    if app
        .pending_model_pull
        .as_ref()
        .is_some_and(|pull| !pull.approved)
    {
        return InspectorReason::ApprovalRequired;
    }
    if app.mode == AppMode::LeaseConflict {
        return InspectorReason::RecoveryRequired;
    }
    if app.conversation.validation_needs_attention() {
        return InspectorReason::ValidationAttention;
    }
    if app.inspector_open {
        return InspectorReason::Requested;
    }
    InspectorReason::Hidden
}

pub fn draw(frame: &mut Frame<'_>, app: &App) {
    let theme = app.theme.clone();
    let tokens = Tokens::new(&theme);
    let progress = &app.conversation.progress;
    let areas = WorkbenchAreas::compute(
        frame.area(),
        LayoutRequest {
            composer_lines: app.composer.line_count().min(u16::MAX as usize) as u16,
            activity_lines: progress.activity.len().min(u16::MAX as usize) as u16,
            inspector: inspector_reason(app),
            focused_surface: false,
        },
    );

    draw_header(frame, areas.header, app, &tokens);

    if let Some(area) = areas.conversation {
        draw_conversation(frame, area, app, &tokens);
    }
    if let Some(area) = areas.activity {
        ActivityRail::new(&progress.activity)
            .selected(app.activity_selected)
            .focused(app.focus == Focus::Activity)
            .render(frame, crate::design::Spacing::gutter(area), &tokens);
    }
    if let Some(area) = areas.inspector {
        draw_inspector(frame, area, app, &tokens);
    }
    if let Some(area) = areas.composer {
        draw_composer(frame, area, app, &tokens);
    }
    draw_hints(frame, areas.hints, app, &tokens);
}

fn draw_header(frame: &mut Frame<'_>, area: Rect, app: &App, tokens: &Tokens<'_>) {
    Header {
        repository: &app.workspace.repository_name,
        branch: &app.workspace.branch,
        model: &app.status_bar.model,
        mode: mode_label(app),
        phase: &app.workspace.session_phase,
        local_only: app.status_bar.privacy == "local-only",
    }
    .render(frame, area, tokens);
}

pub fn mode_label(app: &App) -> &'static str {
    match app.conversation.mode {
        purrcode_runtime_core::ConversationMode::Plan => "Plan",
        purrcode_runtime_core::ConversationMode::Build => "Build",
        purrcode_runtime_core::ConversationMode::Review => "Review",
        purrcode_runtime_core::ConversationMode::Ask => "Ask",
    }
}

/// The conversation: user and assistant blocks plus short system notices. No
/// runtime event ever appears here.
fn draw_conversation(frame: &mut Frame<'_>, area: Rect, app: &App, tokens: &Tokens<'_>) {
    let area = crate::design::Spacing::gutter(area);
    if area.width == 0 || area.height == 0 {
        return;
    }
    let mut lines = Vec::new();
    let no_messages =
        app.conversation.messages.is_empty() && app.conversation.streaming_message.is_none();
    let recorded_activity = !app.conversation.progress.activity.is_empty();
    if no_messages && recorded_activity {
        // Work was recorded but no conversation messages were: say so plainly
        // rather than claiming the session did nothing. The activity rail and
        // review screen carry what actually happened.
        lines.push(ratatui::text::Line::from(Span::styled(
            format!(
                "No conversation messages were recorded for this session ({}).",
                app.conversation.progress.session_state.label()
            ),
            tokens.style(Role::Muted),
        )));
        lines.push(ratatui::text::Line::from(Span::styled(
            "The activity below and the review screen show what was done.".to_owned(),
            tokens.styled(Role::Muted, Emphasis::Dim),
        )));
    } else if no_messages {
        let reason = if app.workspace.daemon_health != "connected" {
            EmptyReason::DaemonUnavailable
        } else if !app.has_provider {
            EmptyReason::NoProvider
        } else if app.session_read_only {
            EmptyReason::NoHistory
        } else {
            EmptyReason::Ready
        };
        lines.extend(EmptyState::new(reason).lines(tokens));
    } else {
        for (index, message) in app.conversation.messages.iter().enumerate() {
            lines.extend(
                MessageBlock::new(MessageRole::parse(&message.role), &message.content)
                    .selected(app.conversation.selected_card == Some(index))
                    .lines(tokens, area.width),
            );
        }
        if let Some(streaming) = &app.conversation.streaming_message {
            lines.extend(
                MessageBlock::new(MessageRole::Assistant, &streaming.content)
                    .streaming(true)
                    .lines(tokens, area.width),
            );
        }
    }
    // A concise system notice belongs in the conversation: command results and
    // refusals directly affect what the user just did, and burying them in a
    // status panel is how feedback goes unnoticed.
    if !app.message_bar.is_empty() {
        lines.extend(
            MessageBlock::new(MessageRole::System, &app.message_bar).lines(tokens, area.width),
        );
    }

    // Auto-follow keeps the newest block anchored to the bottom of the region.
    let height = area.height as usize;
    let visible = if lines.len() > height && app.conversation.auto_follow {
        lines[lines.len() - height..].to_vec()
    } else if lines.len() > height {
        let start = app
            .conversation
            .scroll
            .min(lines.len().saturating_sub(height));
        lines[start..start + height].to_vec()
    } else {
        lines
    };
    Paragraph::new(visible).render(area, frame.buffer_mut());
}

fn draw_inspector(frame: &mut Frame<'_>, area: Rect, app: &App, tokens: &Tokens<'_>) {
    let progress = &app.conversation.progress;
    let subject = if let Some(request) = &app.approval {
        InspectorSubject::Decision(request)
    } else if let Some(pull) = &app.pending_model_pull {
        InspectorSubject::ModelPull {
            model: &pull.model,
            action_id: &pull.action_id,
            action_digest: &pull.action_digest,
            approved: pull.approved,
        }
    } else if app.mode == AppMode::LeaseConflict {
        InspectorSubject::Recovery {
            reason: "another PurrCode client currently owns this session's daemon lease",
            options: vec![
                "R Reconnect and refresh durable state".into(),
                "O Open read-only".into(),
                "N Start a new session".into(),
            ],
        }
    } else if let Some(index) = app.activity_selected {
        match progress.activity.get(index) {
            Some(entry) => InspectorSubject::Activity {
                entry,
                detail: app.activity_detail(index),
            },
            None => InspectorSubject::Unavailable("That activity step"),
        }
    } else if progress.validation.state.needs_attention() {
        InspectorSubject::Validation(&progress.validation)
    } else {
        InspectorSubject::StatusDrawer(StatusDrawer {
            session_id: app.session_id.as_deref().unwrap_or("not started"),
            sandbox_backend: &app.workspace.sandbox,
            daemon_version: env!("CARGO_PKG_VERSION"),
            daemon_health: &app.workspace.daemon_health,
            worktree: &app.workspace.source_state,
            provider_endpoint: if app.has_provider {
                "configured"
            } else {
                "none"
            },
            privacy: &app.status_bar.privacy,
            resources: &app.status_bar.context_info,
        })
    };
    Inspector::new(subject)
        .focused(app.focus == Focus::Inspector)
        .render(frame, area, tokens);
}

fn draw_composer(frame: &mut Frame<'_>, area: Rect, app: &App, tokens: &Tokens<'_>) {
    let (cursor_line, cursor_column) = app.composer.cursor_line_column();
    let view = ComposerView {
        unicode: tokens.unicode(),
        mode: mode_label(app),
        buffer: &app.composer.buffer,
        cursor_line,
        cursor_column,
        line_count: app.composer.line_count(),
        grapheme_count: app.composer.grapheme_count(),
        pasted: app.composer.pasted_since_submit(),
        is_command: app.composer.is_command(),
        focused: app.focus == Focus::Composer,
        read_only: app.session_read_only,
    };
    if let Some(position) = view.render(frame, area, tokens) {
        frame.set_cursor_position(position);
    }
}

/// The single hint line, plus the one status line that answers "what is
/// PurrCode doing?" when something is in flight.
fn draw_hints(frame: &mut Frame<'_>, area: Rect, app: &App, tokens: &Tokens<'_>) {
    let progress = &app.conversation.progress;
    let context = app.ui_context();
    if progress.waiting_on != WaitingOn::Nothing || progress.session_state.needs_attention() {
        let mut spans = vec![Span::styled(
            format!("{}  ", progress.waiting_on.describe()),
            tokens.styled(Role::Accent, Emphasis::Normal),
        )];
        if progress.session_state.needs_attention() {
            spans.push(session_chip(progress.session_state).span(tokens));
            spans.push(Span::raw("  "));
        }
        let hints = Hints::for_context(ShortcutContext::Workbench, &context);
        let used: usize = spans
            .iter()
            .map(|span| unicode_width::UnicodeWidthStr::width(span.content.as_ref()))
            .sum();
        spans.extend(
            hints
                .line(tokens, area.width.saturating_sub(used as u16))
                .spans,
        );
        Paragraph::new(Line::from(spans)).render(area, frame.buffer_mut());
        return;
    }
    let hints = if app.session_read_only {
        Hints::for_context(ShortcutContext::History, &context).with(&["Esc Close history"])
    } else {
        Hints {
            entries: vec!["Ctrl+G Send".into(), "Enter New line".into()],
        }
        .with(&["Tab Move focus"])
    };
    let mut merged = hints;
    for entry in Hints::for_context(ShortcutContext::Workbench, &context).entries {
        // Registry hints can restate a shell hint (Enter is both "new line" and a
        // registered action); showing it twice wastes the one line we have.
        if !merged.entries.contains(&entry) {
            merged.entries.push(entry);
        }
    }
    merged.render(frame, area, tokens);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_fixtures::{offline_app, test_terminal};
    use serde_json::json;

    fn screen(app: &App, width: u16, height: u16) -> String {
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
    fn an_empty_workbench_teaches_the_next_action() {
        let mut app = offline_app();
        app.workspace.daemon_health = "connected".into();
        app.has_provider = true;
        let rendered = screen(&app, 120, 30);
        assert!(rendered.contains("Ready for a task"));
        assert!(rendered.contains("Ctrl+G"));
    }

    #[test]
    fn a_missing_provider_names_the_command_to_run() {
        let mut app = offline_app();
        app.workspace.daemon_health = "connected".into();
        app.has_provider = false;
        let rendered = screen(&app, 120, 30);
        assert!(rendered.contains("Connect a model provider"));
        assert!(rendered.contains("/connect"));
    }

    #[test]
    fn conversation_blocks_carry_no_per_line_role_prefix() {
        let mut app = offline_app();
        app.has_provider = true;
        app.workspace.daemon_health = "connected".into();
        app.conversation.add_user_message("first\nsecond\nthird");
        let rendered = screen(&app, 120, 30);
        assert!(rendered.contains("you"));
        assert!(
            !rendered.contains("You:"),
            "no line may be prefixed with a role: {rendered}"
        );
        assert!(!rendered.contains("PurrCode:"));
    }

    #[test]
    fn runtime_events_stay_out_of_the_conversation() {
        let mut app = offline_app();
        app.has_provider = true;
        app.workspace.daemon_health = "connected".into();
        app.conversation.add_user_message("do the thing");
        app.conversation.reconcile(
            None,
            vec![
                json!({"event":"context_indexed","data":{"files":9,"symbols":30,"sensitive_files":0}}),
                json!({"event":"plan_created","data":{"steps":["a","b"]}}),
            ],
        );
        // Reconcile replaces messages only when messages are supplied; re-add so
        // the conversation region has content of its own.
        app.conversation.add_user_message("do the thing");
        let rendered = screen(&app, 120, 30);
        assert!(rendered.contains("Context prepared"), "activity is present");
        assert!(
            !rendered.contains("context_indexed"),
            "no raw event name may reach the screen: {rendered}"
        );
    }

    #[test]
    fn a_pending_approval_forces_the_inspector_open() {
        let mut app = offline_app();
        app.has_provider = true;
        app.workspace.daemon_health = "connected".into();
        app.conversation.pending_action = Some(json!({"type":"write_file","path":"x.rs"}));
        assert_eq!(inspector_reason(&app), InspectorReason::ApprovalRequired);
    }

    #[test]
    fn failing_validation_forces_the_inspector_open() {
        let mut app = offline_app();
        app.conversation.reconcile(
            None,
            vec![
                json!({"event":"validation_recorded","data":{"status":"failed","evidence":"2 failed"}}),
            ],
        );
        assert_eq!(inspector_reason(&app), InspectorReason::ValidationAttention);
        let rendered = screen(&app, 160, 40);
        assert!(rendered.contains("Validation failed"), "{rendered}");
        assert!(
            rendered.contains("2 failed"),
            "the evidence must be visible"
        );
    }

    #[test]
    fn the_inspector_stays_hidden_when_nothing_needs_it() {
        let app = offline_app();
        assert_eq!(inspector_reason(&app), InspectorReason::Hidden);
    }

    #[test]
    fn the_header_never_shows_the_full_session_identifier() {
        let mut app = offline_app();
        app.session_id = Some("0e3f9c22-1111-2222-3333-444455556666".into());
        app.has_provider = true;
        app.workspace.daemon_health = "connected".into();
        let rendered = screen(&app, 160, 40);
        assert!(
            !rendered.contains("0e3f9c22-1111-2222-3333-444455556666"),
            "the full session id belongs in the status drawer"
        );
    }

    #[test]
    fn the_status_drawer_exposes_the_session_identifier_when_opened() {
        let mut app = offline_app();
        app.session_id = Some("0e3f9c22-1111-2222-3333-444455556666".into());
        app.inspector_open = true;
        app.has_provider = true;
        app.workspace.daemon_health = "connected".into();
        let rendered = screen(&app, 160, 40);
        assert!(rendered.contains("0e3f9c22-1111-2222-3333-444455556666"));
    }

    #[test]
    fn the_workbench_renders_at_every_supported_size() {
        let mut app = offline_app();
        app.has_provider = true;
        app.workspace.daemon_health = "connected".into();
        app.conversation.add_user_message("hello");
        for (width, height) in [(60, 24), (80, 24), (120, 30), (160, 40)] {
            let rendered = screen(&app, width, height);
            assert!(
                rendered.contains("PurrCode"),
                "the header disappeared at {width}x{height}"
            );
        }
    }

    #[test]
    fn read_only_history_says_so_in_the_composer() {
        let mut app = offline_app();
        app.session_read_only = true;
        app.has_provider = true;
        app.workspace.daemon_health = "connected".into();
        let rendered = screen(&app, 120, 30);
        assert!(rendered.contains("read-only"));
    }

    #[test]
    fn a_pending_model_pull_forces_the_inspector_open_with_its_exact_identity() {
        let mut app = offline_app();
        app.has_provider = true;
        app.workspace.daemon_health = "connected".into();
        app.pending_model_pull = Some(crate::app::PendingModelPull {
            action_id: "pull-action-123".into(),
            action_digest: "sha256:deadbeef".into(),
            session_id: "pull-session".into(),
            model: "coder:7b".into(),
            approved: false,
        });
        assert_eq!(inspector_reason(&app), InspectorReason::ApprovalRequired);
        let rendered = screen(&app, 120, 30);
        assert!(rendered.contains("coder:7b"), "{rendered}");
        assert!(rendered.contains("pull-action-123"), "{rendered}");
        assert!(rendered.contains("sha256:deadbeef"), "{rendered}");
        assert!(
            rendered.contains("awaiting explicit approval"),
            "{rendered}"
        );
    }

    #[test]
    fn an_approved_model_pull_no_longer_forces_the_inspector_open() {
        let mut app = offline_app();
        app.pending_model_pull = Some(crate::app::PendingModelPull {
            action_id: "pull-action-123".into(),
            action_digest: "sha256:deadbeef".into(),
            session_id: "pull-session".into(),
            model: "coder:7b".into(),
            approved: true,
        });
        assert_ne!(inspector_reason(&app), InspectorReason::ApprovalRequired);
    }

    #[test]
    fn waiting_state_is_reported_on_the_hint_line() {
        let mut app = offline_app();
        app.conversation.reconcile(
            None,
            vec![json!({"event":"model_request_started","data":{"role":"coder"}})],
        );
        let rendered = screen(&app, 120, 30);
        assert!(rendered.contains("Waiting for the model"), "{rendered}");
    }
}
