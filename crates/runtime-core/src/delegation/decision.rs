//! The delegation decision layer (v1.4 §PR2).
//!
//! The question this module answers is *"is more than one agent actually
//! useful here?"*, and its most important answer is **no**. Delegation buys
//! parallelism and independent review; it costs a planning turn, N worktrees,
//! N context assemblies, an integration pass and a conflict risk. For "rename
//! this variable" that trade is strictly worse than doing the work.
//!
//! The classifier is deterministic and explainable: the same signals always
//! produce the same classification, and every decision carries the signals that
//! drove it, so a surprising choice is inspectable rather than mysterious. It is
//! *not* a user-facing mode — v1.3's "no Plan / Build toggle" rule holds. The
//! runtime decides.

use super::{DelegationBudget, ExpectedOutput, PathPattern};
use crate::CapabilityId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Deterministic inputs to the classifier (v1.4 §PR2 "Input Signals").
///
/// Every field is something the runtime can measure or the planner can state
/// plainly — none of them require a model call to fill in.
#[derive(Clone, Debug, Default, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct DelegationSignals {
    /// Components that could be worked on independently (backend, migration,
    /// frontend, docs…). One component is a single-agent task by definition.
    pub independent_components: u32,
    /// How many files the task is expected to touch, when known.
    pub estimated_files: u32,
    /// Distinct modules/directories the change spans.
    pub spanned_modules: u32,
    /// Specialist capabilities available in the registry for this task.
    pub available_specialist_capabilities: u32,
    /// True when the components have no ordering constraint between them.
    pub components_are_independent: bool,
    /// The change touches auth, crypto, permissions, payments or similar.
    pub security_sensitive: bool,
    /// The user (or policy) asked for an independent review.
    pub review_requested: bool,
    /// Tests can be written against an interface without waiting for the
    /// implementation to land.
    pub tests_separable: bool,
    /// The task is a single localized fix (rename, typo, one failing test).
    pub localized_fix: bool,
    /// Rough size hint, 0–10. Used only as a tiebreak.
    pub complexity_hint: u8,
}

impl DelegationSignals {
    /// The coordination cost of delegating, in the same arbitrary units as
    /// [`Self::expected_benefit`]. Fixed overhead plus a per-worker cost:
    /// planning, worktree setup, context assembly, integration and review all
    /// scale with the number of workers.
    pub fn coordination_cost(&self, workers: u32) -> i32 {
        const FIXED_PLANNING_COST: i32 = 3;
        const PER_WORKER_COST: i32 = 2;
        FIXED_PLANNING_COST + PER_WORKER_COST * workers as i32
    }

    /// The benefit delegation is expected to produce. Parallelism only pays when
    /// components are genuinely independent; independent review pays whenever
    /// the change is security-sensitive or a review was asked for.
    pub fn expected_benefit(&self) -> i32 {
        let mut benefit = 0;
        if self.independent_components > 1 {
            let parallelizable = if self.components_are_independent {
                self.independent_components as i32 - 1
            } else {
                // Sequential components still benefit from specialist routing,
                // but not from wall-clock parallelism.
                (self.independent_components as i32 - 1) / 2
            };
            benefit += parallelizable * 3;
        }
        if self.tests_separable && self.independent_components > 1 {
            benefit += 2;
        }
        if self.security_sensitive {
            benefit += 4;
        }
        if self.review_requested {
            benefit += 4;
        }
        if self.spanned_modules >= 3 {
            benefit += 2;
        }
        if self.estimated_files >= 8 {
            benefit += 2;
        }
        benefit += (self.complexity_hint.min(10) as i32) / 4;
        // A specialist that does not exist cannot help.
        if self.available_specialist_capabilities == 0 {
            benefit /= 2;
        }
        benefit
    }
}

/// What the runtime decided to do (v1.4 §PR2).
#[derive(
    Clone, Copy, Debug, Default, Eq, JsonSchema, Ord, PartialEq, PartialOrd, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum DelegationClassification {
    /// One agent, no workers. The default and the common case.
    #[default]
    Single,
    /// Specialists, but ordered: B needs A's result.
    SequentialDelegation,
    /// Specialists that can run at the same time in isolated worktrees.
    ParallelDelegation,
    /// One implementation path plus an independent read-only reviewer.
    ReviewDelegation,
}

impl DelegationClassification {
    pub fn delegates(self) -> bool {
        !matches!(self, DelegationClassification::Single)
    }

    pub fn label(self) -> &'static str {
        match self {
            DelegationClassification::Single => "single",
            DelegationClassification::SequentialDelegation => "sequential",
            DelegationClassification::ParallelDelegation => "parallel",
            DelegationClassification::ReviewDelegation => "review",
        }
    }
}

/// One unit the planner proposes. Turned into a [`super::DelegationRequest`] by
/// the caller, which supplies the parent ids and the authority inputs.
#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct PlannedUnit {
    /// Stable within one plan, so dependencies can name each other before any
    /// [`super::DelegationId`] exists.
    pub key: String,
    pub objective: String,
    pub capability: CapabilityId,
    pub expected_output: ExpectedOutput,
    #[serde(default)]
    pub allowed_paths: Vec<PathPattern>,
    /// Keys of units this one depends on.
    #[serde(default)]
    pub depends_on: Vec<String>,
    pub budget: DelegationBudget,
}

/// The classifier's explainable output.
#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct DelegationPlan {
    pub classification: DelegationClassification,
    pub signals: DelegationSignals,
    pub expected_benefit: i32,
    pub coordination_cost: i32,
    /// Human-readable reason, shown in the agent workspace.
    pub reason: String,
    #[serde(default)]
    pub units: Vec<PlannedUnit>,
}

impl DelegationPlan {
    /// True when the plan will actually spawn workers.
    pub fn delegates(&self) -> bool {
        self.classification.delegates() && !self.units.is_empty()
    }

    /// The dependency graph must be acyclic and reference only keys in the plan
    /// — checked here so a malformed plan is rejected before any worktree is
    /// created (v1.4 §PR4).
    pub fn validate(&self) -> Result<(), super::DelegationError> {
        let invalid = |reason: String| super::DelegationError::InvalidDependencyGraph { reason };
        let mut keys = std::collections::BTreeSet::new();
        for unit in &self.units {
            if unit.key.trim().is_empty() {
                return Err(invalid("a unit has an empty key".into()));
            }
            if !keys.insert(unit.key.as_str()) {
                return Err(invalid(format!("duplicate unit key `{}`", unit.key)));
            }
        }
        for unit in &self.units {
            for dependency in &unit.depends_on {
                if dependency == &unit.key {
                    return Err(invalid(format!("unit `{}` depends on itself", unit.key)));
                }
                if !keys.contains(dependency.as_str()) {
                    return Err(invalid(format!(
                        "unit `{}` depends on unknown unit `{dependency}`",
                        unit.key
                    )));
                }
            }
        }
        // Kahn's algorithm: anything left after the sweep sits in a cycle.
        let mut remaining: Vec<&PlannedUnit> = self.units.iter().collect();
        let mut settled = std::collections::BTreeSet::new();
        loop {
            let ready: Vec<&PlannedUnit> = remaining
                .iter()
                .copied()
                .filter(|unit| unit.depends_on.iter().all(|d| settled.contains(d.as_str())))
                .collect();
            if ready.is_empty() {
                break;
            }
            for unit in ready {
                settled.insert(unit.key.as_str());
            }
            remaining.retain(|unit| !settled.contains(unit.key.as_str()));
            if remaining.is_empty() {
                break;
            }
        }
        if !remaining.is_empty() {
            return Err(invalid(format!(
                "cyclic dependencies among {:?}",
                remaining.iter().map(|u| &u.key).collect::<Vec<_>>()
            )));
        }
        Ok(())
    }
}

/// The benefit margin delegation must clear before it is chosen. Delegation is
/// not free, so a tie goes to the single agent.
const REQUIRED_MARGIN: i32 = 1;

/// Classify a task from deterministic signals (v1.4 §PR2).
///
/// The rule in one line: **delegate only when the expected benefit exceeds the
/// coordination cost by a margin.** Everything else here is the shape of the
/// delegation once that test passes.
pub fn classify(signals: &DelegationSignals) -> DelegationPlan {
    let plan =
        |classification: DelegationClassification, workers: u32, reason: String| DelegationPlan {
            classification,
            signals: signals.clone(),
            expected_benefit: signals.expected_benefit(),
            coordination_cost: signals.coordination_cost(workers),
            reason,
            units: Vec::new(),
        };

    // A localized fix is single-agent regardless of anything else. Splitting a
    // rename across two agents costs a worktree and gains nothing.
    if signals.localized_fix {
        return plan(
            DelegationClassification::Single,
            0,
            "the task is a single localized change; delegation would add coordination \
             cost with nothing to parallelize"
                .into(),
        );
    }

    // Nothing to split and nobody to review: single.
    if signals.independent_components <= 1
        && !signals.security_sensitive
        && !signals.review_requested
    {
        return plan(
            DelegationClassification::Single,
            0,
            "the task has one component and no review requirement".into(),
        );
    }

    // The review-only shape: one implementation stream plus an independent
    // read-only reviewer. This is the cheapest delegation, so it is checked
    // before the multi-worker shapes.
    if signals.independent_components <= 1 {
        let workers = 1;
        let benefit = signals.expected_benefit();
        let cost = signals.coordination_cost(workers);
        if benefit - cost < REQUIRED_MARGIN {
            return plan(
                DelegationClassification::Single,
                workers,
                format!(
                    "an independent review was indicated, but its expected benefit ({benefit}) \
                     does not exceed the coordination cost ({cost})"
                ),
            );
        }
        return plan(
            DelegationClassification::ReviewDelegation,
            workers,
            format!(
                "one implementation stream with an independent read-only review \
                 (benefit {benefit} vs cost {cost})"
            ),
        );
    }

    let implementation_workers = signals.independent_components;
    let reviewers = u32::from(signals.security_sensitive || signals.review_requested);
    let workers = implementation_workers + reviewers;
    let benefit = signals.expected_benefit();
    let cost = signals.coordination_cost(workers);
    if benefit - cost < REQUIRED_MARGIN {
        return plan(
            DelegationClassification::Single,
            workers,
            format!(
                "splitting into {workers} workers would cost more to coordinate ({cost}) \
                 than it is expected to gain ({benefit})"
            ),
        );
    }

    if signals.components_are_independent {
        plan(
            DelegationClassification::ParallelDelegation,
            workers,
            format!(
                "{implementation_workers} independent components can run in isolated worktrees \
                 (benefit {benefit} vs cost {cost})"
            ),
        )
    } else {
        plan(
            DelegationClassification::SequentialDelegation,
            workers,
            format!(
                "{implementation_workers} components with ordering constraints run in sequence \
                 (benefit {benefit} vs cost {cost})"
            ),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn signals() -> DelegationSignals {
        DelegationSignals::default()
    }

    #[test]
    fn a_rename_stays_single_agent() {
        let plan = classify(&DelegationSignals {
            localized_fix: true,
            estimated_files: 1,
            independent_components: 1,
            complexity_hint: 1,
            ..signals()
        });
        assert_eq!(plan.classification, DelegationClassification::Single);
        assert!(!plan.delegates());
        assert!(plan.reason.contains("localized"));
    }

    #[test]
    fn a_failing_test_fix_stays_single_agent() {
        let plan = classify(&DelegationSignals {
            localized_fix: true,
            independent_components: 1,
            estimated_files: 2,
            available_specialist_capabilities: 4,
            complexity_hint: 3,
            ..signals()
        });
        assert_eq!(plan.classification, DelegationClassification::Single);
    }

    #[test]
    fn oauth_backend_migration_tests_and_review_goes_parallel() {
        // The §2 north-star task.
        let plan = classify(&DelegationSignals {
            independent_components: 3,
            components_are_independent: true,
            estimated_files: 14,
            spanned_modules: 4,
            available_specialist_capabilities: 5,
            security_sensitive: true,
            tests_separable: true,
            complexity_hint: 8,
            ..signals()
        });
        assert_eq!(
            plan.classification,
            DelegationClassification::ParallelDelegation
        );
        assert!(plan.expected_benefit > plan.coordination_cost);
    }

    #[test]
    fn ordered_components_go_sequential() {
        let plan = classify(&DelegationSignals {
            independent_components: 3,
            components_are_independent: false,
            estimated_files: 12,
            spanned_modules: 3,
            available_specialist_capabilities: 4,
            review_requested: true,
            complexity_hint: 8,
            ..signals()
        });
        assert_eq!(
            plan.classification,
            DelegationClassification::SequentialDelegation
        );
    }

    #[test]
    fn a_single_component_security_change_gets_a_reviewer() {
        let plan = classify(&DelegationSignals {
            independent_components: 1,
            security_sensitive: true,
            review_requested: true,
            estimated_files: 4,
            available_specialist_capabilities: 3,
            complexity_hint: 6,
            ..signals()
        });
        assert_eq!(
            plan.classification,
            DelegationClassification::ReviewDelegation
        );
    }

    #[test]
    fn delegation_requires_a_measurable_benefit() {
        // Two components, but tiny, with no specialists available: the
        // coordination cost wins and the runtime stays single-agent.
        let plan = classify(&DelegationSignals {
            independent_components: 2,
            components_are_independent: true,
            estimated_files: 2,
            spanned_modules: 1,
            available_specialist_capabilities: 0,
            complexity_hint: 1,
            ..signals()
        });
        assert_eq!(plan.classification, DelegationClassification::Single);
        assert!(plan.reason.contains("coordinate"));
    }

    #[test]
    fn classification_is_deterministic() {
        let input = DelegationSignals {
            independent_components: 3,
            components_are_independent: true,
            estimated_files: 14,
            spanned_modules: 4,
            available_specialist_capabilities: 5,
            security_sensitive: true,
            tests_separable: true,
            complexity_hint: 8,
            ..signals()
        };
        let first = classify(&input);
        let second = classify(&input);
        assert_eq!(first, second);
    }

    fn unit(key: &str, depends_on: &[&str]) -> PlannedUnit {
        PlannedUnit {
            key: key.into(),
            objective: format!("do {key}"),
            capability: CapabilityId::parse("implement_backend").unwrap(),
            expected_output: ExpectedOutput::Patch,
            allowed_paths: vec![PathPattern::parse("src/**").unwrap()],
            depends_on: depends_on.iter().map(|d| (*d).to_owned()).collect(),
            budget: DelegationBudget::modest(),
        }
    }

    fn plan_with(units: Vec<PlannedUnit>) -> DelegationPlan {
        DelegationPlan {
            classification: DelegationClassification::ParallelDelegation,
            signals: signals(),
            expected_benefit: 10,
            coordination_cost: 5,
            reason: "test".into(),
            units,
        }
    }

    #[test]
    fn plan_validation_rejects_cycles_and_unknown_dependencies() {
        let cyclic = plan_with(vec![unit("a", &["b"]), unit("b", &["a"])]);
        assert!(cyclic.validate().is_err());

        let unknown = plan_with(vec![unit("a", &["ghost"])]);
        assert!(unknown.validate().is_err());

        let duplicate = plan_with(vec![unit("a", &[]), unit("a", &[])]);
        assert!(duplicate.validate().is_err());

        let self_dependency = plan_with(vec![unit("a", &["a"])]);
        assert!(self_dependency.validate().is_err());

        let acyclic = plan_with(vec![
            unit("backend", &[]),
            unit("migration", &[]),
            unit("tests", &["backend", "migration"]),
        ]);
        assert!(acyclic.validate().is_ok());
    }
}
