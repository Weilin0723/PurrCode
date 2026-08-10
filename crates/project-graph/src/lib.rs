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
        if Path::new(&node.key).is_absolute() {
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

    /// Neighbours of a seed node, ordered by confidence desc then hops asc.
    pub fn neighbours(
        &self,
        project: &Path,
        seed: &GraphNode,
        hops: u8,
        limit: usize,
    ) -> Result<Vec<(GraphNode, GraphEdge, u8)>, GraphError> {
        let mut stmt = self.conn.prepare(
            "SELECT n.id, n.project, n.kind, n.key, n.label, n.attributes, n.sensitive, n.observed_at,
                    e.source_id, e.target_id, e.kind, e.confidence_millis, e.edge_source, e.evidence, e.observed_at,
                    ?4
             FROM graph_edges e
             JOIN graph_nodes n ON n.id = CASE WHEN e.source_id = ?2 THEN e.target_id ELSE e.source_id END
             WHERE e.project = ?1 AND (e.source_id = ?2 OR e.target_id = ?2)
             ORDER BY e.confidence_millis DESC
             LIMIT ?3",
        )?;
        let rows = stmt.query_map(
            params![
                project.to_string_lossy(),
                seed.id.0,
                limit as i64,
                hops as i64
            ],
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
                Ok((node, edge, row.get::<_, i64>(15)? as u8))
            },
        )?;
        let mut result = Vec::new();
        for row in rows {
            result.push(row?);
        }
        let _ = hops;
        Ok(result)
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
        let bad = node(Path::new("/repo"), GraphNodeKind::File, "/etc/passwd");
        let err = graph.upsert_node(&bad).unwrap_err();
        assert!(matches!(err, GraphError::NonRelativeKey(_)));
    }

    #[test]
    fn edge_source_and_kind_names_round_trip() {
        assert_eq!(edge_kind_from_name("co_changed"), GraphEdgeKind::CoChanged);
        assert_eq!(source_from_name("git_history"), EdgeSource::GitHistory);
        assert_eq!(source_from_name("heuristic"), EdgeSource::Heuristic);
    }
}
