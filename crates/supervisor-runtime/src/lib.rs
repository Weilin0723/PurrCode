//! Bounded supervisor for independent workers in separate Git worktrees.

use async_trait::async_trait;
use futures::future::join_all;
use purrcode_repository_engine::{
    RepositoryEngine, RepositoryError, SessionWorktree, WorktreeEffects,
};
use purrcode_runtime_core::SessionId;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use thiserror::Error;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ParallelismConfig {
    pub max_workers: usize,
    pub max_model_requests: usize,
    pub max_worktrees: usize,
    pub require_isolation: bool,
}

impl Default for ParallelismConfig {
    fn default() -> Self {
        Self {
            max_workers: 3,
            max_model_requests: 6,
            max_worktrees: 4,
            require_isolation: true,
        }
    }
}

impl ParallelismConfig {
    pub fn validate(&self) -> Result<(), SupervisorError> {
        if self.max_workers == 0
            || self.max_model_requests == 0
            || self.max_worktrees == 0
            || self.max_workers > self.max_worktrees
            || !self.require_isolation
        {
            return Err(SupervisorError::UnsafeConfiguration);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct WorkerSpec {
    pub id: String,
    pub objective: String,
    pub dependencies: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkerWorkspace {
    pub worker_id: String,
    pub path: PathBuf,
    pub base_head: String,
    pub model_request_budget: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkerOutput {
    pub summary: String,
    pub model_requests: usize,
}

#[async_trait]
pub trait IsolatedWorker: Send + Sync {
    async fn execute(
        &self,
        spec: &WorkerSpec,
        workspace: &WorkerWorkspace,
    ) -> Result<WorkerOutput, String>;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WorkerStatus {
    Completed,
    Failed(String),
    SkippedDependency(String),
}

#[derive(Clone, Debug)]
pub struct WorkerResult {
    pub spec: WorkerSpec,
    pub worktree: Option<SessionWorktree>,
    pub effects: Option<WorktreeEffects>,
    pub output: Option<WorkerOutput>,
    pub status: WorkerStatus,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PathConflict {
    pub path: PathBuf,
    pub workers: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MergeDecision {
    IndependentReviewRequired,
    ConflictsRequireResolution(Vec<PathConflict>),
}

#[derive(Clone, Debug)]
pub struct SupervisorReport {
    pub results: Vec<WorkerResult>,
    pub model_requests: usize,
    pub merge_decision: MergeDecision,
}

pub struct Supervisor {
    config: ParallelismConfig,
}

impl Supervisor {
    pub fn new(config: ParallelismConfig) -> Result<Self, SupervisorError> {
        config.validate()?;
        Ok(Self { config })
    }

    pub async fn run(
        &self,
        repository: &Path,
        specs: Vec<WorkerSpec>,
        worker: &dyn IsolatedWorker,
    ) -> Result<SupervisorReport, SupervisorError> {
        validate_graph(&specs)?;
        let mut pending: BTreeMap<_, _> = specs
            .into_iter()
            .map(|spec| (spec.id.clone(), spec))
            .collect();
        let mut finished = BTreeMap::<String, WorkerStatus>::new();
        let mut results = Vec::new();
        let mut model_requests = 0_usize;
        while !pending.is_empty() {
            let failed_dependencies: Vec<_> = pending
                .values()
                .filter_map(|spec| {
                    spec.dependencies
                        .iter()
                        .find(|dependency| {
                            matches!(
                                finished.get(*dependency),
                                Some(WorkerStatus::Failed(_) | WorkerStatus::SkippedDependency(_))
                            )
                        })
                        .map(|dependency| (spec.id.clone(), dependency.clone()))
                })
                .collect();
            for (id, dependency) in failed_dependencies {
                let spec = pending
                    .remove(&id)
                    .ok_or(SupervisorError::InvalidDependencyGraph)?;
                let status = WorkerStatus::SkippedDependency(dependency);
                finished.insert(id, status.clone());
                results.push(WorkerResult {
                    spec,
                    worktree: None,
                    effects: None,
                    output: None,
                    status,
                });
            }
            let eligible: Vec<_> = pending
                .values()
                .filter(|spec| {
                    spec.dependencies.iter().all(|dependency| {
                        matches!(finished.get(dependency), Some(WorkerStatus::Completed))
                    })
                })
                .take(
                    self.config.max_workers.min(self.config.max_worktrees).min(
                        self.config
                            .max_model_requests
                            .saturating_sub(model_requests),
                    ),
                )
                .map(|spec| spec.id.clone())
                .collect();
            if eligible.is_empty() {
                if model_requests >= self.config.max_model_requests {
                    return Err(SupervisorError::ModelRequestBudgetExhausted);
                }
                return Err(SupervisorError::InvalidDependencyGraph);
            }
            let wave: Vec<_> = eligible
                .into_iter()
                .map(|id| {
                    pending
                        .remove(&id)
                        .ok_or(SupervisorError::InvalidDependencyGraph)
                })
                .collect::<Result<_, _>>()?;
            let tasks = wave.into_iter().map(|spec| async move {
                let worktree =
                    RepositoryEngine::create_worktree(repository, SessionId::new()).await?;
                let workspace = WorkerWorkspace {
                    worker_id: spec.id.clone(),
                    path: worktree.path.clone(),
                    base_head: worktree.base_head.clone(),
                    model_request_budget: 1,
                };
                let execution = worker.execute(&spec, &workspace).await;
                let effects = RepositoryEngine::effects(&worktree).await?;
                Ok::<_, SupervisorError>((spec, worktree, effects, execution))
            });
            for result in join_all(tasks).await {
                let (spec, worktree, effects, execution) = result?;
                let (status, output) = match execution {
                    Ok(output)
                        if output.model_requests <= 1
                            && model_requests.saturating_add(output.model_requests)
                                <= self.config.max_model_requests =>
                    {
                        model_requests += output.model_requests;
                        (WorkerStatus::Completed, Some(output))
                    }
                    Ok(_) => (
                        WorkerStatus::Failed("model request budget exceeded".into()),
                        None,
                    ),
                    Err(error) => (WorkerStatus::Failed(error), None),
                };
                finished.insert(spec.id.clone(), status.clone());
                results.push(WorkerResult {
                    spec,
                    worktree: Some(worktree),
                    effects: Some(effects),
                    output,
                    status,
                });
            }
        }
        let conflicts = find_conflicts(&results);
        let merge_decision = if conflicts.is_empty() {
            MergeDecision::IndependentReviewRequired
        } else {
            MergeDecision::ConflictsRequireResolution(conflicts)
        };
        Ok(SupervisorReport {
            results,
            model_requests,
            merge_decision,
        })
    }
}

fn validate_graph(specs: &[WorkerSpec]) -> Result<(), SupervisorError> {
    let ids: BTreeSet<_> = specs.iter().map(|spec| spec.id.as_str()).collect();
    if ids.len() != specs.len()
        || specs.iter().any(|spec| {
            spec.id.is_empty()
                || spec
                    .dependencies
                    .iter()
                    .any(|dependency| dependency == &spec.id || !ids.contains(dependency.as_str()))
        })
    {
        return Err(SupervisorError::InvalidDependencyGraph);
    }
    Ok(())
}

fn find_conflicts(results: &[WorkerResult]) -> Vec<PathConflict> {
    let mut paths = BTreeMap::<PathBuf, Vec<String>>::new();
    for result in results
        .iter()
        .filter(|result| result.status == WorkerStatus::Completed)
    {
        if let Some(effects) = &result.effects {
            for path in &effects.changed_files {
                paths
                    .entry(path.clone())
                    .or_default()
                    .push(result.spec.id.clone());
            }
        }
    }
    paths
        .into_iter()
        .filter(|(_, workers)| workers.len() > 1)
        .map(|(path, workers)| PathConflict { path, workers })
        .collect()
}

#[derive(Debug, Error)]
pub enum SupervisorError {
    #[error("parallelism configuration would violate isolation or resource limits")]
    UnsafeConfiguration,
    #[error("worker dependency graph is invalid or cyclic")]
    InvalidDependencyGraph,
    #[error("supervisor model-request budget was exhausted")]
    ModelRequestBudgetExhausted,
    #[error("repository isolation failed: {0}")]
    Repository(#[from] RepositoryError),
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FixtureWorker;

    #[async_trait]
    impl IsolatedWorker for FixtureWorker {
        async fn execute(
            &self,
            spec: &WorkerSpec,
            workspace: &WorkerWorkspace,
        ) -> Result<WorkerOutput, String> {
            std::fs::write(
                workspace.path.join(format!("{}.txt", spec.id)),
                &spec.objective,
            )
            .map_err(|error| error.to_string())?;
            Ok(WorkerOutput {
                summary: "fixture completed".into(),
                model_requests: 1,
            })
        }
    }

    #[tokio::test]
    async fn workers_use_distinct_worktrees_and_cannot_self_merge() {
        let repository = tempfile::tempdir().unwrap();
        git(repository.path(), &["init"]);
        git(
            repository.path(),
            &["config", "user.email", "fixture@example.com"],
        );
        git(repository.path(), &["config", "user.name", "Fixture"]);
        std::fs::write(repository.path().join("README.md"), "base").unwrap();
        git(repository.path(), &["add", "README.md"]);
        git(repository.path(), &["commit", "-m", "base"]);
        let supervisor = Supervisor::new(ParallelismConfig::default()).unwrap();
        let report = supervisor
            .run(
                repository.path(),
                vec![
                    WorkerSpec {
                        id: "one".into(),
                        objective: "first".into(),
                        dependencies: Vec::new(),
                    },
                    WorkerSpec {
                        id: "two".into(),
                        objective: "second".into(),
                        dependencies: Vec::new(),
                    },
                ],
                &FixtureWorker,
            )
            .await
            .unwrap();
        let paths: BTreeSet<_> = report
            .results
            .iter()
            .filter_map(|result| result.worktree.as_ref().map(|worktree| &worktree.path))
            .collect();
        assert_eq!(paths.len(), 2);
        assert_eq!(
            report.merge_decision,
            MergeDecision::IndependentReviewRequired
        );
        assert!(!repository.path().join("one.txt").exists());
        assert!(!repository.path().join("two.txt").exists());
    }

    #[test]
    fn overlapping_worker_effects_require_conflict_resolution() {
        let result = |id: &str| WorkerResult {
            spec: WorkerSpec {
                id: id.into(),
                objective: String::new(),
                dependencies: Vec::new(),
            },
            worktree: None,
            effects: Some(WorktreeEffects {
                status_porcelain: String::new(),
                changed_files: vec![PathBuf::from("src/shared.rs")],
                binary_patch: Vec::new(),
            }),
            output: None,
            status: WorkerStatus::Completed,
        };
        let conflicts = find_conflicts(&[result("one"), result("two")]);
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].workers, vec!["one", "two"]);
    }

    fn git(repository: &Path, arguments: &[&str]) {
        let status = std::process::Command::new("git")
            .args(arguments)
            .current_dir(repository)
            .status()
            .unwrap();
        assert!(status.success());
    }
}
