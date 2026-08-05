//! Provider discovery, manual setup, script import, and profile management.

use purrcode_tui_e2e::fake_daemon::DaemonScript;
use purrcode_tui_e2e::harness::with_artifacts;
use purrcode_tui_e2e::{Harness, Key, assertions};
use serde_json::json;

fn empty() -> DaemonScript {
    DaemonScript::default()
}

fn with_provider(name: &str) -> DaemonScript {
    DaemonScript {
        providers: vec![json!({
            "name": name,
            "provider_type": "ollama",
            "base_url": "http://127.0.0.1:11434",
            "local": true,
        })],
        models: vec![json!({"id": format!("{name}/coder:7b"), "default": true, "local": true})],
        discovered_models: vec!["coder:7b".into()],
        ..DaemonScript::default()
    }
}

#[test]
fn local_provider_discovery_saves_after_real_test() {
    let mut harness = Harness::start(DaemonScript {
        discovered_models: vec!["coder:7b".into(), "reviewer:3b".into()],
        ..empty()
    })
    .expect("start workbench");
    with_artifacts("provider-connect-local", &mut harness, |harness| {
        // Wait for the startup message, not the empty-state headline alone: the
        // headline is present even in the initial pre-check render, before the
        // real async startup round-trip (and the process's raw-mode key loop)
        // has actually settled. Typing before that settles risks a race.
        harness.wait_for_text("Connect a model provider to begin")?;
        harness.run_command("/connect")?;
        // "Connect a model provider" is a substring of the empty-state headline
        // too ("...to begin"), which is already on screen before the command
        // runs — waiting on it would pass trivially without the Discovery
        // screen ever having loaded. "LM Studio" only ever appears there.
        let screen = harness.wait_for_text("LM Studio")?;
        assertions::assert_visible(&screen, &["Ollama", "LM Studio", "OpenAI"]);

        // Ollama is selected by default; Enter chooses it and opens the form.
        harness.key(Key::Enter)?;
        harness.wait_for_request("POST", "/v1/providers/discover")?;
        let screen = harness.wait_for_text("coder:7b")?;
        assertions::assert_readable(&screen, "Discovered models");

        harness.key(Key::Ctrl('g'))?;
        harness.wait_for_request("POST", "/v1/providers")?;
        let screen = harness.wait_for_wrapped("Provider ollama verified")?;
        assertions::assert_no_overflow(&screen);
        assertions::assert_no_secret(&screen, &[]);
        Ok(())
    });
}

#[test]
fn remote_provider_saves_credential_reference_only() {
    let mut harness = Harness::start(empty()).expect("start workbench");
    with_artifacts("provider-connect-remote", &mut harness, |harness| {
        harness.wait_for_text("Connect a model provider to begin")?;
        harness.run_command("/connect import")?;
        harness.wait_for_text("Import provider configuration")?;

        const SECRET: &str = "detected-secret-abcdef123456";
        harness.key(Key::Paste(format!(
            "BASE_URL=https://api.example.test/v1\nMODEL=demo-model\nAPI_KEY={SECRET}\n"
        )))?;
        harness.key(Key::Ctrl('g'))?;
        let screen = harness.wait_for_text("Resolve detected authentication")?;
        assertions::assert_no_secret(&screen, &[SECRET]);

        // Move to "Use existing environment variable" and choose it.
        harness.key(Key::Down)?;
        harness.key(Key::Enter)?;
        harness.wait_for_text("Use an environment-variable reference")?;
        harness.key(Key::Ctrl('g'))?;
        harness.wait_for_text("Review provider")?;

        harness.key(Key::Ctrl('g'))?;
        harness.wait_for_request("POST", "/v1/providers")?;
        let screen = harness.wait_for_wrapped("verified")?;
        assertions::assert_no_secret(&screen, &[SECRET]);

        // The daemon received a credential reference, never the raw secret.
        let requests = harness.daemon().requests();
        let save = requests
            .iter()
            .find(|request| request.method == "POST" && request.path == "/v1/providers")
            .expect("a save request must have been sent");
        let body = save.body.as_deref().unwrap_or_default();
        assert!(
            body.contains("credential_reference"),
            "the save must carry a credential reference: {body}"
        );
        assert!(
            !body.contains(SECRET),
            "the raw secret must never reach the daemon: {body}"
        );
        Ok(())
    });
}

#[test]
fn imported_configuration_is_parsed_not_executed() {
    let mut harness = Harness::start(empty()).expect("start workbench");
    with_artifacts("provider-import-configuration", &mut harness, |harness| {
        harness.wait_for_text("Connect a model provider to begin")?;
        harness.run_command("/connect import")?;
        harness.wait_for_text("Import provider configuration")?;
        harness.key(Key::Paste(
            "BASE_URL=https://api.example.test/v1\nMODEL=demo-model\nAPI_KEY=another-secret-value\n"
                .to_owned(),
        ))?;
        harness.key(Key::Ctrl('g'))?;
        harness.wait_for_text("Resolve detected authentication")?;

        // Reviewing an import must not itself create a session or a provider.
        assert!(!harness.daemon().saw("POST", "/v1/sessions"));
        assert!(!harness.daemon().saw("POST", "/v1/providers"));
        Ok(())
    });
}

#[test]
fn pasted_secret_is_redacted_before_history() {
    let mut harness = Harness::start(with_provider("local")).expect("start workbench");
    with_artifacts("provider-secret-guard", &mut harness, |harness| {
        harness.wait_for_text("Ready for a task")?;
        const SECRET: &str = "sk-live-abcdefghijklmnopqrstuvwxyz012345";
        harness.key(Key::Paste(format!("OPENAI_API_KEY={SECRET}")))?;
        harness.key(Key::Ctrl('g'))?;
        // Not `wait_for_text` then assert: a frame arrives over several PTY
        // reads, so the guard can be on screen while the composer below it is
        // still showing the previous frame's draft. Waiting for both conditions
        // at once pins the assertion to one consistent render — and for a
        // secret assertion, a flaky pass is worse than a flaky failure.
        let screen = harness
            .wait_for_all_and_absent(&["Sensitive content detected"], &[SECRET, "sk-live-"])?;
        assertions::assert_no_secret(&screen, &[SECRET]);
        assertions::assert_readable(&screen, "R  Redact and send");

        harness.key(Key::Char('r'))?;
        harness.wait_for_request("POST", "/v1/sessions")?;
        // The redacted form reaches history and the daemon; the raw secret
        // never does.
        assert!(
            !harness.daemon().request_log().contains(SECRET),
            "the raw secret reached the daemon:\n{}",
            harness.daemon().request_log()
        );
        let screen = harness.screen();
        assertions::assert_no_secret(&screen, &[SECRET]);
        Ok(())
    });
}

#[test]
fn invalid_endpoint_fails_visibly() {
    let mut harness = Harness::start(DaemonScript {
        discovered_models: vec!["coder:7b".into()],
        provider_failure: Some("connection refused".into()),
        ..empty()
    })
    .expect("start workbench");
    with_artifacts("provider-invalid-endpoint", &mut harness, |harness| {
        harness.wait_for_text("Connect a model provider to begin")?;
        harness.run_command("/connect")?;
        harness.key(Key::Enter)?;
        harness.wait_for_text("coder:7b")?;
        harness.key(Key::Ctrl('g'))?;

        let screen = harness.wait_for_wrapped("connection refused")?;
        assertions::assert_readable(&screen, "Error:");
        // The setup screen must remain open and editable, not silently close.
        assertions::assert_visible(&screen, &["Review provider"]);
        assert!(!harness.daemon().saw("GET", "/v1/models"));
        Ok(())
    });
}

#[test]
fn unavailable_provider_never_reports_success() {
    let mut harness = Harness::start(DaemonScript {
        provider_failure: Some("no route to host".into()),
        ..with_provider("local")
    })
    .expect("start workbench");
    with_artifacts("provider-unavailable", &mut harness, |harness| {
        harness.wait_for_text("Ready for a task")?;
        harness.run_command("/provider test local")?;
        harness.wait_for_request("POST", "/v1/providers/test")?;
        let screen = harness.wait_for_wrapped("Provider test failed")?;
        assertions::assert_readable(&screen, "no route to host");
        assertions::assert_absent(&screen, &["verified in"]);
        Ok(())
    });
}

#[test]
fn provider_list_shows_privacy_class() {
    let mut harness = Harness::start(with_provider("local")).expect("start workbench");
    with_artifacts("provider-list", &mut harness, |harness| {
        harness.wait_for_text("Ready for a task")?;
        harness.run_command("/provider list")?;
        harness.wait_for_request("GET", "/v1/providers")?;
        let screen = harness.wait_for_wrapped("Providers:")?;
        assertions::assert_readable(&screen, "local");
        assertions::assert_readable(&screen, "true");
        Ok(())
    });
}

#[test]
fn saved_profile_runs_real_connection_test() {
    let mut harness = Harness::start(with_provider("local")).expect("start workbench");
    with_artifacts("provider-test", &mut harness, |harness| {
        harness.wait_for_text("Ready for a task")?;
        harness.run_command("/provider test local")?;
        harness.wait_for_request("POST", "/v1/providers/test")?;
        let screen = harness.wait_for_wrapped("local: verified in")?;
        assertions::assert_no_overflow(&screen);
        Ok(())
    });
}

#[test]
fn saved_profile_reopens_in_form() {
    let mut harness = Harness::start(with_provider("local")).expect("start workbench");
    with_artifacts("provider-edit", &mut harness, |harness| {
        harness.wait_for_text("Ready for a task")?;
        harness.run_command("/provider edit local")?;
        harness.wait_for_request("GET", "/v1/providers/local")?;
        let screen = harness.wait_for_text("Review provider")?;
        assertions::assert_readable(&screen, "127.0.0.1:11434");
        Ok(())
    });
}

#[test]
fn removed_profile_disappears_from_list() {
    let mut harness = Harness::start(with_provider("local")).expect("start workbench");
    with_artifacts("provider-remove", &mut harness, |harness| {
        harness.wait_for_text("Ready for a task")?;
        harness.run_command("/provider remove local")?;
        harness.wait_for_request("DELETE", "/v1/providers/local")?;
        let screen = harness.wait_for_wrapped("Provider local removed")?;
        assertions::assert_no_overflow(&screen);

        harness.run_command("/provider list")?;
        let screen = harness.wait_for_text("Providers:")?;
        assertions::assert_absent(&screen, &["\"name\":\"local\""]);
        Ok(())
    });
}

#[test]
fn unparseable_import_is_refused() {
    let mut harness = Harness::start(empty()).expect("start workbench");
    with_artifacts("provider-import-unparseable", &mut harness, |harness| {
        harness.wait_for_text("Connect a model provider to begin")?;
        harness.run_command("/connect import")?;
        harness.wait_for_text("Import provider configuration")?;
        harness.key(Key::Paste(
            "just some ordinary prose with no configuration in it at all".to_owned(),
        ))?;
        harness.key(Key::Ctrl('g'))?;
        let screen = harness.wait_for_wrapped("Error:")?;
        // Refused, and still on the import screen so the user can retype it.
        assertions::assert_visible(&screen, &["Import provider configuration"]);
        assert!(!harness.daemon().saw("POST", "/v1/providers"));
        Ok(())
    });
}

#[test]
fn overwriting_a_profile_requires_explicit_replace() {
    let mut harness = Harness::start(DaemonScript {
        discovered_models: vec!["coder:7b".into()],
        ..with_provider("ollama")
    })
    .expect("start workbench");
    with_artifacts("provider-edit-explicit-replace", &mut harness, |harness| {
        harness.wait_for_text("Ready for a task")?;
        // A fresh /connect using the default Ollama profile name collides with
        // the already-saved "ollama" profile.
        harness.run_command("/connect")?;
        harness.key(Key::Enter)?;
        harness.wait_for_text("coder:7b")?;
        harness.key(Key::Ctrl('g'))?;

        let screen = harness.wait_for_wrapped("already exists")?;
        assertions::assert_readable(&screen, "Press Ctrl+G again to explicitly replace it");
        let providers_before = harness
            .daemon()
            .requests()
            .iter()
            .filter(|request| request.method == "POST" && request.path == "/v1/providers")
            .count();
        assert_eq!(
            providers_before, 0,
            "the collision must not have saved anything yet"
        );

        // Pressing Ctrl+G again explicitly replaces it.
        harness.key(Key::Ctrl('g'))?;
        harness.wait_for_request("POST", "/v1/providers")?;
        let screen = harness.wait_for_wrapped("verified")?;
        assertions::assert_no_overflow(&screen);
        Ok(())
    });
}

#[test]
fn removing_an_in_use_provider_is_refused() {
    let mut harness = Harness::start(DaemonScript {
        provider_in_use: Some("local".into()),
        ..with_provider("local")
    })
    .expect("start workbench");
    with_artifacts("provider-remove-in-use", &mut harness, |harness| {
        harness.wait_for_text("Ready for a task")?;
        harness.run_command("/provider remove local")?;
        harness.wait_for_request("DELETE", "/v1/providers/local")?;
        let screen = harness.wait_for_wrapped("in use by an active session")?;
        assertions::assert_absent(&screen, &["Provider local removed"]);

        harness.run_command("/provider list")?;
        let screen = harness.wait_for_text("Providers:")?;
        assertions::assert_readable(&screen, "local");
        Ok(())
    });
}
