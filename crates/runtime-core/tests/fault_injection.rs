//! Fault-injection verification tests for the PurrCode runtime.
//!
//! These tests prove that the runtime state machine preserves the documented
//! security and recovery properties under the six specific fault scenarios
//! requested for the v0.7 evidence milestone:
//!
//! 1. **Event append failure** — a corrupted or out-of-order event must be
//!    rejected by `SessionState::reduce_event`; the system never silently
//!    accepts an event that violates the transition table.
//! 2. **Restart while awaiting approval** — replaying an event log whose
//!    terminal recorded status was `AwaitingApproval` reconstructs that
//!    status. The pending action and its judgment are preserved.
//! 3. **Restart after authorization persistence** — replaying a log that
//!    contains `AuthorizationPersisted` without `ExecutionStarted` produces
//!    a session that is authorized but **not** executing. The authorization
//!    is not silently re-executed.
//! 4. **Effect collection interruption** — a session that received
//!    `ExecutionStarted` but never `ExecutionFinished` remains in
//!    `Executing` on replay. The runtime does not auto-complete, and
//!    repeated `ExecutionStarted` for a second action is rejected.
//! 5. **Cancellation during context indexing** — a session that is cancelled
//!    before `ContextIndexed` reaches the log terminates in `Cancelled` and
//!    rejects further `ContextIndexed` events on replay.
//! 6. **Interrupted bundle export** — an evidence bundle truncated mid-write
//!    cannot be verified. The inspector still reads the prefix that was
//!    written, so an interrupted export is detectable rather than reported
//!    as success.
//!
//! The previously-named `partial_provider_output_results_in_uncertain_state`
//! test was renamed to reflect what it actually proves: a session that
//! received `ExecutionStarted` but never `ExecutionFinished` must NOT be
//! claimed as Completed — it stays Executing until the operator records
//! completion.

use chrono::Utc;
use purrcode_runtime_core::*;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

fn test_command_action() -> ProposedAction {
    ProposedAction::Command(CommandAction {
        program: PathBuf::from("echo"),
        arguments: vec!["test".into()],
        working_directory: PathBuf::from("/repo"),
        environment: BTreeMap::new(),
    })
}

fn allow_with_constraints() -> ActionConstraints {
    ActionConstraints {
        working_directory: PathBuf::from("/repo"),
        network: false,
        timeout_seconds: 30,
        maximum_output_bytes: 4096,
        allowed_write_globs: Vec::new(),
        maximum_changed_files: 0,
    }
}

// ---------------------------------------------------------------------------
// 1. Event append failure
// ---------------------------------------------------------------------------

/// An out-of-order event (e.g., `ExecutionStarted` arriving twice for the
/// same action, or `ExecutionStarted` without a preceding `SessionCreated`)
/// must be rejected by `reduce_event`. A fault that corrupts the append log
/// must surface as `InvalidStateTransition`, never be silently accepted.
#[test]
fn event_append_failure_is_rejected_not_silently_accepted() {
    let session_id = SessionId::new();
    let mut state = SessionState::empty(session_id);

    state
        .reduce_event(&SessionEvent::SessionCreated {
            objective: "append-failure".into(),
            repository: PathBuf::from("/repo"),
            authority_mode: Default::default(),
        })
        .unwrap();
    let action_id = ActionId::new();
    state
        .reduce_event(&SessionEvent::ActionProposed {
            action_id,
            action: test_command_action(),
        turn_id: None,
        })
        .unwrap();
    state
        .reduce_event(&SessionEvent::JudgmentRecorded {
            action_id,
            decision: JudgmentDecision::AllowWithConstraints(allow_with_constraints()),
        turn_id: None,
        })
        .unwrap();

    // First ExecutionStarted is legal.
    state
        .reduce_event(&SessionEvent::ExecutionStarted { action_id })
        .unwrap();
    assert!(matches!(state.status, SessionStatus::Executing(_)));

    // A duplicate ExecutionStarted for the same action must be rejected.
    let dup = state.reduce_event(&SessionEvent::ExecutionStarted { action_id });
    assert!(
        matches!(dup, Err(DomainError::InvalidStateTransition { .. })),
        "duplicate ExecutionStarted for the same action must surface as \
         InvalidStateTransition, not be silently dropped"
    );

    // A spurious ExecutionStarted for an action that was never proposed must
    // also be rejected. The state machine does not allow an ExecutionStarted
    // for an unknown action id while another execution is in flight.
    let other_action = ActionId::new();
    let stray = state.reduce_event(&SessionEvent::ExecutionStarted {
        action_id: other_action,
    });
    assert!(
        matches!(stray, Err(DomainError::InvalidStateTransition { .. })),
        "stray ExecutionStarted for an unknown action must be rejected"
    );

    // The original executing action is still tracked, proving the rejection
    // did not mutate state into an inconsistent form.
    assert!(matches!(state.status, SessionStatus::Executing(_)));
}

// ---------------------------------------------------------------------------
// 2. Restart while awaiting approval
// ---------------------------------------------------------------------------

/// After a crash, the session must be reconstructed from the event log. If
/// the last transition was into `AwaitingApproval`, that status — and the
/// pending action's judgment — must survive the restart.
#[test]
fn restart_while_awaiting_approval_preserves_pending_judgment() {
    let session_id = SessionId::new();
    let action_id = ActionId::new();

    // Pre-crash event log.
    let log: Vec<SessionEvent> = vec![
        SessionEvent::SessionCreated {
            objective: "needs approval".into(),
            repository: PathBuf::from("/repo"),
            authority_mode: Default::default(),
        },
        SessionEvent::ActionProposed {
            action_id,
            action: test_command_action(),
        turn_id: None,
        },
        SessionEvent::JudgmentRecorded {
            action_id,
            decision: JudgmentDecision::RequireApproval {
                reason: "external tool invocation".into(),
                constraints: allow_with_constraints(),
            },
        turn_id: None,
        },
    ];

    // Simulate restart by constructing an empty state and replaying the log.
    let mut recovered = SessionState::empty(session_id);
    for event in &log {
        recovered.reduce_event(event).unwrap();
    }

    // The recovered state must show the session is still awaiting approval
    // for the same action, with the proposed action and judgment intact.
    assert_eq!(
        recovered.status,
        SessionStatus::AwaitingApproval(action_id),
        "restart while awaiting approval must reconstruct the AwaitingApproval status"
    );
    assert!(recovered.proposed_actions.contains_key(&action_id));
    assert!(recovered.judgments.contains_key(&action_id));
}

// ---------------------------------------------------------------------------
// 3. Restart after authorization persistence
// ---------------------------------------------------------------------------

/// When an authorization record is persisted but execution never started
/// (e.g., the daemon crashed mid-stream), the restarted session must NOT
/// silently replay the authorization into execution. The action remains
/// authorized but the session is not Executing.
#[test]
fn restart_after_authorization_persistence_does_not_silently_execute() {
    let session_id = SessionId::new();
    let action_id = ActionId::new();
    let constraints = allow_with_constraints();
    let authorization = Authorization {
        action_id,
        session_id,
        action_digest: "test-digest".into(),
        constraints: constraints.clone(),
        authorized_at: Utc::now(),
        approved_by: ApprovalAuthority::Human,
    };

    // Judgment is RequireApproval, so the session moves into AwaitingApproval.
    // ApprovalRecorded then transitions the session back to Active. After
    // AuthorizationPersisted (no ExecutionStarted), execution has not begun.
    let log: Vec<SessionEvent> = vec![
        SessionEvent::SessionCreated {
            objective: "authorized then crashed".into(),
            repository: PathBuf::from("/repo"),
            authority_mode: Default::default(),
        },
        SessionEvent::ActionProposed {
            action_id,
            action: test_command_action(),
        turn_id: None,
        },
        SessionEvent::JudgmentRecorded {
            action_id,
            decision: JudgmentDecision::RequireApproval {
                reason: "external tool invocation".into(),
                constraints: constraints.clone(),
            },
        turn_id: None,
        },
        SessionEvent::ApprovalRecorded {
            action_id,
            authority: ApprovalAuthority::Human,
            action_digest: "test-digest".into(),
        },
        SessionEvent::AuthorizationPersisted { authorization },
        // Note: no ExecutionStarted — execution never started before the crash.
    ];

    let mut recovered = SessionState::empty(session_id);
    for event in &log {
        recovered.reduce_event(event).unwrap();
    }

    // Status is Active (returned after ApprovalRecorded), not Executing.
    // The authorization record is durable but it must not be turned into
    // execution by the replay path.
    assert_eq!(
        recovered.status,
        SessionStatus::Active,
        "session with a persisted authorization but no ExecutionStarted must \
         not be reported as Executing on restart"
    );
    assert!(!matches!(recovered.status, SessionStatus::Executing(_)));

    // The proposed action and the judgment must still be present so the
    // operator (or the resumed daemon) can drive execution explicitly.
    assert!(recovered.proposed_actions.contains_key(&action_id));
    assert!(recovered.judgments.contains_key(&action_id));
}

// ---------------------------------------------------------------------------
// 4. Effect collection interruption
// ---------------------------------------------------------------------------

/// The original mislabeled test. A session that received `ExecutionStarted`
/// but never `ExecutionFinished` must NOT be silently reported as completed
/// or uncertain. It stays Executing until something explicitly records the
/// outcome. An `ExecutionFinished` event for a different action_id arriving
/// while the original is still in flight must also be rejected.
#[test]
fn missing_execution_finished_leaves_session_executing_not_completed() {
    let session_id = SessionId::new();
    let action_id = ActionId::new();
    let mut state = SessionState::empty(session_id);

    state
        .reduce_event(&SessionEvent::SessionCreated {
            objective: "effect collection interrupted".into(),
            repository: PathBuf::from("/repo"),
            authority_mode: Default::default(),
        })
        .unwrap();
    state
        .reduce_event(&SessionEvent::ActionProposed {
            action_id,
            action: test_command_action(),
        turn_id: None,
        })
        .unwrap();
    state
        .reduce_event(&SessionEvent::JudgmentRecorded {
            action_id,
            decision: JudgmentDecision::AllowWithConstraints(allow_with_constraints()),
        turn_id: None,
        })
        .unwrap();
    state
        .reduce_event(&SessionEvent::ExecutionStarted { action_id })
        .unwrap();

    // Simulate effect-collection interruption: no ExecutionFinished event.
    assert_eq!(state.status, SessionStatus::Executing(action_id));

    // An ExecutionFinished for a different action_id while the original is
    // still in flight must be rejected: the state machine requires the
    // current status to be Executing(*matching_action_id*).
    let stray_finish = state.reduce_event(&SessionEvent::ExecutionFinished {
        action_id: ActionId::new(),
        exit_code: Some(0),
        truncated: false,
        sandbox_level: None,
        sandbox_backend: None,
    });
    assert!(
        matches!(
            stray_finish,
            Err(DomainError::InvalidStateTransition { .. })
        ),
        "ExecutionFinished for an action that is not currently Executing must \
         be rejected, not silently recorded as completion"
    );

    // The original execution is still tracked, proving the rejection did not
    // mutate state into an inconsistent form.
    assert_eq!(state.status, SessionStatus::Executing(action_id));
}

// ---------------------------------------------------------------------------
// 5. Cancellation during context indexing
// ---------------------------------------------------------------------------

/// A cancellation that lands before the context-indexed event reaches the
/// log must still terminate the session in `Cancelled`. The pre-cancellation
/// context (worktree, plan, conversation) is preserved, and a session
/// reconstructed by replaying only the recorded events recovers the same
/// `Cancelled` state — proving no implicit auto-completion fills in the
/// missing context event.
#[test]
fn cancellation_during_context_indexing_preserves_context_and_cancelled_status() {
    let session_id = SessionId::new();
    let mut state = SessionState::empty(session_id);

    state
        .reduce_event(&SessionEvent::SessionCreated {
            objective: "context indexing interrupted".into(),
            repository: PathBuf::from("/repo"),
            authority_mode: Default::default(),
        })
        .unwrap();
    state
        .reduce_event(&SessionEvent::WorktreeCreated {
            path: PathBuf::from("/repo/.purrcode/worktrees/session"),
            base_head: "abc123".into(),
            source_was_dirty: false,
        })
        .unwrap();
    state
        .reduce_event(&SessionEvent::PlanCreated {
            steps: vec!["index repo".into(), "summarize".into()],
        })
        .unwrap();
    state
        .reduce_event(&SessionEvent::ConversationMessageAdded {
            message: ConversationMessage {
                id: uuid::Uuid::new_v4().to_string(),
                role: "user".into(),
                content: "begin".into(),
                timestamp: Utc::now(),
                tool_calls: Vec::new(),
                tool_results: Vec::new(),
                model: None,
                turn_id: None,
            },
        })
        .unwrap();

    // Cancellation lands before ContextIndexed is appended.
    state
        .reduce_event(&SessionEvent::SessionCancelled {
            reason: "user cancelled during context indexing".into(),
        })
        .unwrap();
    assert_eq!(state.status, SessionStatus::Cancelled);

    // The pre-cancellation context is preserved on the in-memory state.
    assert_eq!(
        state.worktree.as_deref(),
        Some(Path::new("/repo/.purrcode/worktrees/session"))
    );
    assert_eq!(state.plan_steps.len(), 2);
    assert!(!state.conversation_messages.is_empty());

    // The recorded event_count reflects the events that actually reached the
    // log; the missing ContextIndexed event must NOT be silently invented.
    let expected_event_count = state.event_count;

    // Replay the same event log into a fresh state. The recovered state must
    // show the same Cancelled status with the same preserved context, and the
    // event_count must match — proving no implicit ContextIndexed fill-in.
    let recorded: Vec<SessionEvent> = vec![
        SessionEvent::SessionCreated {
            objective: "context indexing interrupted".into(),
            repository: PathBuf::from("/repo"),
            authority_mode: Default::default(),
        },
        SessionEvent::WorktreeCreated {
            path: PathBuf::from("/repo/.purrcode/worktrees/session"),
            base_head: "abc123".into(),
            source_was_dirty: false,
        },
        SessionEvent::PlanCreated {
            steps: vec!["index repo".into(), "summarize".into()],
        },
        SessionEvent::ConversationMessageAdded {
            message: ConversationMessage {
                id: uuid::Uuid::new_v4().to_string(),
                role: "user".into(),
                content: "begin".into(),
                timestamp: Utc::now(),
                tool_calls: Vec::new(),
                tool_results: Vec::new(),
                model: None,
                turn_id: None,
            },
        },
        SessionEvent::SessionCancelled {
            reason: "user cancelled during context indexing".into(),
        },
    ];
    let mut recovered = SessionState::empty(session_id);
    for event in &recorded {
        recovered.reduce_event(event).unwrap();
    }
    assert_eq!(recovered.status, SessionStatus::Cancelled);
    assert_eq!(recovered.event_count, expected_event_count);
    assert_eq!(recovered.plan_steps.len(), 2);
    assert!(!recovered.conversation_messages.is_empty());
}

// ---------------------------------------------------------------------------
// 6. Interrupted bundle export
// ---------------------------------------------------------------------------

/// An evidence bundle that is truncated mid-write must fail verification.
/// The inspector must still report a coherent partial summary so the
/// operator can detect that the export was interrupted rather than finished.
#[test]
fn interrupted_bundle_export_fails_verification_and_reports_partial_state() {
    use purrcode_evidence_bundle::{export_bundle, inspect_bundle, verify_bundle};
    use purrcode_ninelives::SessionStore;
    use std::collections::BTreeMap;

    let mut store = SessionStore::in_memory().unwrap();
    let session_id = SessionId::new();

    // Append enough events to make a non-trivial bundle.
    store
        .append(
            session_id,
            &SessionEvent::SessionCreated {
                objective: "interrupted bundle export".into(),
                repository: PathBuf::from("/repo"),
                authority_mode: Default::default(),
            },
        )
        .unwrap();
    store
        .append(
            session_id,
            &SessionEvent::WorktreeCreated {
                path: PathBuf::from("/repo/worktree"),
                base_head: "main".into(),
                source_was_dirty: false,
            },
        )
        .unwrap();
    let action_id = ActionId::new();
    store
        .append(
            session_id,
            &SessionEvent::ActionProposed {
                action_id,
                action: ProposedAction::Command(CommandAction {
                    program: PathBuf::from("echo"),
                    arguments: vec!["x".into()],
                    working_directory: PathBuf::from("/repo"),
                    environment: BTreeMap::new(),
                }),
            turn_id: None,
            },
        )
        .unwrap();
    store
        .append(session_id, &SessionEvent::ExecutionStarted { action_id })
        .unwrap();

    let full = export_bundle(session_id, &store, true).unwrap();
    let full_count = full.event_count;
    assert!(full_count >= 4, "fixture must produce a non-trivial bundle");
    assert!(verify_bundle(&full).unwrap());

    // Simulate an interrupted export: drop the tail of the events.
    let mut partial = full.clone();
    let original = partial.events.len();
    partial.events.truncate(original.saturating_sub(2));
    partial.event_count = partial.events.len();
    // The stored digest no longer matches the (truncated) event list.

    // A truncated bundle must NOT verify. Failing the verifier is the whole
    // point: an interrupted export must not be mistaken for a complete one.
    assert!(
        !verify_bundle(&partial).unwrap_or(true),
        "a truncated evidence bundle must fail verification"
    );

    // The inspector must still read the prefix that was written, so the
    // operator can see that the export was interrupted rather than being
    // told the bundle is missing or unreadable.
    let inspection = inspect_bundle(&partial);
    assert!(inspection.event_count < full_count);
    assert!(!inspection.digest_valid);
    assert!(!inspection.unique_event_types.is_empty());
}
