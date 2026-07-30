//! Conversation message blocks.
//!
//! A message is a visually separated block with a subtle role label, not a
//! sequence of lines each prefixed with `You:` or `PurrCode:`. Runtime events
//! never appear here — they belong to the activity rail — so the conversation
//! stays readable as a conversation.

use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Paragraph, Widget};
use ratatui::Frame;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::design::{Emphasis, Role, Tokens};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MessageRole {
    User,
    Assistant,
    /// A short notice that directly affects the conversation. Long-form runtime
    /// detail belongs to the activity rail and inspector instead.
    System,
}

impl MessageRole {
    pub fn parse(role: &str) -> Self {
        match role {
            "user" => Self::User,
            "assistant" => Self::Assistant,
            _ => Self::System,
        }
    }

    /// The subtle label shown once per block.
    pub const fn label(self) -> &'static str {
        match self {
            Self::User => "you",
            Self::Assistant => "purrcode",
            Self::System => "notice",
        }
    }

    const fn role(self) -> Role {
        match self {
            Self::User => Role::Accent,
            Self::Assistant => Role::Primary,
            Self::System => Role::Muted,
        }
    }
}

#[derive(Clone, Debug)]
pub struct MessageBlock<'content> {
    pub role: MessageRole,
    pub content: &'content str,
    /// Rendered with a trailing marker so a partial answer is never mistaken for
    /// a finished one.
    pub streaming: bool,
    pub selected: bool,
}

impl<'content> MessageBlock<'content> {
    pub fn new(role: MessageRole, content: &'content str) -> Self {
        Self {
            role,
            content,
            streaming: false,
            selected: false,
        }
    }

    pub fn streaming(mut self, streaming: bool) -> Self {
        self.streaming = streaming;
        self
    }

    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    /// Wrapped lines for this block, including its label row and trailing
    /// separator blank line.
    pub fn lines(&self, tokens: &Tokens<'_>, width: u16) -> Vec<Line<'static>> {
        let width = width.max(4) as usize;
        let symbols = crate::design::Symbols::new(tokens.unicode());
        let marker = if self.selected {
            symbols.selection()
        } else {
            symbols.no_selection()
        };
        let mut lines = vec![Line::from(vec![
            Span::styled(
                format!("{marker} "),
                tokens.styled(Role::Selected, Emphasis::Normal),
            ),
            Span::styled(
                self.role.label().to_owned(),
                tokens.styled(Role::Muted, Emphasis::Dim),
            ),
        ])];
        let body_style = tokens.style(self.role.role());
        let mut body = render_markdown(self.content, body_style, tokens);
        if self.streaming {
            body.push(Line::from(Span::styled(
                if tokens.unicode() {
                    "▌ still generating".to_owned()
                } else {
                    "_ still generating".to_owned()
                },
                tokens.styled(Role::Muted, Emphasis::Dim),
            )));
        }
        for line in body {
            for wrapped in wrap_line(line, width.saturating_sub(2)) {
                let mut spans = vec![Span::raw("  ")];
                spans.extend(wrapped.spans);
                lines.push(Line::from(spans));
            }
        }
        lines.push(Line::from(""));
        lines
    }

    pub fn render(&self, frame: &mut Frame<'_>, area: Rect, tokens: &Tokens<'_>) {
        let lines = self.lines(tokens, area.width);
        Paragraph::new(Text::from(lines)).render(area, frame.buffer_mut());
    }
}

/// Minimal Markdown: headings, list items, fenced code, inline code, paragraphs.
///
/// Code fences keep their contents verbatim rather than reinterpreting markers
/// inside them, so a `# ` line inside a code block stays a comment.
pub fn render_markdown(
    content: &str,
    base: ratatui::style::Style,
    tokens: &Tokens<'_>,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let mut in_code = false;
    for raw in content.lines() {
        if raw.trim_start().starts_with("```") {
            in_code = !in_code;
            let language = raw.trim_start().trim_start_matches('`').trim();
            lines.push(Line::from(Span::styled(
                if in_code && !language.is_empty() {
                    format!("code · {language}")
                } else if in_code {
                    "code".to_owned()
                } else {
                    "end code".to_owned()
                },
                tokens.styled(Role::Muted, Emphasis::Dim),
            )));
            continue;
        }
        if in_code {
            lines.push(Line::from(Span::styled(
                raw.to_owned(),
                tokens.style(Role::Success),
            )));
            continue;
        }
        let trimmed = raw.trim_start();
        if let Some(heading) = trimmed.strip_prefix("###").or_else(|| {
            trimmed
                .strip_prefix("##")
                .or_else(|| trimmed.strip_prefix('#'))
        }) {
            lines.push(Line::from(Span::styled(
                heading.trim_start_matches('#').trim().to_owned(),
                base.add_modifier(Modifier::BOLD),
            )));
            continue;
        }
        if trimmed.starts_with("- ") || trimmed.starts_with("* ") {
            let bullet = if tokens.unicode() { "• " } else { "- " };
            lines.push(Line::from(vec![
                Span::styled(bullet.to_owned(), tokens.style(Role::Accent)),
                Span::styled(trimmed[2..].to_owned(), base),
            ]));
            continue;
        }
        lines.push(Line::from(inline_code_spans(raw, base, tokens)));
    }
    if lines.is_empty() {
        lines.push(Line::from(Span::styled(String::new(), base)));
    }
    lines
}

/// Split a line on backtick pairs so inline code is visually distinct.
fn inline_code_spans(
    raw: &str,
    base: ratatui::style::Style,
    tokens: &Tokens<'_>,
) -> Vec<Span<'static>> {
    if !raw.contains('`') {
        return vec![Span::styled(raw.to_owned(), base)];
    }
    let code_style = tokens.style(Role::Success);
    let mut spans = Vec::new();
    let mut in_code = false;
    for segment in raw.split('`') {
        if !segment.is_empty() {
            spans.push(Span::styled(
                segment.to_owned(),
                if in_code { code_style } else { base },
            ));
        }
        in_code = !in_code;
    }
    spans
}

/// Wrap one styled line to `width`, preferring word boundaries and falling back
/// to grapheme splitting for unbroken runs such as long paths.
pub fn wrap_line(line: Line<'_>, width: usize) -> Vec<Line<'static>> {
    let width = width.max(1);
    let mut wrapped = Vec::new();
    let mut current: Vec<Span<'static>> = Vec::new();
    let mut current_width = 0usize;

    for span in line.spans {
        let style = span.style;
        for word in split_keeping_spaces(&span.content) {
            let word_width = UnicodeWidthStr::width(word.as_str());
            if word_width > width {
                // An unbreakable run longer than the line: split by grapheme.
                for grapheme in word.graphemes(true) {
                    let grapheme_width = UnicodeWidthStr::width(grapheme);
                    if current_width + grapheme_width > width && current_width > 0 {
                        wrapped.push(Line::from(std::mem::take(&mut current)));
                        current_width = 0;
                    }
                    current.push(Span::styled(grapheme.to_owned(), style));
                    current_width += grapheme_width;
                }
                continue;
            }
            if current_width + word_width > width {
                if word.trim().is_empty() {
                    // Do not start a line with the space that caused the wrap.
                    wrapped.push(Line::from(std::mem::take(&mut current)));
                    current_width = 0;
                    continue;
                }
                wrapped.push(Line::from(std::mem::take(&mut current)));
                current_width = 0;
            }
            if current_width == 0 && word.trim().is_empty() {
                continue;
            }
            current.push(Span::styled(word.clone(), style));
            current_width += word_width;
        }
    }
    wrapped.push(Line::from(current));
    wrapped
}

fn split_keeping_spaces(content: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut current_is_space = None;
    for grapheme in content.graphemes(true) {
        let is_space = grapheme.chars().all(char::is_whitespace);
        if current_is_space.is_some_and(|previous| previous != is_space) {
            parts.push(std::mem::take(&mut current));
        }
        current_is_space = Some(is_space);
        current.push_str(grapheme);
    }
    if !current.is_empty() {
        parts.push(current);
    }
    parts
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_fixtures::{monochrome_theme, test_theme};

    fn text_of(lines: &[Line<'_>]) -> Vec<String> {
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
    fn a_message_is_one_block_with_a_single_role_label() {
        let theme = test_theme();
        let tokens = Tokens::new(&theme);
        let block = MessageBlock::new(
            MessageRole::Assistant,
            "first line\nsecond line\nthird line",
        );
        let rendered = text_of(&block.lines(&tokens, 40));
        assert!(rendered[0].contains("purrcode"));
        let prefixed = rendered
            .iter()
            .filter(|line| line.contains("PurrCode:") || line.contains("You:"))
            .count();
        assert_eq!(prefixed, 0, "no line may carry a role prefix: {rendered:?}");
        assert_eq!(
            rendered
                .iter()
                .filter(|line| line.contains("purrcode"))
                .count(),
            1,
            "the role label appears once per block"
        );
    }

    #[test]
    fn every_role_has_a_distinct_label() {
        let labels = [
            MessageRole::User.label(),
            MessageRole::Assistant.label(),
            MessageRole::System.label(),
        ];
        assert_eq!(
            labels
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len(),
            3
        );
    }

    #[test]
    fn streaming_blocks_are_marked_so_partial_output_is_not_mistaken_for_final() {
        let theme = test_theme();
        let tokens = Tokens::new(&theme);
        let block = MessageBlock::new(MessageRole::Assistant, "partial").streaming(true);
        let rendered = text_of(&block.lines(&tokens, 40)).join("\n");
        assert!(rendered.contains("still generating"), "{rendered}");
    }

    #[test]
    fn code_fences_keep_their_contents_verbatim() {
        let theme = test_theme();
        let tokens = Tokens::new(&theme);
        let lines = render_markdown(
            "before\n```rust\n# not a heading\nlet x = 1;\n```\nafter",
            ratatui::style::Style::default(),
            &tokens,
        );
        let rendered = text_of(&lines);
        assert!(
            rendered.iter().any(|line| line == "# not a heading"),
            "a comment inside a fence must not become a heading: {rendered:?}"
        );
        assert!(rendered.iter().any(|line| line.contains("code · rust")));
        assert!(rendered.iter().any(|line| line.contains("end code")));
    }

    #[test]
    fn headings_and_lists_render_without_their_markers() {
        let theme = test_theme();
        let tokens = Tokens::new(&theme);
        let lines = render_markdown(
            "# Title\n## Sub\n- one\n* two",
            ratatui::style::Style::default(),
            &tokens,
        );
        let rendered = text_of(&lines);
        assert_eq!(rendered[0], "Title");
        assert_eq!(rendered[1], "Sub");
        assert!(rendered[2].ends_with("one"));
        assert!(!rendered[2].starts_with('-'));
        assert!(rendered[3].ends_with("two"));
    }

    #[test]
    fn inline_code_is_separated_into_its_own_span() {
        let theme = test_theme();
        let tokens = Tokens::new(&theme);
        let lines = render_markdown(
            "run `cargo test` now",
            ratatui::style::Style::default(),
            &tokens,
        );
        let spans = &lines[0].spans;
        assert!(spans.len() >= 3, "{spans:?}");
        assert!(spans.iter().any(|span| span.content == "cargo test"));
    }

    #[test]
    fn wrapping_prefers_word_boundaries() {
        let wrapped = wrap_line(
            Line::from(Span::raw("the quick brown fox jumps over the lazy dog")),
            12,
        );
        let rendered = text_of(&wrapped);
        assert!(rendered.iter().all(|line| line.chars().count() <= 12));
        assert!(
            rendered.iter().all(|line| !line.starts_with(' ')),
            "a wrapped line must not begin with the space that caused the wrap: {rendered:?}"
        );
        assert_eq!(rendered.join(" ").split_whitespace().count(), 9);
    }

    #[test]
    fn an_unbreakable_run_is_split_by_grapheme_instead_of_overflowing() {
        let wrapped = wrap_line(
            Line::from(Span::raw("crates/purrcode-tui/src/components/message.rs")),
            10,
        );
        assert!(wrapped.len() > 1);
        for line in &wrapped {
            assert!(line.width() <= 10, "line {line:?} overflowed");
        }
    }

    #[test]
    fn wide_graphemes_never_exceed_the_available_width() {
        let wrapped = wrap_line(Line::from(Span::raw("日本語のテキストが折り返される")), 9);
        for line in &wrapped {
            assert!(line.width() <= 9, "wide text overflowed: {line:?}");
        }
    }

    #[test]
    fn blocks_render_without_colour_support() {
        let theme = monochrome_theme();
        let tokens = Tokens::new(&theme);
        let block = MessageBlock::new(MessageRole::User, "hello `world`");
        let rendered = text_of(&block.lines(&tokens, 30)).join("\n");
        assert!(rendered.contains("you"));
        assert!(rendered.contains("world"));
    }

    #[test]
    fn selected_blocks_carry_a_visible_marker() {
        let theme = test_theme();
        let tokens = Tokens::new(&theme);
        let plain = text_of(&MessageBlock::new(MessageRole::User, "x").lines(&tokens, 20));
        let picked = text_of(
            &MessageBlock::new(MessageRole::User, "x")
                .selected(true)
                .lines(&tokens, 20),
        );
        assert_ne!(plain[0], picked[0]);
    }

    #[test]
    fn a_very_narrow_area_does_not_panic() {
        let theme = test_theme();
        let tokens = Tokens::new(&theme);
        let block = MessageBlock::new(MessageRole::Assistant, "some content here");
        for width in 0..8 {
            let _ = block.lines(&tokens, width);
        }
    }
}
