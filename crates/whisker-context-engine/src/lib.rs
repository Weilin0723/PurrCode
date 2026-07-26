//! Bounded local repository indexing and hybrid lexical/symbol retrieval.

use ignore::WalkBuilder;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use thiserror::Error;
use tree_sitter::{Language, Node, Parser};

const MAX_INDEXED_FILE_BYTES: u64 = 2 * 1024 * 1024;
const CHUNK_LINES: usize = 100;
const CHUNK_OVERLAP: usize = 10;

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

    pub fn rebuild(&mut self) -> Result<IndexReport, ContextError> {
        let transaction = self.connection.transaction()?;
        transaction.execute("DELETE FROM symbols", [])?;
        transaction.execute("DELETE FROM imports", [])?;
        transaction.execute("DELETE FROM test_relations", [])?;
        transaction.execute("DELETE FROM git_files", [])?;
        transaction.execute("DELETE FROM cochanges", [])?;
        transaction.execute("DELETE FROM repository_commands", [])?;
        transaction.execute("DELETE FROM chunks", [])?;
        transaction.execute("DELETE FROM files", [])?;
        let mut report = IndexReport::default();
        let walker = WalkBuilder::new(&self.root)
            .hidden(false)
            .git_ignore(true)
            .git_global(true)
            .git_exclude(true)
            .filter_entry(|entry| !is_pruned(entry.path()))
            .build();
        for entry in walker {
            let entry = entry?;
            if is_index_storage(entry.path(), &self.database) {
                continue;
            }
            let metadata = entry.metadata()?;
            if !metadata.is_file() {
                continue;
            }
            let relative = entry
                .path()
                .strip_prefix(&self.root)
                .map_err(|_| ContextError::PathEscape(entry.path().to_path_buf()))?
                .to_path_buf();
            if metadata.len() > MAX_INDEXED_FILE_BYTES {
                report.skipped_large_files += 1;
                continue;
            }
            let language = language_for(&relative);
            let sensitive = is_sensitive(&relative);
            let bytes = fs::read(entry.path())?;
            if bytes.contains(&0) {
                continue;
            }
            let text = String::from_utf8_lossy(&bytes);
            let generated = is_generated(&relative, &text);
            let digest = blake3::hash(&bytes).to_hex().to_string();
            transaction.execute(
                "INSERT INTO files(path, language, bytes, digest, generated, sensitive)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    relative.to_string_lossy(),
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
                        params![
                            relative.to_string_lossy(),
                            source.to_string_lossy(),
                            0.5_f64
                        ],
                    )?;
                }
            }
            if sensitive {
                continue;
            }
            let symbols = extract_symbols(language, &relative, &bytes)?;
            for symbol in symbols {
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
                    params![relative.to_string_lossy(), import],
                )?;
                report.imports += 1;
            }
            for (start, end, chunk) in chunks(&text) {
                transaction.execute(
                    "INSERT INTO chunks(path, start_line, end_line, content)
                     VALUES (?1, ?2, ?3, ?4)",
                    params![relative.to_string_lossy(), start, end, chunk],
                )?;
            }
        }
        index_git_metadata(&transaction, &self.root)?;
        detect_repository_commands(&transaction, &self.root)?;
        transaction.commit()?;
        Ok(report)
    }

    pub fn retrieve(
        &self,
        query: &str,
        budget: &RetrievalBudget,
    ) -> Result<Vec<ContextHit>, ContextError> {
        if budget.maximum_hits == 0 || budget.maximum_bytes == 0 {
            return Ok(Vec::new());
        }
        let fts_query = fts_query(query);
        if fts_query.is_empty() {
            return Ok(Vec::new());
        }
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
            params![fts_query, query, (budget.maximum_hits * 4) as i64],
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
        let mut hits = Vec::new();
        let mut bytes = 0;
        let mut seen = BTreeSet::new();
        for row in rows {
            let mut hit = row?;
            if !seen.insert((hit.path.clone(), hit.start_line)) {
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

fn is_pruned(path: &Path) -> bool {
    path.components().any(|component| {
        matches!(
            component.as_os_str().to_str(),
            Some(".git" | ".purrcode" | "node_modules" | "target" | "dist" | "build")
        )
    })
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
        || lower.contains("/.ssh/")
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
    if is_symbol_kind(node.kind()) {
        if let Some(name_node) = node.child_by_field_name("name") {
            if let Ok(name) = name_node.utf8_text(source) {
                output.push(SymbolRecord {
                    path: path.to_path_buf(),
                    name: name.into(),
                    kind: node.kind().into(),
                    line: node.start_position().row + 1,
                });
            }
        }
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
    #[error("parser failed: {0}")]
    Parser(String),
    #[error("git metadata failed: {0}")]
    Git(String),
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
