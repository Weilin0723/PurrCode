//! The contextual hint line.
//!
//! One line below the composer, generated from the action registry for the
//! surface the user is currently on. Only reachable actions are advertised, so a
//! hint never points at something that would refuse to run. The composer border
//! carries no shortcut list and no character counts.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};
use unicode_width::UnicodeWidthStr;

use crate::design::{Emphasis, Role, Symbols, Tokens};
use crate::ui_actions::{self, ShortcutContext, UiContext};

#[derive(Clone, Debug)]
pub struct Hints {
    pub entries: Vec<String>,
}

impl Hints {
    /// Hints for a surface, derived from the registry's primary shortcuts.
    pub fn for_context(context: ShortcutContext, state: &UiContext) -> Self {
        Self {
            entries: ui_actions::contextual_hints(context, state),
        }
    }

    /// Extra hints a surface adds that are not registry actions, such as
    /// "Enter New line".
    pub fn with(mut self, extra: &[&str]) -> Self {
        for entry in extra {
            let entry = (*entry).to_owned();
            if !self.entries.contains(&entry) {
                self.entries.push(entry);
            }
        }
        self
    }

    /// Hints that fit in `width`, dropping from the end. The first hint is the
    /// most important action on the surface and is kept even if truncated.
    pub fn line(&self, tokens: &Tokens<'_>, width: u16) -> Line<'static> {
        let symbols = Symbols::new(tokens.unicode());
        let separator = symbols.field_separator();
        let width = width as usize;
        let mut kept: Vec<&String> = Vec::new();
        let mut used = 0usize;
        for entry in &self.entries {
            let extra = UnicodeWidthStr::width(entry.as_str())
                + if kept.is_empty() {
                    0
                } else {
                    UnicodeWidthStr::width(separator)
                };
            if !kept.is_empty() && used + extra > width {
                break;
            }
            used += extra;
            kept.push(entry);
        }
        let mut spans = Vec::new();
        for (index, entry) in kept.iter().enumerate() {
            if index > 0 {
                spans.push(Span::styled(
                    separator.to_owned(),
                    tokens.styled(Role::Border, Emphasis::Dim),
                ));
            }
            spans.push(Span::styled(
                (*entry).clone(),
                tokens.styled(Role::Muted, Emphasis::Dim),
            ));
        }
        Line::from(spans)
    }

    pub fn render(&self, frame: &mut Frame<'_>, area: Rect, tokens: &Tokens<'_>) {
        Paragraph::new(self.line(tokens, area.width)).render(area, frame.buffer_mut());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_fixtures::test_theme;

    fn rendered(hints: &Hints, width: u16) -> String {
        let theme = test_theme();
        let tokens = Tokens::new(&theme);
        hints
            .line(&tokens, width)
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect()
    }

    #[test]
    fn approval_hints_appear_only_when_an_action_is_pending() {
        let idle = Hints::for_context(ShortcutContext::Approval, &UiContext::default());
        assert!(idle.entries.is_empty());

        let pending = Hints::for_context(
            ShortcutContext::Approval,
            &UiContext {
                daemon_reachable: true,
                pending_approval: true,
                ..UiContext::default()
            },
        );
        let line = rendered(&pending, 120);
        assert!(line.contains("A Approve action"), "{line}");
        assert!(line.contains("R Reject action"), "{line}");
        assert!(line.contains("D Inspect pending action"), "{line}");
    }

    #[test]
    fn workbench_hints_never_advertise_an_unreachable_action() {
        let hints = Hints::for_context(ShortcutContext::Workbench, &UiContext::default());
        let line = rendered(&hints, 200);
        assert!(
            !line.contains("Review diff"),
            "diff review is unavailable with no recorded effects: {line}"
        );
        assert!(line.contains("Command palette"), "{line}");
    }

    #[test]
    fn diff_review_is_advertised_once_effects_exist() {
        let hints = Hints::for_context(
            ShortcutContext::Workbench,
            &UiContext {
                daemon_reachable: true,
                repository_effects: true,
                ..UiContext::default()
            },
        );
        let line = rendered(&hints, 200);
        assert!(line.contains("Ctrl+D Review diff"), "{line}");
    }

    #[test]
    fn a_repeated_hint_is_not_shown_twice() {
        let hints = Hints {
            entries: vec!["Enter New line".into()],
        }
        .with(&["Enter New line", "Tab Move focus"]);
        assert_eq!(hints.entries.len(), 2, "{:?}", hints.entries);
    }

    #[test]
    fn extra_hints_are_appended() {
        let hints = Hints {
            entries: vec!["Ctrl+G Send".into()],
        }
        .with(&["Enter New line"]);
        let line = rendered(&hints, 80);
        assert_eq!(line, "Ctrl+G Send · Enter New line");
    }

    #[test]
    fn hints_are_dropped_from_the_end_and_never_overflow() {
        let hints = Hints {
            entries: vec![
                "Ctrl+G Send".into(),
                "Enter New line".into(),
                "Ctrl+P Commands".into(),
                "Ctrl+D Review diff".into(),
            ],
        };
        for width in [12, 20, 30, 40, 60, 80] {
            let line = rendered(&hints, width);
            assert!(
                line.starts_with("Ctrl+G Send"),
                "the primary action must survive at {width} columns: {line}"
            );
        }
        let narrow = rendered(&hints, 20);
        assert!(!narrow.contains("Review diff"), "{narrow}");
    }

    #[test]
    fn the_first_hint_survives_even_when_it_alone_exceeds_the_width() {
        let hints = Hints {
            entries: vec!["Ctrl+G Send a very long label".into(), "Esc Close".into()],
        };
        let line = rendered(&hints, 5);
        assert!(line.starts_with("Ctrl+G"), "{line}");
        assert!(!line.contains("Esc"), "{line}");
    }

    #[test]
    fn an_empty_hint_set_renders_nothing() {
        assert_eq!(
            rendered(
                &Hints {
                    entries: Vec::new()
                },
                40
            ),
            ""
        );
    }
}
