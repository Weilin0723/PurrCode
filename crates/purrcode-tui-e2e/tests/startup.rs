//! Startup, repository state, palette discovery and settings.
//!
//! Each test drives the real `purrcode` binary in a real PTY and asserts on the
//! parsed screen. Nothing sleeps-and-hopes: every wait is bounded and reports the
//! screen it saw.

use std::collections::BTreeMap;

use purrcode_tui_e2e::fake_daemon::{DaemonScript, ScriptedSession};
use purrcode_tui_e2e::fake_provider;
use purrcode_tui_e2e::harness::with_artifacts;
use purrcode_tui_e2e::{Harness, HarnessOptions, Key, assertions};
use serde_json::json;

/// A daemon with one local provider and one default model: the state a user is in
/// immediately after a successful `purrcode init`.
fn configured() -> DaemonScript {
    DaemonScript {
        providers: vec![json!({
            "name": "local",
            "provider_type": "ollama",
            "base_url": "http://127.0.0.1:11434",
            "local": true,
        })],
        models: vec![json!({
            "id": "local/fake:1b",
            "default": true,
            "local": true,
        })],
        ..DaemonScript::default()
    }
}

#[test]
fn first_launch_shows_workbench_and_next_action() {
    let mut harness = Harness::start(configured()).expect("start workbench");
    with_artifacts("startup-first-launch", &mut harness, |harness| {
        let screen = harness.wait_for_text("PurrCode")?;
        assertions::assert_visible(&screen, &["PurrCode"]);

        // The workbench must tell a new user what to do without documentation.
        let screen = harness.wait_for_text("Ready for a task")?;
        assertions::assert_readable(
            &screen,
            "Describe what you want changed, ask a question about this repository",
        );
        assertions::assert_visible(&screen, &["Ctrl+G"]);
        assertions::assert_no_overflow(&screen);

        // Raw runtime identifiers stay out of the default view.
        assertions::assert_absent(&screen, &["session_created", "context_indexed", "{\""]);
        Ok(())
    });
}

#[test]
fn the_header_orients_without_internal_metadata() {
    let mut harness = Harness::start(configured()).expect("start workbench");
    with_artifacts("startup-header", &mut harness, |harness| {
        let screen = harness.wait_for_all(&["PurrCode", "fake:1b"])?;
        assertions::assert_visible(&screen, &["main", "Build"]);
        // The provider prefix, sandbox backend, daemon version and worktree path
        // belong to `/status`, not to the line the user reads on every frame
        // (PRD §10.1, §14).
        assertions::assert_absent(
            &screen,
            &["local/fake:1b", "seatbelt", "worktree", "daemon.token"],
        );
        Ok(())
    });
}

#[test]
fn daemon_unavailable_reports_recovery_action() {
    let script = DaemonScript {
        unreachable: true,
        ..configured()
    };
    let mut harness = Harness::start(script).expect("start workbench");
    with_artifacts("startup-daemon-unavailable", &mut harness, |harness| {
        let screen = harness.wait_for_all(&[
            "cannot reach its local daemon",
            "purrcode serve",
            "reconnect",
        ])?;
        // A failure with no recovery path is a dead end; this must offer one.
        assertions::assert_failure_offers_recovery(
            &screen,
            "cannot reach its local daemon",
            &["purrcode serve", "reconnect"],
        );
        assertions::assert_readable(&screen, "Your draft and durable sessions are safe");
        Ok(())
    });
}

#[test]
fn a_missing_provider_names_the_command_that_fixes_it() {
    let script = DaemonScript {
        providers: Vec::new(),
        models: Vec::new(),
        ..DaemonScript::default()
    };
    let mut harness = Harness::start(script).expect("start workbench");
    with_artifacts("startup-no-provider", &mut harness, |harness| {
        let screen = harness.wait_for_text("Connect a model provider")?;
        assertions::assert_visible(&screen, &["/connect"]);
        Ok(())
    });
}

#[test]
fn a_dirty_repository_is_reported_as_preserved() {
    let script = DaemonScript {
        repository_dirty: true,
        ..configured()
    };
    let mut harness = Harness::start(script).expect("start workbench");
    with_artifacts("startup-dirty-repository", &mut harness, |harness| {
        harness.workspace().dirty()?;
        harness.wait_for_text("PurrCode")?;
        harness.key(Key::Ctrl('o'))?;
        let screen = harness.wait_for_text("dirty")?;
        assertions::assert_readable(&screen, "preserved");
        Ok(())
    });
}

#[test]
fn recovered_session_requires_explicit_choice() {
    let mut sessions = BTreeMap::new();
    sessions.insert(
        "recovered-session".to_owned(),
        ScriptedSession::new("paused").with_events(fake_provider::startup("Earlier task", "/repo")),
    );
    let script = DaemonScript {
        sessions,
        ..configured()
    };
    let workspace = purrcode_tui_e2e::fixtures::Workspace::new().expect("workspace");
    workspace
        .seed_ui_state(Some("recovered-session"), "")
        .expect("seed saved session");
    let mut harness =
        Harness::start_in(workspace, script, HarnessOptions::default()).expect("start workbench");
    with_artifacts("startup-session-choice", &mut harness, |harness| {
        // Wait for the whole surface, not just its first line: reading the
        // parser mid-repaint can otherwise catch a torn frame.
        let screen = harness.wait_for_all(&[
            "An existing session was found",
            "recovered-session",
            "R  Resume",
            "N  Start a new session",
        ])?;
        // The guarantee that nothing runs is the point of this screen.
        assertions::assert_readable(&screen, "No task will run until you choose");

        // Opening the workbench must not have started or resumed anything.
        let requests = harness.daemon().requests();
        assert!(
            !requests.iter().any(|request| request.method == "POST"
                && (request.path.contains("/resume") || request.path.contains("/messages"))),
            "startup performed a mutating request:\n{}",
            harness.daemon().request_log()
        );
        Ok(())
    });
}

#[test]
fn palette_lists_grouped_actions() {
    let mut harness = Harness::start(configured()).expect("start workbench");
    with_artifacts("startup-palette", &mut harness, |harness| {
        harness.wait_for_text("Ready for a task")?;
        harness.key(Key::Ctrl('p'))?;
        // Wait for the entries, not just the frame title: a frame arrives over
        // several PTY reads, so the palette's border can be on screen while its
        // list is still being written.
        let screen = harness.wait_for_all(&["Commands", "TASK", "Send task", "Search"])?;

        // Grouped by category, generated from the action registry.
        assertions::assert_visible(&screen, &["TASK", "Send task", "Search"]);
        assertions::assert_no_overflow(&screen);

        // Filtering narrows to the matching action and shows its command.
        harness.type_text("diff")?;
        let screen = harness.wait_for_text("Review diff")?;
        assertions::assert_visible(&screen, &["/diff"]);
        Ok(())
    });
}

#[test]
fn unavailable_palette_entries_explain_why() {
    let mut harness = Harness::start(configured()).expect("start workbench");
    with_artifacts("startup-palette-unavailable", &mut harness, |harness| {
        harness.wait_for_text("Ready for a task")?;
        harness.key(Key::Ctrl('p'))?;
        harness.wait_for_text("Commands")?;
        harness.type_text("approve")?;

        // Visible, but honest about not being runnable — never a silent dead end.
        harness.wait_for_all(&[
            "Search approve",
            "6 action(s) matching",
            "Approve action",
            "unavailable",
            "no action is pending",
        ])?;
        Ok(())
    });
}

#[test]
fn an_unavailable_action_run_from_the_palette_explains_itself_and_does_nothing() {
    let mut harness = Harness::start(configured()).expect("start workbench");
    with_artifacts("startup-palette-refusal", &mut harness, |harness| {
        harness.wait_for_text("Ready for a task")?;
        harness.key(Key::Ctrl('p'))?;
        harness.wait_for_text("Commands")?;
        harness.type_text("approve")?;
        harness.wait_for_all(&[
            "Search approve",
            "6 action(s) matching",
            "Approve action",
            "unavailable",
            "no action is pending",
        ])?;
        harness.key(Key::Down)?;
        harness.key(Key::Down)?;
        harness.key(Key::Enter)?;

        harness.wait_for_wrapped("Approve action is unavailable: no action is pending.")?;
        // Crucially: no approval reached the daemon.
        assert!(
            !harness.daemon().saw("POST", "/approve"),
            "an unavailable action must not call the daemon:\n{}",
            harness.daemon().request_log()
        );
        Ok(())
    });
}

#[test]
fn settings_report_live_state() {
    let mut harness = Harness::start(configured()).expect("start workbench");
    with_artifacts("startup-settings", &mut harness, |harness| {
        harness.wait_for_text("Ready for a task")?;
        harness.run_command("/settings")?;
        harness.wait_for_request("GET", "/v1/providers")?;
        let screen = harness.wait_for_text("local")?;
        assertions::assert_no_secret(&screen, &[]);
        Ok(())
    });
}

#[test]
fn an_unknown_command_is_refused_without_side_effects() {
    let mut harness = Harness::start(configured()).expect("start workbench");
    with_artifacts("startup-unknown-command", &mut harness, |harness| {
        harness.wait_for_text("Ready for a task")?;
        harness.run_command("/definitelynotacommand")?;
        let screen = harness.wait_for_text("Unknown command")?;
        assertions::assert_visible(&screen, &["/help"]);
        assert!(
            !harness.daemon().saw("POST", "/v1/sessions"),
            "an unknown command must not create a session:\n{}",
            harness.daemon().request_log()
        );
        Ok(())
    });
}

#[test]
fn the_workbench_survives_a_resize_to_sixty_columns() {
    let mut harness =
        Harness::start_with(configured(), HarnessOptions::size(140, 36)).expect("start workbench");
    with_artifacts("startup-resize", &mut harness, |harness| {
        harness.wait_for_text("Ready for a task")?;
        harness.resize(60, 24)?;
        let screen = harness.wait_for_text("PurrCode")?;
        assertions::assert_no_overflow(&screen);
        assert_eq!(screen.width(), 60);
        // Privacy state is the one thing that must survive every width.
        assertions::assert_readable(&screen, "local");
        Ok(())
    });
}
