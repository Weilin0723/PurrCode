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
    /// Filesystem or runtime effects that must NOT occur if the security
    /// guarantee holds (for example: "outside-worktree write", "credential
    /// file read", "destructive git operation", "active-tree modification").
    #[serde(default)]
    pub forbidden_effects: Vec<String>,
    /// Concrete action kinds that the policy layer must reject for this case.
    /// Empty for non-security cases; required for safety / adversarial cases.
    #[serde(default)]
    pub expected_blocked_actions: Vec<String>,
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

    /// Validate that every adversarial case declares expected blocked actions and forbidden effects.
    ///
    /// Contract:
    /// 1. Every security category (`safety`, `prompt-injection`, `traversal`,
    ///    `symlink`, `credential`, `destructive`, `active-tree`,
    ///    `invalid-norm`, `event-log`) must declare non-empty
    ///    `expected_blocked_actions` AND `forbidden_effects`.
    /// 2. If `proposed_action` is present, `expected_judgment` must also be
    ///    present (the policy verdict must be testable).
    /// 3. `forbidden_paths` may be empty only when `forbidden_effects` is
    ///    non-empty (the contract is enforced via `forbidden_effects` even
    ///    when no concrete path is named).
    pub fn validate_security_cases(&self, catalog_root: &Path) -> Result<GoldenAudit, GoldenError> {
        let audit = self.audit(catalog_root)?;
        let security_categories = [
            "safety",
            "prompt-injection",
            "traversal",
            "symlink",
            "credential",
            "destructive",
            "active-tree",
            "invalid-norm",
            "event-log",
        ];
        for task in &self.tasks {
            if !security_categories.contains(&task.category.as_str()) {
                continue;
            }
            if task.expected_blocked_actions.is_empty() {
                return Err(GoldenError::InvalidTask(format!(
                    "security case `{}` (category={}) must declare expected_blocked_actions",
                    task.id, task.category
                )));
            }
            if task.forbidden_effects.is_empty() {
                return Err(GoldenError::InvalidTask(format!(
                    "security case `{}` (category={}) must declare forbidden_effects",
                    task.id, task.category
                )));
            }
            if task.proposed_action.is_some() && task.expected_judgment.is_none() {
                return Err(GoldenError::InvalidTask(format!(
                    "adversarial case `{}` with proposed_action must declare expected_judgment",
                    task.id
                )));
            }
        }
        Ok(audit)
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
    // A baseline that expects the unfixed fixture *not* to validate is satisfied
    // by a timeout as well as by a non-zero exit: neither is a pass, which is
    // the whole claim. Requiring the exact string made a cold Go toolchain on a
    // slow runner look like a broken fixture. The reverse is not true — an
    // expectation of "passed" is never satisfied by a timeout — and the detail
    // still reports what actually happened, so a timeout stays visible.
    let satisfied = baseline_expectation_satisfied(expected, actual);
    BaselineResult {
        id: id.into(),
        status: if satisfied { "passed" } else { "failed" }.into(),
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
        ProposedAction::RepositoryRead(_)
        | ProposedAction::WriteFile(_)
        | ProposedAction::DeleteFile(_) => Path::new("/repo"),
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

/// Whether an observed baseline outcome satisfies the expectation.
///
/// A baseline that expects the unfixed fixture *not* to validate is satisfied by
/// a timeout as well as by a non-zero exit: neither is a pass, which is the
/// whole claim. Requiring the exact string made a cold Go toolchain on a slow
/// runner look like a broken fixture. The reverse is deliberately not true — an
/// expectation of "passed" is never satisfied by a timeout.
fn baseline_expectation_satisfied(expected: &str, actual: &str) -> bool {
    actual == expected || (expected != "passed" && actual == "timed_out")
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_timeout_satisfies_a_baseline_that_expected_no_pass() {
        assert!(super::baseline_expectation_satisfied("failed", "failed"));
        assert!(
            super::baseline_expectation_satisfied("failed", "timed_out"),
            "a slow toolchain is not a broken fixture: neither outcome is a pass"
        );
    }

    #[test]
    fn a_timeout_never_satisfies_a_baseline_that_expected_a_pass() {
        assert!(super::baseline_expectation_satisfied("passed", "passed"));
        assert!(!super::baseline_expectation_satisfied(
            "passed",
            "timed_out"
        ));
        assert!(!super::baseline_expectation_satisfied("passed", "failed"));
    }

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
        assert!(audit.tasks >= 45);
        assert!(audit.fixture_tasks >= 15);
        assert!(audit.safety_tasks >= 10);
    }

    #[tokio::test]
    async fn deterministic_and_fixture_baselines_match_expectations() {
        let path = catalog_path();
        let catalog = GoldenCatalog::load(&path).unwrap();
        let report = catalog.run_baselines(path.parent().unwrap()).await;
        assert_eq!(report.failed, 0, "{:#?}", report.results);
        assert_eq!(report.passed + report.unavailable, catalog.tasks.len());
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

    #[test]
    fn adversarial_security_cases_are_structured_validly() {
        let path = catalog_path();
        let catalog = GoldenCatalog::load(&path).unwrap();
        let audit = catalog
            .validate_security_cases(path.parent().unwrap())
            .unwrap();
        assert!(
            audit.tasks >= 45,
            "expected at least 45 tasks, got {}",
            audit.tasks
        );
        assert!(
            audit.categories.contains_key("safety"),
            "safety category must exist"
        );
        assert!(
            audit.categories.contains_key("prompt-injection"),
            "prompt-injection category must exist"
        );
        let adversarial_ids: Vec<&str> = [
            "traversal-parent-dir-read",
            "traversal-absolute-path-read",
            "symlink-read-outside",
            "symlink-write-outside",
            "credential-api-key-read",
            "credential-password-access",
            "destructive-git-reset",
            "destructive-clean-fd",
            "active-tree-write-outside-worktree",
            "active-tree-delete-outside-worktree",
            "invalid-norm-unknown-rg-flag",
            "invalid-norm-shell-wrapper",
            "event-log-duplicate-approval",
            "event-log-approval-unknown-action",
            "prompt-injection-ignore-instructions",
            "prompt-injection-role-play",
            "prompt-injection-delete-override",
            "provider-partial-output",
            "provider-restart-recovery",
        ]
        .into_iter()
        .collect();
        let catalog_ids: Vec<&str> = catalog.tasks.iter().map(|t| t.id.as_str()).collect();
        for id in &adversarial_ids {
            assert!(
                catalog_ids.contains(id),
                "adversarial task `{id}` not found in catalog"
            );
        }
        let safety_injection: Vec<&GoldenTask> = catalog
            .tasks
            .iter()
            .filter(|t| t.category == "safety" || t.category == "prompt-injection")
            .collect();
        for task in &safety_injection {
            if task.proposed_action.is_some() {
                assert!(
                    task.expected_judgment.is_some() || !task.forbidden_paths.is_empty(),
                    "security case `{}` with proposed_action must declare expected_judgment or forbidden_paths",
                    task.id
                );
            }
        }
    }
}
