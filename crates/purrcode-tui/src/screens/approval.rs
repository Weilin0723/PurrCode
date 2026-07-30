//! The focused approval screen.
//!
//! Approval owns the whole frame. It is not a panel sharing space with a
//! composer and a timeline, because a decision that grants execution authority
//! must not compete for attention — and because at 60 columns a shared layout
//! cannot show the paths, scope and digest the user needs.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::Frame;

use crate::app::App;
use crate::components::approval_card::ApprovalCard;
use crate::components::header::Header;
use crate::components::hints::Hints;
use crate::design::Tokens;

pub fn draw(frame: &mut Frame<'_>, app: &App) {
    let theme = app.theme.clone();
    let tokens = Tokens::new(&theme);
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(3),
            Constraint::Length(1),
        ])
        .split(frame.area());

    Header {
        repository: &app.workspace.repository_name,
        branch: &app.workspace.branch,
        model: &app.status_bar.model,
        mode: super::workbench::mode_label(app),
        phase: "awaiting approval",
        local_only: app.status_bar.privacy == "local-only",
    }
    .render(frame, rows[0], &tokens);

    match &app.approval {
        Some(request) => {
            let card = ApprovalCard::new(request);
            card.render(frame, rows[1], &tokens);
            Hints {
                entries: card
                    .decision_keys()
                    .into_iter()
                    .map(str::to_owned)
                    .collect(),
            }
            .render(frame, rows[2], &tokens);
        }
        None => {
            draw_no_pending_action(frame, rows[1], &tokens);
            Hints {
                entries: vec!["Esc Return to the workbench".to_owned()],
            }
            .render(frame, rows[2], &tokens);
        }
    }
}

/// A bare `approve` with nothing pending must say so plainly rather than
/// implying something was authorized.
fn draw_no_pending_action(frame: &mut Frame<'_>, area: Rect, tokens: &Tokens<'_>) {
    use crate::components::empty_state::{EmptyReason, EmptyState};
    use crate::components::modal::{Modal, ModalTone};
    use ratatui::text::{Line, Span};
    use ratatui::widgets::{Paragraph, Widget, Wrap};

    let _ = EmptyState::new(EmptyReason::Ready);
    let inner =
        Modal::new("Nothing to approve", ModalTone::Neutral, 70, 9).render(frame, area, tokens);
    Paragraph::new(vec![
        Line::from(Span::styled(
            "No action is awaiting approval.".to_owned(),
            tokens.style(crate::design::Role::Primary),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "Nothing was authorized and nothing will run. PurrCode asks for approval only when an exact action is pending."
                .to_owned(),
            tokens.styled(crate::design::Role::Muted, crate::design::Emphasis::Dim),
        )),
    ])
    .wrap(Wrap { trim: false })
    .render(inner, frame.buffer_mut());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::approval::ApprovalRequest;
    use crate::test_fixtures::{offline_app, test_terminal};
    use purrcode_runtime_core::{ActionConstraints, ProposedAction, WriteFileAction};
    use serde_json::json;
    use std::path::PathBuf;

    fn pending_app() -> App {
        let constraints = ActionConstraints {
            working_directory: PathBuf::from("/repo"),
            network: false,
            timeout_seconds: 120,
            maximum_output_bytes: 65_536,
            allowed_write_globs: vec!["src/**".into()],
            maximum_changed_files: 1,
        };
        let action = ProposedAction::WriteFile(WriteFileAction {
            path: PathBuf::from("src/runtime.rs"),
            content: "fn main() {}".into(),
            expected_digest: None,
        });
        let events = vec![
            json!({"event":"action_proposed","data":{"action_id":"action-xyz","action":action}}),
            json!({"event":"judgment_recorded","data":{
                "action_id":"action-xyz",
                "decision":{"decision":"require_approval","details":{
                    "reason":"writes a file outside the declared plan scope",
                    "constraints":constraints,
                }}
            }}),
        ];
        let mut app = offline_app();
        app.workspace.daemon_health = "connected".into();
        app.has_provider = true;
        app.conversation.reconcile(None, events.clone());
        app.approval = ApprovalRequest::from_events(&events);
        app
    }

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
    fn the_approval_screen_states_everything_needed_to_decide() {
        let app = pending_app();
        let rendered = screen(&app, 120, 30);
        for expected in [
            "Approval required",
            "Write a file",
            "src/runtime.rs",
            "writes a file outside the declared plan scope",
            "modifies files",
            "no network",
            "at most 1 changed file(s)",
            "action-xyz",
            "A Approve exact action",
            "R Reject",
        ] {
            assert!(rendered.contains(expected), "missing {expected:?}");
        }
    }

    #[test]
    fn the_approval_screen_is_complete_at_sixty_columns() {
        let app = pending_app();
        let rendered = screen(&app, 60, 24);
        for expected in ["Approval required", "src/runtime.rs", "Digest", "A Approve"] {
            assert!(
                rendered.contains(expected),
                "missing {expected:?} at 60 columns: {rendered}"
            );
        }
    }

    #[test]
    fn a_bare_approve_with_nothing_pending_authorizes_nothing() {
        let mut app = pending_app();
        app.approval = None;
        let rendered = screen(&app, 120, 24);
        assert!(rendered.contains("Nothing to approve"));
        assert!(rendered.contains("No action is awaiting approval"));
        assert!(rendered.contains("Nothing was authorized"));
        assert!(
            !rendered.contains("A Approve"),
            "there must be no approve affordance with nothing pending"
        );
    }

    #[test]
    fn a_stale_surface_refuses_approval_and_says_why() {
        let mut app = pending_app();
        app.approval = app
            .approval
            .take()
            .map(|request| request.checked_against(Some("a-newer-action")));
        let rendered = screen(&app, 120, 30);
        assert!(rendered.contains("Approval blocked"));
        assert!(rendered.contains("no longer the action"));
        assert!(!rendered.contains("A Approve exact action"));
        assert!(rendered.contains("Refresh"));
    }

    #[test]
    fn privacy_state_remains_visible_on_the_decision_surface() {
        let app = pending_app();
        let rendered = screen(&app, 120, 30);
        assert!(rendered.contains("local-only"));
    }
}
