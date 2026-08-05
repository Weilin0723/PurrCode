//! Task modes and permission modes.
//!
//! PRD §36 accepts the release only when a user can select Ask, Auto or Full
//! Access and can see the active mode at all times. These drive the real binary
//! and read the header, because a mode that is stored but not shown is not a
//! mode the user can rely on.

use std::collections::BTreeMap;

use purrcode_tui_e2e::fake_daemon::{DaemonScript, ScriptedSession};
use purrcode_tui_e2e::fake_provider;
use purrcode_tui_e2e::harness::with_artifacts;
use purrcode_tui_e2e::{Harness, HarnessOptions, Key, assertions};
use serde_json::json;

const SESSION: &str = "modes-session";

fn ready() -> DaemonScript {
    let mut sessions = BTreeMap::new();
    let mut events = fake_provider::startup("Explain the retry path", "/repo");
    events.push(fake_provider::session_completed());
    sessions.insert(
        SESSION.to_owned(),
        ScriptedSession::new("completed").with_events(events),
    );
    DaemonScript {
        providers: vec![json!({"name": "local", "provider_type": "ollama", "local": true})],
        models: vec![json!({"id": "local/fake:1b", "default": true, "local": true})],
        sessions,
        ..DaemonScript::default()
    }
}

fn open_workbench() -> Harness {
    let workspace = purrcode_tui_e2e::fixtures::Workspace::new().expect("workspace");
    workspace
        .seed_ui_state(Some(SESSION), "")
        .expect("seed session");
    Harness::start_in(workspace, ready(), HarnessOptions::default()).expect("start workbench")
}

fn new_session(harness: &mut Harness) -> anyhow::Result<()> {
    harness.wait_for_text("An existing session was found")?;
    harness.key(Key::Char('n'))?;
    harness.wait_for_text("Ready for a task")?;
    Ok(())
}

#[test]
fn task_mode_changes_and_the_header_follows() {
    let mut harness = open_workbench();
    with_artifacts("modes-task", &mut harness, |harness| {
        new_session(harness)?;
        // Build is the default and is stated in the header, not implied.
        let screen = harness.wait_for_text("Build")?;
        assertions::assert_visible(&screen, &["PurrCode"]);

        harness.run_command("/mode plan")?;
        let screen = harness.wait_for_text("Plan mode")?;
        assertions::assert_visible(&screen, &["Plan"]);
        assertions::assert_no_overflow(&screen);

        // `/mode` with no argument cycles, so the mode is reachable without
        // remembering the vocabulary.
        harness.run_command("/mode")?;
        harness.wait_for_text("Build mode")?;

        // Ctrl+K reaches the same command as the palette.
        harness.key(Key::Ctrl('k'))?;
        harness.wait_for_text("Review mode")?;
        Ok(())
    });
}

#[test]
fn permission_mode_is_selectable_and_visible() {
    let mut harness = open_workbench();
    with_artifacts("modes-permission", &mut harness, |harness| {
        new_session(harness)?;
        // Ask is the safe default and the header says so.
        let screen = harness.wait_for_text("Ask")?;
        assertions::assert_no_overflow(&screen);

        harness.run_command("/permission auto")?;
        let screen = harness.wait_for_text("Auto.")?;
        assertions::assert_visible(&screen, &["Auto"]);

        harness.run_command("/permission full access")?;
        let screen = harness.wait_for_text("Full Access.")?;
        // The name invites a larger reading, so the surface has to say what it
        // does not grant.
        assertions::assert_readable(&screen, "it grants no new ones");
        assertions::assert_visible(&screen, &["Full Access"]);
        Ok(())
    });
}

#[test]
fn an_unknown_permission_mode_is_refused() {
    let mut harness = open_workbench();
    with_artifacts("modes-permission-rejected", &mut harness, |harness| {
        new_session(harness)?;
        harness.run_command("/permission auto")?;
        harness.wait_for_text("Auto.")?;

        // A permission value nobody can interpret must be refused, and the
        // previous choice must survive. Silently ignoring it would leave the
        // user believing they had changed the mode.
        harness.run_command("/permission root")?;
        let screen = harness.wait_for_text("is not a permission mode")?;
        assertions::assert_visible(&screen, &["Auto"]);
        assertions::assert_readable(&screen, "Choose Ask, Auto or Full Access");
        Ok(())
    });
}

#[test]
fn a_read_only_task_mode_is_sent_as_a_constraint_not_a_hint() {
    let mut harness = open_workbench();
    with_artifacts("modes-plan-only", &mut harness, |harness| {
        new_session(harness)?;
        // Explicit Auto is an authority choice, not permission to bypass a
        // read-only task mode. The request must preserve both contracts.
        harness.run_command("/permission auto")?;
        harness.wait_for_text("Auto.")?;
        harness.run_command("/mode ask")?;
        harness.wait_for_text("Ask mode")?;

        harness.type_text("Change the retry limit to five")?;
        harness.key(Key::Ctrl('g'))?;
        let requests = harness.wait_for_request("POST", "/v1/sessions")?;
        let body = requests
            .iter()
            .find(|request| request.method == "POST" && request.path == "/v1/sessions")
            .and_then(|request| request.body.clone())
            .unwrap_or_default();
        // The objective asks for a change; the mode forbids one. The canonical
        // task mode reaches the runtime directly instead of masquerading as a
        // Plan request through the legacy plan_only compatibility field.
        assert!(
            body.contains("\"task_mode\":\"ask\"") && body.contains("\"plan_only\":false"),
            "Ask mode must reach the daemon as its own read-only contract: {body}"
        );
        assert!(
            body.contains("\"permission_mode\":\"auto\"")
                && body.contains("\"authority_mode\":\"elevated\""),
            "the explicit permission mode must travel with the session: {body}"
        );
        Ok(())
    });
}

#[test]
fn status_shows_what_the_header_deliberately_omits() {
    let mut harness = open_workbench();
    with_artifacts("modes-status", &mut harness, |harness| {
        new_session(harness)?;
        // The header shows `repo/branch` and a bare model name; PRD §14 forbids
        // the path, the full id and the session id there. Hiding them is only
        // legitimate if they stay reachable.
        let header = harness.screen();
        assert!(
            !header.contains("local/fake:1b"),
            "the provider-qualified id belongs in /status, not the header"
        );

        harness.run_command("/status")?;
        let screen = harness.wait_for_text("Repository:")?;
        assertions::assert_readable(&screen, "Model: local/fake:1b");
        assertions::assert_visible(&screen, &["Branch:", "Permission:", "Sandbox:"]);
        Ok(())
    });
}
