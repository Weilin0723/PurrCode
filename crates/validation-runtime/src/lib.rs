//! Repository validation discovery and evidence reporting.

use chrono::Utc;
use purrcode_claw::{ExecutionError, ToolRuntime};
use purrcode_ninelives::{SessionStore, StoreError};
use purrcode_runtime_core::{
    ActionConstraints, ActionId, ApprovalAuthority, Authorization, CommandAction, JudgmentDecision,
    ProposedAction, SessionEvent, SessionId, ValidationStatus,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use thiserror::Error;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValidationStage {
    Format,
    Lint,
    TypeCheck,
    TargetedTests,
    IntegrationTests,
    Build,
    DiffReview,
    Security,
    CompletionCriteria,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ValidationCommand {
    pub stage: ValidationStage,
    pub program: String,
    pub arguments: Vec<String>,
    pub environment: BTreeMap<String, String>,
    #[serde(default)]
    pub working_directory: Option<PathBuf>,
    pub reason: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceStatus {
    Passed,
    Failed,
    SkippedByConfiguration,
    Unavailable,
    NotDetected,
    TimedOut,
    Uncertain,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ValidationEvidence {
    pub stage: ValidationStage,
    pub status: EvidenceStatus,
    pub command: Option<ValidationCommand>,
    pub exit_code: Option<i32>,
    pub detail: String,
    pub output_truncated: bool,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct ValidationPlan {
    pub commands: Vec<ValidationCommand>,
    pub undetected_stages: Vec<ValidationStage>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ValidationReport {
    pub evidence: Vec<ValidationEvidence>,
}

impl ValidationReport {
    pub fn completion_allowed(&self) -> bool {
        !self.evidence.is_empty()
            && self.evidence.iter().all(|evidence| {
                matches!(
                    evidence.status,
                    EvidenceStatus::Passed
                        | EvidenceStatus::SkippedByConfiguration
                        | EvidenceStatus::Unavailable
                        | EvidenceStatus::NotDetected
                )
            })
    }
}

pub struct ValidationRunner;

impl ValidationRunner {
    pub async fn run(
        store: &mut SessionStore,
        session_id: SessionId,
        repository: &Path,
        plan: &ValidationPlan,
    ) -> Result<ValidationReport, ValidationError> {
        let mut evidence = missing_evidence(plan);
        for item in &evidence {
            store.append(
                session_id,
                &SessionEvent::ValidationRecorded {
                    action_id: ActionId::new(),
                    status: ValidationStatus::NotDetected,
                    evidence: format!("{:?}: {}", item.stage, item.detail),
                },
            )?;
        }
        for command in &plan.commands {
            let working_directory = match &command.working_directory {
                Some(relative)
                    if !relative.as_os_str().is_empty()
                        && !relative.is_absolute()
                        && relative.components().all(|component| {
                            matches!(component, std::path::Component::Normal(_))
                        }) =>
                {
                    repository.join(relative)
                }
                Some(_) => {
                    return Err(ValidationError::UnsafeWorkingDirectory(
                        command.working_directory.clone().unwrap_or_default(),
                    ));
                }
                None => repository.to_path_buf(),
            };
            let action_id = ActionId::new();
            let action = ProposedAction::Command(CommandAction {
                program: command.program.clone().into(),
                arguments: command.arguments.clone(),
                working_directory: working_directory.clone(),
                environment: command.environment.clone(),
            });
            let constraints = ActionConstraints {
                working_directory,
                network: false,
                timeout_seconds: 900,
                maximum_output_bytes: 4 * 1024 * 1024,
                allowed_write_globs: generated_write_globs(&command.program),
                maximum_changed_files: 100_000,
            };
            store.append(
                session_id,
                &SessionEvent::ActionProposed {
                    action_id,
                    action: action.clone(),
                },
            )?;
            store.append(
                session_id,
                &SessionEvent::JudgmentRecorded {
                    action_id,
                    decision: JudgmentDecision::AllowWithConstraints(constraints.clone()),
                },
            )?;
            store.authorize(&Authorization {
                action_id,
                session_id,
                action_digest: action.digest(&constraints)?,
                constraints: constraints.clone(),
                authorized_at: Utc::now(),
                approved_by: ApprovalAuthority::SignedPolicy {
                    policy_id: "purrcode-validation-v1".into(),
                },
            })?;
            store.append(session_id, &SessionEvent::ExecutionStarted { action_id })?;
            let result = ToolRuntime::execute(store, action_id, &action, &constraints).await;
            let item = match result {
                Ok(result) => {
                    let status = if result.exit_code == Some(0) {
                        EvidenceStatus::Passed
                    } else {
                        EvidenceStatus::Failed
                    };
                    store.append(
                        session_id,
                        &SessionEvent::ExecutionFinished {
                            action_id,
                            exit_code: result.exit_code,
                            truncated: result.truncated,
                            sandbox_level: Some(format!("{:?}", result.sandbox_level)),
                            sandbox_backend: Some(result.sandbox_backend.clone()),
                        },
                    )?;
                    ValidationEvidence {
                        stage: command.stage,
                        status,
                        command: Some(command.clone()),
                        exit_code: result.exit_code,
                        detail: combined_output(&result.stdout, &result.stderr),
                        output_truncated: result.truncated,
                    }
                }
                Err(ExecutionError::Timeout) => ValidationEvidence {
                    stage: command.stage,
                    status: EvidenceStatus::TimedOut,
                    command: Some(command.clone()),
                    exit_code: None,
                    detail: "validation command exceeded 900 seconds".into(),
                    output_truncated: false,
                },
                Err(ExecutionError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
                    ValidationEvidence {
                        stage: command.stage,
                        status: EvidenceStatus::Unavailable,
                        command: Some(command.clone()),
                        exit_code: None,
                        detail: format!("validation program is unavailable: {error}"),
                        output_truncated: false,
                    }
                }
                Err(error) => ValidationEvidence {
                    stage: command.stage,
                    status: EvidenceStatus::Uncertain,
                    command: Some(command.clone()),
                    exit_code: None,
                    detail: error.to_string(),
                    output_truncated: false,
                },
            };
            store.append(
                session_id,
                &SessionEvent::ValidationRecorded {
                    action_id,
                    status: to_domain_status(&item.status),
                    evidence: serde_json::to_string(&item)?,
                },
            )?;
            evidence.push(item);
        }
        let completion = ValidationEvidence {
            stage: ValidationStage::CompletionCriteria,
            status: if evidence.iter().any(|item| {
                matches!(
                    item.status,
                    EvidenceStatus::Failed | EvidenceStatus::TimedOut | EvidenceStatus::Uncertain
                )
            }) {
                EvidenceStatus::Failed
            } else {
                EvidenceStatus::Passed
            },
            command: None,
            exit_code: None,
            detail: "completion gate derived from all recorded validation evidence".into(),
            output_truncated: false,
        };
        store.append(
            session_id,
            &SessionEvent::ValidationRecorded {
                action_id: ActionId::new(),
                status: to_domain_status(&completion.status),
                evidence: serde_json::to_string(&completion)?,
            },
        )?;
        evidence.push(completion);
        Ok(ValidationReport { evidence })
    }
}

fn generated_write_globs(program: &str) -> Vec<String> {
    match program {
        "cargo" => vec!["target/**".into()],
        "go" => vec!["**/*.test".into()],
        "npm" | "pnpm" => vec!["dist/**".into(), "build/**".into(), "coverage/**".into()],
        "./gradlew" | "mvn" => vec!["**/build/**".into(), "**/target/**".into()],
        _ => Vec::new(),
    }
}

fn combined_output(stdout: &[u8], stderr: &[u8]) -> String {
    let mut output = String::from_utf8_lossy(stdout).into_owned();
    if !stderr.is_empty() {
        if !output.is_empty() {
            output.push('\n');
        }
        output.push_str(&String::from_utf8_lossy(stderr));
    }
    output
}

fn to_domain_status(status: &EvidenceStatus) -> ValidationStatus {
    match status {
        EvidenceStatus::Passed => ValidationStatus::Passed,
        EvidenceStatus::Failed => ValidationStatus::Failed,
        EvidenceStatus::SkippedByConfiguration => ValidationStatus::SkippedByConfiguration,
        EvidenceStatus::Unavailable => ValidationStatus::Unavailable,
        EvidenceStatus::NotDetected => ValidationStatus::NotDetected,
        EvidenceStatus::TimedOut => ValidationStatus::TimedOut,
        EvidenceStatus::Uncertain => ValidationStatus::Uncertain,
    }
}

pub struct ValidationDetector;

impl ValidationDetector {
    pub fn detect(repository: &Path) -> Result<ValidationPlan, ValidationError> {
        let repository = repository.canonicalize()?;
        let mut commands = Vec::new();
        if repository.join("Cargo.toml").is_file() {
            commands.extend(rust_commands());
        }
        if repository.join("go.mod").is_file() {
            commands.extend(go_commands());
        }
        if repository.join("package.json").is_file() {
            commands.extend(node_commands(&repository)?);
        }
        if repository.join("pyproject.toml").is_file()
            || repository.join("pytest.ini").is_file()
            || repository.join("requirements.txt").is_file()
        {
            commands.extend(python_commands(&repository));
        }
        if repository.join("gradlew").is_file() {
            commands.extend(gradle_commands());
        } else if repository.join("pom.xml").is_file() {
            commands.extend(maven_commands());
        }
        if repository.join("compose.yaml").is_file()
            || repository.join("compose.yml").is_file()
            || repository.join("docker-compose.yaml").is_file()
            || repository.join("docker-compose.yml").is_file()
        {
            commands.push(command(
                ValidationStage::TypeCheck,
                "docker",
                &["compose", "config", "--quiet"],
                "Docker Compose configuration detected",
            ));
        }
        for project in nested_project_roots(&repository)? {
            let relative = project
                .strip_prefix(&repository)
                .map_err(|_| ValidationError::UnsafeWorkingDirectory(project.clone()))?
                .to_path_buf();
            let mut nested = if project.join("Cargo.toml").is_file()
                && !repository.join("Cargo.toml").is_file()
            {
                rust_commands()
            } else if project.join("go.mod").is_file() {
                go_commands()
            } else if project.join("package.json").is_file() {
                node_commands(&project)?
            } else if project.join("pyproject.toml").is_file()
                || project.join("pytest.ini").is_file()
                || project.join("requirements.txt").is_file()
            {
                python_commands(&project)
            } else if project.join("gradlew").is_file() {
                gradle_commands()
            } else if project.join("pom.xml").is_file() {
                maven_commands()
            } else {
                Vec::new()
            };
            for command in &mut nested {
                command.working_directory = Some(relative.clone());
                command.reason = format!("{} in {}", command.reason, relative.display());
            }
            commands.extend(nested);
        }
        let represented: std::collections::BTreeSet<_> =
            commands.iter().map(|command| command.stage).collect();
        let undetected_stages = [
            ValidationStage::Format,
            ValidationStage::Lint,
            ValidationStage::TypeCheck,
            ValidationStage::TargetedTests,
            ValidationStage::IntegrationTests,
            ValidationStage::Build,
            ValidationStage::Security,
        ]
        .into_iter()
        .filter(|stage| !represented.contains(stage))
        .collect();
        Ok(ValidationPlan {
            commands,
            undetected_stages,
        })
    }
}

fn nested_project_roots(repository: &Path) -> Result<Vec<PathBuf>, ValidationError> {
    const MAX_DEPTH: usize = 5;
    const MAX_PROJECTS: usize = 100;
    fn visit(
        directory: &Path,
        depth: usize,
        projects: &mut Vec<PathBuf>,
    ) -> Result<(), ValidationError> {
        if depth > MAX_DEPTH || projects.len() >= MAX_PROJECTS {
            return Ok(());
        }
        for entry in std::fs::read_dir(directory)? {
            let path = entry?.path();
            if !path.is_dir() {
                continue;
            }
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default();
            if matches!(
                name,
                ".git"
                    | ".purrcode"
                    | "target"
                    | "node_modules"
                    | "vendor"
                    | "dist"
                    | "build"
                    | ".gradle"
            ) {
                continue;
            }
            if [
                "Cargo.toml",
                "go.mod",
                "package.json",
                "pyproject.toml",
                "pytest.ini",
                "requirements.txt",
                "gradlew",
                "pom.xml",
            ]
            .iter()
            .any(|manifest| path.join(manifest).is_file())
            {
                projects.push(path.clone());
                if projects.len() >= MAX_PROJECTS {
                    break;
                }
            }
            visit(&path, depth + 1, projects)?;
        }
        Ok(())
    }
    let mut projects = Vec::new();
    visit(repository, 1, &mut projects)?;
    projects.sort();
    projects.dedup();
    Ok(projects)
}

fn command(
    stage: ValidationStage,
    program: &str,
    arguments: &[&str],
    reason: &str,
) -> ValidationCommand {
    ValidationCommand {
        stage,
        program: program.into(),
        arguments: arguments.iter().map(|value| (*value).into()).collect(),
        environment: BTreeMap::new(),
        working_directory: None,
        reason: reason.into(),
    }
}

fn rust_commands() -> Vec<ValidationCommand> {
    let mut environment = BTreeMap::new();
    environment.insert("CARGO_NET_OFFLINE".into(), "true".into());
    [
        (
            ValidationStage::Format,
            "cargo",
            vec!["fmt", "--all", "--", "--check"],
            "Cargo workspace detected",
        ),
        (
            ValidationStage::Lint,
            "cargo",
            vec![
                "clippy",
                "--workspace",
                "--all-targets",
                "--",
                "-D",
                "warnings",
            ],
            "Cargo workspace detected",
        ),
        (
            ValidationStage::TargetedTests,
            "cargo",
            vec!["test", "--workspace"],
            "Cargo workspace detected",
        ),
        (
            ValidationStage::Build,
            "cargo",
            vec!["build", "--workspace"],
            "Cargo workspace detected",
        ),
    ]
    .into_iter()
    .map(|(stage, program, arguments, reason)| {
        let mut result = command(stage, program, &arguments, reason);
        result.environment = environment.clone();
        result
    })
    .collect()
}

fn go_commands() -> Vec<ValidationCommand> {
    vec![
        command(
            ValidationStage::TargetedTests,
            "go",
            &["test", "./..."],
            "Go module detected",
        ),
        command(
            ValidationStage::Build,
            "go",
            &["build", "./..."],
            "Go module detected",
        ),
    ]
}

fn node_commands(repository: &Path) -> Result<Vec<ValidationCommand>, ValidationError> {
    let package: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(repository.join("package.json"))?)?;
    let scripts = package["scripts"].as_object();
    let manager = if repository.join("pnpm-lock.yaml").is_file() {
        "pnpm"
    } else {
        "npm"
    };
    let mut commands = Vec::new();
    for (script, stage) in [
        ("format:check", ValidationStage::Format),
        ("lint", ValidationStage::Lint),
        ("typecheck", ValidationStage::TypeCheck),
        ("test", ValidationStage::TargetedTests),
        ("build", ValidationStage::Build),
    ] {
        if scripts.is_some_and(|scripts| scripts.contains_key(script)) {
            let arguments = vec!["--offline", "run", script];
            commands.push(command(
                stage,
                manager,
                &arguments,
                &format!("package.json script `{script}` detected"),
            ));
        }
    }
    Ok(commands)
}

fn python_commands(repository: &Path) -> Vec<ValidationCommand> {
    let mut commands = Vec::new();
    if repository.join("pyproject.toml").is_file() {
        commands.push(command(
            ValidationStage::Lint,
            "ruff",
            &["check", "."],
            "Python project detected; availability checked at execution",
        ));
        commands.push(command(
            ValidationStage::Format,
            "ruff",
            &["format", "--check", "."],
            "Python project detected; availability checked at execution",
        ));
    }
    commands.push(command(
        ValidationStage::TargetedTests,
        "python3",
        &["-m", "pytest"],
        "Python test configuration detected",
    ));
    commands
}

fn gradle_commands() -> Vec<ValidationCommand> {
    vec![
        command(
            ValidationStage::TargetedTests,
            "./gradlew",
            &["--offline", "test"],
            "Gradle wrapper detected",
        ),
        command(
            ValidationStage::Build,
            "./gradlew",
            &["--offline", "build"],
            "Gradle wrapper detected",
        ),
    ]
}

fn maven_commands() -> Vec<ValidationCommand> {
    vec![command(
        ValidationStage::IntegrationTests,
        "mvn",
        &["--offline", "verify"],
        "Maven project detected",
    )]
}

pub fn missing_evidence(plan: &ValidationPlan) -> Vec<ValidationEvidence> {
    plan.undetected_stages
        .iter()
        .map(|stage| ValidationEvidence {
            stage: *stage,
            status: EvidenceStatus::NotDetected,
            command: None,
            exit_code: None,
            detail: "no repository validation command was detected for this stage".into(),
            output_truncated: false,
        })
        .collect()
}

#[derive(Debug, Error)]
pub enum ValidationError {
    #[error("repository inspection failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("package.json is invalid: {0}")]
    PackageJson(#[from] serde_json::Error),
    #[error("session storage failed: {0}")]
    Store(#[from] StoreError),
    #[error("domain operation failed: {0}")]
    Domain(#[from] purrcode_runtime_core::DomainError),
    #[error("repository path is invalid: {0}")]
    InvalidRepository(PathBuf),
    #[error("validation working directory is unsafe: {0}")]
    UnsafeWorkingDirectory(PathBuf),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn monorepo_and_docker_validation_preserve_project_working_directories() {
        let repository = tempfile::tempdir().unwrap();
        std::fs::write(repository.path().join("compose.yaml"), "services: {}\n").unwrap();
        let python = repository.path().join("services/python-api");
        let kotlin = repository.path().join("services/kotlin-api");
        std::fs::create_dir_all(&python).unwrap();
        std::fs::create_dir_all(&kotlin).unwrap();
        std::fs::write(python.join("pyproject.toml"), "[project]\nname='fixture'\n").unwrap();
        std::fs::write(kotlin.join("gradlew"), "#!/bin/sh\n").unwrap();
        std::fs::write(kotlin.join("build.gradle.kts"), "plugins {}\n").unwrap();
        let plan = ValidationDetector::detect(repository.path()).unwrap();
        assert!(plan.commands.iter().any(|command| {
            command.program == "docker"
                && command.arguments == ["compose", "config", "--quiet"]
                && command.working_directory.is_none()
        }));
        assert!(plan.commands.iter().any(|command| {
            command.program == "python3"
                && command.working_directory == Some(PathBuf::from("services/python-api"))
        }));
        assert!(plan.commands.iter().any(|command| {
            command.program == "./gradlew"
                && command.working_directory == Some(PathBuf::from("services/kotlin-api"))
        }));
    }

    #[test]
    fn rust_plan_is_offline_and_does_not_invent_integration_tests() {
        let repository = tempfile::tempdir().unwrap();
        std::fs::write(repository.path().join("Cargo.toml"), "[workspace]").unwrap();
        let plan = ValidationDetector::detect(repository.path()).unwrap();
        assert!(plan
            .commands
            .iter()
            .all(|command| command.environment.get("CARGO_NET_OFFLINE") == Some(&"true".into())));
        assert!(plan
            .undetected_stages
            .contains(&ValidationStage::IntegrationTests));
    }

    #[test]
    fn empty_repository_reports_not_detected_instead_of_passed() {
        let repository = tempfile::tempdir().unwrap();
        let plan = ValidationDetector::detect(repository.path()).unwrap();
        let evidence = missing_evidence(&plan);
        assert!(!evidence.is_empty());
        assert!(evidence
            .iter()
            .all(|item| item.status == EvidenceStatus::NotDetected));
    }

    #[tokio::test]
    async fn runner_records_real_exit_evidence_before_completion_gate() {
        let repository = tempfile::tempdir().unwrap();
        let plan = ValidationPlan {
            commands: vec![command(
                ValidationStage::Build,
                "git",
                &["--version"],
                "portable smoke command",
            )],
            undetected_stages: vec![ValidationStage::TargetedTests],
        };
        let mut store = SessionStore::in_memory().unwrap();
        let session_id = SessionId::new();
        let report = ValidationRunner::run(&mut store, session_id, repository.path(), &plan)
            .await
            .unwrap();
        assert!(report.completion_allowed());
        assert!(report
            .evidence
            .iter()
            .any(|item| item.stage == ValidationStage::Build
                && item.status == EvidenceStatus::Passed));
        assert!(store
            .events(session_id)
            .unwrap()
            .iter()
            .any(|event| matches!(
                event,
                SessionEvent::ValidationRecorded {
                    status: ValidationStatus::Passed,
                    ..
                }
            )));
    }
}
