//! The terminal surface, driven through a real PTY.
//!
//! These prove the claims the PRD makes about §19 that a unit test cannot: that
//! opening a terminal reaches the daemon, that escape sequences are interpreted
//! on the actual rendered screen rather than printed as text, that typing
//! reaches the process, and that control transfers to a human and back without
//! the process stopping.

use std::collections::BTreeMap;

use purrcode_tui_e2e::fake_daemon::{DaemonScript, ScriptedSession, ScriptedTerminal};
use purrcode_tui_e2e::fake_provider;
use purrcode_tui_e2e::harness::with_artifacts;
use purrcode_tui_e2e::{assertions, Harness, HarnessOptions, Key};
use serde_json::json;

const SESSION: &str = "terminal-session";

fn script_with(terminals: Vec<ScriptedTerminal>) -> DaemonScript {
    let mut sessions = BTreeMap::new();
    let mut events = fake_provider::startup("Run the tests", "/repo");
    events.push(fake_provider::session_completed());
    sessions.insert(
        SESSION.to_owned(),
        ScriptedSession::new("completed").with_events(events),
    );
    DaemonScript {
        providers: vec![json!({"name": "local", "provider_type": "ollama", "local": true})],
        models: vec![json!({"id": "local/fake:1b", "default": true, "local": true})],
        sessions,
        terminals,
        ..DaemonScript::default()
    }
}

fn attached(script: DaemonScript) -> Harness {
    let workspace = purrcode_tui_e2e::fixtures::Workspace::new().expect("workspace");
    workspace
        .seed_ui_state(Some(SESSION), "")
        .expect("seed session");
    Harness::start_in(workspace, script, HarnessOptions::default()).expect("start workbench")
}

/// Open a writable workbench.
///
/// A terminal belongs to the workspace, not to one durable session, so these
/// tests start a new session rather than opening the recovered one read-only —
/// read-only history deliberately refuses every command that can act.
fn open_workbench(harness: &mut Harness) -> anyhow::Result<()> {
    harness.wait_for_text("An existing session was found")?;
    harness.key(Key::Char('n'))?;
    harness.wait_for_text("PurrCode")?;
    Ok(())
}

#[test]
fn terminal_shows_real_pty_output() {
    // Output a build actually emits: a colour sequence, a progress line that
    // rewrites itself with a carriage return, and a cleared screen.
    let output = b"\x1b[32mCompiling\x1b[0m purrcode\r\n  0%\r 50%\r100% done\r\n".to_vec();
    let mut harness = attached(script_with(vec![ScriptedTerminal::new(
        "terminal-1",
        output,
    )]));
    with_artifacts("terminal-output", &mut harness, |harness| {
        open_workbench(harness)?;
        harness.run_command("/terminal")?;
        harness.wait_for_request("GET", "/v1/terminals")?;
        let screen = harness.wait_for_text("Compiling purrcode")?;

        // The escape sequences must have been applied, not printed.
        assertions::assert_visible(&screen, &["Terminal", "100% done"]);
        assert!(
            !screen.contains("[32m") && !screen.contains("\u{1b}"),
            "escape sequences reached the screen as text:\n{}",
            screen.text()
        );
        // A carriage-return progress line leaves one line, not three.
        assert!(
            !screen.contains("  0%") && !screen.contains(" 50%"),
            "the progress line was appended instead of rewritten:\n{}",
            screen.text()
        );
        // The surface says where typing goes and how to leave.
        assertions::assert_visible(&screen, &["Esc Close"]);
        Ok(())
    });
}

#[test]
fn typing_reaches_the_process() {
    let mut harness = attached(script_with(vec![ScriptedTerminal::new(
        "terminal-1",
        b"$ ",
    )]));
    with_artifacts("terminal-input", &mut harness, |harness| {
        open_workbench(harness)?;
        harness.run_command("/terminal")?;
        harness.wait_for_text("Terminal")?;

        harness.key(Key::Char('l'))?;
        harness.key(Key::Char('s'))?;
        harness.wait_for_request("POST", "/v1/terminals/terminal-1/input")?;
        // The fake PTY echoes, exactly as a real one in cooked mode does.
        let screen = harness.wait_for_text("$ ls")?;
        assertions::assert_no_overflow(&screen);
        Ok(())
    });
}

#[test]
fn terminal_control_transfers_to_the_human_and_back() {
    let mut harness = attached(script_with(vec![ScriptedTerminal::new(
        "terminal-1",
        b"running build\r\n",
    )]));
    with_artifacts("terminal-takeover", &mut harness, |harness| {
        open_workbench(harness)?;
        harness.run_command("/terminal")?;
        harness.wait_for_text("running build")?;

        // Inside the terminal every keystroke belongs to the process, so the
        // ownership command is issued from the conversation. That is the point:
        // a surface that intercepted typing would not be a terminal.
        harness.key(Key::Escape)?;
        harness.wait_for_text("Ready for a task")?;
        harness.run_command("/terminal-take")?;
        harness.wait_for_request("POST", "/v1/terminals/terminal-1/owner")?;
        harness.wait_for_text("You control this terminal")?;

        // Ctrl+W is the one key the terminal surface claims for itself, and it
        // hands control back without stopping the process.
        harness.run_command("/terminal")?;
        let screen = harness.wait_for_text("running build")?;
        assertions::assert_visible(&screen, &["Ctrl+W Return control to agent"]);
        harness.key(Key::Ctrl('w'))?;
        harness.wait_for_text("The agent controls this terminal again")?;
        Ok(())
    });
}

#[test]
fn an_exited_terminal_is_not_shown_as_running() {
    let mut terminal = ScriptedTerminal::new("terminal-1", b"tests failed\r\n");
    terminal.alive = false;
    let mut harness = attached(script_with(vec![terminal]));
    with_artifacts("terminal-exited", &mut harness, |harness| {
        open_workbench(harness)?;
        harness.run_command("/terminal")?;
        let screen = harness.wait_for_text("exited")?;
        assert!(
            !screen.contains("Typing goes to the process"),
            "an exited terminal must not advertise input:\n{}",
            screen.text()
        );
        Ok(())
    });
}

#[test]
fn studio_opens_on_the_active_session() {
    let mut harness = attached(script_with(Vec::new()));
    with_artifacts("terminal-studio", &mut harness, |harness| {
        open_workbench(harness)?;
        harness.run_command("/studio")?;
        // Studio is launched as a client of the same daemon. The workbench must
        // stay live and must not hand its terminal to the child process.
        let screen = harness.wait_for_text("Studio")?;
        assertions::assert_no_overflow(&screen);
        assertions::assert_visible(&screen, &["PurrCode"]);
        Ok(())
    });
}
