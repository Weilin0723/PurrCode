//! The terminal, driven through a real PTY with a real shell.
//!
//! Terminal PRD §41 and §46 define acceptance behaviourally, not structurally:
//!
//! > If the user cannot click it, type `pwd`, press Enter, see output, and
//! > Ctrl+C a running command, the terminal does not work.
//!
//! A unit test over the emulator proves the parser; a mock daemon proves the
//! wiring. Neither proves that a keystroke encoded by [`TerminalEmulator`],
//! written to a real PTY, interpreted by a real shell and parsed back produces
//! what the user typed. That round trip is what these tests exercise, and it is
//! the only thing that can catch a wrong control byte or a resize that never
//! reached `TIOCSWINSZ`.
//!
//! Unix only: these spawn `/bin/sh`. The ConPTY path is exercised by the
//! runtime's own tests, which are cross-platform.
#![cfg(unix)]

use std::time::{Duration, Instant};

use purrcode_terminal_runtime::{
    KeyInput, KeyModifiers, OwnershipGeneration, ResizeTerminalAction, SendTerminalInputAction,
    StartTerminalAction, StopProcessAction, TerminalEmulator, TerminalId, TerminalKey,
    TerminalOwner, TerminalRuntime, TerminalSize, WorkspaceId,
};
use tempfile::TempDir;

/// How long a test waits for a shell to answer before giving up.
///
/// Generous, because this is bounded by process startup on a loaded CI box, not
/// by anything PurrCode does. A test that fails here has genuinely hung.
const DEADLINE: Duration = Duration::from_secs(10);

struct Session {
    runtime: TerminalRuntime,
    id: TerminalId,
    emulator: TerminalEmulator,
    offset: u64,
    #[allow(dead_code)]
    directory: TempDir,
}

impl Session {
    /// Open a shell in a scratch directory, sized like a small terminal panel.
    fn open() -> Self {
        Self::open_sized(TerminalSize { rows: 24, cols: 80 })
    }

    fn open_sized(size: TerminalSize) -> Self {
        let directory = tempfile::tempdir().expect("scratch directory");
        // Canonicalised because macOS reports `/private/var/...` for `/var/...`
        // and the shell's `pwd` would then not match the path we asked for.
        let canonical = directory.path().canonicalize().expect("canonical path");
        let runtime = TerminalRuntime::default();
        let started = runtime
            .start(
                WorkspaceId::new(),
                StartTerminalAction {
                    // `None` asks the runtime for the user's shell, started the
                    // way a user terminal starts one — which is the thing under
                    // test, since job control is what makes Ctrl+C safe.
                    program: None,
                    arguments: Vec::new(),
                    working_directory: canonical.clone(),
                    environment: [("PS1".to_owned(), "$ ".to_owned())].into_iter().collect(),
                    initial_size: size,
                    owner: None,
                    background: None,
                },
            )
            .expect("start a shell");
        Self {
            runtime,
            id: started.terminal_id,
            emulator: TerminalEmulator::with_scrollback(size.rows, size.cols, 500),
            offset: 0,
            directory,
        }
    }

    /// Drain whatever the PTY has produced into the emulator.
    fn pump(&mut self) {
        let chunk = self
            .runtime
            .read_since(self.id, self.offset)
            .expect("read terminal output");
        if !chunk.bytes.is_empty() {
            self.emulator.write(&chunk.bytes);
        }
        self.offset = chunk.next_offset;
    }

    /// Send bytes as the current owner.
    fn send(&self, bytes: Vec<u8>) {
        let generation = self
            .runtime
            .inspect(self.id, 0)
            .expect("inspect terminal")
            .generation;
        self.runtime
            .send_input(SendTerminalInputAction {
                terminal_id: self.id,
                owner_generation: generation,
                input: bytes,
            })
            .expect("send input");
    }

    /// Press a key, encoded exactly as the GUI would encode it.
    fn press(&self, key: TerminalKey) {
        let bytes = self
            .emulator
            .encode_key(KeyInput::Named(key, KeyModifiers::NONE))
            .expect("the key has a PTY encoding");
        self.send(bytes);
    }

    fn ctrl(&self, character: char) {
        let bytes = self
            .emulator
            .encode_key(KeyInput::ctrl(character))
            .expect("the control key has a PTY encoding");
        self.send(bytes);
    }

    /// Type text the way a user does: characters, then Enter.
    fn type_line(&self, text: &str) {
        self.send(text.as_bytes().to_vec());
        self.press(TerminalKey::Enter);
    }

    /// Pump until the screen contains `needle`, or fail with what it did show.
    fn wait_for(&mut self, needle: &str) -> String {
        let deadline = Instant::now() + DEADLINE;
        loop {
            self.pump();
            let screen = self.emulator.plain_text();
            if screen.contains(needle) {
                return screen;
            }
            if Instant::now() >= deadline {
                panic!("timed out waiting for {needle:?}; the screen showed:\n{screen}");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Pump until a whole line equals `needle`.
    ///
    /// `wait_for` is not enough when the marker is also in the command the
    /// shell echoes back: it returns on the echo, before the output exists.
    fn wait_for_line(&mut self, needle: &str) -> String {
        let deadline = Instant::now() + DEADLINE;
        loop {
            self.pump();
            let screen = self.emulator.plain_text();
            if screen.lines().any(|line| line.trim() == needle) {
                return screen;
            }
            if Instant::now() >= deadline {
                panic!("timed out waiting for a line {needle:?}; the screen showed:\n{screen}");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn wait_until(&mut self, what: &str, mut condition: impl FnMut(&mut Self) -> bool) {
        let deadline = Instant::now() + DEADLINE;
        loop {
            self.pump();
            if condition(self) {
                return;
            }
            if Instant::now() >= deadline {
                panic!(
                    "timed out waiting for {what}; the screen showed:\n{}",
                    self.emulator.plain_text()
                );
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn alive(&self) -> bool {
        self.runtime
            .inspect(self.id, 0)
            .map(|snapshot| snapshot.alive)
            .unwrap_or(false)
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.runtime.stop(StopProcessAction {
            terminal_id: self.id,
            grace: Some(Duration::from_millis(200).into()),
        });
    }
}

/// PRD §46.2–46.4: type `pwd`, press Enter, see the result.
#[test]
fn typing_pwd_and_pressing_enter_shows_the_directory() {
    let mut session = Session::open();
    let expected = session
        .directory
        .path()
        .canonicalize()
        .expect("canonical path")
        .display()
        .to_string();
    session.type_line("pwd");
    let screen = session.wait_for(&expected);
    assert!(
        screen.contains(&expected),
        "pressing Enter must run the command the user typed"
    );
}

/// PRD §41: a command's exact output reaches the screen.
#[test]
fn a_command_produces_its_exact_output() {
    let mut session = Session::open();
    session.type_line("echo purrcode-terminal-test");
    let screen = session.wait_for_line("purrcode-terminal-test");
    assert_eq!(
        screen
            .lines()
            .filter(|line| line.trim() == "purrcode-terminal-test")
            .count(),
        1,
        "the output must appear once, not once per echo of the typed line:\n{screen}"
    );
}

/// PRD §41: Ctrl+C interrupts the running command without killing the shell.
///
/// This is the single most important behaviour in the PRD — §47 lists
/// "Ctrl+C kills wrong process" as a release blocker.
#[test]
fn ctrl_c_interrupts_the_command_and_leaves_the_shell_alive() {
    let mut session = Session::open();
    session.type_line("echo started; sleep 30");
    // A whole line, not a substring: "started" is also in the command the shell
    // echoes back, and interrupting before `sleep` is the foreground process
    // would send the signal to the shell instead — which is the very confusion
    // this test exists to rule out.
    session.wait_for_line("started");

    session.ctrl('c');

    // The shell survives, and proves it by running something else.
    session.wait_until("the shell to accept another command", |session| {
        session.send(b"echo still-here\n".to_vec());
        session.pump();
        session.emulator.plain_text().contains("still-here")
    });
    assert!(
        session.alive(),
        "Ctrl+C must interrupt the command, not close the terminal"
    );
}

/// PRD §41: shell history via the up arrow.
#[test]
fn the_up_arrow_recalls_the_previous_command() {
    // `sh` has no interactive line editing, so history is tested where it
    // actually lives: a shell that provides it. Skipping when bash is absent
    // keeps this honest rather than asserting something the platform cannot do.
    let Some(bash) = ["/bin/bash", "/usr/bin/bash"]
        .into_iter()
        .find(|path| std::path::Path::new(path).exists())
    else {
        eprintln!("no bash on this machine; skipping the history check");
        return;
    };

    let directory = tempfile::tempdir().expect("scratch directory");
    let runtime = TerminalRuntime::default();
    let started = runtime
        .start(
            WorkspaceId::new(),
            StartTerminalAction {
                program: Some(bash.into()),
                // Interactive, so readline is active and the arrow keys mean
                // something; `--norc` keeps a developer's own prompt out of it.
                arguments: vec!["--norc".into(), "-i".into()],
                working_directory: directory.path().canonicalize().expect("canonical"),
                environment: [
                    ("PS1".to_owned(), "$ ".to_owned()),
                    ("TERM".to_owned(), "xterm".to_owned()),
                ]
                .into_iter()
                .collect(),
                initial_size: TerminalSize { rows: 24, cols: 80 },
                owner: None,
                background: None,
            },
        )
        .expect("start bash");

    let mut emulator = TerminalEmulator::with_scrollback(24, 80, 500);
    let mut offset = 0u64;
    let pump = |emulator: &mut TerminalEmulator, offset: &mut u64| {
        let chunk = runtime
            .read_since(started.terminal_id, *offset)
            .expect("read");
        if !chunk.bytes.is_empty() {
            emulator.write(&chunk.bytes);
        }
        *offset = chunk.next_offset;
    };
    let send = |bytes: Vec<u8>| {
        runtime
            .send_input(SendTerminalInputAction {
                terminal_id: started.terminal_id,
                owner_generation: OwnershipGeneration::INITIAL,
                input: bytes,
            })
            .expect("send");
    };

    send(b"echo first-run\n".to_vec());
    let deadline = Instant::now() + DEADLINE;
    while !emulator.plain_text().contains("first-run") {
        pump(&mut emulator, &mut offset);
        assert!(
            Instant::now() < deadline,
            "bash never ran the first command"
        );
        std::thread::sleep(Duration::from_millis(10));
    }

    // Up recalls it; Enter runs it again.
    send(
        emulator
            .encode_key(KeyInput::key(TerminalKey::Up))
            .expect("arrow encoding"),
    );
    std::thread::sleep(Duration::from_millis(150));
    send(b"\r".to_vec());

    let deadline = Instant::now() + DEADLINE;
    loop {
        pump(&mut emulator, &mut offset);
        let screen = emulator.plain_text();
        let runs = screen.matches("first-run").count();
        // Three: the typed line, its output, and the recalled run's output.
        if runs >= 3 {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the up arrow did not recall the previous command; screen was:\n{screen}"
        );
        std::thread::sleep(Duration::from_millis(10));
    }

    let _ = runtime.stop(StopProcessAction {
        terminal_id: started.terminal_id,
        grace: Some(Duration::from_millis(200).into()),
    });
}

/// PRD §10, §41: resizing the panel changes the size the process sees.
#[test]
fn resizing_the_panel_reaches_the_process() {
    let mut session = Session::open_sized(TerminalSize { rows: 24, cols: 80 });
    session.type_line("stty size");
    session.wait_for("24 80");

    session
        .runtime
        .resize(ResizeTerminalAction {
            terminal_id: session.id,
            size: TerminalSize {
                rows: 40,
                cols: 132,
            },
        })
        .expect("resize the pty");
    session.emulator.resize(40, 132);

    session.type_line("stty size");
    let screen = session.wait_for("40 132");
    assert!(
        screen.contains("40 132"),
        "a resized panel must reach TIOCSWINSZ, or wrapping is wrong forever:\n{screen}"
    );
}

/// PRD §41: `exit` produces a clean exit status, not a hang.
#[test]
fn exiting_the_shell_reports_a_clean_exit() {
    let mut session = Session::open();
    session.type_line("exit 0");
    session.wait_until("the shell to exit", |session| !session.alive());
    let snapshot = session.runtime.inspect(session.id, 0).expect("inspect");
    assert!(!snapshot.alive);
    assert_eq!(
        snapshot.exit_code,
        Some(0),
        "a clean exit and a crash must be distinguishable"
    );
}

/// PRD §41: a non-zero exit is reported as itself.
#[test]
fn a_failing_command_reports_its_own_exit_code() {
    let mut session = Session::open();
    session.type_line("exit 3");
    session.wait_until("the shell to exit", |session| !session.alive());
    assert_eq!(
        session.runtime.inspect(session.id, 0).unwrap().exit_code,
        Some(3)
    );
}

/// PRD §41: UTF-8 survives the round trip.
#[test]
fn unicode_survives_the_round_trip() {
    let mut session = Session::open();
    session.type_line("printf '%s\\n' 'héllo — 世界 🐈'");
    let screen = session.wait_for("世界");
    assert!(
        screen.contains("héllo"),
        "combining and accented characters must survive:\n{screen}"
    );
}

/// PRD §41: colour reaches the cell attributes, and the escape sequence that
/// produced it never reaches the text.
#[test]
fn ansi_colour_lands_on_cells_and_not_in_the_text() {
    use purrcode_terminal_runtime::TerminalColor;

    let mut session = Session::open();
    session.type_line("printf '\\033[31mRED\\033[0m\\n'");
    let screen = session.wait_for_line("RED");
    // The shell echoes the command, so the literal escape appears in the typed
    // line; what matters is that the *output* line is the three characters and
    // nothing else.
    assert!(
        screen.lines().any(|line| line.trim() == "RED"),
        "the escape sequence must be interpreted, not printed:\n{screen}"
    );

    let coloured = (0..24).any(|row| {
        let cells = session.emulator.row(row);
        cells.windows(3).any(|window| {
            window.iter().map(|cell| cell.glyph()).collect::<String>() == "RED"
                && window[0].fg == TerminalColor::Ansi(1)
        })
    });
    assert!(coloured, "red must reach the cell, not just the characters");
}

/// PRD §41: a full-screen program is recognised as one.
#[test]
fn a_program_that_takes_the_alternate_screen_is_recognised() {
    let mut session = Session::open();
    session.type_line("printf '\\033[?1049h'");
    session.wait_until("the alternate screen", |session| {
        session.emulator.alternate_screen()
    });
    session.send(b"\x1b[?1049l".to_vec());
}

/// PRD §41, §11.3: detaching the client leaves the process running, and
/// reattaching restores what it printed in the meantime.
#[test]
fn reconnecting_restores_the_terminal_without_stopping_the_process() {
    let mut session = Session::open();
    session.type_line("echo before-detach");
    session.wait_for("before-detach");

    // The GUI closes: the client goes away, the PTY does not.
    session
        .runtime
        .detach(purrcode_terminal_runtime::DetachTerminalAction {
            terminal_id: session.id,
        })
        .expect("detach");
    session.type_line("echo while-away");

    // A new client attaches and replays the transcript the daemon kept.
    let attached = session
        .runtime
        .attach(purrcode_terminal_runtime::AttachTerminalAction {
            terminal_id: session.id,
            replay_bytes: 64 * 1024,
        })
        .expect("attach");
    assert!(attached.alive, "the process must survive a client leaving");

    let deadline = Instant::now() + DEADLINE;
    let fresh = loop {
        let replay = session
            .runtime
            .attach(purrcode_terminal_runtime::AttachTerminalAction {
                terminal_id: session.id,
                replay_bytes: 64 * 1024,
            })
            .expect("attach");
        let mut replayed = TerminalEmulator::with_scrollback(24, 80, 500);
        replayed.write(&replay.transcript_tail);
        if replayed.plain_text().contains("while-away") {
            break replayed;
        }
        assert!(
            Instant::now() < deadline,
            "the replayed transcript never showed the output produced while detached:\n{}",
            replayed.plain_text()
        );
        std::thread::sleep(Duration::from_millis(20));
    };
    assert!(
        fresh.plain_text().contains("before-detach"),
        "reconnecting must restore what came before, not only what came after"
    );
}

/// PRD §16, §39: after a human takes over, the agent's in-flight input is
/// rejected instead of interleaving with what the human is typing.
#[test]
fn taking_control_rejects_the_agents_stale_input() {
    let session = Session::open();
    let stale = session.runtime.inspect(session.id, 0).unwrap().generation;

    session
        .runtime
        .transfer_ownership(
            session.id,
            TerminalOwner::Agent {
                role: purrcode_terminal_runtime::AgentRoleLabel::new("Build Agent"),
            },
        )
        .expect("hand the terminal to the agent");
    session
        .runtime
        .transfer_ownership(session.id, TerminalOwner::Human)
        .expect("human takeover");

    let rejected = session.runtime.send_input(SendTerminalInputAction {
        terminal_id: session.id,
        owner_generation: stale,
        input: b"rm -rf /\n".to_vec(),
    });
    assert!(
        rejected.is_err(),
        "input issued before the takeover must not land in the human's shell"
    );
    assert!(
        session.alive(),
        "rejecting stale input must not kill the shell"
    );
}

/// PRD §38: what reaches a model is bounded, deduplicated, and honest about
/// what it left out.
#[test]
fn what_a_model_learns_from_a_terminal_is_bounded() {
    use purrcode_terminal_runtime::TerminalContextSummary;

    let mut session = Session::open();
    session.type_line("i=0; while [ $i -lt 60 ]; do echo \"error: build failed\"; i=$((i+1)); done; echo done-loop");
    let screen = session.wait_for("done-loop");

    let summary = TerminalContextSummary::from_transcript("make", Some(2), &screen);
    assert_eq!(summary.exit_status, Some(2));
    assert_eq!(
        summary.key_errors.len(),
        1,
        "sixty identical failures are one fact, not sixty"
    );
    assert!(
        summary.relevant_output.len() <= 20,
        "the whole scrollback must never reach a model"
    );
    assert!(
        !summary.key_errors.iter().any(|line| line.len() > 500),
        "a single pathological line must not smuggle the transcript through"
    );
}
