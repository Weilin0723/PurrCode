//! A terminal emulator that is not tied to any UI toolkit.
//!
//! # Why this exists
//!
//! A PTY is not a stream of lines. It is a stream of instructions that move a
//! cursor around a grid, repaint regions, switch to an alternate screen and
//! change colours. Anything that renders those bytes as text — stripping the
//! escape sequences, or worse printing them — is a log viewer, not a terminal:
//! `top`, `vim`, a progress bar and `clear` all break. The v1.0 Terminal PRD
//! §47 lists "ANSI is stripped" and "terminal is a text widget" as release
//! blockers for exactly this reason.
//!
//! # Why vt100
//!
//! PRD §5 requires evaluating a mature Rust parser rather than hand-rolling
//! one. Two candidates were viable:
//!
//! - **`vt100`** — MIT, four small dependencies (`vte`, `unicode-width`,
//!   `itoa`, `log`). Pure `bytes in → grid out`: it has no opinion about where
//!   the bytes came from.
//! - **`alacritty_terminal` 0.26** — Apache-2.0, MSRV 1.85. Excellent
//!   emulation, but it ships its own `tty` module, event loop and config model
//!   and expects to own the pseudo-terminal.
//!
//! PurrCode already owns the PTY in [`crate::TerminalRuntime`], and clients
//! (the TUI, the native GUI) are *stateless renderers* that receive incremental
//! byte chunks over the daemon API. A parser that owns a PTY is the wrong shape
//! for that; a parser that owns nothing is exactly right. Hence `vt100`.
//!
//! The pin is 0.16 and not the older 0.15, which is worth recording because
//! 0.15 resolves more easily: `visible_rows` in 0.15 computes
//! `rows_len - scrollback_offset` without saturating, so scrolling back further
//! than the terminal is tall panics inside the parser. That is every real use
//! of scrollback. 0.16 fixes it, requires `unicode-width` 0.2.1, and therefore
//! required moving the workspace from `ratatui` 0.29 (which pinned
//! `unicode-width` to exactly 0.2.0) to 0.30, which no longer pins it.
//!
//! # Why a wrapper rather than using vt100 directly
//!
//! Two reasons. Toolkit neutrality: the TUI renders with `ratatui` and the GUI
//! with `egui`, so a colour type from either would leak the wrong dependency
//! into the other. And PTY realism: keyboard encoding depends on emulator state
//! (application-cursor mode changes what an arrow key sends, bracketed paste
//! changes what a paste sends), so the encoder has to live next to the grid, not
//! in each client.

use std::collections::VecDeque;

/// How many lines of history a terminal keeps in memory.
///
/// PRD §40 asks for 10k lines smooth and 50k usable, and for the in-memory
/// scrollback to be bounded — the durable transcript in [`crate::TerminalRuntime`]
/// is the long-term record, not this grid.
pub const DEFAULT_SCROLLBACK_LINES: usize = 10_000;

/// The maximum scrollback a caller may request.
pub const MAX_SCROLLBACK_LINES: usize = 50_000;

/// A colour as the terminal expresses it.
///
/// Deliberately *not* an RGB triple: `Ansi(1)` means "whatever this theme calls
/// red", and PRD §34 requires the dark, light and high-contrast themes to each
/// answer that question differently. Flattening to RGB here would hard-code one
/// theme's palette into the parser.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum TerminalColor {
    /// The theme's default foreground or background.
    #[default]
    Default,
    /// One of the 16 palette colours (0–7 normal, 8–15 bright).
    Ansi(u8),
    /// A 256-colour palette index (16–255: the cube and the greyscale ramp).
    Indexed(u8),
    /// A truecolour value the program asked for by number.
    Rgb(u8, u8, u8),
}

/// The rendering attributes of one cell.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct CellAttrs {
    pub bold: bool,
    pub dim: bool,
    pub italic: bool,
    pub underline: bool,
    pub inverse: bool,
}

/// One character cell.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TerminalCell {
    /// The grapheme in this cell. Empty means "blank"; more than one `char`
    /// happens with combining marks.
    pub text: String,
    pub fg: TerminalColor,
    pub bg: TerminalColor,
    pub attrs: CellAttrs,
    /// True for the left half of a double-width character.
    pub wide: bool,
    /// True for the right half of a double-width character: it occupies a
    /// column but must not be drawn, or the glyph appears twice.
    pub wide_continuation: bool,
}

impl TerminalCell {
    /// Whether this cell would paint anything at all.
    pub fn is_blank(&self) -> bool {
        self.text.trim().is_empty() && self.bg == TerminalColor::Default && !self.attrs.inverse
    }

    /// The character to draw, or a space for an empty cell.
    pub fn glyph(&self) -> &str {
        if self.text.is_empty() {
            " "
        } else {
            &self.text
        }
    }
}

/// The shape the program asked the cursor to take.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum CursorShape {
    #[default]
    Block,
    Underline,
    Bar,
}

/// Where the cursor is and whether it should be drawn.
///
/// PRD §8: "A shell prompt without a cursor is not considered a functional
/// terminal."
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct CursorState {
    /// Row within the *visible viewport*, or `None` when the user has scrolled
    /// the cursor out of view.
    pub row: Option<u16>,
    pub col: u16,
    pub visible: bool,
    pub shape: CursorShape,
}

/// A point in the terminal's scroll-independent coordinate space.
///
/// Rows are counted from the oldest line still in scrollback, so a selection
/// survives new output arriving underneath it. A viewport row is not usable for
/// this: it means something different one line of output later.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct GridPoint {
    /// Absolute row, counted from the oldest retained line.
    pub row: usize,
    pub col: u16,
}

/// A text selection, from where the drag started to where it is now.
///
/// `anchor` may be after `head` — the user dragged upward — so callers should
/// use [`Selection::ordered`] rather than comparing the fields directly.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Selection {
    pub anchor: GridPoint,
    pub head: GridPoint,
}

impl Selection {
    pub fn new(anchor: GridPoint) -> Self {
        Self {
            anchor,
            head: anchor,
        }
    }

    /// `(start, end)` in reading order.
    pub fn ordered(&self) -> (GridPoint, GridPoint) {
        if self.anchor <= self.head {
            (self.anchor, self.head)
        } else {
            (self.head, self.anchor)
        }
    }

    pub fn is_empty(&self) -> bool {
        self.anchor == self.head
    }

    /// Whether `point` falls inside the selection.
    pub fn contains(&self, point: GridPoint) -> bool {
        let (start, end) = self.ordered();
        point >= start && point < end
    }
}

/// A terminal, as a grid of cells plus the state a renderer and a keyboard
/// encoder need.
///
/// Feed it PTY bytes with [`TerminalEmulator::write`]; ask it what to draw with
/// [`TerminalEmulator::viewport`]; ask it what a key means with
/// [`TerminalEmulator::encode_key`].
pub struct TerminalEmulator {
    parser: vt100::Parser<EmulatorCallbacks>,
    rows: u16,
    cols: u16,
    /// How far the user has scrolled back, in lines. `0` is live output.
    scrollback_offset: usize,
    /// How many lines of history exist above the live grid. Refreshed from the
    /// parser after every write rather than counted here, because the parser is
    /// the only thing that knows when a line left the grid.
    history_len: usize,
    /// Lines of history retained.
    scrollback_limit: usize,
    /// True while the viewport is pinned to the newest output.
    following: bool,
    /// New output arrived while the user was reading history (PRD §11).
    unseen_output: bool,
}

impl TerminalEmulator {
    /// A terminal of `rows` × `cols` retaining [`DEFAULT_SCROLLBACK_LINES`].
    pub fn new(rows: u16, cols: u16) -> Self {
        Self::with_scrollback(rows, cols, DEFAULT_SCROLLBACK_LINES)
    }

    pub fn with_scrollback(rows: u16, cols: u16, scrollback: usize) -> Self {
        let rows = rows.max(1);
        let cols = cols.max(1);
        let scrollback = scrollback.min(MAX_SCROLLBACK_LINES);
        Self {
            parser: vt100::Parser::new_with_callbacks(
                rows,
                cols,
                scrollback,
                EmulatorCallbacks::default(),
            ),
            rows,
            cols,
            scrollback_offset: 0,
            history_len: 0,
            scrollback_limit: scrollback,
            following: true,
            unseen_output: false,
        }
    }

    /// Apply a chunk of PTY output.
    ///
    /// Chunk boundaries are arbitrary: a UTF-8 character or an escape sequence
    /// may be split across two calls. `vt100` holds the partial state, so this
    /// is safe to call with whatever the socket delivered.
    pub fn write(&mut self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        self.parser.process(bytes);
        self.refresh_history();
        if self.following {
            self.parser.screen_mut().set_scrollback(0);
            self.scrollback_offset = 0;
        } else {
            // vt100 nudges its own offset when output arrives under a pinned
            // view, so the lines the user is reading stay where they are. Adopt
            // its answer rather than recomputing one that would disagree.
            self.scrollback_offset = self.parser.screen().scrollback();
            self.unseen_output = true;
        }
    }

    /// Ask the parser how much history it is holding.
    ///
    /// `set_scrollback` clamps to the real history length, so setting it past
    /// the end and reading it back is an exact measurement. vt100 exposes no
    /// direct accessor, and guessing from line feeds would drift the moment a
    /// program used a scroll region.
    fn refresh_history(&mut self) {
        let screen = self.parser.screen_mut();
        let current = screen.scrollback();
        screen.set_scrollback(usize::MAX);
        let length = screen.scrollback();
        screen.set_scrollback(current);
        self.history_len = length;
    }

    /// Discard the grid and history. Used when a terminal restarts in place.
    pub fn reset(&mut self) {
        self.parser = vt100::Parser::new_with_callbacks(
            self.rows,
            self.cols,
            self.scrollback_limit,
            EmulatorCallbacks::default(),
        );
        self.scrollback_offset = 0;
        self.history_len = 0;
        self.following = true;
        self.unseen_output = false;
    }

    pub fn size(&self) -> (u16, u16) {
        (self.rows, self.cols)
    }

    /// Resize the grid. Returns true when the size actually changed, so the
    /// caller can skip a needless PTY `TIOCSWINSZ` round trip (PRD §10).
    pub fn resize(&mut self, rows: u16, cols: u16) -> bool {
        let rows = rows.max(1);
        let cols = cols.max(1);
        if rows == self.rows && cols == self.cols {
            return false;
        }
        self.rows = rows;
        self.cols = cols;
        self.parser.screen_mut().set_size(rows, cols);
        true
    }

    // ── Scrollback ────────────────────────────────────────────────────

    /// How many lines of history exist above the viewport.
    pub fn scrollback_available(&self) -> usize {
        self.history_len
    }

    pub fn scrollback_offset(&self) -> usize {
        self.scrollback_offset
    }

    /// True when the viewport is showing live output.
    pub fn is_following(&self) -> bool {
        self.following
    }

    /// True when output arrived while the user was reading history — the cue
    /// for the "↓ New output" affordance in PRD §11.
    pub fn has_unseen_output(&self) -> bool {
        self.unseen_output && !self.following
    }

    /// Scroll by `lines` (negative scrolls up, into history).
    pub fn scroll_by(&mut self, lines: i64) {
        let target = (self.scrollback_offset as i64 - lines).max(0) as usize;
        self.scroll_to(target);
    }

    /// Scroll so `offset` lines of history sit above the viewport.
    pub fn scroll_to(&mut self, offset: usize) {
        self.parser.screen_mut().set_scrollback(offset);
        self.scrollback_offset = self.parser.screen().scrollback();
        self.refresh_history();
        self.following = self.scrollback_offset == 0;
        if self.following {
            self.unseen_output = false;
        }
    }

    /// Return to live output (PRD §11: clicking "New output").
    pub fn scroll_to_bottom(&mut self) {
        self.scroll_to(0);
    }

    pub fn page_up(&mut self) {
        self.scroll_by(-(self.rows as i64).max(1));
    }

    pub fn page_down(&mut self) {
        self.scroll_by((self.rows as i64).max(1));
    }

    // ── Reading the grid ──────────────────────────────────────────────

    /// The absolute row number of the top visible line.
    pub fn viewport_origin(&self) -> usize {
        self.history_len.saturating_sub(self.scrollback_offset)
    }

    /// The visible grid, row by row.
    ///
    /// Trailing blank cells are kept: a renderer needs the full width to paint
    /// background colour, and a shorter row would make a `bg`-coloured line
    /// stop early.
    pub fn viewport(&self) -> Vec<Vec<TerminalCell>> {
        let screen = self.parser.screen();
        (0..self.rows)
            .map(|row| {
                (0..self.cols)
                    .map(|col| screen.cell(row, col).map(convert_cell).unwrap_or_default())
                    .collect()
            })
            .collect()
    }

    /// One visible row, or an empty vector when out of range.
    pub fn row(&self, row: u16) -> Vec<TerminalCell> {
        if row >= self.rows {
            return Vec::new();
        }
        let screen = self.parser.screen();
        (0..self.cols)
            .map(|col| screen.cell(row, col).map(convert_cell).unwrap_or_default())
            .collect()
    }

    /// Where to draw the cursor, expressed in viewport rows.
    pub fn cursor(&self) -> CursorState {
        let screen = self.parser.screen();
        let (row, col) = screen.cursor_position();
        // The cursor belongs to the live grid. When the user scrolls up it is
        // genuinely off screen, and drawing it at the same row anyway would
        // put a fake cursor in the middle of old output.
        let row = if self.scrollback_offset == 0 {
            Some(row.min(self.rows.saturating_sub(1)))
        } else {
            None
        };
        CursorState {
            row,
            col: col.min(self.cols.saturating_sub(1)),
            visible: !screen.hide_cursor(),
            shape: CursorShape::Block,
        }
    }

    /// The visible text, trailing blanks trimmed. For tests, accessibility and
    /// "copy all".
    pub fn plain_text(&self) -> String {
        (0..self.rows)
            .map(|row| {
                // The right half of a double-width character occupies a column
                // but holds no text. Emitting it as a space turns 世界 into
                // "世 界", which breaks every search over terminal output.
                let line: String = self
                    .row(row)
                    .iter()
                    .filter(|cell| !cell.wide_continuation)
                    .map(|cell| cell.glyph().to_owned())
                    .collect();
                line.trim_end().to_owned()
            })
            .collect::<Vec<_>>()
            .join("\n")
            .trim_end()
            .to_owned()
    }

    /// True while a full-screen program (`vim`, `top`, `less`) owns the screen.
    ///
    /// Callers use this to suppress scrollback: the alternate screen has none,
    /// and letting the wheel scroll it would show stale history behind a
    /// running editor.
    pub fn alternate_screen(&self) -> bool {
        self.parser.screen().alternate_screen()
    }

    pub fn application_cursor(&self) -> bool {
        self.parser.screen().application_cursor()
    }

    pub fn bracketed_paste(&self) -> bool {
        self.parser.screen().bracketed_paste()
    }

    /// The window title the program set, if any (OSC 0/2).
    ///
    /// A shell sets this to the running command, which is the most accurate
    /// tab label available — better than guessing from the spawn arguments,
    /// which stop being true the moment the user runs something else.
    pub fn title(&self) -> Option<&str> {
        self.parser.callbacks().title.as_deref()
    }

    /// True when the program rang the bell since the last check, and clears the
    /// flag. A background tab that finished a long build is worth marking.
    pub fn take_bell(&mut self) -> bool {
        std::mem::take(&mut self.parser.callbacks_mut().bell)
    }

    // ── Selection ─────────────────────────────────────────────────────

    /// Convert a viewport position into a stable absolute point.
    pub fn point_at(&self, viewport_row: u16, col: u16) -> GridPoint {
        GridPoint {
            row: self.viewport_origin() + viewport_row as usize,
            col: col.min(self.cols),
        }
    }

    /// Whether an absolute point currently falls inside the viewport, and where.
    pub fn viewport_row_of(&self, point: GridPoint) -> Option<u16> {
        let origin = self.viewport_origin();
        let row = point.row.checked_sub(origin)?;
        (row < self.rows as usize).then_some(row as u16)
    }

    /// The text covered by `selection`.
    ///
    /// Only the visible portion can be read — history outside the viewport is
    /// not addressable through vt100's cell API — so a selection is captured as
    /// the user drags rather than reconstructed later.
    pub fn selected_text(&self, selection: Selection) -> String {
        let (start, end) = selection.ordered();
        let mut lines: Vec<String> = Vec::new();
        for row in 0..self.rows {
            let point_row = self.viewport_origin() + row as usize;
            if point_row < start.row || point_row > end.row {
                continue;
            }
            let from = if point_row == start.row { start.col } else { 0 };
            let to = if point_row == end.row {
                end.col
            } else {
                self.cols
            };
            let cells = self.row(row);
            let text: String = cells
                .iter()
                .enumerate()
                .filter(|(col, _)| *col >= from as usize && *col < to as usize)
                .filter(|(_, cell)| !cell.wide_continuation)
                .map(|(_, cell)| cell.glyph())
                .collect();
            lines.push(text.trim_end().to_owned());
        }
        lines.join("\n")
    }

    /// Expand a point to the word under it (double-click, PRD §12).
    pub fn word_at(&self, point: GridPoint) -> Option<Selection> {
        let row = self.viewport_row_of(point)?;
        let cells = self.row(row);
        let col = (point.col as usize).min(cells.len().saturating_sub(1));
        if cells.get(col).is_none_or(|cell| !is_word_char(cell)) {
            return None;
        }
        let mut start = col;
        while start > 0 && cells.get(start - 1).is_some_and(is_word_char) {
            start -= 1;
        }
        let mut end = col + 1;
        while cells.get(end).is_some_and(is_word_char) {
            end += 1;
        }
        Some(Selection {
            anchor: GridPoint {
                row: point.row,
                col: start as u16,
            },
            head: GridPoint {
                row: point.row,
                col: end as u16,
            },
        })
    }

    /// Expand a point to its whole line (triple-click).
    pub fn line_at(&self, point: GridPoint) -> Selection {
        Selection {
            anchor: GridPoint {
                row: point.row,
                col: 0,
            },
            head: GridPoint {
                row: point.row,
                col: self.cols,
            },
        }
    }

    // ── Input ─────────────────────────────────────────────────────────

    /// The bytes `key` should send to the PTY, or `None` when the key means
    /// nothing to a terminal.
    ///
    /// Arrow and Home/End encodings depend on application-cursor mode, which is
    /// why this is a method rather than a free function: `readline` and `vim`
    /// ask for different bytes for the same physical key.
    pub fn encode_key(&self, key: KeyInput) -> Option<Vec<u8>> {
        encode_key(key, self.application_cursor())
    }

    /// The bytes a paste of `text` should send.
    ///
    /// When the program enabled bracketed paste it is wrapped in the markers
    /// that tell a shell "this is pasted, do not treat newlines as Enter" —
    /// which is what stops a multi-line paste from executing half a script.
    pub fn encode_paste(&self, text: &str) -> Vec<u8> {
        let cleaned: String = text.replace("\r\n", "\r").replace('\n', "\r");
        if self.bracketed_paste() {
            let mut bytes = b"\x1b[200~".to_vec();
            bytes.extend_from_slice(cleaned.as_bytes());
            bytes.extend_from_slice(b"\x1b[201~");
            bytes
        } else {
            cleaned.into_bytes()
        }
    }
}

/// The escape sequences that do not touch the grid but that the interface
/// still needs: the window title a shell sets to the running command, and the
/// bell a finished build rings.
#[derive(Debug, Default)]
struct EmulatorCallbacks {
    title: Option<String>,
    bell: bool,
}

impl vt100::Callbacks for EmulatorCallbacks {
    fn set_window_title(&mut self, _: &mut vt100::Screen, title: &[u8]) {
        // A title is decoration. Invalid UTF-8 in it is not worth losing the
        // title over, and certainly not worth a panic.
        let title = String::from_utf8_lossy(title).trim().to_owned();
        self.title = (!title.is_empty()).then_some(title);
    }

    fn audible_bell(&mut self, _: &mut vt100::Screen) {
        self.bell = true;
    }

    fn visual_bell(&mut self, _: &mut vt100::Screen) {
        self.bell = true;
    }
}

fn is_word_char(cell: &TerminalCell) -> bool {
    cell.text
        .chars()
        .next()
        .is_some_and(|character| character.is_alphanumeric() || "_-./~:@".contains(character))
}

fn convert_cell(cell: &vt100::Cell) -> TerminalCell {
    TerminalCell {
        text: cell.contents().to_owned(),
        fg: convert_color(cell.fgcolor()),
        bg: convert_color(cell.bgcolor()),
        attrs: CellAttrs {
            bold: cell.bold(),
            dim: cell.dim(),
            italic: cell.italic(),
            underline: cell.underline(),
            inverse: cell.inverse(),
        },
        wide: cell.is_wide(),
        wide_continuation: cell.is_wide_continuation(),
    }
}

fn convert_color(color: vt100::Color) -> TerminalColor {
    match color {
        vt100::Color::Default => TerminalColor::Default,
        vt100::Color::Idx(index) if index < 16 => TerminalColor::Ansi(index),
        vt100::Color::Idx(index) => TerminalColor::Indexed(index),
        vt100::Color::Rgb(r, g, b) => TerminalColor::Rgb(r, g, b),
    }
}

// ---------------------------------------------------------------------------
// Toolkit-neutral key input.
// ---------------------------------------------------------------------------

/// Which named key was pressed. Toolkit-neutral so `crossterm` and `egui` can
/// both map into it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TerminalKey {
    Enter,
    Backspace,
    Tab,
    BackTab,
    Escape,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    Insert,
    Delete,
    Function(u8),
}

/// Which modifiers were held.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct KeyModifiers {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
}

impl KeyModifiers {
    pub const NONE: KeyModifiers = KeyModifiers {
        ctrl: false,
        alt: false,
        shift: false,
    };
    pub const CTRL: KeyModifiers = KeyModifiers {
        ctrl: true,
        alt: false,
        shift: false,
    };

    pub fn any(&self) -> bool {
        self.ctrl || self.alt || self.shift
    }
}

/// A key press: either a named key or a typed character.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum KeyInput {
    Named(TerminalKey, KeyModifiers),
    Char(char, KeyModifiers),
}

impl KeyInput {
    pub fn key(key: TerminalKey) -> Self {
        Self::Named(key, KeyModifiers::NONE)
    }

    pub fn ctrl(character: char) -> Self {
        Self::Char(character, KeyModifiers::CTRL)
    }
}

/// Encode a key press for a PTY.
///
/// `application_cursor` selects between the `CSI`-prefixed and `SS3`-prefixed
/// arrow encodings. Getting this wrong is the classic "arrow keys print `^[[A`
/// in my editor" bug.
pub fn encode_key(key: KeyInput, application_cursor: bool) -> Option<Vec<u8>> {
    let cursor_prefix: &[u8] = if application_cursor {
        b"\x1bO"
    } else {
        b"\x1b["
    };
    let arrow = |final_byte: u8, modifiers: KeyModifiers| -> Vec<u8> {
        if let Some(code) = modifier_code(modifiers) {
            // The modified form is always CSI, never SS3.
            format!("\x1b[1;{code}{}", final_byte as char).into_bytes()
        } else {
            let mut bytes = cursor_prefix.to_vec();
            bytes.push(final_byte);
            bytes
        }
    };

    let bytes = match key {
        KeyInput::Named(named, modifiers) => match named {
            // Ctrl+Enter and friends still send a carriage return; a PTY in
            // ONLCR translates it.
            TerminalKey::Enter => b"\r".to_vec(),
            // DEL, not BS: this is what stty erase expects on macOS and Linux.
            TerminalKey::Backspace if modifiers.alt => vec![0x1b, 0x7f],
            TerminalKey::Backspace => vec![0x7f],
            TerminalKey::Tab => b"\t".to_vec(),
            TerminalKey::BackTab => b"\x1b[Z".to_vec(),
            TerminalKey::Escape => vec![0x1b],
            TerminalKey::Up => arrow(b'A', modifiers),
            TerminalKey::Down => arrow(b'B', modifiers),
            TerminalKey::Right => arrow(b'C', modifiers),
            TerminalKey::Left => arrow(b'D', modifiers),
            TerminalKey::Home => arrow(b'H', modifiers),
            TerminalKey::End => arrow(b'F', modifiers),
            TerminalKey::PageUp => b"\x1b[5~".to_vec(),
            TerminalKey::PageDown => b"\x1b[6~".to_vec(),
            TerminalKey::Insert => b"\x1b[2~".to_vec(),
            TerminalKey::Delete => b"\x1b[3~".to_vec(),
            TerminalKey::Function(number) => function_key(number)?,
        },
        KeyInput::Char(character, modifiers) if modifiers.ctrl => {
            let control = control_byte(character)?;
            if modifiers.alt {
                vec![0x1b, control]
            } else {
                vec![control]
            }
        }
        KeyInput::Char(character, modifiers) if modifiers.alt => {
            let mut bytes = vec![0x1b];
            bytes.extend_from_slice(character.to_string().as_bytes());
            bytes
        }
        KeyInput::Char(character, _) => character.to_string().into_bytes(),
    };
    Some(bytes)
}

/// The xterm modifier parameter: 1 + bit flags.
fn modifier_code(modifiers: KeyModifiers) -> Option<u8> {
    if !modifiers.any() {
        return None;
    }
    let mut code = 1;
    if modifiers.shift {
        code += 1;
    }
    if modifiers.alt {
        code += 2;
    }
    if modifiers.ctrl {
        code += 4;
    }
    Some(code)
}

/// The C0 control byte for `Ctrl+<character>`.
///
/// This is the path Ctrl+C takes to become `SIGINT`, so the mapping has to
/// cover the full `@`..`_` range and not just letters.
fn control_byte(character: char) -> Option<u8> {
    let upper = character.to_ascii_uppercase();
    match upper {
        '@' | ' ' => Some(0),
        'A'..='Z' => Some(upper as u8 - b'A' + 1),
        '[' => Some(0x1b),
        '\\' => Some(0x1c),
        ']' => Some(0x1d),
        '^' => Some(0x1e),
        '_' | '?' => Some(0x1f),
        _ => None,
    }
}

fn function_key(number: u8) -> Option<Vec<u8>> {
    let bytes = match number {
        1 => b"\x1bOP".to_vec(),
        2 => b"\x1bOQ".to_vec(),
        3 => b"\x1bOR".to_vec(),
        4 => b"\x1bOS".to_vec(),
        5 => b"\x1b[15~".to_vec(),
        6 => b"\x1b[17~".to_vec(),
        7 => b"\x1b[18~".to_vec(),
        8 => b"\x1b[19~".to_vec(),
        9 => b"\x1b[20~".to_vec(),
        10 => b"\x1b[21~".to_vec(),
        11 => b"\x1b[23~".to_vec(),
        12 => b"\x1b[24~".to_vec(),
        _ => return None,
    };
    Some(bytes)
}

// ---------------------------------------------------------------------------
// Bounded summarisation for model context (PRD §38).
// ---------------------------------------------------------------------------

/// What a model is allowed to learn about a terminal.
///
/// PRD §38 forbids sending scrollback to a model. This carries the command, how
/// it ended, the lines that look like errors, a short tail, and a pointer to
/// the durable evidence for anything more.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TerminalContextSummary {
    pub command: String,
    pub exit_status: Option<i32>,
    pub key_errors: Vec<String>,
    pub relevant_output: Vec<String>,
    pub omitted_lines: usize,
}

/// How many lines of tail a summary keeps.
const SUMMARY_TAIL_LINES: usize = 20;
/// How many error lines a summary keeps.
const SUMMARY_ERROR_LINES: usize = 10;

impl TerminalContextSummary {
    /// Summarise a transcript for a model.
    ///
    /// Errors are pulled out and de-duplicated first — a build that fails the
    /// same way 200 times is one fact, not 200 — then a bounded tail is kept
    /// for context.
    pub fn from_transcript(command: &str, exit_status: Option<i32>, transcript: &str) -> Self {
        let lines: Vec<&str> = transcript
            .lines()
            .map(str::trim_end)
            .filter(|line| !line.is_empty())
            .collect();

        let mut key_errors: Vec<String> = Vec::new();
        let mut seen: VecDeque<String> = VecDeque::new();
        for line in &lines {
            if !looks_like_error(line) {
                continue;
            }
            let normalized = line.trim().to_owned();
            if seen.contains(&normalized) {
                continue;
            }
            seen.push_back(normalized.clone());
            key_errors.push(normalized);
            if key_errors.len() >= SUMMARY_ERROR_LINES {
                break;
            }
        }

        let tail_start = lines.len().saturating_sub(SUMMARY_TAIL_LINES);
        let relevant_output: Vec<String> = lines[tail_start..]
            .iter()
            .map(|line| (*line).to_owned())
            .collect();

        Self {
            command: command.to_owned(),
            exit_status,
            key_errors,
            omitted_lines: tail_start,
            relevant_output,
        }
    }
}

fn looks_like_error(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    const MARKERS: [&str; 8] = [
        "error",
        "failed",
        "failure",
        "panic",
        "traceback",
        "exception",
        "cannot ",
        "not found",
    ];
    MARKERS.iter().any(|marker| lower.contains(marker))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn emulator_with(bytes: &[u8]) -> TerminalEmulator {
        let mut emulator = TerminalEmulator::with_scrollback(6, 20, 100);
        emulator.write(bytes);
        emulator
    }

    #[test]
    fn plain_output_lands_on_the_grid() {
        let emulator = emulator_with(b"hello\r\nworld\r\n");
        assert_eq!(emulator.plain_text(), "hello\nworld");
    }

    #[test]
    fn escape_sequences_are_interpreted_not_printed() {
        let emulator = emulator_with(b"\x1b[31mred\x1b[0m done");
        assert_eq!(
            emulator.plain_text(),
            "red done",
            "the sequence must colour the text, not appear in it"
        );
        let row = emulator.row(0);
        assert_eq!(row[0].fg, TerminalColor::Ansi(1), "red must reach the cell");
        assert_eq!(row[4].fg, TerminalColor::Default, "reset must take effect");
    }

    #[test]
    fn truecolour_reaches_the_cell() {
        let emulator = emulator_with(b"\x1b[38;2;12;34;56mrgb");
        assert_eq!(emulator.row(0)[0].fg, TerminalColor::Rgb(12, 34, 56));
    }

    #[test]
    fn carriage_return_rewrites_a_progress_line() {
        let emulator = emulator_with(b"  0%\r 50%\r100%");
        assert_eq!(emulator.plain_text(), "100%");
    }

    #[test]
    fn clear_screen_actually_clears() {
        let emulator = emulator_with(b"stale\x1b[2J\x1b[Hfresh");
        assert_eq!(emulator.plain_text(), "fresh");
    }

    #[test]
    fn an_escape_split_across_chunks_is_not_printed_as_text() {
        let mut emulator = TerminalEmulator::new(3, 20);
        emulator.write(b"a\x1b[3");
        emulator.write(b"1mred");
        assert_eq!(emulator.plain_text(), "ared");
        assert_eq!(emulator.row(0)[1].fg, TerminalColor::Ansi(1));
    }

    #[test]
    fn utf8_split_across_chunks_survives() {
        let mut emulator = TerminalEmulator::new(2, 10);
        let bytes = "é".as_bytes();
        emulator.write(&bytes[..1]);
        emulator.write(&bytes[1..]);
        assert_eq!(emulator.plain_text(), "é");
    }

    #[test]
    fn invalid_utf8_does_not_panic() {
        let emulator = emulator_with(&[0xff, 0xfe, b'o', b'k']);
        assert!(emulator.plain_text().contains("ok"));
    }

    #[test]
    fn window_title_sequences_never_reach_the_screen() {
        let emulator = emulator_with(b"\x1b]0;my title\x07visible");
        assert_eq!(emulator.plain_text(), "visible");
        assert_eq!(emulator.title(), Some("my title"));
    }

    #[test]
    fn the_cursor_is_reported_and_can_be_hidden() {
        let mut emulator = TerminalEmulator::new(4, 20);
        emulator.write(b"ab");
        let cursor = emulator.cursor();
        assert_eq!(cursor.row, Some(0));
        assert_eq!(cursor.col, 2);
        assert!(cursor.visible, "a shell prompt must show a cursor");
        emulator.write(b"\x1b[?25l");
        assert!(!emulator.cursor().visible);
    }

    #[test]
    fn a_full_screen_program_is_recognised() {
        let mut emulator = TerminalEmulator::new(4, 20);
        assert!(!emulator.alternate_screen());
        emulator.write(b"\x1b[?1049h");
        assert!(
            emulator.alternate_screen(),
            "vim and top must be distinguishable from ordinary output"
        );
    }

    #[test]
    fn scrolled_output_moves_into_history_and_can_be_read_back() {
        let mut emulator = TerminalEmulator::with_scrollback(3, 10, 50);
        for index in 0..10 {
            emulator.write(format!("line{index}\r\n").as_bytes());
        }
        assert!(emulator.plain_text().contains("line9"));
        assert!(emulator.scrollback_available() >= 5);

        emulator.scroll_by(-5);
        assert!(!emulator.is_following());
        assert!(
            emulator.plain_text().contains("line4"),
            "scrolling up must reveal history, got {:?}",
            emulator.plain_text()
        );

        emulator.scroll_to_bottom();
        assert!(emulator.is_following());
        assert!(emulator.plain_text().contains("line9"));
    }

    #[test]
    fn output_while_scrolled_up_does_not_yank_the_view_to_the_bottom() {
        // PRD §11: "new terminal output must not forcibly jump to bottom".
        let mut emulator = TerminalEmulator::with_scrollback(3, 20, 50);
        for index in 0..10 {
            emulator.write(format!("old{index}\r\n").as_bytes());
        }
        emulator.scroll_by(-4);
        let reading = emulator.plain_text();
        emulator.write(b"brand new output\r\n");
        assert_eq!(
            emulator.plain_text(),
            reading,
            "the lines the user was reading must stay put"
        );
        assert!(
            emulator.has_unseen_output(),
            "the UI needs to know there is something new below"
        );
        emulator.scroll_to_bottom();
        assert!(emulator.plain_text().contains("brand new output"));
        assert!(!emulator.has_unseen_output());
    }

    #[test]
    fn the_cursor_is_not_drawn_over_history() {
        let mut emulator = TerminalEmulator::with_scrollback(3, 10, 50);
        for index in 0..10 {
            emulator.write(format!("l{index}\r\n").as_bytes());
        }
        assert!(emulator.cursor().row.is_some());
        emulator.scroll_by(-3);
        assert_eq!(
            emulator.cursor().row,
            None,
            "a cursor painted into old output is a lie about where typing goes"
        );
    }

    #[test]
    fn resize_reports_whether_the_pty_needs_telling() {
        let mut emulator = TerminalEmulator::new(4, 20);
        assert!(emulator.resize(10, 40));
        assert_eq!(emulator.size(), (10, 40));
        assert!(
            !emulator.resize(10, 40),
            "an unchanged size is not a resize"
        );
    }

    #[test]
    fn selection_reads_back_the_characters_under_it() {
        let mut emulator = TerminalEmulator::new(3, 20);
        emulator.write(b"hello world\r\nsecond line");
        let selection = Selection {
            anchor: emulator.point_at(0, 6),
            head: emulator.point_at(0, 11),
        };
        assert_eq!(emulator.selected_text(selection), "world");

        let across = Selection {
            anchor: emulator.point_at(0, 6),
            head: emulator.point_at(1, 6),
        };
        assert_eq!(emulator.selected_text(across), "world\nsecond");
    }

    #[test]
    fn double_click_selects_a_word_and_a_path() {
        let mut emulator = TerminalEmulator::new(3, 40);
        emulator.write(b"see src/main.rs for detail");
        let selection = emulator
            .word_at(emulator.point_at(0, 6))
            .expect("a word under the pointer");
        assert_eq!(
            emulator.selected_text(selection),
            "src/main.rs",
            "a path is one word: selecting half of it is useless"
        );
        assert!(
            emulator.word_at(emulator.point_at(0, 3)).is_none(),
            "a blank column has no word"
        );
    }

    #[test]
    fn keys_encode_the_bytes_a_pty_expects() {
        let emulator = TerminalEmulator::new(4, 20);
        assert_eq!(
            emulator
                .encode_key(KeyInput::key(TerminalKey::Enter))
                .unwrap(),
            b"\r"
        );
        assert_eq!(
            emulator
                .encode_key(KeyInput::key(TerminalKey::Backspace))
                .unwrap(),
            vec![0x7f]
        );
        assert_eq!(
            emulator
                .encode_key(KeyInput::key(TerminalKey::Tab))
                .unwrap(),
            b"\t"
        );
        assert_eq!(
            emulator.encode_key(KeyInput::key(TerminalKey::Up)).unwrap(),
            b"\x1b[A"
        );
        assert_eq!(
            emulator
                .encode_key(KeyInput::key(TerminalKey::PageUp))
                .unwrap(),
            b"\x1b[5~"
        );
    }

    #[test]
    fn the_control_keys_the_prd_requires_all_encode() {
        // PRD §6.2. Ctrl+C is the one that matters most: without it a runaway
        // process cannot be interrupted from the GUI at all.
        let emulator = TerminalEmulator::new(4, 20);
        for (character, expected) in [
            ('c', 3u8),
            ('d', 4),
            ('l', 12),
            ('z', 26),
            ('a', 1),
            ('e', 5),
            ('r', 18),
        ] {
            assert_eq!(
                emulator.encode_key(KeyInput::ctrl(character)).unwrap(),
                vec![expected],
                "Ctrl+{character} must reach the process"
            );
        }
    }

    #[test]
    fn arrow_keys_follow_application_cursor_mode() {
        let mut emulator = TerminalEmulator::new(4, 20);
        assert_eq!(
            emulator.encode_key(KeyInput::key(TerminalKey::Up)).unwrap(),
            b"\x1b[A"
        );
        emulator.write(b"\x1b[?1h");
        assert!(emulator.application_cursor());
        assert_eq!(
            emulator.encode_key(KeyInput::key(TerminalKey::Up)).unwrap(),
            b"\x1bOA",
            "an editor asking for SS3 arrows must not receive CSI arrows"
        );
    }

    #[test]
    fn a_paste_is_bracketed_when_the_program_asked_for_it() {
        let mut emulator = TerminalEmulator::new(4, 20);
        assert_eq!(emulator.encode_paste("a\nb"), b"a\rb".to_vec());
        emulator.write(b"\x1b[?2004h");
        assert_eq!(
            emulator.encode_paste("a\nb"),
            b"\x1b[200~a\rb\x1b[201~".to_vec(),
            "a shell must be able to tell a paste from typing"
        );
    }

    #[test]
    fn a_terminal_summary_is_bounded_and_deduplicated() {
        // PRD §38: complete scrollback must never reach a model.
        let mut transcript = String::new();
        for _ in 0..200 {
            transcript.push_str("error: cannot find module 'left-pad'\n");
        }
        for index in 0..100 {
            transcript.push_str(&format!("compiling unit {index}\n"));
        }
        let summary =
            TerminalContextSummary::from_transcript("npm run build", Some(1), &transcript);
        assert_eq!(summary.exit_status, Some(1));
        assert_eq!(
            summary.key_errors.len(),
            1,
            "the same failure 200 times is one fact"
        );
        assert!(summary.relevant_output.len() <= SUMMARY_TAIL_LINES);
        assert!(
            summary.omitted_lines > 250,
            "the summary must admit how much it dropped"
        );
    }
}
