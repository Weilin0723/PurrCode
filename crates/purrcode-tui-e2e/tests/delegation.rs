//! The agent workspace and integration review, driven through a real PTY
//! (v1.4 §PR11, §PR12).
//!
//! Two claims are worth testing at this level, and neither can be checked from
//! a unit test. First, that the worker tree the user sees is the daemon's
//! answer — including the parts that are uncomfortable, like a conflict or a
//! blocked dependency. Second, that pressing a key really reaches a daemon
//! endpoint: a panel that accepted a patch locally and told the user it had
//! landed would be the worst possible bug in this feature.

use std::collections::BTreeMap;

use purrcode_tui_e2e::fake_daemon::RecordedRequest;
use purrcode_tui_e2e::fake_daemon::{DaemonScript, ScriptedSession};
use purrcode_tui_e2e::{Harness, HarnessOptions, Key};
use serde_json::{Value, json};

const SESSION: &str = "delegation-session";

fn base() -> DaemonScript {
    let mut sessions = BTreeMap::new();
    sessions.insert(SESSION.to_owned(), ScriptedSession::new("active"));
    DaemonScript {
        providers: vec![json!({"name": "local", "provider_type": "ollama", "local": true})],
        models: vec![json!({"id": "local/fake:1b", "default": true, "local": true})],
        sessions,
        ..DaemonScript::default()
    }
}

/// A tree with one worker awaiting review and one blocked reviewer.
fn tree() -> Value {
    json!({
        "session_id": SESSION,
        "objective": "add oauth",
        "classification": "parallel",
        "plan_reason": "3 independent components can run in isolated worktrees",
        "running": 1,
        "awaiting_decision": 1,
        "workers_started": 2,
        "workers_completed": 1,
        "total_worker_usage": { "input_tokens": 12000 },
        "governance": { "maximum_parallel_workers": 3 },
        "delegations": [
            {
                "delegation_id": "11111111-1111-1111-1111-111111111111",
                "objective": "implement the token exchange",
                "capability": "implement_backend",
                "status": "completed",
                "integration_state": "proposed",
                "access": "writable",
                "agent_profile": "backend-specialist",
                "allowed_paths": ["src/auth/**"],
                "changed_paths": ["src/auth/token.rs"],
                "tool_calls": 6,
                "usage": { "input_tokens": 8000, "duration_seconds": 45 },
                "budget": { "maximum_input_tokens": 120000 },
                "validations": [{ "name": "cargo test", "status": "passed" }],
                "findings": [],
                "conflicts": [],
                "evidence_ids": ["e1"],
                "repair_cycles": 0,
                "summary": "implemented"
            },
            {
                "delegation_id": "22222222-2222-2222-2222-222222222222",
                "objective": "review the auth change",
                "capability": "security_review",
                "status": "blocked",
                "integration_state": "not_proposed",
                "access": "read_only",
                "agent_profile": "security-reviewer",
                "allowed_paths": ["src/**"],
                "changed_paths": [],
                "tool_calls": 0,
                "usage": {},
                "budget": { "maximum_input_tokens": 80000 },
                "validations": [],
                "findings": [],
                "conflicts": [],
                "evidence_ids": [],
                "blocked_by": "33333333-3333-3333-3333-333333333333",
                "repair_cycles": 0
            }
        ]
    })
}

fn review() -> Value {
    json!({
        "delegation": { "delegation_id": "11111111-1111-1111-1111-111111111111" },
        "patch_digest": "abcdef0123456789",
        "effective_patch_digest": "abcdef0123456789",
        "base_snapshot_digest": "basedigest",
        "base_drifted": false,
        "decision": "ready_for_approval",
        "patch": "diff --git a/src/auth/token.rs b/src/auth/token.rs\n@@ -10,3 +10,4 @@\n+let token = exchange();\n",
        "hunks": [
            {
                "index": 0,
                "path": "src/auth/token.rs",
                "old_start": 10,
                "old_lines": 3,
                "new_start": 10,
                "new_lines": 4
            },
            {
                "index": 1,
                "path": "src/auth/store.rs",
                "old_start": 1,
                "old_lines": 2,
                "new_start": 1,
                "new_lines": 5
            }
        ]
    })
}

fn attached(script: DaemonScript) -> Harness {
    let workspace = purrcode_tui_e2e::fixtures::Workspace::new().expect("workspace");
    workspace
        .seed_ui_state(Some(SESSION), "")
        .expect("seed session");
    Harness::start_in(workspace, script, HarnessOptions::default()).expect("start workbench")
}

/// Resume the seeded session, then open the agent workspace.
///
/// The workbench asks resume-or-new at startup when a durable session exists,
/// so the panel is only reachable after that choice is made.
fn open_workspace(harness: &mut Harness) -> anyhow::Result<()> {
    harness.wait_for_text("An existing session was found")?;
    harness.key(Key::Char('r'))?;
    harness.run_command("/agents")?;
    harness.wait_for_request("GET", &format!("/v1/sessions/{SESSION}/delegations"))?;
    Ok(())
}

#[test]
fn the_worker_tree_shows_every_specialist_with_its_scope_and_status() -> anyhow::Result<()> {
    let mut harness = attached(DaemonScript {
        delegations: Some(tree()),
        ..base()
    });
    open_workspace(&mut harness)?;

    let screen = harness.wait_for_text("backend-specialist")?.text();
    // The specialist, what it was allowed to touch, and what it actually
    // changed — the three things a reviewer needs before deciding.
    assert!(screen.contains("implement_backend"), "{screen}");
    assert!(screen.contains("src/auth/**"), "{screen}");
    assert!(screen.contains("src/auth/token.rs"), "{screen}");
    assert!(screen.contains("Awaiting review"), "{screen}");
    // A blocked worker says so rather than looking idle.
    assert!(screen.contains("security-reviewer"), "{screen}");
    assert!(screen.contains("Blocked by"), "{screen}");
    // The classifier's reasoning is visible, not buried in a log.
    assert!(screen.contains("isolated worktrees"), "{screen}");
    Ok(())
}

#[test]
fn accepting_selected_hunks_sends_exactly_those_hunks_to_the_daemon() -> anyhow::Result<()> {
    let mut harness = attached(DaemonScript {
        delegations: Some(tree()),
        delegation_review: Some(review()),
        ..base()
    });
    open_workspace(&mut harness)?;
    harness.wait_for_text("backend-specialist")?;

    // Enter opens the review for the selected worker.
    harness.key(Key::Enter)?;
    harness.wait_for_request(
        "GET",
        &format!("/v1/sessions/{SESSION}/delegations/11111111-1111-1111-1111-111111111111"),
    )?;
    let screen = harness.wait_for_text("Integration review")?.text();
    assert!(screen.contains("src/auth/token.rs"), "{screen}");
    assert!(screen.contains("Accept all"), "{screen}");

    // Select the second hunk only, then accept.
    harness.key(Key::Down)?;
    harness.key(Key::Char(' '))?;
    harness.wait_for_text("Accept 1 selected hunk")?;
    harness.key(Key::Char('a'))?;
    let path =
        format!("/v1/sessions/{SESSION}/delegations/11111111-1111-1111-1111-111111111111/accept");
    let requests = harness.wait_for_request("POST", &path)?;
    // The daemon is told which hunks, so the patch it applies is the patch the
    // user selected — not the worker's original.
    let body = body_of(&requests, &path);
    assert!(
        body.contains("\"hunks\":[1]"),
        "the accept must carry exactly the selected hunk: {body}"
    );
    Ok(())
}

#[test]
fn a_daemon_refusal_is_shown_rather_than_reported_as_success() -> anyhow::Result<()> {
    // The failure case that matters: the daemon refuses (an unresolved
    // conflict, a drifted patch), and the panel must not tell the user their
    // change landed.
    let mut harness = attached(DaemonScript {
        delegations: Some(tree()),
        delegation_review: Some(review()),
        integration_refusal: Some(
            "unresolved conflicts must be resolved before this patch can be accepted".into(),
        ),
        ..base()
    });
    open_workspace(&mut harness)?;
    harness.wait_for_text("backend-specialist")?;
    harness.key(Key::Enter)?;
    harness.wait_for_text("Integration review")?;
    harness.key(Key::Char('a'))?;
    harness.wait_for_request(
        "POST",
        &format!("/v1/sessions/{SESSION}/delegations/11111111-1111-1111-1111-111111111111/accept"),
    )?;

    let screen = harness.wait_for_text("The daemon refused")?.text();
    assert!(
        !screen.contains("Worker changes accepted"),
        "a refused accept must not report success: {screen}"
    );
    Ok(())
}

#[test]
fn rejecting_a_worker_reaches_the_daemon() -> anyhow::Result<()> {
    let mut harness = attached(DaemonScript {
        delegations: Some(tree()),
        ..base()
    });
    open_workspace(&mut harness)?;
    harness.wait_for_text("backend-specialist")?;
    harness.key(Key::Char('r'))?;
    let path =
        format!("/v1/sessions/{SESSION}/delegations/11111111-1111-1111-1111-111111111111/reject");
    let requests = harness.wait_for_request("POST", &path)?;
    let body = body_of(&requests, &path);
    assert!(
        body.contains("\"reason\"") && !body.contains("\"reason\":\"\""),
        "a rejection must record why: {body}"
    );
    Ok(())
}

#[test]
fn an_empty_tree_says_so_instead_of_rendering_nothing() -> anyhow::Result<()> {
    let mut harness = attached(base());
    open_workspace(&mut harness)?;
    harness.wait_for_text("No work has been delegated")?;
    Ok(())
}

/// The body of the last matching request, or an empty string.
fn body_of(requests: &[RecordedRequest], path: &str) -> String {
    requests
        .iter()
        .rev()
        .find(|request| request.path == path)
        .and_then(|request| request.body.clone())
        .unwrap_or_default()
}
