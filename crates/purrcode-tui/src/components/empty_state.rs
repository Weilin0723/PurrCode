//! The empty workbench.
//!
//! An empty screen is a teaching opportunity, not a blank panel. It names the
//! single most useful next action for the current state, so a new user can start
//! without consulting documentation or source code.

use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget, Wrap};
use ratatui::Frame;

use crate::design::{Emphasis, Role, Tokens};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EmptyReason {
    /// Everything is configured; the user just has not typed anything.
    Ready,
    /// No provider is configured yet.
    NoProvider,
    /// The daemon is unreachable.
    DaemonUnavailable,
    /// Reading a completed session with nothing recorded.
    NoHistory,
}

#[derive(Clone, Copy, Debug)]
pub struct EmptyState {
    pub reason: EmptyReason,
}

impl EmptyState {
    pub const fn new(reason: EmptyReason) -> Self {
        Self { reason }
    }

    pub const fn headline(&self) -> &'static str {
        match self.reason {
            EmptyReason::Ready => "Ready for a task",
            EmptyReason::NoProvider => "Connect a model provider to begin",
            EmptyReason::DaemonUnavailable => "PurrCode cannot reach its local daemon",
            EmptyReason::NoHistory => "This session recorded no activity",
        }
    }

    pub const fn body(&self) -> &'static str {
        match self.reason {
            EmptyReason::Ready => {
                "Describe what you want changed, ask a question about this repository, or paste code or logs."
            }
            EmptyReason::NoProvider => {
                "PurrCode needs a local or remote model before it can work. Local providers are discovered automatically."
            }
            EmptyReason::DaemonUnavailable => {
                "No work can start until the daemon responds. Your draft and durable sessions are safe."
            }
            EmptyReason::NoHistory => {
                "Nothing was executed in this session. Opening history never starts work."
            }
        }
    }

    /// The single next action, phrased as an instruction.
    pub const fn next_action(&self) -> &'static str {
        match self.reason {
            EmptyReason::Ready => "Type your task, then press Ctrl+G to send",
            EmptyReason::NoProvider => "Run /connect to set up a provider",
            EmptyReason::DaemonUnavailable => {
                "Start the daemon with `purrcode serve`, then reconnect"
            }
            EmptyReason::NoHistory => "Press N to start a new session",
        }
    }

    pub fn lines(&self, tokens: &Tokens<'_>) -> Vec<Line<'static>> {
        vec![
            Line::from(Span::styled(
                self.headline().to_owned(),
                tokens.styled(Role::Accent, Emphasis::Strong),
            )),
            Line::from(""),
            Line::from(Span::styled(
                self.body().to_owned(),
                tokens.style(Role::Primary),
            )),
            Line::from(""),
            Line::from(Span::styled(
                self.next_action().to_owned(),
                tokens.styled(Role::Muted, Emphasis::Dim),
            )),
        ]
    }

    pub fn render(&self, frame: &mut Frame<'_>, area: Rect, tokens: &Tokens<'_>) {
        Paragraph::new(self.lines(tokens))
            .wrap(Wrap { trim: false })
            .render(area, frame.buffer_mut());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_fixtures::test_theme;

    const EVERY_REASON: &[EmptyReason] = &[
        EmptyReason::Ready,
        EmptyReason::NoProvider,
        EmptyReason::DaemonUnavailable,
        EmptyReason::NoHistory,
    ];

    #[test]
    fn every_empty_state_names_one_next_action() {
        for reason in EVERY_REASON {
            let state = EmptyState::new(*reason);
            assert!(!state.headline().is_empty());
            assert!(!state.body().is_empty());
            assert!(
                !state.next_action().is_empty(),
                "{reason:?} left the user with nothing to do"
            );
        }
    }

    #[test]
    fn each_state_has_a_distinct_headline_and_action() {
        let headlines: std::collections::BTreeSet<&str> = EVERY_REASON
            .iter()
            .map(|reason| EmptyState::new(*reason).headline())
            .collect();
        assert_eq!(headlines.len(), EVERY_REASON.len());
        let actions: std::collections::BTreeSet<&str> = EVERY_REASON
            .iter()
            .map(|reason| EmptyState::new(*reason).next_action())
            .collect();
        assert_eq!(actions.len(), EVERY_REASON.len());
    }

    #[test]
    fn the_provider_state_names_the_command_a_new_user_needs() {
        assert!(EmptyState::new(EmptyReason::NoProvider)
            .next_action()
            .contains("/connect"));
    }

    #[test]
    fn the_daemon_state_reassures_about_durable_state() {
        let body = EmptyState::new(EmptyReason::DaemonUnavailable).body();
        assert!(body.contains("draft"), "{body}");
        assert!(body.contains("safe"), "{body}");
    }

    #[test]
    fn history_states_promise_not_to_start_work() {
        assert!(EmptyState::new(EmptyReason::NoHistory)
            .body()
            .contains("never starts work"));
    }

    #[test]
    fn empty_states_render_at_every_supported_width() {
        let theme = test_theme();
        let tokens = Tokens::new(&theme);
        for reason in EVERY_REASON {
            let lines = EmptyState::new(*reason).lines(&tokens);
            assert!(!lines.is_empty());
        }
    }
}
