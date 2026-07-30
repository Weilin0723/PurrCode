//! The terminal screen, parsed from the raw ANSI stream.
//!
//! Assertions run against this model rather than against the byte stream, so a
//! test asserts what a user would see — including text that arrived via cursor
//! movement, scroll regions, or a repaint that overwrote earlier output.

/// A snapshot of the terminal grid.
#[derive(Clone, Debug)]
pub struct Screen {
    rows: Vec<String>,
    cursor: (u16, u16),
    width: u16,
    height: u16,
}

impl Screen {
    pub fn from_parser(parser: &vt100::Parser) -> Self {
        let screen = parser.screen();
        let (height, width) = screen.size();
        let rows = (0..height)
            .map(|row| {
                (0..width)
                    .map(|column| {
                        screen
                            .cell(row, column)
                            .map(|cell| {
                                let contents = cell.contents();
                                if contents.is_empty() {
                                    " ".to_owned()
                                } else {
                                    contents
                                }
                            })
                            .unwrap_or_else(|| " ".to_owned())
                    })
                    .collect::<String>()
            })
            .collect();
        Self {
            rows,
            cursor: screen.cursor_position(),
            width,
            height,
        }
    }

    pub fn width(&self) -> u16 {
        self.width
    }

    pub fn height(&self) -> u16 {
        self.height
    }

    pub fn cursor(&self) -> (u16, u16) {
        self.cursor
    }

    /// One row, with trailing padding removed.
    pub fn row(&self, index: usize) -> &str {
        self.rows.get(index).map(|row| row.trim_end()).unwrap_or("")
    }

    pub fn rows(&self) -> Vec<&str> {
        (0..self.rows.len()).map(|index| self.row(index)).collect()
    }

    /// The whole screen as text, one row per line.
    pub fn text(&self) -> String {
        self.rows()
            .iter()
            .map(|row| (*row).to_owned())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// True when `needle` appears on any single row.
    ///
    /// Row-scoped on purpose: a phrase that only appears because two unrelated
    /// rows happen to abut is not something a user can read.
    pub fn contains(&self, needle: &str) -> bool {
        self.rows().iter().any(|row| row.contains(needle))
    }

    /// True when `needle` appears anywhere, tolerating a wrap across rows.
    ///
    /// Use for long sentences that legitimately wrap; prefer [`Self::contains`]
    /// for labels and keys, which must be readable on one line.
    pub fn contains_wrapped(&self, needle: &str) -> bool {
        let flattened: String = self
            .rows()
            .iter()
            .flat_map(|row| row.split_whitespace())
            .collect::<Vec<_>>()
            .join(" ");
        let wanted = needle.split_whitespace().collect::<Vec<_>>().join(" ");
        flattened.contains(&wanted)
    }

    /// Text of a horizontal slice of the screen, flattened across rows.
    ///
    /// The inspector is a column: its sentences wrap inside their own width, so a
    /// whole-row flatten interleaves them with unrelated text. Scoping to the
    /// column makes those sentences assertable.
    pub fn column_text(&self, from: u16, to: u16) -> String {
        let from = from as usize;
        let to = (to as usize).min(self.width as usize);
        self.rows
            .iter()
            .map(|row| {
                row.chars()
                    .skip(from)
                    .take(to.saturating_sub(from))
                    // A border is not text: leaving box-drawing characters in
                    // would break every sentence that wraps beside one.
                    .map(|character| if is_border(character) { ' ' } else { character })
                    .collect::<String>()
            })
            .flat_map(|slice| {
                slice
                    .split_whitespace()
                    .map(str::to_owned)
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// True when `needle` reads within the given column slice.
    pub fn contains_in_column(&self, from: u16, to: u16, needle: &str) -> bool {
        let wanted = needle.split_whitespace().collect::<Vec<_>>().join(" ");
        self.column_text(from, to).contains(&wanted)
    }

    /// Index of the first row containing `needle`.
    pub fn find_row(&self, needle: &str) -> Option<usize> {
        self.rows().iter().position(|row| row.contains(needle))
    }

    /// True when no row exceeds the terminal width, i.e. nothing was clipped by
    /// the emulator because the interface drew past the edge.
    pub fn fits_width(&self) -> bool {
        self.rows
            .iter()
            .all(|row| row.chars().count() <= self.width as usize)
    }
}

/// Box-drawing and block characters used for borders and rules.
fn is_border(character: char) -> bool {
    matches!(character, '\u{2500}'..='\u{257F}' | '\u{2580}'..='\u{259F}' | '|')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(input: &str, width: u16, height: u16) -> Screen {
        let mut parser = vt100::Parser::new(height, width, 0);
        parser.process(input.as_bytes());
        Screen::from_parser(&parser)
    }

    #[test]
    fn plain_text_lands_on_the_expected_rows() {
        let screen = parse("first\r\nsecond", 20, 4);
        assert_eq!(screen.row(0), "first");
        assert_eq!(screen.row(1), "second");
        assert_eq!(screen.row(2), "");
        assert!(screen.contains("second"));
    }

    #[test]
    fn a_repaint_replaces_earlier_output() {
        // Write, home the cursor, then overwrite: only the final state is visible.
        let screen = parse("stale text\x1b[H\rfresh", 20, 2);
        assert!(screen.contains("fresh"));
        assert!(
            !screen.contains("stale"),
            "an overwritten row must not be reported as visible: {:?}",
            screen.text()
        );
    }

    #[test]
    fn cursor_movement_places_text_where_the_user_sees_it() {
        let screen = parse("\x1b[2;5Hplaced", 20, 4);
        assert_eq!(screen.row(1), "    placed");
    }

    #[test]
    fn styling_is_stripped_from_the_text_model() {
        let screen = parse("\x1b[1;31mred bold\x1b[0m", 20, 2);
        assert_eq!(screen.row(0), "red bold");
    }

    #[test]
    fn row_scoped_matching_does_not_join_unrelated_rows() {
        let screen = parse("Approve\r\naction", 20, 3);
        assert!(!screen.contains("Approve action"));
        assert!(screen.contains_wrapped("Approve action"));
    }

    #[test]
    fn wrapped_matching_normalizes_whitespace() {
        let screen = parse("a long sentence that\r\n  wraps across rows", 24, 3);
        assert!(screen.contains_wrapped("sentence that wraps across rows"));
    }

    #[test]
    fn a_column_slice_reads_independently_of_its_neighbours() {
        // Two columns separated at x = 12; the right one wraps its own sentence.
        let screen = parse(
            "left text   \u{2502} a sentence\r\nmore left   \u{2502} that wraps",
            24,
            2,
        );
        assert!(
            screen.contains_in_column(13, 24, "a sentence that wraps"),
            "column text was {:?}",
            screen.column_text(13, 24)
        );
        assert!(
            !screen.contains_in_column(0, 12, "a sentence"),
            "a column slice must not see its neighbour's text"
        );
        assert!(screen.contains_in_column(0, 12, "left text more left"));
        assert!(
            !screen.column_text(12, 24).contains('\u{2502}'),
            "a border must not appear in flattened column text"
        );
    }

    #[test]
    fn a_column_slice_beyond_the_screen_is_clamped() {
        let screen = parse("abc", 3, 1);
        assert_eq!(screen.column_text(0, 99), "abc");
        assert_eq!(screen.column_text(5, 9), "");
    }

    #[test]
    fn find_row_reports_the_first_match() {
        let screen = parse("one\r\ntwo\r\ntwo", 10, 4);
        assert_eq!(screen.find_row("two"), Some(1));
        assert_eq!(screen.find_row("three"), None);
    }

    #[test]
    fn the_cursor_position_is_reported() {
        let screen = parse("\x1b[3;7H", 20, 5);
        assert_eq!(screen.cursor(), (2, 6));
    }

    #[test]
    fn every_row_fits_the_terminal_width() {
        let screen = parse(&"x".repeat(200), 20, 4);
        assert!(screen.fits_width());
        assert_eq!(screen.width(), 20);
    }
}
