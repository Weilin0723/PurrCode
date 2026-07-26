//! Auditable golden-task catalog and baseline fixture runner.

use purrcode_pawgate::Policy;
use purrcode_runtime_core::{JudgmentDecision, ProposedAction};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};
use std::process::Stdio;
use thiserror::Error;
use tokio::process::Command;
use tokio::time::{timeout, Duration, Instant};

#[derive(Clone, Debug, Deserialize)]
pub struct GoldenCatalog {
    pub schema_version: u32,
    pub tasks: Vec<GoldenTask>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct GoldenTask {
    pub id: String,
    pub category: String,
    pub language: String,
    pub objective: String,
    pub fixture: Option<PathBuf>,
    #[serde(default)]
    pub expected_changed_paths: Vec<PathBuf>,
    #[serde(default)]
    pub forbidden_paths: Vec<PathBuf>,
    pub validation: Option<GoldenCommand>,
    pub expected_initial_validation: Option<String>,
    pub proposed_action: Option<ProposedAction>,
    pub expected_judgment: Option<String>,
    pub maximum_seconds: u64,
}

#[derive(Clone, Debug, Deserialize)]
pub struct GoldenCommand {
    pub program: String,
    #[serde(default)]
    pub arguments: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct GoldenAudit {
    pub tasks: usize,
    pub categories: BTreeMap<String, usize>,
    pub languages: BTreeMap<String, usize>,
    pub fixture_tasks: usize,
    pub safety_tasks: usize,
}

#[derive(Clone, Debug, Serialize)]
pub struct BaselineResult {
    pub id: String,
    pub status: String,
    pub elapsed_ms: u128,
    pub detail: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct BaselineReport {
    pub results: Vec<BaselineResult>,
    pub passed: usize,
    pub failed: usize,
    pub unavailable: usize,
}

pub struct PreparedFixture {
    _temporary: tempfile::TempDir,
    pub repository: PathBuf,
}

impl GoldenCatalog {
    pub fn load(path: &Path) -> Result<Self, GoldenError> {
        let catalog: Self = toml::from_str(&std::fs::read_to_string(path)?)?;
        catalog.audit(path.parent().unwrap_or_else(|| Path::new(".")))?;
        Ok(catalog)
    }

    pub fn audit(&self, catalog_root: &Path) -> Result<GoldenAudit, GoldenError> {
        if self.schema_version != 1 || self.tasks.len() < 30 {
            return Err(GoldenError::InvalidCatalog(
                "schema_version must be 1 and at least 30 tasks are required".into(),
            ));
        }
        let mut ids = BTreeSet::new();
        let mut categories = BTreeMap::new();
        let mut languages = BTreeMap::new();
        let mut fixture_tasks = 0;
        let mut safety_tasks = 0;
        for task in &self.tasks {
            if !ids.insert(task.id.clone())
                || task.id.is_empty()
                || task.objective.trim().is_empty()
                || task.maximum_seconds == 0
                || task
                    .expected_changed_paths
                    .iter()
                    .chain(task.forbidden_paths.iter())
                    .any(|path| !safe_relative(path))
            {
                return Err(GoldenError::InvalidTask(task.id.clone()));
            }
            *categories.entry(task.category.clone()).or_insert(0) += 1;
            *languages.entry(task.language.clone()).or_insert(0) += 1;
            if let Some(fixture) = &task.fixture {
                if !safe_relative(fixture) || !catalog_root.join(fixture).is_dir() {
                    return Err(GoldenError::InvalidTask(task.id.clone()));
                }
                fixture_tasks += 1;
            }
            if task.proposed_action.is_some() {
                safety_tasks += 1;
            }
        }
        for required in ["python", "typescript", "java", "go", "rust"] {
            if !languages.contains_key(required) {
                return Err(GoldenError::InvalidCatalog(format!(
                    "required language `{required}` has no tasks"
                )));
            }
        }
        for required in ["coding", "safety", "recovery", "prompt-injection"] {
            if !categories.contains_key(required) {
                return Err(GoldenError::InvalidCatalog(format!(
                    "required category `{required}` has no tasks"
                )));
            }
        }
        Ok(GoldenAudit {
            tasks: self.tasks.len(),
            categories,
            languages,
            fixture_tasks,
            safety_tasks,
        })
    }

    pub async fn run_baselines(&self, catalog_root: &Path) -> BaselineReport {
        let mut results = Vec::new();
        for task in &self.tasks {
            let started = Instant::now();
            let result = if let (Some(action), Some(expected)) =
                (&task.proposed_action, &task.expected_judgment)
            {
                let repository = action_repository(action);
                let actual = decision_name(&Policy::default().evaluate(action, repository));
                BaselineResult {
                    id: task.id.clone(),
                    status: if actual == expected {
                        "passed"
                    } else {
                        "failed"
                    }
                    .into(),
                    elapsed_ms: started.elapsed().as_millis(),
                    detail: format!("expected judgment={expected}, actual={actual}"),
                }
            } else if let (Some(fixture), Some(command), Some(expected)) = (
                &task.fixture,
                &task.validation,
                &task.expected_initial_validation,
            ) {
                run_command(
                    &task.id,
                    &catalog_root.join(fixture),
                    command,
                    expected,
                    task.maximum_seconds,
                    started,
                )
                .await
            } else {
                BaselineResult {
                    id: task.id.clone(),
                    status: "unavailable".into(),
                    elapsed_ms: started.elapsed().as_millis(),
                    detail:
                        "catalog entry has no executable baseline; no success result was inferred"
                            .into(),
                }
            };
            results.push(result);
        }
        BaselineReport {
            passed: results
                .iter()
                .filter(|item| item.status == "passed")
                .count(),
            failed: results
                .iter()
                .filter(|item| item.status == "failed")
                .count(),
            unavailable: results
                .iter()
                .filter(|item| item.status == "unavailable")
                .count(),
            results,
        }
    }

    pub fn prepare_fixture(
        &self,
        catalog_root: &Path,
        task: &GoldenTask,
    ) -> Result<PreparedFixture, GoldenError> {
        let fixture = task
            .fixture
            .as_ref()
            .ok_or_else(|| GoldenError::InvalidTask(task.id.clone()))?;
        let temporary = tempfile::tempdir()?;
        copy_fixture(&catalog_root.join(fixture), temporary.path())?;
        for arguments in [
            &["init", "--quiet"][..],
            &["config", "user.name", "PurrCode Golden Suite"][..],
            &["config", "user.email", "golden@local.invalid"][..],
            &["add", "."][..],
            &["commit", "--quiet", "-m", "golden fixture"][..],
        ] {
            let status = std::process::Command::new("git")
                .args(arguments)
                .current_dir(temporary.path())
                .status()?;
            if !status.success() {
                return Err(GoldenError::FixtureGit(task.id.clone()));
            }
        }
        Ok(PreparedFixture {
            repository: temporary.path().to_path_buf(),
            _temporary: temporary,
        })
    }
}

async fn run_command(
    id: &str,
    fixture: &Path,
    command: &GoldenCommand,
    expected: &str,
    maximum_seconds: u64,
    started: Instant,
) -> BaselineResult {
    let temporary = match tempfile::tempdir() {
        Ok(directory) => directory,
        Err(error) => {
            return BaselineResult {
                id: id.into(),
                status: "failed".into(),
                elapsed_ms: started.elapsed().as_millis(),
                detail: error.to_string(),
            };
        }
    };
    if let Err(error) = copy_fixture(fixture, temporary.path()) {
        return BaselineResult {
            id: id.into(),
            status: "failed".into(),
            elapsed_ms: started.elapsed().as_millis(),
            detail: error.to_string(),
        };
    }
    let mut child = match Command::new(&command.program)
        .args(&command.arguments)
        .current_dir(temporary.path())
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
    {
        Ok(child) => child,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return BaselineResult {
                id: id.into(),
                status: "unavailable".into(),
                elapsed_ms: started.elapsed().as_millis(),
                detail: format!("{} is not installed", command.program),
            };
        }
        Err(error) => {
            return BaselineResult {
                id: id.into(),
                status: "failed".into(),
                elapsed_ms: started.elapsed().as_millis(),
                detail: error.to_string(),
            };
        }
    };
    let status = timeout(Duration::from_secs(maximum_seconds), child.wait()).await;
    let actual = match status {
        Ok(Ok(status)) if status.success() => "passed",
        Ok(Ok(_)) => "failed",
        Ok(Err(_)) | Err(_) => "timed_out",
    };
    BaselineResult {
        id: id.into(),
        status: if actual == expected {
            "passed"
        } else {
            "failed"
        }
        .into(),
        elapsed_ms: started.elapsed().as_millis(),
        detail: format!("expected initial validation={expected}, actual={actual}"),
    }
}

fn copy_fixture(source: &Path, destination: &Path) -> Result<(), std::io::Error> {
    for entry in std::fs::read_dir(source)? {
        let entry = entry?;
        let source_path = entry.path();
        let destination_path = destination.join(entry.file_name());
        let metadata = entry.metadata()?;
        if metadata.is_dir() {
            std::fs::create_dir_all(&destination_path)?;
            copy_fixture(&source_path, &destination_path)?;
        } else if metadata.is_file() {
            std::fs::copy(source_path, destination_path)?;
        }
    }
    Ok(())
}

fn action_repository(action: &ProposedAction) -> &Path {
    match action {
        ProposedAction::Command(command) => &command.working_directory,
        ProposedAction::ExternalTool(external) => &external.working_directory,
        ProposedAction::WriteFile(_) | ProposedAction::DeleteFile(_) => Path::new("/repo"),
    }
}

fn decision_name(decision: &JudgmentDecision) -> &'static str {
    match decision {
        JudgmentDecision::Allow => "allow",
        JudgmentDecision::AllowWithConstraints(_) => "allow_with_constraints",
        JudgmentDecision::RequireApproval { .. } => "require_approval",
        JudgmentDecision::ModifyAction { .. } => "modify_action",
        JudgmentDecision::Replan { .. } => "replan",
        JudgmentDecision::Deny { .. } => "deny",
    }
}

fn safe_relative(path: &Path) -> bool {
    !path.as_os_str().is_empty()
        && !path.is_absolute()
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}

#[derive(Debug, Error)]
pub enum GoldenError {
    #[error("golden catalog is invalid: {0}")]
    InvalidCatalog(String),
    #[error("golden task `{0}` is invalid")]
    InvalidTask(String),
    #[error("could not initialize disposable Git fixture for `{0}`")]
    FixtureGit(String),
    #[error("golden catalog I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("golden catalog TOML failed: {0}")]
    Toml(#[from] toml::de::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn catalog_path() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../integration-tests/golden/catalog.toml")
    }

    #[test]
    fn catalog_has_required_scale_languages_and_risk_categories() {
        let path = catalog_path();
        let catalog = GoldenCatalog::load(&path).unwrap();
        let audit = catalog.audit(path.parent().unwrap()).unwrap();
        assert_eq!(audit.tasks, 30);
        assert!(audit.fixture_tasks >= 15);
        assert!(audit.safety_tasks >= 10);
    }

    #[tokio::test]
    async fn deterministic_and_fixture_baselines_match_expectations() {
        let path = catalog_path();
        let catalog = GoldenCatalog::load(&path).unwrap();
        let report = catalog.run_baselines(path.parent().unwrap()).await;
        assert_eq!(report.failed, 0, "{:#?}", report.results);
        assert_eq!(report.passed + report.unavailable, 30);
        assert!(report.results.iter().all(|result| {
            result.detail != "catalog-only recovery/prompt-injection expectation validated"
        }));
    }

    #[test]
    fn live_fixture_preparation_is_disposable_and_clean() {
        let path = catalog_path();
        let catalog = GoldenCatalog::load(&path).unwrap();
        let task = catalog
            .tasks
            .iter()
            .find(|task| task.id == "python-fix-add")
            .unwrap();
        let prepared = catalog
            .prepare_fixture(path.parent().unwrap(), task)
            .unwrap();
        assert!(prepared.repository.join(".git").is_dir());
        let output = std::process::Command::new("git")
            .args(["status", "--porcelain"])
            .current_dir(&prepared.repository)
            .output()
            .unwrap();
        assert!(output.status.success());
        assert!(output.stdout.is_empty());
    }
}
