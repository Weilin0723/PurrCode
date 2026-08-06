//! Bounded local repository indexing and hybrid lexical/symbol retrieval.

use ignore::{Walk, WalkBuilder};
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use thiserror::Error;
use tree_sitter::{Language, Node, Parser};

const MAX_INDEXED_FILE_BYTES: u64 = 2 * 1024 * 1024;
/// Maximum proportion of non-text (control/escape) bytes before a file is classified as binary.
/// If more than 10% of examined bytes are non-printable (excluding common whitespace),
/// the file is skipped rather than indexed.
const BINARY_CONTENT_THRESHOLD: f64 = 0.10;
const CHUNK_LINES: usize = 100;
const CHUNK_OVERLAP: usize = 10;
const DEFAULT_INPUT_LATENCY_PAUSE_MILLIS: u64 = 100;
const MAX_TIER0_ENTRIES: usize = 4_096;
const MAX_TIER0_CONTENT_BYTES: usize = 16 * 1024 * 1024;
const MAX_TIER1_HINTS: usize = 1_024;
const MAX_TIER1_TERM_BYTES: usize = 4_096;
const MAX_TIER1_EXAMINED_ENTRIES: usize = 100_000;
const MAX_TIER1_INDEXED_FILES: usize = 4_096;
const MAX_TIER1_INDEXED_BYTES: usize = 128 * 1024 * 1024;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct FileRecord {
    pub path: PathBuf,
    pub language: LanguageId,
    pub bytes: u64,
    pub content_digest: String,
    pub generated: bool,
    pub sensitive: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SymbolRecord {
    pub path: PathBuf,
    pub name: String,
    pub kind: String,
    pub line: usize,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LanguageId {
    Rust,
    Python,
    TypeScript,
    JavaScript,
    Java,
    Kotlin,
    Go,
    Json,
    Toml,
    Markdown,
    Other,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct IndexReport {
    pub indexed_files: usize,
    pub skipped_large_files: usize,
    pub sensitive_files: usize,
    pub generated_files: usize,
    pub symbols: usize,
    pub imports: usize,
    pub test_files: usize,
    pub dependency_manifests: usize,
    pub instruction_files: usize,
    pub languages: BTreeMap<LanguageId, usize>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Tier0Budget {
    pub maximum_entries: usize,
    pub maximum_content_bytes: usize,
    pub maximum_file_bytes: usize,
}

impl Default for Tier0Budget {
    fn default() -> Self {
        Self {
            maximum_entries: 128,
            maximum_content_bytes: 256 * 1024,
            maximum_file_bytes: 64 * 1024,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Tier0EntryKind {
    File,
    Directory,
    Other,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Tier0Entry {
    pub path: PathBuf,
    pub kind: Tier0EntryKind,
    pub bytes: u64,
    pub sensitive: bool,
    pub dependency_manifest: bool,
    pub repository_instruction: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RepositoryRootMetadata {
    pub root: PathBuf,
    pub git_branch: Option<String>,
    pub dirty: Option<bool>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Tier0Snapshot {
    pub metadata: RepositoryRootMetadata,
    pub entries: Vec<Tier0Entry>,
    pub index_report: IndexReport,
    pub indexed_content_bytes: usize,
    pub entry_limit_reached: bool,
    pub byte_limit_reached: bool,
}

#[derive(Clone, Copy, Debug, Default, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IndexLifecycleStage {
    #[default]
    Empty,
    Tier0Ready,
    TaskReady,
    BackgroundInProgress,
    BackgroundComplete,
}

impl IndexLifecycleStage {
    fn as_database_value(self) -> i64 {
        match self {
            Self::Empty => 0,
            Self::Tier0Ready => 1,
            Self::TaskReady => 2,
            Self::BackgroundInProgress => 3,
            Self::BackgroundComplete => 4,
        }
    }

    fn from_database_value(value: i64) -> Result<Self, ContextError> {
        match value {
            0 => Ok(Self::Empty),
            1 => Ok(Self::Tier0Ready),
            2 => Ok(Self::TaskReady),
            3 => Ok(Self::BackgroundInProgress),
            4 => Ok(Self::BackgroundComplete),
            _ => Err(ContextError::InvalidLifecycleStage(value)),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Tier0Preparation {
    pub rebuilt: bool,
    pub stage: IndexLifecycleStage,
    pub snapshot: Option<Tier0Snapshot>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct ContextIndexSummary {
    pub indexed_files: usize,
    pub symbols: usize,
    pub sensitive_files: usize,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Tier1Budget {
    pub maximum_examined_entries: usize,
    pub maximum_indexed_files: usize,
    pub maximum_indexed_bytes: usize,
    pub maximum_file_bytes: usize,
}

impl Default for Tier1Budget {
    fn default() -> Self {
        Self {
            maximum_examined_entries: 20_000,
            maximum_indexed_files: 512,
            maximum_indexed_bytes: 16 * 1024 * 1024,
            maximum_file_bytes: 512 * 1024,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Tier1Request {
    pub mentioned_paths: Vec<PathBuf>,
    pub related_paths: Vec<PathBuf>,
    pub filename_terms: Vec<String>,
    pub languages: BTreeSet<LanguageId>,
    pub include_changed_files: bool,
    pub budget: Tier1Budget,
}

impl Default for Tier1Request {
    fn default() -> Self {
        Self {
            mentioned_paths: Vec::new(),
            related_paths: Vec::new(),
            filename_terms: Vec::new(),
            languages: BTreeSet::new(),
            include_changed_files: true,
            budget: Tier1Budget::default(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Tier1Report {
    pub index_report: IndexReport,
    pub selected_paths: Vec<PathBuf>,
    pub language_map: BTreeMap<LanguageId, usize>,
    pub examined_entries: usize,
    pub indexed_bytes: usize,
    pub entry_limit_reached: bool,
    pub file_limit_reached: bool,
    pub byte_limit_reached: bool,
    /// True when nothing matched the task and the repository's source files
    /// were indexed instead. The context is real but it is not task-scoped, so
    /// a caller that reports index coverage must not present it as such.
    pub selection_widened: bool,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryPressure {
    #[default]
    Normal,
    Elevated,
    High,
    Critical,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct IndexingSignals {
    pub cancel_requested: bool,
    pub memory_pressure: MemoryPressure,
    pub generation_active: bool,
    pub input_latency_millis: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Tier2Policy {
    pub maximum_entries_per_step: usize,
    pub maximum_files_per_step: usize,
    pub maximum_bytes_per_step: usize,
    pub maximum_total_entries: usize,
    pub maximum_total_files: usize,
    pub maximum_total_bytes: usize,
    pub maximum_file_bytes: usize,
    pub pause_at_input_latency_millis: u64,
}

impl Default for Tier2Policy {
    fn default() -> Self {
        Self {
            maximum_entries_per_step: 256,
            maximum_files_per_step: 32,
            maximum_bytes_per_step: 2 * 1024 * 1024,
            maximum_total_entries: 100_000,
            maximum_total_files: 50_000,
            maximum_total_bytes: 256 * 1024 * 1024,
            maximum_file_bytes: 2 * 1024 * 1024,
            pause_at_input_latency_millis: DEFAULT_INPUT_LATENCY_PAUSE_MILLIS,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IndexPauseReason {
    HighMemoryPressure,
    GenerationActive,
    InputLatency,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IndexStopReason {
    CriticalMemoryPressure,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "state", content = "reason")]
pub enum Tier2Status {
    Ready,
    Paused(IndexPauseReason),
    Completed,
    Cancelled,
    Stopped(IndexStopReason),
    BudgetExhausted,
}

impl Tier2Status {
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Cancelled | Self::Stopped(_) | Self::BudgetExhausted
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Tier2StepReport {
    pub status: Tier2Status,
    pub examined_entries: usize,
    pub indexed_files: usize,
    pub indexed_bytes: usize,
    pub cumulative_examined_entries: usize,
    pub cumulative_indexed_bytes: usize,
    pub cumulative_index_report: IndexReport,
}

/// Caller-owned Tier 2 cursor. It performs no work until passed to
/// [`ContextIndex::drive_tier2`].
pub struct Tier2Work {
    root: PathBuf,
    database: PathBuf,
    walker: Walk,
    policy: Tier2Policy,
    status: Tier2Status,
    report: IndexReport,
    examined_entries: usize,
    indexed_bytes: usize,
    pending_path: Option<PathBuf>,
}

impl Tier2Work {
    pub fn status(&self) -> Tier2Status {
        self.status
    }

    pub fn report(&self) -> &IndexReport {
        &self.report
    }

    pub fn examined_entries(&self) -> usize {
        self.examined_entries
    }

    pub fn indexed_bytes(&self) -> usize {
        self.indexed_bytes
    }
}

impl fmt::Debug for Tier2Work {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Tier2Work")
            .field("root", &self.root)
            .field("database", &self.database)
            .field("policy", &self.policy)
            .field("status", &self.status)
            .field("report", &self.report)
            .field("examined_entries", &self.examined_entries)
            .field("indexed_bytes", &self.indexed_bytes)
            .field("has_pending_path", &self.pending_path.is_some())
            .finish()
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ContextHit {
    pub path: PathBuf,
    pub start_line: usize,
    pub end_line: usize,
    pub content: String,
    pub score_millis: i64,
    pub sensitive: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RetrievalBudget {
    pub maximum_hits: usize,
    pub maximum_bytes: usize,
}

impl Default for RetrievalBudget {
    fn default() -> Self {
        Self {
            maximum_hits: 12,
            maximum_bytes: 64 * 1024,
        }
    }
}

pub struct ContextIndex {
    root: PathBuf,
    database: PathBuf,
    connection: Connection,
}

impl ContextIndex {
    pub fn open(root: &Path, database: &Path) -> Result<Self, ContextError> {
        let root = root.canonicalize()?;
        if let Some(parent) = database.parent() {
            fs::create_dir_all(parent)?;
        }
        let connection = Connection::open(database)?;
        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.pragma_update(None, "foreign_keys", "ON")?;
        connection.execute_batch(SCHEMA)?;
        let database = database.canonicalize()?;
        Ok(Self {
            root,
            database,
            connection,
        })
    }

    /// Compatibility full-repository rebuild.
    ///
    /// This preserves the original eager behavior for existing callers: it clears the index,
    /// recursively walks the repository, builds symbols/imports/full-text chunks, and reads bounded
    /// Git history in one call. New interactive paths should explicitly drive Tier 0, Tier 1, and
    /// Tier 2 instead.
    pub fn rebuild(&mut self) -> Result<IndexReport, ContextError> {
        let transaction = self.connection.transaction()?;
        clear_index(&transaction)?;
        let mut report = IndexReport::default();
        let walker = repository_walker(&self.root);
        for entry in walker {
            let entry = entry?;
            let _ = index_file(
                &transaction,
                &self.root,
                &self.database,
                entry.path(),
                MAX_INDEXED_FILE_BYTES as usize,
                usize::MAX,
                &mut report,
            )?;
        }
        index_git_metadata(&transaction, &self.root)?;
        detect_repository_commands(&transaction, &self.root)?;
        set_lifecycle_stage(&transaction, IndexLifecycleStage::BackgroundComplete)?;
        transaction.commit()?;
        Ok(report)
    }

    /// Returns the durable indexing stage for this repository database.
    pub fn lifecycle_stage(&self) -> Result<IndexLifecycleStage, ContextError> {
        let value = self.connection.query_row(
            "SELECT stage FROM index_lifecycle WHERE singleton = 1",
            [],
            |row| row.get::<_, i64>(0),
        )?;
        IndexLifecycleStage::from_database_value(value)
    }

    /// Builds Tier 0 only when the database has not already reached that stage.
    ///
    /// This is the non-destructive task-entry/startup API. Reopening an existing session database
    /// and calling it again does not clear task-specific Tier 1 or background Tier 2 data.
    pub fn ensure_tier0(&mut self, budget: &Tier0Budget) -> Result<Tier0Preparation, ContextError> {
        let stage = self.lifecycle_stage()?;
        if stage >= IndexLifecycleStage::Tier0Ready {
            return Ok(Tier0Preparation {
                rebuilt: false,
                stage,
                snapshot: None,
            });
        }
        let snapshot = self.rebuild_tier0(budget)?;
        Ok(Tier0Preparation {
            rebuilt: true,
            stage: IndexLifecycleStage::Tier0Ready,
            snapshot: Some(snapshot),
        })
    }

    /// Rebuilds only the bounded, startup-safe Tier 0 view.
    ///
    /// Top-level entries are metadata-only. Content is indexed only for root manifests and
    /// repository instruction files, and sensitive paths are never added to searchable chunks.
    pub fn rebuild_tier0(&mut self, budget: &Tier0Budget) -> Result<Tier0Snapshot, ContextError> {
        validate_tier0_budget(budget)?;
        let metadata = repository_root_metadata(&self.root)?;
        // Scan a fixed global maximum before sorting so the selected root entries do not depend
        // on filesystem enumeration order. Work and memory remain bounded even when the root
        // contains more entries than the caller wants displayed.
        let root_entry_capacity = MAX_TIER0_ENTRIES
            .checked_add(1)
            .ok_or_else(|| ContextError::InvalidBudget("Tier 0 entry limit overflowed".into()))?;
        let mut root_entries = Vec::with_capacity(root_entry_capacity);
        for entry in fs::read_dir(&self.root)? {
            let entry = entry?;
            let path = entry.path();
            if is_pruned_relative(&self.root, &path) || is_index_storage(&path, &self.database) {
                continue;
            }
            root_entries.push(entry);
            if root_entries.len() == root_entry_capacity {
                break;
            }
        }
        root_entries.sort_by_key(|entry| {
            let path = PathBuf::from(entry.file_name());
            let control_file = is_dependency_manifest(&path) || is_instruction_file(&path);
            (!control_file, entry.file_name())
        });
        let entry_limit_reached = root_entries.len() > budget.maximum_entries;
        root_entries.truncate(budget.maximum_entries);

        let transaction = self.connection.transaction()?;
        clear_index(&transaction)?;
        let mut entries = Vec::new();
        let mut report = IndexReport::default();
        let mut indexed_content_bytes = 0_usize;
        let mut byte_limit_reached = false;

        for entry in root_entries {
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path)?;
            let relative = relative_path(&self.root, &path)?;
            let kind = if metadata.file_type().is_file() {
                Tier0EntryKind::File
            } else if metadata.file_type().is_dir() {
                Tier0EntryKind::Directory
            } else {
                Tier0EntryKind::Other
            };
            let dependency_manifest = is_dependency_manifest(&relative);
            let repository_instruction = is_instruction_file(&relative);
            let sensitive = is_sensitive(&relative);
            entries.push(Tier0Entry {
                path: relative.clone(),
                kind,
                bytes: metadata.len(),
                sensitive,
                dependency_manifest,
                repository_instruction,
            });

            if kind != Tier0EntryKind::File
                || sensitive
                || (!dependency_manifest && !repository_instruction)
            {
                continue;
            }
            let remaining = budget
                .maximum_content_bytes
                .saturating_sub(indexed_content_bytes);
            match index_file(
                &transaction,
                &self.root,
                &self.database,
                &path,
                budget
                    .maximum_file_bytes
                    .min(MAX_INDEXED_FILE_BYTES as usize),
                remaining,
                &mut report,
            )? {
                FileIndexOutcome::Indexed(summary) => {
                    indexed_content_bytes = indexed_content_bytes.saturating_add(summary.bytes);
                }
                FileIndexOutcome::ByteBudgetExceeded => byte_limit_reached = true,
                FileIndexOutcome::Skipped => {}
            }
        }
        detect_repository_commands(&transaction, &self.root)?;
        set_lifecycle_stage(&transaction, IndexLifecycleStage::Tier0Ready)?;
        transaction.commit()?;
        Ok(Tier0Snapshot {
            metadata,
            entries,
            index_report: report,
            indexed_content_bytes,
            entry_limit_reached,
            byte_limit_reached,
        })
    }

    /// Adds a bounded task-specific Tier 1 selection without recursively indexing unrelated
    /// content.
    pub fn index_tier1(&mut self, request: &Tier1Request) -> Result<Tier1Report, ContextError> {
        validate_tier1_request(request)?;
        let mut exact_paths = BTreeSet::new();
        let mut directory_prefixes = BTreeSet::new();
        for requested in request
            .mentioned_paths
            .iter()
            .chain(request.related_paths.iter())
        {
            let (relative, absolute) = resolve_requested_path(&self.root, requested)?;
            match fs::symlink_metadata(&absolute) {
                Ok(metadata) if metadata.file_type().is_file() => {
                    exact_paths.insert(relative);
                }
                Ok(metadata) if metadata.file_type().is_dir() => {
                    directory_prefixes.insert(relative);
                }
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }
        if request.include_changed_files {
            for path in changed_paths(&self.root)? {
                let (relative, _) = resolve_requested_path(&self.root, &path)?;
                exact_paths.insert(relative);
            }
        }

        let filename_terms = request
            .filename_terms
            .iter()
            .map(|term| term.trim().to_ascii_lowercase())
            .filter(|term| !term.is_empty())
            .collect::<Vec<_>>();
        // Scan whenever the task named no exact file, even with nothing to
        // match on: the walk is what makes the fallback below possible, and it
        // is bounded by `maximum_examined_entries` either way.
        let scan_repository = exact_paths.is_empty()
            || !directory_prefixes.is_empty()
            || !filename_terms.is_empty()
            || !request.languages.is_empty();
        let mut selected = exact_paths;
        let mut source_files = BTreeSet::new();
        let mut language_map = BTreeMap::new();
        let mut language_paths = BTreeSet::new();
        let mut examined_entries = 0_usize;
        let mut entry_limit_reached = false;

        if scan_repository {
            for entry in repository_walker(&self.root) {
                if examined_entries == request.budget.maximum_examined_entries {
                    entry_limit_reached = true;
                    break;
                }
                let entry = entry?;
                examined_entries += 1;
                if is_index_storage(entry.path(), &self.database) {
                    continue;
                }
                let metadata = entry.metadata()?;
                if !metadata.is_file() {
                    continue;
                }
                let relative = relative_path(&self.root, entry.path())?;
                let language = language_for(&relative);
                if language_paths.insert(relative.clone()) {
                    *language_map.entry(language).or_default() += 1;
                }
                let filename = relative
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or_default()
                    .to_ascii_lowercase();
                let path_selected = directory_prefixes
                    .iter()
                    .any(|prefix| relative.starts_with(prefix))
                    || filename_terms.iter().any(|term| filename.contains(term))
                    || request.languages.contains(&language);
                if path_selected {
                    selected.insert(relative);
                } else if language != LanguageId::Other {
                    source_files.insert(relative);
                }
            }
        }

        // A task-scoped selection that matched nothing must not leave the model
        // with no repository context at all. Selection is by filename, and most
        // objectives share no word with any filename — "build a CSV to JSON
        // converter" against a Rust repository selected zero files, so the
        // planner reported "Indexed 0 file(s), 0 symbol(s)" and then planned
        // against nothing. Widening to the repository's source files under the
        // same budget is what retrieval needs to have anything to rank.
        let selection_widened = selected.is_empty() && !source_files.is_empty();
        if selection_widened {
            selected = source_files;
        }

        let transaction = self.connection.transaction()?;
        let mut report = IndexReport::default();
        let mut selected_paths = Vec::new();
        let mut indexed_bytes = 0_usize;
        let mut file_limit_reached = false;
        let mut byte_limit_reached = false;
        for relative in selected {
            if selected_paths.len() == request.budget.maximum_indexed_files {
                file_limit_reached = true;
                break;
            }
            let path = self.root.join(&relative);
            let remaining = request
                .budget
                .maximum_indexed_bytes
                .saturating_sub(indexed_bytes);
            match index_file(
                &transaction,
                &self.root,
                &self.database,
                &path,
                request
                    .budget
                    .maximum_file_bytes
                    .min(MAX_INDEXED_FILE_BYTES as usize),
                remaining,
                &mut report,
            )? {
                FileIndexOutcome::Indexed(summary) => {
                    if language_paths.insert(summary.relative.clone()) {
                        *language_map.entry(summary.language).or_default() += 1;
                    }
                    indexed_bytes = indexed_bytes.saturating_add(summary.bytes);
                    selected_paths.push(summary.relative);
                }
                FileIndexOutcome::ByteBudgetExceeded => byte_limit_reached = true,
                FileIndexOutcome::Skipped => {}
            }
        }
        set_lifecycle_stage(&transaction, IndexLifecycleStage::TaskReady)?;
        transaction.commit()?;
        Ok(Tier1Report {
            index_report: report,
            selected_paths,
            language_map,
            examined_entries,
            indexed_bytes,
            entry_limit_reached,
            file_limit_reached,
            byte_limit_reached,
            selection_widened,
        })
    }

    /// Reports the authoritative indexed row counts currently stored in SQLite.
    pub fn summary(&self) -> Result<ContextIndexSummary, ContextError> {
        let (indexed_files, sensitive_files) = self.connection.query_row(
            "SELECT COUNT(*), COALESCE(SUM(sensitive), 0) FROM files",
            [],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
        )?;
        let symbols = self
            .connection
            .query_row("SELECT COUNT(*) FROM symbols", [], |row| {
                row.get::<_, i64>(0)
            })?;
        Ok(ContextIndexSummary {
            indexed_files: usize::try_from(indexed_files)
                .map_err(|_| ContextError::DatabaseCountOverflow)?,
            symbols: usize::try_from(symbols).map_err(|_| ContextError::DatabaseCountOverflow)?,
            sensitive_files: usize::try_from(sensitive_files)
                .map_err(|_| ContextError::DatabaseCountOverflow)?,
        })
    }

    /// Creates a caller-owned, inert Tier 2 cursor.
    pub fn begin_tier2(&self, policy: Tier2Policy) -> Result<Tier2Work, ContextError> {
        validate_tier2_policy(&policy)?;
        Ok(Tier2Work {
            root: self.root.clone(),
            database: self.database.clone(),
            walker: repository_walker(&self.root),
            policy,
            status: Tier2Status::Ready,
            report: IndexReport::default(),
            examined_entries: 0,
            indexed_bytes: 0,
            pending_path: None,
        })
    }

    /// Performs at most one bounded Tier 2 step.
    ///
    /// The caller supplies current resource signals on every invocation. Paused work can be driven
    /// again after the signal clears; cancellation, critical-pressure stop, completion, and total
    /// budget exhaustion are terminal. This method starts no thread and performs no model work.
    pub fn drive_tier2(
        &mut self,
        work: &mut Tier2Work,
        signals: IndexingSignals,
    ) -> Result<Tier2StepReport, ContextError> {
        if work.root != self.root || work.database != self.database {
            return Err(ContextError::Tier2ContextMismatch);
        }
        if work.status.is_terminal() {
            return Ok(tier2_step_report(work, 0, 0, 0));
        }
        if signals.cancel_requested {
            work.status = Tier2Status::Cancelled;
            return Ok(tier2_step_report(work, 0, 0, 0));
        }
        if signals.memory_pressure == MemoryPressure::Critical {
            work.status = Tier2Status::Stopped(IndexStopReason::CriticalMemoryPressure);
            return Ok(tier2_step_report(work, 0, 0, 0));
        }
        let pause_reason = if signals.memory_pressure == MemoryPressure::High {
            Some(IndexPauseReason::HighMemoryPressure)
        } else if signals.generation_active {
            Some(IndexPauseReason::GenerationActive)
        } else if signals.input_latency_millis >= work.policy.pause_at_input_latency_millis {
            Some(IndexPauseReason::InputLatency)
        } else {
            None
        };
        if let Some(reason) = pause_reason {
            work.status = Tier2Status::Paused(reason);
            return Ok(tier2_step_report(work, 0, 0, 0));
        }
        work.status = Tier2Status::Ready;

        let transaction = self.connection.transaction()?;
        let mut step_examined = 0_usize;
        let mut step_files = 0_usize;
        let mut step_bytes = 0_usize;
        let mut walker_finished = false;

        loop {
            if step_examined == work.policy.maximum_entries_per_step
                || step_files == work.policy.maximum_files_per_step
                || step_bytes == work.policy.maximum_bytes_per_step
            {
                break;
            }
            if work.examined_entries == work.policy.maximum_total_entries
                || work.report.indexed_files == work.policy.maximum_total_files
                || work.indexed_bytes == work.policy.maximum_total_bytes
            {
                work.status = Tier2Status::BudgetExhausted;
                break;
            }

            let path = if let Some(path) = work.pending_path.take() {
                path
            } else {
                match work.walker.next() {
                    Some(entry) => {
                        let entry = entry?;
                        work.examined_entries += 1;
                        step_examined += 1;
                        entry.into_path()
                    }
                    None => {
                        walker_finished = true;
                        break;
                    }
                }
            };
            if is_index_storage(&path, &self.database) {
                continue;
            }
            let metadata = fs::symlink_metadata(&path)?;
            if !metadata.file_type().is_file() {
                continue;
            }
            let file_bytes = usize::try_from(metadata.len()).unwrap_or(usize::MAX);
            if file_bytes > work.policy.maximum_file_bytes {
                work.report.skipped_large_files += 1;
                continue;
            }
            let total_remaining = work
                .policy
                .maximum_total_bytes
                .saturating_sub(work.indexed_bytes);
            if file_bytes > total_remaining {
                work.pending_path = Some(path);
                work.status = Tier2Status::BudgetExhausted;
                break;
            }
            let step_remaining = work
                .policy
                .maximum_bytes_per_step
                .saturating_sub(step_bytes);
            if file_bytes > step_remaining {
                work.pending_path = Some(path);
                break;
            }

            match index_file(
                &transaction,
                &self.root,
                &self.database,
                &path,
                work.policy.maximum_file_bytes,
                total_remaining.min(step_remaining),
                &mut work.report,
            )? {
                FileIndexOutcome::Indexed(summary) => {
                    step_files += 1;
                    step_bytes = step_bytes.saturating_add(summary.bytes);
                    work.indexed_bytes = work.indexed_bytes.saturating_add(summary.bytes);
                }
                FileIndexOutcome::ByteBudgetExceeded => {
                    work.pending_path = Some(path);
                    break;
                }
                FileIndexOutcome::Skipped => {}
            }
        }

        if walker_finished && work.pending_path.is_none() {
            transaction.execute("DELETE FROM git_files", [])?;
            transaction.execute("DELETE FROM cochanges", [])?;
            transaction.execute("DELETE FROM repository_commands", [])?;
            index_git_metadata(&transaction, &self.root)?;
            detect_repository_commands(&transaction, &self.root)?;
            work.status = Tier2Status::Completed;
        }
        if work.status == Tier2Status::Completed {
            set_lifecycle_stage(&transaction, IndexLifecycleStage::BackgroundComplete)?;
        } else if step_examined > 0 || step_files > 0 {
            set_lifecycle_stage(&transaction, IndexLifecycleStage::BackgroundInProgress)?;
        }
        transaction.commit()?;
        Ok(tier2_step_report(
            work,
            step_examined,
            step_files,
            step_bytes,
        ))
    }

    pub fn retrieve(
        &self,
        query: &str,
        budget: &RetrievalBudget,
    ) -> Result<Vec<ContextHit>, ContextError> {
        if budget.maximum_hits == 0 || budget.maximum_bytes == 0 {
            return Ok(Vec::new());
        }
        let fts_query_string = fts_query(query);
        if fts_query_string.is_empty() {
            return Ok(Vec::new());
        }
        let query_terms: Vec<String> = query
            .split(|c: char| !c.is_alphanumeric() && c != '_')
            .filter(|t| t.len() >= 2)
            .take(3)
            .map(|t| t.to_ascii_lowercase())
            .collect();

        // --- candidate pool keyed by (path, start_line) ---
        let mut candidates: BTreeMap<(PathBuf, usize), ContextHit> = BTreeMap::new();

        // --- Phase 1: chunks FTS5 query ---
        {
            let mut statement = self.connection.prepare(
                "SELECT chunks.path, chunks.start_line, chunks.end_line, chunks.content,
                        CAST((-bm25(chunks) * 1000.0) AS INTEGER) +
                        CASE WHEN lower(chunks.path) LIKE '%' || lower(?2) || '%' THEN 5000 ELSE 0 END +
                        CASE WHEN git_files.changed = 1 THEN 2500 ELSE 0 END +
                        COALESCE(MIN(git_files.last_commit, 2000000000) / 1000000, 0)
                        AS score
                 FROM chunks
                 LEFT JOIN git_files ON git_files.path = chunks.path
                 WHERE chunks MATCH ?1
                 ORDER BY score DESC
                 LIMIT ?3",
            )?;
            let rows = statement.query_map(
                params![fts_query_string, query, (budget.maximum_hits * 4) as i64],
                |row| {
                    Ok(ContextHit {
                        path: PathBuf::from(row.get::<_, String>(0)?),
                        start_line: row.get(1)?,
                        end_line: row.get(2)?,
                        content: row.get(3)?,
                        score_millis: row.get(4)?,
                        sensitive: false,
                    })
                },
            )?;
            for row in rows {
                let hit = row?;
                let key = (hit.path.clone(), hit.start_line);
                candidates
                    .entry(key)
                    .and_modify(|existing| {
                        if hit.score_millis > existing.score_millis {
                            *existing = hit.clone();
                        }
                    })
                    .or_insert_with(|| hit);
            }
        }

        // --- Phase 2: symbol search ---
        if !query_terms.is_empty() {
            // Cache filesystem reads: (path_str, lines)
            let mut file_cache: BTreeMap<String, Vec<String>> = BTreeMap::new();

            for term in &query_terms {
                let mut sym_stmt = self.connection.prepare(
                    "SELECT path, line FROM symbols
                     WHERE lower(name) LIKE '%' || lower(?1) || '%'
                        OR lower(kind) LIKE '%' || lower(?1) || '%'
                     LIMIT ?2",
                )?;
                let sym_rows = sym_stmt.query_map(
                    params![term, (budget.maximum_hits * 2) as i64],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, usize>(1)?,
                        ))
                    },
                )?;
                for sym_row in sym_rows {
                    let (sym_path_str, sym_line) = sym_row?;

                    // Compute the scoring signals for this symbol match
                    let path_lower = sym_path_str.to_ascii_lowercase();
                    let file_changed: i64 = self
                        .connection
                        .query_row(
                            "SELECT changed FROM git_files WHERE path = ?1",
                            [&sym_path_str],
                            |row| row.get(0),
                        )
                        .unwrap_or(0);
                    let file_recency: i64 = self
                        .connection
                        .query_row(
                            "SELECT COALESCE(last_commit, 0) FROM git_files WHERE path = ?1",
                            [&sym_path_str],
                            |row| row.get(0),
                        )
                        .unwrap_or(0);
                    let mut score: i64 = 3000; // symbol-match bonus
                    if path_lower.contains(term.as_str()) {
                        score += 5000; // path substring bonus
                    }
                    if file_changed == 1 {
                        score += 2500; // changed-file bonus
                    }
                    score += file_recency.min(2_000_000_000) / 1_000_000; // git recency

                    let start_line = sym_line.saturating_sub(2);
                    let end_line = sym_line.saturating_add(20);
                    let key = (PathBuf::from(&sym_path_str), start_line);

                    if let Some(existing) = candidates.get_mut(&key) {
                        if score > existing.score_millis {
                            existing.score_millis = score;
                        }
                    } else {
                        // Read file content for this line range
                        let lines = match file_cache.get(&sym_path_str) {
                            Some(cached) => cached,
                            None => {
                                let file_path = self.root.join(&sym_path_str);
                                let content = match fs::read_to_string(&file_path) {
                                    Ok(c) => c,
                                    Err(_) => continue,
                                };
                                let line_vec: Vec<String> =
                                    content.lines().map(|l| l.to_owned()).collect();
                                file_cache.insert(sym_path_str.clone(), line_vec);
                                &file_cache[&sym_path_str]
                            }
                        };
                        let start_idx = start_line.saturating_sub(1).min(lines.len());
                        let end_idx = end_line.min(lines.len());
                        if start_idx >= end_idx {
                            continue;
                        }
                        let content = lines[start_idx..end_idx].join("\n");

                        candidates.insert(
                            key,
                            ContextHit {
                                path: PathBuf::from(&sym_path_str),
                                start_line,
                                end_line: end_idx,
                                content,
                                score_millis: score,
                                sensitive: false,
                            },
                        );
                    }
                }
            }
        }

        // --- Phase 3: import-proximity boost ---
        if !query_terms.is_empty() {
            let mut import_boosted_paths: BTreeSet<String> = BTreeSet::new();
            for term in &query_terms {
                let mut imp_stmt = self.connection.prepare(
                    "SELECT DISTINCT source_path FROM imports
                     WHERE lower(target) LIKE '%' || lower(?1) || '%'
                     LIMIT ?2",
                )?;
                let imp_rows = imp_stmt.query_map(
                    params![term, (budget.maximum_hits * 2) as i64],
                    |row| row.get::<_, String>(0),
                )?;
                for imp_row in imp_rows {
                    if let Ok(path) = imp_row {
                        import_boosted_paths.insert(path);
                    }
                }
            }
            for (_, hit) in candidates.iter_mut() {
                if import_boosted_paths.contains(&hit.path.to_string_lossy().into_owned()) {
                    hit.score_millis += 1500;
                }
            }
        }

        // --- Phase 4: sort, deduplicate overlapping spans, apply budget ---
        let mut scored: Vec<ContextHit> = candidates.into_values().collect();
        scored.sort_by(|a, b| b.score_millis.cmp(&a.score_millis));

        let mut hits = Vec::new();
        let mut bytes = 0;
        for mut hit in scored {
            // Deduplicate overlapping spans for the same path
            if hits.iter().any(|existing: &ContextHit| {
                existing.path == hit.path
                    && hit.start_line <= existing.end_line
                    && hit.end_line >= existing.start_line
            }) {
                continue;
            }
            let remaining = budget.maximum_bytes.saturating_sub(bytes);
            if remaining == 0 || hits.len() == budget.maximum_hits {
                break;
            }
            if hit.content.len() > remaining {
                hit.content.truncate(remaining);
            }
            bytes += hit.content.len();
            hits.push(hit);
        }
        Ok(hits)
    }

    pub fn symbols(&self, query: &str, limit: usize) -> Result<Vec<SymbolRecord>, ContextError> {
        let mut statement = self.connection.prepare(
            "SELECT path, name, kind, line FROM symbols
             WHERE lower(name) LIKE '%' || lower(?1) || '%'
             ORDER BY CASE WHEN lower(name) = lower(?1) THEN 0 ELSE 1 END, name
             LIMIT ?2",
        )?;
        let rows = statement.query_map(params![query, limit as i64], |row| {
            Ok(SymbolRecord {
                path: PathBuf::from(row.get::<_, String>(0)?),
                name: row.get(1)?,
                kind: row.get(2)?,
                line: row.get(3)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }
}

fn repository_walker(root: &Path) -> Walk {
    let root = root.to_path_buf();
    WalkBuilder::new(&root)
        .hidden(false)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .filter_entry(move |entry| !is_pruned_relative(&root, entry.path()))
        .build()
}

fn clear_index(transaction: &rusqlite::Transaction<'_>) -> Result<(), ContextError> {
    transaction.execute("DELETE FROM symbols", [])?;
    transaction.execute("DELETE FROM imports", [])?;
    transaction.execute("DELETE FROM test_relations", [])?;
    transaction.execute("DELETE FROM git_files", [])?;
    transaction.execute("DELETE FROM cochanges", [])?;
    transaction.execute("DELETE FROM repository_commands", [])?;
    transaction.execute("DELETE FROM chunks", [])?;
    transaction.execute("DELETE FROM files", [])?;
    Ok(())
}

fn set_lifecycle_stage(
    transaction: &rusqlite::Transaction<'_>,
    stage: IndexLifecycleStage,
) -> Result<(), ContextError> {
    transaction.execute(
        "UPDATE index_lifecycle SET stage = ?1 WHERE singleton = 1",
        [stage.as_database_value()],
    )?;
    Ok(())
}

#[derive(Debug)]
struct IndexedFileSummary {
    relative: PathBuf,
    language: LanguageId,
    bytes: usize,
}

#[derive(Debug)]
enum FileIndexOutcome {
    Indexed(IndexedFileSummary),
    ByteBudgetExceeded,
    Skipped,
}

fn index_file(
    transaction: &rusqlite::Transaction<'_>,
    root: &Path,
    database: &Path,
    path: &Path,
    maximum_file_bytes: usize,
    remaining_bytes: usize,
    report: &mut IndexReport,
) -> Result<FileIndexOutcome, ContextError> {
    if is_index_storage(path, database) {
        return Ok(FileIndexOutcome::Skipped);
    }
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.file_type().is_file() {
        return Ok(FileIndexOutcome::Skipped);
    }
    let relative = relative_path(root, path)?;
    let bytes = usize::try_from(metadata.len()).unwrap_or(usize::MAX);
    if metadata.len() > MAX_INDEXED_FILE_BYTES || bytes > maximum_file_bytes || bytes == usize::MAX
    {
        report.skipped_large_files += 1;
        return Ok(FileIndexOutcome::Skipped);
    }
    if bytes > remaining_bytes {
        return Ok(FileIndexOutcome::ByteBudgetExceeded);
    }
    let content = fs::read(path)?;
    let relative_text = relative.to_string_lossy().into_owned();
    if is_likely_binary(&content) {
        delete_indexed_path(transaction, &relative_text)?;
        return Ok(FileIndexOutcome::Skipped);
    }
    let language = language_for(&relative);
    let sensitive = is_sensitive(&relative);
    let text = String::from_utf8_lossy(&content);
    let generated = is_generated(&relative, &text);
    let digest = blake3::hash(&content).to_hex().to_string();
    delete_indexed_path(transaction, &relative_text)?;
    transaction.execute(
        "INSERT INTO files(path, language, bytes, digest, generated, sensitive)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            relative_text,
            format!("{language:?}"),
            metadata.len(),
            digest,
            generated,
            sensitive
        ],
    )?;
    report.indexed_files += 1;
    *report.languages.entry(language).or_default() += 1;
    if sensitive {
        report.sensitive_files += 1;
    }
    if generated {
        report.generated_files += 1;
    }
    if is_dependency_manifest(&relative) {
        report.dependency_manifests += 1;
    }
    if is_instruction_file(&relative) {
        report.instruction_files += 1;
    }
    if is_test_path(&relative) {
        report.test_files += 1;
        if let Some(source) = probable_source_path(&relative) {
            transaction.execute(
                "INSERT INTO test_relations(test_path, source_path, confidence)
                 VALUES (?1, ?2, ?3)",
                params![relative_text, source.to_string_lossy(), 0.5_f64],
            )?;
        }
    }
    if !sensitive {
        for symbol in extract_symbols(language, &relative, &content)? {
            transaction.execute(
                "INSERT INTO symbols(path, name, kind, line) VALUES (?1, ?2, ?3, ?4)",
                params![
                    symbol.path.to_string_lossy(),
                    symbol.name,
                    symbol.kind,
                    symbol.line
                ],
            )?;
            report.symbols += 1;
        }
        for import in extract_imports(language, &text) {
            transaction.execute(
                "INSERT INTO imports(source_path, target) VALUES (?1, ?2)",
                params![relative_text, import],
            )?;
            report.imports += 1;
        }
        for (start, end, chunk) in chunk_by_nodes(language, &content) {
            transaction.execute(
                "INSERT INTO chunks(path, start_line, end_line, content)
                 VALUES (?1, ?2, ?3, ?4)",
                params![relative_text, start, end, chunk],
            )?;
        }
    }
    Ok(FileIndexOutcome::Indexed(IndexedFileSummary {
        relative,
        language,
        bytes,
    }))
}

fn delete_indexed_path(
    transaction: &rusqlite::Transaction<'_>,
    path: &str,
) -> Result<(), ContextError> {
    transaction.execute("DELETE FROM symbols WHERE path = ?1", [path])?;
    transaction.execute("DELETE FROM imports WHERE source_path = ?1", [path])?;
    transaction.execute("DELETE FROM test_relations WHERE test_path = ?1", [path])?;
    transaction.execute("DELETE FROM chunks WHERE path = ?1", [path])?;
    transaction.execute("DELETE FROM files WHERE path = ?1", [path])?;
    Ok(())
}

fn relative_path(root: &Path, path: &Path) -> Result<PathBuf, ContextError> {
    path.strip_prefix(root)
        .map(Path::to_path_buf)
        .map_err(|_| ContextError::PathEscape(path.to_path_buf()))
}

fn resolve_requested_path(
    root: &Path,
    requested: &Path,
) -> Result<(PathBuf, PathBuf), ContextError> {
    let mut relative = PathBuf::new();
    for component in requested.components() {
        match component {
            Component::Normal(value) => relative.push(value),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(ContextError::InvalidRequestedPath(requested.to_path_buf()));
            }
        }
    }
    let absolute = root.join(&relative);
    match absolute.canonicalize() {
        Ok(canonical) if !canonical.starts_with(root) => {
            return Err(ContextError::PathEscape(canonical));
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    Ok((relative, absolute))
}

fn changed_paths(root: &Path) -> Result<Vec<PathBuf>, ContextError> {
    if !root.join(".git").exists() {
        return Ok(Vec::new());
    }
    let changed = match git_output(root, &["diff", "--name-only", "-z", "HEAD", "--", "."]) {
        Ok(changed) => changed,
        Err(_) => git_output(root, &["diff", "--name-only", "-z", "--", "."])?,
    };
    let untracked = git_output(
        root,
        &[
            "ls-files",
            "--others",
            "--exclude-standard",
            "-z",
            "--",
            ".",
        ],
    )?;
    let mut paths = nul_strings(&changed)
        .chain(nul_strings(&untracked))
        .map(PathBuf::from)
        .collect::<BTreeSet<_>>();
    Ok(std::mem::take(&mut paths).into_iter().collect())
}

fn repository_root_metadata(root: &Path) -> Result<RepositoryRootMetadata, ContextError> {
    if !root.join(".git").exists() {
        return Ok(RepositoryRootMetadata {
            root: root.to_path_buf(),
            git_branch: None,
            dirty: None,
        });
    }
    let branch = git_output(root, &["rev-parse", "--abbrev-ref", "HEAD"])?;
    let branch = String::from_utf8_lossy(&branch).trim().to_owned();
    let dirty = git_output(
        root,
        &["status", "--porcelain=v1", "--untracked-files=normal"],
    )?;
    Ok(RepositoryRootMetadata {
        root: root.to_path_buf(),
        git_branch: (!branch.is_empty()).then_some(branch),
        dirty: Some(!dirty.is_empty()),
    })
}

fn validate_tier2_policy(policy: &Tier2Policy) -> Result<(), ContextError> {
    let invalid = policy.maximum_entries_per_step == 0
        || policy.maximum_files_per_step == 0
        || policy.maximum_bytes_per_step == 0
        || policy.maximum_total_entries == 0
        || policy.maximum_total_files == 0
        || policy.maximum_total_bytes == 0
        || policy.maximum_file_bytes == 0
        || policy.maximum_file_bytes > MAX_INDEXED_FILE_BYTES as usize
        || policy.maximum_entries_per_step > policy.maximum_total_entries
        || policy.maximum_files_per_step > policy.maximum_total_files
        || policy.maximum_bytes_per_step > policy.maximum_total_bytes
        || policy.maximum_file_bytes > policy.maximum_bytes_per_step
        || policy.maximum_file_bytes > policy.maximum_total_bytes
        || policy.pause_at_input_latency_millis == 0;
    if invalid {
        return Err(ContextError::InvalidBudget(
            "Tier 2 limits must be non-zero, step limits must fit total limits, and one file must fit one step"
                .into(),
        ));
    }
    Ok(())
}

fn validate_tier0_budget(budget: &Tier0Budget) -> Result<(), ContextError> {
    let invalid = budget.maximum_entries == 0
        || budget.maximum_entries > MAX_TIER0_ENTRIES
        || budget.maximum_content_bytes == 0
        || budget.maximum_content_bytes > MAX_TIER0_CONTENT_BYTES
        || budget.maximum_file_bytes == 0
        || budget.maximum_file_bytes > MAX_INDEXED_FILE_BYTES as usize
        || budget.maximum_file_bytes > budget.maximum_content_bytes;
    if invalid {
        return Err(ContextError::InvalidBudget(format!(
            "Tier 0 limits must be non-zero and no greater than {MAX_TIER0_ENTRIES} entries, \
             {MAX_TIER0_CONTENT_BYTES} total bytes, and {MAX_INDEXED_FILE_BYTES} bytes per file"
        )));
    }
    Ok(())
}

fn validate_tier1_request(request: &Tier1Request) -> Result<(), ContextError> {
    let hint_count = request
        .mentioned_paths
        .len()
        .checked_add(request.related_paths.len())
        .and_then(|count| count.checked_add(request.filename_terms.len()))
        .ok_or_else(|| ContextError::InvalidBudget("Tier 1 hint count overflowed".into()))?;
    let term_bytes = request
        .filename_terms
        .iter()
        .try_fold(0_usize, |total, term| total.checked_add(term.len()))
        .ok_or_else(|| ContextError::InvalidBudget("Tier 1 term size overflowed".into()))?;
    let budget = &request.budget;
    let invalid = hint_count > MAX_TIER1_HINTS
        || term_bytes > MAX_TIER1_TERM_BYTES
        || budget.maximum_examined_entries == 0
        || budget.maximum_examined_entries > MAX_TIER1_EXAMINED_ENTRIES
        || budget.maximum_indexed_files == 0
        || budget.maximum_indexed_files > MAX_TIER1_INDEXED_FILES
        || budget.maximum_indexed_bytes == 0
        || budget.maximum_indexed_bytes > MAX_TIER1_INDEXED_BYTES
        || budget.maximum_file_bytes == 0
        || budget.maximum_file_bytes > MAX_INDEXED_FILE_BYTES as usize
        || budget.maximum_file_bytes > budget.maximum_indexed_bytes;
    if invalid {
        return Err(ContextError::InvalidBudget(
            "Tier 1 hints or budgets exceed bounded indexing limits".into(),
        ));
    }
    Ok(())
}

fn tier2_step_report(
    work: &Tier2Work,
    examined_entries: usize,
    indexed_files: usize,
    indexed_bytes: usize,
) -> Tier2StepReport {
    Tier2StepReport {
        status: work.status,
        examined_entries,
        indexed_files,
        indexed_bytes,
        cumulative_examined_entries: work.examined_entries,
        cumulative_indexed_bytes: work.indexed_bytes,
        cumulative_index_report: work.report.clone(),
    }
}

fn is_pruned(path: &Path) -> bool {
    path.components().any(|component| {
        matches!(
            component.as_os_str().to_str(),
            Some(".git" | ".purrcode" | "node_modules" | "target" | "dist" | "build")
        )
    })
}

/// Apply repository exclusions to a path relative to the repository root.
///
/// Worktrees commonly live below a repository-owned `.purrcode/worktrees`
/// directory. Testing the absolute path would see that ancestor `.purrcode`
/// component on the root itself and prune the entire walk before it reaches a
/// source file. Paths that cannot be made relative are rejected closed.
fn is_pruned_relative(root: &Path, path: &Path) -> bool {
    path.strip_prefix(root).map(is_pruned).unwrap_or(true)
}

fn is_index_storage(path: &Path, database: &Path) -> bool {
    if path == database {
        return true;
    }
    match (
        path.parent(),
        path.file_name().and_then(|name| name.to_str()),
        database.parent(),
        database.file_name().and_then(|name| name.to_str()),
    ) {
        (Some(parent), Some(name), Some(database_parent), Some(database_name))
            if parent == database_parent =>
        {
            name == format!("{database_name}-wal") || name == format!("{database_name}-shm")
        }
        _ => false,
    }
}

fn language_for(path: &Path) -> LanguageId {
    match path.extension().and_then(|extension| extension.to_str()) {
        Some("rs") => LanguageId::Rust,
        Some("py") => LanguageId::Python,
        Some("ts" | "tsx") => LanguageId::TypeScript,
        Some("js" | "jsx" | "mjs" | "cjs") => LanguageId::JavaScript,
        Some("java") => LanguageId::Java,
        Some("kt" | "kts") => LanguageId::Kotlin,
        Some("go") => LanguageId::Go,
        Some("json") => LanguageId::Json,
        Some("toml") => LanguageId::Toml,
        Some("md" | "markdown") => LanguageId::Markdown,
        _ => LanguageId::Other,
    }
}

fn is_likely_binary(content: &[u8]) -> bool {
    // Quick null-byte check for known binary formats.
    if content.contains(&0) {
        return true;
    }
    // Statistical check: count non-printable, non-whitespace bytes in the first 8 KiB.
    let examined = content.len().min(8192);
    let non_text: usize = content[..examined]
        .iter()
        .filter(|&&b| {
            b < 0x09 // below tab
                || (b > 0x0d && b < 0x20) // above CR, below space
                || b == 0x7f // DEL
        })
        .count();
    non_text as f64 / examined as f64 > BINARY_CONTENT_THRESHOLD
}

fn is_sensitive(path: &Path) -> bool {
    let lower = path.to_string_lossy().to_ascii_lowercase();
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    name == ".env"
        || name.starts_with(".env.")
        || lower.ends_with(".pem")
        || lower.ends_with(".key")
        || lower.ends_with(".p12")
        || lower.contains("credentials")
        || lower.contains("secrets.")
        || lower.starts_with(".ssh/")
        || lower.contains("/.ssh/")
        || matches!(
            name.as_str(),
            ".npmrc" | ".netrc" | "id_rsa" | "id_dsa" | "id_ecdsa" | "id_ed25519"
        )
}

fn is_generated(path: &Path, content: &str) -> bool {
    let lower = path.to_string_lossy().to_ascii_lowercase();
    lower.contains("/generated/")
        || lower.ends_with(".min.js")
        || lower.ends_with(".g.dart")
        || content.lines().take(5).any(|line| {
            line.to_ascii_lowercase().contains("generated") && line.contains("not edit")
        })
}

fn is_dependency_manifest(path: &Path) -> bool {
    matches!(
        path.file_name().and_then(|name| name.to_str()),
        Some(
            "Cargo.toml"
                | "package.json"
                | "pyproject.toml"
                | "requirements.txt"
                | "go.mod"
                | "pom.xml"
                | "build.gradle"
                | "build.gradle.kts"
        )
    )
}

fn is_instruction_file(path: &Path) -> bool {
    matches!(
        path.file_name().and_then(|name| name.to_str()),
        Some("AGENTS.md" | "CLAUDE.md" | "CONTRIBUTING.md")
    ) || path.starts_with(".codex")
}

fn is_test_path(path: &Path) -> bool {
    let lower = path.to_string_lossy().to_ascii_lowercase();
    lower.contains("/tests/")
        || lower.starts_with("tests/")
        || lower.contains("/test/")
        || lower.ends_with("_test.go")
        || lower.ends_with("_test.py")
        || lower.ends_with(".test.ts")
        || lower.ends_with(".spec.ts")
        || lower.ends_with("test.java")
        || lower.ends_with("test.kt")
}

fn probable_source_path(test: &Path) -> Option<PathBuf> {
    let text = test.to_string_lossy();
    let candidate = text
        .replace("/tests/", "/src/")
        .replace("tests/", "src/")
        .replace("_test.go", ".go")
        .replace("_test.py", ".py")
        .replace(".test.ts", ".ts")
        .replace(".spec.ts", ".ts")
        .replace("Test.java", ".java")
        .replace("Test.kt", ".kt");
    (candidate != text).then(|| PathBuf::from(candidate))
}

fn extract_imports(language: LanguageId, content: &str) -> Vec<String> {
    content
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            let import = match language {
                LanguageId::Rust => line.strip_prefix("use "),
                LanguageId::Python => line
                    .strip_prefix("import ")
                    .or_else(|| line.strip_prefix("from ")),
                LanguageId::TypeScript | LanguageId::JavaScript => {
                    line.strip_prefix("import ").or_else(|| {
                        line.contains("require(")
                            .then(|| line.split("require(").nth(1))
                            .flatten()
                    })
                }
                LanguageId::Java | LanguageId::Kotlin => line.strip_prefix("import "),
                LanguageId::Go => line.strip_prefix("import "),
                _ => None,
            }?;
            let target = import.trim_end_matches([';', '{']).trim();
            (!target.is_empty()).then(|| target.chars().take(512).collect())
        })
        .collect()
}

fn index_git_metadata(
    transaction: &rusqlite::Transaction<'_>,
    root: &Path,
) -> Result<(), ContextError> {
    if !root.join(".git").exists() {
        return Ok(());
    }
    let changed = git_output(root, &["diff", "--name-only", "-z", "HEAD", "--", "."])?;
    for path in nul_strings(&changed) {
        transaction.execute(
            "INSERT INTO git_files(path, last_commit, changed) VALUES (?1, NULL, 1)
             ON CONFLICT(path) DO UPDATE SET changed = 1",
            [path],
        )?;
    }
    let history = git_output(
        root,
        &["log", "-100", "--name-only", "--format=@@%ct", "--", "."],
    )?;
    let history = String::from_utf8_lossy(&history);
    let mut timestamp = None;
    let mut commit_files = Vec::new();
    for line in history.lines().chain(std::iter::once("@@END")) {
        if let Some(value) = line.strip_prefix("@@") {
            record_cochanges(transaction, &commit_files)?;
            commit_files.clear();
            timestamp = value.parse::<i64>().ok();
        } else if !line.is_empty() {
            transaction.execute(
                "INSERT INTO git_files(path, last_commit, changed) VALUES (?1, ?2, 0)
                 ON CONFLICT(path) DO UPDATE SET
                   last_commit = MAX(COALESCE(last_commit, 0), excluded.last_commit)",
                params![line, timestamp],
            )?;
            if commit_files.len() < 100 {
                commit_files.push(line.to_owned());
            }
        }
    }
    Ok(())
}

fn record_cochanges(
    transaction: &rusqlite::Transaction<'_>,
    files: &[String],
) -> Result<(), ContextError> {
    for (index, left) in files.iter().enumerate() {
        for right in files.iter().skip(index + 1) {
            transaction.execute(
                "INSERT INTO cochanges(path_a, path_b, occurrences) VALUES (?1, ?2, 1)
                 ON CONFLICT(path_a, path_b)
                 DO UPDATE SET occurrences = occurrences + 1",
                params![left, right],
            )?;
        }
    }
    Ok(())
}

fn detect_repository_commands(
    transaction: &rusqlite::Transaction<'_>,
    root: &Path,
) -> Result<(), ContextError> {
    for (manifest, command) in [
        ("Cargo.toml", "cargo test --workspace"),
        ("package.json", "npm test"),
        ("pyproject.toml", "python3 -m pytest"),
        ("go.mod", "go test ./..."),
        ("gradlew", "./gradlew test"),
        ("pom.xml", "mvn verify"),
    ] {
        if root.join(manifest).is_file() {
            transaction.execute(
                "INSERT INTO repository_commands(kind, command, source)
                 VALUES ('test', ?1, ?2)",
                params![command, manifest],
            )?;
        }
    }
    Ok(())
}

fn git_output(root: &Path, arguments: &[&str]) -> Result<Vec<u8>, ContextError> {
    let output = Command::new("git")
        .args(arguments)
        .current_dir(root)
        .env_clear()
        .envs(
            ["PATH", "TMPDIR", "LANG", "LC_ALL"]
                .into_iter()
                .filter_map(|key| std::env::var(key).ok().map(|value| (key, value))),
        )
        .stdin(Stdio::null())
        .output()?;
    if !output.status.success() {
        return Err(ContextError::Git(
            String::from_utf8_lossy(&output.stderr).into_owned(),
        ));
    }
    Ok(output.stdout)
}

fn nul_strings(bytes: &[u8]) -> impl Iterator<Item = String> + '_ {
    bytes
        .split(|byte| *byte == 0)
        .filter(|value| !value.is_empty())
        .map(|value| String::from_utf8_lossy(value).into_owned())
}

fn chunks(content: &str) -> Vec<(usize, usize, String)> {
    let lines: Vec<_> = content.lines().collect();
    if lines.is_empty() {
        return Vec::new();
    }
    let mut result = Vec::new();
    let mut start = 0;
    while start < lines.len() {
        let end = (start + CHUNK_LINES).min(lines.len());
        result.push((start + 1, end, lines[start..end].join("\n")));
        if end == lines.len() {
            break;
        }
        start = end.saturating_sub(CHUNK_OVERLAP);
    }
    result
}

/// Walk tree-sitter AST to find top-level definition node boundaries.
///
/// Returns a sorted, non-overlapping list of (start_line, end_line, None)
/// spans that enclose function, class, module, struct, enum, trait, and
/// interface definitions. Empty span gaps are filled with fixed-line-window
/// chunks so every source line belongs to at least one chunk.
fn chunk_boundaries(node: Node<'_>) -> Vec<(usize, usize)> {
    let mut boundaries = Vec::new();
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if is_definition_node_kind(child.kind()) {
            let start = child.start_position().row + 1;
            let end = child.end_position().row + 1;
            if end > start {
                boundaries.push((start, end));
            }
        }
        // Recurse into non-leaf children that aren't themselves definitions,
        // to catch module-level definitions nested inside blocks.
        if child.child_count() > 0 && !is_definition_node_kind(child.kind()) {
            boundaries.extend(chunk_boundaries(child));
        }
    }
    boundaries.sort_by_key(|b| b.0);
    boundaries.dedup();
    boundaries
}

/// Splits source text into chunks at AST-node boundaries when tree-sitter
/// grammar is available; falls back to fixed-window chunking otherwise.
fn chunk_by_nodes(language: LanguageId, source: &[u8]) -> Vec<(usize, usize, String)> {
    let text = String::from_utf8_lossy(source);
    let lines: Vec<_> = text.lines().collect();
    if lines.is_empty() {
        return Vec::new();
    }
    let boundaries = match grammar(language)
        .and_then(|grammar| {
            let mut parser = Parser::new();
            parser.set_language(&grammar).ok()?;
            parser.parse(source, None)
        })
        .map(|tree| chunk_boundaries(tree.root_node()))
    {
        Some(boundaries) if !boundaries.is_empty() => boundaries,
        _ => return chunks(&text),
    };

    let mut result = Vec::new();
    let mut cursor = 1_usize; // 1-indexed line
    let total = lines.len();

    for (def_start, def_end) in &boundaries {
        let def_start = (*def_start).min(total);
        let def_end = (*def_end).min(total);

        // Emit fixed-window chunks for lines before this definition.
        while cursor < def_start {
            let chunk_end = (cursor + CHUNK_LINES - 1).min(def_start - 1).min(total);
            result.push((cursor, chunk_end, lines[cursor - 1..chunk_end].join("\n")));
            if chunk_end >= def_start - 1 || chunk_end == total {
                cursor = chunk_end + 1;
                break;
            }
            cursor = (chunk_end + 1).saturating_sub(CHUNK_OVERLAP);
        }

        // Emit the definition as one chunk (possibly split if too large).
        if def_start <= total {
            let mut node_start = def_start;
            while node_start < def_end {
                let node_end = (node_start + CHUNK_LINES - 1).min(def_end - 1).min(total);
                result.push((
                    node_start,
                    node_end,
                    lines[node_start - 1..node_end].join("\n"),
                ));
                if node_end >= def_end - 1 || node_end == total {
                    break;
                }
                node_start = (node_end + 1).saturating_sub(CHUNK_OVERLAP);
            }
            cursor = def_end;
        }
    }

    // Emit fixed-window chunks for remaining trailing lines.
    while cursor <= total {
        let chunk_end = (cursor + CHUNK_LINES - 1).min(total);
        result.push((cursor, chunk_end, lines[cursor - 1..chunk_end].join("\n")));
        if chunk_end == total {
            break;
        }
        cursor = (chunk_end + 1).saturating_sub(CHUNK_OVERLAP);
    }

    result
}

/// Returns true when the tree-sitter node kind is a top-level definition that
/// should anchor a chunk boundary.
fn is_definition_node_kind(kind: &str) -> bool {
    matches!(
        kind,
        "function_item"
            | "struct_item"
            | "enum_item"
            | "trait_item"
            | "impl_item"
            | "mod_item"
            | "function_definition"
            | "class_definition"
            | "function_declaration"
            | "method_definition"
            | "class_declaration"
            | "method_declaration"
            | "interface_declaration"
            | "enum_declaration"
            | "type_declaration"
            | "export_statement"
    )
}

fn fts_query(query: &str) -> String {
    query
        .split(|character: char| !character.is_alphanumeric() && character != '_')
        .filter(|token| token.len() >= 2)
        .take(12)
        .map(|token| format!("\"{}\"", token.replace('"', "")))
        .collect::<Vec<_>>()
        .join(" OR ")
}

fn extract_symbols(
    language: LanguageId,
    path: &Path,
    source: &[u8],
) -> Result<Vec<SymbolRecord>, ContextError> {
    let Some(grammar) = grammar(language) else {
        return Ok(fallback_symbols(language, path, source));
    };
    let mut parser = Parser::new();
    parser
        .set_language(&grammar)
        .map_err(|error| ContextError::Parser(error.to_string()))?;
    let tree = parser
        .parse(source, None)
        .ok_or_else(|| ContextError::Parser("tree-sitter returned no tree".into()))?;
    let mut symbols = Vec::new();
    collect_symbols(tree.root_node(), source, path, &mut symbols);
    Ok(symbols)
}

fn grammar(language: LanguageId) -> Option<Language> {
    match language {
        LanguageId::Rust => Some(tree_sitter_rust::LANGUAGE.into()),
        LanguageId::Python => Some(tree_sitter_python::LANGUAGE.into()),
        LanguageId::TypeScript => Some(tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()),
        LanguageId::JavaScript => Some(tree_sitter_typescript::LANGUAGE_TSX.into()),
        LanguageId::Java => Some(tree_sitter_java::LANGUAGE.into()),
        LanguageId::Go => Some(tree_sitter_go::LANGUAGE.into()),
        _ => None,
    }
}

fn collect_symbols(node: Node<'_>, source: &[u8], path: &Path, output: &mut Vec<SymbolRecord>) {
    if is_symbol_kind(node.kind())
        && let Some(name_node) = node.child_by_field_name("name")
        && let Ok(name) = name_node.utf8_text(source)
    {
        output.push(SymbolRecord {
            path: path.to_path_buf(),
            name: name.into(),
            kind: node.kind().into(),
            line: node.start_position().row + 1,
        });
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_symbols(child, source, path, output);
    }
}

fn is_symbol_kind(kind: &str) -> bool {
    matches!(
        kind,
        "function_item"
            | "struct_item"
            | "enum_item"
            | "trait_item"
            | "mod_item"
            | "function_definition"
            | "class_definition"
            | "function_declaration"
            | "method_definition"
            | "class_declaration"
            | "method_declaration"
            | "interface_declaration"
            | "enum_declaration"
            | "type_declaration"
    )
}

fn fallback_symbols(language: LanguageId, path: &Path, source: &[u8]) -> Vec<SymbolRecord> {
    if language != LanguageId::Kotlin {
        return Vec::new();
    }
    String::from_utf8_lossy(source)
        .lines()
        .enumerate()
        .filter_map(|(line, text)| {
            let trimmed = text.trim_start();
            let rest = ["class ", "interface ", "object ", "fun "]
                .iter()
                .find_map(|prefix| trimmed.strip_prefix(prefix))?;
            let name = rest
                .split(|character: char| !character.is_alphanumeric() && character != '_')
                .next()?;
            (!name.is_empty()).then(|| SymbolRecord {
                path: path.to_path_buf(),
                name: name.into(),
                kind: "kotlin_declaration".into(),
                line: line + 1,
            })
        })
        .collect()
}

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS files (
    path TEXT PRIMARY KEY,
    language TEXT NOT NULL,
    bytes INTEGER NOT NULL,
    digest TEXT NOT NULL,
    generated INTEGER NOT NULL,
    sensitive INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS symbols (
    path TEXT NOT NULL,
    name TEXT NOT NULL,
    kind TEXT NOT NULL,
    line INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_symbols_name ON symbols(name);
CREATE TABLE IF NOT EXISTS imports (
    source_path TEXT NOT NULL,
    target TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_imports_target ON imports(target);
CREATE TABLE IF NOT EXISTS test_relations (
    test_path TEXT NOT NULL,
    source_path TEXT NOT NULL,
    confidence REAL NOT NULL
);
CREATE TABLE IF NOT EXISTS git_files (
    path TEXT PRIMARY KEY,
    last_commit INTEGER,
    changed INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS cochanges (
    path_a TEXT NOT NULL,
    path_b TEXT NOT NULL,
    occurrences INTEGER NOT NULL,
    PRIMARY KEY(path_a, path_b)
);
CREATE TABLE IF NOT EXISTS repository_commands (
    kind TEXT NOT NULL,
    command TEXT NOT NULL,
    source TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS index_lifecycle (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    stage INTEGER NOT NULL
);
INSERT OR IGNORE INTO index_lifecycle(singleton, stage) VALUES (1, 0);
CREATE VIRTUAL TABLE IF NOT EXISTS chunks USING fts5(
    path UNINDEXED,
    start_line UNINDEXED,
    end_line UNINDEXED,
    content,
    tokenize = 'unicode61'
);
"#;

#[derive(Debug, Error)]
pub enum ContextError {
    #[error("repository I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("repository walk failed: {0}")]
    Walk(#[from] ignore::Error),
    #[error("index database failed: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("repository path escaped root: {0}")]
    PathEscape(PathBuf),
    #[error("invalid task-related repository path: {0}")]
    InvalidRequestedPath(PathBuf),
    #[error("invalid indexing budget: {0}")]
    InvalidBudget(String),
    #[error("invalid persisted indexing lifecycle stage: {0}")]
    InvalidLifecycleStage(i64),
    #[error("indexed row count exceeded this platform's addressable size")]
    DatabaseCountOverflow,
    #[error("Tier 2 work belongs to a different context index")]
    Tier2ContextMismatch,
    #[error("parser failed: {0}")]
    Parser(String),
    #[error("git metadata failed: {0}")]
    Git(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn open_test_index(repository: &Path) -> ContextIndex {
        ContextIndex::open(repository, &repository.join(".purrcode").join("context.db")).unwrap()
    }

    fn bounded_tier2_policy() -> Tier2Policy {
        Tier2Policy {
            maximum_entries_per_step: 64,
            maximum_files_per_step: 4,
            maximum_bytes_per_step: 4 * 1024,
            maximum_total_entries: 1_000,
            maximum_total_files: 100,
            maximum_total_bytes: 1024 * 1024,
            maximum_file_bytes: 1024,
            pause_at_input_latency_millis: 100,
        }
    }

    #[test]
    fn a_task_that_matches_no_filename_still_gets_repository_context() {
        // Observed in a real run: "Indexed 0 file(s), 0 symbol(s)", after which
        // the planner had nothing to plan against. Selection is by filename, so
        // this was not an edge case — most objectives share no word with any
        // file in the tree.
        let repository = tempfile::tempdir().unwrap();
        fs::create_dir(repository.path().join("src")).unwrap();
        fs::write(
            repository.path().join("src/lib.rs"),
            "pub struct OrderService;\npub fn paginate_orders() {}\n",
        )
        .unwrap();
        fs::write(repository.path().join("src/main.rs"), "fn main() {}\n").unwrap();
        let mut index = open_test_index(repository.path());
        index.ensure_tier0(&Tier0Budget::default()).unwrap();
        let request = Tier1Request {
            mentioned_paths: Vec::new(),
            related_paths: Vec::new(),
            filename_terms: vec!["build".into(), "converter".into()],
            languages: BTreeSet::new(),
            include_changed_files: true,
            budget: Tier1Budget::default(),
        };
        let report = index.index_tier1(&request).unwrap();
        assert!(
            report.selection_widened,
            "nothing matched, so it must widen"
        );
        assert!(report.index_report.indexed_files >= 2);
        assert!(report.index_report.symbols >= 2);
        // Retrieval can only rank what was indexed.
        let hits = index
            .retrieve("paginate orders", &RetrievalBudget::default())
            .unwrap();
        assert_eq!(hits[0].path, PathBuf::from("src/lib.rs"));
    }

    #[test]
    fn a_task_that_names_a_file_stays_scoped_to_it() {
        // Widening must be the fallback, not the behaviour: a task that named
        // its file would otherwise drag in the whole repository and bury it.
        let repository = tempfile::tempdir().unwrap();
        fs::create_dir(repository.path().join("src")).unwrap();
        fs::write(repository.path().join("src/lib.rs"), "pub fn a() {}\n").unwrap();
        fs::write(repository.path().join("src/other.rs"), "pub fn b() {}\n").unwrap();
        let mut index = open_test_index(repository.path());
        index.ensure_tier0(&Tier0Budget::default()).unwrap();
        let report = index
            .index_tier1(&Tier1Request {
                mentioned_paths: vec![PathBuf::from("src/lib.rs")],
                related_paths: Vec::new(),
                filename_terms: Vec::new(),
                languages: BTreeSet::new(),
                include_changed_files: false,
                budget: Tier1Budget::default(),
            })
            .unwrap();
        assert!(!report.selection_widened);
        assert_eq!(report.selected_paths, vec![PathBuf::from("src/lib.rs")]);
    }

    #[test]
    fn tier1_indexes_a_worktree_nested_under_purrcode() {
        // Session worktrees are commonly stored below `.purrcode/worktrees`.
        // The absolute ancestor must not make the worktree root look pruned.
        let container = tempfile::tempdir().unwrap();
        let repository = container.path().join(".purrcode/worktrees/session");
        fs::create_dir_all(repository.join("src")).unwrap();
        fs::write(
            repository.join("src/lib.rs"),
            "pub struct NestedWorktree;\npub fn indexed_from_nested_worktree() {}\n",
        )
        .unwrap();

        let mut index = open_test_index(&repository);
        let report = index.index_tier1(&Tier1Request::default()).unwrap();

        assert!(report.index_report.indexed_files >= 1);
        assert!(report.index_report.symbols >= 2);
        assert!(report.selected_paths.contains(&PathBuf::from("src/lib.rs")));
    }

    #[test]
    fn indexes_symbols_and_excludes_sensitive_content() {
        let repository = tempfile::tempdir().unwrap();
        fs::create_dir(repository.path().join("src")).unwrap();
        fs::write(
            repository.path().join("src/lib.rs"),
            "pub struct OrderService;\npub fn paginate_orders() {}\n",
        )
        .unwrap();
        fs::write(repository.path().join(".env"), "SUPER_SECRET=value").unwrap();
        let database = repository.path().join("index.db");
        let mut index = ContextIndex::open(repository.path(), &database).unwrap();
        let report = index.rebuild().unwrap();
        assert_eq!(report.sensitive_files, 1);
        assert!(report.symbols >= 2);
        let hits = index
            .retrieve("paginate orders", &RetrievalBudget::default())
            .unwrap();
        assert_eq!(hits[0].path, PathBuf::from("src/lib.rs"));
        assert!(!hits.iter().any(|hit| hit.content.contains("SUPER_SECRET")));
        assert_eq!(index.symbols("OrderService", 10).unwrap().len(), 1);
    }

    #[test]
    fn retrieval_honors_strict_byte_budget() {
        let repository = tempfile::tempdir().unwrap();
        fs::write(
            repository.path().join("notes.md"),
            "pagination ".repeat(1000),
        )
        .unwrap();
        let database = repository.path().join("index.db");
        let mut index = ContextIndex::open(repository.path(), &database).unwrap();
        index.rebuild().unwrap();
        let hits = index
            .retrieve(
                "pagination",
                &RetrievalBudget {
                    maximum_hits: 2,
                    maximum_bytes: 37,
                },
            )
            .unwrap();
        assert!(hits.iter().map(|hit| hit.content.len()).sum::<usize>() <= 37);
    }

    #[test]
    fn tier0_lists_only_bounded_root_metadata_and_indexes_only_root_control_files() {
        let repository = tempfile::tempdir().unwrap();
        fs::create_dir_all(repository.path().join("src/deep")).unwrap();
        for index in 0..250 {
            fs::write(
                repository
                    .path()
                    .join("src/deep")
                    .join(format!("file-{index}.rs")),
                "nested_content_must_remain_lazy",
            )
            .unwrap();
        }
        fs::write(
            repository.path().join("Cargo.toml"),
            "tierzero_manifest_token",
        )
        .unwrap();
        fs::write(
            repository.path().join("AGENTS.md"),
            "tierzero_instruction_token",
        )
        .unwrap();
        fs::write(
            repository.path().join("README.md"),
            "top_level_metadata_only_token",
        )
        .unwrap();
        fs::write(repository.path().join(".env"), "TIER_ZERO_SECRET=value").unwrap();

        let mut index = open_test_index(repository.path());
        let snapshot = index
            .rebuild_tier0(&Tier0Budget {
                maximum_entries: 8,
                maximum_content_bytes: 1024,
                maximum_file_bytes: 512,
            })
            .unwrap();

        assert!(snapshot.entries.len() <= 8);
        assert!(
            snapshot
                .entries
                .iter()
                .all(|entry| entry.path.components().count() == 1)
        );
        assert!(
            snapshot
                .entries
                .iter()
                .any(|entry| entry.path == Path::new(".env") && entry.sensitive)
        );
        assert_eq!(snapshot.index_report.indexed_files, 2);
        assert_eq!(snapshot.index_report.dependency_manifests, 1);
        assert_eq!(snapshot.index_report.instruction_files, 1);
        assert_eq!(snapshot.index_report.sensitive_files, 0);
        assert!(
            !index
                .retrieve(
                    "nested_content_must_remain_lazy",
                    &RetrievalBudget::default()
                )
                .unwrap()
                .iter()
                .any(|hit| hit.content.contains("nested_content_must_remain_lazy"))
        );
        assert!(
            index
                .retrieve("tierzero_manifest_token", &RetrievalBudget::default())
                .unwrap()
                .iter()
                .any(|hit| hit.path == Path::new("Cargo.toml"))
        );
        assert!(
            index
                .retrieve("top_level_metadata_only_token", &RetrievalBudget::default())
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn tier0_content_and_entry_limits_are_exact() {
        let repository = tempfile::tempdir().unwrap();
        fs::write(repository.path().join("AGENTS.md"), "12345").unwrap();
        fs::write(repository.path().join("Cargo.toml"), "abcde").unwrap();
        fs::write(repository.path().join("README.md"), "not-indexed").unwrap();
        let mut index = open_test_index(repository.path());

        let exact = index
            .rebuild_tier0(&Tier0Budget {
                maximum_entries: 2,
                maximum_content_bytes: 10,
                maximum_file_bytes: 5,
            })
            .unwrap();
        assert_eq!(exact.entries.len(), 2);
        assert!(exact.entry_limit_reached);
        assert_eq!(exact.indexed_content_bytes, 10);
        assert_eq!(exact.index_report.indexed_files, 2);
        assert!(!exact.byte_limit_reached);

        let one_byte_short = index
            .rebuild_tier0(&Tier0Budget {
                maximum_entries: 3,
                maximum_content_bytes: 9,
                maximum_file_bytes: 5,
            })
            .unwrap();
        assert_eq!(one_byte_short.indexed_content_bytes, 5);
        assert_eq!(one_byte_short.index_report.indexed_files, 1);
        assert!(one_byte_short.byte_limit_reached);
    }

    #[test]
    fn ensure_tier0_runs_once_and_preserves_task_index_after_reopen() {
        let repository = tempfile::tempdir().unwrap();
        fs::write(repository.path().join("Cargo.toml"), "manifest_token").unwrap();
        fs::create_dir_all(repository.path().join("src")).unwrap();
        fs::write(
            repository.path().join("src/relevant.rs"),
            "pub fn task_specific_token() {}",
        )
        .unwrap();
        let database = repository.path().join(".purrcode").join("context.db");
        let mut index = ContextIndex::open(repository.path(), &database).unwrap();

        let first = index.ensure_tier0(&Tier0Budget::default()).unwrap();
        assert!(first.rebuilt);
        assert_eq!(first.stage, IndexLifecycleStage::Tier0Ready);
        assert_eq!(
            index.lifecycle_stage().unwrap(),
            IndexLifecycleStage::Tier0Ready
        );
        assert!(
            index
                .retrieve("task_specific_token", &RetrievalBudget::default())
                .unwrap()
                .is_empty()
        );

        index
            .index_tier1(&Tier1Request {
                mentioned_paths: vec![PathBuf::from("src/relevant.rs")],
                include_changed_files: false,
                ..Tier1Request::default()
            })
            .unwrap();
        assert_eq!(
            index.lifecycle_stage().unwrap(),
            IndexLifecycleStage::TaskReady
        );
        drop(index);

        let mut reopened = ContextIndex::open(repository.path(), &database).unwrap();
        let second = reopened.ensure_tier0(&Tier0Budget::default()).unwrap();
        assert!(!second.rebuilt);
        assert!(second.snapshot.is_none());
        assert_eq!(second.stage, IndexLifecycleStage::TaskReady);
        assert!(
            reopened
                .retrieve("task_specific_token", &RetrievalBudget::default())
                .unwrap()
                .iter()
                .any(|hit| hit.path == Path::new("src/relevant.rs"))
        );
    }

    #[test]
    fn tier1_indexes_only_task_selected_paths_filename_matches_and_languages() {
        let repository = tempfile::tempdir().unwrap();
        fs::create_dir_all(repository.path().join("src")).unwrap();
        fs::create_dir_all(repository.path().join("docs")).unwrap();
        fs::write(
            repository.path().join("src/mentioned.rs"),
            "pub fn mentioned_only_token() {}",
        )
        .unwrap();
        fs::write(
            repository.path().join("src/unrelated.rs"),
            "pub fn unrelated_only_token() {}",
        )
        .unwrap();
        fs::write(
            repository.path().join("docs/task-guide.md"),
            "guide_only_token",
        )
        .unwrap();
        fs::write(
            repository.path().join("worker.py"),
            "def language_selected_token():\n    pass\n",
        )
        .unwrap();
        let mut index = open_test_index(repository.path());
        let request = Tier1Request {
            mentioned_paths: vec![PathBuf::from("src/mentioned.rs")],
            related_paths: Vec::new(),
            filename_terms: vec!["task-guide".into()],
            languages: BTreeSet::from([LanguageId::Python]),
            include_changed_files: false,
            budget: Tier1Budget::default(),
        };
        let report = index.index_tier1(&request).unwrap();

        assert_eq!(report.index_report.indexed_files, 3);
        assert!(
            report
                .selected_paths
                .contains(&PathBuf::from("src/mentioned.rs"))
        );
        assert!(
            report
                .selected_paths
                .contains(&PathBuf::from("docs/task-guide.md"))
        );
        assert!(report.selected_paths.contains(&PathBuf::from("worker.py")));
        assert!(
            index
                .retrieve("mentioned_only_token", &RetrievalBudget::default())
                .unwrap()
                .iter()
                .any(|hit| hit.path == Path::new("src/mentioned.rs"))
        );
        assert!(
            index
                .retrieve("guide_only_token", &RetrievalBudget::default())
                .unwrap()
                .iter()
                .any(|hit| hit.path == Path::new("docs/task-guide.md"))
        );
        assert!(
            index
                .retrieve("language_selected_token", &RetrievalBudget::default())
                .unwrap()
                .iter()
                .any(|hit| hit.path == Path::new("worker.py"))
        );
        assert!(
            index
                .retrieve("unrelated_only_token", &RetrievalBudget::default())
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn tier1_includes_changed_files_without_broad_content_indexing() {
        let repository = tempfile::tempdir().unwrap();
        let status = Command::new("git")
            .args(["init", "--quiet"])
            .current_dir(repository.path())
            .stdin(Stdio::null())
            .status()
            .unwrap();
        assert!(status.success());
        fs::write(
            repository.path().join("changed.rs"),
            "pub fn changed_file_token() {}",
        )
        .unwrap();
        fs::write(
            repository.path().join("ignored.txt"),
            "also untracked but intentionally still a changed file",
        )
        .unwrap();
        let mut index = open_test_index(repository.path());
        let report = index
            .index_tier1(&Tier1Request {
                include_changed_files: true,
                ..Tier1Request::default()
            })
            .unwrap();
        assert!(report.selected_paths.contains(&PathBuf::from("changed.rs")));
        assert!(
            report
                .selected_paths
                .contains(&PathBuf::from("ignored.txt"))
        );
        assert!(
            index
                .retrieve("changed_file_token", &RetrievalBudget::default())
                .unwrap()
                .iter()
                .any(|hit| hit.path == Path::new("changed.rs"))
        );
    }

    #[test]
    fn tier1_sensitive_paths_are_metadata_only_and_path_escape_fails_closed() {
        let repository = tempfile::tempdir().unwrap();
        fs::create_dir(repository.path().join(".ssh")).unwrap();
        fs::write(
            repository.path().join(".ssh/id_ed25519"),
            "ssh_private_material",
        )
        .unwrap();
        fs::write(
            repository.path().join(".env"),
            "DATABASE_PASSWORD=database_private_material",
        )
        .unwrap();
        let mut index = open_test_index(repository.path());
        let report = index
            .index_tier1(&Tier1Request {
                mentioned_paths: vec![PathBuf::from(".ssh/id_ed25519"), PathBuf::from(".env")],
                include_changed_files: false,
                ..Tier1Request::default()
            })
            .unwrap();
        assert_eq!(report.index_report.sensitive_files, 2);
        assert!(
            index
                .retrieve("private_material", &RetrievalBudget::default())
                .unwrap()
                .is_empty()
        );
        let chunks: i64 = index
            .connection
            .query_row(
                "SELECT COUNT(*) FROM chunks WHERE path IN ('.env', '.ssh/id_ed25519')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(chunks, 0);

        let error = index
            .index_tier1(&Tier1Request {
                mentioned_paths: vec![PathBuf::from("../outside")],
                include_changed_files: false,
                ..Tier1Request::default()
            })
            .unwrap_err();
        assert!(matches!(error, ContextError::InvalidRequestedPath(_)));
    }

    #[test]
    fn tier2_is_explicitly_driven_and_cancellation_is_terminal() {
        let repository = tempfile::tempdir().unwrap();
        for file in 0..8 {
            fs::write(
                repository.path().join(format!("source-{file}.rs")),
                format!("pub fn cancellable_{file}() {{}}"),
            )
            .unwrap();
        }
        let mut index = open_test_index(repository.path());
        let mut policy = bounded_tier2_policy();
        policy.maximum_files_per_step = 1;
        let mut work = index.begin_tier2(policy).unwrap();
        assert_eq!(work.status(), Tier2Status::Ready);
        assert_eq!(work.report().indexed_files, 0);

        let first = index
            .drive_tier2(&mut work, IndexingSignals::default())
            .unwrap();
        assert_eq!(first.indexed_files, 1);
        assert_eq!(first.cumulative_index_report.indexed_files, 1);
        let cancelled = index
            .drive_tier2(
                &mut work,
                IndexingSignals {
                    cancel_requested: true,
                    ..IndexingSignals::default()
                },
            )
            .unwrap();
        assert_eq!(cancelled.status, Tier2Status::Cancelled);
        assert_eq!(cancelled.indexed_files, 0);
        let after_cancel = index
            .drive_tier2(&mut work, IndexingSignals::default())
            .unwrap();
        assert_eq!(after_cancel.status, Tier2Status::Cancelled);
        assert_eq!(after_cancel.cumulative_index_report.indexed_files, 1);
    }

    #[test]
    fn tier2_pauses_for_pressure_generation_and_latency_and_resumes() {
        let repository = tempfile::tempdir().unwrap();
        fs::write(
            repository.path().join("source.rs"),
            "pub fn resumable_tier_two_token() {}",
        )
        .unwrap();
        let mut index = open_test_index(repository.path());
        let policy = bounded_tier2_policy();
        let threshold = policy.pause_at_input_latency_millis;
        let mut work = index.begin_tier2(policy.clone()).unwrap();

        let high_pressure = index
            .drive_tier2(
                &mut work,
                IndexingSignals {
                    memory_pressure: MemoryPressure::High,
                    ..IndexingSignals::default()
                },
            )
            .unwrap();
        assert_eq!(
            high_pressure.status,
            Tier2Status::Paused(IndexPauseReason::HighMemoryPressure)
        );
        assert_eq!(high_pressure.cumulative_examined_entries, 0);

        let generation = index
            .drive_tier2(
                &mut work,
                IndexingSignals {
                    generation_active: true,
                    ..IndexingSignals::default()
                },
            )
            .unwrap();
        assert_eq!(
            generation.status,
            Tier2Status::Paused(IndexPauseReason::GenerationActive)
        );
        let latency = index
            .drive_tier2(
                &mut work,
                IndexingSignals {
                    input_latency_millis: threshold,
                    ..IndexingSignals::default()
                },
            )
            .unwrap();
        assert_eq!(
            latency.status,
            Tier2Status::Paused(IndexPauseReason::InputLatency)
        );

        let resumed = index
            .drive_tier2(&mut work, IndexingSignals::default())
            .unwrap();
        assert_eq!(resumed.status, Tier2Status::Completed);
        assert!(
            index
                .retrieve("resumable_tier_two_token", &RetrievalBudget::default())
                .unwrap()
                .iter()
                .any(|hit| hit.path == Path::new("source.rs"))
        );

        let mut stopped_work = index.begin_tier2(policy).unwrap();
        let stopped = index
            .drive_tier2(
                &mut stopped_work,
                IndexingSignals {
                    memory_pressure: MemoryPressure::Critical,
                    ..IndexingSignals::default()
                },
            )
            .unwrap();
        assert_eq!(
            stopped.status,
            Tier2Status::Stopped(IndexStopReason::CriticalMemoryPressure)
        );
        assert_eq!(
            index
                .drive_tier2(&mut stopped_work, IndexingSignals::default())
                .unwrap()
                .status,
            Tier2Status::Stopped(IndexStopReason::CriticalMemoryPressure)
        );
    }

    #[test]
    fn tier2_large_repository_steps_and_total_budgets_are_strict() {
        let repository = tempfile::tempdir().unwrap();
        for file in 0..31 {
            fs::write(
                repository.path().join(format!("batch-{file:02}.txt")),
                format!("batch_boundary_token_{file:02}"),
            )
            .unwrap();
        }
        let mut index = open_test_index(repository.path());
        let policy = Tier2Policy {
            maximum_entries_per_step: 8,
            maximum_files_per_step: 3,
            maximum_bytes_per_step: 96,
            maximum_total_entries: 1_000,
            maximum_total_files: 100,
            maximum_total_bytes: 4096,
            maximum_file_bytes: 32,
            pause_at_input_latency_millis: 100,
        };
        let mut work = index.begin_tier2(policy).unwrap();
        for _ in 0..100 {
            let step = index
                .drive_tier2(&mut work, IndexingSignals::default())
                .unwrap();
            assert!(step.examined_entries <= 8);
            assert!(step.indexed_files <= 3);
            assert!(step.indexed_bytes <= 96);
            if step.status == Tier2Status::Completed {
                break;
            }
            assert_eq!(step.status, Tier2Status::Ready);
        }
        assert_eq!(work.status(), Tier2Status::Completed);
        assert_eq!(work.report().indexed_files, 31);

        let mut capped_policy = bounded_tier2_policy();
        capped_policy.maximum_files_per_step = 2;
        capped_policy.maximum_total_files = 2;
        let mut capped_work = index.begin_tier2(capped_policy).unwrap();
        for _ in 0..3 {
            let step = index
                .drive_tier2(&mut capped_work, IndexingSignals::default())
                .unwrap();
            if step.status.is_terminal() {
                break;
            }
        }
        assert_eq!(capped_work.status(), Tier2Status::BudgetExhausted);
        assert_eq!(capped_work.report().indexed_files, 2);
    }

    #[test]
    fn tier2_rejects_zero_and_internally_inconsistent_limits() {
        let repository = tempfile::tempdir().unwrap();
        let index = open_test_index(repository.path());
        let mut policy = bounded_tier2_policy();
        policy.maximum_entries_per_step = 0;
        assert!(matches!(
            index.begin_tier2(policy),
            Err(ContextError::InvalidBudget(_))
        ));

        let mut policy = bounded_tier2_policy();
        policy.maximum_file_bytes = policy.maximum_bytes_per_step + 1;
        assert!(matches!(
            index.begin_tier2(policy),
            Err(ContextError::InvalidBudget(_))
        ));
    }
}
