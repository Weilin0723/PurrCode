//! Persistent skill library with global, repository, and session scopes.

use chrono::{DateTime, Utc};
use purrcode_runtime_core::QualificationStatus;
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use std::cmp::Reverse;
use std::path::{Path, PathBuf};
use thiserror::Error;
use uuid::Uuid;

const MAX_SKILL_FILES: u64 = 10_000;
const MAX_SKILL_BYTES: u64 = 100 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SkillRecord {
    pub skill_id: String,
    pub version: String,
    pub scope: SkillScope,
    pub source_type: String,
    pub source_location: Option<String>,
    pub publisher: Option<String>,
    pub content_digest: String,
    pub signature_status: String,
    pub installed_at: DateTime<Utc>,
    pub approved_permissions: serde_json::Value,
    pub qualification_status: QualificationStatus,
    pub last_used_at: Option<DateTime<Utc>>,
    pub successful_uses: u64,
    pub failed_uses: u64,
    pub pinned: bool,
    /// A user can disable a skill without uninstalling it. Disabled skills are
    /// installed and inspectable but never invoked.
    pub enabled: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillScope {
    User,
    Repository,
    Session,
}

impl std::fmt::Display for SkillScope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SkillScope::User => write!(f, "user"),
            SkillScope::Repository => write!(f, "repository"),
            SkillScope::Session => write!(f, "session"),
        }
    }
}

impl SkillScope {
    pub fn from_str_name(s: &str) -> Option<Self> {
        match s {
            "user" => Some(Self::User),
            "repository" => Some(Self::Repository),
            "session" => Some(Self::Session),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SkillStoreEntry {
    pub path: PathBuf,
    pub record: SkillRecord,
}

/// Result of the mandatory installed-skill lookup that precedes any external search.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct InstalledCapabilityResolution {
    pub capability: String,
    /// Only dynamically qualified skills are eligible for reuse.
    pub qualified_matches: Vec<SkillRecord>,
    /// Matching records that fail closed because they are not currently invocable.
    pub non_invocable_matches: Vec<SkillRecord>,
    /// Explicit audit signal for the caller. `true` means no registry/web adapter should run.
    pub external_search_avoided: bool,
}

impl InstalledCapabilityResolution {
    pub fn requires_external_search(&self) -> bool {
        !self.external_search_avoided
    }
}

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("SQLite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("skill not found: {0}")]
    NotFound(String),
    #[error("skill already installed: {0}")]
    AlreadyInstalled(String),
    #[error("invalid scope: {0}")]
    InvalidScope(String),
    #[error("invalid publisher: {0}")]
    InvalidPublisher(String),
    #[error("invalid capability: {0}")]
    InvalidCapability(String),
    #[error("invalid skill package: {0}")]
    InvalidPackage(String),
    #[error("skill content digest mismatch: expected {expected}, observed {observed}")]
    DigestMismatch { expected: String, observed: String },
    #[error("skill must pass dynamic qualification before qualified install: {0:?}")]
    QualificationRequired(QualificationStatus),
}

pub struct SkillStore {
    conn: Connection,
    library_root: PathBuf,
}

impl SkillStore {
    pub fn open(database: &Path, library_root: &Path) -> Result<Self, StoreError> {
        std::fs::create_dir_all(library_root)?;
        let conn = Connection::open(database)?;
        let store = Self {
            conn,
            library_root: library_root.to_owned(),
        };
        store.migrate()?;
        Ok(store)
    }

    fn migrate(&self) -> Result<(), StoreError> {
        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS skill_store (
                skill_id TEXT NOT NULL,
                version TEXT NOT NULL,
                scope TEXT NOT NULL,
                source_type TEXT NOT NULL,
                source_location TEXT,
                publisher TEXT,
                content_digest TEXT NOT NULL,
                signature_status TEXT NOT NULL DEFAULT 'unavailable',
                installed_at TEXT NOT NULL,
                approved_permissions TEXT NOT NULL DEFAULT '{}',
                qualification_status TEXT NOT NULL DEFAULT 'unverified',
                last_used_at TEXT,
                successful_uses INTEGER NOT NULL DEFAULT 0,
                failed_uses INTEGER NOT NULL DEFAULT 0,
                pinned INTEGER NOT NULL DEFAULT 0,
                enabled INTEGER NOT NULL DEFAULT 1,
                PRIMARY KEY (skill_id, scope)
            );
            CREATE TABLE IF NOT EXISTS blocked_publishers (
                publisher TEXT PRIMARY KEY,
                blocked_at TEXT NOT NULL,
                reason TEXT NOT NULL
            );",
        )?;
        // Older databases lack the enabled column; add it, defaulting to
        // enabled so existing installs stay invocable.
        let has_enabled = self
            .conn
            .prepare("PRAGMA table_info(skill_store)")?
            .query_map([], |row| Ok(row.get::<_, String>(1)?))?
            .filter_map(Result::ok)
            .any(|name| name == "enabled");
        if !has_enabled {
            self.conn
                .execute_batch("ALTER TABLE skill_store ADD COLUMN enabled INTEGER NOT NULL DEFAULT 1;")?;
        }
        let scope_is_key = self
            .conn
            .prepare("PRAGMA table_info(skill_store)")?
            .query_map([], |row| {
                Ok((row.get::<_, String>(1)?, row.get::<_, i64>(5)?))
            })?
            .filter_map(Result::ok)
            .any(|(name, key_order)| name == "scope" && key_order > 0);
        if !scope_is_key {
            self.conn.execute_batch(
                "BEGIN IMMEDIATE;
                 ALTER TABLE skill_store RENAME TO skill_store_v1;
                 CREATE TABLE skill_store (
                    skill_id TEXT NOT NULL, version TEXT NOT NULL, scope TEXT NOT NULL,
                    source_type TEXT NOT NULL, source_location TEXT, publisher TEXT,
                    content_digest TEXT NOT NULL, signature_status TEXT NOT NULL DEFAULT 'unavailable',
                    installed_at TEXT NOT NULL, approved_permissions TEXT NOT NULL DEFAULT '{}',
                    qualification_status TEXT NOT NULL DEFAULT 'unverified', last_used_at TEXT,
                    successful_uses INTEGER NOT NULL DEFAULT 0, failed_uses INTEGER NOT NULL DEFAULT 0,
                    pinned INTEGER NOT NULL DEFAULT 0, enabled INTEGER NOT NULL DEFAULT 1,
                    PRIMARY KEY (skill_id, scope));
                 INSERT INTO skill_store (skill_id, version, scope, source_type, source_location,
                    publisher, content_digest, signature_status, installed_at, approved_permissions,
                    qualification_status, last_used_at, successful_uses, failed_uses, pinned, enabled)
                 SELECT skill_id, version, scope, source_type, source_location,
                    publisher, content_digest, signature_status, installed_at, approved_permissions,
                    qualification_status, last_used_at, successful_uses, failed_uses, pinned, 1
                 FROM skill_store_v1;
                 DROP TABLE skill_store_v1;
                 COMMIT;",
            )?;
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn install(
        &mut self,
        skill_id: &str,
        version: &str,
        scope: SkillScope,
        source_type: &str,
        source_location: Option<&str>,
        publisher: Option<&str>,
        content_digest: &str,
        approved_permissions: &serde_json::Value,
        source_path: &Path,
    ) -> Result<SkillRecord, StoreError> {
        let observed_digest = skill_content_digest(source_path)?;
        if observed_digest != content_digest {
            return Err(StoreError::DigestMismatch {
                expected: content_digest.to_owned(),
                observed: observed_digest,
            });
        }
        if self
            .conn
            .query_row(
                "SELECT 1 FROM skill_store WHERE skill_id = ?1 AND scope = ?2",
                params![skill_id, scope.to_string()],
                |_| Ok(()),
            )
            .is_ok()
        {
            return Err(StoreError::AlreadyInstalled(skill_id.to_string()));
        }

        let scope_str = scope.to_string();
        let scope_root = self.scope_root(&scope);
        std::fs::create_dir_all(&scope_root)?;
        let dest = scope_root.join(skill_id);
        if dest.exists() {
            return Err(StoreError::AlreadyInstalled(format!(
                "{skill_id} ({scope})"
            )));
        }
        let staging = scope_root.join(format!(".{skill_id}.install-{}", Uuid::new_v4()));
        std::fs::create_dir(&staging)?;
        if let Err(error) = Self::copy_dir(source_path, &staging) {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(error);
        }
        let staged_digest = match skill_content_digest(&staging) {
            Ok(digest) => digest,
            Err(error) => {
                let _ = std::fs::remove_dir_all(&staging);
                return Err(error);
            }
        };
        if staged_digest != content_digest {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(StoreError::DigestMismatch {
                expected: content_digest.to_owned(),
                observed: staged_digest,
            });
        }
        std::fs::rename(&staging, &dest)?;

        let now = Utc::now();
        let perms_json = serde_json::to_string(approved_permissions)?;

        if let Err(error) = self.conn.execute(
            "INSERT INTO skill_store
                (skill_id, version, scope, source_type, source_location, publisher,
                 content_digest, signature_status, installed_at, approved_permissions,
                 qualification_status, successful_uses, failed_uses, pinned, enabled)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'unavailable', ?8, ?9, 'unverified', 0, 0, 0, 1)",
            params![
                skill_id,
                version,
                scope_str,
                source_type,
                source_location,
                publisher,
                content_digest,
                now.to_rfc3339(),
                perms_json,
            ],
        ) {
            let trash = self.library_root.join(".trash");
            std::fs::create_dir_all(&trash)?;
            std::fs::rename(
                &dest,
                trash.join(format!("failed-{skill_id}-{}", Uuid::new_v4())),
            )?;
            return Err(error.into());
        }

        Ok(SkillRecord {
            skill_id: skill_id.to_string(),
            version: version.to_string(),
            scope,
            source_type: source_type.to_string(),
            source_location: source_location.map(String::from),
            publisher: publisher.map(String::from),
            content_digest: content_digest.to_string(),
            signature_status: "unavailable".into(),
            installed_at: now,
            approved_permissions: approved_permissions.clone(),
            qualification_status: QualificationStatus::Unverified,
            last_used_at: None,
            successful_uses: 0,
            failed_uses: 0,
            pinned: false,
            enabled: true,
        })
    }

    /// Install a package only after dynamic qualification has reached an invocable state.
    ///
    /// The package is copied and its digest is rechecked by [`Self::install`]. The initial
    /// unverified record is fail-closed and is promoted only after the copy succeeds.
    #[allow(clippy::too_many_arguments)]
    pub fn install_qualified(
        &mut self,
        skill_id: &str,
        version: &str,
        scope: SkillScope,
        source_type: &str,
        source_location: Option<&str>,
        publisher: Option<&str>,
        content_digest: &str,
        approved_permissions: &serde_json::Value,
        source_path: &Path,
        qualification_status: &QualificationStatus,
    ) -> Result<SkillRecord, StoreError> {
        if !qualification_is_invocable(qualification_status) {
            return Err(StoreError::QualificationRequired(
                qualification_status.clone(),
            ));
        }
        let mut record = self.install(
            skill_id,
            version,
            scope,
            source_type,
            source_location,
            publisher,
            content_digest,
            approved_permissions,
            source_path,
        )?;
        self.update_qualification(skill_id, qualification_status)?;
        record.qualification_status = qualification_status.clone();
        Ok(record)
    }

    pub fn remove(&mut self, skill_id: &str) -> Result<SkillRecord, StoreError> {
        let record = self.get(skill_id)?;

        let dest = self.scope_root(&record.scope).join(skill_id);
        if dest.exists() {
            let trash = self.library_root.join(".trash");
            std::fs::create_dir_all(&trash)?;
            let trash_name = format!("{}-{}-{}", skill_id, record.version, Uuid::new_v4());
            std::fs::rename(&dest, trash.join(&trash_name))?;
        }

        self.conn.execute(
            "DELETE FROM skill_store WHERE skill_id = ?1",
            params![skill_id],
        )?;

        Ok(record)
    }

    pub fn list(&self) -> Result<Vec<SkillRecord>, StoreError> {
        let mut stmt = self.conn.prepare(
            "SELECT skill_id, version, scope, source_type, source_location, publisher,
                    content_digest, signature_status, installed_at, approved_permissions,
                    qualification_status, last_used_at, successful_uses, failed_uses, pinned, enabled
             FROM skill_store
             ORDER BY installed_at DESC",
        )?;

        let rows = stmt.query_map([], |row| {
            let scope_str: String = row.get(2)?;
            let perms_json: String = row.get(9)?;
            let status_str: String = row.get(10)?;
            let qual_status = match status_str.as_str() {
                "qualified" => QualificationStatus::Qualified,
                "qualified_with_constraints" => QualificationStatus::QualifiedWithConstraints,
                "failed" => QualificationStatus::Failed,
                "blocked" => QualificationStatus::Blocked,
                "outdated" => QualificationStatus::Outdated,
                "incompatible" => QualificationStatus::Incompatible,
                _ => QualificationStatus::Unverified,
            };

            Ok(SkillRecord {
                skill_id: row.get(0)?,
                version: row.get(1)?,
                scope: SkillScope::from_str_name(&scope_str).unwrap_or(SkillScope::User),
                source_type: row.get(3)?,
                source_location: row.get(4)?,
                publisher: row.get(5)?,
                content_digest: row.get(6)?,
                signature_status: row.get(7)?,
                installed_at: row
                    .get::<_, String>(8)?
                    .parse()
                    .unwrap_or_else(|_| Utc::now()),
                approved_permissions: serde_json::from_str(&perms_json).unwrap_or_default(),
                qualification_status: qual_status,
                last_used_at: row
                    .get::<_, Option<String>>(11)?
                    .and_then(|s| s.parse().ok()),
                successful_uses: row.get::<_, i64>(12)? as u64,
                failed_uses: row.get::<_, i64>(13)? as u64,
                pinned: row.get::<_, i64>(14)? != 0,
                enabled: row.get::<_, i64>(15)? != 0,
            })
        })?;

        let mut records = Vec::new();
        for row in rows {
            records.push(row?);
        }
        Ok(records)
    }

    pub fn get(&self, skill_id: &str) -> Result<SkillRecord, StoreError> {
        let mut stmt = self.conn.prepare(
            "SELECT skill_id, version, scope, source_type, source_location, publisher,
                    content_digest, signature_status, installed_at, approved_permissions,
                    qualification_status, last_used_at, successful_uses, failed_uses, pinned, enabled
             FROM skill_store WHERE skill_id = ?1
             ORDER BY CASE scope WHEN 'session' THEN 0 WHEN 'repository' THEN 1 ELSE 2 END
             LIMIT 1",
        )?;

        stmt.query_row(params![skill_id], |row| {
            let scope_str: String = row.get(2)?;
            let perms_json: String = row.get(9)?;
            let status_str: String = row.get(10)?;
            let qual_status = match status_str.as_str() {
                "qualified" => QualificationStatus::Qualified,
                "qualified_with_constraints" => QualificationStatus::QualifiedWithConstraints,
                "failed" => QualificationStatus::Failed,
                "blocked" => QualificationStatus::Blocked,
                "outdated" => QualificationStatus::Outdated,
                "incompatible" => QualificationStatus::Incompatible,
                _ => QualificationStatus::Unverified,
            };

            Ok(SkillRecord {
                skill_id: row.get(0)?,
                version: row.get(1)?,
                scope: SkillScope::from_str_name(&scope_str).unwrap_or(SkillScope::User),
                source_type: row.get(3)?,
                source_location: row.get(4)?,
                publisher: row.get(5)?,
                content_digest: row.get(6)?,
                signature_status: row.get(7)?,
                installed_at: row
                    .get::<_, String>(8)?
                    .parse()
                    .unwrap_or_else(|_| Utc::now()),
                approved_permissions: serde_json::from_str(&perms_json).unwrap_or_default(),
                qualification_status: qual_status,
                last_used_at: row
                    .get::<_, Option<String>>(11)?
                    .and_then(|s| s.parse().ok()),
                successful_uses: row.get::<_, i64>(12)? as u64,
                failed_uses: row.get::<_, i64>(13)? as u64,
                pinned: row.get::<_, i64>(14)? != 0,
                enabled: row.get::<_, i64>(15)? != 0,
            })
        })
        .map_err(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => StoreError::NotFound(skill_id.to_string()),
            other => StoreError::Sqlite(other),
        })
    }

    pub fn find_by_capability(&self, capability: &str) -> Result<Vec<SkillRecord>, StoreError> {
        let trimmed = capability.trim().to_lowercase();
        if trimmed.is_empty() {
            return Err(StoreError::InvalidCapability(
                "capability cannot be empty".into(),
            ));
        }
        let all = self.list()?;
        let mut matches = all
            .into_iter()
            .filter(|s| {
                let id = s.skill_id.to_lowercase();
                let src = s.source_type.to_lowercase();
                let publisher = s.publisher.as_deref().unwrap_or_default().to_lowercase();
                id.contains(&trimmed) || src.contains(&trimmed) || publisher.contains(&trimmed)
            })
            .collect::<Vec<_>>();
        matches.sort_by_key(|record| {
            (
                scope_priority(&record.scope),
                Reverse(record.pinned),
                Reverse(record.successful_uses),
                record.skill_id.clone(),
            )
        });
        Ok(matches)
    }

    /// Resolve installed capabilities before a caller is allowed to consider registry/web search.
    ///
    /// `external_search_avoided` is deliberately part of the returned evidence so callers can
    /// durably assert that no external adapter was needed.
    pub fn resolve_installed_capability(
        &self,
        capability: &str,
    ) -> Result<InstalledCapabilityResolution, StoreError> {
        let matches = self.find_by_capability(capability)?;
        let (qualified_matches, non_invocable_matches): (Vec<_>, Vec<_>) = matches
            .into_iter()
            // A disabled skill is installed but never invoked, so it must not
            // count as an invocable match (and must not suppress external
            // search either).
            .partition(|record| record.enabled && qualification_is_invocable(&record.qualification_status));
        let external_search_avoided = !qualified_matches.is_empty();
        Ok(InstalledCapabilityResolution {
            capability: capability.trim().to_owned(),
            qualified_matches,
            non_invocable_matches,
            external_search_avoided,
        })
    }

    pub fn record_use(&mut self, skill_id: &str, success: bool) -> Result<(), StoreError> {
        let now = Utc::now().to_rfc3339();
        if success {
            self.conn.execute(
                "UPDATE skill_store SET last_used_at = ?1, successful_uses = successful_uses + 1 WHERE skill_id = ?2",
                params![now, skill_id],
            )?;
        } else {
            self.conn.execute(
                "UPDATE skill_store SET last_used_at = ?1, failed_uses = failed_uses + 1 WHERE skill_id = ?2",
                params![now, skill_id],
            )?;
        }
        Ok(())
    }

    pub fn update_qualification(
        &mut self,
        skill_id: &str,
        status: &QualificationStatus,
    ) -> Result<(), StoreError> {
        let status_str = match status {
            QualificationStatus::Qualified => "qualified",
            QualificationStatus::QualifiedWithConstraints => "qualified_with_constraints",
            QualificationStatus::Unverified => "unverified",
            QualificationStatus::Failed => "failed",
            QualificationStatus::Blocked => "blocked",
            QualificationStatus::Outdated => "outdated",
            QualificationStatus::Incompatible => "incompatible",
        };
        self.conn.execute(
            "UPDATE skill_store SET qualification_status = ?1 WHERE skill_id = ?2",
            params![status_str, skill_id],
        )?;
        Ok(())
    }

    pub fn update_signature_status(
        &mut self,
        skill_id: &str,
        scope: &SkillScope,
        status: &str,
    ) -> Result<(), StoreError> {
        self.conn.execute(
            "UPDATE skill_store SET signature_status = ?1 WHERE skill_id = ?2 AND scope = ?3",
            params![status, skill_id, scope.to_string()],
        )?;
        Ok(())
    }

    pub fn pin(&mut self, skill_id: &str, pinned: bool) -> Result<(), StoreError> {
        self.conn.execute(
            "UPDATE skill_store SET pinned = ?1 WHERE skill_id = ?2",
            params![pinned as i64, skill_id],
        )?;
        Ok(())
    }

    /// Enables or disables an installed skill without uninstalling it. A
    /// disabled skill is inspectable but never invoked.
    pub fn set_enabled(&mut self, skill_id: &str, enabled: bool) -> Result<(), StoreError> {
        let changed = self.conn.execute(
            "UPDATE skill_store SET enabled = ?1 WHERE skill_id = ?2",
            params![enabled as i64, skill_id],
        )?;
        if changed == 0 {
            return Err(StoreError::NotFound(skill_id.to_string()));
        }
        Ok(())
    }

    /// Whether an installed skill is enabled. Skills missing from the store
    /// fail closed (not found, not silently enabled).
    pub fn is_enabled(&self, skill_id: &str) -> Result<bool, StoreError> {
        let enabled: i64 = self
            .conn
            .query_row(
                "SELECT enabled FROM skill_store WHERE skill_id = ?1
                 ORDER BY CASE scope WHEN 'session' THEN 0 WHEN 'repository' THEN 1 ELSE 2 END
                 LIMIT 1",
                params![skill_id],
                |row| row.get(0),
            )
            .map_err(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => StoreError::NotFound(skill_id.to_string()),
                other => StoreError::Sqlite(other),
            })?;
        Ok(enabled != 0)
    }

    pub fn path_for(&self, skill_id: &str, scope: &SkillScope) -> PathBuf {
        self.scope_root(scope).join(skill_id)
    }

    pub fn block_publisher(&mut self, publisher: &str, reason: &str) -> Result<(), StoreError> {
        let publisher = publisher.trim().to_ascii_lowercase();
        if publisher.is_empty() {
            return Err(StoreError::InvalidPublisher(
                "publisher cannot be empty".into(),
            ));
        }
        self.conn.execute(
            "INSERT INTO blocked_publishers(publisher, blocked_at, reason) VALUES (?1, ?2, ?3)
             ON CONFLICT(publisher) DO UPDATE SET blocked_at = excluded.blocked_at, reason = excluded.reason",
            params![publisher, Utc::now().to_rfc3339(), reason],
        )?;
        Ok(())
    }

    pub fn is_publisher_blocked(&self, publisher: &str) -> Result<bool, StoreError> {
        Ok(self
            .conn
            .query_row(
                "SELECT 1 FROM blocked_publishers WHERE publisher = ?1",
                params![publisher.trim().to_ascii_lowercase()],
                |_| Ok(()),
            )
            .is_ok())
    }

    fn scope_root(&self, scope: &SkillScope) -> PathBuf {
        match scope {
            SkillScope::User => self.library_root.join("user"),
            SkillScope::Repository => self.library_root.join("repository"),
            SkillScope::Session => self.library_root.join("session"),
        }
    }

    fn copy_dir(src: &Path, dst: &Path) -> Result<(), StoreError> {
        fn visit(
            src: &Path,
            dst: &Path,
            files: &mut u64,
            bytes: &mut u64,
        ) -> Result<(), StoreError> {
            for entry in std::fs::read_dir(src)? {
                let entry = entry?;
                let metadata = std::fs::symlink_metadata(entry.path())?;
                if metadata.file_type().is_symlink() {
                    return Err(StoreError::InvalidPackage(
                        "symbolic links are forbidden".into(),
                    ));
                }
                let dst_path = dst.join(entry.file_name());
                if metadata.is_dir() {
                    std::fs::create_dir_all(&dst_path)?;
                    visit(&entry.path(), &dst_path, files, bytes)?;
                } else if metadata.is_file() {
                    *files = files.saturating_add(1);
                    *bytes = bytes.saturating_add(metadata.len());
                    if *files > MAX_SKILL_FILES || *bytes > MAX_SKILL_BYTES {
                        return Err(StoreError::InvalidPackage(
                            "package exceeds file-count or byte limit".into(),
                        ));
                    }
                    std::fs::copy(entry.path(), dst_path)?;
                } else {
                    return Err(StoreError::InvalidPackage(
                        "package contains an unsupported filesystem entry".into(),
                    ));
                }
            }
            Ok(())
        }

        let mut files = 0;
        let mut bytes = 0;
        visit(src, dst, &mut files, &mut bytes)
    }
}

fn qualification_is_invocable(status: &QualificationStatus) -> bool {
    matches!(
        status,
        QualificationStatus::Qualified | QualificationStatus::QualifiedWithConstraints
    )
}

fn scope_priority(scope: &SkillScope) -> u8 {
    match scope {
        SkillScope::Session => 0,
        SkillScope::Repository => 1,
        SkillScope::User => 2,
    }
}

/// Compute the deterministic digest bound by download, qualification, and install approvals.
///
/// Paths and bytes are both included, directory traversal is sorted, symbolic links are rejected,
/// and the install metadata file is excluded so integrity can be rechecked after installation.
pub fn skill_content_digest(root: &Path) -> Result<String, StoreError> {
    fn collect(
        root: &Path,
        current: &Path,
        output: &mut Vec<PathBuf>,
        files: &mut u64,
        bytes: &mut u64,
    ) -> Result<(), StoreError> {
        for entry in std::fs::read_dir(current)? {
            let entry = entry?;
            let path = entry.path();
            let metadata = std::fs::symlink_metadata(&path)?;
            if metadata.file_type().is_symlink() {
                return Err(StoreError::InvalidPackage(
                    "symbolic links are forbidden".into(),
                ));
            }
            if metadata.is_dir() {
                collect(root, &path, output, files, bytes)?;
            } else if metadata.is_file()
                && path.file_name().and_then(|name| name.to_str()) != Some(".purrcode-install.json")
            {
                *files = files.saturating_add(1);
                *bytes = bytes.saturating_add(metadata.len());
                if *files > MAX_SKILL_FILES || *bytes > MAX_SKILL_BYTES {
                    return Err(StoreError::InvalidPackage(
                        "package exceeds file-count or byte limit".into(),
                    ));
                }
                let relative = path
                    .strip_prefix(root)
                    .map_err(|_| StoreError::InvalidPackage("path escaped package root".into()))?;
                if relative.to_str().is_none() {
                    return Err(StoreError::InvalidPackage(
                        "package paths must be valid UTF-8".into(),
                    ));
                }
                output.push(relative.to_owned());
            } else if !metadata.is_file() {
                return Err(StoreError::InvalidPackage(
                    "package contains an unsupported filesystem entry".into(),
                ));
            }
        }
        Ok(())
    }

    let root_metadata = std::fs::symlink_metadata(root)?;
    if root_metadata.file_type().is_symlink() || !root_metadata.is_dir() {
        return Err(StoreError::InvalidPackage(
            "skill package root must be a real directory".into(),
        ));
    }
    let mut paths = Vec::new();
    let mut files = 0;
    let mut bytes = 0;
    collect(root, root, &mut paths, &mut files, &mut bytes)?;
    paths.sort();
    let mut hasher = blake3::Hasher::new();
    for relative in paths {
        hasher.update(relative.to_string_lossy().as_bytes());
        hasher.update(&[0]);
        hasher.update(&std::fs::read(root.join(&relative))?);
        hasher.update(&[0]);
    }
    Ok(hasher.finalize().to_hex().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn install_list_remove_cycle() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("skills.db");
        let lib = dir.path().join("library");
        let mut store = SkillStore::open(&db, &lib).unwrap();

        let skill_dir = dir.path().join("source-skill");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(skill_dir.join("SKILL.md"), "# Test Skill").unwrap();
        std::fs::write(skill_dir.join("tool.py"), "print('hello')").unwrap();

        let perms = json!({"read": ["**/*.tf"]});
        let digest = skill_content_digest(&skill_dir).unwrap();

        let record = store
            .install(
                "test-skill",
                "1.0.0",
                SkillScope::User,
                "local",
                None,
                None,
                &digest,
                &perms,
                &skill_dir,
            )
            .unwrap();

        assert_eq!(record.skill_id, "test-skill");
        assert_eq!(record.version, "1.0.0");
        assert_eq!(record.qualification_status, QualificationStatus::Unverified);

        let all = store.list().unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].skill_id, "test-skill");

        assert!(
            store
                .path_for("test-skill", &SkillScope::User)
                .join("SKILL.md")
                .exists()
        );
        assert!(
            store
                .path_for("test-skill", &SkillScope::User)
                .join("tool.py")
                .exists()
        );

        store.record_use("test-skill", true).unwrap();
        let updated = store.get("test-skill").unwrap();
        assert_eq!(updated.successful_uses, 1);

        store.remove("test-skill").unwrap();
        assert!(store.list().unwrap().is_empty());
    }

    #[test]
    fn duplicate_install_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = SkillStore::open(&dir.path().join("db"), &dir.path().join("lib")).unwrap();

        let skill_dir = dir.path().join("src");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(skill_dir.join("SKILL.md"), "# Skill").unwrap();
        let digest = skill_content_digest(&skill_dir).unwrap();

        store
            .install(
                "dup",
                "1.0",
                SkillScope::User,
                "local",
                None,
                None,
                &digest,
                &serde_json::json!({}),
                &skill_dir,
            )
            .unwrap();

        let err = store
            .install(
                "dup",
                "2.0",
                SkillScope::User,
                "local",
                None,
                None,
                &digest,
                &serde_json::json!({}),
                &skill_dir,
            )
            .unwrap_err();
        assert!(matches!(err, StoreError::AlreadyInstalled(_)));
    }

    #[test]
    fn same_skill_can_be_installed_in_distinct_scopes() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = SkillStore::open(&dir.path().join("db"), &dir.path().join("lib")).unwrap();
        let source = dir.path().join("source");
        std::fs::create_dir(&source).unwrap();
        std::fs::write(source.join("SKILL.md"), "# scoped").unwrap();
        let digest = skill_content_digest(&source).unwrap();
        for scope in [
            SkillScope::User,
            SkillScope::Repository,
            SkillScope::Session,
        ] {
            store
                .install(
                    "scoped",
                    "1.0.0",
                    scope,
                    "local",
                    None,
                    None,
                    &digest,
                    &serde_json::json!({}),
                    &source,
                )
                .unwrap();
        }
        assert_eq!(store.list().unwrap().len(), 3);
    }

    #[test]
    fn publisher_blocklist_is_case_insensitive_and_durable() {
        let dir = tempfile::tempdir().unwrap();
        let database = dir.path().join("db");
        let library = dir.path().join("lib");
        let mut store = SkillStore::open(&database, &library).unwrap();
        store
            .block_publisher("Untrusted-Publisher", "reviewed by user")
            .unwrap();
        assert!(store.is_publisher_blocked("untrusted-publisher").unwrap());
        drop(store);
        assert!(
            SkillStore::open(&database, &library)
                .unwrap()
                .is_publisher_blocked("UNTRUSTED-PUBLISHER")
                .unwrap()
        );
    }

    #[test]
    fn find_by_capability_matches() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = SkillStore::open(&dir.path().join("db"), &dir.path().join("lib")).unwrap();

        let sd = dir.path().join("s1");
        std::fs::create_dir_all(&sd).unwrap();
        std::fs::write(sd.join("SKILL.md"), "").unwrap();
        let digest = skill_content_digest(&sd).unwrap();

        store
            .install(
                "terraform-inspector",
                "1.0",
                SkillScope::User,
                "registry",
                None,
                Some("example"),
                &digest,
                &json!({}),
                &sd,
            )
            .unwrap();

        store
            .install(
                "k8s-debug",
                "1.0",
                SkillScope::User,
                "github",
                None,
                None,
                &digest,
                &json!({}),
                &sd,
            )
            .unwrap();

        let results = store.find_by_capability("terraform").unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].skill_id, "terraform-inspector");
    }

    #[test]
    fn installed_resolution_avoids_external_search_only_for_qualified_matches() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = SkillStore::open(&dir.path().join("db"), &dir.path().join("lib")).unwrap();
        let source = dir.path().join("source");
        std::fs::create_dir(&source).unwrap();
        std::fs::write(source.join("SKILL.md"), "# Pull request reader").unwrap();
        let digest = skill_content_digest(&source).unwrap();
        store
            .install(
                "github-pr-reader",
                "1.0.0",
                SkillScope::Repository,
                "local",
                None,
                Some("trusted"),
                &digest,
                &json!({"read": ["**/*"]}),
                &source,
            )
            .unwrap();

        let unverified = store.resolve_installed_capability("github-pr").unwrap();
        assert!(unverified.qualified_matches.is_empty());
        assert_eq!(unverified.non_invocable_matches.len(), 1);
        assert!(unverified.requires_external_search());

        store
            .update_qualification("github-pr-reader", &QualificationStatus::Qualified)
            .unwrap();
        let qualified = store.resolve_installed_capability("github-pr").unwrap();
        assert_eq!(qualified.qualified_matches.len(), 1);
        assert!(qualified.non_invocable_matches.is_empty());
        assert!(qualified.external_search_avoided);
        assert!(!qualified.requires_external_search());
    }

    #[test]
    fn disabled_skills_are_inspectable_but_never_invocable() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = SkillStore::open(&dir.path().join("db"), &dir.path().join("lib")).unwrap();
        let source = dir.path().join("source");
        std::fs::create_dir(&source).unwrap();
        std::fs::write(source.join("SKILL.md"), "# git review").unwrap();
        let digest = skill_content_digest(&source).unwrap();

        store
            .install(
                "git-review",
                "1.0.0",
                SkillScope::Repository,
                "local",
                None,
                Some("trusted"),
                &digest,
                &json!({"read": ["**/*"]}),
                &source,
            )
            .unwrap();
        store
            .update_qualification("git-review", &QualificationStatus::Qualified)
            .unwrap();

        // Fresh installs are enabled and invocable.
        assert!(store.is_enabled("git-review").unwrap());
        let enabled = store.resolve_installed_capability("git-review").unwrap();
        assert_eq!(enabled.qualified_matches.len(), 1);
        assert!(enabled.external_search_avoided);

        // Disabling removes it from invocable matches and re-enables external
        // search, but the record stays listed.
        store.set_enabled("git-review", false).unwrap();
        assert!(!store.is_enabled("git-review").unwrap());
        let disabled = store.resolve_installed_capability("git-review").unwrap();
        assert!(disabled.qualified_matches.is_empty());
        assert_eq!(disabled.non_invocable_matches.len(), 1);
        assert!(disabled.requires_external_search());
        assert_eq!(store.list().unwrap().len(), 1);

        store.set_enabled("git-review", true).unwrap();
        assert!(store.is_enabled("git-review").unwrap());
        assert!(store.set_enabled("missing-skill", true).is_err());
    }

    #[test]
    fn install_recomputes_and_rejects_a_mismatched_digest() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = SkillStore::open(&dir.path().join("db"), &dir.path().join("lib")).unwrap();
        let source = dir.path().join("source");
        std::fs::create_dir(&source).unwrap();
        std::fs::write(source.join("SKILL.md"), "# immutable").unwrap();

        let error = store
            .install(
                "digest-bound",
                "1.0.0",
                SkillScope::User,
                "registry",
                None,
                None,
                "not-the-observed-digest",
                &json!({}),
                &source,
            )
            .unwrap_err();
        assert!(matches!(error, StoreError::DigestMismatch { .. }));
        assert!(store.list().unwrap().is_empty());
    }

    #[test]
    fn qualified_install_fails_closed_for_every_non_invocable_status() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source");
        std::fs::create_dir(&source).unwrap();
        std::fs::write(source.join("SKILL.md"), "# qualification gate").unwrap();
        let digest = skill_content_digest(&source).unwrap();
        for (index, status) in [
            QualificationStatus::Unverified,
            QualificationStatus::Failed,
            QualificationStatus::Blocked,
            QualificationStatus::Outdated,
            QualificationStatus::Incompatible,
        ]
        .into_iter()
        .enumerate()
        {
            let mut store = SkillStore::open(
                &dir.path().join(format!("db-{index}")),
                &dir.path().join(format!("lib-{index}")),
            )
            .unwrap();
            let error = store
                .install_qualified(
                    "qualification-gated",
                    "1.0.0",
                    SkillScope::User,
                    "registry",
                    None,
                    None,
                    &digest,
                    &json!({}),
                    &source,
                    &status,
                )
                .unwrap_err();
            assert!(matches!(error, StoreError::QualificationRequired(_)));
        }
    }

    #[cfg(unix)]
    #[test]
    fn digest_and_install_reject_symbolic_links() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source");
        std::fs::create_dir(&source).unwrap();
        std::fs::write(source.join("SKILL.md"), "# no links").unwrap();
        symlink("/etc/passwd", source.join("escape")).unwrap();
        assert!(matches!(
            skill_content_digest(&source),
            Err(StoreError::InvalidPackage(_))
        ));

        let clean_source = dir.path().join("clean-source");
        let linked_root = dir.path().join("linked-root");
        std::fs::create_dir(&clean_source).unwrap();
        std::fs::write(clean_source.join("SKILL.md"), "# root link").unwrap();
        symlink(&clean_source, &linked_root).unwrap();
        assert!(matches!(
            skill_content_digest(&linked_root),
            Err(StoreError::InvalidPackage(_))
        ));
    }
}
