//! Glyphs with a mandatory ASCII fallback.
//!
//! Every symbol here has an ASCII form. A terminal without Unicode support must
//! still render every state distinguishably, so no call site may inline a
//! Unicode character directly.

#[derive(Clone, Copy, Debug)]
pub struct Symbols {
    unicode: bool,
}

impl Symbols {
    pub const fn new(unicode: bool) -> Self {
        Self { unicode }
    }

    /// Separator between header fields.
    pub const fn field_separator(self) -> &'static str {
        if self.unicode { " · " } else { " | " }
    }

    /// Horizontal rule used as the single separator between major regions.
    pub const fn horizontal_rule(self) -> &'static str {
        if self.unicode { "─" } else { "-" }
    }

    /// Marker for the selected row.
    pub const fn selection(self) -> &'static str {
        if self.unicode { "▸" } else { ">" }
    }

    /// Marker for a row that is not selected. Same display width as
    /// [`Self::selection`] so rows do not shift.
    pub const fn no_selection(self) -> &'static str {
        " "
    }

    /// Local-only inference.
    pub const fn local(self) -> &'static str {
        if self.unicode { "⌂" } else { "[local]" }
    }

    /// Network-reachable inference.
    pub const fn remote(self) -> &'static str {
        if self.unicode { "↗" } else { "[remote]" }
    }

    /// Attention marker used by decision surfaces.
    pub const fn attention(self) -> &'static str {
        if self.unicode { "▲" } else { "!" }
    }

    pub const fn ellipsis(self) -> &'static str {
        if self.unicode { "…" } else { "..." }
    }

    /// Dash used before an explanatory clause.
    pub const fn dash(self) -> &'static str {
        if self.unicode { "—" } else { "-" }
    }

    /// Arrow used when the automatic workflow resolves to a profile.
    pub const fn workflow_arrow(self) -> &'static str {
        if self.unicode { "→" } else { "->" }
    }

    /// Border character set for blocks and modals.
    ///
    /// A terminal without Unicode support renders box-drawing characters as
    /// mojibake, so borders need the same fallback as status glyphs.
    pub const fn border_set(self) -> ratatui::symbols::border::Set<'static> {
        if self.unicode {
            ratatui::symbols::border::PLAIN
        } else {
            ratatui::symbols::border::Set {
                top_left: "+",
                top_right: "+",
                bottom_left: "+",
                bottom_right: "+",
                vertical_left: "|",
                vertical_right: "|",
                horizontal_top: "-",
                horizontal_bottom: "-",
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_fallback_exists_for_every_symbol() {
        let ascii = Symbols::new(false);
        for symbol in [
            ascii.field_separator(),
            ascii.horizontal_rule(),
            ascii.selection(),
            ascii.no_selection(),
            ascii.local(),
            ascii.remote(),
            ascii.attention(),
            ascii.ellipsis(),
            ascii.dash(),
        ] {
            assert!(
                symbol.is_ascii(),
                "{symbol:?} must have an ASCII fallback form"
            );
        }
    }

    #[test]
    fn selection_and_non_selection_have_the_same_width_so_rows_do_not_shift() {
        for unicode in [true, false] {
            let symbols = Symbols::new(unicode);
            assert_eq!(
                unicode_width::UnicodeWidthStr::width(symbols.selection()),
                unicode_width::UnicodeWidthStr::width(symbols.no_selection()),
            );
        }
    }

    #[test]
    fn borders_fall_back_to_ascii_without_unicode() {
        let set = Symbols::new(false).border_set();
        for part in [
            set.top_left,
            set.top_right,
            set.bottom_left,
            set.bottom_right,
            set.vertical_left,
            set.vertical_right,
            set.horizontal_top,
            set.horizontal_bottom,
        ] {
            assert!(part.is_ascii(), "{part:?} must have an ASCII form");
        }
        assert_ne!(
            Symbols::new(true).border_set().top_left,
            set.top_left,
            "the Unicode set must still be used when Unicode is available"
        );
    }

    #[test]
    fn local_and_remote_are_never_the_same_marker() {
        for unicode in [true, false] {
            let symbols = Symbols::new(unicode);
            assert_ne!(symbols.local(), symbols.remote());
        }
    }
}
