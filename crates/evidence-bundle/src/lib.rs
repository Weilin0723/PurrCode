use chrono::{DateTime, Utc};
use purrcode_ninelives::{SessionStore, StoreError};
use purrcode_runtime_core::{ActionId, DomainError, SessionEvent, SessionId, SessionState};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use thiserror::Error;

pub const SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EvidenceBundle {
    pub schema_version: u32,
    pub bundle_id: String,
    pub session_id: String,
    pub export_timestamp: DateTime<Utc>,
    pub event_count: usize,
    pub events: Vec<BundleEvent>,
    pub bundle_digest: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BundleEvent {
    pub sequence: u64,
    pub event_type: String,
    pub action_id: Option<String>,
    pub timestamp: DateTime<Utc>,
    pub evidence: serde_json::Value,
    /// Field paths inside `evidence` that were redacted in this bundle event.
    /// Empty when the bundle was exported with `include_sensitive = true` or
    /// when no sensitive fields applied to this event type. Used by
    /// `restore_event` to surface honest "redacted; cannot reconstruct"
    /// errors rather than fabricating substitute values.
    #[serde(default)]
    pub redacted_fields: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BundleInspection {
    pub bundle_id: String,
    pub session_id: String,
    pub export_timestamp: DateTime<Utc>,
    pub event_count: usize,
    pub unique_event_types: BTreeSet<String>,
    pub digest_valid: bool,
    pub size_bytes: usize,
}

#[derive(Debug, Error)]
pub enum BundleError {
    #[error("store error: {0}")]
    Store(#[from] StoreError),
    #[error("domain error: {0}")]
    Domain(#[from] DomainError),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("digest mismatch: stored {stored}, computed {computed}")]
    DigestMismatch { stored: String, computed: String },
    #[error("unsupported schema version: {0}")]
    UnsupportedVersion(u32),
    #[error("session not found: {0}")]
    SessionNotFound(String),
    #[error("invalid bundle: {0}")]
    InvalidBundle(String),
    #[error(
        "event {event_type} at sequence {sequence} was redacted in fields {redacted_fields:?}; \
         cannot faithfully replay without fabricating substitute values"
    )]
    Redacted {
        event_type: String,
        sequence: u64,
        redacted_fields: Vec<String>,
    },
}

// ── Exporter ──────────────────────────────────────────────────────

pub fn export_bundle(
    session_id: SessionId,
    store: &SessionStore,
    include_sensitive: bool,
) -> Result<EvidenceBundle, BundleError> {
    let timestamped = store.timestamped_events(session_id)?;
    if timestamped.is_empty() {
        return Err(BundleError::SessionNotFound(session_id.0.to_string()));
    }

    let bundle_id = uuid::Uuid::new_v4().to_string();
    let export_timestamp = Utc::now();
    let session_id_str = session_id.0.to_string();

    let bundle_events: Vec<BundleEvent> = timestamped
        .into_iter()
        .enumerate()
        .filter_map(|(idx, (ts, event))| {
            let sequence = (idx + 1) as u64;
            build_bundle_event(sequence, &event, ts, include_sensitive)
        })
        .collect();

    let event_count = bundle_events.len();
    let bundle_digest = compute_digest(&bundle_events);

    Ok(EvidenceBundle {
        schema_version: SCHEMA_VERSION,
        bundle_id,
        session_id: session_id_str,
        export_timestamp,
        event_count,
        events: bundle_events,
        bundle_digest,
    })
}

// Fields redacted in a non-sensitive export per event type. The path is a
// JSON pointer-like list of segment keys to null out in the serialized event
// before it is stored in the bundle. The first segment is always `data`
// because the runtime serializes `SessionEvent` with
// `#[serde(tag = "event", content = "data")]`.
//
// Documented here so the redaction policy is auditable from one place.
fn redacted_paths_for(event_type: &str) -> &'static [&'static [&'static str]] {
    match event_type {
        "conversation_message_added" => &[&["data", "message", "content"]],
        // `environment` is handled separately below: its object shape is
        // preserved while every value is replaced with the redaction marker.
        "action_proposed" => &[&["data", "action", "working_directory"]],
        "action_output_recorded" => &[&["data", "stdout"], &["data", "stderr"]],
        "research_search_performed" => {
            &[&["data", "query"], &["data", "url"], &["data", "excerpt"]]
        }
        "session_created" => &[&["data", "repository"]],
        "worktree_created" => &[&["data", "path"]],
        "checkpoint_created" => &[&["data", "patch_digest"]],
        _ => &[],
    }
}

const REDACTED_MARKER: &str = "<redacted>";

fn redact_json(value: &mut serde_json::Value, path: &[&str]) -> bool {
    let mut current = value;
    let last = path.len().saturating_sub(1);
    for (i, segment) in path.iter().enumerate() {
        match current {
            serde_json::Value::Object(map) => {
                if i == last {
                    if map.contains_key(*segment) {
                        map.insert(
                            (*segment).to_string(),
                            serde_json::Value::String(REDACTED_MARKER.to_string()),
                        );
                        return true;
                    }
                    return false;
                }
                match map.get_mut(*segment) {
                    Some(next) => current = next,
                    None => return false,
                }
            }
            _ => return false,
        }
    }
    false
}

fn redact_object_values(value: &mut serde_json::Value, path: &[&str]) -> bool {
    let mut current = value;
    for segment in path {
        match current {
            serde_json::Value::Object(map) => match map.get_mut(*segment) {
                Some(next) => current = next,
                None => return false,
            },
            _ => return false,
        }
    }
    let serde_json::Value::Object(map) = current else {
        return false;
    };
    for value in map.values_mut() {
        *value = serde_json::Value::String(REDACTED_MARKER.to_string());
    }
    true
}

fn build_bundle_event(
    sequence: u64,
    event: &SessionEvent,
    timestamp: DateTime<Utc>,
    include_sensitive: bool,
) -> Option<BundleEvent> {
    let event_type = event_type_name(event).to_string();
    let action_id = extract_action_id(event).map(|id| id.0.to_string());

    if include_sensitive {
        let evidence = serde_json::to_value(event).ok()?;
        return Some(BundleEvent {
            sequence,
            event_type,
            action_id,
            timestamp,
            evidence,
            redacted_fields: Vec::new(),
        });
    }

    // Authoritative export: serialize the full event as JSON, then null out
    // a documented set of sensitive fields. The event structure is preserved
    // so `restore_event` can deserialize it back via `serde_json::from_value`
    // without fabricating substitute values.
    let mut evidence = match serde_json::to_value(event) {
        Ok(value) => value,
        Err(_) => return None,
    };
    let mut redacted_fields = Vec::new();
    for path in redacted_paths_for(&event_type) {
        if redact_json(&mut evidence, path) {
            redacted_fields.push(path.join("."));
        }
    }
    if event_type == "action_proposed" {
        let environment_path = ["data", "action", "environment"];
        if redact_object_values(&mut evidence, &environment_path) {
            redacted_fields.push(environment_path.join("."));
        }
    }

    Some(BundleEvent {
        sequence,
        event_type,
        action_id,
        timestamp,
        evidence,
        redacted_fields,
    })
}

// ── Verifier ──────────────────────────────────────────────────────

pub fn verify_bundle(bundle: &EvidenceBundle) -> Result<bool, BundleError> {
    if bundle.schema_version != SCHEMA_VERSION {
        return Err(BundleError::UnsupportedVersion(bundle.schema_version));
    }
    let recomputed = compute_digest(&bundle.events);
    Ok(bundle.bundle_digest == recomputed)
}

// ── Inspector ─────────────────────────────────────────────────────

pub fn inspect_bundle(bundle: &EvidenceBundle) -> BundleInspection {
    let unique_event_types: BTreeSet<String> =
        bundle.events.iter().map(|e| e.event_type.clone()).collect();

    let serialized = serde_json::to_vec(bundle).unwrap_or_default();
    let size_bytes = serialized.len();

    let digest_valid = compute_digest(&bundle.events) == bundle.bundle_digest;

    BundleInspection {
        bundle_id: bundle.bundle_id.clone(),
        session_id: bundle.session_id.clone(),
        export_timestamp: bundle.export_timestamp,
        event_count: bundle.event_count,
        unique_event_types,
        digest_valid,
        size_bytes,
    }
}

// ── Replayer ─────────────────────────────────────────────────────

pub fn replay_bundle(
    bundle: &EvidenceBundle,
    _store: &SessionStore,
) -> Result<SessionState, BundleError> {
    if bundle.schema_version != SCHEMA_VERSION {
        return Err(BundleError::UnsupportedVersion(bundle.schema_version));
    }

    let session_id_json = serde_json::Value::String(bundle.session_id.clone());
    let session_id: SessionId = serde_json::from_value(session_id_json)
        .map_err(|e| BundleError::InvalidBundle(format!("invalid session_id: {e}")))?;

    let mut state = SessionState::empty(session_id);

    for bundle_event in &bundle.events {
        let event = restore_event(bundle_event)?;
        state.reduce_event(&event)?;
    }

    Ok(state)
}

/// Restore a `SessionEvent` from a `BundleEvent`.
///
/// The default export now preserves the authoritative event as JSON in
/// `bundle_event.evidence`, with sensitive fields only redacted via a
/// field-level marker. Replay therefore deserializes the original event
/// directly via `serde_json::from_value`. No substitute or synthetic event
/// is ever fabricated: if the event was redacted beyond the point where the
/// `SessionEvent` shape can be reconstructed, replay returns an explicit
/// `BundleError::Redacted` error so the operator knows the bundle cannot be
/// faithfully replayed. This is the only honest answer for a redacted bundle.
fn restore_event(bundle_event: &BundleEvent) -> Result<SessionEvent, BundleError> {
    match serde_json::from_value::<SessionEvent>(bundle_event.evidence.clone()) {
        Ok(event) => Ok(event),
        Err(error) => {
            if bundle_event.redacted_fields.is_empty() {
                Err(BundleError::InvalidBundle(format!(
                    "event {} at sequence {} is not a valid SessionEvent: {error}",
                    bundle_event.event_type, bundle_event.sequence
                )))
            } else {
                Err(BundleError::Redacted {
                    event_type: bundle_event.event_type.clone(),
                    sequence: bundle_event.sequence,
                    redacted_fields: bundle_event.redacted_fields.clone(),
                })
            }
        }
    }
}

// ── Helpers ───────────────────────────────────────────────────────

fn compute_digest(events: &[BundleEvent]) -> String {
    let serialized = serde_json::to_vec(events).unwrap_or_default();
    let hash = blake3::hash(&serialized);
    hex::encode(hash.as_bytes())
}

fn event_type_name(event: &SessionEvent) -> &'static str {
    match event {
        SessionEvent::SessionCreated { .. } => "session_created",
        SessionEvent::ConversationMessageAdded { .. } => "conversation_message_added",
        SessionEvent::WorktreeCreated { .. } => "worktree_created",
        SessionEvent::SubmodulesPrepared { .. } => "submodules_prepared",
        SessionEvent::PlanCreated { .. } => "plan_created",
        SessionEvent::PlanRevised { .. } => "plan_revised",
        SessionEvent::ContextCompacted { .. } => "context_compacted",
        SessionEvent::SessionPaused { .. } => "session_paused",
        SessionEvent::SessionResumed => "session_resumed",
        SessionEvent::ModelSelected { .. } => "model_selected",
        SessionEvent::SupervisorStarted { .. } => "supervisor_started",
        SessionEvent::WorkerFinished { .. } => "worker_finished",
        SessionEvent::SupervisorReviewRequired { .. } => "supervisor_review_required",
        SessionEvent::ContextIndexed { .. } => "context_indexed",
        SessionEvent::ModelRequestStarted { .. } => "model_request_started",
        SessionEvent::ModelRequestFinished { .. } => "model_request_finished",
        SessionEvent::ActionProposed { .. } => "action_proposed",
        SessionEvent::ActionSuperseded { .. } => "action_superseded",
        SessionEvent::JudgmentRecorded { .. } => "judgment_recorded",
        SessionEvent::ContextualJudgmentRecorded { .. } => "contextual_judgment_recorded",
        SessionEvent::OutcomeJudgmentRecorded { .. } => "outcome_judgment_recorded",
        SessionEvent::OutcomeReviewRequired { .. } => "outcome_review_required",
        SessionEvent::OutcomeReviewApproved { .. } => "outcome_review_approved",
        SessionEvent::ApprovalRecorded { .. } => "approval_recorded",
        SessionEvent::ApprovalRejected { .. } => "approval_rejected",
        SessionEvent::AuthorizationPersisted { .. } => "authorization_persisted",
        SessionEvent::ExecutionStarted { .. } => "execution_started",
        SessionEvent::ExecutionFinished { .. } => "execution_finished",
        SessionEvent::ActionOutputRecorded { .. } => "action_output_recorded",
        SessionEvent::ValidationRecorded { .. } => "validation_recorded",
        SessionEvent::CheckpointCreated { .. } => "checkpoint_created",
        SessionEvent::WorktreeDispositionRecorded { .. } => "worktree_disposition_recorded",
        SessionEvent::SessionCancelled { .. } => "session_cancelled",
        SessionEvent::RecoveryRequired { .. } => "recovery_required",
        SessionEvent::SessionCompleted => "session_completed",
        SessionEvent::SessionFailed { .. } => "session_failed",
        SessionEvent::CapabilityGapDetected { .. } => "capability_gap_detected",
        SessionEvent::SkillSearchStarted { .. } => "skill_search_started",
        SessionEvent::SkillCandidateDiscovered { .. } => "skill_candidate_discovered",
        SessionEvent::SkillCandidateRanked { .. } => "skill_candidate_ranked",
        SessionEvent::SkillInspectionOpened { .. } => "skill_inspection_opened",
        SessionEvent::SkillInstallApproved { .. } => "skill_install_approved",
        SessionEvent::SkillInstallRejected { .. } => "skill_install_rejected",
        SessionEvent::SkillQualified { .. } => "skill_qualified",
        SessionEvent::SkillQualificationStarted { .. } => "skill_qualification_started",
        SessionEvent::SkillQualificationFailed { .. } => "skill_qualification_failed",
        SessionEvent::SkillInvoked { .. } => "skill_invoked",
        SessionEvent::SkillInvocationSucceeded { .. } => "skill_invocation_succeeded",
        SessionEvent::SkillInvocationFailed { .. } => "skill_invocation_failed",
        SessionEvent::InstalledSkillReused { .. } => "installed_skill_reused",
        SessionEvent::InstalledSkillMatched { .. } => "installed_skill_matched",
        SessionEvent::ExternalSearchAvoided { .. } => "external_search_avoided",
        SessionEvent::SkillUpdated { .. } => "skill_updated",
        SessionEvent::SkillRemoved { .. } => "skill_removed",
        SessionEvent::ResearchSearchPerformed { .. } => "research_search_performed",
    }
}

fn extract_action_id(event: &SessionEvent) -> Option<ActionId> {
    match event {
        SessionEvent::ActionProposed { action_id, .. } => Some(*action_id),
        SessionEvent::ActionSuperseded { .. } => None,
        SessionEvent::JudgmentRecorded { action_id, .. } => Some(*action_id),
        SessionEvent::ContextualJudgmentRecorded { action_id, .. } => Some(*action_id),
        SessionEvent::ApprovalRecorded { action_id, .. } => Some(*action_id),
        SessionEvent::ApprovalRejected { action_id, .. } => Some(*action_id),
        SessionEvent::ExecutionStarted { action_id } => Some(*action_id),
        SessionEvent::ExecutionFinished { action_id, .. } => Some(*action_id),
        SessionEvent::ActionOutputRecorded { action_id, .. } => Some(*action_id),
        SessionEvent::ValidationRecorded { action_id, .. } => Some(*action_id),
        SessionEvent::AuthorizationPersisted { authorization } => Some(authorization.action_id),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use purrcode_runtime_core::{CommandAction, JudgmentDecision, ProposedAction, SessionStatus};
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    fn setup_store() -> (SessionStore, SessionId, ActionId) {
        let mut store = SessionStore::in_memory().unwrap();
        let session_id = SessionId::new();
        let action_id = ActionId::new();
        let repo = PathBuf::from("/test-repo");

        store
            .append(
                session_id,
                &SessionEvent::SessionCreated {
                    objective: "test objective".into(),
                    repository: repo.clone(),
                },
            )
            .unwrap();

        store
            .append(
                session_id,
                &SessionEvent::WorktreeCreated {
                    path: repo.join("worktree"),
                    base_head: "main".into(),
                    source_was_dirty: false,
                },
            )
            .unwrap();

        store
            .append(
                session_id,
                &SessionEvent::ActionProposed {
                    action_id,
                    action: ProposedAction::Command(CommandAction {
                        program: "echo".into(),
                        arguments: vec!["hello".into()],
                        working_directory: repo,
                        environment: BTreeMap::from([(
                            "API_TOKEN".into(),
                            "super-secret-value".into(),
                        )]),
                    }),
                },
            )
            .unwrap();

        store
            .append(
                session_id,
                &SessionEvent::JudgmentRecorded {
                    action_id,
                    decision: JudgmentDecision::Allow,
                },
            )
            .unwrap();

        store
            .append(session_id, &SessionEvent::ExecutionStarted { action_id })
            .unwrap();

        store
            .append(
                session_id,
                &SessionEvent::ExecutionFinished {
                    action_id,
                    exit_code: Some(0),
                    truncated: false,
                    sandbox_level: None,
                    sandbox_backend: None,
                },
            )
            .unwrap();

        store
            .append(session_id, &SessionEvent::SessionCompleted)
            .unwrap();

        (store, session_id, action_id)
    }

    #[test]
    fn export_bundle_creates_valid_bundle() {
        let (store, session_id, _) = setup_store();
        let bundle = export_bundle(session_id, &store, true).unwrap();
        assert_eq!(bundle.schema_version, 1);
        assert_eq!(bundle.session_id, session_id.0.to_string());
        assert_eq!(bundle.event_count, 7);
        assert!(!bundle.bundle_digest.is_empty());
        assert_eq!(bundle.events.len(), 7);
        assert!(bundle.events.iter().all(|e| e.sequence >= 1));
    }

    #[test]
    fn verify_bundle_accepts_valid() {
        let (store, session_id, _) = setup_store();
        let bundle = export_bundle(session_id, &store, true).unwrap();
        assert!(verify_bundle(&bundle).unwrap());
    }

    #[test]
    fn verify_bundle_rejects_tampered_digest() {
        let (store, session_id, _) = setup_store();
        let mut bundle = export_bundle(session_id, &store, true).unwrap();
        bundle.bundle_digest =
            "0000000000000000000000000000000000000000000000000000000000000000".into();
        assert!(!verify_bundle(&bundle).unwrap());
    }

    #[test]
    fn replay_bundle_returns_session_state() {
        let (store, session_id, _) = setup_store();
        let bundle = export_bundle(session_id, &store, true).unwrap();
        let state = replay_bundle(&bundle, &store).unwrap();
        assert_eq!(state.id, session_id);
        assert_eq!(state.event_count, 7);
        assert_eq!(state.status, SessionStatus::Completed);
        assert_eq!(state.objective.as_deref(), Some("test objective"));
    }

    #[test]
    fn non_sensitive_export_preserves_authoritative_events_with_field_redaction() {
        let (store, session_id, _) = setup_store();
        let bundle = export_bundle(session_id, &store, false).unwrap();

        // Authoritative export: ALL events are present. State-bearing events
        // (session_created, session_completed, plan_created, etc.) are NOT
        // dropped. Sensitive FIELDS within them are redacted.
        assert!(bundle
            .events
            .iter()
            .any(|e| e.event_type == "session_created"));
        assert!(bundle
            .events
            .iter()
            .any(|e| e.event_type == "session_completed"));

        // The session_created event must have its `repository` field redacted
        // (per the documented redaction policy for that event type).
        let session_created = bundle
            .events
            .iter()
            .find(|e| e.event_type == "session_created")
            .expect("session_created event must be present");
        assert!(
            session_created
                .redacted_fields
                .iter()
                .any(|f| f == "data.repository"),
            "session_created.repository must be marked redacted, got {:?}",
            session_created.redacted_fields
        );
        assert_eq!(
            session_created.evidence["data"]["repository"],
            serde_json::Value::String("<redacted>".into())
        );

        // The action_proposed event must keep the action shape (so replay can
        // round-trip the original event) while redacting working_directory and
        // every environment value. Keeping the environment as an object lets
        // serde reconstruct the action without exposing credential values.
        let action_proposed = bundle
            .events
            .iter()
            .find(|e| e.event_type == "action_proposed")
            .expect("action_proposed event must be present");
        assert!(action_proposed
            .redacted_fields
            .iter()
            .any(|f| f == "data.action.working_directory"));
        assert!(action_proposed
            .redacted_fields
            .iter()
            .any(|f| f == "data.action.environment"));

        // State-bearing execution events are preserved.
        assert!(bundle
            .events
            .iter()
            .any(|e| e.event_type == "worktree_created"));
        assert!(bundle
            .events
            .iter()
            .any(|e| e.event_type == "execution_started"));
        assert!(bundle
            .events
            .iter()
            .any(|e| e.event_type == "execution_finished"));
    }

    #[test]
    fn replay_bundle_reconstructs_redacted_state() {
        let (store, session_id, action_id) = setup_store();
        let bundle = export_bundle(session_id, &store, false).unwrap();
        // Authoritative replay must reproduce the original session status
        // (Completed) rather than fabricating a substitute. The redacted
        // bundle still round-trips because the default export preserves
        // event structure with field-level redaction only.
        let state = replay_bundle(&bundle, &store).unwrap();
        assert_eq!(state.id, session_id);
        assert!(state.proposed_actions.contains_key(&action_id));
        assert_eq!(state.status, SessionStatus::Completed);
        assert!(state.judgments.contains_key(&action_id));
    }

    #[test]
    fn replay_bundle_does_not_fabricate_substitute_actions() {
        // Build a session whose ActionProposed has a unique working_directory
        // and program. A faithful (non-fabricating) replay must preserve the
        // original values, modulo the documented redacted fields. The old
        // `minimal_action` fallback fabricated an empty `ProposedAction` on
        // replay; that bug must remain gone.
        let mut store = SessionStore::in_memory().unwrap();
        let session_id = SessionId::new();
        let action_id = ActionId::new();
        store
            .append(
                session_id,
                &SessionEvent::SessionCreated {
                    objective: "no fabrication".into(),
                    repository: PathBuf::from("/unique/repo"),
                },
            )
            .unwrap();
        store
            .append(
                session_id,
                &SessionEvent::ActionProposed {
                    action_id,
                    action: ProposedAction::Command(CommandAction {
                        program: PathBuf::from("/usr/bin/unique-binary"),
                        arguments: vec!["--flag".into(), "value".into()],
                        working_directory: PathBuf::from("/unique/workdir"),
                        environment: BTreeMap::from([(
                            "API_TOKEN".into(),
                            "super-secret-value".into(),
                        )]),
                    }),
                },
            )
            .unwrap();
        store
            .append(
                session_id,
                &SessionEvent::JudgmentRecorded {
                    action_id,
                    decision: JudgmentDecision::Allow,
                },
            )
            .unwrap();

        let bundle = export_bundle(session_id, &store, false).unwrap();
        let encoded = serde_json::to_string(&bundle).unwrap();
        assert!(!encoded.contains("super-secret-value"));
        let state = replay_bundle(&bundle, &store).unwrap();
        let restored = state
            .proposed_actions
            .get(&action_id)
            .expect("proposed action must survive replay");
        // Non-redacted fields are preserved verbatim from the original event.
        match restored {
            ProposedAction::Command(cmd) => {
                assert_eq!(cmd.program, PathBuf::from("/usr/bin/unique-binary"));
                assert_eq!(
                    cmd.arguments,
                    vec!["--flag".to_string(), "value".to_string()]
                );
                // Redacted field is replaced with the marker, not fabricated.
                assert_eq!(cmd.working_directory, PathBuf::from("<redacted>"));
                assert_eq!(
                    cmd.environment.get("API_TOKEN").map(String::as_str),
                    Some("<redacted>")
                );
            }
            other => panic!("expected Command action, got {other:?}"),
        }
    }

    #[test]
    fn replay_bundle_reports_redacted_event_when_round_trip_fails() {
        // Construct a BundleEvent with `evidence` that cannot deserialize as a
        // SessionEvent and a non-empty `redacted_fields` list. Replay must
        // surface a `BundleError::Redacted` error rather than silently
        // fabricating a substitute event.
        let bundle_event = BundleEvent {
            sequence: 1,
            event_type: "action_proposed".into(),
            action_id: Some(uuid::Uuid::new_v4().to_string()),
            timestamp: chrono::Utc::now(),
            evidence: serde_json::json!({
                "event": "action_proposed",
                "data": {
                    "action_id": "not-a-valid-uuid",
                    "action": {
                        "command": {
                            "program": "rm",
                            "arguments": ["-rf", "/"],
                            "working_directory": "<redacted>",
                            "environment": {},
                        }
                    }
                }
            }),
            redacted_fields: vec!["data.action.working_directory".into()],
        };
        match restore_event(&bundle_event) {
            Err(BundleError::Redacted { event_type, .. }) => {
                assert_eq!(event_type, "action_proposed");
            }
            other => panic!("expected Redacted error, got {other:?}"),
        }
    }

    #[test]
    fn inspect_bundle_reports_correct_metadata() {
        let (store, session_id, _) = setup_store();
        let bundle = export_bundle(session_id, &store, true).unwrap();
        let inspection = inspect_bundle(&bundle);
        assert_eq!(inspection.bundle_id, bundle.bundle_id);
        assert_eq!(inspection.event_count, 7);
        assert!(inspection.digest_valid);
        assert!(inspection.size_bytes > 0);
        assert!(inspection.unique_event_types.contains("execution_started"));
    }

    #[test]
    fn verify_bundle_rejects_unsupported_version() {
        let (store, session_id, _) = setup_store();
        let mut bundle = export_bundle(session_id, &store, true).unwrap();
        bundle.schema_version = 99;
        match verify_bundle(&bundle) {
            Err(BundleError::UnsupportedVersion(99)) => {}
            _ => panic!("expected UnsupportedVersion error"),
        }
    }

    #[test]
    fn export_bundle_of_empty_session_returns_error() {
        let store = SessionStore::in_memory().unwrap();
        let session_id = SessionId::new();
        match export_bundle(session_id, &store, true) {
            Err(BundleError::SessionNotFound(_)) => {}
            _ => panic!("expected SessionNotFound error"),
        }
    }
}
