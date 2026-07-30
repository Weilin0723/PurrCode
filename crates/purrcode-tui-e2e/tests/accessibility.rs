//! Accessibility, narrow terminals and privacy visibility.
//!
//! These cover the release gates that are easy to regress invisibly: a 60-column
//! terminal staying usable, status surviving without colour, and the local/remote
//! privacy state never disappearing.

use std::collections::BTreeMap;

use purrcode_tui_e2e::fake_daemon::{DaemonScript, ScriptedSession};
use purrcode_tui_e2e::fake_provider;
use purrcode_tui_e2e::harness::with_artifacts;
use purrcode_tui_e2e::{assertions, Harness, HarnessOptions, Key};
use serde_json::json;

fn local_provider() -> DaemonScript {
    DaemonScript {
        providers: vec![json!({"name": "local", "provider_type": "ollama", "local": true})],
        models: vec![json!({"id": "local/fake:1b", "default": true, "local": true})],
        ..DaemonScript::default()
    }
}

fn remote_provider() -> DaemonScript {
    DaemonScript {
        providers: vec![json!({
            "name": "remote",
            "provider_type": "openai",
            "local": false,
            "credential_reference": "keychain:remote",
        })],
        models: vec![json!({"id": "remote/gpt-x", "default": true, "local": false})],
        ..DaemonScript::default()
    }
}

#[test]
fn narrow_terminal_keeps_one_focused_surface() {
    let mut harness = Harness::start_with(local_provider(), HarnessOptions::size(60, 24))
        .expect("start workbench");
    with_artifacts("accessibility-narrow", &mut harness, |harness| {
        let screen = harness.wait_for_text("Ready for a task")?;
        assertions::assert_no_overflow(&screen);
        assert_eq!(screen.width(), 60);

        // The inspector replaces the conversation rather than squeezing beside it.
        harness.key(Key::Ctrl('o'))?;
        let screen = harness.wait_for_text("Session status")?;
        assertions::assert_no_overflow(&screen);
        assertions::assert_absent(&screen, &["Ready for a task"]);

        harness.key(Key::Ctrl('o'))?;
        let screen = harness.wait_for_text("Ready for a task")?;
        assertions::assert_absent(&screen, &["Session status"]);
        Ok(())
    });
}

#[test]
fn an_approval_is_complete_and_readable_at_sixty_columns() {
    let (approval_events, _digest) = fake_provider::write_awaiting_approval(
        "narrow-action",
        "src/lib.rs",
        "pub fn narrow() {}\n",
        "/repo",
        "writes a file outside the plan",
    );
    let mut events = fake_provider::startup("Add a function", "/repo");
    events.extend(approval_events);
    let mut sessions = BTreeMap::new();
    sessions.insert(
        "narrow-session".to_owned(),
        ScriptedSession::new("active")
            .with_events(events)
            .awaiting("narrow-action"),
    );
    let script = DaemonScript {
        sessions,
        ..local_provider()
    };
    let workspace = purrcode_tui_e2e::fixtures::Workspace::new().expect("workspace");
    workspace
        .seed_ui_state(Some("narrow-session"), "")
        .expect("seed");
    let mut harness = Harness::start_in(workspace, script, HarnessOptions::size(60, 24))
        .expect("start workbench");
    with_artifacts("accessibility-narrow-approval", &mut harness, |harness| {
        harness.wait_for_text("An existing session was found")?;
        harness.key(Key::Char('r'))?;
        let screen = harness.wait_for_all(&["Approval required", "src/lib.rs"])?;
        assertions::assert_no_overflow(&screen);
        assertions::assert_visible(&screen, &["Digest", "Action"]);
        assertions::assert_readable(&screen, "A Approve exact action");
        Ok(())
    });
}

#[test]
fn monochrome_keeps_status_distinguishable() {
    let mut events = fake_provider::startup("Mixed outcomes", "/repo");
    events.push(fake_provider::validation("failed", "2 checks failed"));
    events.push(fake_provider::session_completed());
    let mut sessions = BTreeMap::new();
    sessions.insert(
        "mono-session".to_owned(),
        ScriptedSession::new("completed").with_events(events),
    );
    let script = DaemonScript {
        sessions,
        ..local_provider()
    };
    let workspace = purrcode_tui_e2e::fixtures::Workspace::new().expect("workspace");
    workspace
        .seed_ui_state(Some("mono-session"), "")
        .expect("seed");
    // NO_COLOR is the contract users rely on; PURRCODE_THEME confirms the
    // monochrome palette is reachable explicitly too.
    let options = HarnessOptions::default()
        .env("NO_COLOR", "1")
        .env("PURRCODE_THEME", "monochrome");
    let mut harness = Harness::start_in(workspace, script, options).expect("start workbench");
    with_artifacts("accessibility-monochrome", &mut harness, |harness| {
        harness.wait_for_text("An existing session was found")?;
        harness.key(Key::Char('o'))?;

        // Without colour, every state still carries a word.
        let screen = harness.wait_for_text("Validation failed")?;
        assertions::assert_visible(&screen, &["Validation failed"]);
        assertions::assert_readable(&screen, "uncertain");
        // Glyphs fall back to ASCII.
        assert!(
            screen.text().is_ascii(),
            "a non-ASCII glyph survived NO_COLOR:\n{}",
            screen.text()
        );
        assertions::assert_no_overflow(&screen);
        Ok(())
    });
}

#[test]
fn privacy_state_stays_visible() {
    let mut harness = Harness::start(local_provider()).expect("start workbench");
    with_artifacts("accessibility-privacy-local", &mut harness, |harness| {
        let screen = harness.wait_for_text("local-only")?;
        assertions::assert_absent(&screen, &["network"]);

        // The state survives shrinking to the narrowest supported width.
        harness.resize(60, 24)?;
        let screen = harness.wait_for_text("PurrCode")?;
        assertions::assert_readable(&screen, "local");
        assertions::assert_no_overflow(&screen);
        Ok(())
    });
}

#[test]
fn remote_inference_is_labelled_as_such() {
    let mut harness = Harness::start(remote_provider()).expect("start workbench");
    with_artifacts("accessibility-privacy-remote", &mut harness, |harness| {
        harness.wait_for_text("PurrCode")?;
        // A remote provider must never be presented as local-only.
        harness.run_command("/privacy")?;
        let screen = harness.wait_for_text("Privacy mode")?;
        assertions::assert_no_overflow(&screen);
        Ok(())
    });
}

#[test]
fn policy_refuses_switching_away_from_local_only() {
    let mut harness = Harness::start(local_provider()).expect("start workbench");
    with_artifacts("accessibility-privacy-policy", &mut harness, |harness| {
        harness.wait_for_text("local-only")?;
        harness.run_command("/privacy")?;
        // Toggling reports the new state explicitly; the header must agree with it
        // rather than continuing to claim local-only inference.
        let screen = harness.wait_for_wrapped("Privacy mode: mixed")?;
        assertions::assert_no_overflow(&screen);

        harness.run_command("/privacy")?;
        let screen = harness.wait_for_wrapped("Privacy mode: local-only")?;
        assertions::assert_visible(&screen, &["local-only"]);
        Ok(())
    });
}

#[test]
fn every_state_word_survives_without_colour() {
    let mut events = fake_provider::startup("Several states", "/repo");
    events.extend(
        fake_provider::write_awaiting_approval(
            "mono-action",
            "src/lib.rs",
            "x",
            "/repo",
            "needs approval",
        )
        .0,
    );
    let mut sessions = BTreeMap::new();
    sessions.insert(
        "states-session".to_owned(),
        ScriptedSession::new("active")
            .with_events(events)
            .awaiting("mono-action"),
    );
    let script = DaemonScript {
        sessions,
        ..local_provider()
    };
    let workspace = purrcode_tui_e2e::fixtures::Workspace::new().expect("workspace");
    workspace
        .seed_ui_state(Some("states-session"), "")
        .expect("seed");
    let mut harness = Harness::start_in(
        workspace,
        script,
        HarnessOptions::default().env("NO_COLOR", "1"),
    )
    .expect("start workbench");
    with_artifacts("accessibility-state-words", &mut harness, |harness| {
        harness.wait_for_text("An existing session was found")?;
        harness.key(Key::Char('r'))?;
        let screen = harness.wait_for_text("Approval required")?;
        // A decision must be identifiable from the words alone.
        assertions::assert_visible(&screen, &["Approval required", "A Approve exact action"]);
        assert!(screen.text().is_ascii(), "{}", screen.text());
        Ok(())
    });
}

#[test]
fn the_palette_remains_usable_without_colour_or_unicode() {
    let mut harness = Harness::start_with(
        local_provider(),
        HarnessOptions::size(80, 24).env("NO_COLOR", "1"),
    )
    .expect("start workbench");
    with_artifacts("accessibility-palette-mono", &mut harness, |harness| {
        harness.wait_for_text("Ready for a task")?;
        harness.key(Key::Ctrl('p'))?;
        let screen = harness.wait_for_text("Commands")?;
        assertions::assert_visible(&screen, &["TASK"]);
        assertions::assert_no_overflow(&screen);
        assert!(screen.text().is_ascii(), "{}", screen.text());
        Ok(())
    });
}

#[test]
fn resizing_between_every_supported_width_never_overflows() {
    let mut harness = Harness::start_with(local_provider(), HarnessOptions::size(160, 40))
        .expect("start workbench");
    with_artifacts("accessibility-resize-sweep", &mut harness, |harness| {
        harness.wait_for_text("Ready for a task")?;
        for (columns, rows) in [(60, 24), (80, 24), (120, 30), (160, 40), (60, 24)] {
            harness.resize(columns, rows)?;
            let screen = harness.wait_for_text("PurrCode")?;
            assertions::assert_no_overflow(&screen);
            assert_eq!(screen.width(), columns);
            // Privacy is the invariant that must never be dropped for space.
            assertions::assert_readable(&screen, "local");
        }
        Ok(())
    });
}

#[test]
fn a_very_short_terminal_still_shows_the_composer_and_hints() {
    let mut harness = Harness::start_with(local_provider(), HarnessOptions::size(80, 10))
        .expect("start workbench");
    with_artifacts("accessibility-short", &mut harness, |harness| {
        let screen = harness.wait_for_text("PurrCode")?;
        assertions::assert_no_overflow(&screen);
        // Even at ten rows the user can see where to type and what to press.
        assertions::assert_visible(&screen, &["Ask PurrCode"]);
        assertions::assert_readable(&screen, "Ctrl+G Send");
        Ok(())
    });
}
