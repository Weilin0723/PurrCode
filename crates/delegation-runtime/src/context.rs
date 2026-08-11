//! Worker context and structured handoff (v1.4 §PR6).
//!
//! A worker gets what its delegated task needs and nothing else. Cloning the
//! main agent's context into every specialist is the obvious implementation and
//! the wrong one: N workers each carrying the parent's full history is how a
//! multi-agent run costs five times a single-agent run and still does worse,
//! because each worker's attention is spread across material it was not asked
//! about.
//!
//! Just as important is the return direction. Only a
//! [`purrcode_runtime_core::delegation::WorkerResult`] flows back to the parent.
//! A worker's conversation stays inspectable in the UI and in the audit log, but
//! it is never poured wholesale into the parent's context.

use purrcode_runtime_core::delegation::{
    ContextRef, Delegation, DelegationOrigin, StructuredFinding, WorkerResult, WorkspaceAccess,
};
use serde::{Deserialize, Serialize};

/// The byte cap for one assembled worker context. Generous enough for a real
/// task brief, small enough that three workers cannot triple a session's input
/// cost on context alone.
pub const MAXIMUM_CONTEXT_BYTES: usize = 24_000;

/// What a dependency contributed, in the form the dependent actually needs.
///
/// Deliberately not the dependency's `WorkerResult` verbatim: the dependent
/// needs to know what changed and what is still open, not how the other worker
/// arrived there.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DependencySummary {
    pub capability: String,
    pub summary: String,
    pub changed_paths: Vec<String>,
    pub open_issues: Vec<String>,
    pub origin: DelegationOrigin,
}

impl DependencySummary {
    pub fn from_result(result: &WorkerResult, capability: &str) -> Self {
        Self {
            capability: capability.to_owned(),
            summary: result.summary.clone(),
            changed_paths: result
                .changed_paths
                .iter()
                .map(|path| path.display().to_string())
                .collect(),
            open_issues: result
                .unresolved
                .iter()
                .map(|issue| issue.summary.clone())
                .collect(),
            origin: DelegationOrigin::WorkerResult {
                delegation_id: result.delegation_id,
                worker_id: result.worker_id,
                capability: capability.to_owned(),
            },
        }
    }
}

/// A finding routed to a worker (a repair task, or a reviewer's report the main
/// agent asked someone to act on), carried with its provenance.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CarriedFinding {
    pub finding: StructuredFinding,
    pub origin: DelegationOrigin,
}

/// Everything one worker is told (v1.4 §PR6).
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct WorkerContext {
    pub objective: String,
    pub acceptance_criteria: Vec<String>,
    pub allowed_paths: Vec<String>,
    pub access: String,
    pub references: Vec<ContextRef>,
    /// A bounded summary of the parent's intent — never the parent's transcript.
    pub parent_summary: String,
    pub dependencies: Vec<DependencySummary>,
    pub findings: Vec<CarriedFinding>,
    /// Named project-memory entries judged relevant, never all of memory.
    pub memory_excerpts: Vec<String>,
    /// Graph context (files, symbols) the parent selected for this objective.
    pub graph_context: Vec<String>,
    /// Tool ids the worker's effective registry admitted, so the brief and the
    /// manifest cannot disagree.
    pub tool_manifest: Vec<String>,
    pub expected_output: String,
    pub budget_summary: String,
    /// True when assembly had to drop material to stay inside the byte cap. The
    /// worker's brief says so explicitly rather than silently ending early.
    pub truncated: bool,
}

/// Inputs the caller gathers; assembly decides what survives.
#[derive(Clone, Debug, Default)]
pub struct ContextInputs<'a> {
    pub parent_summary: &'a str,
    pub dependencies: Vec<DependencySummary>,
    pub findings: Vec<CarriedFinding>,
    pub memory_excerpts: Vec<String>,
    pub graph_context: Vec<String>,
    pub tool_manifest: Vec<String>,
}

/// Assemble one worker's context.
///
/// Ordering is by how load-bearing each section is, because truncation drops
/// from the end: objective and scope are never dropped; graph hints and memory
/// are the first to go.
pub fn assemble(delegation: &Delegation, inputs: ContextInputs<'_>) -> WorkerContext {
    let mut context = WorkerContext {
        objective: delegation.objective().to_owned(),
        acceptance_criteria: delegation
            .acceptance_criteria()
            .iter()
            .map(|criterion| criterion.statement.clone())
            .collect(),
        allowed_paths: delegation
            .allowed_paths()
            .iter()
            .map(|pattern| pattern.as_str().to_owned())
            .collect(),
        access: match delegation.access() {
            WorkspaceAccess::ReadOnly => "read-only".into(),
            WorkspaceAccess::Writable => "writable (isolated worktree)".into(),
        },
        references: delegation.context_refs().to_vec(),
        parent_summary: inputs.parent_summary.to_owned(),
        dependencies: inputs.dependencies,
        findings: inputs.findings,
        memory_excerpts: inputs.memory_excerpts,
        graph_context: inputs.graph_context,
        tool_manifest: inputs.tool_manifest,
        expected_output: format!("{:?}", delegation.expected_output()),
        budget_summary: format!(
            "{} input tokens, {} tool calls, {} changed files, {}s",
            delegation.budget().maximum_input_tokens,
            delegation.budget().maximum_tool_calls,
            delegation.budget().maximum_changed_files,
            delegation.budget().maximum_duration_seconds,
        ),
        truncated: false,
    };
    enforce_budget(&mut context);
    context
}

/// Drop the least load-bearing sections until the rendered brief fits.
fn enforce_budget(context: &mut WorkerContext) {
    // Each step removes one optional section, cheapest-to-lose first, and stops
    // as soon as the brief fits.
    let shrinkers: [fn(&mut WorkerContext) -> bool; 5] = [
        |ctx| take_if_present(&mut ctx.graph_context),
        |ctx| take_if_present(&mut ctx.memory_excerpts),
        |ctx| {
            let present = !ctx.findings.is_empty();
            ctx.findings.truncate(ctx.findings.len().saturating_sub(1));
            present
        },
        |ctx| {
            let present = !ctx.dependencies.is_empty();
            ctx.dependencies
                .truncate(ctx.dependencies.len().saturating_sub(1));
            present
        },
        |ctx| {
            let present = ctx.parent_summary.len() > 1_000;
            if present {
                ctx.parent_summary.truncate(1_000);
            }
            present
        },
    ];
    let mut shrinker = 0;
    while render(context).len() > MAXIMUM_CONTEXT_BYTES && shrinker < shrinkers.len() {
        if shrinkers[shrinker](context) {
            context.truncated = true;
        } else {
            shrinker += 1;
        }
    }
}

fn take_if_present(section: &mut Vec<String>) -> bool {
    let present = !section.is_empty();
    section.pop();
    present
}

/// Render the worker's brief as prose. This is what goes into the worker's
/// prompt; the structured form stays available for the UI and for evidence.
pub fn render(context: &WorkerContext) -> String {
    let mut out = String::new();
    out.push_str("# Delegated task\n\n");
    out.push_str(&context.objective);
    out.push_str("\n\n");

    if !context.acceptance_criteria.is_empty() {
        out.push_str("## Acceptance criteria\n");
        for criterion in &context.acceptance_criteria {
            out.push_str(&format!("- {criterion}\n"));
        }
        out.push('\n');
    }

    out.push_str("## Scope\n");
    out.push_str(&format!("Workspace access: {}\n", context.access));
    // A read-only worker's paths are what it should look at, not what it may
    // change. Labelling them "you may modify" would be an invitation to attempt
    // writes PawGate is going to deny anyway.
    let writable = context.access.starts_with("writable");
    if !writable {
        out.push_str("No paths are writable for this task; report findings instead.\n");
    }
    if !context.allowed_paths.is_empty() {
        out.push_str(if writable {
            "You may only modify:\n"
        } else {
            "Concentrate on:\n"
        });
        for path in &context.allowed_paths {
            out.push_str(&format!("- {path}\n"));
        }
    } else if writable {
        out.push_str("No paths are writable for this task.\n");
    }
    out.push_str(&format!("Budget: {}\n", context.budget_summary));
    out.push_str(&format!("Expected output: {}\n\n", context.expected_output));

    if !context.parent_summary.is_empty() {
        out.push_str("## What the main agent is doing\n");
        out.push_str(&context.parent_summary);
        out.push_str("\n\n");
    }

    if !context.references.is_empty() {
        out.push_str("## References\n");
        for reference in &context.references {
            out.push_str(&format!("- {}\n", reference.display()));
        }
        out.push('\n');
    }

    if !context.dependencies.is_empty() {
        out.push_str("## Work already completed by other specialists\n");
        for dependency in &context.dependencies {
            out.push_str(&format!(
                "- [{}] {} ({})\n",
                dependency.capability,
                dependency.summary,
                dependency.origin.why_included()
            ));
            for path in &dependency.changed_paths {
                out.push_str(&format!("    changed: {path}\n"));
            }
            for issue in &dependency.open_issues {
                out.push_str(&format!("    open: {issue}\n"));
            }
        }
        out.push('\n');
    }

    if !context.findings.is_empty() {
        out.push_str("## Findings routed to you\n");
        for carried in &context.findings {
            out.push_str(&format!(
                "- [{:?}] {} — {} ({})\n",
                carried.finding.severity,
                carried.finding.title,
                carried.finding.detail,
                carried.origin.why_included()
            ));
        }
        out.push('\n');
    }

    if !context.graph_context.is_empty() {
        out.push_str("## Relevant code\n");
        for entry in &context.graph_context {
            out.push_str(&format!("- {entry}\n"));
        }
        out.push('\n');
    }

    if !context.memory_excerpts.is_empty() {
        out.push_str("## Project memory\n");
        for entry in &context.memory_excerpts {
            out.push_str(&format!("- {entry}\n"));
        }
        out.push('\n');
    }

    if !context.tool_manifest.is_empty() {
        out.push_str("## Tools available to you\n");
        out.push_str(&context.tool_manifest.join(", "));
        out.push('\n');
    }

    if context.truncated {
        out.push_str(
            "\n(Some background material was omitted to stay inside this task's \
             context budget. Ask for a specific file if you need it.)\n",
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{admitted_delegation, worker_result};
    use purrcode_runtime_core::delegation::{ExpectedOutput, FindingSeverity, WorkerId};
    use purrcode_runtime_core::work::EvidenceId;
    use std::path::PathBuf;

    #[test]
    fn a_worker_brief_states_its_scope_and_never_the_parent_transcript() {
        let delegation = admitted_delegation(&["src/auth/**"], ExpectedOutput::Patch, &[]);
        let context = assemble(
            &delegation,
            ContextInputs {
                parent_summary: "Adding OAuth with persistent account linking.",
                tool_manifest: vec!["native:read_file".into(), "native:write_file".into()],
                ..ContextInputs::default()
            },
        );
        let brief = render(&context);
        assert!(brief.contains("src/auth/**"));
        assert!(brief.contains("writable (isolated worktree)"));
        assert!(brief.contains("Adding OAuth"));
        assert!(brief.contains("native:write_file"));
        assert!(!context.truncated);
    }

    #[test]
    fn a_dependency_contributes_a_summary_not_its_transcript() {
        let dependency = admitted_delegation(&["migrations/**"], ExpectedOutput::Patch, &[]);
        let result = worker_result(&dependency, WorkerId::new(), &["migrations/0007.sql"]);
        let summary = DependencySummary::from_result(&result, "database_migration");
        assert_eq!(summary.changed_paths, ["migrations/0007.sql"]);
        assert!(summary.origin.why_included().contains("database_migration"));

        let delegation = admitted_delegation(&["src/auth/**"], ExpectedOutput::Patch, &[]);
        let context = assemble(
            &delegation,
            ContextInputs {
                dependencies: vec![summary],
                ..ContextInputs::default()
            },
        );
        let brief = render(&context);
        assert!(brief.contains("database_migration"));
        assert!(brief.contains("migrations/0007.sql"));
    }

    #[test]
    fn a_routed_finding_carries_its_provenance_into_the_brief() {
        // §PR13: a model must never be told "security issue detected" without
        // being able to trace which worker and evidence produced it.
        let delegation = admitted_delegation(&["src/auth/**"], ExpectedOutput::Patch, &[]);
        let origin = DelegationOrigin::WorkerFinding {
            delegation_id: delegation.id(),
            worker_id: WorkerId::new(),
            finding_id: "f1".into(),
            evidence_ids: vec![EvidenceId::new()],
        };
        let context = assemble(
            &delegation,
            ContextInputs {
                findings: vec![CarriedFinding {
                    finding: StructuredFinding {
                        id: "f1".into(),
                        title: "token logged in plaintext".into(),
                        detail: "redact before logging".into(),
                        severity: FindingSeverity::Critical,
                        path: Some(PathBuf::from("src/auth/token.rs")),
                        line: Some(42),
                        evidence_ids: vec![EvidenceId::new()],
                        recommended_action: Some("redact".into()),
                    },
                    origin,
                }],
                ..ContextInputs::default()
            },
        );
        let brief = render(&context);
        assert!(brief.contains("token logged in plaintext"));
        assert!(brief.contains("evidence record"));
    }

    #[test]
    fn an_oversized_context_is_truncated_and_says_so() {
        let delegation = admitted_delegation(&["src/auth/**"], ExpectedOutput::Patch, &[]);
        let bulk: Vec<String> = (0..4_000)
            .map(|i| format!("src/generated/file_{i}.rs"))
            .collect();
        let context = assemble(
            &delegation,
            ContextInputs {
                parent_summary: &"parent history ".repeat(500),
                graph_context: bulk.clone(),
                memory_excerpts: bulk,
                ..ContextInputs::default()
            },
        );
        assert!(context.truncated);
        let brief = render(&context);
        assert!(
            brief.len() <= MAXIMUM_CONTEXT_BYTES + 512,
            "brief was {} bytes",
            brief.len()
        );
        assert!(brief.contains("omitted to stay inside"));
        // The load-bearing parts survive truncation.
        assert!(brief.contains("src/auth/**"));
        assert!(brief.contains("# Delegated task"));
    }

    #[test]
    fn a_read_only_worker_is_told_it_cannot_write() {
        let delegation = admitted_delegation(&["src/**"], ExpectedOutput::Review, &[]);
        let context = assemble(&delegation, ContextInputs::default());
        let brief = render(&context);
        assert!(brief.contains("read-only"));
        assert!(brief.contains("No paths are writable"));
    }
}
