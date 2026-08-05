//! Durable product work model.
//!
//! Conversation is how a person steers PurrCode, but it is not an adequate
//! source of truth for a long-running change. This module owns the stable
//! requirement → decision → task → evidence relationships used by every
//! client and replayed by NineLives.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use crate::{ActionId, ValidationStatus};

macro_rules! durable_id {
    ($name:ident) => {
        #[derive(
            Clone,
            Copy,
            Debug,
            Eq,
            Hash,
            JsonSchema,
            Ord,
            PartialEq,
            PartialOrd,
            Serialize,
            Deserialize,
        )]
        #[serde(transparent)]
        pub struct $name(pub Uuid);

        impl $name {
            pub fn new() -> Self {
                Self(Uuid::new_v4())
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }
    };
}

durable_id!(RequirementId);
durable_id!(CriterionId);
durable_id!(DesignDecisionId);
durable_id!(WorkTaskId);
durable_id!(EvidenceId);

#[derive(Clone, Copy, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpecKind {
    Direct,
    FeatureRequirementsFirst,
    FeatureDesignFirst,
    Bugfix,
    Quick,
}

#[derive(Clone, Copy, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkPriority {
    Required,
    Optional,
}

#[derive(
    Clone, Copy, Debug, Eq, JsonSchema, Ord, PartialEq, PartialOrd, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum WorkRisk {
    Low,
    Medium,
    High,
    Critical,
}

#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct AcceptanceCriterion {
    pub id: CriterionId,
    pub statement: String,
}

#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct Requirement {
    pub id: RequirementId,
    pub statement: String,
    pub priority: WorkPriority,
    #[serde(default)]
    pub acceptance_criteria: Vec<AcceptanceCriterion>,
}

#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct DesignDecision {
    pub id: DesignDecisionId,
    pub title: String,
    pub decision: String,
    pub rationale: String,
    #[serde(default)]
    pub affected_components: Vec<String>,
}

#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct SpecBundle {
    pub revision: u64,
    pub kind: SpecKind,
    pub title: String,
    #[serde(default)]
    pub requirements: Vec<Requirement>,
    #[serde(default)]
    pub non_goals: Vec<String>,
    #[serde(default)]
    pub design_decisions: Vec<DesignDecision>,
}

impl SpecBundle {
    pub fn validate(&self) -> Result<(), WorkModelError> {
        if self.revision == 0 {
            return Err(WorkModelError::InvalidSpec(
                "spec revision must start at one".into(),
            ));
        }
        require_text("spec title", &self.title)?;
        let mut requirement_ids = BTreeSet::new();
        let mut criterion_ids = BTreeSet::new();
        for requirement in &self.requirements {
            if !requirement_ids.insert(requirement.id) {
                return Err(WorkModelError::DuplicateRequirement(requirement.id));
            }
            require_text("requirement", &requirement.statement)?;
            if requirement.priority == WorkPriority::Required
                && requirement.acceptance_criteria.is_empty()
            {
                return Err(WorkModelError::MissingAcceptanceCriteria(requirement.id));
            }
            for criterion in &requirement.acceptance_criteria {
                if !criterion_ids.insert(criterion.id) {
                    return Err(WorkModelError::DuplicateCriterion(criterion.id));
                }
                require_text("acceptance criterion", &criterion.statement)?;
            }
        }
        let mut decision_ids = BTreeSet::new();
        for decision in &self.design_decisions {
            if !decision_ids.insert(decision.id) {
                return Err(WorkModelError::DuplicateDecision(decision.id));
            }
            require_text("design decision title", &decision.title)?;
            require_text("design decision", &decision.decision)?;
            require_text("design decision rationale", &decision.rationale)?;
        }
        Ok(())
    }
}

#[derive(
    Clone, Copy, Debug, Eq, JsonSchema, Ord, PartialEq, PartialOrd, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum WorkTaskStatus {
    Pending,
    Ready,
    Running,
    Blocked,
    NeedsAttention,
    Passed,
    Failed,
    Skipped,
    Cancelled,
}

impl WorkTaskStatus {
    pub const fn terminal(self) -> bool {
        matches!(self, Self::Passed | Self::Skipped | Self::Cancelled)
    }
}

#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct EvidenceObligation {
    pub requirement_id: RequirementId,
    pub criterion_id: CriterionId,
    pub description: String,
    pub required: bool,
}

#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct WorkTask {
    pub id: WorkTaskId,
    pub objective: String,
    #[serde(default)]
    pub dependencies: Vec<WorkTaskId>,
    pub priority: WorkPriority,
    pub risk: WorkRisk,
    #[serde(default)]
    pub acceptance_criteria: Vec<CriterionId>,
    #[serde(default)]
    pub scope: Vec<PathBuf>,
    #[serde(default)]
    pub owner: Option<String>,
    pub status: WorkTaskStatus,
    #[serde(default)]
    pub retry_count: u32,
    #[serde(default)]
    pub evidence_obligations: Vec<EvidenceObligation>,
}

#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct TaskGraph {
    pub revision: u64,
    pub tasks: Vec<WorkTask>,
}

impl TaskGraph {
    pub fn validate(&self, spec: Option<&SpecBundle>) -> Result<(), WorkModelError> {
        if self.revision == 0 {
            return Err(WorkModelError::InvalidGraph(
                "task graph revision must start at one".into(),
            ));
        }
        let task_ids: BTreeSet<_> = self.tasks.iter().map(|task| task.id).collect();
        if task_ids.len() != self.tasks.len() {
            return Err(WorkModelError::InvalidGraph(
                "task ids must be unique".into(),
            ));
        }
        let criteria: BTreeSet<_> = spec
            .into_iter()
            .flat_map(|bundle| &bundle.requirements)
            .flat_map(|requirement| &requirement.acceptance_criteria)
            .map(|criterion| criterion.id)
            .collect();
        for task in &self.tasks {
            require_text("task objective", &task.objective)?;
            if task.priority == WorkPriority::Required && task.acceptance_criteria.is_empty() {
                return Err(WorkModelError::TaskMissingAcceptanceCriteria(task.id));
            }
            for dependency in &task.dependencies {
                if *dependency == task.id {
                    return Err(WorkModelError::DependencyCycle(task.id));
                }
                if !task_ids.contains(dependency) {
                    return Err(WorkModelError::MissingDependency {
                        task: task.id,
                        dependency: *dependency,
                    });
                }
            }
            if spec.is_some() {
                for criterion in &task.acceptance_criteria {
                    if !criteria.contains(criterion) {
                        return Err(WorkModelError::UnknownCriterion(*criterion));
                    }
                }
                for obligation in &task.evidence_obligations {
                    let requirement = spec
                        .into_iter()
                        .flat_map(|bundle| &bundle.requirements)
                        .find(|requirement| requirement.id == obligation.requirement_id)
                        .ok_or(WorkModelError::UnknownRequirement(
                            obligation.requirement_id,
                        ))?;
                    if !requirement
                        .acceptance_criteria
                        .iter()
                        .any(|criterion| criterion.id == obligation.criterion_id)
                    {
                        return Err(WorkModelError::CriterionRequirementMismatch {
                            requirement: obligation.requirement_id,
                            criterion: obligation.criterion_id,
                        });
                    }
                    if !task.acceptance_criteria.contains(&obligation.criterion_id) {
                        return Err(WorkModelError::TaskDoesNotOwnCriterion {
                            task: task.id,
                            criterion: obligation.criterion_id,
                        });
                    }
                    require_text("evidence obligation", &obligation.description)?;
                }
            }
        }
        self.ensure_acyclic()
    }

    pub fn task(&self, id: WorkTaskId) -> Option<&WorkTask> {
        self.tasks.iter().find(|task| task.id == id)
    }

    pub fn ready_tasks(&self) -> Vec<&WorkTask> {
        self.tasks
            .iter()
            .filter(|task| task.status == WorkTaskStatus::Ready)
            .collect()
    }

    /// Recompute tasks whose prerequisites have completed successfully.
    ///
    /// A skipped/cancelled prerequisite is deliberately not considered
    /// satisfied: the graph must be revised explicitly if downstream work no
    /// longer needs it.
    pub fn refresh_ready(&mut self) {
        let passed: BTreeSet<_> = self
            .tasks
            .iter()
            .filter(|task| task.status == WorkTaskStatus::Passed)
            .map(|task| task.id)
            .collect();
        for task in &mut self.tasks {
            if task.status == WorkTaskStatus::Pending
                && task
                    .dependencies
                    .iter()
                    .all(|dependency| passed.contains(dependency))
            {
                task.status = WorkTaskStatus::Ready;
            }
        }
    }

    pub fn transition(
        &mut self,
        id: WorkTaskId,
        next: WorkTaskStatus,
    ) -> Result<(), WorkModelError> {
        let current = self.task(id).ok_or(WorkModelError::UnknownTask(id))?.status;
        if !valid_task_transition(current, next) {
            return Err(WorkModelError::InvalidTaskTransition {
                task: id,
                current,
                next,
            });
        }
        let task = self
            .tasks
            .iter_mut()
            .find(|task| task.id == id)
            .expect("task existence checked above");
        if matches!(
            (current, next),
            (
                WorkTaskStatus::Failed | WorkTaskStatus::NeedsAttention,
                WorkTaskStatus::Ready
            )
        ) {
            task.retry_count = task.retry_count.saturating_add(1);
        }
        task.status = next;
        if next == WorkTaskStatus::Passed {
            self.refresh_ready();
        }
        Ok(())
    }

    fn ensure_acyclic(&self) -> Result<(), WorkModelError> {
        let dependencies: BTreeMap<_, _> = self
            .tasks
            .iter()
            .map(|task| (task.id, task.dependencies.as_slice()))
            .collect();
        let mut permanent = BTreeSet::new();
        let mut temporary = BTreeSet::new();
        for id in dependencies.keys().copied() {
            visit(id, &dependencies, &mut permanent, &mut temporary)?;
        }
        Ok(())
    }
}

fn visit(
    id: WorkTaskId,
    dependencies: &BTreeMap<WorkTaskId, &[WorkTaskId]>,
    permanent: &mut BTreeSet<WorkTaskId>,
    temporary: &mut BTreeSet<WorkTaskId>,
) -> Result<(), WorkModelError> {
    if permanent.contains(&id) {
        return Ok(());
    }
    if !temporary.insert(id) {
        return Err(WorkModelError::DependencyCycle(id));
    }
    for dependency in dependencies.get(&id).copied().unwrap_or_default() {
        visit(*dependency, dependencies, permanent, temporary)?;
    }
    temporary.remove(&id);
    permanent.insert(id);
    Ok(())
}

fn valid_task_transition(current: WorkTaskStatus, next: WorkTaskStatus) -> bool {
    use WorkTaskStatus::*;
    matches!(
        (current, next),
        (Pending, Ready | Skipped | Cancelled)
            | (Ready, Running | Blocked | Skipped | Cancelled)
            | (
                Running,
                Passed | Failed | Blocked | NeedsAttention | Cancelled
            )
            | (Blocked, Ready | NeedsAttention | Failed | Cancelled)
            | (NeedsAttention, Ready | Failed | Cancelled)
            | (Failed, Ready | Cancelled)
    )
}

#[derive(Clone, Copy, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceCoverage {
    Covered,
    Failed,
    NotRun,
    Unavailable,
    NotApplicable,
    Stale,
    AcceptedException,
}

#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct EvidenceLink {
    pub id: EvidenceId,
    pub requirement_id: RequirementId,
    pub criterion_id: CriterionId,
    pub task_id: WorkTaskId,
    #[serde(default)]
    pub action_id: Option<ActionId>,
    pub coverage: EvidenceCoverage,
    #[serde(default)]
    pub validation_status: Option<ValidationStatus>,
    pub source: String,
    pub summary: String,
    pub digest: String,
    pub recorded_at: chrono::DateTime<chrono::Utc>,
}

impl EvidenceLink {
    pub fn validate(&self, spec: &SpecBundle, graph: &TaskGraph) -> Result<(), WorkModelError> {
        let requirement = spec
            .requirements
            .iter()
            .find(|requirement| requirement.id == self.requirement_id)
            .ok_or(WorkModelError::UnknownRequirement(self.requirement_id))?;
        if !requirement
            .acceptance_criteria
            .iter()
            .any(|criterion| criterion.id == self.criterion_id)
        {
            return Err(WorkModelError::UnknownCriterion(self.criterion_id));
        }
        let task = graph
            .task(self.task_id)
            .ok_or(WorkModelError::UnknownTask(self.task_id))?;
        if !task.acceptance_criteria.contains(&self.criterion_id) {
            return Err(WorkModelError::TaskDoesNotOwnCriterion {
                task: self.task_id,
                criterion: self.criterion_id,
            });
        }
        if self.coverage == EvidenceCoverage::Covered
            && self.validation_status != Some(ValidationStatus::Passed)
        {
            return Err(WorkModelError::InconsistentEvidence(
                "covered evidence requires a passed validation status".into(),
            ));
        }
        if self.coverage == EvidenceCoverage::Failed
            && self.validation_status == Some(ValidationStatus::Passed)
        {
            return Err(WorkModelError::InconsistentEvidence(
                "failed evidence cannot carry a passed validation status".into(),
            ));
        }
        require_text("evidence source", &self.source)?;
        require_text("evidence summary", &self.summary)?;
        require_text("evidence digest", &self.digest)?;
        Ok(())
    }
}

fn require_text(field: &'static str, value: &str) -> Result<(), WorkModelError> {
    if value.trim().is_empty() {
        Err(WorkModelError::EmptyField(field))
    } else {
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum WorkModelError {
    #[error("{0} must not be empty")]
    EmptyField(&'static str),
    #[error("invalid spec: {0}")]
    InvalidSpec(String),
    #[error("invalid task graph: {0}")]
    InvalidGraph(String),
    #[error("duplicate requirement {0:?}")]
    DuplicateRequirement(RequirementId),
    #[error("duplicate acceptance criterion {0:?}")]
    DuplicateCriterion(CriterionId),
    #[error("duplicate design decision {0:?}")]
    DuplicateDecision(DesignDecisionId),
    #[error("required requirement {0:?} has no acceptance criteria")]
    MissingAcceptanceCriteria(RequirementId),
    #[error("required task {0:?} has no acceptance criteria")]
    TaskMissingAcceptanceCriteria(WorkTaskId),
    #[error("task {task:?} depends on missing task {dependency:?}")]
    MissingDependency {
        task: WorkTaskId,
        dependency: WorkTaskId,
    },
    #[error("task dependency graph contains a cycle at {0:?}")]
    DependencyCycle(WorkTaskId),
    #[error("unknown requirement {0:?}")]
    UnknownRequirement(RequirementId),
    #[error("unknown acceptance criterion {0:?}")]
    UnknownCriterion(CriterionId),
    #[error("criterion {criterion:?} does not belong to requirement {requirement:?}")]
    CriterionRequirementMismatch {
        requirement: RequirementId,
        criterion: CriterionId,
    },
    #[error("task {task:?} does not own acceptance criterion {criterion:?}")]
    TaskDoesNotOwnCriterion {
        task: WorkTaskId,
        criterion: CriterionId,
    },
    #[error("inconsistent evidence: {0}")]
    InconsistentEvidence(String),
    #[error("unknown task {0:?}")]
    UnknownTask(WorkTaskId),
    #[error("task {task:?} cannot transition from {current:?} to {next:?}")]
    InvalidTaskTransition {
        task: WorkTaskId,
        current: WorkTaskStatus,
        next: WorkTaskStatus,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bundle() -> (SpecBundle, RequirementId, CriterionId) {
        let requirement_id = RequirementId::new();
        let criterion_id = CriterionId::new();
        (
            SpecBundle {
                revision: 1,
                kind: SpecKind::FeatureRequirementsFirst,
                title: "Truthful evidence panels".into(),
                requirements: vec![Requirement {
                    id: requirement_id,
                    statement: "A failed evidence request is visible".into(),
                    priority: WorkPriority::Required,
                    acceptance_criteria: vec![AcceptanceCriterion {
                        id: criterion_id,
                        statement: "A timeout renders Error, never Empty".into(),
                    }],
                }],
                non_goals: vec![],
                design_decisions: vec![],
            },
            requirement_id,
            criterion_id,
        )
    }

    fn task(id: WorkTaskId, dependency: Option<WorkTaskId>, criterion_id: CriterionId) -> WorkTask {
        WorkTask {
            id,
            objective: "Implement typed panel state".into(),
            dependencies: dependency.into_iter().collect(),
            priority: WorkPriority::Required,
            risk: WorkRisk::High,
            acceptance_criteria: vec![criterion_id],
            scope: vec![PathBuf::from("crates/purrcode-ide")],
            owner: None,
            status: WorkTaskStatus::Pending,
            retry_count: 0,
            evidence_obligations: vec![],
        }
    }

    #[test]
    fn required_requirements_need_acceptance_criteria() {
        let (mut bundle, _, _) = bundle();
        bundle.requirements[0].acceptance_criteria.clear();
        assert!(matches!(
            bundle.validate(),
            Err(WorkModelError::MissingAcceptanceCriteria(_))
        ));
    }

    #[test]
    fn task_graph_rejects_cycles_and_missing_dependencies() {
        let (bundle, _, criterion) = bundle();
        let first = WorkTaskId::new();
        let second = WorkTaskId::new();
        let cycle = TaskGraph {
            revision: 1,
            tasks: vec![
                task(first, Some(second), criterion),
                task(second, Some(first), criterion),
            ],
        };
        assert!(matches!(
            cycle.validate(Some(&bundle)),
            Err(WorkModelError::DependencyCycle(_))
        ));

        let missing = TaskGraph {
            revision: 1,
            tasks: vec![task(first, Some(WorkTaskId::new()), criterion)],
        };
        assert!(matches!(
            missing.validate(Some(&bundle)),
            Err(WorkModelError::MissingDependency { .. })
        ));
    }

    #[test]
    fn passing_a_task_releases_only_satisfied_dependents() {
        let (bundle, _, criterion) = bundle();
        let first = WorkTaskId::new();
        let second = WorkTaskId::new();
        let mut graph = TaskGraph {
            revision: 1,
            tasks: vec![
                task(first, None, criterion),
                task(second, Some(first), criterion),
            ],
        };
        graph.validate(Some(&bundle)).unwrap();
        graph.refresh_ready();
        assert_eq!(graph.task(first).unwrap().status, WorkTaskStatus::Ready);
        assert_eq!(graph.task(second).unwrap().status, WorkTaskStatus::Pending);
        graph.transition(first, WorkTaskStatus::Running).unwrap();
        graph.transition(first, WorkTaskStatus::Passed).unwrap();
        assert_eq!(graph.task(second).unwrap().status, WorkTaskStatus::Ready);
    }

    #[test]
    fn terminal_tasks_cannot_be_reopened_silently() {
        let (bundle, _, criterion) = bundle();
        let id = WorkTaskId::new();
        let mut graph = TaskGraph {
            revision: 1,
            tasks: vec![task(id, None, criterion)],
        };
        graph.validate(Some(&bundle)).unwrap();
        graph.refresh_ready();
        graph.transition(id, WorkTaskStatus::Running).unwrap();
        graph.transition(id, WorkTaskStatus::Passed).unwrap();
        assert!(matches!(
            graph.transition(id, WorkTaskStatus::Ready),
            Err(WorkModelError::InvalidTaskTransition { .. })
        ));
    }

    #[test]
    fn repair_retry_is_explicit_and_counted() {
        let (bundle, _, criterion) = bundle();
        let id = WorkTaskId::new();
        let mut graph = TaskGraph {
            revision: 1,
            tasks: vec![task(id, None, criterion)],
        };
        graph.validate(Some(&bundle)).unwrap();
        graph.refresh_ready();
        graph.transition(id, WorkTaskStatus::Running).unwrap();
        graph
            .transition(id, WorkTaskStatus::NeedsAttention)
            .unwrap();
        assert!(matches!(
            graph.transition(id, WorkTaskStatus::Running),
            Err(WorkModelError::InvalidTaskTransition { .. })
        ));
        graph.transition(id, WorkTaskStatus::Ready).unwrap();
        graph.transition(id, WorkTaskStatus::Running).unwrap();
        assert_eq!(graph.task(id).unwrap().retry_count, 1);
    }

    #[test]
    fn evidence_must_reference_the_same_requirement_criterion() {
        let (bundle, requirement_id, criterion_id) = bundle();
        let task_id = WorkTaskId::new();
        let graph = TaskGraph {
            revision: 1,
            tasks: vec![task(task_id, None, criterion_id)],
        };
        let evidence = EvidenceLink {
            id: EvidenceId::new(),
            requirement_id,
            criterion_id,
            task_id,
            action_id: None,
            coverage: EvidenceCoverage::Covered,
            validation_status: Some(ValidationStatus::Passed),
            source: "cargo test".into(),
            summary: "contract test passed".into(),
            digest: "abc123".into(),
            recorded_at: chrono::Utc::now(),
        };
        evidence.validate(&bundle, &graph).unwrap();
    }

    #[test]
    fn evidence_cannot_close_a_criterion_owned_by_another_task() {
        let (mut bundle, requirement_id, first_criterion) = bundle();
        let second_criterion = CriterionId::new();
        bundle.requirements[0]
            .acceptance_criteria
            .push(AcceptanceCriterion {
                id: second_criterion,
                statement: "A decode failure renders Error".into(),
            });
        let task_id = WorkTaskId::new();
        let graph = TaskGraph {
            revision: 1,
            tasks: vec![task(task_id, None, first_criterion)],
        };
        let evidence = EvidenceLink {
            id: EvidenceId::new(),
            requirement_id,
            criterion_id: second_criterion,
            task_id,
            action_id: None,
            coverage: EvidenceCoverage::Covered,
            validation_status: Some(ValidationStatus::Passed),
            source: "cargo test".into(),
            summary: "unrelated contract test passed".into(),
            digest: "abc123".into(),
            recorded_at: chrono::Utc::now(),
        };
        assert!(matches!(
            evidence.validate(&bundle, &graph),
            Err(WorkModelError::TaskDoesNotOwnCriterion { .. })
        ));
    }

    #[test]
    fn covered_evidence_requires_a_real_passing_validation() {
        let (bundle, requirement_id, criterion_id) = bundle();
        let task_id = WorkTaskId::new();
        let graph = TaskGraph {
            revision: 1,
            tasks: vec![task(task_id, None, criterion_id)],
        };
        let evidence = EvidenceLink {
            id: EvidenceId::new(),
            requirement_id,
            criterion_id,
            task_id,
            action_id: None,
            coverage: EvidenceCoverage::Covered,
            validation_status: Some(ValidationStatus::Unavailable),
            source: "missing test runner".into(),
            summary: "the required tool was unavailable".into(),
            digest: "abc123".into(),
            recorded_at: chrono::Utc::now(),
        };
        assert!(matches!(
            evidence.validate(&bundle, &graph),
            Err(WorkModelError::InconsistentEvidence(_))
        ));
    }
}
