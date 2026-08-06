//! Repository validation discovery and evidence reporting.

use chrono::Utc;
use purrcode_claw::{ExecutionError, ToolRuntime};
use purrcode_ninelives::{SessionStore, StoreError};
use purrcode_runtime_core::{
    ActionConstraints, ActionId, ApprovalAuthority, Authorization, CommandAction, JudgmentDecision,
    ProposedAction, SessionEvent, SessionId, ValidationStatus,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use thiserror::Error;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValidationStage {
    SyntaxStatic,
    FocusedTests,
    ModuleTests,
    FullUnitTests,
    Packaging,
    ProductionSmoke,
    /// Legacy evidence stages retained for v0.7 bundle compatibility.
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
    /// Stages the detected project must satisfy before completion.
    #[serde(default)]
    pub required_stages: BTreeSet<ValidationStage>,
    /// Explicit blueprint acceptance for required stages that cannot run.
    #[serde(default)]
    pub accepted_unavailable_stages: BTreeSet<ValidationStage>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ValidationReport {
    pub evidence: Vec<ValidationEvidence>,
    #[serde(default)]
    pub required_stages: BTreeSet<ValidationStage>,
    #[serde(default)]
    pub accepted_unavailable_stages: BTreeSet<ValidationStage>,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureClass {
    Compilation,
    Dependency,
    TestAssertion,
    Configuration,
    Environment,
    Compatibility,
    DatabaseMigration,
    Network,
    Timeout,
    ResourceExhaustion,
    Infrastructure,
    Unknown,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RepairRoute {
    pub class: FailureClass,
    pub specialist_role: String,
    pub focused_stage: ValidationStage,
    pub evidence_excerpt: String,
}

impl ValidationReport {
    pub fn completion_allowed(&self) -> bool {
        self.evidence.iter().any(|evidence| {
            evidence.stage == ValidationStage::CompletionCriteria
                && evidence.status == EvidenceStatus::Passed
        }) && self.required_stages.iter().all(|stage| {
            self.evidence.iter().any(|evidence| {
                evidence.stage == *stage
                    && (evidence.status == EvidenceStatus::Passed
                        || (self.accepted_unavailable_stages.contains(stage)
                            && matches!(
                                evidence.status,
                                EvidenceStatus::Unavailable
                                    | EvidenceStatus::NotDetected
                                    | EvidenceStatus::SkippedByConfiguration
                            )))
            })
        })
    }

    pub fn repair_routes(&self) -> Vec<RepairRoute> {
        self.evidence
            .iter()
            .filter(|evidence| {
                matches!(
                    evidence.status,
                    EvidenceStatus::Failed | EvidenceStatus::TimedOut | EvidenceStatus::Uncertain
                )
            })
            .map(classify_failure)
            .collect()
    }
}

pub fn classify_failure(evidence: &ValidationEvidence) -> RepairRoute {
    let lower = evidence.detail.to_ascii_lowercase();
    let class = if evidence.status == EvidenceStatus::TimedOut {
        FailureClass::Timeout
    } else if contains_any(
        &lower,
        &[
            "out of memory",
            "cannot allocate memory",
            "enospc",
            "no space left",
            "resource exhausted",
        ],
    ) {
        FailureClass::ResourceExhaustion
    } else if contains_any(
        &lower,
        &[
            "migration failed",
            "flyway",
            "liquibase",
            "schema migration",
            "database migration",
        ],
    ) {
        FailureClass::DatabaseMigration
    } else if contains_any(
        &lower,
        &[
            "assertion failed",
            "assertionerror",
            "expected:",
            "expected ",
            "test failed",
        ],
    ) {
        FailureClass::TestAssertion
    } else if contains_any(
        &lower,
        &[
            "could not resolve",
            "dependency",
            "package not found",
            "no matching package",
            "module not found",
        ],
    ) {
        FailureClass::Dependency
    } else if contains_any(
        &lower,
        &[
            "compile error",
            "compilation failed",
            "cannot find symbol",
            "mismatched types",
            "syntax error",
        ],
    ) {
        FailureClass::Compilation
    } else if contains_any(
        &lower,
        &[
            "unsupported class file",
            "incompatible",
            "requires java",
            "version conflict",
            "abi",
        ],
    ) {
        FailureClass::Compatibility
    } else if contains_any(
        &lower,
        &[
            "connection refused",
            "network is unreachable",
            "dns",
            "tls",
            "timed out connecting",
        ],
    ) {
        FailureClass::Network
    } else if contains_any(
        &lower,
        &[
            "command not found",
            "no such file or directory",
            "toolchain",
            "jdk_home",
            "java_home",
        ],
    ) {
        FailureClass::Environment
    } else if contains_any(
        &lower,
        &[
            "invalid configuration",
            "configuration error",
            "parse config",
            "invalid yaml",
            "invalid toml",
        ],
    ) {
        FailureClass::Configuration
    } else if contains_any(
        &lower,
        &[
            "docker daemon",
            "runner unavailable",
            "service unavailable",
            "infrastructure",
        ],
    ) {
        FailureClass::Infrastructure
    } else {
        FailureClass::Unknown
    };
    let specialist_role = match class {
        FailureClass::Compilation | FailureClass::Compatibility => "Build Repair Agent",
        FailureClass::Dependency => "Dependency Agent",
        FailureClass::TestAssertion => "Test Repair Agent",
        FailureClass::Configuration => "Configuration Agent",
        FailureClass::Environment => "Environment Agent",
        FailureClass::DatabaseMigration => "Database Migration Agent",
        FailureClass::Network | FailureClass::Infrastructure => "Infrastructure Agent",
        FailureClass::Timeout | FailureClass::ResourceExhaustion => "Runtime Reliability Agent",
        FailureClass::Unknown => "Diagnostic Agent",
    };
    RepairRoute {
        class,
        specialist_role: specialist_role.into(),
        focused_stage: evidence.stage,
        evidence_excerpt: evidence.detail.chars().take(2048).collect(),
    }
}

fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| haystack.contains(needle))
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
            let mut environment = command.environment.clone();
            environment.insert(
                "HOME".into(),
                working_directory.to_string_lossy().into_owned(),
            );
            if matches!(command.program.as_str(), "npm" | "pnpm" | "yarn" | "bun") {
                environment.insert("NPM_CONFIG_UPDATE_NOTIFIER".into(), "false".into());
                environment.insert("NO_UPDATE_NOTIFIER".into(), "1".into());
            }
            let action = ProposedAction::Command(CommandAction {
                program: command.program.clone().into(),
                arguments: command.arguments.clone(),
                working_directory: working_directory.clone(),
                environment,
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
                    // Validation-repair actions run outside `run_until_pause`'s
                    // main turn loop, so there is no TurnId to stamp them with
                    // (PRD v1.1 §6.3).
                    turn_id: None,
                },
            )?;
            store.append(
                session_id,
                &SessionEvent::JudgmentRecorded {
                    action_id,
                    decision: JudgmentDecision::AllowWithConstraints(constraints.clone()),
                    turn_id: None,
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
        let required_passed = plan.required_stages.iter().all(|stage| {
            evidence.iter().any(|item| {
                item.stage == *stage
                    && (item.status == EvidenceStatus::Passed
                        || (plan.accepted_unavailable_stages.contains(stage)
                            && matches!(
                                item.status,
                                EvidenceStatus::Unavailable
                                    | EvidenceStatus::NotDetected
                                    | EvidenceStatus::SkippedByConfiguration
                            )))
            })
        });
        let completion = ValidationEvidence {
            stage: ValidationStage::CompletionCriteria,
            status: if required_passed {
                EvidenceStatus::Passed
            } else {
                EvidenceStatus::Failed
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
        Ok(ValidationReport {
            evidence,
            required_stages: plan.required_stages.clone(),
            accepted_unavailable_stages: plan.accepted_unavailable_stages.clone(),
        })
    }
}

fn generated_write_globs(program: &str) -> Vec<String> {
    match program {
        "cargo" => vec!["target/**".into()],
        "go" => vec!["**/*.test".into()],
        "npm" | "pnpm" | "yarn" | "bun" => vec![
            "dist/**".into(),
            "build/**".into(),
            "coverage/**".into(),
            ".npm/**".into(),
        ],
        "./gradlew" | "gradle" | "mvn" | "make" | "cmake" => {
            vec!["**/build/**".into(), "**/target/**".into()]
        }
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
            || is_regular_directory(&repository.join("tests"))
        {
            commands.extend(python_commands(&repository));
        }
        if has_dotnet_project(&repository)? {
            commands.extend(dotnet_commands());
        }
        if repository.join("gradlew").is_file() {
            commands.extend(gradle_commands());
        } else if repository.join("build.gradle").is_file()
            || repository.join("build.gradle.kts").is_file()
        {
            commands.extend(system_gradle_commands());
        } else if repository.join("pom.xml").is_file() {
            commands.extend(maven_commands());
        }
        commands.extend(make_and_cmake_commands(&repository)?);
        if repository.join("compose.yaml").is_file()
            || repository.join("compose.yml").is_file()
            || repository.join("docker-compose.yaml").is_file()
            || repository.join("docker-compose.yml").is_file()
        {
            commands.push(command(
                ValidationStage::SyntaxStatic,
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
                || is_regular_directory(&project.join("tests"))
            {
                python_commands(&project)
            } else if has_dotnet_project(&project)? {
                dotnet_commands()
            } else if project.join("gradlew").is_file() {
                gradle_commands()
            } else if project.join("pom.xml").is_file() {
                maven_commands()
            } else {
                Vec::new()
            };
            for command in &mut nested {
                if command.stage == ValidationStage::FullUnitTests {
                    command.stage = ValidationStage::ModuleTests;
                }
                command.working_directory = Some(relative.clone());
                command.reason = format!("{} in {}", command.reason, relative.display());
            }
            commands.extend(nested);
        }
        commands.sort_by_key(|command| progressive_rank(command.stage));
        let represented: BTreeSet<_> = commands.iter().map(|command| command.stage).collect();
        let undetected_stages = [
            ValidationStage::SyntaxStatic,
            ValidationStage::FocusedTests,
            ValidationStage::ModuleTests,
            ValidationStage::FullUnitTests,
            ValidationStage::IntegrationTests,
            ValidationStage::Packaging,
            ValidationStage::ProductionSmoke,
            ValidationStage::Security,
        ]
        .into_iter()
        .filter(|stage| !represented.contains(stage))
        .collect();
        Ok(ValidationPlan {
            commands,
            undetected_stages,
            required_stages: represented
                .into_iter()
                .filter(|stage| {
                    matches!(
                        stage,
                        ValidationStage::FocusedTests
                            | ValidationStage::ModuleTests
                            | ValidationStage::FullUnitTests
                            | ValidationStage::IntegrationTests
                            | ValidationStage::Packaging
                            | ValidationStage::ProductionSmoke
                    )
                })
                .collect(),
            accepted_unavailable_stages: BTreeSet::new(),
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
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let path = entry.path();
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
                || has_dotnet_project(&path)?
                || is_regular_directory(&path.join("tests"))
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
            ValidationStage::SyntaxStatic,
            "cargo",
            vec!["fmt", "--all", "--", "--check"],
            "Cargo workspace detected",
        ),
        (
            ValidationStage::SyntaxStatic,
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
            ValidationStage::FullUnitTests,
            "cargo",
            vec!["test", "--workspace"],
            "Cargo workspace detected",
        ),
        (
            ValidationStage::Packaging,
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
            ValidationStage::FullUnitTests,
            "go",
            &["test", "./..."],
            "Go module detected",
        ),
        command(
            ValidationStage::Packaging,
            "go",
            &["build", "./..."],
            "Go module detected",
        ),
    ]
}

fn node_commands(repository: &Path) -> Result<Vec<ValidationCommand>, ValidationError> {
    let package_path = repository.join("package.json");
    let metadata = fs::symlink_metadata(&package_path)?;
    if !metadata.file_type().is_file() || metadata.len() > 1024 * 1024 {
        return Err(ValidationError::UnsafeManifest(package_path));
    }
    let package: serde_json::Value = serde_json::from_str(&fs::read_to_string(package_path)?)?;
    let scripts = package["scripts"].as_object();
    let declared_manager = package["packageManager"]
        .as_str()
        .and_then(|value| value.split('@').next());
    let manager = if repository.join("pnpm-lock.yaml").is_file() || declared_manager == Some("pnpm")
    {
        "pnpm"
    } else if repository.join("yarn.lock").is_file() || declared_manager == Some("yarn") {
        "yarn"
    } else if repository.join("bun.lock").is_file()
        || repository.join("bun.lockb").is_file()
        || declared_manager == Some("bun")
    {
        "bun"
    } else {
        "npm"
    };
    let mut commands = Vec::new();
    for (script, stage) in [
        ("format:check", ValidationStage::SyntaxStatic),
        ("lint", ValidationStage::SyntaxStatic),
        ("typecheck", ValidationStage::SyntaxStatic),
        ("test", ValidationStage::FullUnitTests),
        ("test:integration", ValidationStage::IntegrationTests),
        ("ci", ValidationStage::IntegrationTests),
        ("test:smoke", ValidationStage::ProductionSmoke),
        ("smoke", ValidationStage::ProductionSmoke),
        ("build", ValidationStage::Packaging),
    ] {
        if scripts.is_some_and(|scripts| scripts.contains_key(script)) {
            let arguments = match manager {
                "npm" | "pnpm" => vec!["--offline", "run", script],
                "yarn" => vec!["--offline", script],
                "bun" => vec!["run", script],
                _ => unreachable!("manager is selected from a closed set"),
            };
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
            ValidationStage::SyntaxStatic,
            "ruff",
            &["check", "."],
            "Python project detected; availability checked at execution",
        ));
        commands.push(command(
            ValidationStage::SyntaxStatic,
            "ruff",
            &["format", "--check", "."],
            "Python project detected; availability checked at execution",
        ));
    }
    if repository.join("pytest.ini").is_file()
        || repository.join("pyproject.toml").is_file()
        || repository.join("requirements.txt").is_file()
    {
        commands.push(command(
            ValidationStage::FullUnitTests,
            "python3",
            &["-m", "pytest"],
            "Python pytest configuration detected",
        ));
    } else if is_regular_directory(&repository.join("tests")) {
        commands.push(command(
            ValidationStage::FullUnitTests,
            "python3",
            &["-m", "unittest", "discover", "-s", "tests"],
            "Python unittest directory detected",
        ));
    }
    commands
}

fn is_regular_directory(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .map(|metadata| metadata.file_type().is_dir())
        .unwrap_or(false)
}

fn has_dotnet_project(repository: &Path) -> Result<bool, ValidationError> {
    for entry in fs::read_dir(repository)? {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        let path = entry.path();
        let extension = path.extension().and_then(|value| value.to_str());
        if matches!(extension, Some("sln" | "csproj" | "fsproj" | "vbproj")) {
            return Ok(true);
        }
    }
    Ok(false)
}

fn dotnet_commands() -> Vec<ValidationCommand> {
    vec![
        command(
            ValidationStage::FullUnitTests,
            "dotnet",
            &["test", "--no-restore"],
            ".NET project detected",
        ),
        command(
            ValidationStage::Packaging,
            "dotnet",
            &["build", "--no-restore"],
            ".NET project detected",
        ),
    ]
}

fn progressive_rank(stage: ValidationStage) -> u8 {
    match stage {
        ValidationStage::SyntaxStatic
        | ValidationStage::Format
        | ValidationStage::Lint
        | ValidationStage::TypeCheck => 0,
        ValidationStage::FocusedTests | ValidationStage::TargetedTests => 1,
        ValidationStage::ModuleTests => 2,
        ValidationStage::FullUnitTests => 3,
        ValidationStage::IntegrationTests => 4,
        ValidationStage::Packaging | ValidationStage::Build => 5,
        ValidationStage::ProductionSmoke => 6,
        ValidationStage::DiffReview | ValidationStage::Security => 6,
        ValidationStage::CompletionCriteria => 7,
    }
}

fn gradle_commands() -> Vec<ValidationCommand> {
    vec![
        command(
            ValidationStage::FullUnitTests,
            "./gradlew",
            &["--offline", "test"],
            "Gradle wrapper detected",
        ),
        command(
            ValidationStage::Packaging,
            "./gradlew",
            &["--offline", "build"],
            "Gradle wrapper detected",
        ),
    ]
}

fn system_gradle_commands() -> Vec<ValidationCommand> {
    vec![
        command(
            ValidationStage::FullUnitTests,
            "gradle",
            &["--offline", "test"],
            "Gradle build detected; wrapper unavailable",
        ),
        command(
            ValidationStage::Packaging,
            "gradle",
            &["--offline", "build"],
            "Gradle build detected; wrapper unavailable",
        ),
    ]
}

fn make_and_cmake_commands(repository: &Path) -> Result<Vec<ValidationCommand>, ValidationError> {
    let mut commands = Vec::new();
    let makefile = ["Makefile", "makefile"]
        .into_iter()
        .map(|name| repository.join(name))
        .find(|path| path.is_file());
    if let Some(makefile) = makefile {
        let metadata = fs::symlink_metadata(&makefile)?;
        if !metadata.file_type().is_file() || metadata.len() > 1024 * 1024 {
            return Err(ValidationError::UnsafeManifest(makefile));
        }
        let source = fs::read_to_string(makefile)?;
        if has_make_target(&source, "test") {
            commands.push(command(
                ValidationStage::FullUnitTests,
                "make",
                &["test"],
                "Makefile test target detected",
            ));
        }
        if has_make_target(&source, "build") {
            commands.push(command(
                ValidationStage::Packaging,
                "make",
                &["build"],
                "Makefile build target detected",
            ));
        }
        if has_make_target(&source, "ci") {
            commands.push(command(
                ValidationStage::IntegrationTests,
                "make",
                &["ci"],
                "Makefile custom CI target detected",
            ));
        }
        if has_make_target(&source, "smoke") {
            commands.push(command(
                ValidationStage::ProductionSmoke,
                "make",
                &["smoke"],
                "Makefile production smoke target detected",
            ));
        }
    }
    if repository.join("CMakeLists.txt").is_file() {
        commands.push(command(
            ValidationStage::Packaging,
            "cmake",
            &["--build", "build"],
            "CMake project detected; configured build directory required",
        ));
    }
    Ok(commands)
}

fn has_make_target(source: &str, target: &str) -> bool {
    source.lines().any(|line| {
        line.strip_prefix(target)
            .is_some_and(|suffix| suffix.trim_start().starts_with(':'))
    })
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
    #[error("validation manifest is not a bounded regular file: {0}")]
    UnsafeManifest(PathBuf),
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
        assert!(
            plan.undetected_stages
                .contains(&ValidationStage::IntegrationTests)
        );
    }

    #[test]
    fn empty_repository_reports_not_detected_instead_of_passed() {
        let repository = tempfile::tempdir().unwrap();
        let plan = ValidationDetector::detect(repository.path()).unwrap();
        let evidence = missing_evidence(&plan);
        assert!(!evidence.is_empty());
        assert!(
            evidence
                .iter()
                .all(|item| item.status == EvidenceStatus::NotDetected)
        );
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
            required_stages: BTreeSet::from([ValidationStage::Build]),
            accepted_unavailable_stages: BTreeSet::new(),
        };
        let mut store = SessionStore::in_memory().unwrap();
        let session_id = SessionId::new();
        let report = ValidationRunner::run(&mut store, session_id, repository.path(), &plan)
            .await
            .unwrap();
        assert!(report.completion_allowed());
        assert!(
            report
                .evidence
                .iter()
                .any(|item| item.stage == ValidationStage::Build
                    && item.status == EvidenceStatus::Passed)
        );
        assert!(
            store
                .events(session_id)
                .unwrap()
                .iter()
                .any(|event| matches!(
                    event,
                    SessionEvent::ValidationRecorded {
                        status: ValidationStatus::Passed,
                        ..
                    }
                ))
        );
        assert!(store.events(session_id).unwrap().iter().any(|event| {
            matches!(
                event,
                SessionEvent::ActionProposed {
                    action: ProposedAction::Command(action),
                    ..
                } if action.environment.get("HOME")
                    == Some(&repository.path().to_string_lossy().into_owned())
            )
        }));
    }

    #[test]
    fn unavailable_required_stage_needs_explicit_blueprint_acceptance() {
        let evidence = vec![
            ValidationEvidence {
                stage: ValidationStage::IntegrationTests,
                status: EvidenceStatus::Unavailable,
                command: None,
                exit_code: None,
                detail: "external database is unavailable".into(),
                output_truncated: false,
            },
            ValidationEvidence {
                stage: ValidationStage::CompletionCriteria,
                status: EvidenceStatus::Passed,
                command: None,
                exit_code: None,
                detail: "derived gate".into(),
                output_truncated: false,
            },
        ];
        let required = BTreeSet::from([ValidationStage::IntegrationTests]);
        let denied = ValidationReport {
            evidence: evidence.clone(),
            required_stages: required.clone(),
            accepted_unavailable_stages: BTreeSet::new(),
        };
        assert!(!denied.completion_allowed());
        let accepted = ValidationReport {
            evidence,
            required_stages: required.clone(),
            accepted_unavailable_stages: required,
        };
        assert!(accepted.completion_allowed());
    }

    #[test]
    fn failure_parser_routes_specialists_from_bounded_evidence() {
        let cases = [
            (
                "error: mismatched types",
                FailureClass::Compilation,
                "Build Repair Agent",
            ),
            (
                "AssertionError: expected 4",
                FailureClass::TestAssertion,
                "Test Repair Agent",
            ),
            (
                "Flyway migration failed",
                FailureClass::DatabaseMigration,
                "Database Migration Agent",
            ),
            (
                "No space left on device",
                FailureClass::ResourceExhaustion,
                "Runtime Reliability Agent",
            ),
        ];
        for (detail, class, specialist) in cases {
            let route = classify_failure(&ValidationEvidence {
                stage: ValidationStage::TargetedTests,
                status: EvidenceStatus::Failed,
                command: None,
                exit_code: Some(1),
                detail: detail.into(),
                output_truncated: false,
            });
            assert_eq!(route.class, class);
            assert_eq!(route.specialist_role, specialist);
        }
    }

    #[test]
    fn detects_bun_make_and_system_gradle_without_inventing_commands() {
        let repository = tempfile::tempdir().unwrap();
        std::fs::write(
            repository.path().join("package.json"),
            r#"{"packageManager":"bun@1.2.0","scripts":{"test":"vitest"}}"#,
        )
        .unwrap();
        std::fs::write(
            repository.path().join("Makefile"),
            "test:\n\ttrue\nbuild:\n\ttrue\n",
        )
        .unwrap();
        std::fs::write(repository.path().join("build.gradle.kts"), "plugins {}\n").unwrap();
        let plan = ValidationDetector::detect(repository.path()).unwrap();
        assert!(
            plan.commands.iter().any(|command| {
                command.program == "bun" && command.arguments == ["run", "test"]
            })
        );
        assert!(
            plan.commands
                .iter()
                .any(|command| { command.program == "make" && command.arguments == ["test"] })
        );
        assert!(
            plan.commands
                .iter()
                .any(|command| command.program == "gradle")
        );
    }

    #[test]
    fn detects_dotnet_unittest_and_custom_ci_in_progressive_order() {
        let repository = tempfile::tempdir().unwrap();
        std::fs::write(
            repository.path().join("fixture.csproj"),
            "<Project Sdk=\"Microsoft.NET.Sdk\" />\n",
        )
        .unwrap();
        std::fs::create_dir(repository.path().join("tests")).unwrap();
        std::fs::write(
            repository.path().join("Makefile"),
            "ci:\n\ttrue\nbuild:\n\ttrue\n",
        )
        .unwrap();
        let plan = ValidationDetector::detect(repository.path()).unwrap();
        assert!(plan.commands.iter().any(|command| {
            command.program == "dotnet" && command.arguments == ["test", "--no-restore"]
        }));
        assert!(plan.commands.iter().any(|command| {
            command.program == "python3"
                && command.arguments == ["-m", "unittest", "discover", "-s", "tests"]
        }));
        assert!(plan.commands.iter().any(|command| {
            command.program == "make"
                && command.arguments == ["ci"]
                && command.stage == ValidationStage::IntegrationTests
        }));
        assert!(
            plan.commands
                .windows(2)
                .all(|pair| progressive_rank(pair[0].stage) <= progressive_rank(pair[1].stage))
        );
    }

    #[cfg(unix)]
    #[test]
    fn external_symlinked_tests_directory_is_not_detected() {
        let repository = tempfile::tempdir().unwrap();
        let external = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(external.path(), repository.path().join("tests")).unwrap();
        let plan = ValidationDetector::detect(repository.path()).unwrap();
        assert!(!plan.commands.iter().any(|command| {
            command.program == "python3" && command.arguments.contains(&"unittest".into())
        }));
    }
}
