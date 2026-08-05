//! A centred, bordered overlay.
//!
//! Full borders are reserved for focused modals and decisions — regions of the
//! ordinary workbench are grouped with whitespace and a single separator
//! instead. A modal clears the cells it covers so the surface underneath cannot
//! bleed through and make a decision ambiguous.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::widgets::{Block, Borders, Clear, Widget};

use crate::design::{Role, Tokens};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ModalTone {
    Neutral,
    Attention,
    Danger,
}

impl ModalTone {
    const fn role(self) -> Role {
        match self {
            Self::Neutral => Role::Border,
            Self::Attention => Role::Warning,
            Self::Danger => Role::Danger,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Modal<'title> {
    pub title: &'title str,
    pub tone: ModalTone,
    /// Preferred size. Clamped to the available area, so a modal is usable at 60
    /// columns even when it asks for more.
    pub width: u16,
    pub height: u16,
}

impl<'title> Modal<'title> {
    pub const fn new(title: &'title str, tone: ModalTone, width: u16, height: u16) -> Self {
        Self {
            title,
            tone,
            width,
            height,
        }
    }

    /// The outer rect this modal occupies inside `area`.
    pub fn outer(&self, area: Rect) -> Rect {
        let width = self.width.min(area.width);
        let height = self.height.min(area.height);
        Rect::new(
            area.x + area.width.saturating_sub(width) / 2,
            area.y + area.height.saturating_sub(height) / 2,
            width,
            height,
        )
    }

    /// Draw the frame and return the content area.
    pub fn render(&self, frame: &mut Frame<'_>, area: Rect, tokens: &Tokens<'_>) -> Rect {
        let outer = self.outer(area);
        Clear.render(outer, frame.buffer_mut());
        let block = Block::default()
            .title(format!(" {} ", self.title))
            .borders(Borders::ALL)
            .border_set(crate::design::Symbols::new(tokens.unicode()).border_set())
            .border_style(tokens.style(self.tone.role()));
        let inner = block.inner(outer);
        block.render(outer, frame.buffer_mut());
        inner
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_fixtures::{test_terminal, test_theme};

    #[test]
    fn a_modal_is_centred() {
        let modal = Modal::new("Title", ModalTone::Neutral, 40, 10);
        let outer = modal.outer(Rect::new(0, 0, 100, 30));
        assert_eq!(outer, Rect::new(30, 10, 40, 10));
    }

    #[test]
    fn a_modal_never_exceeds_the_terminal() {
        let modal = Modal::new("Title", ModalTone::Neutral, 120, 40);
        let outer = modal.outer(Rect::new(0, 0, 60, 24));
        assert_eq!(outer, Rect::new(0, 0, 60, 24));
    }

    #[test]
    fn a_modal_remains_usable_at_sixty_columns() {
        let modal = Modal::new("Approval required", ModalTone::Attention, 100, 30);
        let outer = modal.outer(Rect::new(0, 0, 60, 24));
        assert!(outer.width >= 60);
        assert!(outer.height >= 24);
    }

    #[test]
    fn a_modal_clears_what_it_covers() {
        let theme = test_theme();
        let mut terminal = test_terminal(40, 10);
        terminal
            .draw(|frame| {
                let area = frame.area();
                // Paint the background with a marker the modal must erase.
                let filler = ratatui::widgets::Paragraph::new(
                    (0..area.height)
                        .map(|_| ratatui::text::Line::from("X".repeat(area.width as usize)))
                        .collect::<Vec<_>>(),
                );
                ratatui::widgets::Widget::render(filler, area, frame.buffer_mut());
                let tokens = Tokens::new(&theme);
                let inner =
                    Modal::new("Modal", ModalTone::Neutral, 20, 6).render(frame, area, &tokens);
                assert!(inner.width > 0 && inner.height > 0);
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        let centre = buffer
            .cell((20, 5))
            .expect("centre cell must exist")
            .symbol()
            .to_owned();
        assert_ne!(centre, "X", "the modal must clear the surface underneath");
    }

    #[test]
    fn tones_change_the_border_style() {
        let theme = test_theme();
        let tokens = Tokens::new(&theme);
        let neutral = tokens.style(ModalTone::Neutral.role());
        let danger = tokens.style(ModalTone::Danger.role());
        let attention = tokens.style(ModalTone::Attention.role());
        assert_ne!(neutral, danger);
        assert_ne!(attention, danger);
    }

    #[test]
    fn a_zero_sized_area_does_not_panic() {
        let modal = Modal::new("Title", ModalTone::Neutral, 40, 10);
        let outer = modal.outer(Rect::new(0, 0, 0, 0));
        assert_eq!(outer.width, 0);
        assert_eq!(outer.height, 0);
    }
}
