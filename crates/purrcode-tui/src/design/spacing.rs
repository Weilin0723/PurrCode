//! Named spacing.
//!
//! Depth comes from whitespace and a single separator rather than a border
//! around every region, because a terminal that only supports black and white
//! backgrounds cannot express elevation with colour.

use ratatui::layout::Rect;

pub struct Spacing;

impl Spacing {
    /// Gap between major regions.
    pub const REGION: u16 = 1;
    /// Left gutter inside a region, so text never touches the terminal edge.
    pub const GUTTER: u16 = 1;
    /// Indentation for a nested detail line.
    pub const INDENT: u16 = 2;

    /// Inset a rect by a horizontal gutter, leaving vertical extent alone.
    /// Returns a zero-width rect when the area is too narrow to inset, which
    /// callers treat as "nothing to draw".
    pub fn gutter(area: Rect) -> Rect {
        Self::inset(area, Self::GUTTER, 0)
    }

    pub fn inset(area: Rect, horizontal: u16, vertical: u16) -> Rect {
        let width = area.width.saturating_sub(horizontal.saturating_mul(2));
        let height = area.height.saturating_sub(vertical.saturating_mul(2));
        if width == 0 || height == 0 {
            return Rect::new(area.x, area.y, 0, 0);
        }
        Rect::new(
            area.x.saturating_add(horizontal),
            area.y.saturating_add(vertical),
            width,
            height,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gutter_keeps_text_off_the_terminal_edge() {
        let inner = Spacing::gutter(Rect::new(0, 0, 20, 4));
        assert_eq!(inner, Rect::new(1, 0, 18, 4));
    }

    #[test]
    fn insetting_past_the_available_space_yields_nothing_to_draw() {
        assert_eq!(
            Spacing::inset(Rect::new(0, 0, 2, 2), 4, 0),
            Rect::new(0, 0, 0, 0)
        );
        assert_eq!(
            Spacing::inset(Rect::new(0, 0, 2, 1), 0, 2),
            Rect::new(0, 0, 0, 0)
        );
    }

    #[test]
    fn a_one_column_area_survives_without_underflow() {
        let inner = Spacing::gutter(Rect::new(5, 5, 1, 1));
        assert_eq!(inner.width, 0);
        assert_eq!(inner.x, 5);
    }
}
