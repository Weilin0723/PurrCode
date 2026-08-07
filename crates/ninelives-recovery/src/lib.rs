//! SQLite-backed append-only session log and authorization ledger.

use chrono::{DateTime, Duration as ChronoDuration, Utc};
use purrcode_runtime_core::{ActionId, Authorization, SessionEvent, SessionId, SessionState};
use rusqlite::{Connection, DatabaseName, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use thiserror::Error;
use uuid::Uuid;

const MIGRATION_1: &str = include_str!("../../../migrations/0001_initial.sql");
const MIGRATION_2: &str = include_str!("../../../migrations/0002_automations.sql");
const MIGRATION_3: &str = include_str!("../../../migrations/0003_session_workspace.sql");

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

/// Presentation/workspace metadata for a session. Kept out of the replayable
/// event log because title/archive/pin are organization state, not audit state.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct SessionMeta {
    pub title: Option<String>,
    pub archived: bool,
    pub pinned: bool,
    pub parent_id: Option<SessionId>,
    pub deleted: bool,
}

impl SessionMeta {
    /// A session with no stored metadata row shows its objective as the title.
    pub fn titled(objective: Option<&str>) -> Self {
        SessionMeta {
            title: objective.map(ToOwned::to_owned),
            ..Self::default()
        }
    }
}

/// A full-text search hit over the session event log.
#[derive(Clone, Debug, PartialEq)]
pub struct SessionSearchHit {
    pub session_id: SessionId,
    pub event_type: String,
    pub snippet: String,
    pub occurred_at: DateTime<Utc>,
}

/// A restorable worktree checkpoint. The patch blob is persisted at capture
/// time so a later "restore here" can reverse-apply it; the `CheckpointCreated`
/// event remains the durable audit record.
#[derive(Clone, Debug, PartialEq)]
pub struct SessionCheckpoint {
    pub id: Uuid,
    pub session_id: SessionId,
    pub sequence: u64,
    pub label: String,
    pub head: String,
    pub patch: Vec<u8>,
    pub patch_digest: String,
    pub created_at: DateTime<Utc>,
}

/// A durable, auditable piece of project knowledge. Entries are user-authored
/// (never silently inferred by the agent) and carry their provenance.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct ProjectMemoryEntry {
    pub id: Uuid,
    pub repository: PathBuf,
    pub kind: String,
    pub content: String,
    pub source: String,
    pub confidence: String,
    pub scope: String,
    pub created_at: DateTime<Utc>,
    pub last_used_at: Option<DateTime<Utc>>,
}

/// Result of startup reconciliation. A legacy session whose event log no
/// longer satisfies the current state-machine invariants is isolated by ID;
/// healthy sessions still recover normally and the daemon can serve new work.
/// The invalid log is never rewritten or treated as valid.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RecoveryReport {
    pub recovered: Vec<SessionId>,
    pub unavailable: BTreeMap<SessionId, String>,
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
        transaction.execute_batch(MIGRATION_3)?;
        transaction.commit()?;
        Ok(())
    }

    pub fn append(
        &mut self,
        session_id: SessionId,
        event: &SessionEvent,
    ) -> Result<u64, StoreError> {
        // Every durable prefix must replay deterministically. Persisting an
        // invalid event and skipping it later would make the audit log and the
        // product state disagree.
        let mut next = self.load(session_id)?;
        next.reduce_event(event)
            .map_err(|error| StoreError::InvalidEvent {
                session: session_id,
                reason: error.to_string(),
            })?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let sequence: u64 = transaction.query_row(
            "SELECT COALESCE(MAX(sequence), 0) + 1 FROM session_events WHERE session_id = ?1",
            [session_id.0.to_string()],
            |row| row.get(0),
        )?;
        let payload = serde_json::to_string(event)?;
        transaction.execute(
            "INSERT INTO session_events(session_id, sequence, event_type, payload, occurred_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                session_id.0.to_string(),
                sequence,
                event_name(event),
                payload,
                Utc::now()
            ],
        )?;
        // Keep the live FTS index in the same transaction as the append so
        // search can never lag the durable log.
        transaction.execute(
            "INSERT INTO session_search(session_id, event_type, payload)
             VALUES (?1, ?2, ?3)",
            params![session_id.0.to_string(), event_name(event), payload],
        )?;
        transaction.commit()?;
        Ok(sequence)
    }

    /// Persists the judgment event and exact authorization in one durable transaction.
    pub fn authorize(&mut self, authorization: &Authorization) -> Result<(), StoreError> {
        let mut next = self.load(authorization.session_id)?;
        if authorization.approved_by == purrcode_runtime_core::ApprovalAuthority::Human {
            next.reduce_event(&SessionEvent::ApprovalRecorded {
                action_id: authorization.action_id,
                authority: authorization.approved_by.clone(),
                action_digest: authorization.action_digest.clone(),
            })
            .map_err(|error| StoreError::InvalidEvent {
                session: authorization.session_id,
                reason: error.to_string(),
            })?;
        }
        next.reduce_event(&SessionEvent::AuthorizationPersisted {
            authorization: authorization.clone(),
        })
        .map_err(|error| StoreError::InvalidEvent {
            session: authorization.session_id,
            reason: error.to_string(),
        })?;
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
        if authorization.approved_by == purrcode_runtime_core::ApprovalAuthority::Human {
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
        for (index, event) in events.into_iter().enumerate() {
            state
                .reduce_event(&event)
                .map_err(|error| StoreError::ReplayInconsistent {
                    session: session_id,
                    sequence: index as u64 + 1,
                    reason: error.to_string(),
                })?;
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

    pub fn timestamped_events(
        &self,
        session_id: SessionId,
    ) -> Result<Vec<(DateTime<Utc>, SessionEvent)>, StoreError> {
        let mut statement = self.connection.prepare(
            "SELECT occurred_at, payload FROM session_events WHERE session_id = ?1 ORDER BY sequence",
        )?;
        let rows = statement.query_map([session_id.0.to_string()], |row| {
            Ok((row.get::<_, DateTime<Utc>>(0)?, row.get::<_, String>(1)?))
        })?;
        let mut events = Vec::new();
        for row in rows {
            let (timestamp, payload) = row?;
            events.push((timestamp, serde_json::from_str(&payload)?));
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

    /// Returns the stored metadata row for a session, or a default that titles
    /// the session by its objective. Sessions created before v1.2 have no row.
    pub fn session_meta(&self, session_id: SessionId) -> Result<SessionMeta, StoreError> {
        self.connection
            .query_row(
                "SELECT title, archived, pinned, parent_id, deleted
                 FROM session_meta WHERE session_id = ?1",
                [session_id.0.to_string()],
                |row| {
                    Ok(SessionMeta {
                        title: row.get(0)?,
                        archived: row.get::<_, i64>(1)? != 0,
                        pinned: row.get::<_, i64>(2)? != 0,
                        parent_id: row
                            .get::<_, Option<String>>(3)?
                            .map(|value| SessionId(Uuid::parse_str(&value).unwrap_or_default())),
                        deleted: row.get::<_, i64>(4)? != 0,
                    })
                },
            )
            .optional()?
            .map(Ok)
            .unwrap_or_else(|| Ok(SessionMeta::default()))
    }

    fn upsert_meta(
        &mut self,
        session_id: SessionId,
        title: Option<&str>,
        archived: Option<bool>,
        pinned: Option<bool>,
        parent_id: Option<Option<SessionId>>,
        deleted: Option<bool>,
    ) -> Result<(), StoreError> {
        let current = self.session_meta(session_id)?;
        let next = SessionMeta {
            title: title.map(ToOwned::to_owned).or(current.title),
            archived: archived.unwrap_or(current.archived),
            pinned: pinned.unwrap_or(current.pinned),
            parent_id: match parent_id {
                Some(Some(id)) => Some(id),
                Some(None) => None,
                None => current.parent_id,
            },
            deleted: deleted.unwrap_or(current.deleted),
        };
        let now = Utc::now();
        self.connection.execute(
            "INSERT INTO session_meta(session_id, title, archived, pinned, parent_id, deleted, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)
             ON CONFLICT(session_id) DO UPDATE SET
                title = excluded.title,
                archived = excluded.archived,
                pinned = excluded.pinned,
                parent_id = excluded.parent_id,
                deleted = excluded.deleted,
                updated_at = excluded.updated_at",
            params![
                session_id.0.to_string(),
                next.title,
                next.archived,
                next.pinned,
                next.parent_id.map(|id| id.0.to_string()),
                next.deleted,
                now,
            ],
        )?;
        Ok(())
    }

    pub fn set_session_title(
        &mut self,
        session_id: SessionId,
        title: &str,
    ) -> Result<(), StoreError> {
        if title.trim().is_empty() {
            return Err(StoreError::InvalidAutomation(
                "session title must be a non-empty string".into(),
            ));
        }
        self.upsert_meta(session_id, Some(title), None, None, None, None)
    }

    pub fn set_session_archived(
        &mut self,
        session_id: SessionId,
        archived: bool,
    ) -> Result<(), StoreError> {
        self.upsert_meta(session_id, None, Some(archived), None, None, None)
    }

    pub fn set_session_pinned(
        &mut self,
        session_id: SessionId,
        pinned: bool,
    ) -> Result<(), StoreError> {
        self.upsert_meta(session_id, None, None, Some(pinned), None, None)
    }

    pub fn set_session_deleted(
        &mut self,
        session_id: SessionId,
        deleted: bool,
    ) -> Result<(), StoreError> {
        self.upsert_meta(session_id, None, None, None, None, Some(deleted))
    }

    pub fn set_session_parent(
        &mut self,
        session_id: SessionId,
        parent_id: SessionId,
    ) -> Result<(), StoreError> {
        self.upsert_meta(session_id, None, None, None, Some(Some(parent_id)), None)
    }

    /// Full-text search across the durable event log. Returns a bounded,
    /// most-recent-first set of hits with a highlight snippet.
    pub fn search_sessions(
        &self,
        query: &str,
        limit: u64,
    ) -> Result<Vec<SessionSearchHit>, StoreError> {
        let query = query.trim();
        if query.is_empty() {
            return Err(StoreError::InvalidAutomation(
                "search query must be a non-empty string".into(),
            ));
        }
        let bound = limit.clamp(1, 100);
        let mut statement = self.connection.prepare(
            "SELECT session_search.session_id, session_search.event_type,
                    snippet(session_search, 2, '', '', '…', 80) AS snippet,
                    session_events.occurred_at
             FROM session_search
             JOIN session_events
               ON session_events.session_id = session_search.session_id
              AND session_events.event_type = session_search.event_type
             WHERE session_search.payload MATCH ?1
             ORDER BY session_events.occurred_at DESC
             LIMIT ?2",
        )?;
        let rows = statement.query_map(params![query, bound], |row| {
            let session_id = Uuid::parse_str(&row.get::<_, String>(0)?).map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(
                    0,
                    rusqlite::types::Type::Text,
                    Box::new(error),
                )
            })?;
            Ok(SessionSearchHit {
                session_id: SessionId(session_id),
                event_type: row.get(1)?,
                snippet: row.get(2)?,
                occurred_at: row.get(3)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Copies a session's event prefix `[1..=anchor_sequence]` (the parent's
    /// own `SessionCreated` is not copied) into a fresh child session in one
    /// transaction. The child's meta row records the parent linkage. The
    /// caller is responsible for appending the child's own `SessionCreated`
    /// and `WorktreeCreated` events before the copied prefix is meaningful.
    pub fn fork_session_events(
        &mut self,
        parent_id: SessionId,
        child_id: SessionId,
        anchor_sequence: u64,
    ) -> Result<u64, StoreError> {
        let parent_events = self.events(parent_id)?;
        if parent_events.is_empty() {
            return Err(StoreError::InvalidAutomation(
                "cannot fork an empty parent session".into(),
            ));
        }
        // The parent's SessionCreated is event index 0 (sequence 1); skip it.
        let copy_from = parent_events
            .split_first()
            .map(|(_, rest)| rest)
            .unwrap_or(&[]);
        let copy_from = copy_from
            .iter()
            .take(anchor_sequence as usize)
            .collect::<Vec<_>>();
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut sequence: u64 = transaction.query_row(
            "SELECT COALESCE(MAX(sequence), 0) + 1 FROM session_events WHERE session_id = ?1",
            [child_id.0.to_string()],
            |row| row.get(0),
        )?;
        for event in copy_from {
            let payload = serde_json::to_string(event)?;
            transaction.execute(
                "INSERT INTO session_events(session_id, sequence, event_type, payload, occurred_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    child_id.0.to_string(),
                    sequence,
                    event_name(event),
                    payload,
                    Utc::now()
                ],
            )?;
            transaction.execute(
                "INSERT INTO session_search(session_id, event_type, payload)
                 VALUES (?1, ?2, ?3)",
                params![child_id.0.to_string(), event_name(event), payload],
            )?;
            sequence += 1;
        }
        let now = Utc::now();
        transaction.execute(
            "INSERT INTO session_meta(session_id, title, archived, pinned, parent_id, deleted, created_at, updated_at)
             VALUES (?1, NULL, 0, 0, ?2, 0, ?3, ?3)",
            params![child_id.0.to_string(), parent_id.0.to_string(), now],
        )?;
        transaction.commit()?;
        Ok(sequence - 1)
    }

    /// Persists a restorable checkpoint. The patch blob is stored so a later
    /// restore can reverse-apply it, and the caller's `CheckpointCreated`
    /// event is the durable audit record.
    pub fn insert_checkpoint(&mut self, checkpoint: &SessionCheckpoint) -> Result<(), StoreError> {
        self.connection.execute(
            "INSERT INTO session_checkpoints(id, session_id, sequence, label, head, patch, patch_digest, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                checkpoint.id.to_string(),
                checkpoint.session_id.0.to_string(),
                checkpoint.sequence,
                checkpoint.label,
                checkpoint.head,
                checkpoint.patch,
                checkpoint.patch_digest,
                checkpoint.created_at,
            ],
        )?;
        Ok(())
    }

    pub fn checkpoints(&self, session_id: SessionId) -> Result<Vec<SessionCheckpoint>, StoreError> {
        let mut statement = self.connection.prepare(
            "SELECT id, session_id, sequence, label, head, patch, patch_digest, created_at
             FROM session_checkpoints WHERE session_id = ?1 ORDER BY sequence",
        )?;
        let rows = statement.query_map([session_id.0.to_string()], checkpoint_from_row)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn checkpoint(&self, id: Uuid) -> Result<SessionCheckpoint, StoreError> {
        self.connection
            .query_row(
                "SELECT id, session_id, sequence, label, head, patch, patch_digest, created_at
                 FROM session_checkpoints WHERE id = ?1",
                [id.to_string()],
                checkpoint_from_row,
            )
            .optional()?
            .ok_or(StoreError::CheckpointNotFound(id))
    }

    /// Copies the checkpoint rows of a parent session into a forked child,
    /// re-keyed with fresh ids for the child, so the child keeps its own
    /// restore history without colliding on the primary key.
    pub fn copy_checkpoints(
        &mut self,
        parent_id: SessionId,
        child_id: SessionId,
    ) -> Result<(), StoreError> {
        let checkpoints = self.checkpoints(parent_id)?;
        for checkpoint in checkpoints {
            self.connection.execute(
                "INSERT INTO session_checkpoints(id, session_id, sequence, label, head, patch, patch_digest, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    Uuid::new_v4().to_string(),
                    child_id.0.to_string(),
                    checkpoint.sequence,
                    checkpoint.label,
                    checkpoint.head,
                    checkpoint.patch,
                    checkpoint.patch_digest,
                    checkpoint.created_at,
                ],
            )?;
        }
        Ok(())
    }

    /// Inserts a user-authored project memory entry. The caller has already
    /// validated and secret-scanned the content.
    pub fn insert_memory(&mut self, entry: &ProjectMemoryEntry) -> Result<(), StoreError> {
        self.connection.execute(
            "INSERT INTO project_memory(id, repository, kind, content, source, confidence, scope, created_at, last_used_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                entry.id.to_string(),
                entry.repository.to_string_lossy(),
                entry.kind,
                entry.content,
                entry.source,
                entry.confidence,
                entry.scope,
                entry.created_at,
                entry.last_used_at,
            ],
        )?;
        Ok(())
    }

    /// Lists project memory for a repository, optionally filtered by kind,
    /// newest first.
    pub fn memory(
        &self,
        repository: &Path,
        kind: Option<&str>,
    ) -> Result<Vec<ProjectMemoryEntry>, StoreError> {
        let repository = repository.to_string_lossy();
        let mut sql = String::from(
            "SELECT id, repository, kind, content, source, confidence, scope, created_at, last_used_at
             FROM project_memory WHERE repository = ?1",
        );
        if kind.is_some() {
            sql.push_str(" AND kind = ?2");
        }
        sql.push_str(" ORDER BY created_at DESC");
        let mut statement = self.connection.prepare(&sql)?;
        let rows = match kind {
            Some(kind) => {
                statement.query_map(params![repository.as_ref(), kind], memory_from_row)?
            }
            None => statement.query_map(params![repository.as_ref()], memory_from_row)?,
        };
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn memory_entry(&self, id: Uuid) -> Result<ProjectMemoryEntry, StoreError> {
        self.connection
            .query_row(
                "SELECT id, repository, kind, content, source, confidence, scope, created_at, last_used_at
                 FROM project_memory WHERE id = ?1",
                [id.to_string()],
                memory_from_row,
            )
            .optional()?
            .ok_or(StoreError::MemoryNotFound(id))
    }

    /// Edits a memory entry's content, preserving its provenance.
    pub fn update_memory_content(&mut self, id: Uuid, content: &str) -> Result<(), StoreError> {
        let changed = self.connection.execute(
            "UPDATE project_memory SET content = ?2 WHERE id = ?1",
            params![id.to_string(), content],
        )?;
        if changed == 0 {
            return Err(StoreError::MemoryNotFound(id));
        }
        Ok(())
    }

    /// Forgets a memory entry (removes it). Deletion is the only action: an
    /// entry the user removes should not silently reappear.
    pub fn forget_memory(&mut self, id: Uuid) -> Result<(), StoreError> {
        let changed = self
            .connection
            .execute("DELETE FROM project_memory WHERE id = ?1", [id.to_string()])?;
        if changed == 0 {
            return Err(StoreError::MemoryNotFound(id));
        }
        Ok(())
    }

    /// Marks a memory entry as used so recency ranking reflects real usage.
    pub fn touch_memory(&mut self, id: Uuid) -> Result<(), StoreError> {
        self.connection.execute(
            "UPDATE project_memory SET last_used_at = ?2 WHERE id = ?1",
            params![id.to_string(), Utc::now()],
        )?;
        Ok(())
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
        Ok(self.recover_uncertain_sessions_with_quarantine()?.recovered)
    }

    /// Reconcile healthy sessions while quarantining only legacy event logs
    /// that cannot be replayed under today's state machine. Database/I/O
    /// failures still abort startup; this is not a general error suppression
    /// path. Appending to a quarantined session continues to fail closed via
    /// [`SessionStore::append`] and [`SessionStore::load`].
    pub fn recover_uncertain_sessions_with_quarantine(
        &mut self,
    ) -> Result<RecoveryReport, StoreError> {
        let session_ids = self.list_session_ids()?;
        let mut report = RecoveryReport::default();
        for session_id in session_ids {
            let state = match self.load(session_id) {
                Ok(state) => state,
                Err(
                    error @ (StoreError::ReplayInconsistent { .. }
                    | StoreError::Serialization(_)
                    | StoreError::Identifier(_)),
                ) => {
                    report.unavailable.insert(session_id, error.to_string());
                    continue;
                }
                Err(error) => return Err(error),
            };
            if let purrcode_runtime_core::SessionStatus::Executing(action_id) = state.status {
                self.append(
                    session_id,
                    &SessionEvent::ValidationRecorded {
                        action_id,
                        status: purrcode_runtime_core::ValidationStatus::Uncertain,
                        evidence: "process state was uncertain after runtime restart; action will not be retried automatically".into(),
                    },
                )?;
                report.recovered.push(session_id);
            } else if state.status == purrcode_runtime_core::SessionStatus::Active {
                let events = self.events(session_id)?;
                let mut model_requests = 0_i64;
                let mut has_run_activity = false;
                for event in events {
                    match event {
                        SessionEvent::ModelRequestStarted { .. } => {
                            model_requests += 1;
                            has_run_activity = true;
                        }
                        SessionEvent::ModelRequestFinished { .. } => model_requests -= 1,
                        SessionEvent::ExecutionStarted { .. }
                        | SessionEvent::PlanCreated { .. } => {
                            has_run_activity = true;
                        }
                        _ => {}
                    }
                }
                // A mid-run crash is not always visible as an outstanding model
                // request: the daemon can die between a finished request and the
                // next one (during a tool execution, or while appending a
                // non-model event). If any run work began, treat the orphaned
                // `Active` session as uncertain so it can be recovered. A fresh
                // session that was created and never started stays untouched.
                if model_requests > 0 || (has_run_activity && model_requests == 0) {
                    let reason = if model_requests > 0 {
                        "model request was interrupted before its response was durably recorded; review the worktree before resume"
                    } else {
                        "runtime restart interrupted this session after work had begun; review the worktree before resume"
                    };
                    self.append(
                        session_id,
                        &SessionEvent::RecoveryRequired {
                            reason: reason.into(),
                        },
                    )?;
                    report.recovered.push(session_id);
                }
            }
        }
        Ok(report)
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

fn memory_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ProjectMemoryEntry> {
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
    Ok(ProjectMemoryEntry {
        id: uuid_at(row, 0)?,
        repository: PathBuf::from(row.get::<_, String>(1)?),
        kind: row.get(2)?,
        content: row.get(3)?,
        source: row.get(4)?,
        confidence: row.get(5)?,
        scope: row.get(6)?,
        created_at: row.get(7)?,
        last_used_at: row.get(8)?,
    })
}

fn checkpoint_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<SessionCheckpoint> {
    Ok(SessionCheckpoint {
        id: Uuid::parse_str(&row.get::<_, String>(0)?).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                0,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        })?,
        session_id: SessionId(Uuid::parse_str(&row.get::<_, String>(1)?).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                1,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        })?),
        sequence: row.get(2)?,
        label: row.get(3)?,
        head: row.get(4)?,
        patch: row.get(5)?,
        patch_digest: row.get(6)?,
        created_at: row.get(7)?,
    })
}

fn event_name(event: &SessionEvent) -> &'static str {
    match event {
        SessionEvent::SessionCreated { .. } => "session_created",
        SessionEvent::SessionControlsUpdated { .. } => "session_controls_updated",
        SessionEvent::WorkflowPlanCreated { .. } => "workflow_plan_created",
        SessionEvent::UsageRecorded { .. } => "usage_recorded",
        SessionEvent::WorktreeCreated { .. } => "worktree_created",
        SessionEvent::SubmodulesPrepared { .. } => "submodules_prepared",
        SessionEvent::PlanCreated { .. } => "plan_created",
        SessionEvent::PlanRevised { .. } => "plan_revised",
        SessionEvent::SpecBundleRecorded { .. } => "spec_bundle_recorded",
        SessionEvent::TaskGraphRecorded { .. } => "task_graph_recorded",
        SessionEvent::TaskStatusChanged { .. } => "task_status_changed",
        SessionEvent::EvidenceLinked { .. } => "evidence_linked",
        SessionEvent::ContextCompacted { .. } => "context_compacted",
        SessionEvent::ContextAssembled { .. } => "context_assembled",
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
        SessionEvent::CheckpointRestored { .. } => "checkpoint_restored",
        SessionEvent::SessionForked { .. } => "session_forked",
        SessionEvent::WorktreeDispositionRecorded { .. } => "worktree_disposition_recorded",
        SessionEvent::SessionCancelled { .. } => "session_cancelled",
        SessionEvent::RecoveryRequired { .. } => "recovery_required",
        SessionEvent::SessionCompleted => "session_completed",
        SessionEvent::SessionFailed { .. } => "session_failed",
        _ => "research_event",
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
    #[error("checkpoint `{0}` was not found")]
    CheckpointNotFound(Uuid),
    #[error("project memory entry `{0}` was not found")]
    MemoryNotFound(Uuid),
    #[error("session {session:?} rejected invalid event: {reason}")]
    InvalidEvent { session: SessionId, reason: String },
    #[error("session {session:?} event log is inconsistent at sequence {sequence}: {reason}")]
    ReplayInconsistent {
        session: SessionId,
        sequence: u64,
        reason: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use purrcode_runtime_core::{ActionConstraints, ApprovalAuthority};
    use std::path::PathBuf;

    #[test]
    fn invalid_events_are_rejected_before_they_enter_the_log() {
        let mut store = SessionStore::in_memory().unwrap();
        let session = SessionId::new();
        let error = store
            .append(
                session,
                &SessionEvent::ApprovalRecorded {
                    action_id: ActionId::new(),
                    authority: ApprovalAuthority::Human,
                    action_digest: "not-proposed".into(),
                },
            )
            .unwrap_err();
        assert!(matches!(error, StoreError::InvalidEvent { .. }));
        assert!(store.events(session).unwrap().is_empty());
    }

    #[test]
    fn an_inconsistent_persisted_log_fails_loudly_at_its_sequence() {
        let mut store = SessionStore::in_memory().unwrap();
        let session = SessionId::new();
        let created = SessionEvent::SessionCreated {
            objective: "preserve replay integrity".into(),
            repository: PathBuf::from("/repo"),
            authority_mode: Default::default(),
        };
        store.append(session, &created).unwrap();
        store
            .connection
            .execute(
                "INSERT INTO session_events(session_id, sequence, event_type, payload, occurred_at)
                 VALUES (?1, 2, 'session_created', ?2, ?3)",
                params![
                    session.0.to_string(),
                    serde_json::to_string(&created).unwrap(),
                    Utc::now()
                ],
            )
            .unwrap();

        assert!(matches!(
            store.load(session),
            Err(StoreError::ReplayInconsistent { sequence: 2, .. })
        ));
    }

    #[test]
    fn startup_recovery_quarantines_one_invalid_session_and_keeps_healthy_sessions() {
        let mut store = SessionStore::in_memory().unwrap();
        let healthy = SessionId::new();
        store
            .append(
                healthy,
                &SessionEvent::SessionCreated {
                    objective: "healthy".into(),
                    repository: PathBuf::from("/healthy"),
                    authority_mode: Default::default(),
                },
            )
            .unwrap();

        let invalid = SessionId::new();
        store
            .append(
                invalid,
                &SessionEvent::SessionCreated {
                    objective: "legacy approval".into(),
                    repository: PathBuf::from("/legacy"),
                    authority_mode: Default::default(),
                },
            )
            .unwrap();
        let invalid_event = SessionEvent::ApprovalRecorded {
            action_id: ActionId::new(),
            authority: ApprovalAuthority::Human,
            action_digest: "legacy-digest".into(),
        };
        store
            .connection
            .execute(
                "INSERT INTO session_events(session_id, sequence, event_type, payload, occurred_at)
                 VALUES (?1, 2, 'approval_recorded', ?2, ?3)",
                params![
                    invalid.0.to_string(),
                    serde_json::to_string(&invalid_event).unwrap(),
                    Utc::now()
                ],
            )
            .unwrap();

        let report = store.recover_uncertain_sessions_with_quarantine().unwrap();
        assert!(report.recovered.is_empty());
        assert!(report.unavailable.contains_key(&invalid));
        assert!(!report.unavailable.contains_key(&healthy));
        assert_eq!(store.load(healthy).unwrap().event_count, 1);
        assert!(matches!(
            store.load(invalid),
            Err(StoreError::ReplayInconsistent { session, .. }) if session == invalid
        ));
        assert!(matches!(
            store.append(
                invalid,
                &SessionEvent::SessionFailed {
                    reason: "must remain fail-closed".into(),
                }
            ),
            Err(StoreError::ReplayInconsistent { session, .. }) if session == invalid
        ));
    }

    #[test]
    fn automations_are_durable_and_claimed_before_execution() {
        let repository = tempfile::tempdir().unwrap();
        let mut store = SessionStore::in_memory().unwrap();
        let automation = store
            .create_automation("run repository health check", repository.path(), 60)
            .unwrap();
        assert!(automation.enabled);
        assert_eq!(store.schema_version().unwrap(), 3);
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
    fn signed_policy_authorization_does_not_fabricate_a_human_approval() {
        let mut store = SessionStore::in_memory().unwrap();
        let session_id = SessionId::new();
        let auth = Authorization {
            action_id: ActionId::new(),
            session_id,
            action_digest: "signed-digest".into(),
            constraints: ActionConstraints::read_only(PathBuf::from("/repo")),
            authorized_at: Utc::now(),
            approved_by: ApprovalAuthority::SignedPolicy {
                policy_id: "validation-runtime".into(),
            },
        };
        store.authorize(&auth).unwrap();
        let events = store.events(session_id).unwrap();
        assert!(
            events
                .iter()
                .any(|event| matches!(event, SessionEvent::AuthorizationPersisted { .. }))
        );
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, SessionEvent::ApprovalRecorded { .. }))
        );
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
                        authority_mode: Default::default(),
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
                    authority_mode: Default::default(),
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
                    authority_mode: Default::default(),
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

    #[test]
    fn restart_marks_active_session_with_run_activity_for_review() {
        // The daemon can die between a finished model request and the next one
        // (during a tool execution). The session is `Active` with zero
        // outstanding model requests, but work had begun — it must not be
        // left orphaned as `running` forever.
        let mut store = SessionStore::in_memory().unwrap();
        let session = SessionId::new();
        store
            .append(
                session,
                &SessionEvent::SessionCreated {
                    objective: "recover mid-run".into(),
                    repository: PathBuf::from("/repo"),
                    authority_mode: Default::default(),
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
        store
            .append(
                session,
                &SessionEvent::ModelRequestFinished {
                    role: "coder".into(),
                    input_tokens: None,
                    output_tokens: None,
                },
            )
            .unwrap();
        store
            .append(
                session,
                &SessionEvent::PlanCreated {
                    steps: vec!["Do the work".into()],
                },
            )
            .unwrap();
        assert_eq!(store.recover_uncertain_sessions().unwrap(), vec![session]);
        assert_eq!(
            store.load(session).unwrap().status,
            purrcode_runtime_core::SessionStatus::Uncertain
        );
    }

    #[test]
    fn restart_leaves_fresh_active_session_untouched() {
        // A session that was created and never began running (no worktree,
        // no plan, no model request) stays `Active` so the user's first
        // follow-up starts it normally.
        let mut store = SessionStore::in_memory().unwrap();
        let session = SessionId::new();
        store
            .append(
                session,
                &SessionEvent::SessionCreated {
                    objective: "fresh".into(),
                    repository: PathBuf::from("/repo"),
                    authority_mode: Default::default(),
                },
            )
            .unwrap();
        store
            .append(
                session,
                &SessionEvent::SessionControlsUpdated {
                    controls: Default::default(),
                },
            )
            .unwrap();
        assert!(store.recover_uncertain_sessions().unwrap().is_empty());
        assert_eq!(
            store.load(session).unwrap().status,
            purrcode_runtime_core::SessionStatus::Active
        );
    }

    /// PRD v1.1 §14.1: replaying a session's durable event log must
    /// reconstruct the exact same `ContextLedgerEntry` values that were
    /// appended — the same durability/replay parity every other
    /// `SessionEvent` variant already gets from `append`/`load`/`events`
    /// (PRD §6.4, §11.3: `ContextAssembled` is "one more `SessionEvent`
    /// variant flowing through the exact same path").
    #[test]
    fn replay_reconstructs_identical_context_ledger_entries() {
        use purrcode_runtime_core::{
            ContextClass, ContextLedgerEntry, ContextLedgerSection, TokenEstimator, TurnId,
            WhyIncluded,
        };

        let temporary = tempfile::tempdir().unwrap();
        let database = temporary.path().join("sessions.db");
        let session_id = SessionId::new();
        let turn_id = TurnId::new();
        let entry = ContextLedgerEntry {
            turn_id,
            session_id,
            sections: vec![
                ContextLedgerSection {
                    class: ContextClass::Instructions,
                    label: "developer_instructions".into(),
                    estimated_tokens: 120,
                    byte_len: 480,
                    why_included: WhyIncluded::AlwaysPresent,
                },
                ContextLedgerSection {
                    class: ContextClass::RetrievedContext,
                    label: "retrieved_context".into(),
                    estimated_tokens: 42,
                    byte_len: 168,
                    why_included: WhyIncluded::MatchedQuery {
                        term: "replay integrity".into(),
                    },
                },
            ],
            total_estimated_tokens: 162,
            estimator: TokenEstimator::CharDiv4,
            recorded_at: Utc::now(),
        };

        {
            let mut store = SessionStore::open(&database).unwrap();
            store
                .append(
                    session_id,
                    &SessionEvent::SessionCreated {
                        objective: "preserve context ledger replay".into(),
                        repository: PathBuf::from("/repo"),
                        authority_mode: Default::default(),
                    },
                )
                .unwrap();
            store
                .append(
                    session_id,
                    &SessionEvent::ContextAssembled {
                        entry: entry.clone(),
                    },
                )
                .unwrap();
            // Loading before reopening the store must already reflect the
            // entry — replay parity is not only about surviving a restart.
            let state = store.load(session_id).unwrap();
            assert_eq!(state.recent_context_ledger.back(), Some(&entry));
        }

        // Reopen the store fresh so `load`/`events` reconstruct the session
        // purely by replaying the persisted event log from scratch.
        let reopened = SessionStore::open(&database).unwrap();
        let replayed_state = reopened.load(session_id).unwrap();
        assert_eq!(replayed_state.recent_context_ledger.len(), 1);
        assert_eq!(replayed_state.recent_context_ledger.back(), Some(&entry));

        let replayed_events = reopened.events(session_id).unwrap();
        let replayed_entry = replayed_events
            .iter()
            .find_map(|event| match event {
                SessionEvent::ContextAssembled { entry } => Some(entry.clone()),
                _ => None,
            })
            .expect("a ContextAssembled event survives replay");
        assert_eq!(replayed_entry, entry);
    }

    #[test]
    fn session_meta_title_archive_pin_and_delete_are_durable() {
        let mut store = SessionStore::in_memory().unwrap();
        let session = SessionId::new();
        // No row yet: defaults to an objective title.
        assert_eq!(store.session_meta(session).unwrap().title, None);
        store
            .append(
                session,
                &SessionEvent::SessionCreated {
                    objective: "meta round trip".into(),
                    repository: PathBuf::from("/repo"),
                    authority_mode: Default::default(),
                },
            )
            .unwrap();

        store.set_session_title(session, "my project").unwrap();
        store.set_session_archived(session, true).unwrap();
        store.set_session_pinned(session, true).unwrap();
        store.set_session_deleted(session, true).unwrap();

        let meta = store.session_meta(session).unwrap();
        assert_eq!(meta.title.as_deref(), Some("my project"));
        assert!(meta.archived && meta.pinned && meta.deleted);
        assert!(store.set_session_title(session, "").is_err());

        // Reopen: metadata survives because it is not event-sourced.
        let reopened = SessionStore::in_memory().unwrap();
        reopened
            .connection
            .execute(
                "INSERT INTO session_meta(session_id, title, archived, pinned, parent_id, deleted, created_at, updated_at)
                 VALUES (?1, 'my project', 1, 1, NULL, 1, ?2, ?2)",
                params![session.0.to_string(), Utc::now()],
            )
            .unwrap();
        let meta = reopened.session_meta(session).unwrap();
        assert_eq!(meta.title.as_deref(), Some("my project"));
        assert!(meta.archived && meta.pinned && meta.deleted);
    }

    #[test]
    fn session_search_indexes_appends_and_returns_snippets() {
        let mut store = SessionStore::in_memory().unwrap();
        let session = SessionId::new();
        store
            .append(
                session,
                &SessionEvent::SessionCreated {
                    objective: "search finds the health endpoint work".into(),
                    repository: PathBuf::from("/repo"),
                    authority_mode: Default::default(),
                },
            )
            .unwrap();
        store
            .append(
                session,
                &SessionEvent::ConversationMessageAdded {
                    message: purrcode_runtime_core::ConversationMessage {
                        id: Uuid::new_v4().to_string(),
                        role: "user".into(),
                        content: "fix the flaky login test".into(),
                        timestamp: Utc::now(),
                        tool_calls: Vec::new(),
                        tool_results: Vec::new(),
                        model: None,
                        turn_id: None,
                    },
                },
            )
            .unwrap();

        let hits = store.search_sessions("flaky", 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].session_id, session);
        assert!(hits[0].snippet.contains("flaky"));
        assert!(store.search_sessions("", 10).is_err());
    }

    #[test]
    fn fork_session_events_copies_prefix_and_links_parent() {
        let mut store = SessionStore::in_memory().unwrap();
        let parent = SessionId::new();
        store
            .append(
                parent,
                &SessionEvent::SessionCreated {
                    objective: "parent session".into(),
                    repository: PathBuf::from("/repo"),
                    authority_mode: Default::default(),
                },
            )
            .unwrap();
        let first_turn = SessionEvent::ConversationMessageAdded {
            message: purrcode_runtime_core::ConversationMessage {
                id: Uuid::new_v4().to_string(),
                role: "user".into(),
                content: "first message".into(),
                timestamp: Utc::now(),
                tool_calls: Vec::new(),
                tool_results: Vec::new(),
                model: None,
                turn_id: None,
            },
        };
        store.append(parent, &first_turn).unwrap();
        store
            .append(
                parent,
                &SessionEvent::SessionPaused {
                    reason: "work in progress".into(),
                },
            )
            .unwrap();

        let child = SessionId::new();
        let copied = store.fork_session_events(parent, child, 1).unwrap();
        assert_eq!(copied, 1);
        let meta = store.session_meta(child).unwrap();
        assert_eq!(meta.parent_id, Some(parent));
        let child_events = store.events(child).unwrap();
        assert_eq!(child_events.len(), 1);
        assert!(matches!(
            child_events[0],
            SessionEvent::ConversationMessageAdded { .. }
        ));
        // The parent keeps its full log.
        assert_eq!(store.events(parent).unwrap().len(), 3);
    }

    #[test]
    fn checkpoints_are_persisted_and_restorable() {
        let mut store = SessionStore::in_memory().unwrap();
        let session = SessionId::new();
        store
            .append(
                session,
                &SessionEvent::SessionCreated {
                    objective: "checkpoint round trip".into(),
                    repository: PathBuf::from("/repo"),
                    authority_mode: Default::default(),
                },
            )
            .unwrap();
        store
            .append(
                session,
                &SessionEvent::CheckpointCreated {
                    label: "turn".into(),
                    head: "abc123".into(),
                    patch_digest: "deadbeef".into(),
                },
            )
            .unwrap();

        let patch = b"diff --git a/a.rs b/a.rs\nnew file mode 100644\n";
        store
            .insert_checkpoint(&SessionCheckpoint {
                id: Uuid::new_v4(),
                session_id: session,
                sequence: 2,
                label: "turn".into(),
                head: "abc123".into(),
                patch: patch.to_vec(),
                patch_digest: "deadbeef".into(),
                created_at: Utc::now(),
            })
            .unwrap();
        assert_eq!(store.checkpoints(session).unwrap().len(), 1);
        let loaded = store.checkpoints(session).unwrap().remove(0);
        assert_eq!(loaded.patch, patch);
        assert_eq!(loaded.patch_digest, "deadbeef");
    }

    #[test]
    fn copy_checkpoints_rekeys_to_child() {
        let mut store = SessionStore::in_memory().unwrap();
        let parent = SessionId::new();
        store
            .append(
                parent,
                &SessionEvent::SessionCreated {
                    objective: "parent".into(),
                    repository: PathBuf::from("/repo"),
                    authority_mode: Default::default(),
                },
            )
            .unwrap();
        store
            .insert_checkpoint(&SessionCheckpoint {
                id: Uuid::new_v4(),
                session_id: parent,
                sequence: 2,
                label: "turn".into(),
                head: "abc".into(),
                patch: b"patch-bytes".to_vec(),
                patch_digest: "d1".into(),
                created_at: Utc::now(),
            })
            .unwrap();
        let child = SessionId::new();
        store.copy_checkpoints(parent, child).unwrap();
        let child_checkpoints = store.checkpoints(child).unwrap();
        assert_eq!(child_checkpoints.len(), 1);
        assert_eq!(child_checkpoints[0].session_id, child);
        assert_eq!(child_checkpoints[0].patch, b"patch-bytes");
    }

    #[test]
    fn project_memory_is_repository_scoped_and_auditable() {
        let mut store = SessionStore::in_memory().unwrap();
        let repository = std::path::PathBuf::from("/repo");
        store
            .insert_memory(&ProjectMemoryEntry {
                id: Uuid::new_v4(),
                repository: repository.clone(),
                kind: "build".into(),
                content: "Integration tests require Redis".into(),
                source: "Session \"Fix auth test\"".into(),
                confidence: "unverified".into(),
                scope: "repository".into(),
                created_at: Utc::now(),
                last_used_at: None,
            })
            .unwrap();
        store
            .insert_memory(&ProjectMemoryEntry {
                id: Uuid::new_v4(),
                repository: repository.clone(),
                kind: "architecture".into(),
                content: "Auth uses middleware pipeline".into(),
                source: "docs/architecture.md".into(),
                confidence: "unverified".into(),
                scope: "repository".into(),
                created_at: Utc::now(),
                last_used_at: None,
            })
            .unwrap();

        // Scoped to repository.
        let other_repo = std::path::PathBuf::from("/other");
        assert!(store.memory(&other_repo, None).unwrap().is_empty());
        assert_eq!(store.memory(&repository, None).unwrap().len(), 2);
        // Filtered by kind.
        assert_eq!(store.memory(&repository, Some("build")).unwrap().len(), 1);

        // Edit + touch + forget round trip.
        let entry = store.memory(&repository, Some("build")).unwrap().remove(0);
        store
            .update_memory_content(
                entry.id,
                "Integration tests require a Redis-compatible store",
            )
            .unwrap();
        assert!(
            store
                .memory_entry(entry.id)
                .unwrap()
                .content
                .starts_with("Integration tests require")
        );
        store.touch_memory(entry.id).unwrap();
        assert!(store.memory_entry(entry.id).unwrap().last_used_at.is_some());
        store.forget_memory(entry.id).unwrap();
        assert!(matches!(
            store.memory_entry(entry.id),
            Err(StoreError::MemoryNotFound(_))
        ));
    }
}
