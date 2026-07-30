//! Validation outcome as a single, unambiguous statement.
//!
//! Passed, failed, unavailable, timed out and uncertain are five different
//! things with five different next actions. None of the last four may read as a
//! pass, so the state word is always printed and the recommended next action is
//! always shown beside it.

use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget, Wrap};
use ratatui::Frame;

use crate::activity::{ValidationState, ValidationSummary as Summary};
use crate::design::{Emphasis, Role, Symbols, Tokens};

#[derive(Clone, Debug)]
pub struct ValidationSummaryView<'summary> {
    pub summary: &'summary Summary,
}

impl<'summary> ValidationSummaryView<'summary> {
    pub fn new(summary: &'summary Summary) -> Self {
        Self { summary }
    }

    const fn role(state: ValidationState) -> Role {
        match state {
            ValidationState::Passed => Role::Success,
            ValidationState::Failed => Role::Danger,
            ValidationState::Running | ValidationState::NotRun => Role::Muted,
            _ => Role::Warning,
        }
    }

    pub fn lines(&self, tokens: &Tokens<'_>) -> Vec<Line<'static>> {
        let symbols = Symbols::new(tokens.unicode());
        let state = self.summary.state;
        let role = Self::role(state);
        let mut lines = vec![Line::from(vec![
            Span::styled(
                if state.needs_attention() {
                    format!("{} ", symbols.attention())
                } else {
                    String::new()
                },
                tokens.style(role),
            ),
            Span::styled(
                "Validation ".to_owned(),
                tokens.styled(Role::Muted, Emphasis::Dim),
            ),
            Span::styled(
                state.label().to_owned(),
                tokens.styled(role, Emphasis::Strong),
            ),
        ])];
        if !self.summary.evidence.is_empty() {
            lines.push(Line::from(Span::styled(
                self.summary.evidence.clone(),
                tokens.style(Role::Primary),
            )));
        }
        // Every state says what to do next, so the user is never left with a
        // status word and no path forward.
        lines.push(Line::from(Span::styled(
            state.next_action().to_owned(),
            tokens.styled(Role::Muted, Emphasis::Dim),
        )));
        if self.summary.records > 1 {
            lines.push(Line::from(Span::styled(
                format!("{} validation record(s)", self.summary.records),
                tokens.styled(Role::Muted, Emphasis::Dim),
            )));
        }
        lines
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
    use crate::test_fixtures::{monochrome_theme, test_theme};

    fn text(summary: &Summary, mono: bool) -> String {
        let theme = if mono {
            monochrome_theme()
        } else {
            test_theme()
        };
        let tokens = Tokens::new(&theme);
        ValidationSummaryView::new(summary)
            .lines(&tokens)
            .iter()
            .flat_map(|line| line.spans.clone())
            .map(|span| span.content.to_string())
            .collect::<Vec<_>>()
            .join("")
    }

    #[test]
    fn a_pass_reads_as_a_pass() {
        let rendered = text(
            &Summary {
                state: ValidationState::Passed,
                evidence: "42 tests passed".into(),
                records: 1,
            },
            false,
        );
        assert!(rendered.contains("Validation passed"));
        assert!(rendered.contains("42 tests passed"));
    }

    #[test]
    fn no_non_passing_state_can_be_read_as_a_pass() {
        for state in [
            ValidationState::Failed,
            ValidationState::Unavailable,
            ValidationState::TimedOut,
            ValidationState::Uncertain,
            ValidationState::NotDetected,
            ValidationState::SkippedByConfiguration,
        ] {
            let rendered = text(
                &Summary {
                    state,
                    evidence: String::new(),
                    records: 1,
                },
                false,
            );
            assert!(
                !rendered.contains("Validation passed"),
                "{state:?} rendered as a pass: {rendered}"
            );
            assert!(
                rendered.contains(state.label()),
                "{state:?} must state its own outcome: {rendered}"
            );
        }
    }

    #[test]
    fn unavailable_is_distinguishable_from_failed() {
        let unavailable = text(
            &Summary {
                state: ValidationState::Unavailable,
                evidence: String::new(),
                records: 1,
            },
            false,
        );
        let failed = text(
            &Summary {
                state: ValidationState::Failed,
                evidence: String::new(),
                records: 1,
            },
            false,
        );
        assert_ne!(unavailable, failed);
        assert!(unavailable.contains("verify manually"));
        assert!(failed.contains("correct or roll back"));
    }

    #[test]
    fn a_timeout_is_distinguishable_from_a_failure() {
        let timed_out = text(
            &Summary {
                state: ValidationState::TimedOut,
                evidence: String::new(),
                records: 1,
            },
            false,
        );
        assert!(timed_out.contains("timed out"));
        assert!(timed_out.contains("did not finish"));
    }

    #[test]
    fn every_state_offers_a_next_action() {
        for state in [
            ValidationState::NotRun,
            ValidationState::Running,
            ValidationState::Passed,
            ValidationState::Failed,
            ValidationState::Unavailable,
            ValidationState::NotDetected,
            ValidationState::SkippedByConfiguration,
            ValidationState::TimedOut,
            ValidationState::Uncertain,
        ] {
            let rendered = text(
                &Summary {
                    state,
                    evidence: String::new(),
                    records: 1,
                },
                false,
            );
            assert!(
                rendered.contains(state.next_action()),
                "{state:?} left the user with no next action"
            );
        }
    }

    #[test]
    fn attention_states_are_marked_without_colour() {
        let rendered = text(
            &Summary {
                state: ValidationState::Failed,
                evidence: String::new(),
                records: 1,
            },
            true,
        );
        assert!(rendered.starts_with('!'), "{rendered}");
        assert!(rendered.is_ascii());
    }

    #[test]
    fn repeated_validation_records_are_visible() {
        let rendered = text(
            &Summary {
                state: ValidationState::Failed,
                evidence: "still failing".into(),
                records: 3,
            },
            false,
        );
        assert!(rendered.contains("3 validation record(s)"));
    }
}
