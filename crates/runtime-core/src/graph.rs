//! Project-graph vocabulary shared with `purrcode-project-graph` (v1.3 §4.5).
//!
//! The graph's node/edge *structs* live in the `project-graph` crate; the
//! enums live here because runtime-core cannot depend on project-graph, and
//! [`crate::WhyIncluded::RelatedByGraph`] carries a [`GraphEdgeKind`]. The
//! `project-graph` crate reuses these types (PR7).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(
    Clone, Copy, Debug, Eq, Hash, JsonSchema, Ord, PartialEq, PartialOrd, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum GraphNodeKind {
    File,
    Symbol,
    Module,
    Test,
    Dependency,
    Task,
    Decision,
    Failure,
    Memory,
    Session,
}

#[derive(
    Clone, Copy, Debug, Eq, Hash, JsonSchema, Ord, PartialEq, PartialOrd, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum GraphEdgeKind {
    /// symbol -> file. tree-sitter, high confidence.
    DefinedIn,
    /// file -> file. Requires import resolution (the one piece of genuinely
    /// new work). LSP-assisted where available, never LSP-dependent.
    Imports,
    /// symbol -> symbol. From LSP textDocument/references.
    References,
    /// test -> file.
    Tests,
    /// file <-> file. From git log co-change. Symmetric, weighted.
    CoChanged,
    /// session -> file. From RepositoryEngine::changes and WriteFile actions.
    ModifiedBy,
    /// file -> failure. From ValidationRecorded projection.
    FailedWith,
    /// memory -> file|module. From project_memory usage.
    RelatesTo,
    /// task -> decision.
    Decided,
}
