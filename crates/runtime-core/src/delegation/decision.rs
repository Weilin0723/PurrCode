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

/// One unit as *proposed* — by the main agent mid-turn, or by an API caller.
///
/// Deliberately lenient: every field is a plain string, so a model that names a
/// capability that does not exist produces a refusable unit rather than a turn
/// that fails to deserialize. Validation happens where it can be reported
/// (`PlannedUnit` conversion), not in serde.
#[derive(Clone, Debug, Default, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct DelegationUnitProposal {
    /// Stable within one proposal, so `depends_on` can name siblings.
    pub key: String,
    pub objective: String,
    pub capability: String,
    #[serde(default)]
    pub allowed_paths: Vec<String>,
    /// `patch` | `review` | `investigation` | `validation`. Absent means patch.
    #[serde(default)]
    pub expected_output: Option<String>,
    #[serde(default)]
    pub depends_on: Vec<String>,
}

impl DelegationUnitProposal {
    /// True when this unit is expected to return findings rather than a patch.
    pub fn is_read_only(&self) -> bool {
        matches!(
            self.expected_output.as_deref(),
            Some("review" | "investigation" | "validation")
        )
    }
}

/// Words that mark a change as security-sensitive. Matched against the
/// objective and the delegated paths, so the signal is measured from the
/// request rather than taken from the model's own assessment of its work.
const SECURITY_SENSITIVE_MARKERS: &[&str] = &[
    "auth",
    "oauth",
    "login",
    "session",
    "token",
    "password",
    "credential",
    "secret",
    "crypto",
    "encrypt",
    "signature",
    "permission",
    "privilege",
    "payment",
    "billing",
    "invoice",
];

impl DelegationSignals {
    /// Derive the classifier's inputs from a proposal (v1.4 §PR2).
    ///
    /// Every signal here is *measured* — from the units, their paths and the
    /// objective text — rather than asked of the model. A model that wants
    /// three workers cannot get them by claiming the task is complex; it gets
    /// them only if three independent units with distinct scopes are actually
    /// present and specialists exist to run them.
    pub fn from_proposal(
        objective: &str,
        units: &[DelegationUnitProposal],
        available_specialist_capabilities: u32,
    ) -> Self {
        let haystack = {
            let mut haystack = objective.to_ascii_lowercase();
            for unit in units {
                haystack.push(' ');
                haystack.push_str(&unit.objective.to_ascii_lowercase());
                for path in &unit.allowed_paths {
                    haystack.push(' ');
                    haystack.push_str(&path.to_ascii_lowercase());
                }
            }
            haystack
        };
        let security_sensitive = SECURITY_SENSITIVE_MARKERS
            .iter()
            .any(|marker| haystack.contains(marker));

        // Distinct top-level directories across every delegated path.
        let spanned_modules: std::collections::BTreeSet<&str> = units
            .iter()
            .flat_map(|unit| unit.allowed_paths.iter())
            .filter_map(|path| path.split('/').next())
            .filter(|segment| !segment.is_empty() && *segment != "**")
            .collect();

        // A scope naming only concrete files (no glob) in one place is a
        // localized fix however many units were proposed for it.
        let concrete_paths: Vec<&String> = units
            .iter()
            .flat_map(|unit| unit.allowed_paths.iter())
            .collect();
        let localized_fix = !concrete_paths.is_empty()
            && concrete_paths.len() <= 1
            && concrete_paths
                .iter()
                .all(|path| !path.contains('*') && path.contains('.'));

        let writer_units = units.iter().filter(|unit| !unit.is_read_only()).count() as u32;
        Self {
            independent_components: writer_units.max(1),
            estimated_files: 0,
            spanned_modules: spanned_modules.len() as u32,
            available_specialist_capabilities,
            components_are_independent: units.iter().all(|unit| unit.depends_on.is_empty()),
            security_sensitive,
            review_requested: units.iter().any(|unit| {
                unit.expected_output.as_deref() == Some("review")
                    || unit.capability.contains("review")
            }),
            tests_separable: units
                .iter()
                .any(|unit| unit.capability.contains("test") || unit.key.contains("test")),
            localized_fix,
            complexity_hint: 0,
        }
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

    fn proposal(key: &str, capability: &str, paths: &[&str]) -> DelegationUnitProposal {
        DelegationUnitProposal {
            key: key.into(),
            objective: format!("do {key}"),
            capability: capability.into(),
            allowed_paths: paths.iter().map(|p| (*p).to_owned()).collect(),
            expected_output: None,
            depends_on: Vec::new(),
        }
    }

    #[test]
    fn signals_are_measured_from_the_proposal_not_claimed_by_the_model() {
        let units = vec![
            proposal("backend", "implement_backend", &["src/auth/**"]),
            proposal("migration", "database_migration", &["migrations/**"]),
            DelegationUnitProposal {
                expected_output: Some("review".into()),
                ..proposal("review", "security_review", &["src/**"])
            },
        ];
        let signals = DelegationSignals::from_proposal("add oauth login", &units, 3);
        // Reviewers are not implementation components: two writers, one review.
        assert_eq!(signals.independent_components, 2);
        assert!(signals.review_requested);
        assert!(
            signals.security_sensitive,
            "`oauth`/`auth` is security-sensitive"
        );
        assert!(signals.components_are_independent);
        assert_eq!(signals.spanned_modules, 2);
        assert!(!signals.localized_fix);
        assert_eq!(signals.available_specialist_capabilities, 3);
    }

    #[test]
    fn two_small_ordered_components_are_not_worth_splitting() {
        // The cost model doing its job. "Add an endpoint, then test it" is two
        // components, but ordering them removes the parallelism that would pay
        // for two worktrees and two context assemblies.
        let units = vec![
            proposal("backend", "implement_backend", &["src/api/**"]),
            DelegationUnitProposal {
                depends_on: vec!["backend".into()],
                ..proposal("tests", "write_tests", &["tests/**"])
            },
        ];
        let signals = DelegationSignals::from_proposal("add an endpoint", &units, 2);
        assert!(!signals.components_are_independent);
        assert!(signals.tests_separable);
        let plan = classify(&signals);
        assert_eq!(plan.classification, DelegationClassification::Single);
        assert!(plan.expected_benefit < plan.coordination_cost, "{plan:?}");
    }

    #[test]
    fn a_dependency_makes_a_worthwhile_split_sequential_rather_than_parallel() {
        let units = vec![
            proposal("migration", "database_migration", &["migrations/**"]),
            DelegationUnitProposal {
                depends_on: vec!["migration".into()],
                ..proposal("backend", "implement_backend", &["src/auth/**"])
            },
            DelegationUnitProposal {
                depends_on: vec!["backend".into()],
                ..proposal("tests", "write_tests", &["tests/auth/**"])
            },
            DelegationUnitProposal {
                expected_output: Some("review".into()),
                depends_on: vec!["backend".into()],
                ..proposal("review", "security_review", &["src/**"])
            },
        ];
        let signals = DelegationSignals::from_proposal("add oauth account linking", &units, 4);
        assert!(!signals.components_are_independent);
        assert_eq!(
            signals.independent_components, 3,
            "the reviewer is not a component"
        );
        let plan = classify(&signals);
        assert_eq!(
            plan.classification,
            DelegationClassification::SequentialDelegation,
            "{plan:?}"
        );
    }

    #[test]
    fn the_north_star_task_delegates_from_a_measured_proposal() {
        // §2: "Add OAuth support, update the database schema, add tests, and
        // perform a security review." Derived signals only — nothing the model
        // asserted about complexity.
        let units = vec![
            proposal("backend", "implement_backend", &["src/auth/**"]),
            proposal("migration", "database_migration", &["migrations/**"]),
            proposal("tests", "write_tests", &["tests/auth/**"]),
            DelegationUnitProposal {
                expected_output: Some("review".into()),
                ..proposal("review", "security_review", &["src/**"])
            },
        ];
        let signals = DelegationSignals::from_proposal(
            "Add OAuth support, update the database schema, add tests, and perform a security review",
            &units,
            4,
        );
        let plan = classify(&signals);
        assert_eq!(
            plan.classification,
            DelegationClassification::ParallelDelegation,
            "{plan:?}"
        );
        assert!(plan.expected_benefit - plan.coordination_cost >= 1);
    }

    #[test]
    fn a_single_concrete_file_stays_single_agent_however_many_units_are_proposed() {
        // The guard against a model splitting a one-line change three ways:
        // the scope is one concrete file, so the runtime refuses regardless.
        let units = vec![proposal("a", "implement_backend", &["src/lib.rs"])];
        let signals = DelegationSignals::from_proposal("rename the retry limit", &units, 5);
        assert!(signals.localized_fix);
        let plan = classify(&signals);
        assert_eq!(plan.classification, DelegationClassification::Single);
        assert!(plan.reason.contains("localized"));
    }

    #[test]
    fn a_proposal_with_no_specialists_available_does_not_delegate() {
        let units = vec![
            proposal("backend", "implement_backend", &["src/auth/**"]),
            proposal("frontend", "implement_frontend", &["web/**"]),
        ];
        // No registered specialist can satisfy either capability.
        let signals = DelegationSignals::from_proposal("wire up sign-in", &units, 0);
        let plan = classify(&signals);
        assert_eq!(
            plan.classification,
            DelegationClassification::Single,
            "specialists that do not exist cannot help: {}",
            plan.reason
        );
    }

    #[test]
    fn a_read_only_unit_is_recognized_without_parsing() {
        let review = DelegationUnitProposal {
            expected_output: Some("review".into()),
            ..proposal("r", "security_review", &[])
        };
        assert!(review.is_read_only());
        assert!(!proposal("w", "implement_backend", &[]).is_read_only());
        // An unknown output kind is not silently treated as read-only.
        let unknown = DelegationUnitProposal {
            expected_output: Some("something-else".into()),
            ..proposal("u", "implement_backend", &[])
        };
        assert!(!unknown.is_read_only());
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
