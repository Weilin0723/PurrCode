#[derive(Clone, Copy, Debug, PartialEq)]
pub enum StatusGlyph {
    User,
    Assistant,
    Plan,
    Action,
    PawGate,
    Claw,
    Output,
    Validation,
    Checkpoint,
    Recovery,
    Completion,
    Skill,
    Context,
}

impl StatusGlyph {
    pub fn unicode(&self) -> &'static str {
        match self {
            Self::User => "\u{1F464}",
            Self::Assistant => "\u{1F916}",
            Self::Plan => "\u{1F4CB}",
            Self::Action => "\u{27A1}",
            Self::PawGate => "\u{1F6E1}",
            Self::Claw => "\u{2699}",
            Self::Output => "\u{2502}",
            Self::Validation => "\u{2713}",
            Self::Checkpoint => "\u{1F4BE}",
            Self::Recovery => "\u{26A0}",
            Self::Completion => "\u{2714}",
            Self::Skill => "\u{1F9D0}",
            Self::Context => "\u{00B7}",
        }
    }

    pub fn ascii(&self) -> &'static str {
        match self {
            Self::User => "[U]",
            Self::Assistant => "[A]",
            Self::Plan => "[P]",
            Self::Action => "[>]",
            Self::PawGate => "[G]",
            Self::Claw => "[X]",
            Self::Output => "[|]",
            Self::Validation => "[V]",
            Self::Checkpoint => "[S]",
            Self::Recovery => "[!]",
            Self::Completion => "[C]",
            Self::Skill => "[K]",
            Self::Context => "[.]",
        }
    }

    pub fn render(&self, unicode: bool) -> &'static str {
        if unicode {
            self.unicode()
        } else {
            self.ascii()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_glyphs_have_ascii_and_unicode() {
        let variants = [
            StatusGlyph::User,
            StatusGlyph::Assistant,
            StatusGlyph::Plan,
            StatusGlyph::Action,
            StatusGlyph::PawGate,
            StatusGlyph::Claw,
            StatusGlyph::Output,
            StatusGlyph::Validation,
            StatusGlyph::Checkpoint,
            StatusGlyph::Recovery,
            StatusGlyph::Completion,
            StatusGlyph::Skill,
            StatusGlyph::Context,
        ];
        for glyph in &variants {
            assert!(!glyph.unicode().is_empty());
            assert!(!glyph.ascii().is_empty());
            assert_ne!(glyph.unicode(), glyph.ascii());
        }
    }

    #[test]
    fn unicode_mode_returns_unicode_glyph() {
        assert_eq!(StatusGlyph::Plan.render(true), "\u{1F4CB}");
    }

    #[test]
    fn ascii_mode_returns_ascii_glyph() {
        assert_eq!(StatusGlyph::Plan.render(false), "[P]");
    }

    #[test]
    fn glyphs_are_distinct_in_ascii_mode() {
        use std::collections::HashSet;
        let variants = [
            StatusGlyph::User,
            StatusGlyph::Assistant,
            StatusGlyph::Plan,
            StatusGlyph::Action,
            StatusGlyph::PawGate,
            StatusGlyph::Claw,
            StatusGlyph::Output,
            StatusGlyph::Validation,
            StatusGlyph::Checkpoint,
            StatusGlyph::Recovery,
            StatusGlyph::Completion,
            StatusGlyph::Skill,
            StatusGlyph::Context,
        ];
        let ascii: HashSet<&str> = variants.iter().map(|g| g.ascii()).collect();
        assert_eq!(ascii.len(), variants.len());
    }
}
