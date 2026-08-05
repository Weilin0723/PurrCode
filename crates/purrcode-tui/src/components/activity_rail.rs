//! The compact activity rail.
//!
//! Runtime progress as a short list of stages, not as expanded event cards.
//! Activity is deliberately lighter than the conversation: dim labels, no
//! border, and a bounded height enforced by the layout. Selecting an entry opens
//! its detail in the inspector rather than expanding it in place.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};

use crate::activity::{ActivityEntry, ActivityState};
use crate::design::{Emphasis, Role, Symbols, Tokens};

#[derive(Clone, Debug)]
pub struct ActivityRail<'entries> {
    pub entries: &'entries [ActivityEntry],
    pub selected: Option<usize>,
    pub focused: bool,
}

impl<'entries> ActivityRail<'entries> {
    pub fn new(entries: &'entries [ActivityEntry]) -> Self {
        Self {
            entries,
            selected: None,
            focused: false,
        }
    }

    pub fn selected(mut self, selected: Option<usize>) -> Self {
        self.selected = selected;
        self
    }

    pub fn focused(mut self, focused: bool) -> Self {
        self.focused = focused;
        self
    }

    /// Entries that fit in `height`, keeping the most recent visible and
    /// summarizing what scrolled off. The tail matters most: it is what PurrCode
    /// is doing right now.
    pub fn lines(&self, tokens: &Tokens<'_>, height: u16) -> Vec<Line<'static>> {
        let symbols = Symbols::new(tokens.unicode());
        if self.entries.is_empty() {
            return vec![Line::from(Span::styled(
                "No runtime activity yet".to_owned(),
                tokens.styled(Role::Muted, Emphasis::Dim),
            ))];
        }
        let height = height.max(1) as usize;
        let (skipped, visible) = if self.entries.len() > height {
            // Reserve one row for the overflow summary, unless there is only one
            // row available — then the current stage matters more than the count.
            let shown = if height == 1 { 1 } else { height - 1 };
            let start = self.entries.len() - shown;
            (if height == 1 { 0 } else { start }, &self.entries[start..])
        } else {
            (0, self.entries)
        };

        let mut lines = Vec::new();
        if skipped > 0 {
            lines.push(Line::from(Span::styled(
                format!("{} earlier step(s){}", skipped, symbols.ellipsis()),
                tokens.styled(Role::Muted, Emphasis::Dim),
            )));
        }
        for (offset, entry) in visible.iter().enumerate() {
            let index = skipped + offset;
            let selected = self.selected == Some(index);
            let role = match entry.state {
                ActivityState::Done => Role::Success,
                ActivityState::Active => Role::Accent,
                ActivityState::Pending => Role::Muted,
                ActivityState::Attention => Role::Warning,
                ActivityState::Failed => Role::Danger,
            };
            let mut spans = vec![
                Span::styled(
                    format!(
                        "{}{} ",
                        if selected {
                            symbols.selection()
                        } else {
                            symbols.no_selection()
                        },
                        entry.state.glyph(tokens.unicode())
                    ),
                    tokens.style(role),
                ),
                Span::styled(
                    entry.label.clone(),
                    tokens.styled(
                        if selected {
                            Role::Selected
                        } else {
                            Role::Muted
                        },
                        if entry.state == ActivityState::Active {
                            Emphasis::Strong
                        } else {
                            Emphasis::Normal
                        },
                    ),
                ),
            ];
            // States that need the user always spell out the word, so the glyph
            // and colour are never the only signal.
            if matches!(
                entry.state,
                ActivityState::Attention | ActivityState::Failed | ActivityState::Pending
            ) {
                spans.push(Span::styled(
                    format!(" ({})", entry.state.word()),
                    tokens.styled(role, Emphasis::Dim),
                ));
            }
            lines.push(Line::from(spans));
        }
        lines
    }

    pub fn render(&self, frame: &mut Frame<'_>, area: Rect, tokens: &Tokens<'_>) {
        let lines = self.lines(tokens, area.height);
        Paragraph::new(lines).render(area, frame.buffer_mut());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::activity;
    use crate::test_fixtures::{monochrome_theme, test_theme};
    use serde_json::json;

    fn entries() -> Vec<ActivityEntry> {
        activity::derive(&[
            json!({"event":"session_created","data":{"objective":"Add a test"}}),
            json!({"event":"context_indexed","data":{"files":4,"symbols":9,"sensitive_files":0}}),
            json!({"event":"plan_created","data":{"steps":["a","b"]}}),
            json!({"event":"action_proposed","data":{"action":{"type":"write_file","path":"src/runtime.rs"}}}),
        ])
        .activity
    }

    fn text(lines: &[Line<'_>]) -> Vec<String> {
        lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect()
    }

    #[test]
    fn rail_reads_as_stages_not_as_events() {
        let theme = test_theme();
        let tokens = Tokens::new(&theme);
        let entries = entries();
        let rendered = text(&ActivityRail::new(&entries).lines(&tokens, 10));
        assert!(
            rendered
                .iter()
                .any(|line| line.contains("Context prepared"))
        );
        assert!(rendered.iter().any(|line| line.contains("Plan created")));
        assert!(
            rendered
                .iter()
                .any(|line| line.contains("Editing src/runtime.rs"))
        );
        assert!(
            rendered.iter().all(|line| !line.contains('{')),
            "no raw JSON may reach the rail: {rendered:?}"
        );
    }

    #[test]
    fn an_empty_rail_says_so_instead_of_rendering_nothing() {
        let theme = test_theme();
        let tokens = Tokens::new(&theme);
        let rendered = text(&ActivityRail::new(&[]).lines(&tokens, 4));
        assert_eq!(rendered, vec!["No runtime activity yet".to_owned()]);
    }

    #[test]
    fn overflow_keeps_the_newest_stages_and_counts_what_scrolled_off() {
        let theme = test_theme();
        let tokens = Tokens::new(&theme);
        let entries = entries();
        let rendered = text(&ActivityRail::new(&entries).lines(&tokens, 3));
        assert_eq!(rendered.len(), 3);
        assert!(rendered[0].contains("earlier step"), "{rendered:?}");
        assert!(
            rendered.last().unwrap().contains("Editing"),
            "the current stage must stay visible: {rendered:?}"
        );
    }

    #[test]
    fn states_needing_the_user_spell_out_the_word() {
        let theme = monochrome_theme();
        let tokens = Tokens::new(&theme);
        let entries = activity::derive(&[
            json!({"event":"action_proposed","data":{"action_id":"a","action":{"type":"write_file","path":"x.rs"}}}),
            json!({"event":"judgment_recorded","data":{"action_id":"a","decision":{"decision":"require_approval"}}}),
        ])
        .activity;
        let rendered = text(&ActivityRail::new(&entries).lines(&tokens, 6)).join("\n");
        assert!(rendered.contains("Awaiting your approval"), "{rendered}");
        assert!(
            rendered.contains("needs you"),
            "attention must not depend on colour or glyph alone: {rendered}"
        );
    }

    #[test]
    fn ascii_fallback_renders_every_state() {
        let theme = monochrome_theme();
        let tokens = Tokens::new(&theme);
        let entries = entries();
        let rendered = text(&ActivityRail::new(&entries).lines(&tokens, 10)).join("\n");
        assert!(rendered.is_ascii(), "non-ASCII glyph leaked: {rendered}");
    }

    #[test]
    fn selection_is_visible() {
        let theme = test_theme();
        let tokens = Tokens::new(&theme);
        let entries = entries();
        let plain = text(&ActivityRail::new(&entries).lines(&tokens, 10));
        let picked = text(
            &ActivityRail::new(&entries)
                .selected(Some(1))
                .lines(&tokens, 10),
        );
        assert_ne!(plain[1], picked[1]);
    }

    #[test]
    fn a_single_row_rail_still_shows_the_current_stage() {
        let theme = test_theme();
        let tokens = Tokens::new(&theme);
        let entries = entries();
        let rendered = text(&ActivityRail::new(&entries).lines(&tokens, 1));
        assert_eq!(rendered.len(), 1);
        assert!(rendered[0].contains("Editing"), "{rendered:?}");
    }

    #[test]
    fn zero_height_does_not_panic() {
        let theme = test_theme();
        let tokens = Tokens::new(&theme);
        let entries = entries();
        let _ = ActivityRail::new(&entries).lines(&tokens, 0);
    }
}
