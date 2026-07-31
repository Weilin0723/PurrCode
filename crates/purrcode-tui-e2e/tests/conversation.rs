//! Task submission, streaming, modes and cancellation.

use std::collections::BTreeMap;

use purrcode_tui_e2e::fake_daemon::{DaemonScript, ScriptedSession, StreamFrame};
use purrcode_tui_e2e::fake_provider;
use purrcode_tui_e2e::harness::with_artifacts;
use purrcode_tui_e2e::{assertions, Harness, HarnessOptions, Key};
use serde_json::json;

fn configured() -> DaemonScript {
    DaemonScript {
        providers: vec![json!({"name": "local", "provider_type": "ollama", "local": true})],
        models: vec![json!({"id": "local/fake:1b", "default": true, "local": true})],
        ..DaemonScript::default()
    }
}

/// A daemon that answers the next task with `frames`.
fn answering(frames: Vec<StreamFrame>) -> DaemonScript {
    DaemonScript {
        stream_frames: frames,
        ..configured()
    }
}

/// Submit a task and wait until the workbench has created the session.
fn submit(harness: &mut Harness, task: &str) -> anyhow::Result<()> {
    harness.wait_for_text("Ready for a task")?;
    harness.type_text(task)?;
    harness.key(Key::Ctrl('g'))?;
    harness.wait_for_request("POST", "/v1/sessions")?;
    Ok(())
}

#[test]
fn first_task_progress_is_understandable() {
    let script = answering(vec![
        StreamFrame::Phase {
            phase: "receiving".into(),
            role: "coder".into(),
        },
        StreamFrame::DurableAudit(json!({
            "event": "context_indexed",
            "data": {"files": 7, "symbols": 40, "sensitive_files": 0}
        })),
        StreamFrame::DurableAudit(fake_provider::plan(&["Read the code", "Answer"])),
        StreamFrame::ContentDelta("The repository builds with cargo.".into()),
        StreamFrame::DurableAudit(fake_provider::session_completed()),
    ]);
    let mut harness = Harness::start(script).expect("start workbench");
    with_artifacts("conversation-first-task", &mut harness, |harness| {
        submit(harness, "What does this repository build?")?;

        // The task appears as a conversation block, not as an event dump.
        let screen = harness.wait_for_text("What does this repository build?")?;
        assertions::assert_visible(&screen, &["you"]);
        assertions::assert_absent(&screen, &["You:", "PurrCode:", "context_indexed"]);

        // Progress is legible as stages.
        let screen = harness.wait_for_all(&["Context prepared", "Plan created"])?;
        assertions::assert_no_overflow(&screen);

        // The answer arrives as an assistant block.
        let screen = harness.wait_for_text("The repository builds with cargo.")?;
        assertions::assert_visible(&screen, &["purrcode"]);
        harness.wait_for_text("Completed")?;
        Ok(())
    });
}

#[test]
fn long_streaming_answer_stays_readable() {
    let long = (0..40)
        .map(|index| format!("Sentence {index} is long enough that it has to wrap on a terminal. "))
        .collect::<String>();
    let script = answering(vec![
        StreamFrame::ContentDelta(long),
        StreamFrame::DurableAudit(fake_provider::session_completed()),
    ]);
    let mut harness = Harness::start(script).expect("start workbench");
    with_artifacts("conversation-long-answer", &mut harness, |harness| {
        submit(harness, "Explain the architecture")?;
        let screen = harness.wait_for_text("Sentence 39")?;
        // Wrapped, never drawn past the edge, and the newest text is visible.
        assertions::assert_no_overflow(&screen);
        Ok(())
    });
}

#[test]
fn multiline_and_paste_do_not_send_early() {
    let mut harness = Harness::start(configured()).expect("start workbench");
    with_artifacts("conversation-multiline", &mut harness, |harness| {
        harness.wait_for_text("Ready for a task")?;

        harness.type_text("first line")?;
        harness.key(Key::Enter)?;
        harness.type_text("second line")?;
        harness.wait_for_text("second line")?;
        assert!(
            !harness.daemon().saw("POST", "/v1/sessions"),
            "Enter must add a line, not submit:\n{}",
            harness.daemon().request_log()
        );

        // A bracketed paste containing newlines must also not submit.
        harness.key(Key::Paste("pasted one\npasted two\npasted three".into()))?;
        let screen = harness.wait_for_text("pasted three")?;
        assertions::assert_visible(&screen, &["pasted"]);
        assert!(
            !harness.daemon().saw("POST", "/v1/sessions"),
            "a multi-line paste must not submit:\n{}",
            harness.daemon().request_log()
        );

        // Only the explicit send key submits.
        harness.key(Key::Ctrl('g'))?;
        harness.wait_for_request("POST", "/v1/sessions")?;
        Ok(())
    });
}

#[test]
fn ask_mode_proposes_no_changes() {
    let script = answering(vec![
        StreamFrame::ContentDelta("This repository is a Rust workspace.".into()),
        StreamFrame::DurableAudit(fake_provider::session_completed()),
    ]);
    let mut harness = Harness::start(script).expect("start workbench");
    with_artifacts("conversation-ask-mode", &mut harness, |harness| {
        harness.wait_for_text("Ready for a task")?;
        harness.run_command("/ask")?;
        let screen = harness.wait_for_wrapped("without proposing repository changes")?;
        assertions::assert_visible(&screen, &["Ask"]);

        submit(harness, "What is this?")?;
        harness.wait_for_text("This repository is a Rust workspace.")?;
        let screen = harness.screen();
        assertions::assert_absent(&screen, &["Awaiting your approval", "Approval required"]);
        Ok(())
    });
}

#[test]
fn plan_mode_modifies_nothing() {
    let script = answering(vec![
        StreamFrame::DurableAudit(fake_provider::plan(&["Step one", "Step two"])),
        StreamFrame::DurableAudit(fake_provider::session_completed()),
    ]);
    let mut harness = Harness::start(script).expect("start workbench");
    with_artifacts("conversation-plan-mode", &mut harness, |harness| {
        harness.wait_for_text("Ready for a task")?;
        harness.run_command("/plan")?;
        harness.wait_for_text("Plan mode")?;
        submit(harness, "Plan the refactor")?;
        harness.wait_for_text("Plan created")?;
        let screen = harness.screen();
        assertions::assert_absent(&screen, &["Approval required", "Editing"]);
        Ok(())
    });
}

#[test]
fn a_plan_under_review_is_revised_by_replying_to_it() {
    // PRD §11: in Plan mode the plan is the deliverable, so review has to be
    // able to change it. The workbench only reused a session whose status was
    // `active`, so typing into a plan-only run that had paused started a brand
    // new session — the plan was discarded and the whole task had to be
    // described again to alter one step.
    let script = answering(vec![
        StreamFrame::DurableAudit(fake_provider::plan(&["Add the parser", "Add the tests"])),
        StreamFrame::DurableAudit(fake_provider::plan_ready_for_review(false)),
    ]);
    let mut harness = Harness::start(script).expect("start workbench");
    with_artifacts("conversation-plan-revision", &mut harness, |harness| {
        harness.wait_for_text("Ready for a task")?;
        harness.run_command("/plan")?;
        harness.wait_for_text("Plan mode")?;
        submit(harness, "Plan the refactor")?;

        // The pause invites a reply rather than reporting a fault.
        let screen = harness.wait_for_text("Plan ready for review")?;
        assertions::assert_absent(&screen, &["Outcome review required"]);
        assertions::assert_no_overflow(&screen);

        harness.daemon().update(|script| {
            script.stream_frames = vec![
                StreamFrame::DurableAudit(fake_provider::plan_revised(
                    2,
                    "add a migration step",
                    &["Add the parser", "Add the migration", "Add the tests"],
                )),
                StreamFrame::DurableAudit(fake_provider::plan_ready_for_review(true)),
            ];
        });
        harness.type_text("Add a migration step before the tests")?;
        harness.key(Key::Ctrl('g'))?;

        // The reply reaches the session as a message. That is what routes it to
        // a revision; a second POST /v1/sessions would mean the plan was thrown
        // away and the work re-described from scratch.
        harness.wait_for_request("POST", "/messages")?;
        let starts = harness
            .daemon()
            .requests()
            .into_iter()
            .filter(|request| request.method == "POST" && request.path == "/v1/sessions")
            .count();
        assert_eq!(starts, 1, "feedback on a plan must not start a new session");

        // The new plan is distinguishable from the one that was complained
        // about, and it pauses for review again rather than starting work.
        let screen = harness.wait_for_text("Plan revised (revision 2)")?;
        assertions::assert_visible(&screen, &["Plan ready for review"]);
        assertions::assert_absent(&screen, &["Approval required", "Editing"]);
        Ok(())
    });
}

#[test]
fn build_mode_requires_approval() {
    let (approval_events, _digest) = fake_provider::write_awaiting_approval(
        "build-action",
        "src/lib.rs",
        "pub fn built() {}\n",
        "/repo",
        "writes a file",
    );
    let frames = approval_events
        .into_iter()
        .map(StreamFrame::DurableAudit)
        .collect();
    let mut harness = Harness::start(answering(frames)).expect("start workbench");
    with_artifacts("conversation-build-mode", &mut harness, |harness| {
        harness.wait_for_text("Ready for a task")?;
        harness.run_command("/build")?;
        harness.wait_for_text("Build mode")?;
        submit(harness, "Add a function")?;
        let screen =
            harness.wait_for_all(&["Approval required", "src/lib.rs", "A Approve exact action"])?;
        assertions::assert_no_overflow(&screen);
        Ok(())
    });
}

#[test]
fn review_mode_reports_recorded_work() {
    let mut harness = Harness::start(configured()).expect("start workbench");
    with_artifacts("conversation-review-mode", &mut harness, |harness| {
        harness.wait_for_text("Ready for a task")?;
        harness.run_command("/review")?;
        let screen = harness.wait_for_text("Review mode")?;
        assertions::assert_visible(&screen, &["Review"]);
        Ok(())
    });
}

#[test]
fn cancel_midstream_preserves_partial_output() {
    let script = DaemonScript {
        // Held open: the response is still in flight when the user cancels.
        stream_stays_open: true,
        stream_frames: vec![
            StreamFrame::Phase {
                phase: "receiving".into(),
                role: "coder".into(),
            },
            StreamFrame::ContentDelta("This is the beginning of a long answer".into()),
        ],
        ..configured()
    };
    let mut harness = Harness::start(script).expect("start workbench");
    with_artifacts("conversation-cancel", &mut harness, |harness| {
        submit(harness, "Write a long answer")?;
        harness.wait_for_text("This is the beginning of a long answer")?;

        harness.run_command("/cancel")?;
        harness.wait_for_request("POST", "/cancel")?;

        // The partial answer survives the cancellation. Losing it would make the
        // user redo work the model already did.
        let screen = harness.wait_for_wrapped("This is the beginning of a long answer")?;
        assertions::assert_no_overflow(&screen);
        harness.wait_for_durable("the session to be cancelled", |script| {
            script
                .sessions
                .values()
                .any(|session| session.status_code == "cancelled")
        })?;
        Ok(())
    });
}

#[test]
fn provider_failure_explains_retry_path() {
    let script = DaemonScript {
        stream_fails: true,
        ..configured()
    };
    let mut harness = Harness::start(script).expect("start workbench");
    with_artifacts("conversation-provider-failure", &mut harness, |harness| {
        submit(harness, "Do something")?;
        // The failure is explained rather than leaving a silent dead stream.
        let screen = harness.wait_for_wrapped("Live stream interrupted")?;
        assertions::assert_readable(&screen, "partial output was preserved");
        assertions::assert_no_overflow(&screen);
        Ok(())
    });
}

#[test]
fn compaction_reports_what_was_summarized() {
    let mut sessions = BTreeMap::new();
    sessions.insert(
        "compact-session".to_owned(),
        ScriptedSession::new("active")
            .with_events(fake_provider::startup("A long conversation", "/repo")),
    );
    let script = DaemonScript {
        sessions,
        ..configured()
    };
    let workspace = purrcode_tui_e2e::fixtures::Workspace::new().expect("workspace");
    workspace
        .seed_ui_state(Some("compact-session"), "")
        .expect("seed");
    let mut harness =
        Harness::start_in(workspace, script, HarnessOptions::default()).expect("start workbench");
    with_artifacts("conversation-compact", &mut harness, |harness| {
        harness.wait_for_text("An existing session was found")?;
        harness.key(Key::Char('r'))?;
        harness.wait_for_text("PurrCode")?;
        harness.run_command("/compact")?;
        harness.wait_for_request("POST", "/compact")?;
        harness.wait_for_text("Context compacted")?;
        Ok(())
    });
}

#[test]
fn web_fetch_requires_approval() {
    let mut sessions = BTreeMap::new();
    sessions.insert(
        "research-session".to_owned(),
        ScriptedSession::new("active")
            .with_events(fake_provider::startup("Look something up", "/repo")),
    );
    let script = DaemonScript {
        sessions,
        ..configured()
    };
    let workspace = purrcode_tui_e2e::fixtures::Workspace::new().expect("workspace");
    workspace
        .seed_ui_state(Some("research-session"), "")
        .expect("seed");
    let mut harness =
        Harness::start_in(workspace, script, HarnessOptions::default()).expect("start workbench");
    with_artifacts("conversation-research", &mut harness, |harness| {
        harness.wait_for_text("An existing session was found")?;
        harness.key(Key::Char('r'))?;
        harness.wait_for_text("PurrCode")?;
        harness.run_command("/research https://example.invalid/page")?;

        // The fetch is proposed, never performed on the spot.
        harness.wait_for_request("POST", "/v1/research/fetch")?;
        let screen = harness.wait_for_text("PurrCode")?;
        assertions::assert_absent(&screen, &["fetched", "page content"]);
        Ok(())
    });
}

#[test]
fn rejected_web_fetch_never_reaches_the_network() {
    let mut sessions = BTreeMap::new();
    sessions.insert(
        "research-session".to_owned(),
        ScriptedSession::new("active")
            .with_events(fake_provider::startup("Look something up", "/repo")),
    );
    let script = DaemonScript {
        sessions,
        ..configured()
    };
    let workspace = purrcode_tui_e2e::fixtures::Workspace::new().expect("workspace");
    workspace
        .seed_ui_state(Some("research-session"), "")
        .expect("seed");
    let mut harness =
        Harness::start_in(workspace, script, HarnessOptions::default()).expect("start workbench");
    with_artifacts("conversation-research-denied", &mut harness, |harness| {
        harness.wait_for_text("An existing session was found")?;
        harness.key(Key::Char('r'))?;
        harness.wait_for_text("PurrCode")?;

        // Never approving the fetch means the approve endpoint is never called.
        harness.run_command("/research https://example.invalid/page")?;
        harness.wait_for_request("POST", "/v1/research/fetch")?;
        assert!(
            !harness.daemon().saw("POST", "research-approve"),
            "an unapproved fetch must never be confirmed:\n{}",
            harness.daemon().request_log()
        );
        Ok(())
    });
}

#[test]
fn a_large_draft_stays_editable() {
    let mut harness = Harness::start(configured()).expect("start workbench");
    with_artifacts("conversation-large-draft", &mut harness, |harness| {
        harness.wait_for_text("Ready for a task")?;
        // 64 KB of pasted text, delivered as a real bracketed paste.
        harness.key(Key::Paste("abcdefgh".repeat(8 * 1024)))?;
        harness.type_text("TAIL")?;
        let screen = harness.wait_for_text("TAIL")?;
        assertions::assert_no_overflow(&screen);
        // Counts disclose themselves only once the draft is genuinely large.
        assertions::assert_readable(&screen, "chars");
        Ok(())
    });
}
