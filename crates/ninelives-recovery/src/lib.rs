//! SQLite-backed append-only session log and authorization ledger.

use chrono::{DateTime, Duration as ChronoDuration, Utc};
use purrcode_runtime_core::{ActionId, Authorization, SessionEvent, SessionId, SessionState};
use rusqlite::{params, Connection, DatabaseName, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use thiserror::Error;
use uuid::Uuid;

const MIGRATION_1: &str = include_str!("../../../migrations/0001_initial.sql");
const MIGRATION_2: &str = include_str!("../../../migrations/0002_automations.sql");

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Automation {
    pub id: Uuid,
    pub objective: String,
    pub repository: PathBuf,
    pub interval_seconds: u64,
    pub enabled: bool,
    pub next_run_at: DateTime<Utc>,
    pub last_session_id: Option<SessionId>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

pub struct SessionStore {
    connection: Connection,
}

impl SessionStore {
    pub fn open(path: &Path) -> Result<Self, StoreError> {
        let connection = Connection::open(path)?;
        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.pragma_update(None, "foreign_keys", "ON")?;
        connection.busy_timeout(std::time::Duration::from_secs(5))?;
        let mut store = Self { connection };
        store.migrate()?;
        Ok(store)
    }

    pub fn in_memory() -> Result<Self, StoreError> {
        let connection = Connection::open_in_memory()?;
        connection.pragma_update(None, "foreign_keys", "ON")?;
        let mut store = Self { connection };
        store.migrate()?;
        Ok(store)
    }

    fn migrate(&mut self) -> Result<(), StoreError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        transaction.execute_batch(MIGRATION_1)?;
        transaction.execute_batch(MIGRATION_2)?;
        transaction.commit()?;
        Ok(())
    }

    pub fn append(
        &mut self,
        session_id: SessionId,
        event: &SessionEvent,
    ) -> Result<u64, StoreError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let sequence: u64 = transaction.query_row(
            "SELECT COALESCE(MAX(sequence), 0) + 1 FROM session_events WHERE session_id = ?1",
            [session_id.0.to_string()],
            |row| row.get(0),
        )?;
        transaction.execute(
            "INSERT INTO session_events(session_id, sequence, event_type, payload, occurred_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                session_id.0.to_string(),
                sequence,
                event_name(event),
                serde_json::to_string(event)?,
                Utc::now()
            ],
        )?;
        transaction.commit()?;
        Ok(sequence)
    }

    /// Persists the judgment event and exact authorization in one durable transaction.
    pub fn authorize(&mut self, authorization: &Authorization) -> Result<(), StoreError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let event = SessionEvent::AuthorizationPersisted {
            authorization: authorization.clone(),
        };
        let mut sequence: u64 = transaction.query_row(
            "SELECT COALESCE(MAX(sequence), 0) + 1 FROM session_events WHERE session_id = ?1",
            [authorization.session_id.0.to_string()],
            |row| row.get(0),
        )?;
        if authorization.approved_by
            != purrcode_runtime_core::ApprovalAuthority::DeterministicPolicy
        {
            let approval = SessionEvent::ApprovalRecorded {
                action_id: authorization.action_id,
                authority: authorization.approved_by.clone(),
                action_digest: authorization.action_digest.clone(),
            };
            transaction.execute(
                "INSERT INTO session_events(session_id, sequence, event_type, payload, occurred_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    authorization.session_id.0.to_string(),
                    sequence,
                    event_name(&approval),
                    serde_json::to_string(&approval)?,
                    Utc::now()
                ],
            )?;
            sequence += 1;
        }
        transaction.execute(
            "INSERT INTO authorizations(
                action_id, session_id, action_digest, constraints, authorized_at, approved_by, consumed_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL)",
            params![
                authorization.action_id.0.to_string(),
                authorization.session_id.0.to_string(),
                authorization.action_digest,
                serde_json::to_string(&authorization.constraints)?,
                authorization.authorized_at,
                serde_json::to_string(&authorization.approved_by)?,
            ],
        )?;
        transaction.execute(
            "INSERT INTO session_events(session_id, sequence, event_type, payload, occurred_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                authorization.session_id.0.to_string(),
                sequence,
                event_name(&event),
                serde_json::to_string(&event)?,
                Utc::now()
            ],
        )?;
        transaction.commit()?;
        Ok(())
    }

    /// Atomically consumes an authorization, enforcing at-most-once execution.
    pub fn consume_authorization(
        &mut self,
        action_id: ActionId,
        expected_digest: &str,
    ) -> Result<Authorization, StoreError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let row: Option<(String, String, String, String, chrono::DateTime<Utc>, String)> = transaction
            .query_row(
                "SELECT action_id, session_id, action_digest, constraints, authorized_at, approved_by
                 FROM authorizations
                 WHERE action_id = ?1 AND action_digest = ?2 AND consumed_at IS NULL",
                params![action_id.0.to_string(), expected_digest],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?)),
            )
            .optional()?;
        let Some((action, session, digest, constraints, authorized_at, approved_by)) = row else {
            return Err(StoreError::AuthorizationUnavailable);
        };
        let updated = transaction.execute(
            "UPDATE authorizations SET consumed_at = ?2 WHERE action_id = ?1 AND consumed_at IS NULL",
            params![action_id.0.to_string(), Utc::now()],
        )?;
        if updated != 1 {
            return Err(StoreError::AuthorizationUnavailable);
        }
        transaction.commit()?;
        Ok(Authorization {
            action_id: ActionId(Uuid::parse_str(&action)?),
            session_id: SessionId(Uuid::parse_str(&session)?),
            action_digest: digest,
            constraints: serde_json::from_str(&constraints)?,
            authorized_at,
            approved_by: serde_json::from_str(&approved_by)?,
        })
    }

    pub fn load(&self, session_id: SessionId) -> Result<SessionState, StoreError> {
        let events = self.events(session_id)?;
        let mut state = SessionState::empty(session_id);
        for event in events {
            state.apply(&event);
        }
        Ok(state)
    }

    pub fn events(&self, session_id: SessionId) -> Result<Vec<SessionEvent>, StoreError> {
        let mut statement = self.connection.prepare(
            "SELECT payload FROM session_events WHERE session_id = ?1 ORDER BY sequence",
        )?;
        let rows =
            statement.query_map([session_id.0.to_string()], |row| row.get::<_, String>(0))?;
        let mut events = Vec::new();
        for row in rows {
            events.push(serde_json::from_str(&row?)?);
        }
        Ok(events)
    }

    pub fn integrity_check(&self) -> Result<bool, StoreError> {
        let result: String = self
            .connection
            .query_row("PRAGMA integrity_check", [], |row| row.get(0))?;
        Ok(result == "ok")
    }

    pub fn schema_version(&self) -> Result<u32, StoreError> {
        self.connection
            .query_row(
                "SELECT COALESCE(MAX(version), 0) FROM schema_migrations",
                [],
                |row| row.get(0),
            )
            .map_err(Into::into)
    }

    /// Creates a transactionally consistent SQLite backup without copying live WAL files.
    pub fn backup(&self, destination: &Path) -> Result<(), StoreError> {
        if destination.exists() {
            return Err(StoreError::BackupDestinationExists(
                destination.to_path_buf(),
            ));
        }
        if let Some(parent) = destination.parent() {
            std::fs::create_dir_all(parent)?;
        }
        self.connection
            .backup(DatabaseName::Main, destination, None)?;
        let destination_connection = Connection::open(destination)?;
        let integrity: String =
            destination_connection.query_row("PRAGMA integrity_check", [], |row| row.get(0))?;
        if integrity != "ok" {
            return Err(StoreError::BackupIntegrity(integrity));
        }
        Ok(())
    }

    pub fn list_session_ids(&self) -> Result<Vec<SessionId>, StoreError> {
        let mut statement = self.connection.prepare(
            "SELECT session_id, MAX(occurred_at) AS latest
             FROM session_events GROUP BY session_id ORDER BY latest DESC",
        )?;
        let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
        let mut sessions = Vec::new();
        for row in rows {
            sessions.push(SessionId(Uuid::parse_str(&row?)?));
        }
        Ok(sessions)
    }

    pub fn latest_session_id(&self) -> Result<Option<SessionId>, StoreError> {
        Ok(self.list_session_ids()?.into_iter().next())
    }

    pub fn create_automation(
        &mut self,
        objective: &str,
        repository: &Path,
        interval_seconds: u64,
    ) -> Result<Automation, StoreError> {
        if objective.trim().is_empty() || interval_seconds < 60 {
            return Err(StoreError::InvalidAutomation(
                "objective is required and interval must be at least 60 seconds".into(),
            ));
        }
        let repository = repository.canonicalize()?;
        let now = Utc::now();
        let automation = Automation {
            id: Uuid::new_v4(),
            objective: objective.into(),
            repository,
            interval_seconds,
            enabled: true,
            next_run_at: now + ChronoDuration::seconds(interval_seconds as i64),
            last_session_id: None,
            created_at: now,
            updated_at: now,
        };
        self.connection.execute(
            "INSERT INTO automations(
                id, objective, repository, interval_seconds, enabled, next_run_at,
                last_session_id, created_at, updated_at
             ) VALUES (?1, ?2, ?3, ?4, 1, ?5, NULL, ?6, ?6)",
            params![
                automation.id.to_string(),
                automation.objective,
                automation.repository.to_string_lossy(),
                automation.interval_seconds,
                automation.next_run_at,
                automation.created_at,
            ],
        )?;
        Ok(automation)
    }

    pub fn automations(&self) -> Result<Vec<Automation>, StoreError> {
        let mut statement = self.connection.prepare(
            "SELECT id, objective, repository, interval_seconds, enabled, next_run_at,
                    last_session_id, created_at, updated_at
             FROM automations ORDER BY created_at",
        )?;
        let rows = statement.query_map([], automation_from_row)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn due_automations(&self, now: DateTime<Utc>) -> Result<Vec<Automation>, StoreError> {
        let mut statement = self.connection.prepare(
            "SELECT id, objective, repository, interval_seconds, enabled, next_run_at,
                    last_session_id, created_at, updated_at
             FROM automations WHERE enabled = 1 AND next_run_at <= ?1 ORDER BY next_run_at",
        )?;
        let rows = statement.query_map([now], automation_from_row)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn set_automation_enabled(&mut self, id: Uuid, enabled: bool) -> Result<(), StoreError> {
        let changed = self.connection.execute(
            "UPDATE automations SET enabled = ?2, updated_at = ?3 WHERE id = ?1",
            params![id.to_string(), enabled, Utc::now()],
        )?;
        if changed == 0 {
            return Err(StoreError::AutomationNotFound(id));
        }
        Ok(())
    }

    pub fn mark_automation_started(
        &mut self,
        id: Uuid,
        session_id: SessionId,
    ) -> Result<(), StoreError> {
        let automation = self
            .automations()?
            .into_iter()
            .find(|item| item.id == id)
            .ok_or(StoreError::AutomationNotFound(id))?;
        let now = Utc::now();
        self.connection.execute(
            "UPDATE automations
             SET last_session_id = ?2, next_run_at = ?3, updated_at = ?4
             WHERE id = ?1",
            params![
                id.to_string(),
                session_id.0.to_string(),
                now + ChronoDuration::seconds(automation.interval_seconds as i64),
                now,
            ],
        )?;
        Ok(())
    }

    /// Marks actions that were durably started but never durably finished as uncertain.
    ///
    /// This method is idempotent: once an uncertainty event is recorded, replay no longer
    /// reconstructs the session as executing.
    pub fn recover_uncertain_sessions(&mut self) -> Result<Vec<SessionId>, StoreError> {
        let session_ids = self.list_session_ids()?;
        let mut recovered = Vec::new();
        for session_id in session_ids {
            let state = self.load(session_id)?;
            if let purrcode_runtime_core::SessionStatus::Executing(action_id) = state.status {
                self.append(
                    session_id,
                    &SessionEvent::ValidationRecorded {
                        action_id,
                        status: purrcode_runtime_core::ValidationStatus::Uncertain,
                        evidence: "process state was uncertain after runtime restart; action will not be retried automatically".into(),
                    },
                )?;
                recovered.push(session_id);
            } else if state.status == purrcode_runtime_core::SessionStatus::Active {
                let events = self.events(session_id)?;
                let mut model_requests = 0_i64;
                for event in events {
                    match event {
                        SessionEvent::ModelRequestStarted { .. } => model_requests += 1,
                        SessionEvent::ModelRequestFinished { .. } => model_requests -= 1,
                        _ => {}
                    }
                }
                if model_requests > 0 {
                    self.append(
                        session_id,
                        &SessionEvent::RecoveryRequired {
                            reason: "model request was interrupted before its response was durably recorded; review the worktree before resume".into(),
                        },
                    )?;
                    recovered.push(session_id);
                }
            }
        }
        Ok(recovered)
    }
}

fn automation_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Automation> {
    fn uuid_at(row: &rusqlite::Row<'_>, index: usize) -> rusqlite::Result<Uuid> {
        let raw: String = row.get(index)?;
        Uuid::parse_str(&raw).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                index,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        })
    }
    let last_session: Option<String> = row.get(6)?;
    let last_session_id = last_session
        .map(|raw| {
            Uuid::parse_str(&raw).map(SessionId).map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(
                    6,
                    rusqlite::types::Type::Text,
                    Box::new(error),
                )
            })
        })
        .transpose()?;
    Ok(Automation {
        id: uuid_at(row, 0)?,
        objective: row.get(1)?,
        repository: PathBuf::from(row.get::<_, String>(2)?),
        interval_seconds: row.get(3)?,
        enabled: row.get(4)?,
        next_run_at: row.get(5)?,
        last_session_id,
        created_at: row.get(7)?,
        updated_at: row.get(8)?,
    })
}

fn event_name(event: &SessionEvent) -> &'static str {
    match event {
        SessionEvent::SessionCreated { .. } => "session_created",
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
    }
}

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("database operation failed: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("event serialization failed: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("stored identifier is corrupt: {0}")]
    Identifier(#[from] uuid::Error),
    #[error("authorization is missing, mismatched, or already consumed")]
    AuthorizationUnavailable,
    #[error("backup destination already exists: {0}")]
    BackupDestinationExists(std::path::PathBuf),
    #[error("backup integrity check failed: {0}")]
    BackupIntegrity(String),
    #[error("filesystem operation failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("automation configuration is invalid: {0}")]
    InvalidAutomation(String),
    #[error("automation `{0}` was not found")]
    AutomationNotFound(Uuid),
}

#[cfg(test)]
mod tests {
    use super::*;
    use purrcode_runtime_core::{ActionConstraints, ApprovalAuthority};
    use std::path::PathBuf;

    #[test]
    fn automations_are_durable_and_claimed_before_execution() {
        let repository = tempfile::tempdir().unwrap();
        let mut store = SessionStore::in_memory().unwrap();
        let automation = store
            .create_automation("run repository health check", repository.path(), 60)
            .unwrap();
        assert!(automation.enabled);
        assert_eq!(store.schema_version().unwrap(), 2);
        assert!(store.due_automations(Utc::now()).unwrap().is_empty());
        store
            .connection
            .execute(
                "UPDATE automations SET next_run_at = ?2 WHERE id = ?1",
                params![
                    automation.id.to_string(),
                    Utc::now() - ChronoDuration::seconds(1)
                ],
            )
            .unwrap();
        assert_eq!(store.due_automations(Utc::now()).unwrap().len(), 1);
        let session = SessionId::new();
        store
            .mark_automation_started(automation.id, session)
            .unwrap();
        let updated = store.automations().unwrap().pop().unwrap();
        assert_eq!(updated.last_session_id, Some(session));
        assert!(updated.next_run_at > Utc::now());
    }

    #[test]
    fn authorization_can_only_be_consumed_once() {
        let mut store = SessionStore::in_memory().unwrap();
        let auth = Authorization {
            action_id: ActionId::new(),
            session_id: SessionId::new(),
            action_digest: "digest".into(),
            constraints: ActionConstraints::read_only(PathBuf::from("/repo")),
            authorized_at: Utc::now(),
            approved_by: ApprovalAuthority::DeterministicPolicy,
        };
        store.authorize(&auth).unwrap();
        store
            .consume_authorization(auth.action_id, "digest")
            .unwrap();
        assert!(matches!(
            store.consume_authorization(auth.action_id, "digest"),
            Err(StoreError::AuthorizationUnavailable)
        ));
    }

    #[test]
    fn restart_marks_started_but_unfinished_action_uncertain() {
        let temporary = tempfile::tempdir().unwrap();
        let database = temporary.path().join("sessions.db");
        let session_id = SessionId::new();
        let action_id = ActionId::new();
        {
            let mut store = SessionStore::open(&database).unwrap();
            store
                .append(
                    session_id,
                    &SessionEvent::SessionCreated {
                        objective: "recover".into(),
                        repository: PathBuf::from("/repo"),
                    },
                )
                .unwrap();
            store
                .append(session_id, &SessionEvent::ExecutionStarted { action_id })
                .unwrap();
        }
        let mut reopened = SessionStore::open(&database).unwrap();
        assert_eq!(
            reopened.recover_uncertain_sessions().unwrap(),
            vec![session_id]
        );
        assert_eq!(
            reopened.load(session_id).unwrap().status,
            purrcode_runtime_core::SessionStatus::Uncertain
        );
        assert!(reopened.recover_uncertain_sessions().unwrap().is_empty());
    }

    #[test]
    fn online_backup_is_integrity_checked_and_does_not_overwrite() {
        let temporary = tempfile::tempdir().unwrap();
        let backup = temporary.path().join("backup.db");
        let mut store = SessionStore::in_memory().unwrap();
        let session = SessionId::new();
        store
            .append(
                session,
                &SessionEvent::SessionCreated {
                    objective: "backup".into(),
                    repository: PathBuf::from("/repo"),
                },
            )
            .unwrap();
        store.backup(&backup).unwrap();
        let restored = SessionStore::open(&backup).unwrap();
        assert_eq!(
            restored.load(session).unwrap().objective.as_deref(),
            Some("backup")
        );
        assert!(matches!(
            store.backup(&backup),
            Err(StoreError::BackupDestinationExists(_))
        ));
    }

    #[test]
    fn restart_marks_interrupted_provider_request_for_review() {
        let mut store = SessionStore::in_memory().unwrap();
        let session = SessionId::new();
        store
            .append(
                session,
                &SessionEvent::SessionCreated {
                    objective: "recover provider".into(),
                    repository: PathBuf::from("/repo"),
                },
            )
            .unwrap();
        store
            .append(
                session,
                &SessionEvent::ModelRequestStarted {
                    role: "coder".into(),
                    provider: "fixture".into(),
                    model: "model".into(),
                },
            )
            .unwrap();
        assert_eq!(store.recover_uncertain_sessions().unwrap(), vec![session]);
        assert_eq!(
            store.load(session).unwrap().status,
            purrcode_runtime_core::SessionStatus::Uncertain
        );
        assert!(store.recover_uncertain_sessions().unwrap().is_empty());
    }
}
