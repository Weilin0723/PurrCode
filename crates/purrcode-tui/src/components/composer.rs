//! The composer's presentation.
//!
//! The default title is minimal — `Build · Ask PurrCode…` — with no character
//! count, line count, paste count, or shortcut list. Those move to the hint line
//! below, or appear only when they matter: a paste badge and counts show up once
//! the draft is large enough that they are genuinely useful.
//!
//! Editing state itself stays in [`crate::composer::Composer`]; this only draws.

use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Widget};
use ratatui::Frame;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::design::{Role, Tokens};

/// Counts are noise below this size and orientation above it.
const COUNT_DISCLOSURE_GRAPHEMES: usize = 2_000;

#[derive(Clone, Debug)]
pub struct ComposerView<'data> {
    /// True when the terminal renders Unicode; drives separator and ellipsis.
    pub unicode: bool,
    pub mode: &'data str,
    pub buffer: &'data str,
    pub cursor_line: usize,
    pub cursor_column: usize,
    pub line_count: usize,
    pub grapheme_count: usize,
    pub pasted: bool,
    pub is_command: bool,
    pub focused: bool,
    /// True when input is refused, e.g. read-only history.
    pub read_only: bool,
}

impl<'data> ComposerView<'data> {
    /// The border title. Minimal by default; discloses counts only when large.
    pub fn title(&self) -> String {
        let symbols = crate::design::Symbols::new(self.unicode);
        let separator = symbols.field_separator();
        if self.read_only {
            return format!("History{separator}read-only");
        }
        let mut title = format!("{}{separator}Ask PurrCode{}", self.mode, symbols.ellipsis());
        if self.grapheme_count >= COUNT_DISCLOSURE_GRAPHEMES {
            title.push_str(&format!(
                "{separator}{} lines{separator}{} chars",
                self.line_count, self.grapheme_count
            ));
        }
        if self.pasted {
            title.push_str(&format!("{separator}pasted"));
        }
        title
    }

    /// Render and return the terminal cursor position, or `None` when the
    /// composer cannot accept input.
    pub fn render(
        &self,
        frame: &mut Frame<'_>,
        area: Rect,
        tokens: &Tokens<'_>,
    ) -> Option<(u16, u16)> {
        let block = Block::default()
            .title(format!(" {} ", self.title()))
            .borders(Borders::TOP)
            .border_set(crate::design::Symbols::new(tokens.unicode()).border_set())
            .border_style(tokens.style(if self.focused {
                Role::Accent
            } else {
                Role::Border
            }));
        let inner = block.inner(area);
        block.render(area, frame.buffer_mut());
        if inner.width == 0 || inner.height == 0 {
            return None;
        }

        let height = inner.height as usize;
        let width = inner.width as usize;
        let first_line = self.cursor_line.saturating_sub(height.saturating_sub(1));
        let first_column = self.cursor_column.saturating_sub(width.saturating_sub(1));
        let style = tokens.style(if self.is_command {
            Role::Success
        } else if self.read_only {
            Role::Muted
        } else {
            Role::Primary
        });

        let lines: Vec<Line<'static>> = self
            .buffer
            .split('\n')
            .skip(first_line)
            .take(height)
            .map(|line| {
                let visible: String = line
                    .graphemes(true)
                    .skip(first_column)
                    .take(width)
                    .collect();
                Line::from(Span::styled(visible, style))
            })
            .collect();
        Paragraph::new(lines).render(inner, frame.buffer_mut());

        if self.read_only {
            return None;
        }
        let current = self.buffer.split('\n').nth(self.cursor_line).unwrap_or("");
        let prefix: String = current
            .graphemes(true)
            .skip(first_column)
            .take(self.cursor_column.saturating_sub(first_column))
            .collect();
        Some((
            inner
                .x
                .saturating_add(UnicodeWidthStr::width(prefix.as_str()) as u16)
                .min(inner.right().saturating_sub(1)),
            inner
                .y
                .saturating_add(self.cursor_line.saturating_sub(first_line) as u16)
                .min(inner.bottom().saturating_sub(1)),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_fixtures::{test_terminal, test_theme};

    fn view(buffer: &str) -> ComposerView<'_> {
        ComposerView {
            unicode: true,
            mode: "Build",
            buffer,
            cursor_line: 0,
            cursor_column: buffer.split('\n').next().unwrap_or("").chars().count(),
            line_count: buffer.split('\n').count(),
            grapheme_count: buffer.graphemes(true).count(),
            pasted: false,
            is_command: buffer.starts_with('/'),
            focused: true,
            read_only: false,
        }
    }

    #[test]
    fn the_default_title_is_minimal() {
        assert_eq!(view("hello").title(), "Build · Ask PurrCode…");
    }

    #[test]
    fn the_default_title_carries_no_counts_or_shortcut_list() {
        let title = view("a short draft").title();
        for forbidden in ["chars", "lines", "Ctrl+G", "send", "newline"] {
            assert!(
                !title.contains(forbidden),
                "{forbidden:?} belongs on the hint line, not the composer title: {title}"
            );
        }
    }

    #[test]
    fn counts_appear_only_for_a_large_draft() {
        let mut small = view("x");
        small.grapheme_count = 100;
        assert!(!small.title().contains("chars"));

        let mut large = view("x");
        large.grapheme_count = 5_000;
        large.line_count = 90;
        let title = large.title();
        assert!(title.contains("5000 chars"), "{title}");
        assert!(title.contains("90 lines"), "{title}");
    }

    #[test]
    fn the_title_falls_back_to_ascii_without_unicode() {
        let mut plain = view("hello");
        plain.unicode = false;
        assert!(plain.title().is_ascii(), "{}", plain.title());
        plain.read_only = true;
        assert!(plain.title().is_ascii(), "{}", plain.title());
    }

    #[test]
    fn a_paste_is_badged() {
        let mut pasted = view("content");
        pasted.pasted = true;
        assert!(pasted.title().ends_with("· pasted"));
    }

    #[test]
    fn read_only_history_says_so_and_refuses_a_cursor() {
        let theme = test_theme();
        let mut read_only = view("cannot type here");
        read_only.read_only = true;
        assert_eq!(read_only.title(), "History · read-only");
        let mut terminal = test_terminal(60, 5);
        let cursor = terminal
            .draw(|frame| {
                let tokens = Tokens::new(&theme);
                let position = read_only.render(frame, frame.area(), &tokens);
                assert!(position.is_none());
            })
            .map(|_| ())
            .is_ok();
        assert!(cursor);
    }

    #[test]
    fn the_cursor_stays_inside_the_composer() {
        let theme = test_theme();
        let filled = "x".repeat(200);
        let mut long = view(&filled);
        long.cursor_column = 200;
        let mut terminal = test_terminal(40, 4);
        terminal
            .draw(|frame| {
                let tokens = Tokens::new(&theme);
                let area = frame.area();
                let position = long.render(frame, area, &tokens).unwrap();
                assert!(position.0 < area.right(), "cursor escaped horizontally");
                assert!(position.1 < area.bottom(), "cursor escaped vertically");
            })
            .unwrap();
    }

    #[test]
    fn a_zero_height_composer_does_not_panic() {
        let theme = test_theme();
        let composer = view("x");
        let mut terminal = test_terminal(20, 1);
        terminal
            .draw(|frame| {
                let tokens = Tokens::new(&theme);
                let _ = composer.render(frame, Rect::new(0, 0, 20, 1), &tokens);
            })
            .unwrap();
    }
}
