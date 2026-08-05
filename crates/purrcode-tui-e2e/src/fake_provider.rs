//! Deterministic provider behaviour, expressed as durable event sequences.
//!
//! The workbench never talks to a provider directly — the daemon does. So a
//! "fake provider" here is a scripted set of durable events that a real provider
//! interaction would have produced, plus the latency and failure shapes a test
//! needs to exercise streaming, retry and cancellation.
//!
//! Using the runtime's own action and constraint types (rather than hand-written
//! JSON) means a test cannot accidentally assert against a payload shape the
//! daemon would never emit.

use purrcode_runtime_core::{ActionConstraints, ProposedAction, WriteFileAction};
use serde_json::{Value, json};
use std::path::PathBuf;

/// Constraints for an action scoped to one directory with no network access.
pub fn scoped_constraints(repository: &str, write_globs: &[&str]) -> ActionConstraints {
    ActionConstraints {
        working_directory: PathBuf::from(repository),
        network: false,
        timeout_seconds: 120,
        maximum_output_bytes: 65_536,
        allowed_write_globs: write_globs.iter().map(|glob| (*glob).to_owned()).collect(),
        maximum_changed_files: write_globs.len().max(1),
    }
}

/// The events a session records when it starts and prepares context.
pub fn startup(objective: &str, repository: &str) -> Vec<Value> {
    vec![
        json!({"event": "session_created", "data": {"objective": objective, "repository": repository}}),
        json!({"event": "context_indexed", "data": {"files": 12, "symbols": 84, "sensitive_files": 0}}),
    ]
}

/// A plan with the given steps.
///
/// `PlanCreated` carries no revision — the first plan is simply the plan. The
/// fixture used to add `revision: 1`, which meant every client was tested
/// against a field the runtime never emits.
pub fn plan(steps: &[&str]) -> Value {
    json!({"event": "plan_created", "data": {"steps": steps}})
}

/// A plan rewritten after a reviewer asked for a change (PRD §11).
pub fn plan_revised(revision: u64, reason: &str, steps: &[&str]) -> Value {
    json!({"event": "plan_revised", "data": {"revision": revision, "reason": reason, "steps": steps}})
}

/// The pause a plan-only run records when its plan is waiting to be read.
pub fn plan_ready_for_review(revised: bool) -> Value {
    let reason = if revised {
        format!("revised {}", purrcode_runtime_core::PLAN_REVIEW_PAUSE)
    } else {
        purrcode_runtime_core::PLAN_REVIEW_PAUSE.to_owned()
    };
    json!({"event": "session_paused", "data": {"reason": reason}})
}

/// An assistant message.
pub fn assistant_message(content: &str) -> Value {
    json!({
        "event": "conversation_message_added",
        "data": {"message": {"role": "assistant", "content": content}}
    })
}

/// A write proposal parked on an approval boundary.
///
/// Returns the events plus the digest the workbench must display, so a test can
/// assert the screen shows exactly what the runtime computed.
pub fn write_awaiting_approval(
    action_id: &str,
    path: &str,
    content: &str,
    repository: &str,
    reason: &str,
) -> (Vec<Value>, String) {
    let action = ProposedAction::WriteFile(WriteFileAction {
        path: PathBuf::from(path),
        content: content.to_owned(),
        expected_digest: None,
    });
    let constraints = scoped_constraints(repository, &[path]);
    let digest = action
        .digest(&constraints)
        .expect("digesting a proposed action must not fail");
    let events = vec![
        json!({"event": "action_proposed", "data": {"action_id": action_id, "action": action}}),
        json!({"event": "judgment_recorded", "data": {
            "action_id": action_id,
            "decision": {"decision": "require_approval", "details": {
                "reason": reason,
                "constraints": constraints,
            }}
        }}),
        json!({"event": "approval_requested", "data": {"action_id": action_id}}),
    ];
    (events, digest)
}

/// A read the policy allows without asking, so a test can prove safe reads are
/// not turned into approval prompts.
pub fn allowed_read(action_id: &str, path: &str, repository: &str) -> Vec<Value> {
    let action = json!({"type": "repository_read", "read": {"read_file": {"path": path}}});
    vec![
        json!({"event": "action_proposed", "data": {"action_id": action_id, "action": action}}),
        json!({"event": "judgment_recorded", "data": {
            "action_id": action_id,
            "decision": {"decision": "allow_with_constraints",
                         "details": scoped_constraints(repository, &[])}
        }}),
        json!({"event": "execution_started", "data": {"action_id": action_id}}),
        json!({"event": "execution_finished", "data": {"action_id": action_id, "exit_code": 0}}),
    ]
}

/// A validation outcome. `status` uses the runtime's own snake_case spelling.
pub fn validation(status: &str, evidence: &str) -> Value {
    json!({"event": "validation_recorded", "data": {
        "action_id": "validated-action",
        "status": status,
        "evidence": evidence,
    }})
}

pub fn session_completed() -> Value {
    json!({"event": "session_completed", "data": {}})
}

pub fn session_failed(reason: &str) -> Value {
    json!({"event": "session_failed", "data": {"reason": reason}})
}

pub fn session_cancelled(reason: &str) -> Value {
    json!({"event": "session_cancelled", "data": {"reason": reason}})
}

pub fn recovery_required(reason: &str) -> Value {
    json!({"event": "recovery_required", "data": {"reason": reason}})
}

/// A provider failure the user must be able to understand and retry.
pub fn provider_failure(detail: &str) -> Vec<Value> {
    vec![
        json!({"event": "model_request_started", "data": {"role": "coder", "provider": "fake", "model": "fake:1b"}}),
        json!({"event": "session_failed", "data": {"reason": format!("provider request failed: {detail}")}}),
    ]
}

/// A long answer, for exercising scrolling and wrapping.
pub fn long_answer(paragraphs: usize) -> Value {
    let body = (0..paragraphs)
        .map(|index| {
            format!(
                "Paragraph {index}: this sentence exists to occupy enough width that it must wrap on a standard terminal."
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    assistant_message(&body)
}

/// A patch and its porcelain status, for the review screen.
pub fn diff_fixture() -> (String, String) {
    let patch = "\
diff --git a/src/lib.rs b/src/lib.rs
@@ -1,3 +1,4 @@
 fn existing() {}
+fn added() {}
 fn trailing() {}
diff --git a/src/removed.rs b/src/removed.rs
@@ -1,2 +0,0 @@
-fn gone() {}
"
    .to_owned();
    let porcelain = " M src/lib.rs\n D src/removed.rs\n".to_owned();
    (patch, porcelain)
}

#[cfg(test)]
mod tests {
    use super::*;
    use purrcode_tui::activity;
    use purrcode_tui::approval::ApprovalRequest;

    #[test]
    fn a_write_proposal_produces_a_pending_boundary_with_the_runtime_digest() {
        let (events, digest) = write_awaiting_approval(
            "action-1",
            "src/lib.rs",
            "fn added() {}",
            "/repo",
            "writes a file the plan did not declare",
        );
        let request = ApprovalRequest::from_events(&events)
            .expect("the fixture must produce a pending approval");
        assert_eq!(request.action_id, "action-1");
        assert_eq!(
            request.digest, digest,
            "the fixture's digest must match what the workbench derives"
        );
        assert_eq!(request.operation, "write src/lib.rs");
        assert_eq!(request.affected_paths, vec!["src/lib.rs"]);
    }

    #[test]
    fn an_allowed_read_never_produces_an_approval_boundary() {
        let events = allowed_read("read-1", "src/lib.rs", "/repo");
        assert!(
            ApprovalRequest::from_events(&events).is_none(),
            "a policy-allowed read must not prompt the user"
        );
    }

    #[test]
    fn startup_events_derive_a_context_prepared_stage() {
        let progress = activity::derive(&startup("Add a test", "/repo"));
        assert_eq!(progress.objective, "Add a test");
        assert!(
            progress
                .activity
                .iter()
                .any(|entry| entry.label == "Context prepared")
        );
    }

    #[test]
    fn every_validation_status_maps_to_the_runtime_spelling() {
        for (status, expected) in [
            ("passed", activity::ValidationState::Passed),
            ("failed", activity::ValidationState::Failed),
            ("unavailable", activity::ValidationState::Unavailable),
            ("timed_out", activity::ValidationState::TimedOut),
            ("uncertain", activity::ValidationState::Uncertain),
        ] {
            let progress = activity::derive(&[validation(status, "evidence")]);
            assert_eq!(progress.validation.state, expected, "status {status}");
        }
    }

    #[test]
    fn a_provider_failure_ends_the_session_with_an_explanation() {
        let progress = activity::derive(&provider_failure("connection refused"));
        assert_eq!(progress.session_state, activity::SessionState::Failed);
        assert!(progress.activity.iter().any(|entry| {
            entry
                .detail
                .as_deref()
                .is_some_and(|detail| detail.contains("connection refused"))
        }));
    }

    #[test]
    fn the_diff_fixture_parses_into_reviewable_files() {
        let (patch, porcelain) = diff_fixture();
        let review = purrcode_tui::review::ReviewState::from_daemon(&patch, &porcelain);
        assert_eq!(review.files.len(), 2);
        assert_eq!(review.total_hunks(), 2);
        assert_eq!(review.files[0].path, "src/lib.rs");
    }

    #[test]
    fn a_long_answer_wraps_across_many_lines() {
        let message = long_answer(6);
        let content = message
            .pointer("/data/message/content")
            .and_then(Value::as_str)
            .expect("content");
        assert!(content.lines().count() >= 6);
        assert!(content.contains("Paragraph 5"));
    }

    #[test]
    fn scoped_constraints_never_grant_network_access() {
        let constraints = scoped_constraints("/repo", &["src/**"]);
        assert!(!constraints.network);
        assert_eq!(constraints.allowed_write_globs, vec!["src/**"]);
    }
}
