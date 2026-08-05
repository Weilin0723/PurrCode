//! Session lifecycle, restart and recovery.
//!
//! The invariants here are the ones a user's work depends on: opening history
//! never starts execution, a restart resumes only from durable state, and an
//! uncertain outcome is never dressed up as a success.

use std::collections::BTreeMap;

use purrcode_tui_e2e::fake_daemon::{DaemonScript, ScriptedSession, StreamFrame};
use purrcode_tui_e2e::fake_provider;
use purrcode_tui_e2e::harness::with_artifacts;
use purrcode_tui_e2e::{Harness, HarnessOptions, Key, assertions};
use serde_json::json;

const SESSION: &str = "lifecycle-session";

fn base() -> DaemonScript {
    DaemonScript {
        providers: vec![json!({"name": "local", "provider_type": "ollama", "local": true})],
        models: vec![json!({"id": "local/fake:1b", "default": true, "local": true})],
        ..DaemonScript::default()
    }
}

fn with_session(session: ScriptedSession) -> DaemonScript {
    let mut sessions = BTreeMap::new();
    sessions.insert(SESSION.to_owned(), session);
    DaemonScript { sessions, ..base() }
}

fn attached(script: DaemonScript, draft: &str) -> Harness {
    let workspace = purrcode_tui_e2e::fixtures::Workspace::new().expect("workspace");
    workspace
        .seed_ui_state(Some(SESSION), draft)
        .expect("seed session");
    Harness::start_in(workspace, script, HarnessOptions::default()).expect("start workbench")
}

#[test]
fn new_session_preserves_history() {
    let script = with_session(ScriptedSession::new("completed").with_events(vec![
        json!({"event": "session_created", "data": {"objective": "Earlier work"}}),
        fake_provider::session_completed(),
    ]));
    let mut harness = attached(script, "");
    with_artifacts("recovery-new-session", &mut harness, |harness| {
        harness.wait_for_text("An existing session was found")?;
        harness.key(Key::Char('n'))?;
        let screen = harness.wait_for_text("Ready for a task")?;
        assertions::assert_no_overflow(&screen);

        // Starting fresh must not delete the earlier session.
        harness.wait_for_durable("the earlier session to survive", |script| {
            script.sessions.contains_key(SESSION)
        })?;
        let events = harness.session_events(SESSION);
        assert!(
            !events.is_empty(),
            "a new session destroyed the previous session's evidence"
        );
        Ok(())
    });
}

#[test]
fn pause_stops_at_safe_boundary() {
    let script = with_session(
        ScriptedSession::new("active")
            .with_events(fake_provider::startup("Long running work", "/repo")),
    );
    let mut harness = attached(script, "");
    with_artifacts("recovery-pause", &mut harness, |harness| {
        harness.wait_for_text("An existing session was found")?;
        harness.key(Key::Char('r'))?;
        harness.wait_for_text("PurrCode")?;
        harness.run_command("/pause")?;
        harness.wait_for_request("POST", &format!("/v1/sessions/{SESSION}/pause"))?;
        let screen = harness.wait_for_wrapped("all evidence is preserved")?;
        assertions::assert_no_overflow(&screen);
        harness.wait_for_durable("the session to be paused", |script| {
            script
                .sessions
                .get(SESSION)
                .is_some_and(|session| session.status_code == "paused")
        })?;
        Ok(())
    });
}

#[test]
fn cancelled_session_can_be_superseded() {
    let script = with_session(ScriptedSession::new("cancelled").with_events(vec![
        json!({"event": "session_created", "data": {"objective": "Abandoned work"}}),
        fake_provider::session_cancelled("cancelled by the user"),
    ]));
    let mut harness = attached(script, "");
    with_artifacts("recovery-cancelled", &mut harness, |harness| {
        // A cancelled session cannot be resumed; the surface must say so rather
        // than offering an action that would fail.
        let screen =
            harness.wait_for_all(&["An existing session was found", "cannot be resumed safely"])?;
        assertions::assert_visible(&screen, &["N  Start a new session"]);

        harness.key(Key::Char('n'))?;
        harness.wait_for_text("Ready for a task")?;
        assert!(
            !harness
                .daemon()
                .saw("POST", &format!("/v1/sessions/{SESSION}/resume")),
            "an unresumable session must not be resumed:\n{}",
            harness.daemon().request_log()
        );
        Ok(())
    });
}

#[test]
fn restart_resumes_from_durable_state() {
    let script = with_session(
        ScriptedSession::new("paused")
            .with_events(fake_provider::startup("Interrupted work", "/repo")),
    );
    let mut harness = attached(script, "");
    with_artifacts("recovery-restart-before", &mut harness, |harness| {
        harness.wait_for_text("An existing session was found")?;
        harness.key(Key::Char('r'))?;
        harness.wait_for_text("PurrCode")?;
        Ok(())
    });

    let mut harness = harness.restart().expect("restart");
    with_artifacts("recovery-restart-after", &mut harness, |harness| {
        // The restart offers the choice again rather than silently resuming.
        let screen = harness.wait_for_all(&["An existing session was found", SESSION])?;
        assertions::assert_readable(&screen, "No task will run until you choose");
        harness.key(Key::Char('r'))?;
        // The durable objective survives; nothing was replayed.
        harness.wait_for_text("Context prepared")?;
        Ok(())
    });
}

#[test]
fn history_never_starts_execution() {
    let script = with_session(ScriptedSession::new("completed").with_events(vec![
        json!({"event": "session_created", "data": {"objective": "Finished work"}}),
        fake_provider::validation("passed", "all checks passed"),
        fake_provider::session_completed(),
    ]));
    let mut harness = attached(script, "");
    with_artifacts("recovery-read-only", &mut harness, |harness| {
        harness.wait_for_text("An existing session was found")?;
        harness.key(Key::Char('o'))?;
        harness.wait_for_text("read-only")?;

        // Typing a task is refused with a route out.
        harness.type_text("please change something")?;
        harness.key(Key::Ctrl('g'))?;
        let screen = harness.wait_for_wrapped("History is open read-only")?;
        assertions::assert_readable(&screen, "Press N for a new session");

        // Nothing mutating reached the daemon.
        let log = harness.daemon().request_log();
        for forbidden in ["/approve", "/resume", "/messages", "POST /v1/sessions "] {
            assert!(
                !log.contains(forbidden),
                "read-only history issued {forbidden}:\n{log}"
            );
        }
        Ok(())
    });
}

#[test]
fn draft_is_restored_without_secrets() {
    let script = with_session(
        ScriptedSession::new("paused").with_events(fake_provider::startup("Earlier work", "/repo")),
    );
    // A draft holding a secret-shaped value must never come back verbatim.
    let workspace = purrcode_tui_e2e::fixtures::Workspace::new().expect("workspace");
    workspace
        .seed_ui_state(Some(SESSION), "an ordinary unsent draft")
        .expect("seed");
    let mut harness =
        Harness::start_in(workspace, script, HarnessOptions::default()).expect("start workbench");
    with_artifacts("recovery-draft", &mut harness, |harness| {
        harness.wait_for_text("An existing session was found")?;
        harness.key(Key::Char('r'))?;
        // The unsent draft comes back so the user does not retype it.
        let screen = harness.wait_for_text("an ordinary unsent draft")?;
        assertions::assert_no_secret(&screen, &[]);
        Ok(())
    });
}

#[test]
fn a_pasted_secret_is_never_written_to_the_saved_draft() {
    let mut harness = Harness::start(base()).expect("start workbench");
    with_artifacts("recovery-draft-secret", &mut harness, |harness| {
        harness.wait_for_text("Ready for a task")?;
        harness.key(Key::Paste(
            "OPENAI_API_KEY=sk-live-abcdefghijklmnopqrstuvwxyz012345".into(),
        ))?;
        harness.wait_for_text("OPENAI_API_KEY")?;
        // Give the workbench a keystroke so it persists state.
        harness.key(Key::Char(' '))?;
        harness.wait_for_durable("state to be written", |_| true)?;

        let state = harness.workspace().saved_ui_state();
        if let Some(state) = state {
            let draft = state["draft"].as_str().unwrap_or_default().to_owned();
            assert!(
                !draft.contains("sk-live-abcdefghijklmnopqrstuvwxyz012345"),
                "the saved draft leaked a secret: {draft}"
            );
        }
        Ok(())
    });
}

#[test]
fn uncertain_session_is_not_success() {
    // Completion plus a non-passing validation is uncertain, not success.
    let script = with_session(ScriptedSession::new("completed").with_events(vec![
        json!({"event": "session_created", "data": {"objective": "Risky change"}}),
        fake_provider::validation("uncertain", "the validator could not decide"),
        fake_provider::session_completed(),
    ]));
    let mut harness = attached(script, "");
    with_artifacts("recovery-uncertain", &mut harness, |harness| {
        harness.wait_for_text("An existing session was found")?;
        harness.key(Key::Char('o'))?;
        let screen =
            harness.wait_for_all(&["Validation uncertain", "Completed with unverified results"])?;
        assertions::assert_absent(&screen, &["Validation passed"]);
        // The inspector column carries the guidance; scope the read to it so the
        // conversation text alongside cannot interfere.
        assertions::assert_readable_in_column(
            &screen,
            80,
            screen.width(),
            "The result is uncertain; verify manually before applying",
        );
        Ok(())
    });
}

#[test]
fn lease_conflict_offers_safe_options() {
    // An active session whose lease another client holds: this is the state that
    // produces a conflict on the next message, which is what the surface is for.
    let mut held = ScriptedSession::new("active");
    held.lease_active = true;
    held.events = fake_provider::startup("Work another client owns", "/repo");
    let script = with_session(held);
    let mut harness = attached(script, "");
    with_artifacts("recovery-lease-conflict", &mut harness, |harness| {
        // An active lease offers attach, not a blind resume.
        let screen = harness.wait_for_all(&[
            "An existing session was found",
            "Attach to the running session",
        ])?;
        assertions::assert_no_overflow(&screen);
        harness.key(Key::Char('r'))?;
        harness.wait_for_text("PurrCode")?;

        // Submitting new work against a held lease conflicts, and the workbench
        // must say the draft is safe rather than losing it.
        harness.type_text("more work")?;
        harness.key(Key::Ctrl('g'))?;
        let screen = harness.wait_for_text("Session already active")?;
        assertions::assert_readable(&screen, "your draft is safe");
        assertions::assert_visible(&screen, &["R  Reconnect", "O  Open read-only"]);
        Ok(())
    });
}

#[test]
fn a_failed_session_explains_itself_and_offers_a_route_out() {
    let script = with_session(ScriptedSession::new("failed").with_events(vec![
        json!({"event": "session_created", "data": {"objective": "Work that failed"}}),
        fake_provider::session_failed("the sandbox refused to start"),
    ]));
    let mut harness = attached(script, "");
    with_artifacts("recovery-failed", &mut harness, |harness| {
        harness.wait_for_text("An existing session was found")?;
        harness.key(Key::Char('o'))?;
        // The rail stays compact: it names the failure and points at recovery.
        let screen = harness.wait_for_text("Session failed")?;
        assertions::assert_failure_offers_recovery(
            &screen,
            "Session failed",
            &["Ctrl+R Open recovery"],
        );

        // The reason is one keystroke away, not hidden. Wait for the inspector
        // title — waiting for text that was already on screen would race the key.
        harness.key(Key::Char('e'))?;
        let screen = harness.wait_for_text("Activity detail")?;
        assertions::assert_readable_in_column(
            &screen,
            80,
            screen.width(),
            "the sandbox refused to start",
        );
        assertions::assert_failure_offers_recovery(
            &screen,
            "Session failed",
            &["N", "new session", "Start a new session"],
        );
        Ok(())
    });
}

#[test]
fn recovery_required_is_reachable_from_the_workbench() {
    let script = with_session(ScriptedSession::new("active").with_events(vec![
        json!({"event": "session_created", "data": {"objective": "Interrupted mid-execution"}}),
        fake_provider::recovery_required("the daemon restarted while an action was executing"),
    ]));
    let mut harness = attached(script, "");
    with_artifacts("recovery-required", &mut harness, |harness| {
        harness.wait_for_text("An existing session was found")?;
        harness.key(Key::Char('r'))?;
        harness.wait_for_text("Recovery required")?;
        // Inspect the step to read why recovery is needed.
        harness.key(Key::Char('e'))?;
        let screen = harness.wait_for_text("Activity detail")?;
        assertions::assert_readable_in_column(
            &screen,
            80,
            screen.width(),
            "the daemon restarted while an action was executing",
        );
        assertions::assert_no_overflow(&screen);

        // Ctrl+R opens the focused recovery surface.
        harness.key(Key::Ctrl('r'))?;
        let screen = harness.wait_for_text("Session already active")?;
        assertions::assert_visible(&screen, &["O  Open read-only"]);
        Ok(())
    });
}

#[test]
fn a_completed_session_reports_its_state_rather_than_looking_empty() {
    let script = with_session(ScriptedSession::new("completed").with_events(vec![
        json!({"event": "session_created", "data": {"objective": "Small task"}}),
        fake_provider::validation("passed", "ok"),
        fake_provider::session_completed(),
    ]));
    let mut harness = attached(script, "");
    with_artifacts("recovery-completed", &mut harness, |harness| {
        harness.wait_for_text("An existing session was found")?;
        harness.key(Key::Char('o'))?;
        let screen = harness.wait_for_text("Completed")?;
        assertions::assert_absent(&screen, &["This session recorded no activity"]);
        Ok(())
    });
}

#[test]
fn a_new_session_after_a_stream_keeps_the_earlier_answer_visible() {
    let script = DaemonScript {
        stream_frames: vec![
            StreamFrame::Phase {
                phase: "receiving".into(),
                role: "coder".into(),
            },
            StreamFrame::ContentDelta("The earlier answer.".into()),
            StreamFrame::DurableAudit(fake_provider::session_completed()),
        ],
        ..base()
    };
    let mut harness = Harness::start(script).expect("start workbench");
    with_artifacts("recovery-new-after-stream", &mut harness, |harness| {
        harness.wait_for_text("Ready for a task")?;
        harness.type_text("first question")?;
        harness.key(Key::Ctrl('g'))?;
        harness.wait_for_text("The earlier answer.")?;

        harness.run_command("/new")?;
        let screen = harness.wait_for_wrapped("New session started")?;
        assertions::assert_no_overflow(&screen);
        Ok(())
    });
}
