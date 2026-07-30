//! Diff review and validation outcomes.
//!
//! The point of these tests is that a technically successful run whose validation
//! did not pass is never presented as success, and that a missing diff is
//! reported as missing rather than as "no changes".

use std::collections::BTreeMap;

use purrcode_tui_e2e::fake_daemon::{DaemonScript, ScriptedSession};
use purrcode_tui_e2e::fake_provider;
use purrcode_tui_e2e::harness::with_artifacts;
use purrcode_tui_e2e::{assertions, Harness, HarnessOptions, Key};
use serde_json::{json, Value};

const SESSION: &str = "review-session";

fn completed_session(events: Vec<Value>, diff: Option<(String, String)>) -> DaemonScript {
    let mut sessions = BTreeMap::new();
    sessions.insert(
        SESSION.to_owned(),
        ScriptedSession::new("completed").with_events(events),
    );
    DaemonScript {
        providers: vec![json!({"name": "local", "provider_type": "ollama", "local": true})],
        models: vec![json!({"id": "local/fake:1b", "default": true, "local": true})],
        sessions,
        diff,
        ..DaemonScript::default()
    }
}

fn attached(script: DaemonScript, options: HarnessOptions) -> Harness {
    let workspace = purrcode_tui_e2e::fixtures::Workspace::new().expect("workspace");
    workspace
        .seed_ui_state(Some(SESSION), "")
        .expect("seed session");
    Harness::start_in(workspace, script, options).expect("start workbench")
}

/// A completed session with edits and passing validation.
fn passing() -> DaemonScript {
    let mut events = fake_provider::startup("Add a helper", "/repo");
    events.push(fake_provider::plan(&["Edit src/lib.rs"]));
    events.extend(fake_provider::allowed_read("read-1", "src/lib.rs", "/repo"));
    events.push(fake_provider::validation("passed", "12 tests passed"));
    events.push(fake_provider::session_completed());
    completed_session(events, Some(fake_provider::diff_fixture()))
}

/// Open review from the recovered session.
fn open_review(harness: &mut Harness) -> anyhow::Result<()> {
    harness.wait_for_text("An existing session was found")?;
    // A completed session cannot be resumed; open it read-only.
    harness.key(Key::Char('o'))?;
    harness.wait_for_text("PurrCode")?;
    harness.run_command("/diff")?;
    harness.wait_for_text("Review")?;
    Ok(())
}

#[test]
fn review_shows_daemon_backed_changed_files() {
    let mut harness = attached(passing(), HarnessOptions::default());
    with_artifacts("review-files", &mut harness, |harness| {
        open_review(harness)?;
        harness.wait_for_request("GET", &format!("/v1/sessions/{SESSION}/diff"))?;
        let screen = harness.wait_for_all(&["src/lib.rs", "src/removed.rs"])?;
        assertions::assert_visible(&screen, &["modified", "deleted", "+fn added() {}"]);
        assertions::assert_no_overflow(&screen);
        // Navigation keys are advertised, not memorized.
        assertions::assert_visible(&screen, &["J Next file", "Esc Close"]);
        Ok(())
    });
}

#[test]
fn file_and_hunk_navigation_works() {
    let mut harness = attached(passing(), HarnessOptions::default());
    with_artifacts("review-navigation", &mut harness, |harness| {
        open_review(harness)?;
        harness.wait_for_text("src/lib.rs")?;
        harness.wait_for_text("hunk 1 of 1")?;

        harness.key(Key::Char('j'))?;
        let screen = harness.wait_for_text("-fn gone() {}")?;
        assertions::assert_visible(&screen, &["src/removed.rs"]);

        harness.key(Key::Char('k'))?;
        harness.wait_for_text("+fn added() {}")?;
        Ok(())
    });
}

#[test]
fn large_diff_loads_incrementally() {
    let mut patch = String::from("diff --git a/big.rs b/big.rs\n@@ -1,2000 +1,2000 @@\n");
    for index in 0..2_000 {
        patch.push_str(&format!("+line {index}\n"));
    }
    let mut events = fake_provider::startup("Rewrite a large file", "/repo");
    events.push(fake_provider::validation("passed", "ok"));
    events.push(fake_provider::session_completed());
    let script = completed_session(events, Some((patch, "M  big.rs\n".to_owned())));

    let mut harness = attached(script, HarnessOptions::default());
    with_artifacts("review-large-diff", &mut harness, |harness| {
        open_review(harness)?;
        let screen = harness.wait_for_text("big.rs")?;
        // A bounded hunk says what it withheld rather than pretending it is whole.
        assertions::assert_readable(&screen, "line(s) in this hunk are not shown");
        assertions::assert_no_overflow(&screen);
        Ok(())
    });
}

#[test]
fn failed_validation_is_not_success() {
    let mut events = fake_provider::startup("Fix the bug", "/repo");
    events.push(fake_provider::validation("failed", "3 of 12 checks failed"));
    events.push(fake_provider::session_completed());
    let script = completed_session(events, Some(fake_provider::diff_fixture()));

    let mut harness = attached(script, HarnessOptions::default());
    with_artifacts("review-validation-failed", &mut harness, |harness| {
        open_review(harness)?;
        let screen = harness.wait_for_text("Validation failed")?;
        assertions::assert_visible(&screen, &["3 of 12 checks failed"]);
        assertions::assert_absent(&screen, &["Validation passed"]);
        // A failure must offer a next step.
        assertions::assert_readable(&screen, "correct or roll back");
        Ok(())
    });
}

#[test]
fn unavailable_validation_stays_visible() {
    let mut events = fake_provider::startup("Change something", "/repo");
    events.push(fake_provider::validation(
        "unavailable",
        "no validator was configured for this repository",
    ));
    events.push(fake_provider::session_completed());
    let script = completed_session(events, Some(fake_provider::diff_fixture()));

    let mut harness = attached(script, HarnessOptions::default());
    with_artifacts("review-validation-unavailable", &mut harness, |harness| {
        open_review(harness)?;
        let screen = harness.wait_for_text("Validation unavailable")?;
        assertions::assert_absent(&screen, &["Validation passed"]);
        assertions::assert_readable(&screen, "verify manually before applying");
        Ok(())
    });
}

#[test]
fn validation_timeout_is_distinct_from_failure() {
    let mut events = fake_provider::startup("Change something", "/repo");
    events.push(fake_provider::validation(
        "timed_out",
        "the test command exceeded its time budget",
    ));
    events.push(fake_provider::session_completed());
    let script = completed_session(events, Some(fake_provider::diff_fixture()));

    let mut harness = attached(script, HarnessOptions::default());
    with_artifacts("review-validation-timeout", &mut harness, |harness| {
        open_review(harness)?;
        let screen = harness.wait_for_text("Validation timed out")?;
        assertions::assert_absent(&screen, &["Validation failed", "Validation passed"]);
        assertions::assert_readable(&screen, "did not finish");
        Ok(())
    });
}

#[test]
fn a_binary_effect_says_there_is_nothing_to_read() {
    let patch = "diff --git a/logo.png b/logo.png\nBinary files a/logo.png and b/logo.png differ\n"
        .to_owned();
    let mut events = fake_provider::startup("Replace the logo", "/repo");
    events.push(fake_provider::validation("passed", "ok"));
    events.push(fake_provider::session_completed());
    let script = completed_session(events, Some((patch, "M  logo.png\n".to_owned())));

    let mut harness = attached(script, HarnessOptions::default());
    with_artifacts("review-binary", &mut harness, |harness| {
        open_review(harness)?;
        let screen = harness.wait_for_text("logo.png")?;
        assertions::assert_readable(&screen, "no reviewable text difference");
        Ok(())
    });
}

#[test]
fn an_unavailable_diff_is_reported_as_unavailable() {
    let mut events = fake_provider::startup("Answer a question", "/repo");
    events.push(fake_provider::assistant_message("The answer is 42."));
    events.push(fake_provider::session_completed());
    // No worktree: the daemon reports a conflict, not an empty diff.
    let script = completed_session(events, None);

    let mut harness = attached(script, HarnessOptions::default());
    with_artifacts("review-unavailable", &mut harness, |harness| {
        open_review(harness)?;
        let screen = harness.wait_for_wrapped("The session diff is unavailable")?;
        assertions::assert_readable(&screen, "missing record, not an empty one");
        assertions::assert_absent(&screen, &["no recorded changes", "Validation passed"]);
        Ok(())
    });
}

#[test]
fn rollback_discards_agent_owned_work() {
    let mut harness = attached(passing(), HarnessOptions::default());
    with_artifacts("review-rollback", &mut harness, |harness| {
        open_review(harness)?;
        harness.wait_for_text("src/lib.rs")?;
        harness.key(Key::Escape)?;
        harness.wait_for_text("PurrCode")?;
        harness.run_command("/rollback")?;
        harness.wait_for_request("POST", &format!("/v1/sessions/{SESSION}/rollback"))?;
        let screen = harness.wait_for_wrapped("Agent-owned changes were discarded")?;
        assertions::assert_readable(&screen, "your working tree is untouched");
        Ok(())
    });
}

#[test]
fn rollback_without_agent_owned_work_is_refused() {
    let mut events = fake_provider::startup("Answer a question", "/repo");
    events.push(fake_provider::session_completed());
    let script = completed_session(events, None);

    let mut harness = attached(script, HarnessOptions::default());
    with_artifacts("review-rollback-refused", &mut harness, |harness| {
        harness.wait_for_text("An existing session was found")?;
        harness.key(Key::Char('o'))?;
        harness.wait_for_text("PurrCode")?;
        harness.run_command("/rollback")?;
        harness.wait_for_request("POST", &format!("/v1/sessions/{SESSION}/rollback"))?;
        // A refusal, not a silent no-op.
        let screen = harness.wait_for_wrapped("no agent-owned worktree")?;
        assertions::assert_absent(&screen, &["Agent-owned changes were discarded"]);
        Ok(())
    });
}

#[test]
fn review_is_usable_at_sixty_columns_one_surface_at_a_time() {
    let mut harness = attached(passing(), HarnessOptions::size(60, 24));
    with_artifacts("review-sixty-columns", &mut harness, |harness| {
        open_review(harness)?;
        let screen = harness.wait_for_text("src/lib.rs")?;
        assertions::assert_no_overflow(&screen);
        // At 60 columns the file list and the diff must not share a row.
        assertions::assert_absent(&screen, &["+fn added() {}"]);
        assertions::assert_visible(&screen, &["F Files/diff"]);

        harness.key(Key::Char('f'))?;
        let screen = harness.wait_for_text("+fn added() {}")?;
        assertions::assert_no_overflow(&screen);
        Ok(())
    });
}

#[test]
fn validation_evidence_can_be_expanded() {
    let mut events = fake_provider::startup("Fix the bug", "/repo");
    events.push(fake_provider::validation(
        "failed",
        "cargo test reported 3 failures across 2 files; see the trace for detail",
    ));
    events.push(fake_provider::session_completed());
    let script = completed_session(events, Some(fake_provider::diff_fixture()));

    let mut harness = attached(script, HarnessOptions::default());
    with_artifacts("review-validation-expand", &mut harness, |harness| {
        open_review(harness)?;
        harness.wait_for_text("Validation failed")?;
        harness.key(Key::Char('v'))?;
        let screen = harness.wait_for_wrapped("3 failures across 2 files")?;
        assertions::assert_no_overflow(&screen);
        Ok(())
    });
}
