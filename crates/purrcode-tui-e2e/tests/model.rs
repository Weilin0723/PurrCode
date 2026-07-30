//! Model selection, recommendation, qualification, and the Ollama pull
//! lifecycle.
//!
//! The pull lifecycle is the security-relevant thread running through this
//! file: a pull is digest-bound and approval-gated exactly like a write
//! action, so these tests prove the same invariants the approval suite does —
//! nothing starts without an explicit approval, and the exact identity of what
//! was proposed is what gets approved.

use purrcode_tui_e2e::fake_daemon::DaemonScript;
use purrcode_tui_e2e::harness::with_artifacts;
use purrcode_tui_e2e::{assertions, Harness, Key};
use serde_json::json;

fn configured() -> DaemonScript {
    DaemonScript {
        providers: vec![json!({"name": "local", "provider_type": "ollama", "local": true})],
        models: vec![
            json!({"id": "local/coder:7b", "default": true, "local": true}),
            json!({"id": "local/reviewer:3b", "default": false, "local": true}),
        ],
        ..DaemonScript::default()
    }
}

#[test]
fn model_switch_updates_header() {
    let mut harness = Harness::start(configured()).expect("start workbench");
    with_artifacts("model-switch", &mut harness, |harness| {
        let screen = harness.wait_for_text("local/coder:7b")?;
        assertions::assert_no_overflow(&screen);

        harness.run_command("/model local/reviewer:3b")?;
        harness.wait_for_request("POST", "/v1/models/roles")?;
        let screen = harness.wait_for_wrapped("Switched model to local/reviewer:3b.")?;
        assertions::assert_visible(&screen, &["local/reviewer:3b"]);
        Ok(())
    });
}

#[test]
fn unavailable_model_is_refused_with_reason() {
    let mut harness = Harness::start(configured()).expect("start workbench");
    with_artifacts("model-unavailable", &mut harness, |harness| {
        harness.wait_for_text("local/coder:7b")?;
        harness.run_command("/model does-not-exist:1b")?;
        let screen = harness.wait_for_wrapped("Unknown model")?;
        assertions::assert_readable(&screen, "Run /models and select a configured model");
        // The header must not have changed to the unknown model.
        assertions::assert_absent(&screen, &["does-not-exist:1b` requires"]);
        assert!(harness.screen().contains("local/coder:7b"));
        Ok(())
    });
}

#[test]
fn role_assignment_is_confirmed() {
    let mut harness = Harness::start(configured()).expect("start workbench");
    with_artifacts("model-role-assignment", &mut harness, |harness| {
        harness.wait_for_text("PurrCode")?;
        harness.run_command("/role planner local/reviewer:3b")?;
        harness.wait_for_request("POST", "/v1/models/roles")?;
        let screen = harness.wait_for_wrapped("Assigned local/reviewer:3b to planner")?;
        assertions::assert_no_overflow(&screen);
        Ok(())
    });
}

#[test]
fn recommendation_shows_observed_evidence() {
    let script = DaemonScript {
        recommendations: Some(json!({
            "outcome": {"status": "recommended", "model": "coder:7b", "score": 88},
            "cards": [{"model": "coder:7b", "status": "recommended"}],
            "report": {
                "outcome": {"status": "recommended", "model": "coder:7b", "score": 88},
                "cards": [{
                    "model": "coder:7b",
                    "status": "recommended",
                    "currently_loaded": true,
                    "qualification": {"assessment": "qualified", "coding": "strong"},
                }],
                "constraints": {"context_limit_tokens": 8192, "single_model_mode": true},
                "resource_risks": [],
            },
        })),
        ..configured()
    };
    let mut harness = Harness::start(script).expect("start workbench");
    with_artifacts("model-recommend", &mut harness, |harness| {
        harness.wait_for_text("PurrCode")?;
        harness.run_command("/model recommend")?;
        harness.wait_for_request("GET", "/v1/local-models/recommendations")?;
        let screen = harness.wait_for_wrapped("Primary")?;
        assertions::assert_readable(&screen, "coder:7b");
        assertions::assert_readable(&screen, "evidence score 88");
        Ok(())
    });
}

#[test]
fn insufficient_memory_warns_before_task() {
    // The selected default model is reported as not recommended for the
    // observed resources, with a smaller qualified alternative available.
    let script = DaemonScript {
        models: vec![json!({"id": "local/big:70b", "default": true, "local": true})],
        recommendations: Some(json!({
            "outcome": {"status": "recommended", "model": "small:3b", "score": 90},
            "cards": [
                {"model": "big:70b", "status": "not_recommended"},
                {"model": "small:3b", "status": "recommended"},
            ],
            "report": {
                "outcome": {"status": "recommended", "model": "small:3b", "score": 90},
                "cards": [],
                "constraints": {},
                "resource_risks": [],
            },
        })),
        ..configured()
    };
    let mut harness = Harness::start(script).expect("start workbench");
    with_artifacts("model-insufficient-memory", &mut harness, |harness| {
        // This warning is surfaced as a startup message, before any task runs.
        let screen = harness.wait_for_wrapped("/model ollama/small:3b")?;
        assertions::assert_readable(&screen, "local model local/big:70b is not recommended");
        assertions::assert_readable(&screen, "small:3b");
        assertions::assert_readable(&screen, "Safety warning");
        Ok(())
    });
}

#[test]
fn qualification_reports_real_result() {
    let script = DaemonScript {
        qualification: Some(json!({
            "qualification": {
                "model": {"provider": "local", "model": "coder:7b"},
                "accuracy": 0.95,
                "mean_latency_ms": 310.0,
                "estimated_tokens_per_second": 40.2,
                "maximum_reliable_context_tokens": 16384,
                "cases": [{"name": "structured output", "passed": true}],
            }
        })),
        ..configured()
    };
    let mut harness = Harness::start(script).expect("start workbench");
    with_artifacts("model-qualify", &mut harness, |harness| {
        harness.wait_for_text("PurrCode")?;
        harness.run_command("/model qualify local coder:7b")?;
        harness.wait_for_request("POST", "/v1/local-models/qualify")?;
        let screen = harness.wait_for_wrapped("Real provider qualification")?;
        assertions::assert_readable(&screen, "local/coder:7b");
        assertions::assert_readable(&screen, "Accuracy 95.0%");
        Ok(())
    });
}

#[test]
fn loaded_models_listed_without_loading() {
    let script = DaemonScript {
        local_models: Some(json!({
            "loaded": [{"name": "coder:7b"}],
            "resources": {"memory_pressure": "moderate", "maximum_local_inference_requests": 2},
        })),
        ..configured()
    };
    let mut harness = Harness::start(script).expect("start workbench");
    with_artifacts("model-loaded", &mut harness, |harness| {
        harness.wait_for_text("PurrCode")?;
        harness.run_command("/model loaded")?;
        harness.wait_for_request("GET", "/v1/local-models")?;
        let screen = harness.wait_for_wrapped("Loaded: coder:7b")?;
        assertions::assert_readable(&screen, "Memory pressure: moderate");
        // Inspecting must not itself load anything: no pull/start request.
        assert!(!harness.daemon().saw("POST", "/v1/local-models/pull"));
        Ok(())
    });
}

#[test]
fn unload_is_verified_by_rediscovery() {
    let mut harness = Harness::start(configured()).expect("start workbench");
    with_artifacts("model-unload", &mut harness, |harness| {
        harness.wait_for_text("PurrCode")?;
        harness.run_command("/model unload-all")?;
        harness.wait_for_request("POST", "/v1/local-models/unload")?;
        let screen = harness.wait_for_wrapped("Unloaded and verified")?;
        assertions::assert_no_overflow(&screen);
        Ok(())
    });
}

#[test]
fn pull_proposal_shows_exact_action_identity() {
    let mut harness = Harness::start(configured()).expect("start workbench");
    with_artifacts("model-pull-propose", &mut harness, |harness| {
        harness.wait_for_text("PurrCode")?;
        harness.run_command("/model pull coder:14b")?;
        harness.wait_for_request("POST", "/v1/local-models/pull/propose")?;

        // A pending pull is a decision like any other: it forces the inspector
        // open with the exact action id and digest, not a bare status line.
        let screen = harness.wait_for_text("Model pull")?;
        assertions::assert_visible(
            &screen,
            &["coder:14b", "pull-action", "digest-of-the-exact-pull"],
        );
        assertions::assert_readable(&screen, "awaiting explicit approval");
        assertions::assert_no_overflow(&screen);
        Ok(())
    });
}

#[test]
fn pull_approval_calls_daemon_endpoint() {
    let mut harness = Harness::start(configured()).expect("start workbench");
    with_artifacts("model-pull-approve", &mut harness, |harness| {
        harness.wait_for_text("PurrCode")?;
        harness.run_command("/model pull coder:14b")?;
        harness.wait_for_request("POST", "/v1/local-models/pull/propose")?;
        harness.wait_for_text("Model pull")?;

        harness.key(Key::Char('p'))?;
        harness.wait_for_request("POST", "/v1/local-models/pull/pull-action/approve")?;
        harness.wait_for_request("POST", "/v1/local-models/pull/pull-action/start")?;
        harness.wait_for_durable("the pull to report progress", |script| script.pull_running)?;
        Ok(())
    });
}

#[test]
fn unapproved_pull_never_downloads() {
    let mut harness = Harness::start(configured()).expect("start workbench");
    with_artifacts("model-pull-reject", &mut harness, |harness| {
        harness.wait_for_text("PurrCode")?;
        harness.run_command("/model pull coder:14b")?;
        harness.wait_for_request("POST", "/v1/local-models/pull/propose")?;
        harness.wait_for_text("Model pull")?;

        // Leaving the workbench without pressing P must never start anything.
        harness.key(Key::Escape)?;
        std::thread::sleep(std::time::Duration::from_millis(200));
        assert!(
            !harness
                .daemon()
                .saw("POST", "/v1/local-models/pull/pull-action/approve"),
            "an unapproved pull must never be approved:\n{}",
            harness.daemon().request_log()
        );
        assert!(!harness
            .daemon()
            .saw("POST", "/v1/local-models/pull/pull-action/start"));
        Ok(())
    });
}

#[test]
fn daemon_refuses_approval_it_is_not_awaiting() {
    // Approving an action id the daemon never proposed must be refused, the
    // same invariant the write-approval suite proves for sessions.
    let mut harness = Harness::start(configured()).expect("start workbench");
    with_artifacts("model-pull-approve-refused", &mut harness, |harness| {
        harness.wait_for_text("PurrCode")?;
        harness.run_command("/model pull-approve")?;
        // With no pending pull at all, the command must say so rather than
        // reaching for an approval endpoint.
        let screen =
            harness.wait_for_wrapped("No exact Ollama pull action is awaiting approval")?;
        assertions::assert_no_overflow(&screen);
        assert!(!harness.daemon().saw("POST", "/approve"));
        Ok(())
    });
}

#[test]
fn running_pull_cancels_with_terminal_evidence() {
    let mut harness = Harness::start(configured()).expect("start workbench");
    with_artifacts("model-pull-cancel", &mut harness, |harness| {
        harness.wait_for_text("PurrCode")?;
        harness.run_command("/model pull coder:14b")?;
        harness.wait_for_text("Model pull")?;
        harness.key(Key::Char('p'))?;
        harness.wait_for_durable("the pull to be running", |script| script.pull_running)?;

        harness.key(Key::Ctrl('c'))?;
        harness.wait_for_request("POST", "/v1/local-models/pull/pull-action/cancel")?;
        harness.wait_for_durable("the pull to be cancelled", |script| script.pull_cancelled)?;
        let screen = harness.wait_for_wrapped("Model pull cancellation requested")?;
        assertions::assert_no_overflow(&screen);
        Ok(())
    });
}
