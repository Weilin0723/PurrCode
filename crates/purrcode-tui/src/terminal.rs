//! The workbench terminal surface (PRD §19).
//!
//! PurrCode runs builds, tests and servers in real PTYs. The TUI has to show
//! what those processes actually print, which means interpreting the escape
//! sequences they emit rather than deleting them: a cleared screen, a progress
//! bar that rewrites its line, and a coloured test summary are all invisible to
//! a viewer that strips ANSI and prints the rest.
//!
//! [`Screen`] keeps a cell grid and applies the sequences a build or shell
//! session emits. It deliberately mirrors the Studio emulator in
//! `crates/studio-shell/assets/term.js`, so the same output looks the same in
//! both clients (PRD §25 parity).
//!
//! Everything here is pure: bytes in, cells out. The daemon owns the PTY.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

/// Terminal tabs PurrCode opens by purpose (PRD §19.2). A tab is created when
/// it is first needed, never up front.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TabKind {
    Agent,
    Build,
    Tests,
    Server,
    Shell,
}

impl TabKind {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Agent => "Agent",
            Self::Build => "Build",
            Self::Tests => "Tests",
            Self::Server => "Server",
            Self::Shell => "Shell",
        }
    }

    /// Classify a terminal from the command it runs, so the tab strip reads as
    /// purposes rather than as a row of UUIDs.
    pub fn classify(command: &str) -> Self {
        let lower = command.to_ascii_lowercase();
        if lower.contains("test") || lower.contains("pytest") || lower.contains("jest") {
            Self::Tests
        } else if lower.contains("build") || lower.contains("compile") || lower.contains("cargo b")
        {
            Self::Build
        } else if lower.contains("serve") || lower.contains("dev") || lower.contains("run ") {
            Self::Server
        } else if lower.is_empty() {
            Self::Shell
        } else {
            Self::Agent
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Cell {
    ch: char,
    fg: Option<Color>,
    bg: Option<Color>,
    bold: bool,
    inverse: bool,
    underline: bool,
}

impl Cell {
    const BLANK: Cell = Cell {
        ch: ' ',
        fg: None,
        bg: None,
        bold: false,
        inverse: false,
        underline: false,
    };

    fn style(self) -> Style {
        let (fg, bg) = if self.inverse {
            (
                self.bg.or(Some(Color::Black)),
                self.fg.or(Some(Color::Gray)),
            )
        } else {
            (self.fg, self.bg)
        };
        let mut style = Style::default();
        if let Some(fg) = fg {
            style = style.fg(fg);
        }
        if let Some(bg) = bg {
            style = style.bg(bg);
        }
        if self.bold {
            style = style.add_modifier(Modifier::BOLD);
        }
        if self.underline {
            style = style.add_modifier(Modifier::UNDERLINED);
        }
        style
    }

    fn same_style(self, other: Cell) -> bool {
        self.fg == other.fg
            && self.bg == other.bg
            && self.bold == other.bold
            && self.inverse == other.inverse
            && self.underline == other.underline
    }
}

/// An emulated terminal screen.
pub struct Screen {
    rows: usize,
    cols: usize,
    cells: Vec<Vec<Cell>>,
    cursor_row: usize,
    cursor_col: usize,
    style: Cell,
    /// Bytes of an escape sequence split across chunk boundaries.
    pending: Vec<u8>,
}

impl Screen {
    pub fn new(rows: usize, cols: usize) -> Self {
        let rows = rows.max(1);
        let cols = cols.max(1);
        Self {
            rows,
            cols,
            cells: vec![vec![Cell::BLANK; cols]; rows],
            cursor_row: 0,
            cursor_col: 0,
            style: Cell::BLANK,
            pending: Vec::new(),
        }
    }

    pub fn size(&self) -> (usize, usize) {
        (self.rows, self.cols)
    }

    pub fn resize(&mut self, rows: usize, cols: usize) {
        let rows = rows.max(1);
        let cols = cols.max(1);
        if rows == self.rows && cols == self.cols {
            return;
        }
        let mut cells = vec![vec![Cell::BLANK; cols]; rows];
        for (row_index, row) in self.cells.iter().take(rows).enumerate() {
            for (col_index, cell) in row.iter().take(cols).enumerate() {
                cells[row_index][col_index] = *cell;
            }
        }
        self.cells = cells;
        self.rows = rows;
        self.cols = cols;
        self.cursor_row = self.cursor_row.min(rows - 1);
        self.cursor_col = self.cursor_col.min(cols - 1);
    }

    pub fn clear(&mut self) {
        self.cells = vec![vec![Cell::BLANK; self.cols]; self.rows];
        self.cursor_row = 0;
        self.cursor_col = 0;
        self.style = Cell::BLANK;
        self.pending.clear();
    }

    /// Apply a chunk of PTY output.
    pub fn write(&mut self, chunk: &[u8]) {
        let mut input = std::mem::take(&mut self.pending);
        input.extend_from_slice(chunk);
        let mut index = 0;
        while index < input.len() {
            if input[index] == 0x1b {
                match self.escape(&input, index) {
                    Some(next) => index = next,
                    None => {
                        // Incomplete sequence: hold it for the next chunk rather
                        // than printing the escape as literal text.
                        self.pending = input[index..].to_vec();
                        break;
                    }
                }
                continue;
            }
            index += self.text(&input, index);
        }
        // A sequence that never terminates must not grow without bound.
        if self.pending.len() > 512 {
            self.pending.clear();
        }
    }

    /// Consume one printable character (decoding UTF-8) and return its length.
    fn text(&mut self, input: &[u8], index: usize) -> usize {
        let byte = input[index];
        if byte < 0x80 {
            self.printable(byte as char);
            return 1;
        }
        let width = utf8_width(byte);
        if index + width > input.len() {
            // Hold the incomplete character for the next chunk.
            self.pending = input[index..].to_vec();
            return input.len() - index;
        }
        match std::str::from_utf8(&input[index..index + width]) {
            Ok(text) => {
                for character in text.chars() {
                    self.printable(character);
                }
            }
            // Invalid UTF-8 from a process is data, not a crash: show the
            // replacement character and keep going.
            Err(_) => self.printable('\u{fffd}'),
        }
        width
    }

    fn escape(&mut self, input: &[u8], start: usize) -> Option<usize> {
        let next = *input.get(start + 1)?;
        match next {
            b'[' => self.csi(input, start),
            b']' => osc_end(input, start),
            // Reverse index scrolls the screen down one line.
            b'M' => {
                self.scroll_down(1);
                Some(start + 2)
            }
            b'c' => {
                self.clear();
                Some(start + 2)
            }
            _ => Some(start + 2),
        }
    }

    fn csi(&mut self, input: &[u8], start: usize) -> Option<usize> {
        let mut index = start + 2;
        let mut parameters = Vec::new();
        while index < input.len() && matches!(input[index], b'0'..=b'9' | b';' | b'?' | b' ') {
            parameters.push(input[index]);
            index += 1;
        }
        let final_byte = *input.get(index)?;
        let private = parameters.first() == Some(&b'?');
        let numbers: Vec<Option<u16>> = String::from_utf8_lossy(&parameters)
            .replace(['?', ' '], "")
            .split(';')
            .map(|value| value.parse().ok())
            .collect();
        let at = |position: usize, fallback: u16| {
            numbers.get(position).copied().flatten().unwrap_or(fallback)
        };

        match final_byte {
            b'A' => self.cursor_row = self.cursor_row.saturating_sub(at(0, 1) as usize),
            b'B' => self.cursor_row = self.clamp_row(self.cursor_row + at(0, 1) as usize),
            b'C' => self.cursor_col = self.clamp_col(self.cursor_col + at(0, 1) as usize),
            b'D' => self.cursor_col = self.cursor_col.saturating_sub(at(0, 1) as usize),
            b'E' => {
                self.cursor_row = self.clamp_row(self.cursor_row + at(0, 1) as usize);
                self.cursor_col = 0;
            }
            b'F' => {
                self.cursor_row = self.cursor_row.saturating_sub(at(0, 1) as usize);
                self.cursor_col = 0;
            }
            b'G' => self.cursor_col = self.clamp_col(at(0, 1).saturating_sub(1) as usize),
            b'H' | b'f' => {
                self.cursor_row = self.clamp_row(at(0, 1).saturating_sub(1) as usize);
                self.cursor_col = self.clamp_col(at(1, 1).saturating_sub(1) as usize);
            }
            b'J' => self.erase_display(at(0, 0)),
            b'K' => self.erase_line(at(0, 0)),
            b'L' => self.insert_lines(at(0, 1) as usize),
            b'M' => self.delete_lines(at(0, 1) as usize),
            b'P' => self.delete_characters(at(0, 1) as usize),
            b'S' => self.scroll_up(at(0, 1) as usize),
            b'T' => self.scroll_down(at(0, 1) as usize),
            b'X' => self.erase_characters(at(0, 1) as usize),
            b'd' => self.cursor_row = self.clamp_row(at(0, 1).saturating_sub(1) as usize),
            b'm' if !private => self.apply_style(&numbers),
            _ => {}
        }
        Some(index + 1)
    }

    fn printable(&mut self, character: char) {
        match character {
            '\n' => {
                self.line_feed();
                return;
            }
            '\r' => {
                self.cursor_col = 0;
                return;
            }
            '\u{8}' => {
                self.cursor_col = self.cursor_col.saturating_sub(1);
                return;
            }
            '\t' => {
                self.cursor_col = self.clamp_col((self.cursor_col / 8 + 1) * 8);
                return;
            }
            c if (c as u32) < 0x20 || c == '\u{7f}' => return,
            _ => {}
        }
        if self.cursor_col >= self.cols {
            self.cursor_col = 0;
            self.line_feed();
        }
        self.cells[self.cursor_row][self.cursor_col] = Cell {
            ch: character,
            ..self.style
        };
        self.cursor_col += 1;
    }

    fn clamp_row(&self, row: usize) -> usize {
        row.min(self.rows - 1)
    }

    fn clamp_col(&self, col: usize) -> usize {
        col.min(self.cols - 1)
    }

    fn line_feed(&mut self) {
        if self.cursor_row + 1 >= self.rows {
            self.scroll_up(1);
        } else {
            self.cursor_row += 1;
        }
    }

    fn blank_row(&self) -> Vec<Cell> {
        vec![Cell::BLANK; self.cols]
    }

    fn scroll_up(&mut self, count: usize) {
        for _ in 0..count.min(self.rows) {
            self.cells.remove(0);
            let blank = self.blank_row();
            self.cells.push(blank);
        }
    }

    fn scroll_down(&mut self, count: usize) {
        for _ in 0..count.min(self.rows) {
            self.cells.pop();
            let blank = self.blank_row();
            self.cells.insert(0, blank);
        }
    }

    fn insert_lines(&mut self, count: usize) {
        for _ in 0..count.min(self.rows) {
            self.cells.pop();
            let blank = self.blank_row();
            self.cells.insert(self.cursor_row, blank);
        }
    }

    fn delete_lines(&mut self, count: usize) {
        for _ in 0..count.min(self.rows) {
            self.cells.remove(self.cursor_row);
            let blank = self.blank_row();
            self.cells.push(blank);
        }
    }

    fn delete_characters(&mut self, count: usize) {
        let row = &mut self.cells[self.cursor_row];
        for _ in 0..count.min(row.len() - self.cursor_col) {
            row.remove(self.cursor_col);
            row.push(Cell::BLANK);
        }
    }

    fn erase_characters(&mut self, count: usize) {
        let end = (self.cursor_col + count).min(self.cols);
        for col in self.cursor_col..end {
            self.cells[self.cursor_row][col] = Cell::BLANK;
        }
    }

    fn erase_line(&mut self, mode: u16) {
        let (from, to) = match mode {
            1 => (0, self.cursor_col + 1),
            2 => (0, self.cols),
            _ => (self.cursor_col, self.cols),
        };
        for col in from..to.min(self.cols) {
            self.cells[self.cursor_row][col] = Cell::BLANK;
        }
    }

    fn erase_display(&mut self, mode: u16) {
        match mode {
            2 | 3 => {
                self.cells = vec![vec![Cell::BLANK; self.cols]; self.rows];
            }
            1 => {
                for row in 0..self.cursor_row {
                    self.cells[row] = self.blank_row();
                }
                self.erase_line(1);
            }
            _ => {
                self.erase_line(0);
                for row in self.cursor_row + 1..self.rows {
                    self.cells[row] = self.blank_row();
                }
            }
        }
    }

    fn apply_style(&mut self, numbers: &[Option<u16>]) {
        let values: Vec<u16> = if numbers.iter().all(Option::is_none) {
            vec![0]
        } else {
            numbers.iter().map(|value| value.unwrap_or(0)).collect()
        };
        let mut index = 0;
        while index < values.len() {
            let code = values[index];
            match code {
                0 => self.style = Cell::BLANK,
                1 => self.style.bold = true,
                4 => self.style.underline = true,
                7 => self.style.inverse = true,
                22 => self.style.bold = false,
                24 => self.style.underline = false,
                27 => self.style.inverse = false,
                30..=37 => self.style.fg = Some(basic_color(code - 30)),
                90..=97 => self.style.fg = Some(bright_color(code - 90)),
                40..=47 => self.style.bg = Some(basic_color(code - 40)),
                100..=107 => self.style.bg = Some(bright_color(code - 100)),
                39 => self.style.fg = None,
                49 => self.style.bg = None,
                38 | 48 => {
                    // 256-colour and truecolour selectors carry their own
                    // parameters; consume them so they never reach the screen.
                    let target_is_foreground = code == 38;
                    match values.get(index + 1) {
                        Some(5) => {
                            let color = values.get(index + 2).map(|value| indexed_color(*value));
                            if target_is_foreground {
                                self.style.fg = color;
                            } else {
                                self.style.bg = color;
                            }
                            index += 2;
                        }
                        Some(2) => {
                            let color = match values.get(index + 2..index + 5) {
                                Some([r, g, b]) => Some(Color::Rgb(*r as u8, *g as u8, *b as u8)),
                                _ => None,
                            };
                            if target_is_foreground {
                                self.style.fg = color;
                            } else {
                                self.style.bg = color;
                            }
                            index += 4;
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
            index += 1;
        }
    }

    /// The screen as styled ratatui lines, trailing blanks trimmed so a mostly
    /// empty screen does not paint a block of background colour.
    pub fn lines(&self) -> Vec<Line<'static>> {
        self.cells
            .iter()
            .map(|row| {
                let end = row
                    .iter()
                    .rposition(|cell| cell.ch != ' ' || cell.bg.is_some())
                    .map_or(0, |position| position + 1);
                let mut spans: Vec<Span<'static>> = Vec::new();
                let mut text = String::new();
                let mut current: Option<Cell> = None;
                for cell in &row[..end] {
                    match current {
                        Some(style) if style.same_style(*cell) => text.push(cell.ch),
                        Some(style) => {
                            spans.push(Span::styled(std::mem::take(&mut text), style.style()));
                            text.push(cell.ch);
                            current = Some(*cell);
                        }
                        None => {
                            text.push(cell.ch);
                            current = Some(*cell);
                        }
                    }
                }
                if let Some(style) = current {
                    spans.push(Span::styled(text, style.style()));
                }
                Line::from(spans)
            })
            .collect()
    }

    /// Plain text of the screen. Used by tests and by accessibility output,
    /// where styling is not available.
    pub fn plain_text(&self) -> String {
        self.cells
            .iter()
            .map(|row| {
                let row: String = row.iter().map(|cell| cell.ch).collect();
                row.trim_end().to_owned()
            })
            .collect::<Vec<_>>()
            .join("\n")
            .trim_end()
            .to_owned()
    }
}

fn utf8_width(byte: u8) -> usize {
    match byte {
        0x00..=0x7f => 1,
        0xc0..=0xdf => 2,
        0xe0..=0xef => 3,
        0xf0..=0xf7 => 4,
        _ => 1,
    }
}

fn osc_end(input: &[u8], start: usize) -> Option<usize> {
    let mut index = start + 2;
    while index < input.len() {
        if input[index] == 0x07 {
            return Some(index + 1);
        }
        if input[index] == 0x1b && input.get(index + 1) == Some(&b'\\') {
            return Some(index + 2);
        }
        index += 1;
    }
    None
}

fn basic_color(index: u16) -> Color {
    match index {
        0 => Color::Black,
        1 => Color::Red,
        2 => Color::Green,
        3 => Color::Yellow,
        4 => Color::Blue,
        5 => Color::Magenta,
        6 => Color::Cyan,
        _ => Color::Gray,
    }
}

fn bright_color(index: u16) -> Color {
    match index {
        0 => Color::DarkGray,
        1 => Color::LightRed,
        2 => Color::LightGreen,
        3 => Color::LightYellow,
        4 => Color::LightBlue,
        5 => Color::LightMagenta,
        6 => Color::LightCyan,
        _ => Color::White,
    }
}

fn indexed_color(index: u16) -> Color {
    match index {
        0..=7 => basic_color(index),
        8..=15 => bright_color(index - 8),
        _ => Color::Indexed(index.min(255) as u8),
    }
}

/// One terminal the workbench is showing.
pub struct TerminalTab {
    pub terminal_id: String,
    pub kind: TabKind,
    pub alive: bool,
    pub generation: u64,
    /// Byte offset already applied to `screen`.
    pub offset: u64,
    pub screen: Screen,
    /// Set when the daemon reported that output was discarded before we read it.
    pub lost_output: bool,
}

impl TerminalTab {
    pub fn new(terminal_id: String, kind: TabKind, rows: usize, cols: usize) -> Self {
        Self {
            terminal_id,
            kind,
            alive: true,
            generation: 0,
            offset: 0,
            screen: Screen::new(rows, cols),
            lost_output: false,
        }
    }

    pub fn title(&self) -> String {
        if self.alive {
            self.kind.label().to_owned()
        } else {
            format!("{} (exited)", self.kind.label())
        }
    }
}

/// The workbench terminal pane: a set of tabs and which one has focus.
#[derive(Default)]
pub struct TerminalPane {
    pub tabs: Vec<TerminalTab>,
    pub selected: usize,
}

impl TerminalPane {
    pub fn active(&self) -> Option<&TerminalTab> {
        self.tabs.get(self.selected)
    }

    pub fn active_mut(&mut self) -> Option<&mut TerminalTab> {
        self.tabs.get_mut(self.selected)
    }

    pub fn next_tab(&mut self) {
        if !self.tabs.is_empty() {
            self.selected = (self.selected + 1) % self.tabs.len();
        }
    }

    pub fn previous_tab(&mut self) {
        if !self.tabs.is_empty() {
            self.selected = (self.selected + self.tabs.len() - 1) % self.tabs.len();
        }
    }

    pub fn find(&mut self, terminal_id: &str) -> Option<&mut TerminalTab> {
        self.tabs
            .iter_mut()
            .find(|tab| tab.terminal_id == terminal_id)
    }

    /// Tab strip text. Always includes the position, so the active tab is
    /// identifiable without colour.
    pub fn tab_strip(&self) -> String {
        if self.tabs.is_empty() {
            return "No terminal".to_owned();
        }
        self.tabs
            .iter()
            .enumerate()
            .map(|(index, tab)| {
                if index == self.selected {
                    format!("[{}]", tab.title())
                } else {
                    format!(" {} ", tab.title())
                }
            })
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// Encode a key press as the bytes a PTY expects.
pub fn key_bytes(key: crossterm::event::KeyEvent) -> Option<Vec<u8>> {
    use crossterm::event::{KeyCode, KeyModifiers};
    let bytes = match key.code {
        KeyCode::Enter => b"\r".to_vec(),
        KeyCode::Backspace => vec![0x7f],
        KeyCode::Tab => b"\t".to_vec(),
        KeyCode::Up => b"\x1b[A".to_vec(),
        KeyCode::Down => b"\x1b[B".to_vec(),
        KeyCode::Right => b"\x1b[C".to_vec(),
        KeyCode::Left => b"\x1b[D".to_vec(),
        KeyCode::Home => b"\x1b[H".to_vec(),
        KeyCode::End => b"\x1b[F".to_vec(),
        KeyCode::PageUp => b"\x1b[5~".to_vec(),
        KeyCode::PageDown => b"\x1b[6~".to_vec(),
        KeyCode::Delete => b"\x1b[3~".to_vec(),
        KeyCode::Char(character) => {
            if key.modifiers.contains(KeyModifiers::CONTROL) {
                let upper = character.to_ascii_uppercase();
                if upper.is_ascii_uppercase() {
                    vec![upper as u8 - 64]
                } else {
                    return None;
                }
            } else {
                character.to_string().into_bytes()
            }
        }
        _ => return None,
    };
    Some(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen_with(input: &[u8]) -> Screen {
        let mut screen = Screen::new(6, 20);
        screen.write(input);
        screen
    }

    #[test]
    fn plain_output_lands_on_the_screen() {
        let screen = screen_with(b"hello\r\nworld\r\n");
        assert_eq!(screen.plain_text(), "hello\nworld");
    }

    #[test]
    fn a_bare_line_feed_does_not_return_the_carriage() {
        // Strict VT semantics. A PTY translates \n to \r\n itself (ONLCR), so
        // resetting the column here would instead corrupt the output of any
        // program that moves the cursor down deliberately.
        let screen = screen_with(b"ab\ncd");
        assert_eq!(screen.plain_text(), "ab\n  cd");
    }

    #[test]
    fn escape_sequences_are_interpreted_not_printed() {
        let screen = screen_with(b"\x1b[31mred\x1b[0m done");
        assert_eq!(
            screen.plain_text(),
            "red done",
            "the sequence must colour the text, not appear in it"
        );
        let line = &screen.lines()[0];
        assert!(
            line.spans
                .iter()
                .any(|span| span.style.fg == Some(Color::Red) && span.content.contains("red")),
            "colour must reach the rendered span"
        );
    }

    #[test]
    fn carriage_return_rewrites_a_progress_line() {
        // A progress bar rewrites one line rather than printing many.
        let screen = screen_with(b"  0%\r 50%\r100%");
        assert_eq!(screen.plain_text(), "100%");
    }

    #[test]
    fn clear_screen_actually_clears() {
        let screen = screen_with(b"stale output\x1b[2J\x1b[Hfresh");
        assert_eq!(screen.plain_text(), "fresh");
    }

    #[test]
    fn cursor_addressing_places_text() {
        let mut screen = Screen::new(4, 20);
        screen.write(b"\x1b[3;5Hplaced");
        assert_eq!(screen.plain_text(), "\n\n    placed");
    }

    #[test]
    fn erase_to_end_of_line_removes_the_rest() {
        let screen = screen_with(b"keepDROP\x1b[5D\x1b[K");
        assert_eq!(screen.plain_text(), "kee");
    }

    #[test]
    fn output_scrolls_when_it_passes_the_last_row() {
        let mut screen = Screen::new(3, 10);
        screen.write(b"one\r\ntwo\r\nthree\r\nfour\r\n");
        assert_eq!(screen.plain_text(), "three\nfour");
    }

    #[test]
    fn an_escape_split_across_chunks_is_not_printed_as_text() {
        let mut screen = Screen::new(3, 20);
        screen.write(b"a\x1b[3");
        screen.write(b"1mred");
        assert_eq!(screen.plain_text(), "ared");
        assert!(screen.lines()[0]
            .spans
            .iter()
            .any(|span| span.style.fg == Some(Color::Red)));
    }

    #[test]
    fn utf8_split_across_chunks_survives() {
        let mut screen = Screen::new(2, 10);
        let bytes = "é".as_bytes();
        screen.write(&bytes[..1]);
        screen.write(&bytes[1..]);
        assert_eq!(screen.plain_text(), "é");
    }

    #[test]
    fn invalid_utf8_does_not_panic() {
        let screen = screen_with(&[0xff, 0xfe, b'o', b'k']);
        assert!(screen.plain_text().ends_with("ok"));
    }

    #[test]
    fn window_title_sequences_never_reach_the_screen() {
        let screen = screen_with(b"\x1b]0;my title\x07visible");
        assert_eq!(screen.plain_text(), "visible");
    }

    #[test]
    fn truecolour_parameters_are_consumed() {
        let screen = screen_with(b"\x1b[38;2;255;0;0mred\x1b[0m");
        assert_eq!(screen.plain_text(), "red");
        assert!(screen.lines()[0]
            .spans
            .iter()
            .any(|span| span.style.fg == Some(Color::Rgb(255, 0, 0))));
    }

    #[test]
    fn resize_keeps_the_visible_content() {
        let mut screen = Screen::new(4, 20);
        screen.write(b"kept\n");
        screen.resize(8, 40);
        assert_eq!(screen.size(), (8, 40));
        assert_eq!(screen.plain_text(), "kept");
    }

    #[test]
    fn tabs_are_named_by_purpose_not_by_identifier() {
        assert_eq!(TabKind::classify("cargo test --workspace"), TabKind::Tests);
        assert_eq!(TabKind::classify("cargo build"), TabKind::Build);
        assert_eq!(TabKind::classify("npm run dev"), TabKind::Server);
        assert_eq!(TabKind::classify(""), TabKind::Shell);
    }

    #[test]
    fn the_tab_strip_marks_the_active_tab_without_colour() {
        let mut pane = TerminalPane::default();
        pane.tabs
            .push(TerminalTab::new("a".into(), TabKind::Build, 4, 10));
        pane.tabs
            .push(TerminalTab::new("b".into(), TabKind::Tests, 4, 10));
        assert_eq!(pane.tab_strip(), "[Build]  Tests ");
        pane.next_tab();
        assert_eq!(pane.tab_strip(), " Build  [Tests]");
        pane.next_tab();
        assert_eq!(pane.selected, 0, "tab cycling wraps");
        pane.previous_tab();
        assert_eq!(pane.selected, 1);
    }

    #[test]
    fn an_exited_terminal_says_so_in_its_tab() {
        let mut tab = TerminalTab::new("a".into(), TabKind::Build, 4, 10);
        assert_eq!(tab.title(), "Build");
        tab.alive = false;
        assert_eq!(tab.title(), "Build (exited)");
    }

    #[test]
    fn keys_encode_the_bytes_a_pty_expects() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        assert_eq!(key_bytes(KeyEvent::from(KeyCode::Enter)).unwrap(), b"\r");
        assert_eq!(key_bytes(KeyEvent::from(KeyCode::Up)).unwrap(), b"\x1b[A");
        assert_eq!(
            key_bytes(KeyEvent::from(KeyCode::Backspace)).unwrap(),
            vec![0x7f]
        );
        assert_eq!(
            key_bytes(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)).unwrap(),
            vec![3],
            "Ctrl+C must reach the process as an interrupt"
        );
        assert_eq!(key_bytes(KeyEvent::from(KeyCode::Char('x'))).unwrap(), b"x");
    }
}
