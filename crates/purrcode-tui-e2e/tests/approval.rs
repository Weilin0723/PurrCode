//! The approval decision boundary.
//!
//! These are the tests the release gate depends on. Each one proves something
//! about authority, not about appearance:
//!
//! * pressing `A` calls the daemon's approval endpoint;
//! * the displayed action is the action the daemon holds;
//! * a digest mismatch fails visibly and submits nothing;
//! * rejection never executes;
//! * a restart preserves the pending boundary;
//! * natural-language text never silently approves.

use std::collections::BTreeMap;

use purrcode_tui_e2e::fake_daemon::{DaemonScript, ScriptedSession};
use purrcode_tui_e2e::fake_provider;
use purrcode_tui_e2e::harness::with_artifacts;
use purrcode_tui_e2e::{assertions, Harness, HarnessOptions, Key};
use serde_json::{json, Value};

const SESSION: &str = "approval-session";
const ACTION: &str = "action-under-review";

fn configured_models() -> Vec<Value> {
    vec![json!({"id": "local/fake:1b", "default": true, "local": true})]
}

/// A session parked on a write-approval boundary, with the digest the runtime
/// computed for that exact action.
fn awaiting_write() -> (DaemonScript, String) {
    let (approval_events, digest) = fake_provider::write_awaiting_approval(
        ACTION,
        "src/lib.rs",
        "pub fn added() {}\n",
        "/repo",
        "writes a file the plan did not declare",
    );
    let mut events = fake_provider::startup("Add a helper function", "/repo");
    events.push(fake_provider::plan(&[
        "Inspect src/lib.rs",
        "Add the helper",
    ]));
    events.extend(approval_events);

    let mut sessions = BTreeMap::new();
    sessions.insert(
        SESSION.to_owned(),
        ScriptedSession::new("active")
            .with_events(events)
            .awaiting(ACTION),
    );
    let script = DaemonScript {
        providers: vec![json!({"name": "local", "provider_type": "ollama", "local": true})],
        models: configured_models(),
        sessions,
        ..DaemonScript::default()
    };
    (script, digest)
}

/// Start the workbench already attached to the pending session.
fn attached(script: DaemonScript, options: HarnessOptions) -> Harness {
    let workspace = purrcode_tui_e2e::fixtures::Workspace::new().expect("workspace");
    workspace
        .seed_ui_state(Some(SESSION), "")
        .expect("seed session");
    Harness::start_in(workspace, script, options).expect("start workbench")
}

/// Resume the recovered session and wait for the focused approval surface.
///
/// A pending boundary takes over the frame, so the surface — not the activity
/// rail — is what a test observes.
fn resume_to_approval(harness: &mut Harness) -> anyhow::Result<()> {
    harness.wait_for_text("An existing session was found")?;
    harness.key(Key::Char('r'))?;
    harness.wait_for_text("Approval required")?;
    Ok(())
}

/// Resume a session with no pending boundary.
fn resume(harness: &mut Harness) -> anyhow::Result<()> {
    harness.wait_for_text("An existing session was found")?;
    harness.key(Key::Char('r'))?;
    Ok(())
}

#[test]
fn approval_shows_paths_scope_and_limits() {
    let (script, digest) = awaiting_write();
    let mut harness = attached(script, HarnessOptions::default());
    with_artifacts("approval-detail", &mut harness, |harness| {
        resume_to_approval(harness)?;
        let screen = harness.wait_for_all(&["Approval required", "src/lib.rs"])?;
        assertions::assert_no_overflow(&screen);
        assertions::assert_visible(
            &screen,
            &[
                "Write a file",
                "src/lib.rs",
                "modifies files",
                "no network",
                ACTION,
            ],
        );
        assertions::assert_readable(&screen, "writes a file the plan did not declare");
        assertions::assert_readable(&screen, "at most 1 changed file(s)");
        // The digest must be shown in full so it can be compared by eye.
        assert!(
            screen.contains(&digest),
            "the exact digest {digest} was not visible:\n{}",
            screen.text()
        );
        // The decision keys are on screen, not memorized.
        assertions::assert_visible(&screen, &["A Approve exact action", "R Reject"]);
        Ok(())
    });
}

#[test]
fn approve_key_authorizes_the_displayed_action() {
    let (script, _digest) = awaiting_write();
    let mut harness = attached(script, HarnessOptions::default());
    with_artifacts("approval-approve", &mut harness, |harness| {
        resume_to_approval(harness)?;

        harness.key(Key::Char('a'))?;

        // The key must reach the daemon's approval endpoint, not merely change
        // the screen.
        harness.wait_for_request("POST", &format!("/v1/sessions/{SESSION}/approve"))?;
        harness.wait_for_durable("the authorization to be recorded", |script| {
            script.sessions.get(SESSION).is_some_and(|session| {
                session.awaiting_action.is_none()
                    && session
                        .events
                        .iter()
                        .any(|event| event["event"] == "approval_recorded")
            })
        })?;
        let screen = harness.wait_for_text("Approved exact action")?;
        assertions::assert_visible(&screen, &[ACTION]);
        Ok(())
    });
}

#[test]
fn rejection_prevents_execution() {
    let (script, _digest) = awaiting_write();
    let mut harness = attached(script, HarnessOptions::default());
    with_artifacts("approval-reject", &mut harness, |harness| {
        resume_to_approval(harness)?;

        harness.key(Key::Char('r'))?;
        harness.wait_for_request("POST", &format!("/v1/sessions/{SESSION}/deny"))?;
        harness.wait_for_durable("the rejection to be recorded", |script| {
            script.sessions.get(SESSION).is_some_and(|session| {
                session
                    .events
                    .iter()
                    .any(|event| event["event"] == "approval_rejected")
            })
        })?;

        // Nothing executed. This is the assertion that matters.
        let events = harness.session_events(SESSION);
        assert!(
            !events
                .iter()
                .any(|event| event["event"] == "execution_started"),
            "a rejection produced an execution: {events:#?}"
        );
        assert!(
            !harness.daemon().saw("POST", "/approve"),
            "rejecting must never call the approval endpoint:\n{}",
            harness.daemon().request_log()
        );
        let screen = harness.wait_for_text("nothing was executed")?;
        assertions::assert_visible(&screen, &[ACTION]);
        Ok(())
    });
}

#[test]
fn digest_mismatch_fails_visibly() {
    let (script, digest) = awaiting_write();
    let mut harness = attached(script, HarnessOptions::default());
    with_artifacts("approval-digest-mismatch", &mut harness, |harness| {
        resume_to_approval(harness)?;

        // Replace the pending action underneath the open surface: the same action
        // id now carries different content, so the digest no longer matches what
        // the user is looking at.
        harness.daemon().update(|script| {
            if let Some(session) = script.sessions.get_mut(SESSION) {
                let (replacement, _) = fake_provider::write_awaiting_approval(
                    ACTION,
                    "src/lib.rs",
                    "pub fn something_entirely_different() {}\n",
                    "/repo",
                    "writes a file the plan did not declare",
                );
                session.events.extend(replacement);
            }
        });

        // The surface notices the substitution and blocks itself before the key
        // is even pressed. Whichever guard fires — the surface refusing, or the
        // submit-time re-derivation — the invariants are the same.
        let screen = harness.wait_for_text("Approval blocked")?;
        assertions::assert_readable(&screen, "no longer the action");
        assertions::assert_absent(&screen, &["A Approve exact action"]);

        harness.key(Key::Char('a'))?;

        assert!(
            !harness
                .daemon()
                .saw("POST", &format!("/v1/sessions/{SESSION}/approve")),
            "a changed action must not be approved:\n{}",
            harness.daemon().request_log()
        );
        harness.wait_for_durable("the boundary to remain pending", |script| {
            script
                .sessions
                .get(SESSION)
                .is_some_and(|session| session.awaiting_action.is_some())
        })?;
        let events = harness.session_events(SESSION);
        assert!(
            !events
                .iter()
                .any(|event| event["event"] == "approval_recorded"),
            "a mismatched digest produced an authorization: {events:#?}"
        );
        // The original digest is no longer offered as approvable.
        let screen = harness.screen();
        assert!(
            screen.contains("Approval blocked"),
            "the surface must stay labelled as blocked (digest {digest}):\n{}",
            screen.text()
        );
        Ok(())
    });
}

#[test]
fn natural_language_never_approves() {
    let (script, _digest) = awaiting_write();
    let mut harness = attached(script, HarnessOptions::default());
    with_artifacts("approval-natural-language", &mut harness, |harness| {
        resume_to_approval(harness)?;
        // Leave the decision pending and return to the composer: prose is typed
        // there, and it must still never authorize the pending action.
        harness.key(Key::Escape)?;
        harness.wait_for_text("still pending")?;

        // Prose that looks like consent must be treated as task text.
        for phrase in [
            "yes please go ahead and do it",
            "sounds good, approved",
            "sure",
        ] {
            harness.type_text(phrase)?;
            harness.key(Key::Ctrl('g'))?;
            harness.wait_for_durable("no approval to be submitted", |script| {
                script
                    .sessions
                    .get(SESSION)
                    .is_some_and(|session| session.awaiting_action.is_some())
            })?;
            assert!(
                !harness
                    .daemon()
                    .saw("POST", &format!("/v1/sessions/{SESSION}/approve")),
                "the phrase {phrase:?} reached the approval endpoint:\n{}",
                harness.daemon().request_log()
            );
        }
        let events = harness.session_events(SESSION);
        assert!(
            !events
                .iter()
                .any(|event| event["event"] == "approval_recorded"),
            "prose produced an authorization: {events:#?}"
        );
        Ok(())
    });
}

#[test]
fn a_bare_approve_with_nothing_pending_authorizes_nothing() {
    let mut sessions = BTreeMap::new();
    sessions.insert(
        SESSION.to_owned(),
        ScriptedSession::new("active")
            .with_events(fake_provider::startup("Nothing pending", "/repo")),
    );
    let script = DaemonScript {
        providers: vec![json!({"name": "local", "provider_type": "ollama", "local": true})],
        models: configured_models(),
        sessions,
        ..DaemonScript::default()
    };
    let mut harness = attached(script, HarnessOptions::default());
    with_artifacts("approval-nothing-pending", &mut harness, |harness| {
        resume(harness)?;
        harness.wait_for_text("PurrCode")?;
        harness.run_command("/approve")?;

        let screen = harness.wait_for_wrapped("No action is awaiting approval")?;
        assertions::assert_readable(&screen, "Nothing was authorized");
        assertions::assert_absent(&screen, &["A Approve exact action"]);
        assert!(
            !harness
                .daemon()
                .saw("POST", &format!("/v1/sessions/{SESSION}/approve")),
            "a bare approve must not call the daemon:\n{}",
            harness.daemon().request_log()
        );
        Ok(())
    });
}

#[test]
fn restart_preserves_pending_boundary() {
    let (script, digest) = awaiting_write();
    let mut harness = attached(script, HarnessOptions::default());
    with_artifacts("approval-restart", &mut harness, |harness| {
        resume_to_approval(harness)?;
        Ok(())
    });

    // Restart the client. Durable state survives; client state does not.
    let mut harness = harness.restart().expect("restart workbench");
    with_artifacts("approval-restart-after", &mut harness, |harness| {
        // The recovered session offers to restore the decision boundary, not to
        // replay the model.
        let screen = harness.wait_for_text("An existing session was found")?;
        assertions::assert_readable(&screen, "No task will run until you choose");
        harness.key(Key::Char('r'))?;
        let screen = harness.wait_for_text("Approval required")?;
        assert!(
            screen.contains(&digest),
            "the same exact action must be presented after a restart:\n{}",
            screen.text()
        );
        assertions::assert_visible(&screen, &[ACTION]);
        Ok(())
    });
}

#[test]
fn leaving_an_approval_pending_runs_nothing() {
    let (script, _digest) = awaiting_write();
    let mut harness = attached(script, HarnessOptions::default());
    with_artifacts("approval-leave-pending", &mut harness, |harness| {
        resume_to_approval(harness)?;

        harness.key(Key::Escape)?;
        let screen = harness.wait_for_wrapped("still pending")?;
        assertions::assert_readable(&screen, "Nothing has run");
        harness.wait_for_durable("the boundary to survive", |script| {
            script
                .sessions
                .get(SESSION)
                .is_some_and(|session| session.awaiting_action.is_some())
        })?;
        Ok(())
    });
}

#[test]
fn adding_an_instruction_leaves_the_action_pending() {
    let (script, _digest) = awaiting_write();
    let mut harness = attached(script, HarnessOptions::default());
    with_artifacts("approval-instruct", &mut harness, |harness| {
        resume_to_approval(harness)?;

        harness.key(Key::Char('i'))?;
        let screen = harness.wait_for_wrapped("The action stays pending")?;
        assertions::assert_absent(&screen, &["Approval required"]);
        assert!(
            !harness
                .daemon()
                .saw("POST", &format!("/v1/sessions/{SESSION}/approve")),
            "adding an instruction must not approve:\n{}",
            harness.daemon().request_log()
        );
        Ok(())
    });
}

#[test]
fn rejecting_a_resolved_action_changes_nothing() {
    let (script, _digest) = awaiting_write();
    let mut harness = attached(script, HarnessOptions::default());
    with_artifacts("approval-resolved", &mut harness, |harness| {
        resume_to_approval(harness)?;

        // The boundary resolves elsewhere while the surface is open.
        harness.daemon().update(|script| {
            if let Some(session) = script.sessions.get_mut(SESSION) {
                session.awaiting_action = None;
                session.status_code = "active".to_owned();
                session.events.push(json!({
                    "event": "approval_rejected",
                    "data": {"action_id": ACTION, "reason": "decided in another client"}
                }));
            }
        });

        harness.key(Key::Char('r'))?;
        let screen = harness.wait_for_wrapped("already decided")?;
        assertions::assert_absent(&screen, &["nothing was executed"]);
        let events = harness.session_events(SESSION);
        assert_eq!(
            events
                .iter()
                .filter(|event| event["event"] == "approval_rejected")
                .count(),
            1,
            "a second rejection was recorded: {events:#?}"
        );
        Ok(())
    });
}

#[test]
fn the_approval_surface_is_complete_at_sixty_columns() {
    let (script, digest) = awaiting_write();
    let mut harness = attached(script, HarnessOptions::size(60, 24));
    with_artifacts("approval-sixty-columns", &mut harness, |harness| {
        resume_to_approval(harness)?;
        let screen = harness.wait_for_text("Approval required")?;
        assertions::assert_no_overflow(&screen);
        assertions::assert_visible(&screen, &["src/lib.rs", "Digest"]);
        assertions::assert_readable(&screen, "A Approve exact action");
        assert!(
            screen.contains(&digest[..32]),
            "the digest must remain inspectable at 60 columns:\n{}",
            screen.text()
        );
        Ok(())
    });
}
