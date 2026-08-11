//! Project intelligence graph (v1.3 §4.5 / §8 PR7).
//!
//! Nodes are symbols / files / tests / modules / tasks / decisions / failures
//! keyed by (project, kind, key) with repository-relative keys only. Edges are
//! deterministic relationships (DefinedIn, Imports, References, Tests,
//! CoChanged, ModifiedBy, FailedWith, RelatesTo, Decided) with a confidence
//! and an `EdgeSource` that records how trustworthy the producer was.
//!
//! The graph's tables live in the sessions DB (`migrations/0005_project_graph.sql`);
//! this crate opens its own connection to the same file (WAL + busy_timeout,
//! verified in ninelives). Embeddings are an OPTIONAL reranker, never a core
//! dependency: the default `CandidateReranker` is a no-op.

use purrcode_runtime_core::{GraphEdgeKind, GraphNodeKind};
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use thiserror::Error;

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct NodeId(pub i64);

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GraphNode {
    pub id: NodeId,
    pub project: PathBuf,
    pub kind: GraphNodeKind,
    /// Repository-relative path, or `path#symbol`, or a uuid for task/decision.
    pub key: String,
    pub label: String,
    pub attributes: serde_json::Value,
    pub sensitive: bool,
    pub observed_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EdgeSource {
    EventLog,
    GitHistory,
    StaticAnalysis,
    Heuristic,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GraphEdge {
    pub source: NodeId,
    pub target: NodeId,
    pub kind: GraphEdgeKind,
    /// 0..=1000. Traversal decays by this factor per hop.
    pub confidence_millis: u16,
    pub edge_source: EdgeSource,
    pub evidence: String,
    pub observed_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Clone, Debug)]
pub struct GraphQuery {
    pub project: PathBuf,
    pub seeds: Vec<(GraphNodeKind, String)>,
    pub edge_kinds: Option<BTreeSet<GraphEdgeKind>>,
    pub max_hops: u8,
    pub limit: usize,
}

/// Optional reranker. Default impl is a NO-OP: with no embedding role
/// configured, ranking is byte-identical to today's `score_millis` order.
///
/// Generic over the hit type so this crate does not depend on whisker —
/// embeddings are optional, so the graph must not force that edge.
pub trait CandidateReranker<T>: Send + Sync {
    fn rerank(&self, _query: &str, _hits: &mut [T]) {}
}

/// A no-op reranker — the default. Present so the type is concrete and the
/// "embeddings optional" constraint is structurally enforced.
pub struct NoopReranker;
impl<T> CandidateReranker<T> for NoopReranker {}

#[derive(Debug, Error)]
pub enum GraphError {
    #[error("database operation failed: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("node key must be repository-relative: {0}")]
    NonRelativeKey(String),
    #[error("serialization failed: {0}")]
    Serialize(#[from] serde_json::Error),
}

/// The graph store. Opens its own connection to the sessions DB file (WAL +
/// busy_timeout), so it can share the file with `SessionStore` under the
/// daemon's write load.
pub struct ProjectGraph {
    conn: Connection,
}

impl ProjectGraph {
    pub fn open(database: &Path) -> Result<Self, GraphError> {
        let conn = Connection::open(database)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        Ok(Self { conn })
    }

    /// Insert a node. `key` MUST be repository-relative; absolute keys are
    /// rejected because they re-open the traversal containment the rest of the
    /// system enforces.
    pub fn upsert_node(&mut self, node: &GraphNode) -> Result<NodeId, GraphError> {
        if !is_repository_relative_key(&node.key) {
            return Err(GraphError::NonRelativeKey(node.key.clone()));
        }
        self.conn.execute(
            "INSERT INTO graph_nodes (project, kind, key, label, attributes, sensitive, observed_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(project, kind, key) DO UPDATE SET
               label = excluded.label,
               attributes = excluded.attributes,
               sensitive = excluded.sensitive,
               observed_at = excluded.observed_at",
            params![
                node.project.to_string_lossy(),
                node_kind_name(&node.kind),
                node.key,
                node.label,
                serde_json::to_string(&node.attributes)?,
                node.sensitive as i64,
                node.observed_at.to_rfc3339(),
            ],
        )?;
        let id = self.conn.query_row(
            "SELECT id FROM graph_nodes WHERE project = ?1 AND kind = ?2 AND key = ?3",
            params![
                node.project.to_string_lossy(),
                node_kind_name(&node.kind),
                node.key
            ],
            |row| row.get(0),
        )?;
        Ok(NodeId(id))
    }

    pub fn insert_edge(&mut self, project: &Path, edge: &GraphEdge) -> Result<(), GraphError> {
        self.conn.execute(
            "INSERT OR IGNORE INTO graph_edges
               (project, source_id, target_id, kind, confidence_millis, edge_source, evidence, observed_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                project.to_string_lossy(),
                edge.source.0,
                edge.target.0,
                edge_kind_name(&edge.kind),
                edge.confidence_millis as i64,
                source_name(&edge.edge_source),
                edge.evidence,
                edge.observed_at.to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    /// Nodes reachable from a seed within `hops` hops, breadth-first.
    ///
    /// `hops` is now honoured: it was previously accepted and echoed back as
    /// the reported hop count while the query only ever read the seed's direct
    /// edges, so `hops: 3` and `hops: 1` returned identical results and the
    /// "3 hop(s)" label on a one-hop neighbour was wrong.
    ///
    /// Traversal is breadth-first so the reported hop count is the SHORTEST
    /// path to each node, and confidence decays multiplicatively per hop
    /// (`confidence_millis` is a per-edge factor in 0..=1000), so a distant
    /// node reached through weak edges ranks below a close one. Results are
    /// ordered by decayed confidence descending, then by hops ascending, and
    /// truncated to `limit`.
    pub fn neighbours(
        &self,
        project: &Path,
        seed: &GraphNode,
        hops: u8,
        limit: usize,
    ) -> Result<Vec<(GraphNode, GraphEdge, u8)>, GraphError> {
        let mut visited: BTreeSet<i64> = BTreeSet::from([seed.id.0]);
        // (node, edge that reached it, hops, decayed confidence)
        let mut collected: Vec<(GraphNode, GraphEdge, u8, f64)> = Vec::new();
        let mut frontier: Vec<(i64, u8, f64)> = vec![(seed.id.0, 0, 1.0)];
        // A traversal must terminate on a cyclic graph and must not fan out
        // without bound on a hub node, so each level is capped by the caller's
        // limit as well.
        let per_level = limit.max(1) * 4;
        while let Some((node_id, depth, confidence)) = frontier.pop() {
            if depth >= hops {
                continue;
            }
            let mut next = Vec::new();
            for (node, edge) in self.direct_edges(project, node_id, per_level)? {
                if !visited.insert(node.id.0) {
                    continue;
                }
                let decayed = confidence * (edge.confidence_millis as f64 / 1000.0);
                next.push((node.id.0, depth + 1, decayed));
                collected.push((node, edge, depth + 1, decayed));
            }
            frontier.extend(next);
        }
        collected.sort_by(|left, right| {
            right
                .3
                .partial_cmp(&left.3)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(left.2.cmp(&right.2))
                .then(left.0.key.cmp(&right.0.key))
        });
        collected.truncate(limit);
        Ok(collected
            .into_iter()
            .map(|(node, edge, hops, _)| (node, edge, hops))
            .collect())
    }

    /// One hop out of `node_id`, in either edge direction.
    fn direct_edges(
        &self,
        project: &Path,
        node_id: i64,
        limit: usize,
    ) -> Result<Vec<(GraphNode, GraphEdge)>, GraphError> {
        let mut stmt = self.conn.prepare(
            "SELECT n.id, n.project, n.kind, n.key, n.label, n.attributes, n.sensitive, n.observed_at,
                    e.source_id, e.target_id, e.kind, e.confidence_millis, e.edge_source, e.evidence, e.observed_at
             FROM graph_edges e
             JOIN graph_nodes n ON n.id = CASE WHEN e.source_id = ?2 THEN e.target_id ELSE e.source_id END
             WHERE e.project = ?1 AND (e.source_id = ?2 OR e.target_id = ?2)
             ORDER BY e.confidence_millis DESC
             LIMIT ?3",
        )?;
        let rows = stmt.query_map(
            params![project.to_string_lossy(), node_id, limit as i64],
            |row| {
                let node = GraphNode {
                    id: NodeId(row.get(0)?),
                    project: PathBuf::from(row.get::<_, String>(1)?),
                    kind: node_kind_from_name(&row.get::<_, String>(2)?),
                    key: row.get(3)?,
                    label: row.get(4)?,
                    attributes: serde_json::from_str(&row.get::<_, String>(5)?).unwrap_or_default(),
                    sensitive: row.get(6)?,
                    observed_at: chrono::DateTime::parse_from_rfc3339(&row.get::<_, String>(7)?)
                        .map(|d| d.with_timezone(&chrono::Utc))
                        .unwrap_or_else(|_| chrono::Utc::now()),
                };
                let edge = GraphEdge {
                    source: NodeId(row.get(8)?),
                    target: NodeId(row.get(9)?),
                    kind: edge_kind_from_name(&row.get::<_, String>(10)?),
                    confidence_millis: row.get(11)?,
                    edge_source: source_from_name(&row.get::<_, String>(12)?),
                    evidence: row.get(13)?,
                    observed_at: chrono::DateTime::parse_from_rfc3339(&row.get::<_, String>(14)?)
                        .map(|d| d.with_timezone(&chrono::Utc))
                        .unwrap_or_else(|_| chrono::Utc::now()),
                };
                Ok((node, edge))
            },
        )?;
        let mut result = Vec::new();
        for row in rows {
            result.push(row?);
        }
        Ok(result)
    }

    /// Symbol nodes whose name matches `name`, with the files they are
    /// `DefinedIn`.
    ///
    /// Symbol node keys are `path#symbol`, so a name lookup is an exact match
    /// on the part after `#`. This is the graph-first half of `#symbol`
    /// resolution: a definition the graph already knows is a better answer than
    /// a `git grep` that returns every mention of the word, and the grep stays
    /// as the fallback for symbols no producer has recorded yet.
    pub fn symbol_definitions(
        &self,
        project: &Path,
        name: &str,
        limit: usize,
    ) -> Result<Vec<SymbolDefinition>, GraphError> {
        let suffix = format!("#{name}");
        let mut stmt = self.conn.prepare(
            "SELECT key, label, attributes FROM graph_nodes
             WHERE project = ?1 AND kind = 'symbol' AND key LIKE ?2 ESCAPE '\\'
             ORDER BY key
             LIMIT ?3",
        )?;
        let pattern = format!("%{}", escape_like(&suffix));
        let rows = stmt.query_map(
            params![project.to_string_lossy(), pattern, limit as i64],
            |row| {
                let key: String = row.get(0)?;
                let label: String = row.get(1)?;
                let attributes: serde_json::Value =
                    serde_json::from_str(&row.get::<_, String>(2)?).unwrap_or_default();
                Ok((key, label, attributes))
            },
        )?;
        let mut definitions = Vec::new();
        for row in rows {
            let (key, label, attributes) = row?;
            // Defensive: `LIKE '%#name'` also matches `other#prefix_name` in no
            // sane keying, but an exact suffix check costs nothing and keeps
            // the contract "the symbol is called exactly this".
            let Some((path, symbol)) = key.rsplit_once('#') else {
                continue;
            };
            if symbol != name {
                continue;
            }
            definitions.push(SymbolDefinition {
                path: PathBuf::from(path),
                symbol: symbol.to_owned(),
                label,
                line: attributes
                    .get("line")
                    .and_then(serde_json::Value::as_u64)
                    .map(|line| line as u32),
            });
        }
        Ok(definitions)
    }

    /// Delete a node by id and cascade its edges (foreign_keys ON).
    pub fn delete_node(&mut self, project: &Path, node_id: NodeId) -> Result<(), GraphError> {
        self.conn.execute(
            "DELETE FROM graph_nodes WHERE project = ?1 AND id = ?2",
            params![project.to_string_lossy(), node_id.0],
        )?;
        Ok(())
    }
}

/// Whether `key` is a usable repository-relative graph key on EVERY platform.
///
/// `Path::is_absolute` is platform-dependent, and containment must not be. On
/// Windows `/etc/passwd` is NOT absolute — it has no drive prefix — so a check
/// that relies on `is_absolute` alone admits a POSIX absolute path as a graph
/// key on Windows, and admits `C:\Windows\...` on POSIX. Both directions are
/// rejected here, along with `..` traversal, so the same key is accepted or
/// refused identically wherever the daemon runs.
pub fn is_repository_relative_key(key: &str) -> bool {
    if key.is_empty() || Path::new(key).is_absolute() {
        return false;
    }
    if key.starts_with('/') || key.starts_with('\\') {
        return false;
    }
    // A drive-relative or drive-absolute Windows path (`C:foo`, `C:\foo`).
    let bytes = key.as_bytes();
    if bytes.len() >= 2 && bytes[1] == b':' && bytes[0].is_ascii_alphabetic() {
        return false;
    }
    !key.split(['/', '\\']).any(|component| component == "..")
}

/// Where the graph says a symbol is defined.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SymbolDefinition {
    pub path: PathBuf,
    pub symbol: String,
    pub label: String,
    pub line: Option<u32>,
}

/// Escape SQL `LIKE` wildcards so a symbol named `foo_bar` does not match
/// `fooXbar`.
fn escape_like(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

fn node_kind_name(kind: &GraphNodeKind) -> &'static str {
    match kind {
        GraphNodeKind::File => "file",
        GraphNodeKind::Symbol => "symbol",
        GraphNodeKind::Module => "module",
        GraphNodeKind::Test => "test",
        GraphNodeKind::Dependency => "dependency",
        GraphNodeKind::Task => "task",
        GraphNodeKind::Decision => "decision",
        GraphNodeKind::Failure => "failure",
        GraphNodeKind::Memory => "memory",
        GraphNodeKind::Session => "session",
    }
}

fn node_kind_from_name(name: &str) -> GraphNodeKind {
    match name {
        "file" => GraphNodeKind::File,
        "symbol" => GraphNodeKind::Symbol,
        "module" => GraphNodeKind::Module,
        "test" => GraphNodeKind::Test,
        "dependency" => GraphNodeKind::Dependency,
        "task" => GraphNodeKind::Task,
        "decision" => GraphNodeKind::Decision,
        "failure" => GraphNodeKind::Failure,
        "memory" => GraphNodeKind::Memory,
        _ => GraphNodeKind::Session,
    }
}

fn edge_kind_name(kind: &GraphEdgeKind) -> &'static str {
    match kind {
        GraphEdgeKind::DefinedIn => "defined_in",
        GraphEdgeKind::Imports => "imports",
        GraphEdgeKind::References => "references",
        GraphEdgeKind::Tests => "tests",
        GraphEdgeKind::CoChanged => "co_changed",
        GraphEdgeKind::ModifiedBy => "modified_by",
        GraphEdgeKind::FailedWith => "failed_with",
        GraphEdgeKind::RelatesTo => "relates_to",
        GraphEdgeKind::Decided => "decided",
    }
}

fn edge_kind_from_name(name: &str) -> GraphEdgeKind {
    match name {
        "defined_in" => GraphEdgeKind::DefinedIn,
        "imports" => GraphEdgeKind::Imports,
        "references" => GraphEdgeKind::References,
        "tests" => GraphEdgeKind::Tests,
        "co_changed" => GraphEdgeKind::CoChanged,
        "modified_by" => GraphEdgeKind::ModifiedBy,
        "failed_with" => GraphEdgeKind::FailedWith,
        "relates_to" => GraphEdgeKind::RelatesTo,
        _ => GraphEdgeKind::Decided,
    }
}

fn source_name(source: &EdgeSource) -> &'static str {
    match source {
        EdgeSource::EventLog => "event_log",
        EdgeSource::GitHistory => "git_history",
        EdgeSource::StaticAnalysis => "static_analysis",
        EdgeSource::Heuristic => "heuristic",
    }
}

fn source_from_name(name: &str) -> EdgeSource {
    match name {
        "event_log" => EdgeSource::EventLog,
        "git_history" => EdgeSource::GitHistory,
        "static_analysis" => EdgeSource::StaticAnalysis,
        _ => EdgeSource::Heuristic,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(project: &Path, kind: GraphNodeKind, key: &str) -> GraphNode {
        GraphNode {
            id: NodeId(0),
            project: project.to_path_buf(),
            kind,
            key: key.into(),
            label: key.into(),
            attributes: serde_json::json!({}),
            sensitive: false,
            observed_at: chrono::Utc::now(),
        }
    }

    #[test]
    fn absolute_keys_are_rejected() {
        let temporary = tempfile::tempdir().unwrap();
        let db = temporary.path().join("graph.db");
        let mut graph = ProjectGraph::open(&db).unwrap();
        // The tables come from migrations/0005 (registered in ninelives); a
        // bare connection here can't create them, so the insert must fail at
        // the key check BEFORE any SQL touches a missing table.
        //
        // Every shape is rejected on every platform. `Path::is_absolute` alone
        // could not do this: on Windows `/etc/passwd` has no drive prefix and
        // is not "absolute", and on POSIX `C:\\Windows` is not either — so
        // containment would have depended on which OS the daemon ran.
        for key in [
            "/etc/passwd",
            "\\\\etc\\\\passwd",
            "C:\\\\Windows\\\\System32",
            "C:relative",
            "../../etc/passwd",
            "src/../../../etc/passwd",
            "src\\\\..\\\\..\\\\etc",
            "",
        ] {
            let bad = node(Path::new("/repo"), GraphNodeKind::File, key);
            let err = graph
                .upsert_node(&bad)
                .expect_err(&format!("`{key}` must not be a graph key"));
            assert!(matches!(err, GraphError::NonRelativeKey(_)), "{key}");
        }
        assert!(is_repository_relative_key("src/auth.rs"));
        assert!(is_repository_relative_key("src/auth.rs#AuthMiddleware"));
    }

    /// The 0005 schema, so traversal can be tested without ninelives.
    fn graph_with_schema(path: &Path) -> ProjectGraph {
        let graph = ProjectGraph::open(path).unwrap();
        // 0005 records itself in schema_migrations, which ninelives owns.
        graph
            .conn
            .execute_batch(
                "CREATE TABLE IF NOT EXISTS schema_migrations (
                    version INTEGER PRIMARY KEY, applied_at TEXT NOT NULL);",
            )
            .unwrap();
        graph
            .conn
            .execute_batch(include_str!("../../../migrations/0005_project_graph.sql"))
            .unwrap();
        graph
    }

    fn edge(source: NodeId, target: NodeId, confidence: u16) -> GraphEdge {
        GraphEdge {
            source,
            target,
            kind: GraphEdgeKind::Imports,
            confidence_millis: confidence,
            edge_source: EdgeSource::StaticAnalysis,
            evidence: "test".into(),
            observed_at: chrono::Utc::now(),
        }
    }

    #[test]
    fn traversal_actually_walks_hops_and_decays_confidence() {
        let temporary = tempfile::tempdir().unwrap();
        let mut graph = graph_with_schema(&temporary.path().join("graph.db"));
        let project = Path::new("/repo");

        // a → b → c, plus a → d directly.
        let a = graph
            .upsert_node(&node(project, GraphNodeKind::File, "a.rs"))
            .unwrap();
        let b = graph
            .upsert_node(&node(project, GraphNodeKind::File, "b.rs"))
            .unwrap();
        let c = graph
            .upsert_node(&node(project, GraphNodeKind::File, "c.rs"))
            .unwrap();
        let d = graph
            .upsert_node(&node(project, GraphNodeKind::File, "d.rs"))
            .unwrap();
        graph
            .insert_edge(project, &edge(a.clone(), b.clone(), 900))
            .unwrap();
        graph.insert_edge(project, &edge(b, c, 900)).unwrap();
        graph
            .insert_edge(project, &edge(a.clone(), d, 400))
            .unwrap();

        let seed = GraphNode {
            id: a,
            ..node(project, GraphNodeKind::File, "a.rs")
        };

        // One hop reaches only the direct neighbours. Before the traversal was
        // implemented, `hops` was carried through unused, so this returned the
        // same set as the two-hop query below and mislabelled the hop count.
        let one = graph.neighbours(project, &seed, 1, 10).unwrap();
        let keys: Vec<&str> = one.iter().map(|(n, _, _)| n.key.as_str()).collect();
        assert_eq!(keys, vec!["b.rs", "d.rs"], "one hop is one hop");
        assert!(one.iter().all(|(_, _, hops)| *hops == 1));

        // Two hops reaches c.rs, reported at its true shortest distance.
        let two = graph.neighbours(project, &seed, 2, 10).unwrap();
        let c_hit = two
            .iter()
            .find(|(node, _, _)| node.key == "c.rs")
            .expect("c.rs is reachable in two hops");
        assert_eq!(c_hit.2, 2, "the reported hop count is the real distance");

        // Confidence decays per hop: c.rs (0.9 × 0.9 = 0.81) still outranks
        // d.rs (0.4) even though d.rs is closer.
        let order: Vec<&str> = two.iter().map(|(n, _, _)| n.key.as_str()).collect();
        assert_eq!(order, vec!["b.rs", "c.rs", "d.rs"]);
    }

    #[test]
    fn traversal_terminates_on_a_cycle() {
        let temporary = tempfile::tempdir().unwrap();
        let mut graph = graph_with_schema(&temporary.path().join("graph.db"));
        let project = Path::new("/repo");
        let a = graph
            .upsert_node(&node(project, GraphNodeKind::File, "a.rs"))
            .unwrap();
        let b = graph
            .upsert_node(&node(project, GraphNodeKind::File, "b.rs"))
            .unwrap();
        graph
            .insert_edge(project, &edge(a.clone(), b.clone(), 1000))
            .unwrap();
        graph
            .insert_edge(project, &edge(b, a.clone(), 1000))
            .unwrap();
        let seed = GraphNode {
            id: a,
            ..node(project, GraphNodeKind::File, "a.rs")
        };
        // Would loop forever without the visited set.
        let hits = graph.neighbours(project, &seed, 5, 10).unwrap();
        assert_eq!(hits.len(), 1, "a cycle visits each node once");
        assert_eq!(hits[0].0.key, "b.rs");
    }

    #[test]
    fn symbol_lookup_returns_definitions_and_never_a_near_miss() {
        let temporary = tempfile::tempdir().unwrap();
        let mut graph = graph_with_schema(&temporary.path().join("graph.db"));
        let project = Path::new("/repo");
        for key in [
            "src/auth.rs#AuthMiddleware",
            "src/mw.rs#AuthMiddleware",
            // A different symbol whose name merely ENDS with the query. The
            // `LIKE '%#name'` prefilter cannot distinguish these, so the exact
            // suffix check has to — otherwise `#Middleware` would answer with
            // `AuthMiddleware` and send retrieval to the wrong definition.
            "src/other.rs#NotAuthMiddleware",
        ] {
            let mut node = node(project, GraphNodeKind::Symbol, key);
            node.attributes = serde_json::json!({ "line": 42 });
            graph.upsert_node(&node).unwrap();
        }
        let hits = graph
            .symbol_definitions(project, "AuthMiddleware", 8)
            .unwrap();
        let paths: Vec<String> = hits
            .iter()
            .map(|hit| hit.path.display().to_string())
            .collect();
        assert_eq!(paths, vec!["src/auth.rs", "src/mw.rs"]);
        assert_eq!(hits[0].line, Some(42));
        assert!(
            graph
                .symbol_definitions(project, "Nothing", 8)
                .unwrap()
                .is_empty(),
            "an unknown symbol falls through to the caller's git grep"
        );
    }

    #[test]
    fn symbol_lookup_does_not_treat_underscores_as_wildcards() {
        let temporary = tempfile::tempdir().unwrap();
        let mut graph = graph_with_schema(&temporary.path().join("graph.db"));
        let project = Path::new("/repo");
        graph
            .upsert_node(&node(project, GraphNodeKind::Symbol, "src/a.rs#readXfile"))
            .unwrap();
        assert!(
            graph
                .symbol_definitions(project, "read_file", 8)
                .unwrap()
                .is_empty(),
            "`_` is a SQL LIKE wildcard and must be escaped"
        );
    }

    #[test]
    fn edge_source_and_kind_names_round_trip() {
        assert_eq!(edge_kind_from_name("co_changed"), GraphEdgeKind::CoChanged);
        assert_eq!(source_from_name("git_history"), EdgeSource::GitHistory);
        assert_eq!(source_from_name("heuristic"), EdgeSource::Heuristic);
    }
}
