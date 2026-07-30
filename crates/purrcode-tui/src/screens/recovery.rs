//! Focused recovery surfaces: session choice, lease conflict, uncertain outcome.
//!
//! Every one of these states offers concrete options and states what will *not*
//! happen. None of them starts work as a side effect of being shown.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget, Wrap};
use ratatui::Frame;

use crate::app::App;
use crate::components::hints::Hints;
use crate::components::modal::{Modal, ModalTone};
use crate::design::{Emphasis, Role, Tokens};

#[derive(Clone, Debug)]
pub struct RecoverySurface {
    pub title: &'static str,
    pub headline: String,
    /// What is guaranteed not to happen. Stated explicitly, because a recovery
    /// screen that does not say this leaves the user guessing.
    pub guarantee: &'static str,
    pub options: Vec<String>,
}

/// The recovery surface for a session recovered at startup.
pub fn session_choice(app: &App) -> RecoverySurface {
    let choice = app.session_choice.as_ref();
    let session = choice
        .map(|choice| choice.session_id.as_str())
        .or(app.session_id.as_deref())
        .unwrap_or("the latest session");
    let status = choice.map_or("unknown", |choice| choice.status.as_str());
    let resume = match choice {
        Some(choice)
            if matches!(
                choice.status.as_str(),
                "awaiting_approval" | "awaiting_review"
            ) =>
        {
            "R  Restore the saved decision boundary"
        }
        Some(choice)
            if choice.lease_active && matches!(choice.status.as_str(), "active" | "executing") =>
        {
            "R  Attach to the running session and refresh"
        }
        Some(choice) if !choice.resumable => {
            "This session cannot be resumed safely — start new, or open history read-only"
        }
        _ => "R  Resume this session",
    };
    RecoverySurface {
        title: "Start PurrCode",
        headline: format!(
            "An existing session was found for this repository: {session} ({status})."
        ),
        guarantee: "No task will run until you choose.",
        options: vec![
            resume.to_owned(),
            "N  Start a new session".to_owned(),
            "O  Open history read-only".to_owned(),
        ],
    }
}

pub fn lease_conflict() -> RecoverySurface {
    RecoverySurface {
        title: "Session already active",
        headline: "Another PurrCode client currently owns this session's daemon lease.".to_owned(),
        guarantee: "No new action was started and your draft is safe.",
        options: vec![
            "R  Reconnect and refresh durable state".to_owned(),
            "O  Open read-only".to_owned(),
            "N  Start a new session".to_owned(),
            "D  Technical details".to_owned(),
            "Esc  Keep the draft and return".to_owned(),
        ],
    }
}

/// The surface for a session whose outcome cannot be trusted as a success.
pub fn uncertain_outcome(reason: &str) -> RecoverySurface {
    RecoverySurface {
        title: "Outcome uncertain",
        headline: format!(
            "This session did not finish in a state PurrCode can call successful: {reason}"
        ),
        guarantee: "Nothing has been applied to your working tree.",
        options: vec![
            "D  Review the diff and validation evidence".to_owned(),
            "R  Resume after correcting the reported problem".to_owned(),
            "Ctrl+R  Roll back agent-owned changes".to_owned(),
            "N  Start a new session".to_owned(),
        ],
    }
}

pub fn draw(frame: &mut Frame<'_>, app: &App, surface: &RecoverySurface) {
    let theme = app.theme.clone();
    let tokens = Tokens::new(&theme);
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(3), Constraint::Length(1)])
        .split(frame.area());
    render_surface(frame, rows[0], surface, &tokens);
    Hints {
        entries: surface
            .options
            .iter()
            .filter_map(|option| option.split_whitespace().next().map(str::to_owned))
            .collect(),
    }
    .render(frame, rows[1], &tokens);
}

pub fn render_surface(
    frame: &mut Frame<'_>,
    area: Rect,
    surface: &RecoverySurface,
    tokens: &Tokens<'_>,
) {
    let height = (surface.options.len() as u16).saturating_add(8);
    let inner =
        Modal::new(surface.title, ModalTone::Attention, 78, height).render(frame, area, tokens);
    let mut lines = vec![
        Line::from(Span::styled(
            surface.headline.clone(),
            tokens.style(Role::Primary),
        )),
        Line::from(""),
        Line::from(Span::styled(
            surface.guarantee.to_owned(),
            tokens.styled(Role::Success, Emphasis::Strong),
        )),
        Line::from(""),
    ];
    for option in &surface.options {
        lines.push(Line::from(Span::styled(
            option.clone(),
            tokens.style(Role::Primary),
        )));
    }
    Paragraph::new(lines)
        .wrap(Wrap { trim: false })
        .render(inner, frame.buffer_mut());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::SessionChoice;
    use crate::test_fixtures::{offline_app, test_terminal};

    fn screen(surface: &RecoverySurface, width: u16, height: u16) -> String {
        let app = offline_app();
        let mut terminal = test_terminal(width, height);
        terminal.draw(|frame| draw(frame, &app, surface)).unwrap();
        terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }

    #[test]
    fn session_choice_never_implies_work_has_started() {
        let mut app = offline_app();
        app.session_choice = Some(SessionChoice {
            session_id: "session-123".into(),
            status: "paused".into(),
            resumable: true,
            lease_active: false,
        });
        let surface = session_choice(&app);
        let rendered = screen(&surface, 120, 24);
        assert!(rendered.contains("session-123"));
        assert!(rendered.contains("No task will run until you choose"));
        assert!(rendered.contains("R  Resume this session"));
        assert!(rendered.contains("N  Start a new session"));
        assert!(rendered.contains("O  Open history read-only"));
    }

    #[test]
    fn an_unresumable_session_says_so_instead_of_offering_resume() {
        let mut app = offline_app();
        app.session_choice = Some(SessionChoice {
            session_id: "done-1".into(),
            status: "completed".into(),
            resumable: false,
            lease_active: false,
        });
        let surface = session_choice(&app);
        assert!(surface.options[0].contains("cannot be resumed safely"));
    }

    #[test]
    fn a_decision_boundary_is_restored_rather_than_replayed() {
        let mut app = offline_app();
        app.session_choice = Some(SessionChoice {
            session_id: "waiting-1".into(),
            status: "awaiting_approval".into(),
            resumable: true,
            lease_active: false,
        });
        assert!(session_choice(&app).options[0].contains("Restore the saved decision boundary"));
    }

    #[test]
    fn an_active_lease_offers_attach_not_resume() {
        let mut app = offline_app();
        app.session_choice = Some(SessionChoice {
            session_id: "running-1".into(),
            status: "executing".into(),
            resumable: true,
            lease_active: true,
        });
        assert!(session_choice(&app).options[0].contains("Attach to the running session"));
    }

    #[test]
    fn a_lease_conflict_promises_the_draft_is_safe() {
        let rendered = screen(&lease_conflict(), 120, 24);
        assert!(rendered.contains("Session already active"));
        assert!(rendered.contains("No new action was started and your draft is safe"));
        assert!(rendered.contains("R  Reconnect"));
        assert!(rendered.contains("O  Open read-only"));
    }

    #[test]
    fn an_uncertain_outcome_is_never_worded_as_a_success() {
        let surface = uncertain_outcome("validation failed with 3 failing checks");
        let rendered = screen(&surface, 120, 24);
        assert!(rendered.contains("Outcome uncertain"));
        assert!(rendered.contains("3 failing checks"));
        assert!(!rendered.contains("completed successfully"));
        assert!(rendered.contains("Nothing has been applied to your working tree"));
        assert!(rendered.contains("Roll back agent-owned changes"));
    }

    #[test]
    fn every_recovery_surface_is_usable_at_sixty_columns() {
        let app = offline_app();
        for surface in [
            session_choice(&app),
            lease_conflict(),
            uncertain_outcome("the daemon restarted mid-execution"),
        ] {
            let rendered = screen(&surface, 60, 24);
            assert!(
                rendered.contains(surface.title),
                "{} lost its title at 60 columns",
                surface.title
            );
            assert!(
                surface
                    .options
                    .iter()
                    .any(|option| rendered.contains(option.split_whitespace().next().unwrap())),
                "{} lost its options at 60 columns",
                surface.title
            );
        }
    }

    #[test]
    fn every_surface_states_a_guarantee() {
        let app = offline_app();
        for surface in [
            session_choice(&app),
            lease_conflict(),
            uncertain_outcome("x"),
        ] {
            assert!(!surface.guarantee.is_empty());
            assert!(!surface.options.is_empty());
        }
    }
}
