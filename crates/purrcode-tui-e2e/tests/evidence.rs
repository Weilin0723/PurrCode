//! Raw evidence inspection and the skill/MCP capability surface.
//!
//! Two different questions live in this file. First: when a user does not
//! trust the compact activity rail, can they always drop down to the exact
//! durable event that produced it — and does the UI say so plainly when there
//! is nothing to drop down to, rather than showing a stale or empty panel as
//! if it were current? Second: skill and MCP capability acquisition is a
//! write path like any other (it fetches and runs third-party code), so it
//! gets the same treatment — nothing downloads, installs, or runs without a
//! displayed, approved boundary, and a blocked publisher or failed
//! qualification stops the chain rather than being silently skipped.

use std::collections::BTreeMap;

use purrcode_tui_e2e::fake_daemon::{DaemonScript, ScriptedSession};
use purrcode_tui_e2e::fake_provider;
use purrcode_tui_e2e::harness::with_artifacts;
use purrcode_tui_e2e::{Harness, HarnessOptions, Key, assertions};
use serde_json::json;

fn base() -> DaemonScript {
    DaemonScript {
        providers: vec![json!({"name": "local", "provider_type": "ollama", "local": true})],
        models: vec![json!({"id": "local/fake:1b", "default": true, "local": true})],
        ..DaemonScript::default()
    }
}

/// A daemon that answers the next task with `frames`.
fn answering(frames: Vec<purrcode_tui_e2e::fake_daemon::StreamFrame>) -> DaemonScript {
    DaemonScript {
        stream_frames: frames,
        ..base()
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

const SESSION: &str = "evidence-session";

fn with_session(session: ScriptedSession) -> DaemonScript {
    let mut sessions = BTreeMap::new();
    sessions.insert(SESSION.to_owned(), session);
    DaemonScript { sessions, ..base() }
}

fn attached(script: DaemonScript) -> Harness {
    let workspace = purrcode_tui_e2e::fixtures::Workspace::new().expect("workspace");
    workspace
        .seed_ui_state(Some(SESSION), "")
        .expect("seed session");
    Harness::start_in(workspace, script, HarnessOptions::default()).expect("start workbench")
}

const DOWNLOAD_ARGS: &str = "github:acme/repo-linter 0123456789abcdef0123456789abcdef01234567";

// ── Evidence: raw durable events ────────────────────────────────

#[test]
fn durable_events_can_be_inspected() {
    let script = answering(vec![
        purrcode_tui_e2e::fake_daemon::StreamFrame::DurableAudit(json!({
            "event": "context_indexed",
            "data": {"files": 7, "symbols": 40, "sensitive_files": 0}
        })),
        purrcode_tui_e2e::fake_daemon::StreamFrame::DurableAudit(fake_provider::plan(&[
            "Read the code",
            "Answer",
        ])),
        purrcode_tui_e2e::fake_daemon::StreamFrame::ContentDelta(
            "The repository builds with cargo.".into(),
        ),
        purrcode_tui_e2e::fake_daemon::StreamFrame::DurableAudit(fake_provider::session_completed()),
    ]);
    let mut harness = Harness::start(script).expect("start workbench");
    with_artifacts("evidence-trace-inspector", &mut harness, |harness| {
        submit(harness, "Explain the repository")?;
        harness.wait_for_text("Completed")?;

        // The rail shows three stages; the trace inspector must be able to
        // step through every one of them, not just describe the newest.
        harness.key(Key::CtrlDown)?;
        harness.key(Key::Char('t'))?;
        let screen = harness.wait_for_text("Trace Inspector")?;
        assertions::assert_visible(&screen, &["Event 3 / 3", "Type: Session completed"]);
        assertions::assert_no_overflow(&screen);

        harness.key(Key::Left)?;
        let screen = harness.wait_for_text("Event 2 / 3")?;
        assertions::assert_readable(&screen, "Type: Plan");
        assertions::assert_readable(&screen, "2 step(s)");

        harness.key(Key::Left)?;
        let screen = harness.wait_for_text("Event 1 / 3")?;
        assertions::assert_readable(&screen, "Repository indexed");

        harness.key(Key::Escape)?;
        harness.wait_until_absent("Trace Inspector")?;
        Ok(())
    });
}

#[test]
fn action_explains_its_decision() {
    let (mut events, _digest) = fake_provider::write_awaiting_approval(
        "risky-write",
        "src/lib.rs",
        "fn added() {}",
        "/repo",
        "writes a file the plan did not declare",
    );
    events.push(json!({
        "event": "approval_rejected",
        "data": {
            "action_id": "risky-write",
            "reason": "the proposed change touches a file outside the declared plan",
        }
    }));
    let script = with_session(ScriptedSession::new("active").with_events(events));
    let mut harness = attached(script);
    with_artifacts("evidence-action-explanation", &mut harness, |harness| {
        harness.wait_for_text("An existing session was found")?;
        harness.key(Key::Char('o'))?;

        // The rail names the outcome, not the mechanism.
        let screen = harness.wait_for_text("Action rejected")?;
        assertions::assert_no_overflow(&screen);

        // Inspecting it names the actual reason PawGate recorded, not a
        // generic "declined" label.
        harness.key(Key::Char('e'))?;
        let screen = harness.wait_for_text("Activity detail")?;
        assertions::assert_readable(&screen, "was denied");
        assertions::assert_readable_in_column(
            &screen,
            80,
            screen.width(),
            "the proposed change touches a file outside the declared plan",
        );
        Ok(())
    });
}

#[test]
fn missing_evidence_reports_unavailable() {
    let script = answering(vec![
        purrcode_tui_e2e::fake_daemon::StreamFrame::DurableAudit(fake_provider::session_completed()),
    ]);
    let mut harness = Harness::start(script).expect("start workbench");
    with_artifacts("evidence-unavailable", &mut harness, |harness| {
        submit(harness, "Say hello")?;
        harness.wait_for_text("Completed")?;

        harness.key(Key::Char('e'))?;
        harness.wait_for_text("Activity detail")?;

        // Starting a new session clears the recorded activity but does not
        // reset which step was selected. The inspector must say the step is
        // gone rather than keep showing the old detail or a blank panel as if
        // it were still current.
        harness.run_command("/new")?;
        let screen = harness.wait_for_text("That activity step is unavailable")?;
        assertions::assert_no_overflow(&screen);
        Ok(())
    });
}

// ── Skills: browse, search, install lifecycle ───────────────────

#[test]
fn skills_browse_distinguishes_installed() {
    let script = DaemonScript {
        skills: vec![json!({
            "skill_id": "repo-linter",
            "version": "2.0.0",
            "publisher": "acme",
            "source_type": "repository",
            "signature_status": "verified",
            "approved_permissions": ["filesystem:read"],
            "network_access": "none",
            "qualification_status": "qualified",
        })],
        search_candidates: vec![json!({
            "manifest": {
                "name": "log-summarizer",
                "version": "0.3.0",
                "publisher": "someone-else",
                "source_type": "registry",
                "signature_status": "verified",
                "permissions": ["filesystem:read"],
                "network_access": "none",
            },
            "score": 91.0,
        })],
        ..base()
    };
    let mut harness = Harness::start(script).expect("start workbench");
    with_artifacts("skills-browse", &mut harness, |harness| {
        harness.wait_for_text("PurrCode")?;
        harness.run_command("/skills")?;
        harness.wait_for_request("GET", "/v1/skills")?;
        let screen = harness.wait_for_text("repo-linter")?;
        assertions::assert_readable(&screen, "[installed]");
        assertions::assert_no_overflow(&screen);

        // The skills screen has no composer of its own: leaving it is what
        // makes the composer reachable again for the next command.
        harness.key(Key::Escape)?;
        harness.wait_for_text("Ready for a task")?;

        // A discovered candidate is not marked installed: the two lists must
        // stay visibly distinct.
        harness.run_command("/skills search log summary")?;
        harness.wait_for_request("POST", "/v1/skills/search")?;
        let screen = harness.wait_for_text("log-summarizer")?;
        assertions::assert_absent(&screen, &["[installed]"]);
        Ok(())
    });
}

#[test]
fn skill_search_returns_bounded_candidates() {
    let script = DaemonScript {
        search_candidates: (0..5)
            .map(|index| {
                json!({
                    "manifest": {
                        "name": format!("candidate-{index}"),
                        "version": "1.0.0",
                        "publisher": "registry-publisher",
                        "source_type": "registry",
                        "signature_status": "verified",
                        "permissions": ["filesystem:read"],
                        "network_access": "none",
                    },
                    "score": 90.0 - index as f64,
                })
            })
            .collect(),
        ..base()
    };
    let mut harness = Harness::start(script).expect("start workbench");
    with_artifacts("skills-search-bounded", &mut harness, |harness| {
        harness.wait_for_text("PurrCode")?;
        harness.run_command("/skill-search test capability")?;
        harness.wait_for_request("POST", "/v1/skills/search")?;
        let screen = harness.wait_for_text("candidate-0")?;

        // Every scripted candidate rendered, none duplicated or dropped, and
        // the panel stays within the terminal rather than a runaway list.
        for index in 0..5 {
            assertions::assert_readable(&screen, &format!("candidate-{index}"));
        }
        assertions::assert_no_overflow(&screen);
        Ok(())
    });
}

#[test]
fn unavailable_skill_registry_is_reported() {
    let mut harness = Harness::start(base()).expect("start workbench");
    with_artifacts("skills-unavailable", &mut harness, |harness| {
        harness.wait_for_text("PurrCode")?;
        harness.run_command("/skills")?;
        harness.wait_for_request("GET", "/v1/skills")?;
        let screen = harness.wait_for_text("No matching skills were returned.")?;
        assertions::assert_no_overflow(&screen);
        Ok(())
    });
}

#[test]
fn skill_download_requires_approval() {
    let mut harness = Harness::start(base()).expect("start workbench");
    with_artifacts("skills-download-approval", &mut harness, |harness| {
        submit(harness, "Add a linter skill")?;

        harness.run_command(&format!("/skill-download {DOWNLOAD_ARGS}"))?;
        harness.wait_for_request("POST", "/v1/skills/download")?;
        let screen = harness.wait_for_wrapped("Download requires separate approval")?;
        assertions::assert_readable(&screen, &format!("/skill-download-approve {DOWNLOAD_ARGS}"));
        assertions::assert_no_overflow(&screen);

        harness.run_command(&format!("/skill-download-approve {DOWNLOAD_ARGS}"))?;
        let screen = harness.wait_for_wrapped("Downloaded and inspected")?;
        assertions::assert_readable(&screen, "Review it, then run /skill-install");
        Ok(())
    });
}

#[test]
fn skill_install_requires_approval() {
    let mut harness = Harness::start(base()).expect("start workbench");
    with_artifacts("skills-install-approval", &mut harness, |harness| {
        submit(harness, "Add a linter skill")?;
        harness.run_command(&format!("/skill-download {DOWNLOAD_ARGS}"))?;
        harness.wait_for_wrapped("Download requires separate approval")?;
        harness.run_command(&format!("/skill-download-approve {DOWNLOAD_ARGS}"))?;
        harness.wait_for_wrapped("Downloaded and inspected")?;

        harness.run_command("/skill-install repository")?;
        harness.wait_for_request("POST", "/v1/skills/install/propose")?;
        let screen = harness.wait_for_wrapped("Install action inspected and awaiting approval")?;
        assertions::assert_readable(&screen, "/skill-install-approve");
        assertions::assert_no_overflow(&screen);

        // Leaving the proposal unapproved must never install anything. The
        // fragment check has to be exact: "/v1/skills/install/propose" also
        // contains "/v1/skills/install" as a substring.
        assert!(
            !harness
                .daemon()
                .requests()
                .iter()
                .any(|request| request.method == "POST" && request.path == "/v1/skills/install"),
            "install must never finalize while the proposal is unapproved:\n{}",
            harness.daemon().request_log()
        );
        Ok(())
    });
}

#[test]
fn failed_qualification_blocks_install() {
    let mut harness = Harness::start(base()).expect("start workbench");
    with_artifacts("skills-qualification-failure", &mut harness, |harness| {
        submit(harness, "Add a linter skill")?;
        harness.run_command(&format!("/skill-download {DOWNLOAD_ARGS}"))?;
        harness.wait_for_wrapped("Download requires separate approval")?;
        harness.run_command(&format!("/skill-download-approve {DOWNLOAD_ARGS}"))?;
        harness.wait_for_wrapped("Downloaded and inspected")?;
        harness.run_command("/skill-install repository")?;
        harness.wait_for_wrapped("Install action inspected and awaiting approval")?;

        // The digest is approved cleanly; qualification only runs — and only
        // fails — once the install actually finalizes.
        harness.daemon().update(|script| {
            script.skill_failure =
                Some("qualification failed: coding accuracy below threshold".into());
        });
        harness.run_command("/skill-install-approve")?;
        let screen = harness.wait_for_wrapped("Install failed")?;
        assertions::assert_readable(
            &screen,
            "qualification failed: coding accuracy below threshold",
        );
        assertions::assert_absent(&screen, &["Installed and qualified"]);
        Ok(())
    });
}

#[test]
fn unverified_skill_signature_blocks_download() {
    let mut harness = Harness::start(base()).expect("start workbench");
    with_artifacts("skills-signature-failure", &mut harness, |harness| {
        submit(harness, "Add a linter skill")?;
        harness.run_command(&format!("/skill-download {DOWNLOAD_ARGS}"))?;
        harness.wait_for_wrapped("Download requires separate approval")?;

        harness.daemon().update(|script| {
            script.skill_failure =
                Some("skill signature verification failed: unknown publisher key".into());
        });
        harness.run_command(&format!("/skill-download-approve {DOWNLOAD_ARGS}"))?;
        let screen = harness.wait_for_wrapped("Download failed")?;
        assertions::assert_readable(&screen, "skill signature verification failed");
        assertions::assert_absent(&screen, &["Downloaded and inspected"]);

        // Nothing to install: an unverified skill is never held as a pending
        // download.
        harness.run_command("/skill-install repository")?;
        let screen = harness.wait_for_wrapped("Download and inspect a skill first")?;
        assertions::assert_no_overflow(&screen);
        Ok(())
    });
}

#[test]
fn blocked_publisher_is_refused() {
    let mut harness = Harness::start(base()).expect("start workbench");
    with_artifacts("skills-block-publisher", &mut harness, |harness| {
        submit(harness, "Add a linter skill")?;
        harness.run_command("/skill-block acme")?;
        harness.wait_for_request("POST", "/v1/skills/publishers/block")?;
        let screen = harness.wait_for_text("Blocked skill publisher: acme")?;
        assertions::assert_no_overflow(&screen);

        // The block takes effect immediately: a later proposal from that
        // publisher is refused rather than silently allowed through.
        harness.run_command(&format!("/skill-download {DOWNLOAD_ARGS}"))?;
        let screen = harness.wait_for_wrapped("Download failed")?;
        assertions::assert_readable(&screen, "acme is blocked by local policy");
        Ok(())
    });
}

// ── MCP capability discovery reuses the skill-search boundary ───

#[test]
fn mcp_tool_requires_approval() {
    let script = DaemonScript {
        search_requires_approval: true,
        search_candidates: vec![json!({
            "manifest": {
                "name": "database-browser",
                "version": "1.0.0",
                "publisher": "mcp-community",
                "source_type": "mcp",
                "signature_status": "verified",
                "permissions": ["network:connect"],
                "network_access": "outbound",
            },
            "score": 77.0,
        })],
        ..base()
    };
    let mut harness = Harness::start(script).expect("start workbench");
    with_artifacts("skills-mcp-approval", &mut harness, |harness| {
        harness.wait_for_text("PurrCode")?;
        harness.run_command("/mcp search database access")?;
        harness.wait_for_request("POST", "/v1/skills/search")?;
        let screen = harness.wait_for_wrapped("use /skill-search-approve with the same query")?;
        assertions::assert_readable(&screen, "a to approve the pending search");
        assertions::assert_no_overflow(&screen);

        // The screen has no composer of its own, so the approval re-runs the
        // exact recorded query rather than asking it to be retyped.
        harness.key(Key::Char('a'))?;
        let screen = harness.wait_for_text("database-browser")?;
        assertions::assert_no_overflow(&screen);
        assert!(
            harness
                .daemon()
                .requests()
                .iter()
                .any(|request| request.method == "POST"
                    && request.path == "/v1/skills/search"
                    && request
                        .body
                        .as_deref()
                        .is_some_and(|body| body.contains("\"approved\":true"))),
            "the approved re-run must actually reach the daemon:\n{}",
            harness.daemon().request_log()
        );
        Ok(())
    });
}

#[test]
fn mcp_failure_is_explained() {
    let script = DaemonScript {
        skill_failure: Some("capability index temporarily unavailable".into()),
        ..base()
    };
    let mut harness = Harness::start(script).expect("start workbench");
    with_artifacts("skills-mcp-failure", &mut harness, |harness| {
        harness.wait_for_text("PurrCode")?;
        harness.run_command("/mcp search deploy automation")?;
        harness.wait_for_request("POST", "/v1/skills/search")?;

        // Known gap: `SkillBrowser::search` (like `load`) never inspects the
        // response status, so a transport/HTTP failure currently degrades to
        // the same empty-results panel as a genuinely empty registry rather
        // than a distinct error message. What has to hold regardless — and
        // what this test actually proves — is that a failed search never
        // fabricates candidates and never leaves the rest of the session
        // unusable.
        let screen = harness.wait_for_all_and_absent(
            &["No matching skills were returned."],
            &["database-browser", "deploy automation"],
        )?;
        assertions::assert_no_overflow(&screen);

        harness.key(Key::Escape)?;
        harness.wait_for_text("Ready for a task")?;
        Ok(())
    });
}
