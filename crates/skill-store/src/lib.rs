//! Persistent skill library with global, repository, and session scopes.

use chrono::{DateTime, Utc};
use purrcode_runtime_core::QualificationStatus;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use thiserror::Error;
use uuid::Uuid;

#[derive(Clone, Debug, Serialize, Deserialize)]
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
                PRIMARY KEY (skill_id, scope)
            );
            CREATE TABLE IF NOT EXISTS blocked_publishers (
                publisher TEXT PRIMARY KEY,
                blocked_at TEXT NOT NULL,
                reason TEXT NOT NULL
            );",
        )?;
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
                    pinned INTEGER NOT NULL DEFAULT 0, PRIMARY KEY (skill_id, scope));
                 INSERT INTO skill_store SELECT * FROM skill_store_v1;
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
            return Err(error.into());
        }
        std::fs::rename(&staging, &dest)?;

        let now = Utc::now();
        let perms_json = serde_json::to_string(approved_permissions)?;

        if let Err(error) = self.conn.execute(
            "INSERT INTO skill_store
                (skill_id, version, scope, source_type, source_location, publisher,
                 content_digest, signature_status, installed_at, approved_permissions,
                 qualification_status, successful_uses, failed_uses, pinned)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'unavailable', ?8, ?9, 'unverified', 0, 0, 0)",
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
        })
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
                    qualification_status, last_used_at, successful_uses, failed_uses, pinned
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
                    qualification_status, last_used_at, successful_uses, failed_uses, pinned
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
            })
        })
        .map_err(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => StoreError::NotFound(skill_id.to_string()),
            other => StoreError::Sqlite(other),
        })
    }

    pub fn find_by_capability(&self, _capability: &str) -> Result<Vec<SkillRecord>, StoreError> {
        let all = self.list()?;
        let trimmed = _capability.to_lowercase();
        Ok(all
            .into_iter()
            .filter(|s| {
                let id = s.skill_id.to_lowercase();
                let src = s.source_type.to_lowercase();
                id.contains(&trimmed) || src.contains(&trimmed)
            })
            .collect())
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

    fn copy_dir(src: &Path, dst: &Path) -> std::io::Result<()> {
        for entry in std::fs::read_dir(src)? {
            let entry = entry?;
            let file_type = entry.file_type()?;
            if file_type.is_symlink() {
                continue;
            }
            let name = entry.file_name();
            let src_path = entry.path();
            let dst_path = dst.join(&name);
            if file_type.is_dir() {
                std::fs::create_dir_all(&dst_path)?;
                Self::copy_dir(&src_path, &dst_path)?;
            } else {
                std::fs::copy(&src_path, &dst_path)?;
            }
        }
        Ok(())
    }
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

        let record = store
            .install(
                "test-skill",
                "1.0.0",
                SkillScope::User,
                "local",
                None,
                None,
                "abc123",
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

        assert!(store
            .path_for("test-skill", &SkillScope::User)
            .join("SKILL.md")
            .exists());
        assert!(store
            .path_for("test-skill", &SkillScope::User)
            .join("tool.py")
            .exists());

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

        store
            .install(
                "dup",
                "1.0",
                SkillScope::User,
                "local",
                None,
                None,
                "digest1",
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
                "digest2",
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
                    "digest",
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
        assert!(SkillStore::open(&database, &library)
            .unwrap()
            .is_publisher_blocked("UNTRUSTED-PUBLISHER")
            .unwrap());
    }

    #[test]
    fn find_by_capability_matches() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = SkillStore::open(&dir.path().join("db"), &dir.path().join("lib")).unwrap();

        let sd = dir.path().join("s1");
        std::fs::create_dir_all(&sd).unwrap();
        std::fs::write(sd.join("SKILL.md"), "").unwrap();

        store
            .install(
                "terraform-inspector",
                "1.0",
                SkillScope::User,
                "registry",
                None,
                Some("example"),
                "digest",
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
                "digest",
                &json!({}),
                &sd,
            )
            .unwrap();

        let results = store.find_by_capability("terraform").unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].skill_id, "terraform-inspector");
    }
}
