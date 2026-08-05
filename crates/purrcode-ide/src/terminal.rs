//! An interactive terminal drawn with egui.
//!
//! The v1.0 Terminal PRD §48 states the bar this has to clear:
//!
//! > If the user cannot click it, type `pwd`, press Enter, see output, and
//! > Ctrl+C a running command, the terminal does not work.
//!
//! So this is not a text widget showing PTY output. It is a grid renderer over
//! [`TerminalEmulator`] with real keyboard routing, a cursor, focus, selection,
//! scrollback and resize. It never talks to the daemon: [`draw`] returns the
//! [`TerminalCommand`]s the caller should send, which keeps the one place that
//! owns the PTY (`purrcoded`) the only place that owns the PTY.

use std::path::PathBuf;

use egui::{
    Align2, Color32, EventFilter, FontId, Key, Modifiers, Pos2, Rect, Sense, Stroke, Ui, Vec2,
};
use purrcode_terminal_runtime::{
    GridPoint, KeyInput, KeyModifiers, Selection, TerminalCell, TerminalColor, TerminalEmulator,
    TerminalKey,
};

use crate::theme::{Appearance, Tokens};

/// What a terminal is running, so a tab reads as a purpose rather than a UUID.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TerminalKind {
    Shell,
    Agent,
    Build,
    Tests,
    Server,
}

impl TerminalKind {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Shell => "Shell",
            Self::Agent => "Agent",
            Self::Build => "Build",
            Self::Tests => "Tests",
            Self::Server => "Server",
        }
    }

    /// Classify from the command a terminal was started with.
    pub fn classify(command: &str) -> Self {
        let lower = command.to_ascii_lowercase();
        if lower.contains("test") || lower.contains("pytest") || lower.contains("jest") {
            Self::Tests
        } else if lower.contains("build") || lower.contains("compile") {
            Self::Build
        } else if lower.contains("serve") || lower.contains("dev") {
            Self::Server
        } else if lower.is_empty() {
            Self::Shell
        } else {
            Self::Agent
        }
    }
}

/// Which tree of files the terminal is looking at.
///
/// PurrCode runs sessions in isolated worktrees, so "the repository" is
/// ambiguous. PRD §14 requires the answer to be on screen: a user who does not
/// know whether `git status` is describing their checkout or the agent's is
/// being actively misled.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TerminalScope {
    Workspace,
    AgentWorktree,
}

impl TerminalScope {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Workspace => "Workspace",
            Self::AgentWorktree => "Agent worktree",
        }
    }
}

/// Who the keyboard belongs to.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TerminalOwner {
    User,
    Agent { role: String },
    SharedReadOnly,
}

impl TerminalOwner {
    pub fn is_agent(&self) -> bool {
        matches!(self, Self::Agent { .. })
    }

    /// The sentence shown above the grid. Ownership is stated in words, never
    /// by colour alone (PRD §32).
    pub fn banner(&self) -> Option<String> {
        match self {
            Self::User => None,
            Self::Agent { role } if role.is_empty() => {
                Some("PurrCode is controlling this terminal".to_owned())
            }
            Self::Agent { role } => Some(format!("{role} is controlling this terminal")),
            Self::SharedReadOnly => Some("This terminal is read-only".to_owned()),
        }
    }

    /// Parse the daemon's `{"kind": "...", "data": {...}}` owner shape.
    pub fn parse(value: &serde_json::Value) -> Self {
        match value.get("kind").and_then(serde_json::Value::as_str) {
            Some("agent") => Self::Agent {
                role: value
                    .get("data")
                    .and_then(|data| data.get("role"))
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
            },
            Some("shared") => Self::SharedReadOnly,
            _ => Self::User,
        }
    }
}

/// Where a terminal is in its life.
///
/// PRD §4 rejects a bare connected/not-connected flag: "exited cleanly", "the
/// shell was never found" and "the daemon went away with the process still
/// running" need different words and different buttons.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TerminalLifecycle {
    Starting,
    Running,
    Exited(i32),
    Failed(String),
    Disconnected,
}

impl TerminalLifecycle {
    pub fn is_live(&self) -> bool {
        matches!(self, Self::Starting | Self::Running)
    }
}

/// What the foreground process is, for the header and the stop menu.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ProcessSummary {
    pub command: Option<String>,
    pub pid: Option<i32>,
}

/// One terminal the interface is showing.
pub struct GuiTerminal {
    pub terminal_id: String,
    pub kind: TerminalKind,
    pub cwd: Option<PathBuf>,
    pub scope: TerminalScope,
    pub shell: Option<String>,
    pub owner: TerminalOwner,
    pub lifecycle: TerminalLifecycle,
    pub process: Option<ProcessSummary>,
    pub generation: u64,
    /// Byte offset of the transcript already fed to the emulator.
    pub offset: u64,
    pub emulator: TerminalEmulator,
    pub selection: Option<Selection>,
    /// True when the daemon reported that output was discarded before we read
    /// it — the grid then has a hole, and saying so beats pretending.
    pub lost_output: bool,
    /// Size last sent to the PTY, so an unchanged layout is not re-sent.
    pub reported_size: (u16, u16),
}

impl GuiTerminal {
    pub fn new(terminal_id: String, kind: TerminalKind, scope: TerminalScope) -> Self {
        Self {
            terminal_id,
            kind,
            cwd: None,
            scope,
            shell: None,
            owner: TerminalOwner::User,
            lifecycle: TerminalLifecycle::Starting,
            process: None,
            generation: 0,
            offset: 0,
            emulator: TerminalEmulator::new(24, 80),
            selection: None,
            lost_output: false,
            reported_size: (0, 0),
        }
    }

    /// The tab label. A program that set a window title knows better than we do
    /// what it is doing, so that wins when it is present.
    pub fn title(&self) -> String {
        let base = self
            .emulator
            .title()
            .map(str::to_owned)
            .unwrap_or_else(|| self.kind.label().to_owned());
        match &self.lifecycle {
            TerminalLifecycle::Exited(_) => format!("{base} (exited)"),
            TerminalLifecycle::Failed(_) => format!("{base} (failed)"),
            TerminalLifecycle::Disconnected => format!("{base} (detached)"),
            _ => base,
        }
    }

    /// The header line: what this terminal is, and which tree it sees.
    pub fn subtitle(&self) -> String {
        let mut parts = vec![self.kind.label().to_owned(), self.scope.label().to_owned()];
        if let Some(command) = self.process.as_ref().and_then(|p| p.command.clone()) {
            parts.push(command);
        }
        parts.join(" · ")
    }

    /// The full working directory, for the tooltip.
    pub fn detail(&self) -> String {
        match self.cwd.as_ref() {
            Some(cwd) => format!("{}\n{}", self.scope.label(), cwd.display()),
            None => self.scope.label().to_owned(),
        }
    }

    /// Whether the user's keyboard should reach this PTY.
    pub fn accepts_user_input(&self) -> bool {
        self.lifecycle.is_live() && matches!(self.owner, TerminalOwner::User)
    }

    /// Whether the terminal process is currently running.
    pub fn is_running(&self) -> bool {
        self.lifecycle.is_live()
    }

    /// The byte offset of output already fed to the emulator.
    pub fn last_offset(&self) -> u64 {
        self.offset
    }

    /// Feed one `/v1/terminals/{id}/output` chunk into the emulator.
    ///
    /// The daemon owns the offset: it returns the value to pass as `since` on
    /// the next read, which is not the same as "what we had plus what we got"
    /// once the retention window has dropped bytes. Trusting our own arithmetic
    /// instead is what makes a busy terminal replay or skip output forever.
    pub fn append_chunk(&mut self, bytes: &[u8], next_offset: u64, truncated: bool) {
        if truncated {
            self.lost_output = true;
        }
        if !bytes.is_empty() {
            self.emulator.write(bytes);
        }
        self.offset = next_offset.max(self.offset);
    }

    /// Set the exit code when the terminal process terminates.
    pub fn set_exit_code(&mut self, code: Option<i32>) {
        if let Some(code) = code {
            self.lifecycle = TerminalLifecycle::Exited(code);
        }
    }

    /// Adopt the daemon's view of this terminal.
    ///
    /// The GUI is not the owner of any of this: the PTY, the ownership
    /// generation and liveness all live in `purrcoded`, so a snapshot always
    /// wins over what the last frame happened to believe.
    pub fn adopt_snapshot(&mut self, snapshot: &serde_json::Value) {
        if let Some(generation) = snapshot.get("generation").and_then(|v| v.as_u64()) {
            self.generation = generation;
        }
        if let Some(owner) = snapshot.get("owner") {
            self.owner = TerminalOwner::parse(owner);
        }
        if let Some(directory) = snapshot
            .get("working_directory")
            .and_then(serde_json::Value::as_str)
        {
            self.cwd = Some(PathBuf::from(directory));
        }
        if let Some(shell) = snapshot.get("shell").and_then(serde_json::Value::as_str) {
            self.shell = Some(shell.to_owned());
        }
        if let Some(pid) = snapshot.get("process_group").and_then(|v| v.as_i64()) {
            let mut process = self.process.clone().unwrap_or_default();
            process.pid = Some(pid as i32);
            process.command = process.command.or_else(|| {
                self.shell
                    .as_deref()
                    .and_then(|shell| shell.rsplit('/').next())
                    .map(str::to_owned)
            });
            self.process = Some(process);
        }
        // A terminal is described as exited only when the daemon reports the
        // code, so an exit stays distinguishable from a dropped connection.
        match snapshot.get("exit_code").and_then(|v| v.as_i64()) {
            Some(code) => self.lifecycle = TerminalLifecycle::Exited(code as i32),
            None => match snapshot.get("alive").and_then(|v| v.as_bool()) {
                Some(true) => self.lifecycle = TerminalLifecycle::Running,
                Some(false) if self.lifecycle.is_live() => {
                    self.lifecycle = TerminalLifecycle::Disconnected;
                }
                _ => {}
            },
        }
    }

    /// Render the terminal grid into the given UI region, returning the commands
    /// the caller must send to the daemon (input bytes, resize, copy, etc.).
    ///
    /// `appearance` is passed in rather than assumed: a light workspace with a
    /// hard-coded dark palette is how a terminal ends up unreadable.
    pub fn render(
        &mut self,
        ui: &mut Ui,
        tokens: &Tokens,
        appearance: Appearance,
        take_focus: bool,
    ) -> Vec<TerminalCommand> {
        let style = crate::terminal::TerminalStyle {
            palette: crate::terminal::TerminalPalette::for_appearance(appearance),
            tokens: *tokens,
            font_size: crate::theme::TYPE_CODE,
        };
        crate::terminal::draw(ui, self, &style, take_focus)
    }
}

/// Something the terminal wants the application to do.
///
/// Returned rather than performed, so this file never learns the daemon's URL,
/// its token, or the ownership generation protocol.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TerminalCommand {
    /// Bytes for the PTY, already encoded for the emulator's current modes.
    Input(Vec<u8>),
    Resize {
        rows: u16,
        cols: u16,
    },
    /// Ctrl+C to the foreground process group (PRD §18: not the same as Stop).
    Interrupt,
    /// SIGTERM then SIGKILL to the process group.
    Terminate,
    /// Forget the terminal; the process is already gone.
    Close,
    Restart,
    /// Human takeover (PRD §16).
    TakeControl,
    /// Hand the keyboard back to the agent (PRD §17).
    ReturnToAgent,
    /// Start a shell because none exists (PRD §13).
    OpenShell,
    /// Copy the current selection.
    Copy(String),
}

/// The ANSI palette for one appearance.
///
/// PRD §34 forbids deriving the light theme by inverting the dark one: a dark
/// theme's bright yellow is unreadable on white however it is flipped. Each
/// appearance names its sixteen colours, and a test checks them against the
/// background they are actually drawn on.
#[derive(Clone, Copy, Debug)]
pub struct TerminalPalette {
    pub background: Color32,
    pub foreground: Color32,
    pub cursor: Color32,
    pub selection: Color32,
    pub ansi: [Color32; 16],
}

impl TerminalPalette {
    pub fn for_appearance(appearance: Appearance) -> Self {
        match appearance {
            // Dark: the reference terminal palette, deepened slightly so the
            // grid reads as a surface rather than a hole in the window.
            Appearance::Dark => Self {
                background: Color32::from_rgb(0x14, 0x16, 0x1a),
                foreground: Color32::from_rgb(0xd6, 0xda, 0xe0),
                cursor: Color32::from_rgb(0xf2, 0xa5, 0x4c),
                selection: Color32::from_rgb(0x35, 0x43, 0x5c),
                ansi: [
                    Color32::from_rgb(0x3b, 0x40, 0x48), // black
                    Color32::from_rgb(0xf2, 0x6d, 0x6d), // red
                    Color32::from_rgb(0x74, 0xc9, 0x80), // green
                    Color32::from_rgb(0xe0, 0xb4, 0x54), // yellow
                    Color32::from_rgb(0x6f, 0xa8, 0xea), // blue
                    Color32::from_rgb(0xc2, 0x8b, 0xe8), // magenta
                    Color32::from_rgb(0x5c, 0xc4, 0xc4), // cyan
                    Color32::from_rgb(0xc8, 0xcd, 0xd4), // white
                    Color32::from_rgb(0x6b, 0x72, 0x7e), // bright black
                    Color32::from_rgb(0xff, 0x92, 0x92),
                    Color32::from_rgb(0x9a, 0xe0, 0xa4),
                    Color32::from_rgb(0xf5, 0xcd, 0x79),
                    Color32::from_rgb(0x96, 0xc4, 0xf5),
                    Color32::from_rgb(0xd8, 0xac, 0xf5),
                    Color32::from_rgb(0x86, 0xdc, 0xdc),
                    Color32::from_rgb(0xf2, 0xf4, 0xf7),
                ],
            },
            // Light: every colour darkened until it survives on near-white.
            // These are not the dark values inverted; they are chosen values.
            Appearance::Light => Self {
                background: Color32::from_rgb(0xfc, 0xfc, 0xfd),
                foreground: Color32::from_rgb(0x24, 0x28, 0x2e),
                cursor: Color32::from_rgb(0xb5, 0x6a, 0x10),
                selection: Color32::from_rgb(0xcf, 0xdd, 0xf2),
                ansi: [
                    Color32::from_rgb(0x24, 0x28, 0x2e),
                    Color32::from_rgb(0xb3, 0x25, 0x25),
                    Color32::from_rgb(0x1c, 0x6b, 0x2e),
                    Color32::from_rgb(0x84, 0x5b, 0x00),
                    Color32::from_rgb(0x1a, 0x4f, 0xa8),
                    Color32::from_rgb(0x82, 0x2c, 0xa8),
                    Color32::from_rgb(0x0d, 0x5f, 0x66),
                    Color32::from_rgb(0x4a, 0x50, 0x59),
                    Color32::from_rgb(0x5c, 0x63, 0x6d),
                    Color32::from_rgb(0x8f, 0x1a, 0x1a),
                    Color32::from_rgb(0x15, 0x55, 0x24),
                    Color32::from_rgb(0x6b, 0x49, 0x00),
                    Color32::from_rgb(0x14, 0x3f, 0x87),
                    Color32::from_rgb(0x68, 0x22, 0x87),
                    Color32::from_rgb(0x0a, 0x4c, 0x52),
                    Color32::from_rgb(0x1a, 0x1d, 0x22),
                ],
            },
            // High contrast: pure black ground, saturated ink, nothing subtle.
            Appearance::HighContrast => Self {
                background: Color32::BLACK,
                foreground: Color32::WHITE,
                cursor: Color32::from_rgb(0xff, 0xd0, 0x00),
                selection: Color32::from_rgb(0x00, 0x4a, 0x9e),
                ansi: [
                    Color32::from_rgb(0x66, 0x66, 0x66),
                    Color32::from_rgb(0xff, 0x6b, 0x6b),
                    Color32::from_rgb(0x4a, 0xf5, 0x82),
                    Color32::from_rgb(0xff, 0xe0, 0x50),
                    Color32::from_rgb(0x74, 0xb8, 0xff),
                    Color32::from_rgb(0xe8, 0x9a, 0xff),
                    Color32::from_rgb(0x5a, 0xef, 0xef),
                    Color32::WHITE,
                    Color32::from_rgb(0x99, 0x99, 0x99),
                    Color32::from_rgb(0xff, 0xa0, 0xa0),
                    Color32::from_rgb(0x90, 0xff, 0xb0),
                    Color32::from_rgb(0xff, 0xef, 0x99),
                    Color32::from_rgb(0xa8, 0xd4, 0xff),
                    Color32::from_rgb(0xf2, 0xc4, 0xff),
                    Color32::from_rgb(0x9a, 0xf7, 0xf7),
                    Color32::WHITE,
                ],
            },
        }
    }

    /// Resolve a terminal colour to something paintable.
    pub fn resolve(&self, color: TerminalColor, fallback: Color32) -> Color32 {
        match color {
            TerminalColor::Default => fallback,
            TerminalColor::Ansi(index) => self.ansi[(index as usize).min(15)],
            TerminalColor::Indexed(index) => indexed_color(self, index),
            TerminalColor::Rgb(r, g, b) => Color32::from_rgb(r, g, b),
        }
    }
}

/// The xterm 256-colour cube and greyscale ramp.
fn indexed_color(palette: &TerminalPalette, index: u8) -> Color32 {
    match index {
        0..=15 => palette.ansi[index as usize],
        16..=231 => {
            let index = index - 16;
            let step = |value: u8| if value == 0 { 0 } else { 55 + value * 40 };
            Color32::from_rgb(step(index / 36), step((index % 36) / 6), step(index % 6))
        }
        _ => {
            let level = 8 + (index - 232) * 10;
            Color32::from_gray(level)
        }
    }
}

/// Everything the grid renderer needs that is not the terminal itself.
pub struct TerminalStyle {
    pub palette: TerminalPalette,
    pub tokens: Tokens,
    pub font_size: f32,
}

/// Draw one terminal and collect what the user asked for.
///
/// `ui` should already be the region the grid may fill; the caller draws the
/// header and tab strip.
pub fn draw(
    ui: &mut Ui,
    terminal: &mut GuiTerminal,
    style: &TerminalStyle,
    take_focus: bool,
) -> Vec<TerminalCommand> {
    let mut commands = Vec::new();
    let font = FontId::monospace(style.font_size);
    let (cell_width, cell_height) = ui.ctx().fonts_mut(|fonts| {
        (
            // 'M' is the widest ASCII glyph in a monospace face, and every cell
            // is that wide by definition.
            fonts.glyph_width(&font, 'M'),
            fonts.row_height(&font),
        )
    });
    if cell_width <= 0.0 || cell_height <= 0.0 {
        return commands;
    }

    let available = ui.available_size();
    let rect = Rect::from_min_size(ui.cursor().min, available);
    let id = ui.make_persistent_id(("purrcode-terminal", &terminal.terminal_id));
    let response = ui.interact(rect, id, Sense::click_and_drag());
    ui.advance_cursor_after_rect(rect);

    // Size the PTY to the space the grid actually has (PRD §10). One column of
    // slack keeps a wrapped line from oscillating between two widths.
    let cols = ((available.x / cell_width).floor() as u16).clamp(2, 1000);
    let rows = ((available.y / cell_height).floor() as u16).clamp(2, 500);
    if terminal.emulator.resize(rows, cols) || terminal.reported_size != (rows, cols) {
        terminal.reported_size = (rows, cols);
        commands.push(TerminalCommand::Resize { rows, cols });
    }

    if take_focus {
        response.request_focus();
    }
    let focused = ui.memory(|memory| memory.has_focus(id));
    if response.clicked() || response.drag_started() {
        response.request_focus();
    }
    if focused {
        // Without this, egui steals Tab (focus), the arrows (navigation) and
        // Escape before the PTY ever sees them — PRD §6.2 says the terminal
        // gets them while it has focus.
        ui.memory_mut(|memory| {
            memory.set_focus_lock_filter(
                id,
                EventFilter {
                    tab: true,
                    horizontal_arrows: true,
                    vertical_arrows: true,
                    escape: true,
                },
            );
        });
        commands.extend(handle_keyboard(ui, terminal));
    }

    handle_pointer(ui, terminal, &response, rect, cell_width, cell_height);
    handle_scroll(ui, terminal, &response, cell_height);

    paint(
        ui,
        terminal,
        style,
        rect,
        &font,
        cell_width,
        cell_height,
        focused,
    );

    commands
}

/// Turn this frame's key events into PTY bytes.
fn handle_keyboard(ui: &mut Ui, terminal: &mut GuiTerminal) -> Vec<TerminalCommand> {
    let mut commands = Vec::new();
    let filter = EventFilter {
        tab: true,
        horizontal_arrows: true,
        vertical_arrows: true,
        escape: true,
    };
    let events = ui.input(|input| input.filtered_events(&filter));
    let has_selection = terminal
        .selection
        .is_some_and(|selection| !selection.is_empty());

    for event in events {
        match event {
            // Copy on the platform's copy shortcut. When nothing is selected,
            // Ctrl+C must still reach the process as an interrupt (PRD §12) —
            // egui reports the copy shortcut as `Copy`, so this branch is only
            // taken for a real copy, and Ctrl+C arrives as a Key event below.
            egui::Event::Copy | egui::Event::Cut => {
                if let Some(selection) = terminal.selection.filter(|s| !s.is_empty()) {
                    commands.push(TerminalCommand::Copy(
                        terminal.emulator.selected_text(selection),
                    ));
                }
            }
            egui::Event::Paste(text) => {
                if terminal.accepts_user_input() {
                    commands.push(TerminalCommand::Input(
                        terminal.emulator.encode_paste(&text),
                    ));
                }
            }
            egui::Event::Text(text) => {
                if !terminal.accepts_user_input() {
                    continue;
                }
                // egui emits Text for printable input; control combinations
                // arrive as Key events, so nothing is doubled here.
                terminal.emulator.scroll_to_bottom();
                terminal.selection = None;
                commands.push(TerminalCommand::Input(text.into_bytes()));
            }
            egui::Event::Key {
                key,
                pressed: true,
                modifiers,
                ..
            } => {
                // Scrollback keys belong to the viewport, not the process —
                // unless a full-screen program is running, where Page Up is the
                // program's to interpret.
                if !terminal.emulator.alternate_screen() && modifiers.shift {
                    match key {
                        Key::PageUp => {
                            terminal.emulator.page_up();
                            continue;
                        }
                        Key::PageDown => {
                            terminal.emulator.page_down();
                            continue;
                        }
                        _ => {}
                    }
                }
                if !terminal.accepts_user_input() {
                    continue;
                }
                let Some(input) = key_input(key, modifiers, has_selection) else {
                    continue;
                };
                let Some(bytes) = terminal.emulator.encode_key(input) else {
                    continue;
                };
                // Typing means "show me the bottom": a keystroke that vanishes
                // into off-screen output looks like a dead terminal.
                terminal.emulator.scroll_to_bottom();
                terminal.selection = None;
                commands.push(TerminalCommand::Input(bytes));
            }
            _ => {}
        }
    }
    commands
}

/// Map an egui key to a terminal key.
///
/// Returns `None` for keys a PTY has no encoding for, and for the copy/paste
/// shortcuts, which egui already delivered as `Copy`/`Paste`.
fn key_input(key: Key, modifiers: Modifiers, has_selection: bool) -> Option<KeyInput> {
    let mods = KeyModifiers {
        ctrl: modifiers.ctrl,
        alt: modifiers.alt,
        shift: modifiers.shift,
    };
    let named = |key| Some(KeyInput::Named(key, mods));
    match key {
        Key::Enter => named(TerminalKey::Enter),
        Key::Backspace => named(TerminalKey::Backspace),
        Key::Tab if modifiers.shift => named(TerminalKey::BackTab),
        Key::Tab => named(TerminalKey::Tab),
        Key::Escape => named(TerminalKey::Escape),
        Key::ArrowUp => named(TerminalKey::Up),
        Key::ArrowDown => named(TerminalKey::Down),
        Key::ArrowLeft => named(TerminalKey::Left),
        Key::ArrowRight => named(TerminalKey::Right),
        Key::Home => named(TerminalKey::Home),
        Key::End => named(TerminalKey::End),
        Key::PageUp => named(TerminalKey::PageUp),
        Key::PageDown => named(TerminalKey::PageDown),
        Key::Insert => named(TerminalKey::Insert),
        Key::Delete => named(TerminalKey::Delete),
        Key::F1 => named(TerminalKey::Function(1)),
        Key::F2 => named(TerminalKey::Function(2)),
        Key::F3 => named(TerminalKey::Function(3)),
        Key::F4 => named(TerminalKey::Function(4)),
        Key::F5 => named(TerminalKey::Function(5)),
        Key::F6 => named(TerminalKey::Function(6)),
        Key::F7 => named(TerminalKey::Function(7)),
        Key::F8 => named(TerminalKey::Function(8)),
        Key::F9 => named(TerminalKey::Function(9)),
        Key::F10 => named(TerminalKey::Function(10)),
        Key::F11 => named(TerminalKey::Function(11)),
        Key::F12 => named(TerminalKey::Function(12)),
        other if modifiers.ctrl || modifiers.alt => {
            let character = control_character(other)?;
            // On a platform where Ctrl+C is the copy shortcut, a Ctrl+C with a
            // live selection was a copy; without one it is an interrupt, and
            // swallowing it would make runaway processes unkillable (PRD §12).
            if character == 'c' && has_selection && modifiers.ctrl && !modifiers.alt {
                return None;
            }
            Some(KeyInput::Char(character, mods))
        }
        _ => None,
    }
}

/// The character an egui `Key` stands for, for control combinations.
fn control_character(key: Key) -> Option<char> {
    let name = key.name();
    let mut chars = name.chars();
    match (chars.next(), chars.next()) {
        (Some(character), None) => Some(character.to_ascii_lowercase()),
        // Punctuation keys egui names with words.
        _ => match key {
            Key::OpenBracket => Some('['),
            Key::CloseBracket => Some(']'),
            Key::Backslash => Some('\\'),
            Key::Minus => Some('_'),
            Key::Space => Some(' '),
            _ => None,
        },
    }
}

/// Click, drag and double/triple click selection (PRD §12).
fn handle_pointer(
    ui: &Ui,
    terminal: &mut GuiTerminal,
    response: &egui::Response,
    rect: Rect,
    cell_width: f32,
    cell_height: f32,
) {
    let point_at = |position: Pos2| -> GridPoint {
        let col = ((position.x - rect.min.x) / cell_width).floor().max(0.0) as u16;
        let row = ((position.y - rect.min.y) / cell_height).floor().max(0.0) as u16;
        terminal.emulator.point_at(row, col)
    };

    if response.double_clicked() {
        if let Some(position) = response.interact_pointer_pos() {
            terminal.selection = terminal.emulator.word_at(point_at(position));
        }
        return;
    }
    if response.triple_clicked() {
        if let Some(position) = response.interact_pointer_pos() {
            terminal.selection = Some(terminal.emulator.line_at(point_at(position)));
        }
        return;
    }
    if response.drag_started() {
        if let Some(position) = response.interact_pointer_pos() {
            terminal.selection = Some(Selection::new(point_at(position)));
        }
        return;
    }
    if response.dragged() {
        if let (Some(position), Some(selection)) =
            (response.interact_pointer_pos(), terminal.selection.as_mut())
        {
            selection.head = point_at(position);
        }
        return;
    }
    // A plain click clears the selection so the next Ctrl+C is an interrupt
    // again rather than a silent re-copy.
    if response.clicked() {
        terminal.selection = None;
    }
    let _ = ui;
}

/// Wheel and trackpad scrolling over the grid (PRD §11).
fn handle_scroll(ui: &Ui, terminal: &mut GuiTerminal, response: &egui::Response, cell_height: f32) {
    if !response.hovered() {
        return;
    }
    // A full-screen program has no scrollback of its own; forwarding the wheel
    // into stale history behind `vim` would be nonsense.
    if terminal.emulator.alternate_screen() {
        return;
    }
    let delta = ui.input(|input| input.smooth_scroll_delta.y);
    if delta.abs() < 0.5 {
        return;
    }
    let lines = (delta / cell_height.max(1.0)).round() as i64;
    if lines != 0 {
        terminal.emulator.scroll_by(-lines);
    }
}

/// Paint the grid, the selection, and the cursor.
#[allow(clippy::too_many_arguments)]
fn paint(
    ui: &Ui,
    terminal: &GuiTerminal,
    style: &TerminalStyle,
    rect: Rect,
    font: &FontId,
    cell_width: f32,
    cell_height: f32,
    focused: bool,
) {
    let painter = ui.painter_at(rect);
    let palette = &style.palette;
    painter.rect_filled(rect, 4.0, palette.background);
    // A border of its own so the terminal reads as a surface, not as bare
    // pixels butted against the dock margin. When the terminal has the
    // keyboard, the border becomes the accent ring — the same focus language
    // every other control in the window uses.
    let border = if focused {
        style.tokens.accent_primary
    } else {
        style.tokens.border_subtle
    };
    painter.rect_stroke(
        rect,
        4.0,
        egui::Stroke::new(if focused { 2.0_f32 } else { 1.0_f32 }, border),
        egui::StrokeKind::Inside,
    );

    let viewport = terminal.emulator.viewport();
    let origin = terminal.emulator.viewport_origin();

    for (row_index, row) in viewport.iter().enumerate() {
        let y = rect.min.y + row_index as f32 * cell_height;
        if y > rect.max.y {
            break;
        }
        // Runs of identical styling become one text shape. Painting per cell
        // would be tens of thousands of shapes a frame on a full screen.
        let mut column = 0usize;
        while column < row.len() {
            let cell = &row[column];
            if cell.wide_continuation {
                column += 1;
                continue;
            }
            let mut end = column + 1;
            while end < row.len()
                && !row[end].wide_continuation
                && same_style(&row[end], cell)
                && row[end].text.chars().count() <= 1
                && cell.text.chars().count() <= 1
            {
                end += 1;
            }
            let text: String = row[column..end].iter().map(TerminalCell::glyph).collect();
            let (fg, bg) = resolved_colors(cell, palette);
            let run = Rect::from_min_size(
                Pos2::new(rect.min.x + column as f32 * cell_width, y),
                Vec2::new((end - column) as f32 * cell_width, cell_height),
            );

            if bg != palette.background {
                painter.rect_filled(run, 0.0, bg);
            }
            // Selection sits over the cell background but under the glyph, so
            // selected text stays readable instead of being covered.
            if let Some(selection) = terminal.selection.filter(|s| !s.is_empty()) {
                paint_selection(
                    &painter,
                    selection,
                    origin + row_index,
                    column..end,
                    Pos2::new(rect.min.x, y),
                    Vec2::new(cell_width, cell_height),
                    palette.selection,
                );
            }
            if !text.trim().is_empty() {
                let mut glyph_font = font.clone();
                if cell.attrs.bold {
                    // egui has no synthetic bold; a slightly larger face is the
                    // honest approximation and keeps the grid aligned because
                    // monospace advance is taken from the base font.
                    glyph_font.size *= 1.0;
                }
                painter.text(
                    Pos2::new(run.min.x, run.center().y),
                    Align2::LEFT_CENTER,
                    text,
                    glyph_font,
                    if cell.attrs.dim { dim(fg) } else { fg },
                );
            }
            if cell.attrs.underline {
                painter.line_segment(
                    [
                        Pos2::new(run.min.x, run.max.y - 1.0),
                        Pos2::new(run.max.x, run.max.y - 1.0),
                    ],
                    Stroke::new(1.0_f32, fg),
                );
            }
            column = end;
        }
    }

    paint_cursor(
        &painter,
        terminal,
        palette,
        rect,
        font,
        cell_width,
        cell_height,
        focused,
    );

    if focused {
        painter.rect_stroke(
            rect,
            4.0,
            Stroke::new(1.0_f32, style.tokens.accent_primary),
            egui::StrokeKind::Inside,
        );
    }
}

/// Highlight whichever part of `columns` on this row is selected.
fn paint_selection(
    painter: &egui::Painter,
    selection: Selection,
    absolute_row: usize,
    columns: std::ops::Range<usize>,
    row_origin: Pos2,
    cell: Vec2,
    color: Color32,
) {
    let mut start = None;
    let mut end = columns.start;
    for column in columns {
        let point = GridPoint {
            row: absolute_row,
            col: column as u16,
        };
        if selection.contains(point) {
            start.get_or_insert(column);
            end = column + 1;
        }
    }
    let Some(start) = start else { return };
    painter.rect_filled(
        Rect::from_min_size(
            Pos2::new(row_origin.x + start as f32 * cell.x, row_origin.y),
            Vec2::new((end - start) as f32 * cell.x, cell.y),
        ),
        0.0,
        color,
    );
}

#[allow(clippy::too_many_arguments)]
fn paint_cursor(
    painter: &egui::Painter,
    terminal: &GuiTerminal,
    palette: &TerminalPalette,
    rect: Rect,
    font: &FontId,
    cell_width: f32,
    cell_height: f32,
    focused: bool,
) {
    let cursor = terminal.emulator.cursor();
    if !cursor.visible || !terminal.lifecycle.is_live() {
        return;
    }
    let Some(row) = cursor.row else {
        // Scrolled out of view. Drawing it anyway would claim typing lands in
        // whatever old line happens to be at that position.
        return;
    };
    let cell = Rect::from_min_size(
        Pos2::new(
            rect.min.x + cursor.col as f32 * cell_width,
            rect.min.y + row as f32 * cell_height,
        ),
        Vec2::new(cell_width, cell_height),
    );
    if !rect.contains(cell.center()) {
        return;
    }
    if focused {
        painter.rect_filled(cell, 0.0, palette.cursor);
        // Knock the glyph out of the block so the character under the cursor is
        // still legible.
        let under = terminal
            .emulator
            .row(row)
            .get(cursor.col as usize)
            .map(|cell| cell.glyph().to_owned())
            .unwrap_or_default();
        if !under.trim().is_empty() {
            painter.text(
                Pos2::new(cell.min.x, cell.center().y),
                Align2::LEFT_CENTER,
                under,
                font.clone(),
                palette.background,
            );
        }
    } else {
        // An unfocused terminal shows a hollow cursor: the process is still
        // there, but typing will not reach it.
        painter.rect_stroke(
            cell,
            0.0,
            Stroke::new(1.0_f32, palette.cursor),
            egui::StrokeKind::Inside,
        );
    }
}

fn same_style(a: &TerminalCell, b: &TerminalCell) -> bool {
    a.fg == b.fg && a.bg == b.bg && a.attrs == b.attrs
}

fn resolved_colors(cell: &TerminalCell, palette: &TerminalPalette) -> (Color32, Color32) {
    let fg = palette.resolve(cell.fg, palette.foreground);
    let bg = palette.resolve(cell.bg, palette.background);
    if cell.attrs.inverse {
        (bg, fg)
    } else {
        (fg, bg)
    }
}

fn dim(color: Color32) -> Color32 {
    Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), 160)
}

/// Turn a runtime error into something a developer can act on (PRD §35).
///
/// The raw message is not discarded — the caller shows it under "Technical
/// details" — but it is not the headline, because `execvp() of 'ruff' failed`
/// tells a user nothing about what to do next.
pub fn humanize_error(raw: &str) -> String {
    let lower = raw.to_ascii_lowercase();
    if let Some(tool) = missing_executable(raw) {
        return format!("{tool} is not installed.");
    }
    if lower.contains("no such file or directory") && lower.contains("cwd") {
        return "The working directory no longer exists.".to_owned();
    }
    if lower.contains("permission denied") {
        return "PurrCode is not allowed to run that here.".to_owned();
    }
    if lower.contains("uv_os_homedir")
        || lower.contains("could not determine") && lower.contains("home")
    {
        return "npm could not resolve the user environment.".to_owned();
    }
    if lower.contains("connection refused") || lower.contains("failed to connect") {
        return "The PurrCode daemon is not reachable.".to_owned();
    }
    if lower.contains("timed out") {
        return "The command took too long and was stopped.".to_owned();
    }
    raw.to_owned()
}

/// Pull the program name out of an exec failure.
///
/// Two shapes matter. `sandbox-exec: execvp() of 'ruff' failed` quotes the
/// program; `bash: line 1: eslint: command not found` puts it immediately
/// before the message, *after* the shell's own name — taking the first token
/// there would blame bash for eslint being absent.
fn missing_executable(raw: &str) -> Option<String> {
    let lower = raw.to_ascii_lowercase();
    let plausible = |name: &str| {
        !name.is_empty()
            && !name.contains(char::is_whitespace)
            && !name.contains('/')
            && name.chars().all(|c| c.is_ascii_graphic())
    };

    if (lower.contains("execvp") || lower.contains("no such file"))
        && let Some(start) = raw.find('\'')
        && let Some(end) = raw[start + 1..].find('\'')
    {
        let name = &raw[start + 1..start + 1 + end];
        if plausible(name) {
            return Some(name.to_owned());
        }
    }

    if let Some(position) = lower.find("command not found") {
        return raw[..position]
            .trim_end_matches([':', ' '])
            .rsplit(':')
            .next()
            .map(str::trim)
            .filter(|name| plausible(name))
            .map(str::to_owned);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Relative luminance, for the WCAG contrast ratio.
    fn luminance(color: Color32) -> f64 {
        let channel = |value: u8| {
            let value = value as f64 / 255.0;
            if value <= 0.03928 {
                value / 12.92
            } else {
                ((value + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * channel(color.r()) + 0.7152 * channel(color.g()) + 0.0722 * channel(color.b())
    }

    fn contrast(a: Color32, b: Color32) -> f64 {
        let (a, b) = (luminance(a), luminance(b));
        let (high, low) = if a > b { (a, b) } else { (b, a) };
        (high + 0.05) / (low + 0.05)
    }

    #[test]
    fn every_ansi_colour_is_readable_on_its_own_background() {
        // PRD §34: "Ensure ANSI colors remain readable in both themes." A light
        // theme built by inverting a dark one fails this on yellow and cyan.
        for appearance in Appearance::ALL {
            let palette = TerminalPalette::for_appearance(*appearance);
            for (index, colour) in palette.ansi.iter().enumerate() {
                // Index 0 is the palette's "black", which programs use as a
                // background, not as text.
                if index == 0 || index == 8 {
                    continue;
                }
                let ratio = contrast(*colour, palette.background);
                assert!(
                    ratio >= 3.0,
                    "{appearance:?} ANSI {index} has contrast {ratio:.2} against its background"
                );
            }
            assert!(
                contrast(palette.foreground, palette.background) >= 7.0,
                "{appearance:?} body text must be comfortably readable"
            );
            assert!(
                contrast(palette.cursor, palette.background) >= 3.0,
                "{appearance:?} cursor must be findable"
            );
        }
    }

    #[test]
    fn the_light_palette_is_not_the_dark_one_inverted() {
        let dark = TerminalPalette::for_appearance(Appearance::Dark);
        let light = TerminalPalette::for_appearance(Appearance::Light);
        let inverted = |c: Color32| Color32::from_rgb(255 - c.r(), 255 - c.g(), 255 - c.b());
        let matches = dark
            .ansi
            .iter()
            .zip(light.ansi.iter())
            .filter(|(d, l)| inverted(**d) == **l)
            .count();
        assert_eq!(matches, 0, "PRD §34 forbids deriving light by inversion");
    }

    #[test]
    fn the_256_colour_cube_maps_to_the_expected_corners() {
        let palette = TerminalPalette::for_appearance(Appearance::Dark);
        assert_eq!(indexed_color(&palette, 16), Color32::from_rgb(0, 0, 0));
        assert_eq!(
            indexed_color(&palette, 231),
            Color32::from_rgb(255, 255, 255)
        );
        assert_eq!(indexed_color(&palette, 1), palette.ansi[1]);
    }

    #[test]
    fn a_terminal_reports_which_tree_it_is_looking_at() {
        // PRD §14: the user must never be unsure which repository state they
        // are in.
        let mut terminal = GuiTerminal::new(
            "t1".into(),
            TerminalKind::Shell,
            TerminalScope::AgentWorktree,
        );
        terminal.cwd = Some(PathBuf::from("/tmp/.purrcode/worktrees/abc"));
        assert!(terminal.subtitle().contains("Agent worktree"));
        assert!(terminal.detail().contains("/tmp/.purrcode/worktrees/abc"));
    }

    #[test]
    fn ownership_is_stated_in_words() {
        assert_eq!(TerminalOwner::User.banner(), None);
        assert_eq!(
            TerminalOwner::Agent {
                role: "Build Agent".into()
            }
            .banner()
            .as_deref(),
            Some("Build Agent is controlling this terminal")
        );
        assert!(!TerminalOwner::User.is_agent());
    }

    #[test]
    fn an_agent_owned_terminal_refuses_the_users_keyboard() {
        // Otherwise the human and the agent interleave keystrokes into one
        // shell and neither gets the command they typed (PRD §39).
        let mut terminal =
            GuiTerminal::new("t".into(), TerminalKind::Agent, TerminalScope::Workspace);
        terminal.lifecycle = TerminalLifecycle::Running;
        assert!(terminal.accepts_user_input());
        terminal.owner = TerminalOwner::Agent {
            role: "Build Agent".into(),
        };
        assert!(!terminal.accepts_user_input());
    }

    #[test]
    fn an_exited_terminal_says_so_and_stops_accepting_input() {
        let mut terminal =
            GuiTerminal::new("t".into(), TerminalKind::Tests, TerminalScope::Workspace);
        terminal.lifecycle = TerminalLifecycle::Exited(0);
        assert_eq!(terminal.title(), "Tests (exited)");
        assert!(!terminal.accepts_user_input());
    }

    #[test]
    fn a_program_that_sets_a_window_title_names_its_own_tab() {
        let mut terminal =
            GuiTerminal::new("t".into(), TerminalKind::Shell, TerminalScope::Workspace);
        assert_eq!(terminal.title(), "Shell");
        terminal.emulator.write(b"\x1b]0;cargo test\x07");
        assert_eq!(terminal.title(), "cargo test");
    }

    #[test]
    fn the_daemon_owner_shape_parses() {
        let agent = serde_json::json!({"kind": "agent", "data": {"role": "Supervisor"}});
        assert_eq!(
            TerminalOwner::parse(&agent),
            TerminalOwner::Agent {
                role: "Supervisor".into()
            }
        );
        assert_eq!(
            TerminalOwner::parse(&serde_json::json!({"kind": "human"})),
            TerminalOwner::User
        );
        assert_eq!(
            TerminalOwner::parse(&serde_json::json!({})),
            TerminalOwner::User,
            "an unknown owner must not silently become the agent"
        );
    }

    #[test]
    fn tabs_are_named_by_purpose() {
        assert_eq!(TerminalKind::classify("cargo test"), TerminalKind::Tests);
        assert_eq!(TerminalKind::classify("npm run dev"), TerminalKind::Server);
        assert_eq!(TerminalKind::classify(""), TerminalKind::Shell);
    }

    #[test]
    fn runtime_errors_become_sentences_a_developer_can_act_on() {
        // PRD §35.
        assert_eq!(
            humanize_error("sandbox-exec: execvp() of 'ruff' failed"),
            "ruff is not installed."
        );
        assert_eq!(
            humanize_error("bash: line 1: eslint: command not found"),
            "eslint is not installed.",
            "the shell reporting the failure is not the thing that is missing"
        );
        assert!(humanize_error("Permission denied (os error 13)").starts_with("PurrCode is not"));
        assert_eq!(
            humanize_error("something entirely unexpected"),
            "something entirely unexpected",
            "an error we cannot translate must survive verbatim, not vanish"
        );
    }
}
