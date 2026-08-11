//! Producers for the project intelligence graph (v1.3 §4.5 / §8 PR7).
//!
//! Every edge here is derived from something that ACTUALLY HAPPENED or from
//! source the repository actually contains. That constraint is the whole point
//! of the module: the first producer scanned `ActionProposed`, so a delete
//! PawGate refused still recorded "this session modified auth.rs". A graph that
//! records intentions rather than effects gives later sessions confident,
//! wrong answers, which is worse than an empty graph.
//!
//! Producers, and where their evidence comes from:
//!
//! | edge          | source                                                  |
//! |---------------|---------------------------------------------------------|
//! | `ModifiedBy`  | `ToolEvidenceRecorded` with a SUCCEEDED outcome         |
//! | `CoChanged`   | two files in one session's validated effects            |
//! | `DefinedIn`   | symbol declarations parsed from the changed file        |
//! | `Imports`     | import/`use` statements resolved to repository paths    |
//! | `Tests`       | a test file that imports or names a changed module      |
//! | `FailedWith`  | `ValidationRecorded { status: Failed }`                 |
//! | `RelatesTo`   | project memory entries that name a repository path      |
//!
//! Everything is best-effort and bounded: a graph write failure never fails a
//! session, and no producer reads an unbounded amount of a file.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use chrono::Utc;
use purrcode_ninelives::{ProjectMemoryEntry, SessionStore};
use purrcode_project_graph::{EdgeSource, GraphEdge, GraphNode, NodeId, ProjectGraph};
use purrcode_runtime_core::{
    ExecutionOutcome, GraphEdgeKind, GraphNodeKind, SessionEvent, SessionId, ValidationStatus,
};

/// Largest file a producer will parse. Beyond this the file is recorded as a
/// node but not scanned: a generated 4 MB bundle contributes noise, not
/// structure.
const MAX_PARSE_BYTES: u64 = 512 * 1024;
/// Most symbols recorded per file. A file with 400 declarations is a generated
/// binding table.
const MAX_SYMBOLS_PER_FILE: usize = 64;
/// Most co-change edges per session. A session that rewrites 200 files would
/// otherwise produce 20k edges that relate everything to everything.
const MAX_COCHANGE_FILES: usize = 24;

/// Record everything one finished session taught the graph.
///
/// Returns the number of edges written, which the caller can log; the graph is
/// advisory, so every failure inside is swallowed rather than propagated.
pub(crate) fn record_session_graph(
    store: &SessionStore,
    session_id: SessionId,
    repository: &Path,
    worktree: Option<&Path>,
    database: &Path,
    memory: &[ProjectMemoryEntry],
) -> usize {
    let Ok(events) = store.events(session_id) else {
        return 0;
    };
    let Ok(mut graph) = ProjectGraph::open(database) else {
        return 0;
    };
    let mut writer = GraphWriter {
        graph: &mut graph,
        repository: repository.to_path_buf(),
        nodes: BTreeMap::new(),
        edges: 0,
    };

    // ── The effects that really happened ──────────────────────────────
    let changed = validated_effects(&events);
    if !changed.is_empty() {
        let session_node = writer.upsert(
            GraphNodeKind::Session,
            &format!("session:{}", session_id.0),
            true,
        );
        for path in &changed {
            let Some(file) = writer.file(path) else {
                continue;
            };
            if let Some(session) = session_node.clone() {
                writer.edge(
                    session,
                    file,
                    GraphEdgeKind::ModifiedBy,
                    1000,
                    EdgeSource::EventLog,
                    &format!("session {} recorded a successful write", session_id.0),
                );
            }
        }
    }

    // ── Files that changed together ───────────────────────────────────
    let cochanged: Vec<&PathBuf> = changed.iter().take(MAX_COCHANGE_FILES).collect();
    for (index, left) in cochanged.iter().enumerate() {
        for right in cochanged.iter().skip(index + 1) {
            let (Some(left_id), Some(right_id)) = (writer.file(left), writer.file(right)) else {
                continue;
            };
            writer.edge(
                left_id,
                right_id,
                GraphEdgeKind::CoChanged,
                700,
                EdgeSource::EventLog,
                &format!("changed together in session {}", session_id.0),
            );
        }
    }

    // ── Structure of what changed ─────────────────────────────────────
    // Read from the worktree first for the same reason retrieval does: that is
    // the tree the agent just changed, and the source checkout still holds the
    // pre-edit bytes.
    for path in &changed {
        let Some(content) = read_source(worktree, repository, path) else {
            continue;
        };
        let Some(file) = writer.file(path) else {
            continue;
        };
        for symbol in declared_symbols(path, &content) {
            let key = format!("{}#{}", path.display(), symbol.name);
            let Some(symbol_id) = writer.upsert_with(
                GraphNodeKind::Symbol,
                &key,
                false,
                serde_json::json!({ "line": symbol.line }),
            ) else {
                continue;
            };
            writer.edge(
                symbol_id,
                file.clone(),
                GraphEdgeKind::DefinedIn,
                1000,
                EdgeSource::StaticAnalysis,
                &format!("declared at {}:{}", path.display(), symbol.line),
            );
        }
        for target in imported_paths(repository, worktree, path, &content) {
            let Some(target_id) = writer.file(&target) else {
                continue;
            };
            let kind = if is_test_path(path) {
                GraphEdgeKind::Tests
            } else {
                GraphEdgeKind::Imports
            };
            writer.edge(
                file.clone(),
                target_id,
                kind,
                900,
                EdgeSource::StaticAnalysis,
                &format!("{} references {}", path.display(), target.display()),
            );
        }
    }

    // ── Who names whom ────────────────────────────────────────────────
    // A `References` edge is the call-site relationship `Imports` cannot
    // express: a file that uses `AuthMiddleware` without importing the module
    // by a path this producer can resolve. Restricted to symbols defined by
    // files in the same change set, so it never invents a relationship to a
    // symbol nothing here defines.
    let defined: Vec<(PathBuf, String)> = changed
        .iter()
        .filter_map(|path| read_source(worktree, repository, path).map(|body| (path.clone(), body)))
        .flat_map(|(path, body)| {
            declared_symbols(&path, &body)
                .into_iter()
                .map(move |symbol| (path.clone(), symbol.name))
        })
        .collect();
    for path in &changed {
        let Some(content) = read_source(worktree, repository, path) else {
            continue;
        };
        for (owner, name) in &defined {
            if owner == path || !mentions_symbol(&content, name) {
                continue;
            }
            let key = format!("{}#{name}", owner.display());
            let (Some(file), Some(symbol)) = (
                writer.file(path),
                writer.upsert(GraphNodeKind::Symbol, &key, false),
            ) else {
                continue;
            };
            writer.edge(
                file,
                symbol,
                GraphEdgeKind::References,
                800,
                EdgeSource::StaticAnalysis,
                &format!("{} names {name}", path.display()),
            );
        }
    }

    // ── Failures ──────────────────────────────────────────────────────
    for (stage, detail) in failed_validations(&events) {
        let Some(failure) = writer.upsert_with(
            GraphNodeKind::Failure,
            &format!("failure:{stage}"),
            false,
            serde_json::json!({ "detail": detail }),
        ) else {
            continue;
        };
        for path in changed.iter().take(MAX_COCHANGE_FILES) {
            let Some(file) = writer.file(path) else {
                continue;
            };
            writer.edge(
                file,
                failure.clone(),
                GraphEdgeKind::FailedWith,
                600,
                EdgeSource::EventLog,
                &format!("validation `{stage}` failed while this file was changed"),
            );
        }
    }

    // ── Durable project knowledge ─────────────────────────────────────
    for entry in memory {
        let Some(node) = writer.upsert_with(
            GraphNodeKind::Memory,
            &format!("memory:{}", entry.id),
            false,
            serde_json::json!({ "kind": entry.kind, "confidence": entry.confidence }),
        ) else {
            continue;
        };
        for path in mentioned_paths(repository, &entry.content) {
            let Some(file) = writer.file(&path) else {
                continue;
            };
            writer.edge(
                node.clone(),
                file,
                GraphEdgeKind::RelatesTo,
                500,
                EdgeSource::Heuristic,
                "a project memory entry names this path",
            );
        }
    }

    writer.edges
}

/// The repository-relative paths this session ACTUALLY changed.
///
/// Two sources, both post-execution: `ToolEvidenceRecorded` with a succeeded
/// outcome (every provider, including MCP and skills), and `ExecutionFinished`
/// paired with the validated effect delta recorded alongside it. A proposed
/// action that PawGate denied appears in neither.
fn validated_effects(events: &[SessionEvent]) -> BTreeSet<PathBuf> {
    let mut paths = BTreeSet::new();
    for event in events {
        if let SessionEvent::ToolEvidenceRecorded { evidence } = event
            && let ExecutionOutcome::Succeeded { affected_paths, .. } = &evidence.outcome
        {
            for path in affected_paths {
                if is_repository_relative(path) {
                    paths.insert(path.clone());
                }
            }
        }
    }
    paths
}

/// Validation stages that failed, with their evidence text.
fn failed_validations(events: &[SessionEvent]) -> Vec<(String, String)> {
    events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::ValidationRecorded {
                status: ValidationStatus::Failed,
                evidence,
                action_id,
            } => Some((format!("action/{}", action_id.0), evidence.clone())),
            _ => None,
        })
        .collect()
}

/// A path that may be used as a graph key: relative, non-empty, no traversal.
///
/// Delegates to the graph's own platform-independent check. Using
/// `Path::is_absolute` here was wrong on Windows, where `/etc/passwd` is not
/// "absolute" (no drive prefix) and would have been accepted as a key.
fn is_repository_relative(path: &Path) -> bool {
    path.to_str()
        .is_some_and(purrcode_project_graph::is_repository_relative_key)
}

fn read_source(worktree: Option<&Path>, repository: &Path, path: &Path) -> Option<String> {
    let candidates = worktree
        .map(|worktree| worktree.join(path))
        .into_iter()
        .chain(std::iter::once(repository.join(path)));
    for candidate in candidates {
        let Ok(meta) = std::fs::metadata(&candidate) else {
            continue;
        };
        if meta.len() > MAX_PARSE_BYTES {
            return None;
        }
        if let Ok(content) = std::fs::read_to_string(&candidate) {
            return Some(content);
        }
    }
    None
}

/// One declaration found in a source file.
struct DeclaredSymbol {
    name: String,
    line: u32,
}

/// Top-level declarations, by language.
///
/// Deliberately line-based and conservative rather than a parser per language:
/// the graph's value comes from having *some* structure for every file the
/// agent touches, and a missed symbol degrades `#symbol` to the `git grep` it
/// already falls back to. A WRONG symbol, by contrast, sends retrieval to the
/// wrong file, so every pattern here requires the declaration keyword at the
/// start of a line (allowing `pub`/`export`/`async` prefixes).
fn declared_symbols(path: &Path, content: &str) -> Vec<DeclaredSymbol> {
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default();
    let keywords: &[&str] = match extension {
        "rs" => &["fn", "struct", "enum", "trait", "type", "const", "static"],
        "ts" | "tsx" | "js" | "jsx" | "mjs" => {
            &["function", "class", "interface", "type", "const", "enum"]
        }
        "py" => &["def", "class"],
        "go" => &["func", "type"],
        "swift" | "kt" | "java" => &["func", "class", "struct", "enum", "interface", "protocol"],
        _ => return Vec::new(),
    };
    let mut symbols = Vec::new();
    for (index, line) in content.lines().enumerate() {
        if symbols.len() >= MAX_SYMBOLS_PER_FILE {
            break;
        }
        let trimmed = line.trim_start();
        // Only declarations at the start of a logical line, after the modifiers
        // a declaration may legitimately carry.
        let mut rest = trimmed;
        for modifier in [
            "pub(crate) ",
            "pub(super) ",
            "pub ",
            "export default ",
            "export ",
            "async ",
            "unsafe ",
            "extern ",
            "public ",
            "private ",
            "internal ",
            "final ",
            "static ",
        ] {
            if let Some(stripped) = rest.strip_prefix(modifier) {
                rest = stripped.trim_start();
            }
        }
        let Some(keyword) = keywords
            .iter()
            .find(|keyword| rest.starts_with(&format!("{keyword} ")))
        else {
            continue;
        };
        let after = rest[keyword.len()..].trim_start();
        let name: String = after
            .chars()
            .take_while(|character| character.is_alphanumeric() || *character == '_')
            .collect();
        if name.is_empty() {
            continue;
        }
        symbols.push(DeclaredSymbol {
            name,
            line: index as u32 + 1,
        });
    }
    symbols
}

/// Whether a path looks like a test file, by the conventions of the languages
/// this producer parses.
fn is_test_path(path: &Path) -> bool {
    let text = path.to_string_lossy();
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    text.starts_with("tests/")
        || text.contains("/tests/")
        || text.contains("/__tests__/")
        || name.starts_with("test_")
        || name.contains("_test.")
        || name.contains(".test.")
        || name.contains(".spec.")
}

/// Repository paths a file imports, resolved to files that exist.
///
/// Only imports that resolve to a real path in the tree become edges. An
/// unresolved `import { thing } from "some-package"` is a dependency, not a
/// file relationship, and inventing a node for it would fill the graph with
/// keys nothing else ever matches.
fn imported_paths(
    repository: &Path,
    worktree: Option<&Path>,
    from: &Path,
    content: &str,
) -> Vec<PathBuf> {
    let mut targets = BTreeSet::new();
    let parent = from.parent().unwrap_or(Path::new(""));
    for line in content.lines().take(400) {
        let trimmed = line.trim();
        let candidate = if let Some(rest) = trimmed.strip_prefix("mod ") {
            // Rust: `mod foo;` → foo.rs or foo/mod.rs beside this file.
            rest.trim_end_matches(';')
                .split_whitespace()
                .next()
                .map(str::to_owned)
        } else if trimmed.starts_with("import ") || trimmed.starts_with("export ") {
            quoted_specifier(trimmed)
        } else if trimmed.starts_with("from ") && trimmed.contains(" import ") {
            // Python: `from a.b import c`.
            trimmed
                .trim_start_matches("from ")
                .split(" import ")
                .next()
                .map(|module| module.trim().replace('.', "/"))
        } else {
            None
        };
        let Some(candidate) = candidate else {
            continue;
        };
        let candidate = candidate.trim().trim_matches(['"', '\'']).to_owned();
        if candidate.is_empty() || candidate.starts_with('@') {
            continue;
        }
        let base = if candidate.starts_with('.') {
            parent.join(candidate.trim_start_matches("./"))
        } else {
            parent.join(&candidate)
        };
        for extension in ["rs", "ts", "tsx", "js", "jsx", "py", "go", "swift", "kt"] {
            for shape in [
                base.with_extension(extension),
                base.join(format!("mod.{extension}")),
                base.join(format!("index.{extension}")),
                base.join(format!("__init__.{extension}")),
            ] {
                let Some(normalized) = normalize(&shape) else {
                    continue;
                };
                if exists_in_tree(repository, worktree, &normalized) {
                    targets.insert(normalized);
                }
            }
        }
    }
    targets.into_iter().collect()
}

/// The module specifier out of an ES import/export line.
fn quoted_specifier(line: &str) -> Option<String> {
    let start = line.find(['"', '\''])?;
    let quote = line.as_bytes()[start] as char;
    let rest = &line[start + 1..];
    let end = rest.find(quote)?;
    Some(rest[..end].to_owned())
}

/// Collapse `.` components and reject anything that escapes the repository.
fn normalize(path: &Path) -> Option<PathBuf> {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::Normal(part) => normalized.push(part),
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                if !normalized.pop() {
                    return None;
                }
            }
            _ => return None,
        }
    }
    if normalized.as_os_str().is_empty() {
        None
    } else {
        Some(normalized)
    }
}

fn exists_in_tree(repository: &Path, worktree: Option<&Path>, path: &Path) -> bool {
    worktree.is_some_and(|worktree| worktree.join(path).is_file())
        || repository.join(path).is_file()
}

/// Repository-relative paths a piece of text names, restricted to files that
/// exist. Used to relate durable memory to the code it is about.
fn mentioned_paths(repository: &Path, text: &str) -> Vec<PathBuf> {
    let mut paths = BTreeSet::new();
    for token in
        text.split(|character: char| !character.is_alphanumeric() && !"./_-".contains(character))
    {
        let token = token.trim_matches(|character| character == '.' || character == ',');
        if token.is_empty() || token.starts_with('/') || !token.contains('/') {
            continue;
        }
        let candidate = PathBuf::from(token);
        if candidate.extension().is_none() {
            continue;
        }
        let Some(normalized) = normalize(&candidate) else {
            continue;
        };
        if repository.join(&normalized).is_file() {
            paths.insert(normalized);
        }
    }
    paths.into_iter().take(8).collect()
}

/// Whether `content` uses `name` as a whole identifier.
///
/// A substring match would make `auth` match `authenticate`, which is exactly
/// the class of near-miss that made the previous capability lookup unusable.
fn mentions_symbol(content: &str, name: &str) -> bool {
    let bytes = content.as_bytes();
    let mut from = 0;
    while let Some(offset) = content[from..].find(name) {
        let start = from + offset;
        let end = start + name.len();
        let before_ok = start == 0 || !is_identifier_byte(bytes[start - 1]);
        let after_ok = end == bytes.len() || !is_identifier_byte(bytes[end]);
        if before_ok && after_ok {
            return true;
        }
        from = end;
    }
    false
}

fn is_identifier_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

/// Largest number of files one repository indexing pass will parse.
const MAX_INDEXED_FILES: usize = 2_000;

/// Index the repository's own structure into the graph.
///
/// Without this the graph only ever knows the files sessions happened to touch,
/// so graph-first `#symbol` would fall back to `git grep` for every symbol in a
/// repository the agent has not edited yet — which is most of them. Runs in the
/// background tier alongside the context index, bounded by file count and file
/// size, and reads the tracked file list from git so it inherits `.gitignore`
/// rather than walking `target/` and `node_modules/`.
pub(crate) fn index_repository_structure(repository: &Path, database: &Path) -> usize {
    let Ok(output) = std::process::Command::new("git")
        .args(["ls-files", "-z"])
        .current_dir(repository)
        .output()
    else {
        return 0;
    };
    if !output.status.success() {
        return 0;
    }
    let Ok(mut graph) = ProjectGraph::open(database) else {
        return 0;
    };
    let mut writer = GraphWriter {
        graph: &mut graph,
        repository: repository.to_path_buf(),
        nodes: BTreeMap::new(),
        edges: 0,
    };
    for entry in output
        .stdout
        .split(|byte| *byte == 0)
        .take(MAX_INDEXED_FILES)
    {
        if entry.is_empty() {
            continue;
        }
        let path = PathBuf::from(String::from_utf8_lossy(entry).into_owned());
        if !is_repository_relative(&path) {
            continue;
        }
        let Some(content) = read_source(None, repository, &path) else {
            continue;
        };
        let symbols = declared_symbols(&path, &content);
        let imports = imported_paths(repository, None, &path, &content);
        if symbols.is_empty() && imports.is_empty() {
            continue;
        }
        let Some(file) = writer.file(&path) else {
            continue;
        };
        for symbol in symbols {
            let key = format!("{}#{}", path.display(), symbol.name);
            let Some(symbol_id) = writer.upsert_with(
                GraphNodeKind::Symbol,
                &key,
                false,
                serde_json::json!({ "line": symbol.line }),
            ) else {
                continue;
            };
            writer.edge(
                symbol_id,
                file.clone(),
                GraphEdgeKind::DefinedIn,
                1000,
                EdgeSource::StaticAnalysis,
                &format!("declared at {}:{}", path.display(), symbol.line),
            );
        }
        for target in imports {
            let Some(target_id) = writer.file(&target) else {
                continue;
            };
            let kind = if is_test_path(&path) {
                GraphEdgeKind::Tests
            } else {
                GraphEdgeKind::Imports
            };
            writer.edge(
                file.clone(),
                target_id,
                kind,
                900,
                EdgeSource::StaticAnalysis,
                &format!("{} references {}", path.display(), target.display()),
            );
        }
    }
    writer.edges
}

/// Node/edge writer that caches ids so one path is upserted once per pass.
struct GraphWriter<'a> {
    graph: &'a mut ProjectGraph,
    repository: PathBuf,
    nodes: BTreeMap<(GraphNodeKind, String), NodeId>,
    edges: usize,
}

impl GraphWriter<'_> {
    fn file(&mut self, path: &Path) -> Option<NodeId> {
        if !is_repository_relative(path) {
            return None;
        }
        self.upsert(GraphNodeKind::File, &path.to_string_lossy(), false)
    }

    fn upsert(&mut self, kind: GraphNodeKind, key: &str, sensitive: bool) -> Option<NodeId> {
        self.upsert_with(kind, key, sensitive, serde_json::json!({}))
    }

    fn upsert_with(
        &mut self,
        kind: GraphNodeKind,
        key: &str,
        sensitive: bool,
        attributes: serde_json::Value,
    ) -> Option<NodeId> {
        if let Some(id) = self.nodes.get(&(kind, key.to_owned())) {
            return Some(id.clone());
        }
        let node = GraphNode {
            id: NodeId(0),
            project: self.repository.clone(),
            kind,
            key: key.to_owned(),
            label: key.to_owned(),
            attributes,
            sensitive,
            observed_at: Utc::now(),
        };
        let id = self.graph.upsert_node(&node).ok()?;
        self.nodes.insert((kind, key.to_owned()), id.clone());
        Some(id)
    }

    fn edge(
        &mut self,
        source: NodeId,
        target: NodeId,
        kind: GraphEdgeKind,
        confidence_millis: u16,
        edge_source: EdgeSource,
        evidence: &str,
    ) {
        let edge = GraphEdge {
            source,
            target,
            kind,
            confidence_millis,
            edge_source,
            evidence: evidence.to_owned(),
            observed_at: Utc::now(),
        };
        if self
            .graph
            .insert_edge(&self.repository.clone(), &edge)
            .is_ok()
        {
            self.edges += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_successful_evidence_produces_modified_by() {
        // The regression: the producer read `ActionProposed`, so a delete
        // PawGate refused still taught the graph that the session modified the
        // file. Only validated effects count.
        let denied = SessionEvent::ActionProposed {
            action_id: purrcode_runtime_core::ActionId::new(),
            action: purrcode_runtime_core::ProposedAction::DeleteFile(
                purrcode_runtime_core::DeleteFileAction {
                    path: PathBuf::from("src/auth.rs"),
                    expected_digest: "digest".into(),
                },
            ),
            turn_id: None,
        };
        assert!(
            validated_effects(&[denied]).is_empty(),
            "a proposal is not an effect"
        );
    }

    #[test]
    fn succeeded_evidence_contributes_its_validated_paths() {
        let evidence = purrcode_runtime_core::ExecutionEvidence {
            action_id: purrcode_runtime_core::ActionId::new(),
            session_id: SessionId::new(),
            turn_id: None,
            tool_id: purrcode_runtime_core::ToolId::native("write_file"),
            provider: purrcode_runtime_core::ToolProvider::Native,
            descriptor_digest: "d".into(),
            decision: purrcode_runtime_core::JudgmentDecision::AllowWithConstraints(
                purrcode_runtime_core::ActionConstraints::read_only(PathBuf::from("/repo")),
            ),
            approved_by: purrcode_runtime_core::ApprovalAuthority::Human,
            constraints: purrcode_runtime_core::ActionConstraints::read_only(PathBuf::from(
                "/repo",
            )),
            effective_network_scope: purrcode_runtime_core::NetworkScope::None,
            effective_filesystem_scope: purrcode_runtime_core::FilesystemScope::WorktreeRead,
            initiator: purrcode_runtime_core::EvidenceInitiator::Human,
            outcome: ExecutionOutcome::Succeeded {
                exit_code: Some(0),
                truncated: false,
                affected_paths: vec![PathBuf::from("src/auth.rs"), PathBuf::from("/etc/passwd")],
            },
            structured_output: None,
            redaction_class: purrcode_runtime_core::RedactionClass::Public,
            started_at: Utc::now(),
            finished_at: Utc::now(),
        };
        let paths = validated_effects(&[SessionEvent::ToolEvidenceRecorded {
            evidence: Box::new(evidence),
        }]);
        assert_eq!(
            paths.into_iter().collect::<Vec<_>>(),
            vec![PathBuf::from("src/auth.rs")],
            "absolute paths are not graph keys"
        );
    }

    #[test]
    fn failed_evidence_does_not_produce_edges() {
        let mut evidence = sample_evidence();
        evidence.outcome = ExecutionOutcome::Failed {
            reason: "the tool exited with status 1".into(),
            exit_code: Some(1),
        };
        assert!(
            validated_effects(&[SessionEvent::ToolEvidenceRecorded {
                evidence: Box::new(evidence)
            }])
            .is_empty()
        );
    }

    fn sample_evidence() -> purrcode_runtime_core::ExecutionEvidence {
        purrcode_runtime_core::ExecutionEvidence {
            action_id: purrcode_runtime_core::ActionId::new(),
            session_id: SessionId::new(),
            turn_id: None,
            tool_id: purrcode_runtime_core::ToolId::native("write_file"),
            provider: purrcode_runtime_core::ToolProvider::Native,
            descriptor_digest: "d".into(),
            decision: purrcode_runtime_core::JudgmentDecision::AllowWithConstraints(
                purrcode_runtime_core::ActionConstraints::read_only(PathBuf::from("/repo")),
            ),
            approved_by: purrcode_runtime_core::ApprovalAuthority::Human,
            constraints: purrcode_runtime_core::ActionConstraints::read_only(PathBuf::from(
                "/repo",
            )),
            effective_network_scope: purrcode_runtime_core::NetworkScope::None,
            effective_filesystem_scope: purrcode_runtime_core::FilesystemScope::WorktreeRead,
            initiator: purrcode_runtime_core::EvidenceInitiator::Human,
            outcome: ExecutionOutcome::Succeeded {
                exit_code: Some(0),
                truncated: false,
                affected_paths: vec![PathBuf::from("src/auth.rs")],
            },
            structured_output: None,
            redaction_class: purrcode_runtime_core::RedactionClass::Public,
            started_at: Utc::now(),
            finished_at: Utc::now(),
        }
    }

    #[test]
    fn rust_declarations_are_found_and_call_sites_are_not() {
        let source = "use std::fmt;\n\
                      pub struct AuthMiddleware {\n    inner: u8,\n}\n\
                      pub async fn authenticate(token: &str) -> bool { true }\n\
                      fn helper() { let _ = authenticate(\"x\"); }\n";
        let symbols = declared_symbols(Path::new("src/auth.rs"), source);
        let names: Vec<&str> = symbols.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["AuthMiddleware", "authenticate", "helper"]);
        assert_eq!(symbols[0].line, 2, "the recorded line is the declaration");
    }

    #[test]
    fn python_and_typescript_declarations_are_found() {
        let python = declared_symbols(Path::new("app/auth.py"), "def login(user):\n    pass\n");
        assert_eq!(python[0].name, "login");
        let ts = declared_symbols(
            Path::new("src/auth.ts"),
            "export class Session {}\nexport default function boot() {}\n",
        );
        let names: Vec<&str> = ts.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["Session", "boot"]);
    }

    #[test]
    fn imports_only_resolve_to_files_that_exist() {
        let repository = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(repository.path().join("src")).unwrap();
        std::fs::write(repository.path().join("src/auth.rs"), "").unwrap();
        let resolved = imported_paths(
            repository.path(),
            None,
            Path::new("src/lib.rs"),
            "mod auth;\nmod missing;\n",
        );
        assert_eq!(resolved, vec![PathBuf::from("src/auth.rs")]);
    }

    #[test]
    fn symbol_mentions_are_whole_identifiers_only() {
        assert!(mentions_symbol(
            "let x = AuthMiddleware::new();",
            "AuthMiddleware"
        ));
        assert!(mentions_symbol("AuthMiddleware", "AuthMiddleware"));
        assert!(
            !mentions_symbol("NotAuthMiddlewareHere", "AuthMiddleware"),
            "a substring match relates files that have nothing to do with each other"
        );
        assert!(!mentions_symbol("auth_middleware_x", "auth_middleware"));
    }

    #[test]
    fn test_paths_are_recognised_across_conventions() {
        for path in [
            "tests/auth.rs",
            "crates/x/tests/auth.rs",
            "src/__tests__/auth.ts",
            "app/test_auth.py",
            "src/auth_test.go",
            "src/auth.test.ts",
            "src/auth.spec.ts",
        ] {
            assert!(
                is_test_path(Path::new(path)),
                "{path} should read as a test"
            );
        }
        assert!(!is_test_path(Path::new("src/latest.rs")));
    }
}
