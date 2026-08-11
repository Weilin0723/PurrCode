//! Bounded multi-agent delegation (PurrCode v1.4).
//!
//! v1.3 made PurrCode extensible. v1.4 lets those extensions cooperate without
//! giving up any of the guarantees that made them safe:
//!
//! ```text
//!   Main agent
//!       ↓ should we delegate?              decision.rs   (runtime-core)
//!   Delegation planner
//!       ↓ who can do this?                 routing.rs
//!   Capability registry
//!       ↓ what may they touch?             Delegation::admit (runtime-core)
//!   Isolated worker workspaces             workspace.rs
//!       ↓ in what order, how many at once? scheduler.rs
//!   WorkerResult
//!       ↓ is it safe to integrate?         integration.rs (runtime-core)
//!   Integration coordinator                integrate.rs
//!       ↓ PawGate
//!   Parent session worktree
//! ```
//!
//! The pure decisions — authority intersection, conflict detection, budget
//! arithmetic, the classifier — live in `purrcode-runtime-core` so they can be
//! reasoned about and replayed without a filesystem. This crate owns the parts
//! that touch git and time, and it can only express decisions those types
//! already permit.
//!
//! What this crate deliberately does **not** own: running a model. A worker's
//! execution is supplied by the caller through [`SpecialistWorker`], because the
//! daemon already owns the agent loop, PawGate, evidence and the provider
//! gateway — reimplementing any of that here would be a second, unaudited path
//! to the same capabilities.

pub mod context;
pub mod integrate;
pub mod review;
pub mod routing;
pub mod scheduler;
pub mod workspace;

#[cfg(test)]
pub(crate) mod test_support;

use purrcode_repository_engine::RepositoryError;
use purrcode_runtime_core::delegation::{
    AuthorityInputs, Delegation, DelegationBudget, DelegationError, DelegationGovernance,
    DelegationLedger, DelegationPlan, DelegationRequest, PlannedUnit, WorkerId,
};
use purrcode_runtime_core::{CapabilityRegistry, SessionId, ToolCeiling, TurnId};
use routing::{RoutedSpecialist, RoutingPolicy};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum DelegationRuntimeError {
    #[error(transparent)]
    Delegation(#[from] DelegationError),
    #[error("repository operation failed: {0}")]
    Repository(#[from] RepositoryError),
    #[error(
        "refusing to apply a patch that was not the one approved \
         (approved {approved}, actual {actual})"
    )]
    PatchDigestMismatch { approved: String, actual: String },
    #[error("there is nothing to apply")]
    EmptyPatch,
    #[error("hunk {index} does not exist in this patch")]
    UnknownHunk { index: usize },
    #[error("a binary patch cannot be integrated hunk by hunk; accept or reject the whole file")]
    BinaryHunkSelection,
    #[error("worker {worker} has no workspace on disk; it cannot be resumed")]
    WorkspaceMissing { worker: String },
}

/// One admitted delegation with the specialist that will run it.
#[derive(Clone, Debug)]
pub struct PlannedDelegation {
    pub delegation: Delegation,
    pub specialist: RoutedSpecialist,
    /// The planner's key for this unit, so dependencies can be resolved by name.
    pub key: String,
}

/// Everything the planner needs that is not in the plan itself.
#[derive(Clone, Copy, Debug)]
pub struct PlanningContext<'a> {
    pub parent_session_id: SessionId,
    pub parent_turn_id: TurnId,
    /// The workspace policy ceiling (PawGate).
    pub workspace_ceiling: &'a ToolCeiling,
    /// The ceiling the parent agent is running under this turn.
    pub parent_ceiling: &'a ToolCeiling,
    pub governance: &'a DelegationGovernance,
    pub ledger: &'a DelegationLedger,
}

/// Turn a classifier plan into admitted delegations (v1.4 §PR1 + §PR5).
///
/// Each unit is routed to a specialist, then admitted through
/// [`DelegationRequest::admit`], which is the only place authority is decided.
/// A unit whose capability nothing provides, or whose authority cannot be
/// satisfied, is reported as an error against that unit rather than failing the
/// whole plan silently — the caller decides whether a partial plan is still
/// worth running.
pub fn admit_plan(
    plan: &DelegationPlan,
    registry: &CapabilityRegistry,
    routing_policy: &RoutingPolicy,
    context: PlanningContext<'_>,
) -> (Vec<PlannedDelegation>, Vec<(String, DelegationError)>) {
    if let Err(error) = plan.validate() {
        return (Vec::new(), vec![(String::new(), error)]);
    }

    let mut admitted: Vec<PlannedDelegation> = Vec::new();
    let mut refusals: Vec<(String, DelegationError)> = Vec::new();
    let remaining = context.ledger.remaining_budget(context.governance);

    // Units are admitted in plan order so a dependency's id exists before the
    // unit that names it. `DelegationPlan::validate` already proved the graph is
    // acyclic, but not that it is topologically ordered, so unresolved
    // dependencies are reported rather than assumed.
    for unit in &plan.units {
        match admit_unit(
            unit,
            registry,
            routing_policy,
            context,
            &remaining,
            &admitted,
        ) {
            Ok(planned) => admitted.push(planned),
            Err(error) => refusals.push((unit.key.clone(), error)),
        }
    }
    (admitted, refusals)
}

fn admit_unit(
    unit: &PlannedUnit,
    registry: &CapabilityRegistry,
    routing_policy: &RoutingPolicy,
    context: PlanningContext<'_>,
    remaining: &DelegationBudget,
    already: &[PlannedDelegation],
) -> Result<PlannedDelegation, DelegationError> {
    let specialist = routing::route(registry, &unit.capability, routing_policy)?;

    let dependencies = unit
        .depends_on
        .iter()
        .map(|key| {
            already
                .iter()
                .find(|planned| &planned.key == key)
                .map(|planned| planned.delegation.id())
                .ok_or_else(|| DelegationError::InvalidDependencyGraph {
                    reason: format!(
                        "unit `{}` depends on `{key}`, which has not been admitted yet",
                        unit.key
                    ),
                })
        })
        .collect::<Result<Vec<_>, _>>()?;

    let delegation = DelegationRequest {
        parent_session_id: context.parent_session_id,
        parent_turn_id: context.parent_turn_id,
        objective: unit.objective.clone(),
        capability: unit.capability.clone(),
        acceptance_criteria: Vec::new(),
        context_refs: Vec::new(),
        allowed_paths: unit.allowed_paths.clone(),
        expected_output: unit.expected_output,
        dependencies,
        budget: unit.budget,
    }
    .admit(AuthorityInputs {
        workspace: context.workspace_ceiling,
        parent: context.parent_ceiling,
        profile: &specialist.ceiling,
        parent_remaining_budget: remaining,
        // v1.4 permits exactly one level: main → specialist.
        depth: 1,
    })?;

    Ok(PlannedDelegation {
        delegation,
        specialist,
        key: unit.key.clone(),
    })
}

/// What a caller must implement to actually run a worker.
///
/// The runtime supplies the workspace, the context and the effective authority;
/// the implementation supplies the agent loop. Keeping this a trait is what
/// stops v1.4 from growing a second, unaudited execution path next to the one
/// the daemon already governs with PawGate and evidence.
#[allow(async_fn_in_trait)]
pub trait SpecialistWorker {
    /// Run one delegation to completion and return its structured result.
    ///
    /// Implementations must respect `delegation.effective_ceiling()` and
    /// `delegation.budget()`; the runtime verifies the *result* against the
    /// delegation afterwards, but a worker that ignores its ceiling mid-run
    /// would still have taken the actions.
    async fn execute(
        &self,
        delegation: &Delegation,
        worker_id: WorkerId,
        workspace: &workspace::ProvisionedWorkspace,
        brief: &context::WorkerContext,
    ) -> Result<purrcode_runtime_core::delegation::WorkerResult, String>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use purrcode_runtime_core::delegation::{
        DelegationClassification, DelegationSignals, ExpectedOutput, PathPattern,
    };
    use purrcode_runtime_core::{
        AgentProfile, ApprovalPolicy, CapabilityId, ExtensionLayer, FilesystemScope, NetworkScope,
        PermissionRequest, SideEffectClass, ToolPolicy,
    };
    use std::collections::BTreeSet;

    fn capability(raw: &str) -> CapabilityId {
        CapabilityId::parse(raw).unwrap()
    }

    fn permissive() -> ToolCeiling {
        ToolCeiling {
            maximum_side_effect: SideEffectClass::Destructive,
            maximum_network: NetworkScope::Any,
            maximum_filesystem: FilesystemScope::maximum(),
            minimum_approval: ApprovalPolicy::ByClass,
            denied_tool_ids: BTreeSet::new(),
        }
    }

    fn registry_with(profiles: &[(&str, &str)]) -> CapabilityRegistry {
        let mut registry = CapabilityRegistry::new();
        let ceiling = permissive();
        for (name, capability_name) in profiles {
            registry.admit_agent(
                AgentProfile {
                    name: (*name).into(),
                    description: String::new(),
                    capabilities: [capability(capability_name)].into_iter().collect(),
                    model_role: None,
                    system_prompt: None,
                    tools: ToolPolicy {
                        allow: vec!["native:*".into()],
                        deny: vec![],
                    },
                    permissions: PermissionRequest::default(),
                    context: Default::default(),
                    skills: Default::default(),
                    priority: 0,
                    layer: ExtensionLayer::Project,
                },
                &ceiling,
            );
        }
        registry
    }

    fn unit(key: &str, capability_name: &str, paths: &[&str], depends: &[&str]) -> PlannedUnit {
        PlannedUnit {
            key: key.into(),
            objective: format!("do {key}"),
            capability: capability(capability_name),
            expected_output: ExpectedOutput::Patch,
            allowed_paths: paths
                .iter()
                .map(|p| PathPattern::parse(p).unwrap())
                .collect(),
            depends_on: depends.iter().map(|d| (*d).to_owned()).collect(),
            budget: DelegationBudget::modest(),
        }
    }

    fn plan(units: Vec<PlannedUnit>) -> DelegationPlan {
        DelegationPlan {
            classification: DelegationClassification::ParallelDelegation,
            signals: DelegationSignals::default(),
            expected_benefit: 12,
            coordination_cost: 7,
            reason: "test".into(),
            units,
        }
    }

    fn planning_context<'a>(
        workspace: &'a ToolCeiling,
        parent: &'a ToolCeiling,
        governance: &'a DelegationGovernance,
        ledger: &'a DelegationLedger,
    ) -> PlanningContext<'a> {
        PlanningContext {
            parent_session_id: SessionId::new(),
            parent_turn_id: TurnId::new(),
            workspace_ceiling: workspace,
            parent_ceiling: parent,
            governance,
            ledger,
        }
    }

    #[test]
    fn a_plan_admits_every_unit_with_its_routed_specialist() {
        let registry = registry_with(&[
            ("backend", "implement_backend"),
            ("migrator", "database_migration"),
        ]);
        let ceiling = permissive();
        let governance = DelegationGovernance::default();
        let ledger = DelegationLedger::default();
        let plan = plan(vec![
            unit("backend", "implement_backend", &["src/auth/**"], &[]),
            unit("migration", "database_migration", &["migrations/**"], &[]),
        ]);
        let (admitted, refusals) = admit_plan(
            &plan,
            &registry,
            &RoutingPolicy::default(),
            planning_context(&ceiling, &ceiling, &governance, &ledger),
        );
        assert!(refusals.is_empty(), "{refusals:?}");
        assert_eq!(admitted.len(), 2);
        assert_eq!(admitted[0].specialist.profile_name, "backend");
        assert_eq!(admitted[1].specialist.profile_name, "migrator");
        // Each delegation is narrowed to its own paths — the backend worker
        // cannot write migrations and vice versa.
        assert!(
            admitted[0]
                .delegation
                .permits_path(std::path::Path::new("src/auth/token.rs"))
        );
        assert!(
            !admitted[0]
                .delegation
                .permits_path(std::path::Path::new("migrations/0007.sql"))
        );
    }

    #[test]
    fn dependencies_resolve_to_admitted_ids() {
        let registry =
            registry_with(&[("backend", "implement_backend"), ("tester", "write_tests")]);
        let ceiling = permissive();
        let governance = DelegationGovernance::default();
        let ledger = DelegationLedger::default();
        let plan = plan(vec![
            unit("backend", "implement_backend", &["src/auth/**"], &[]),
            unit("tests", "write_tests", &["tests/**"], &["backend"]),
        ]);
        let (admitted, refusals) = admit_plan(
            &plan,
            &registry,
            &RoutingPolicy::default(),
            planning_context(&ceiling, &ceiling, &governance, &ledger),
        );
        assert!(refusals.is_empty(), "{refusals:?}");
        assert_eq!(
            admitted[1].delegation.dependencies(),
            [admitted[0].delegation.id()]
        );
    }

    #[test]
    fn a_unit_with_no_specialist_is_refused_without_failing_the_plan() {
        let registry = registry_with(&[("backend", "implement_backend")]);
        let ceiling = permissive();
        let governance = DelegationGovernance::default();
        let ledger = DelegationLedger::default();
        let plan = plan(vec![
            unit("backend", "implement_backend", &["src/auth/**"], &[]),
            unit("review", "security_review", &["src/**"], &[]),
        ]);
        let (admitted, refusals) = admit_plan(
            &plan,
            &registry,
            &RoutingPolicy::default(),
            planning_context(&ceiling, &ceiling, &governance, &ledger),
        );
        assert_eq!(admitted.len(), 1);
        assert_eq!(refusals.len(), 1);
        assert_eq!(refusals[0].0, "review");
        assert!(matches!(
            refusals[0].1,
            DelegationError::CapabilityUnavailable { .. }
        ));
    }

    #[test]
    fn a_read_only_parent_refuses_every_writer_unit() {
        // The §9 permission-escalation test, at plan level: no amount of
        // profile authority turns a read-only parent into a writer.
        let registry = registry_with(&[("backend", "implement_backend")]);
        let workspace = permissive();
        let parent = ToolCeiling {
            maximum_side_effect: SideEffectClass::Read,
            maximum_filesystem: FilesystemScope::WorktreeRead,
            ..permissive()
        };
        let governance = DelegationGovernance::default();
        let ledger = DelegationLedger::default();
        let plan = plan(vec![unit(
            "backend",
            "implement_backend",
            &["src/auth/**"],
            &[],
        )]);
        let (admitted, refusals) = admit_plan(
            &plan,
            &registry,
            &RoutingPolicy::default(),
            planning_context(&workspace, &parent, &governance, &ledger),
        );
        assert!(admitted.is_empty());
        assert!(matches!(
            refusals[0].1,
            DelegationError::WriterUnderReadOnlyParent
        ));
    }

    #[test]
    fn a_cyclic_plan_is_refused_before_anything_is_admitted() {
        let registry = registry_with(&[("backend", "implement_backend")]);
        let ceiling = permissive();
        let governance = DelegationGovernance::default();
        let ledger = DelegationLedger::default();
        let plan = plan(vec![
            unit("a", "implement_backend", &["src/**"], &["b"]),
            unit("b", "implement_backend", &["src/**"], &["a"]),
        ]);
        let (admitted, refusals) = admit_plan(
            &plan,
            &registry,
            &RoutingPolicy::default(),
            planning_context(&ceiling, &ceiling, &governance, &ledger),
        );
        assert!(admitted.is_empty());
        assert!(matches!(
            refusals[0].1,
            DelegationError::InvalidDependencyGraph { .. }
        ));
    }

    #[test]
    fn worker_budgets_are_clamped_to_what_the_session_has_left() {
        let registry = registry_with(&[("backend", "implement_backend")]);
        let ceiling = permissive();
        let governance = DelegationGovernance {
            maximum_total_worker_input_tokens: 5_000,
            ..DelegationGovernance::default()
        };
        let ledger = DelegationLedger::default();
        let plan = plan(vec![unit(
            "backend",
            "implement_backend",
            &["src/auth/**"],
            &[],
        )]);
        let (admitted, _) = admit_plan(
            &plan,
            &registry,
            &RoutingPolicy::default(),
            planning_context(&ceiling, &ceiling, &governance, &ledger),
        );
        assert_eq!(
            admitted[0].delegation.budget().maximum_input_tokens,
            5_000,
            "the unit asked for more than the session has; it gets what is left"
        );
    }
}
