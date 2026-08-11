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
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use thiserror::Error;
use tokio::sync::Mutex;
use tokio::sync::mpsc;

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
    /// An optional `provider/model` override for this worker. When set, the
    /// worker runs on that model instead of the supervisor's shared default,
    /// letting different sub-agents use different APIs.
    #[serde(default)]
    pub model: Option<String>,
}

#[derive(Clone, Debug)]
pub struct WorkerWorkspace {
    pub worker_id: String,
    pub path: PathBuf,
    pub base_head: String,
    pub model_request_budget: usize,
    /// Cooperative cancellation for this worker. A worker implementation can
    /// poll it to stop mid-execution when the user requests it.
    pub cancellation: WorkerCancellation,
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

/// Lifecycle transition for a worker, emitted to an observer so a client can
/// render a live worker tree without polling.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WorkerEvent {
    /// A worker became eligible and its isolated worktree was created.
    Started {
        worker_id: String,
        worktree: PathBuf,
    },
    /// A worker produced a terminal result.
    Finished {
        worker_id: String,
        status: WorkerStatus,
        changed_paths: Vec<PathBuf>,
        summary: Option<String>,
    },
}

/// Cooperative cancellation for a single worker. Set from the daemon when the
/// user asks to stop one worker; the worker's execution loop observes it.
#[derive(Clone, Debug, Default)]
pub struct WorkerCancellation {
    inner: Arc<WorkerCancellationInner>,
}

#[derive(Debug, Default)]
struct WorkerCancellationInner {
    cancelled: AtomicBool,
}

impl WorkerCancellation {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.inner.cancelled.store(true, Ordering::Release);
    }

    pub fn is_cancelled(&self) -> bool {
        self.inner.cancelled.load(Ordering::Acquire)
    }
}

/// Live progress for an in-flight supervisor run. Worker cancellations are
/// keyed by worker id so the daemon can stop an individual worker.
#[derive(Clone, Default)]
pub struct SupervisorRunState {
    pub cancellations: Arc<Mutex<BTreeMap<String, WorkerCancellation>>>,
}

impl SupervisorRunState {
    pub async fn cancel_worker(&self, worker_id: &str) -> bool {
        let cancellations = self.cancellations.lock().await;
        match cancellations.get(worker_id) {
            Some(cancellation) => {
                cancellation.cancel();
                true
            }
            None => false,
        }
    }
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
        let (sender, mut receiver) = mpsc::channel::<WorkerEvent>(self.config.max_workers);
        let state = SupervisorRunState::default();
        // The legacy `run` caller does not consume events, so drain them on a
        // separate task to keep the bounded channel from blocking the run.
        let drain = tokio::spawn(async move { while receiver.recv().await.is_some() {} });
        let report = self
            .run_with_events(repository, specs, worker, &state, &sender)
            .await;
        drop(sender);
        let _ = drain.await;
        report
    }

    /// Runs the supervisor, emitting worker lifecycle events on the channel
    /// and consulting per-worker cancellation. The daemon runs this in the
    /// background so a client can observe and stop individual workers.
    pub async fn run_with_events(
        &self,
        repository: &Path,
        specs: Vec<WorkerSpec>,
        worker: &dyn IsolatedWorker,
        run_state: &SupervisorRunState,
        events: &mpsc::Sender<WorkerEvent>,
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
                finished.insert(id.clone(), status.clone());
                let _ = events
                    .send(WorkerEvent::Finished {
                        worker_id: id,
                        status: status.clone(),
                        changed_paths: Vec::new(),
                        summary: None,
                    })
                    .await;
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
            for spec in &wave {
                let cancellation = WorkerCancellation::new();
                run_state
                    .cancellations
                    .lock()
                    .await
                    .insert(spec.id.clone(), cancellation);
            }
            let wave_cancellations = run_state.cancellations.clone();
            let wave_sender = events.clone();
            let tasks = wave.into_iter().map(|spec| {
                let spec_id = spec.id.clone();
                let cancellations = wave_cancellations.clone();
                let sender = wave_sender.clone();
                async move {
                    let worktree =
                        RepositoryEngine::create_worktree(repository, SessionId::new()).await?;
                    let _ = sender
                        .send(WorkerEvent::Started {
                            worker_id: spec.id.clone(),
                            worktree: worktree.path.clone(),
                        })
                        .await;
                    let workspace = {
                        let cancellations = cancellations.lock().await;
                        let cancellation = cancellations
                            .get(&spec_id)
                            .cloned()
                            .unwrap_or_else(WorkerCancellation::new);
                        WorkerWorkspace {
                            worker_id: spec.id.clone(),
                            path: worktree.path.clone(),
                            base_head: worktree.base_head.clone(),
                            model_request_budget: 1,
                            cancellation,
                        }
                    };
                    let cancelled = workspace.cancellation.is_cancelled();
                    let execution = if cancelled {
                        Err(format!("worker `{spec_id}` was stopped by the user"))
                    } else {
                        worker.execute(&spec, &workspace).await
                    };
                    let effects = RepositoryEngine::effects(&worktree).await?;
                    Ok::<_, SupervisorError>((spec, worktree, effects, execution))
                }
            });
            let mut wave_results = Vec::new();
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
                let changed_paths = effects.changed_files.clone();
                let summary = output.as_ref().map(|output| output.summary.clone());
                let _ = events
                    .send(WorkerEvent::Finished {
                        worker_id: spec.id.clone(),
                        status: status.clone(),
                        changed_paths: changed_paths.clone(),
                        summary: summary.clone(),
                    })
                    .await;
                finished.insert(spec.id.clone(), status.clone());
                wave_results.push(WorkerResult {
                    spec,
                    worktree: Some(worktree),
                    effects: Some(effects),
                    output,
                    status,
                });
            }
            results.extend(wave_results);
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
                        model: None,
                    },
                    WorkerSpec {
                        id: "two".into(),
                        objective: "second".into(),
                        dependencies: Vec::new(),
                        model: None,
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
                model: None,
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

    #[test]
    fn worker_spec_carries_an_optional_per_worker_model() {
        // The `model` field is optional so older requests deserialize, but a
        // set value is carried through construction for per-worker routing.
        let with_model = WorkerSpec {
            id: "w".into(),
            objective: "o".into(),
            dependencies: Vec::new(),
            model: Some("deepseek/deepseek-pro".into()),
        };
        assert_eq!(with_model.model.as_deref(), Some("deepseek/deepseek-pro"));
        let without_model = WorkerSpec {
            id: "w".into(),
            objective: "o".into(),
            dependencies: Vec::new(),
            model: None,
        };
        assert_eq!(without_model.model, None);
    }

    struct SlowWorker;

    #[async_trait]
    impl IsolatedWorker for SlowWorker {
        async fn execute(
            &self,
            _spec: &WorkerSpec,
            workspace: &WorkerWorkspace,
        ) -> Result<WorkerOutput, String> {
            // Cooperative: poll the shared cancellation while working so a
            // mid-flight stop is observed, mirroring how the daemon worker
            // cancels an in-flight model turn.
            for _ in 0..10 {
                if workspace.cancellation.is_cancelled() {
                    return Err("worker was stopped by the user".into());
                }
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
            std::fs::write(workspace.path.join("done.txt"), "done").map_err(|e| e.to_string())?;
            Ok(WorkerOutput {
                summary: "slow worker completed".into(),
                model_requests: 1,
            })
        }
    }

    #[tokio::test]
    async fn cancelling_a_worker_stops_it_without_its_worktree() {
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

        let supervisor = Supervisor::new(ParallelismConfig {
            max_workers: 1,
            max_model_requests: 1,
            max_worktrees: 1,
            require_isolation: true,
        })
        .unwrap();
        let state = SupervisorRunState::default();
        let (sender, mut receiver) = mpsc::channel::<WorkerEvent>(8);

        // Start the run, wait for the worker to begin, then cancel it.
        let task = tokio::spawn({
            let state = state.clone();
            let sender = sender.clone();
            let repository = repository.path().to_path_buf();
            async move {
                supervisor
                    .run_with_events(
                        &repository,
                        vec![WorkerSpec {
                            id: "slow".into(),
                            objective: "go slow".into(),
                            dependencies: Vec::new(),
                            model: None,
                        }],
                        &SlowWorker,
                        &state,
                        &sender,
                    )
                    .await
            }
        });
        drop(sender);
        let started = tokio::time::timeout(std::time::Duration::from_secs(2), receiver.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(started, WorkerEvent::Started { worker_id, .. } if worker_id == "slow"));
        assert!(state.cancel_worker("slow").await);
        let report = task.await.unwrap().unwrap();
        assert!(matches!(report.results[0].status, WorkerStatus::Failed(_)));
    }
}
