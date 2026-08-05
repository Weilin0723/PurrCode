//! Client-neutral projection of the durable requirement/task/evidence model.

use std::collections::BTreeMap;

use purrcode_runtime_core::work::{EvidenceCoverage, WorkRisk, WorkTaskStatus};
use purrcode_runtime_core::{SessionEvent, SessionState};
use purrcode_ui_contracts::{
    CoverageStatusView, CriterionCoverageView, DesignDecisionView, EvidenceView, RequirementView,
    SpecBundleView, TaskGraphView, TaskStatusView, TaskView,
};

pub(crate) fn task_graph_view(
    state: &SessionState,
    events: &[SessionEvent],
) -> Option<TaskGraphView> {
    let graph = state.task_graph.as_ref()?;
    let current_activity = events.iter().fold(
        BTreeMap::new(),
        |mut activity: BTreeMap<_, String>, event| {
            if let SessionEvent::TaskStatusChanged {
                task_id, reason, ..
            } = event
            {
                activity.insert(*task_id, reason.clone());
            }
            activity
        },
    );
    let evidence_by_task = state.evidence_links.iter().fold(
        BTreeMap::new(),
        |mut summaries: BTreeMap<_, String>, evidence| {
            summaries.insert(evidence.task_id, evidence.summary.clone());
            summaries
        },
    );
    let tasks = graph
        .tasks
        .iter()
        .map(|task| TaskView {
            id: task.id.0.to_string(),
            objective: task.objective.clone(),
            dependencies: task
                .dependencies
                .iter()
                .map(|dependency| dependency.0.to_string())
                .collect(),
            required: task.priority == purrcode_runtime_core::work::WorkPriority::Required,
            risk: risk_name(task.risk).into(),
            owner: task.owner.clone(),
            status: task_status(task.status),
            acceptance_criteria: task
                .acceptance_criteria
                .iter()
                .map(|criterion| criterion.0.to_string())
                .collect(),
            current_activity: current_activity.get(&task.id).cloned(),
            evidence_summary: evidence_by_task.get(&task.id).cloned(),
            retry_count: task.retry_count,
        })
        .collect::<Vec<_>>();
    let count = |status| tasks.iter().filter(|task| task.status == status).count();
    Some(TaskGraphView {
        revision: graph.revision,
        current_task: tasks
            .iter()
            .find(|task| task.status == TaskStatusView::Running)
            .map(|task| task.id.clone()),
        passed: count(TaskStatusView::Passed),
        running: count(TaskStatusView::Running),
        ready: count(TaskStatusView::Ready),
        blocked: count(TaskStatusView::Blocked) + count(TaskStatusView::NeedsAttention),
        remaining: tasks
            .iter()
            .filter(|task| {
                !matches!(
                    task.status,
                    TaskStatusView::Passed | TaskStatusView::Skipped | TaskStatusView::Cancelled
                )
            })
            .count(),
        tasks,
    })
}

pub(crate) fn spec_bundle_view(state: &SessionState) -> Option<SpecBundleView> {
    let bundle = state.spec_bundle.as_ref()?;
    let latest = latest_coverage(state);
    let requirements = bundle
        .requirements
        .iter()
        .map(|requirement| RequirementView {
            id: requirement.id.0.to_string(),
            statement: requirement.statement.clone(),
            required: requirement.priority == purrcode_runtime_core::work::WorkPriority::Required,
            criteria: requirement
                .acceptance_criteria
                .iter()
                .map(|criterion| {
                    let matching = state
                        .evidence_links
                        .iter()
                        .filter(|evidence| evidence.criterion_id == criterion.id)
                        .collect::<Vec<_>>();
                    CriterionCoverageView {
                        id: criterion.id.0.to_string(),
                        statement: criterion.statement.clone(),
                        status: latest
                            .get(&criterion.id)
                            .copied()
                            .unwrap_or(CoverageStatusView::NotRun),
                        task_ids: matching
                            .iter()
                            .map(|evidence| evidence.task_id.0.to_string())
                            .collect(),
                        evidence_ids: matching
                            .iter()
                            .map(|evidence| evidence.id.0.to_string())
                            .collect(),
                        summary: matching.last().map(|evidence| evidence.summary.clone()),
                    }
                })
                .collect(),
        })
        .collect::<Vec<_>>();
    let required_criteria = requirements
        .iter()
        .filter(|requirement| requirement.required)
        .flat_map(|requirement| &requirement.criteria)
        .collect::<Vec<_>>();
    let covered_criteria = required_criteria
        .iter()
        .filter(|criterion| {
            matches!(
                criterion.status,
                CoverageStatusView::Covered | CoverageStatusView::AcceptedException
            )
        })
        .count();
    let total_required_criteria = required_criteria.len();
    Some(SpecBundleView {
        revision: bundle.revision,
        kind: spec_kind_name(bundle.kind).into(),
        title: bundle.title.clone(),
        requirements,
        non_goals: bundle.non_goals.clone(),
        design_decisions: bundle
            .design_decisions
            .iter()
            .map(|decision| DesignDecisionView {
                id: decision.id.0.to_string(),
                title: decision.title.clone(),
                decision: decision.decision.clone(),
                rationale: decision.rationale.clone(),
                affected_components: decision.affected_components.clone(),
            })
            .collect(),
        covered_criteria,
        total_required_criteria,
    })
}

pub(crate) fn evidence_views(state: &SessionState) -> Vec<EvidenceView> {
    state
        .evidence_links
        .iter()
        .map(|evidence| EvidenceView {
            id: evidence.id.0.to_string(),
            requirement_id: evidence.requirement_id.0.to_string(),
            criterion_id: evidence.criterion_id.0.to_string(),
            task_id: evidence.task_id.0.to_string(),
            status: coverage_status(evidence.coverage),
            source: evidence.source.clone(),
            summary: evidence.summary.clone(),
            digest: evidence.digest.clone(),
            recorded_at: evidence.recorded_at.to_rfc3339(),
        })
        .collect()
}

fn latest_coverage(
    state: &SessionState,
) -> BTreeMap<purrcode_runtime_core::work::CriterionId, CoverageStatusView> {
    state
        .evidence_links
        .iter()
        .fold(BTreeMap::new(), |mut latest, evidence| {
            latest.insert(evidence.criterion_id, coverage_status(evidence.coverage));
            latest
        })
}

fn task_status(status: WorkTaskStatus) -> TaskStatusView {
    match status {
        WorkTaskStatus::Pending => TaskStatusView::Pending,
        WorkTaskStatus::Ready => TaskStatusView::Ready,
        WorkTaskStatus::Running => TaskStatusView::Running,
        WorkTaskStatus::Blocked => TaskStatusView::Blocked,
        WorkTaskStatus::NeedsAttention => TaskStatusView::NeedsAttention,
        WorkTaskStatus::Passed => TaskStatusView::Passed,
        WorkTaskStatus::Failed => TaskStatusView::Failed,
        WorkTaskStatus::Skipped => TaskStatusView::Skipped,
        WorkTaskStatus::Cancelled => TaskStatusView::Cancelled,
    }
}

fn coverage_status(status: EvidenceCoverage) -> CoverageStatusView {
    match status {
        EvidenceCoverage::Covered => CoverageStatusView::Covered,
        EvidenceCoverage::Failed => CoverageStatusView::Failed,
        EvidenceCoverage::NotRun => CoverageStatusView::NotRun,
        EvidenceCoverage::Unavailable => CoverageStatusView::Unavailable,
        EvidenceCoverage::NotApplicable => CoverageStatusView::NotApplicable,
        EvidenceCoverage::Stale => CoverageStatusView::Stale,
        EvidenceCoverage::AcceptedException => CoverageStatusView::AcceptedException,
    }
}

fn risk_name(risk: WorkRisk) -> &'static str {
    match risk {
        WorkRisk::Low => "low",
        WorkRisk::Medium => "medium",
        WorkRisk::High => "high",
        WorkRisk::Critical => "critical",
    }
}

fn spec_kind_name(kind: purrcode_runtime_core::work::SpecKind) -> &'static str {
    use purrcode_runtime_core::work::SpecKind;
    match kind {
        SpecKind::Direct => "direct",
        SpecKind::FeatureRequirementsFirst => "feature_requirements_first",
        SpecKind::FeatureDesignFirst => "feature_design_first",
        SpecKind::Bugfix => "bugfix",
        SpecKind::Quick => "quick",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use purrcode_runtime_core::work::{
        AcceptanceCriterion, CriterionId, Requirement, RequirementId, SpecBundle, SpecKind,
        TaskGraph, WorkPriority, WorkTask, WorkTaskId,
    };

    #[test]
    fn projection_never_counts_ready_work_as_complete() {
        let criterion = CriterionId::new();
        let task = WorkTaskId::new();
        let mut state = SessionState::empty(purrcode_runtime_core::SessionId::new());
        state.spec_bundle = Some(SpecBundle {
            revision: 1,
            kind: SpecKind::Direct,
            title: "Do the work".into(),
            requirements: vec![Requirement {
                id: RequirementId::new(),
                statement: "The work is done".into(),
                priority: WorkPriority::Required,
                acceptance_criteria: vec![AcceptanceCriterion {
                    id: criterion,
                    statement: "A real check passes".into(),
                }],
            }],
            non_goals: vec![],
            design_decisions: vec![],
        });
        state.task_graph = Some(TaskGraph {
            revision: 1,
            tasks: vec![WorkTask {
                id: task,
                objective: "Implement it".into(),
                dependencies: vec![],
                priority: WorkPriority::Required,
                risk: WorkRisk::Medium,
                acceptance_criteria: vec![criterion],
                scope: vec![],
                owner: None,
                status: WorkTaskStatus::Ready,
                retry_count: 0,
                evidence_obligations: vec![],
            }],
        });

        let tasks = task_graph_view(&state, &[]).unwrap();
        let spec = spec_bundle_view(&state).unwrap();
        assert_eq!(tasks.remaining, 1);
        assert_eq!(tasks.passed, 0);
        assert_eq!(spec.covered_criteria, 0);
        assert_eq!(spec.total_required_criteria, 1);
    }
}
