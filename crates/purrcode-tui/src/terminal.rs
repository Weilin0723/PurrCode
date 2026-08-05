//! The workbench terminal surface (PRD §19).
//!
//! PurrCode runs builds, tests and servers in real PTYs. The TUI has to show
//! what those processes actually print, which means interpreting the escape
//! sequences they emit rather than deleting them: a cleared screen, a progress
//! bar that rewrites its line, and a coloured test summary are all invisible to
//! a viewer that strips ANSI and prints the rest.
//!
//! The emulation itself lives in `purrcode-terminal-runtime`, shared with the
//! native GUI, so the same bytes produce the same grid in both clients. This
//! module is the `ratatui` adapter over it plus the tab model: nothing here
//! parses.

use purrcode_terminal_runtime::{
    KeyInput, KeyModifiers, TerminalCell, TerminalColor, TerminalEmulator, TerminalKey,
};
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

/// Render the emulator's visible grid as `ratatui` lines.
///
/// Trailing blank cells are trimmed so a mostly empty screen does not paint a
/// block of background colour across the pane.
pub fn lines(emulator: &TerminalEmulator) -> Vec<Line<'static>> {
    let (rows, _) = emulator.size();
    (0..rows)
        .map(|row| {
            let cells = emulator.row(row);
            let end = cells
                .iter()
                .rposition(|cell| !cell.text.trim().is_empty() || cell.bg != TerminalColor::Default)
                .map_or(0, |position| position + 1);
            let mut spans: Vec<Span<'static>> = Vec::new();
            let mut text = String::new();
            let mut current: Option<Style> = None;
            for cell in &cells[..end] {
                if cell.wide_continuation {
                    continue;
                }
                let style = style_of(cell);
                match current {
                    Some(previous) if previous == style => text.push_str(cell.glyph()),
                    Some(previous) => {
                        spans.push(Span::styled(std::mem::take(&mut text), previous));
                        text.push_str(cell.glyph());
                        current = Some(style);
                    }
                    None => {
                        text.push_str(cell.glyph());
                        current = Some(style);
                    }
                }
            }
            if let Some(style) = current {
                spans.push(Span::styled(text, style));
            }
            Line::from(spans)
        })
        .collect()
}

fn style_of(cell: &TerminalCell) -> Style {
    let (fg, bg) = if cell.attrs.inverse {
        (
            color(cell.bg).or(Some(Color::Black)),
            color(cell.fg).or(Some(Color::Gray)),
        )
    } else {
        (color(cell.fg), color(cell.bg))
    };
    let mut style = Style::default();
    if let Some(fg) = fg {
        style = style.fg(fg);
    }
    if let Some(bg) = bg {
        style = style.bg(bg);
    }
    if cell.attrs.bold {
        style = style.add_modifier(Modifier::BOLD);
    }
    if cell.attrs.italic {
        style = style.add_modifier(Modifier::ITALIC);
    }
    if cell.attrs.underline {
        style = style.add_modifier(Modifier::UNDERLINED);
    }
    if cell.attrs.dim {
        style = style.add_modifier(Modifier::DIM);
    }
    style
}

/// Map a terminal colour onto the palette the user's own terminal is using.
///
/// `Ansi` stays indexed rather than becoming RGB: the host terminal's theme is
/// the one the user chose, and overriding it would make PurrCode the only
/// application on their screen that ignores it.
fn color(color: TerminalColor) -> Option<Color> {
    match color {
        TerminalColor::Default => None,
        TerminalColor::Ansi(index) => Some(match index {
            0 => Color::Black,
            1 => Color::Red,
            2 => Color::Green,
            3 => Color::Yellow,
            4 => Color::Blue,
            5 => Color::Magenta,
            6 => Color::Cyan,
            7 => Color::Gray,
            8 => Color::DarkGray,
            9 => Color::LightRed,
            10 => Color::LightGreen,
            11 => Color::LightYellow,
            12 => Color::LightBlue,
            13 => Color::LightMagenta,
            14 => Color::LightCyan,
            _ => Color::White,
        }),
        TerminalColor::Indexed(index) => Some(Color::Indexed(index)),
        TerminalColor::Rgb(r, g, b) => Some(Color::Rgb(r, g, b)),
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
    pub screen: TerminalEmulator,
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
            screen: TerminalEmulator::new(rows as u16, cols as u16),
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

    /// This tab's grid, ready to render.
    pub fn lines(&self) -> Vec<Line<'static>> {
        lines(&self.screen)
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
///
/// `application_cursor` comes from the emulator, because an editor that asked
/// for SS3 arrows must not receive CSI arrows.
pub fn key_bytes_for(key: crossterm::event::KeyEvent, application_cursor: bool) -> Option<Vec<u8>> {
    use crossterm::event::{KeyCode, KeyModifiers as CrosstermModifiers};
    let modifiers = KeyModifiers {
        ctrl: key.modifiers.contains(CrosstermModifiers::CONTROL),
        alt: key.modifiers.contains(CrosstermModifiers::ALT),
        shift: key.modifiers.contains(CrosstermModifiers::SHIFT),
    };
    let named = |named| Some(KeyInput::Named(named, modifiers));
    let input = match key.code {
        KeyCode::Enter => named(TerminalKey::Enter),
        KeyCode::Backspace => named(TerminalKey::Backspace),
        KeyCode::Tab => named(TerminalKey::Tab),
        KeyCode::BackTab => named(TerminalKey::BackTab),
        KeyCode::Esc => named(TerminalKey::Escape),
        KeyCode::Up => named(TerminalKey::Up),
        KeyCode::Down => named(TerminalKey::Down),
        KeyCode::Right => named(TerminalKey::Right),
        KeyCode::Left => named(TerminalKey::Left),
        KeyCode::Home => named(TerminalKey::Home),
        KeyCode::End => named(TerminalKey::End),
        KeyCode::PageUp => named(TerminalKey::PageUp),
        KeyCode::PageDown => named(TerminalKey::PageDown),
        KeyCode::Insert => named(TerminalKey::Insert),
        KeyCode::Delete => named(TerminalKey::Delete),
        KeyCode::F(number) => named(TerminalKey::Function(number)),
        KeyCode::Char(character) => Some(KeyInput::Char(character, modifiers)),
        _ => None,
    }?;
    purrcode_terminal_runtime::emulator::encode_key(input, application_cursor)
}

/// Encode a key for a terminal in its default mode.
pub fn key_bytes(key: crossterm::event::KeyEvent) -> Option<Vec<u8>> {
    key_bytes_for(key, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen_with(input: &[u8]) -> TerminalEmulator {
        let mut screen = TerminalEmulator::new(6, 20);
        screen.write(input);
        screen
    }

    #[test]
    fn escape_sequences_reach_the_rendered_span() {
        let screen = screen_with(b"\x1b[31mred\x1b[0m done");
        assert_eq!(screen.plain_text(), "red done");
        let line = &lines(&screen)[0];
        assert!(
            line.spans
                .iter()
                .any(|span| span.style.fg == Some(Color::Red) && span.content.contains("red")),
            "colour must reach the rendered span"
        );
    }

    #[test]
    fn truecolour_reaches_the_rendered_span() {
        let screen = screen_with(b"\x1b[38;2;255;0;0mred\x1b[0m");
        assert!(
            lines(&screen)[0]
                .spans
                .iter()
                .any(|span| span.style.fg == Some(Color::Rgb(255, 0, 0)))
        );
    }

    #[test]
    fn a_blank_screen_paints_no_spans() {
        let screen = TerminalEmulator::new(3, 10);
        assert!(
            lines(&screen).iter().all(|line| line.spans.is_empty()),
            "trailing blanks must not paint a rectangle of background"
        );
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
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers as Mods};
        assert_eq!(key_bytes(KeyEvent::from(KeyCode::Enter)).unwrap(), b"\r");
        assert_eq!(key_bytes(KeyEvent::from(KeyCode::Up)).unwrap(), b"\x1b[A");
        assert_eq!(
            key_bytes(KeyEvent::from(KeyCode::Backspace)).unwrap(),
            vec![0x7f]
        );
        assert_eq!(
            key_bytes(KeyEvent::new(KeyCode::Char('c'), Mods::CONTROL)).unwrap(),
            vec![3],
            "Ctrl+C must reach the process as an interrupt"
        );
        assert_eq!(key_bytes(KeyEvent::from(KeyCode::Char('x'))).unwrap(), b"x");
        assert_eq!(
            key_bytes_for(KeyEvent::from(KeyCode::Up), true).unwrap(),
            b"\x1bOA",
            "an editor in application-cursor mode needs SS3 arrows"
        );
    }
}
