//! Environment detection and toolchain-provisioning contracts (PRD §9).
//!
//! This crate fixes the data shapes for *what the project needs* and *what the
//! host has*, and the actions that bridge the two. Detection is bounded and
//! uses explicit argument vectors; missing tools produce governed provisioning
//! plans and independent verification checks rather than placeholder success.
//!
//! `EnvironmentPlan` is the central type (PRD §9.2). It is plain serializable
//! data: required tools the project declares, tools detected on the host,
//! the missing delta, the bounded install actions chosen by the strategy in
//! PRD §9.3, and the validation actions that prove each install worked.

use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;
use thiserror::Error;
use uuid::Uuid;

pub use purrcode_workspace_contracts::EnvironmentProfileId;

/// A build/test/dependency tool kind we know how to detect and provision.
///
/// Mirrors the detection list in PRD §9.1. Adding a variant is a contract
/// change; the orchestrator must be able to reason about every kind.
#[derive(
    Clone, Copy, Debug, Eq, Hash, JsonSchema, Ord, PartialEq, PartialOrd, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum ToolKind {
    Bundler, // bundler/npm/pnpm/yarn/bun tracked separately below
    PackageManager,
    Git,
    Node,
    Npm,
    Pnpm,
    Yarn,
    Bun,
    Python,
    Uv,
    Java,
    Maven,
    Gradle,
    Go,
    Rust,
    Dotnet,
    Docker,
    BuildEssential,
    DatabaseClient,
    ContainerRuntime,
}

impl ToolKind {
    /// Lowercase human label used by the UI and by `purrcode doctor`.
    pub fn label(self) -> &'static str {
        match self {
            ToolKind::Bundler => "bundler",
            ToolKind::PackageManager => "package manager",
            ToolKind::Git => "git",
            ToolKind::Node => "node",
            ToolKind::Npm => "npm",
            ToolKind::Pnpm => "pnpm",
            ToolKind::Yarn => "yarn",
            ToolKind::Bun => "bun",
            ToolKind::Python => "python",
            ToolKind::Uv => "uv",
            ToolKind::Java => "java",
            ToolKind::Maven => "maven",
            ToolKind::Gradle => "gradle",
            ToolKind::Go => "go",
            ToolKind::Rust => "rust",
            ToolKind::Dotnet => "dotnet",
            ToolKind::Docker => "docker",
            ToolKind::BuildEssential => "build-essential",
            ToolKind::DatabaseClient => "database client",
            ToolKind::ContainerRuntime => "container runtime",
        }
    }
}

impl std::fmt::Display for ToolKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

/// Constraint on a required tool (PRD §9.2). The resolver matches this against
/// [`DetectedTool`] versions; a missing match becomes a missing tool.
#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct ToolRequirement {
    pub kind: ToolKind,
    /// Optional minimum version, parsed as a semver-style string. The
    /// detector owns the parser; this crate only stores and serializes it.
    #[serde(default)]
    pub min_version: Option<String>,
    /// True when the requirement is mandatory for a successful build (the
    /// orchestrator refuses to skip it). False means "nice to have".
    #[serde(default)]
    pub required: bool,
    /// Free-text reason recorded so the UI can show why (PRD §3.5 evidence).
    #[serde(default)]
    pub reason: String,
}

/// A tool that was found on the host or in the workspace (PRD §9.2).
#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct DetectedTool {
    pub kind: ToolKind,
    /// Absolute path to the executable that answered the probe.
    pub path: PathBuf,
    /// Observed version string from `<tool> --version` (or equivalent).
    #[serde(default)]
    pub version: String,
    /// Where the tool came from. Drives the §9.3 preference order.
    pub origin: ToolOrigin,
}

/// Where a detected tool lives, in PRD §9.3 preference order.
#[derive(
    Clone, Copy, Debug, Eq, JsonSchema, Ord, PartialEq, PartialOrd, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum ToolOrigin {
    /// Project-local tool already present (e.g. `./mvnw`).
    ProjectLocal,
    /// Compatible system tool already present.
    System,
    /// Repository-provided wrapper script.
    RepositoryWrapper,
    /// PurrCode-managed user-local toolchain (e.g. under ~/.purrcode/toolchains).
    Managed,
    /// Containerized toolchain.
    Container,
    /// Installed via system package manager.
    SystemPackage,
}

impl ToolOrigin {
    /// Preference rank from PRD §9.3 (lower is better).
    pub fn preference_rank(self) -> u8 {
        match self {
            ToolOrigin::ProjectLocal => 1,
            ToolOrigin::System => 2,
            ToolOrigin::RepositoryWrapper => 3,
            ToolOrigin::Managed => 4,
            ToolOrigin::Container => 5,
            ToolOrigin::SystemPackage => 6,
        }
    }

    pub fn is_preferred_over(self, other: ToolOrigin) -> bool {
        self.preference_rank() < other.preference_rank()
    }
}

/// How to install a tool that is missing (PRD §9.3, §9.4).
#[derive(
    Clone, Copy, Debug, Eq, JsonSchema, Ord, PartialEq, PartialOrd, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum InstallStrategy {
    /// Use a project-local compatible tool if one appears after a prior step.
    ExistingLocal,
    /// Use a compatible system tool.
    ExistingSystem,
    /// Use a repository-provided wrapper (mvnw/gradlew).
    RepositoryWrapper,
    /// PurrCode-managed user-local toolchain (default for missing Node/JDK).
    Managed,
    /// Containerized toolchain (when host pollution would be excessive).
    Container,
    /// System package installation (last resort, may need elevation).
    SystemPackage,
}

/// The install preference order from PRD §9.3, in order.
pub const INSTALL_PREFERENCE_ORDER: &[InstallStrategy] = &[
    InstallStrategy::ExistingLocal,
    InstallStrategy::ExistingSystem,
    InstallStrategy::RepositoryWrapper,
    InstallStrategy::Managed,
    InstallStrategy::Container,
    InstallStrategy::SystemPackage,
];

/// A concrete, bounded provisioning action the runtime may execute (PRD §9.2).
///
/// Each action describes *what* to install and *where*, and carries an
/// estimated byte size so the daemon can bound downloads. Execution (and the
/// associated PawGate authorization) happens in PR5; the contract is fixed here.
#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct ProvisionAction {
    pub kind: ToolKind,
    pub strategy: InstallStrategy,
    /// Destination for a managed install (e.g. ~/.purrcode/toolchains/node/22).
    #[serde(default)]
    pub target_path: Option<PathBuf>,
    /// Version to install.
    #[serde(default)]
    pub target_version: String,
    /// Estimated download size in bytes, bounded by the daemon.
    #[serde(default)]
    pub estimated_bytes: u64,
    /// True when root/sudo is required (PRD §9.5).
    #[serde(default)]
    pub requires_elevation: bool,
    /// Human-readable plan line ("download Node 22 → install under …").
    #[serde(default)]
    pub description: String,
}

/// A validation check that proves an installation succeeded (PRD §9.4 "verify
/// every installation before marking it ready"). Typing the check separately
/// keeps the orchestrator honest: an installers's own exit code is not enough;
/// the runtime re-probes the tool.
#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct EnvironmentCheck {
    pub kind: ToolKind,
    /// Program to run for the verification probe.
    pub program: PathBuf,
    /// Explicit argument vector (no shell string).
    #[serde(default)]
    pub arguments: Vec<String>,
    /// Expected substring in stdout, or empty to assert only exit-zero.
    #[serde(default)]
    pub expected_output_contains: String,
    #[serde(default)]
    pub timeout_secs: u32,
}

/// Outcome of running an [`EnvironmentCheck`].
#[derive(
    Clone, Copy, Debug, Eq, JsonSchema, Ord, PartialEq, PartialOrd, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum CheckResult {
    Passed,
    Failed,
    TimedOut,
    /// The check has not been executed yet.
    Pending,
}

/// Evidence attached to a completed check (PRD §3.5 evidence-based completion).
#[derive(Clone, Debug, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct CheckEvidence {
    pub check: EnvironmentCheck,
    pub result: CheckResult,
    pub exit_code: Option<i32>,
    #[serde(default)]
    pub stdout_tail: String,
    #[serde(default)]
    pub checked_at: Option<DateTime<Utc>>,
}

/// The central environment plan (PRD §9.2).
#[derive(Clone, Debug, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct EnvironmentPlan {
    pub profile_id: EnvironmentProfileId,
    pub required_tools: Vec<ToolRequirement>,
    pub detected_tools: Vec<DetectedTool>,
    pub missing_tools: Vec<ToolRequirement>,
    pub installation_actions: Vec<ProvisionAction>,
    pub validation_actions: Vec<EnvironmentCheck>,
}

impl EnvironmentPlan {
    pub fn new(profile_id: EnvironmentProfileId) -> Self {
        Self {
            profile_id,
            required_tools: vec![],
            detected_tools: vec![],
            missing_tools: vec![],
            installation_actions: vec![],
            validation_actions: vec![],
        }
    }

    /// Compute the missing required tools by subtracting detected from
    /// required. A requirement is satisfied when a detected tool of the same
    /// kind exists and (when the requirement pins a min_version) its observed
    /// version is >= the minimum, lexicographically — the detector owns the
    /// real semver parser; this is a safe default for coarse matching.
    pub fn compute_missing(&mut self) {
        self.missing_tools = self
            .required_tools
            .iter()
            .filter(|req| !self.satisfies(req))
            .cloned()
            .collect();
    }

    /// True when a detected tool satisfies the given requirement.
    pub fn satisfies(&self, req: &ToolRequirement) -> bool {
        self.detected_tools.iter().any(|d| {
            d.kind == req.kind
                && match req.min_version.as_deref() {
                    None => true,
                    Some(min) => {
                        // An empty observed version means the detector could
                        // not parse one, so we cannot trust it to meet a pinned
                        // minimum — fail closed toward "missing".
                        !d.version.is_empty() && version_at_least(d.version.as_str(), min)
                    }
                }
        })
    }

    /// All mandatory requirements are covered by detected tools.
    pub fn all_required_satisfied(&self) -> bool {
        self.required_tools
            .iter()
            .filter(|r| r.required)
            .all(|r| self.satisfies(r))
    }
}

/// Naive lexicographic version comparison used as a coarse default. The
/// detector is expected to supply a semver parse, but this keeps the contract
/// self-contained and correct for the common single-dotted forms.
fn version_at_least(found: &str, min: &str) -> bool {
    fn split(s: &str) -> Vec<u64> {
        s.split('.')
            .map(|p| p.trim_start_matches('v'))
            .filter_map(|p| p.parse::<u64>().ok())
            .collect()
    }
    let f = split(found);
    let m = split(min);
    let n = f.len().max(m.len());
    for i in 0..n {
        let fv = f.get(i).copied().unwrap_or(0);
        let mv = m.get(i).copied().unwrap_or(0);
        match fv.cmp(&mv) {
            std::cmp::Ordering::Equal => continue,
            std::cmp::Ordering::Greater => return true,
            std::cmp::Ordering::Less => return false,
        }
    }
    true
}

/// Detected host operating environment (PRD §9.1). Plain data; the actual
/// probes (`uname`, `uname -m`, reading `/etc/os-release`) land in PR5.
#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct HostEnvironment {
    pub os_family: OsFamily,
    pub arch: String,
    pub distribution: Option<String>,
    pub shell: Option<String>,
    pub package_manager: Option<String>,
    pub sudo_available: bool,
    pub container_runtime_present: bool,
    #[serde(default)]
    pub available_memory_bytes: Option<u64>,
    #[serde(default)]
    pub available_disk_bytes: Option<u64>,
}

#[derive(
    Clone, Copy, Debug, Eq, JsonSchema, Ord, PartialEq, PartialOrd, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum OsFamily {
    Linux,
    Macos,
    Windows,
}

/// Complete, evidence-bearing result used by `purrcode doctor` and Studio.
#[derive(Clone, Debug, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct EnvironmentDoctorReport {
    pub host: HostEnvironment,
    pub plan: EnvironmentPlan,
    pub checks: Vec<CheckEvidence>,
    pub ready: bool,
    #[serde(default)]
    pub warnings: Vec<String>,
}

/// Inspect one repository without modifying it or accessing the network.
/// Manifest reads are bounded and tool probes use explicit argv with a timeout.
pub async fn inspect_environment(
    repository: &Path,
    managed_root: &Path,
) -> Result<EnvironmentDoctorReport, EnvironmentError> {
    if !repository.is_absolute() || !repository.is_dir() {
        return Err(EnvironmentError::InvalidRepository(
            repository.display().to_string(),
        ));
    }
    if !managed_root.is_absolute() {
        return Err(EnvironmentError::InvalidManagedRoot(
            managed_root.display().to_string(),
        ));
    }
    let host = detect_host();
    let required_tools = detect_project_requirements(repository)?;
    let kinds: BTreeSet<_> = required_tools.iter().map(|item| item.kind).collect();
    let mut detected_tools = Vec::new();
    for kind in kinds {
        if let Some(tool) = detect_tool(repository, managed_root, kind).await {
            detected_tools.push(tool);
        }
    }
    detected_tools.sort_by_key(|tool| (tool.kind, tool.origin.preference_rank()));
    let mut plan = EnvironmentPlan::new(new_profile_id());
    plan.required_tools = required_tools;
    plan.detected_tools = detected_tools;
    plan.compute_missing();
    plan.installation_actions = plan
        .missing_tools
        .iter()
        .map(|requirement| provision_action(managed_root, requirement))
        .collect();
    plan.validation_actions = plan.detected_tools.iter().map(validation_check).collect();
    let checks = run_environment_checks(&plan.validation_actions).await;
    let checks_pass = checks
        .iter()
        .all(|evidence| evidence.result == CheckResult::Passed);
    let ready = plan.all_required_satisfied() && checks_pass;
    let warnings = plan
        .missing_tools
        .iter()
        .map(|tool| {
            format!(
                "{} is missing; provisioning requires exact authorization and post-install verification",
                tool.kind
            )
        })
        .collect();
    Ok(EnvironmentDoctorReport {
        host,
        plan,
        checks,
        ready,
        warnings,
    })
}

pub async fn run_environment_checks(checks: &[EnvironmentCheck]) -> Vec<CheckEvidence> {
    let mut evidence = Vec::with_capacity(checks.len());
    for check in checks {
        let timeout = Duration::from_secs(u64::from(check.timeout_secs.clamp(1, 30)));
        let mut command = tokio::process::Command::new(&check.program);
        command
            .args(&check.arguments)
            .env_clear()
            .envs(safe_environment())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let outcome = tokio::time::timeout(timeout, command.output()).await;
        let checked_at = Some(Utc::now());
        let item = match outcome {
            Err(_) => CheckEvidence {
                check: check.clone(),
                result: CheckResult::TimedOut,
                exit_code: None,
                stdout_tail: String::new(),
                checked_at,
            },
            Ok(Err(error)) => CheckEvidence {
                check: check.clone(),
                result: CheckResult::Failed,
                exit_code: None,
                stdout_tail: bounded_text(error.to_string().as_bytes()),
                checked_at,
            },
            Ok(Ok(output)) => {
                let mut combined = output.stdout;
                combined.extend_from_slice(&output.stderr);
                let rendered = bounded_text(&combined);
                let matched = check.expected_output_contains.is_empty()
                    || rendered.contains(&check.expected_output_contains);
                CheckEvidence {
                    check: check.clone(),
                    result: if output.status.success() && matched {
                        CheckResult::Passed
                    } else {
                        CheckResult::Failed
                    },
                    exit_code: output.status.code(),
                    stdout_tail: rendered,
                    checked_at,
                }
            }
        };
        evidence.push(item);
    }
    evidence
}

fn detect_host() -> HostEnvironment {
    let mut system = sysinfo::System::new_all();
    system.refresh_memory();
    let disks = sysinfo::Disks::new_with_refreshed_list();
    HostEnvironment {
        os_family: if cfg!(target_os = "macos") {
            OsFamily::Macos
        } else if cfg!(windows) {
            OsFamily::Windows
        } else {
            OsFamily::Linux
        },
        arch: std::env::consts::ARCH.into(),
        distribution: sysinfo::System::long_os_version(),
        shell: std::env::var("SHELL")
            .ok()
            .or_else(|| std::env::var("COMSPEC").ok()),
        package_manager: ["brew", "apt-get", "dnf", "yum", "pacman", "winget"]
            .into_iter()
            .find(|program| find_on_path(program).is_some())
            .map(str::to_owned),
        sudo_available: find_on_path("sudo").is_some(),
        container_runtime_present: find_on_path("docker").is_some()
            || find_on_path("podman").is_some(),
        available_memory_bytes: Some(system.available_memory()),
        available_disk_bytes: Some(disks.iter().map(sysinfo::Disk::available_space).sum()),
    }
}

fn detect_project_requirements(
    repository: &Path,
) -> Result<Vec<ToolRequirement>, EnvironmentError> {
    let mut requirements = BTreeMap::<ToolKind, ToolRequirement>::new();
    {
        let mut add = |kind, min_version: Option<String>, required, reason: &str| {
            requirements
                .entry(kind)
                .and_modify(|existing| {
                    existing.required |= required;
                    if existing.min_version.is_none() {
                        existing.min_version.clone_from(&min_version);
                    }
                    if !existing.reason.contains(reason) {
                        existing.reason.push_str("; ");
                        existing.reason.push_str(reason);
                    }
                })
                .or_insert(ToolRequirement {
                    kind,
                    min_version,
                    required,
                    reason: reason.into(),
                });
        };

        if let Some(package) = read_bounded(repository.join("package.json"))? {
            add(
                ToolKind::Node,
                json_string_at(&package, &["engines", "node"]).and_then(first_version),
                true,
                "package.json",
            );
            let manager = json_string_at(&package, &["packageManager"])
                .unwrap_or_default()
                .split('@')
                .next()
                .unwrap_or_default()
                .to_owned();
            match manager.as_str() {
                "pnpm" => add(ToolKind::Pnpm, None, true, "packageManager"),
                "yarn" => add(ToolKind::Yarn, None, true, "packageManager"),
                "bun" => add(ToolKind::Bun, None, true, "packageManager"),
                _ => add(ToolKind::Npm, None, true, "package.json"),
            }
        }
        if repository.join("pnpm-lock.yaml").exists() {
            add(ToolKind::Pnpm, None, true, "pnpm lockfile");
        }
        if repository.join("yarn.lock").exists() {
            add(ToolKind::Yarn, None, true, "yarn lockfile");
        }
        if repository.join("bun.lockb").exists() || repository.join("bun.lock").exists() {
            add(ToolKind::Bun, None, true, "bun lockfile");
        }
        if let Some(pom) = read_bounded(repository.join("pom.xml"))? {
            add(
                ToolKind::Java,
                xml_version(&pom, &["maven.compiler.release", "java.version"]),
                true,
                "pom.xml",
            );
            add(ToolKind::Maven, None, true, "pom.xml");
        }
        if repository.join("build.gradle").exists() || repository.join("build.gradle.kts").exists()
        {
            add(ToolKind::Java, None, true, "Gradle build");
            add(ToolKind::Gradle, None, true, "Gradle build");
        }
        if repository.join("pyproject.toml").exists()
            || repository.join("requirements.txt").exists()
        {
            add(ToolKind::Python, None, true, "Python project manifest");
            if repository.join("uv.lock").exists() {
                add(ToolKind::Uv, None, true, "uv lockfile");
            }
        }
        if let Some(cargo) = read_bounded(repository.join("Cargo.toml"))? {
            let version = cargo
                .lines()
                .find_map(|line| line.trim().strip_prefix("rust-version"))
                .and_then(|line| line.split('=').nth(1))
                .map(|value| value.trim().trim_matches('"').to_owned());
            add(ToolKind::Rust, version, true, "Cargo.toml");
        }
        if let Some(go_mod) = read_bounded(repository.join("go.mod"))? {
            let version = go_mod
                .lines()
                .find_map(|line| line.trim().strip_prefix("go "))
                .and_then(first_version);
            add(ToolKind::Go, version, true, "go.mod");
        }
        let dotnet = std::fs::read_dir(repository)
            .ok()
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .any(|entry| {
                matches!(
                    entry.path().extension().and_then(|v| v.to_str()),
                    Some("csproj" | "fsproj" | "sln")
                )
            });
        if dotnet {
            add(ToolKind::Dotnet, None, true, ".NET project");
        }
        if repository.join("Dockerfile").exists()
            || repository.join("compose.yaml").exists()
            || repository.join("docker-compose.yml").exists()
        {
            add(ToolKind::Docker, None, false, "container manifest");
        }
    }
    if !requirements.is_empty() {
        requirements.insert(
            ToolKind::Git,
            ToolRequirement {
                kind: ToolKind::Git,
                min_version: None,
                required: true,
                reason: "repository operations".into(),
            },
        );
    }
    Ok(requirements.into_values().collect())
}

async fn detect_tool(
    repository: &Path,
    managed_root: &Path,
    kind: ToolKind,
) -> Option<DetectedTool> {
    let (names, arguments): (&[&str], &[&str]) = match kind {
        ToolKind::Git => (&["git"], &["--version"]),
        ToolKind::Node => (&["node"], &["--version"]),
        ToolKind::Npm => (&["npm"], &["--version"]),
        ToolKind::Pnpm => (&["pnpm"], &["--version"]),
        ToolKind::Yarn => (&["yarn"], &["--version"]),
        ToolKind::Bun => (&["bun"], &["--version"]),
        ToolKind::Python => (&["python3", "python"], &["--version"]),
        ToolKind::Uv => (&["uv"], &["--version"]),
        ToolKind::Java => (&["java"], &["-version"]),
        ToolKind::Maven => (&["mvn"], &["--version"]),
        ToolKind::Gradle => (&["gradle"], &["--version"]),
        ToolKind::Go => (&["go"], &["version"]),
        ToolKind::Rust => (&["rustc"], &["--version"]),
        ToolKind::Dotnet => (&["dotnet"], &["--version"]),
        ToolKind::Docker | ToolKind::ContainerRuntime => (&["docker", "podman"], &["--version"]),
        ToolKind::BuildEssential => (&["cc", "clang", "gcc"], &["--version"]),
        _ => return None,
    };
    let wrapper = match kind {
        ToolKind::Maven => Some(repository.join("mvnw")),
        ToolKind::Gradle => Some(repository.join("gradlew")),
        _ => None,
    };
    let candidates = wrapper
        .into_iter()
        .map(|path| (path, ToolOrigin::RepositoryWrapper))
        .chain(names.iter().filter_map(|name| {
            find_in_root(managed_root, name).map(|path| (path, ToolOrigin::Managed))
        }))
        .chain(
            names
                .iter()
                .filter_map(|name| find_on_path(name).map(|path| (path, ToolOrigin::System))),
        );
    for (path, origin) in candidates {
        if !path.is_file() {
            continue;
        }
        if let Some(version) = probe_version(&path, arguments).await {
            return Some(DetectedTool {
                kind,
                path,
                version,
                origin,
            });
        }
    }
    None
}

async fn probe_version(path: &Path, arguments: &[&str]) -> Option<String> {
    let mut command = tokio::process::Command::new(path);
    command
        .args(arguments)
        .env_clear()
        .envs(safe_environment())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let output = tokio::time::timeout(Duration::from_secs(5), command.output())
        .await
        .ok()?
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let bytes = if output.stdout.is_empty() {
        &output.stderr
    } else {
        &output.stdout
    };
    first_version(String::from_utf8_lossy(bytes))
}

fn validation_check(tool: &DetectedTool) -> EnvironmentCheck {
    let arguments = match tool.kind {
        ToolKind::Java => vec!["-version".into()],
        ToolKind::Go => vec!["version".into()],
        _ => vec!["--version".into()],
    };
    EnvironmentCheck {
        kind: tool.kind,
        program: tool.path.clone(),
        arguments,
        expected_output_contains: String::new(),
        timeout_secs: 5,
    }
}

fn provision_action(managed_root: &Path, requirement: &ToolRequirement) -> ProvisionAction {
    let version = requirement
        .min_version
        .clone()
        .unwrap_or_else(|| "latest-qualified".into());
    ProvisionAction {
        kind: requirement.kind,
        strategy: InstallStrategy::Managed,
        target_path: Some(
            managed_root
                .join(requirement.kind.label().replace(' ', "-"))
                .join(&version),
        ),
        target_version: version.clone(),
        estimated_bytes: 0,
        requires_elevation: false,
        description: format!(
            "install a checksum-verified {} {} into managed storage",
            requirement.kind, version
        ),
    }
}

fn read_bounded(path: PathBuf) -> Result<Option<String>, EnvironmentError> {
    let metadata = match std::fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(EnvironmentError::Io(error.to_string())),
    };
    if !metadata.file_type().is_file() || metadata.len() > 1024 * 1024 {
        return Err(EnvironmentError::UnsafeManifest(path.display().to_string()));
    }
    std::fs::read_to_string(&path)
        .map(Some)
        .map_err(|error| EnvironmentError::Io(error.to_string()))
}

fn json_string_at(source: &str, path: &[&str]) -> Option<String> {
    let mut value: &serde_json::Value = &serde_json::from_str::<serde_json::Value>(source).ok()?;
    for key in path {
        value = value.get(*key)?;
    }
    value.as_str().map(str::to_owned)
}

fn xml_version(source: &str, tags: &[&str]) -> Option<String> {
    tags.iter().find_map(|tag| {
        let start = format!("<{tag}>");
        let end = format!("</{tag}>");
        let value = source.split_once(&start)?.1.split_once(&end)?.0;
        first_version(value)
    })
}

fn first_version(value: impl AsRef<str>) -> Option<String> {
    let value = value.as_ref();
    let start = value.find(|character: char| character.is_ascii_digit())?;
    let version: String = value[start..]
        .chars()
        .take_while(|character| character.is_ascii_digit() || *character == '.')
        .collect();
    (!version.is_empty()).then_some(version)
}

fn find_in_root(root: &Path, name: &str) -> Option<PathBuf> {
    [
        root.join(name).join("bin").join(name),
        root.join("bin").join(name),
    ]
    .into_iter()
    .find(|path| path.is_file())
}

fn find_on_path(name: &str) -> Option<PathBuf> {
    std::env::var_os("PATH")
        .into_iter()
        .flat_map(|path| std::env::split_paths(&path).collect::<Vec<_>>())
        .flat_map(|directory| {
            let direct = directory.join(name);
            #[cfg(windows)]
            {
                vec![
                    direct.clone(),
                    direct.with_extension("exe"),
                    direct.with_extension("cmd"),
                    direct.with_extension("bat"),
                ]
            }
            #[cfg(not(windows))]
            {
                vec![direct]
            }
        })
        .find(|path| path.is_file())
}

fn safe_environment() -> BTreeMap<String, String> {
    [
        "PATH",
        "TMPDIR",
        "TEMP",
        "LANG",
        "LC_ALL",
        "SystemRoot",
        "WINDIR",
    ]
    .into_iter()
    .filter_map(|key| std::env::var(key).ok().map(|value| (key.into(), value)))
    .collect()
}

fn bounded_text(bytes: &[u8]) -> String {
    let start = bytes.len().saturating_sub(16 * 1024);
    String::from_utf8_lossy(&bytes[start..])
        .chars()
        .filter(|character| *character == '\n' || *character == '\t' || !character.is_control())
        .collect()
}

#[derive(Clone, Debug, Error, JsonSchema, PartialEq, Serialize, Deserialize)]
pub enum EnvironmentError {
    #[error("repository must be an existing absolute directory: {0}")]
    InvalidRepository(String),
    #[error("managed toolchain root must be absolute: {0}")]
    InvalidManagedRoot(String),
    #[error("manifest is not a bounded regular file: {0}")]
    UnsafeManifest(String),
    #[error("environment inspection I/O failed: {0}")]
    Io(String),
    #[error("tool {kind} is required but missing and could not be installed")]
    RequiredMissing { kind: ToolKind },
    #[error("install verification failed for {kind}")]
    VerificationFailed {
        kind: ToolKind,
        #[serde(default)]
        detail: String,
    },
    #[error("unsupported host: {0}")]
    UnsupportedHost(String),
    #[error("elevation required but not authorized")]
    ElevationRequired,
}

/// Generate a new [`EnvironmentProfileId`].
pub fn new_profile_id() -> EnvironmentProfileId {
    EnvironmentProfileId(Uuid::new_v4())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(kind: ToolKind, min: Option<&str>) -> ToolRequirement {
        ToolRequirement {
            kind,
            min_version: min.map(String::from),
            required: true,
            reason: "test".into(),
        }
    }

    fn det(kind: ToolKind, version: &str, origin: ToolOrigin) -> DetectedTool {
        DetectedTool {
            kind,
            path: PathBuf::from(format!("/usr/bin/{}", kind.label())),
            version: version.into(),
            origin,
        }
    }

    #[test]
    fn preference_order_matches_prd() {
        assert_eq!(
            INSTALL_PREFERENCE_ORDER,
            &[
                InstallStrategy::ExistingLocal,
                InstallStrategy::ExistingSystem,
                InstallStrategy::RepositoryWrapper,
                InstallStrategy::Managed,
                InstallStrategy::Container,
                InstallStrategy::SystemPackage,
            ]
        );
        assert!(ToolOrigin::ProjectLocal.is_preferred_over(ToolOrigin::System));
        assert!(!ToolOrigin::SystemPackage.is_preferred_over(ToolOrigin::Managed));
    }

    #[test]
    fn compute_missing_flags_unmet_required_tool() {
        let mut plan = EnvironmentPlan::new(new_profile_id());
        plan.required_tools = vec![req(ToolKind::Node, Some("22")), req(ToolKind::Git, None)];
        plan.detected_tools = vec![det(ToolKind::Git, "2.43", ToolOrigin::System)];
        plan.compute_missing();
        assert_eq!(plan.missing_tools.len(), 1);
        assert_eq!(plan.missing_tools[0].kind, ToolKind::Node);
        assert!(!plan.all_required_satisfied());
    }

    #[test]
    fn version_constraint_is_satisfied_when_found_meets_minimum() {
        let mut plan = EnvironmentPlan::new(new_profile_id());
        plan.required_tools = vec![req(ToolKind::Node, Some("22"))];
        plan.detected_tools = vec![det(ToolKind::Node, "22.3.0", ToolOrigin::System)];
        plan.compute_missing();
        assert!(plan.missing_tools.is_empty());
        assert!(plan.all_required_satisfied());
    }

    #[test]
    fn version_constraint_is_unsatisfied_when_found_below_minimum() {
        let mut plan = EnvironmentPlan::new(new_profile_id());
        plan.required_tools = vec![req(ToolKind::Java, Some("21"))];
        plan.detected_tools = vec![det(ToolKind::Java, "11.0.21", ToolOrigin::System)];
        plan.compute_missing();
        assert_eq!(plan.missing_tools.len(), 1);
    }

    #[test]
    fn empty_version_string_does_not_satisfy_a_pinned_minimum() {
        let mut plan = EnvironmentPlan::new(new_profile_id());
        plan.required_tools = vec![req(ToolKind::Python, Some("3.10"))];
        plan.detected_tools = vec![det(ToolKind::Python, "", ToolOrigin::System)];
        plan.compute_missing();
        // An empty observed version cannot be trusted to meet 3.10.
        assert_eq!(plan.missing_tools.len(), 1);
    }

    #[test]
    fn soft_requirement_does_not_block_overall_satisfaction() {
        let mut plan = EnvironmentPlan::new(new_profile_id());
        plan.required_tools = vec![
            ToolRequirement {
                kind: ToolKind::Docker,
                min_version: None,
                required: false,
                reason: "optional".into(),
            },
            req(ToolKind::Git, None),
        ];
        plan.detected_tools = vec![det(ToolKind::Git, "2.4", ToolOrigin::System)];
        plan.compute_missing();
        // Docker is missing but not required, so the repo is buildable.
        assert!(plan.all_required_satisfied());
    }

    #[test]
    fn plan_round_trips_json() {
        let mut plan = EnvironmentPlan::new(new_profile_id());
        plan.required_tools.push(req(ToolKind::Node, Some("22")));
        let j = serde_json::to_string(&plan).unwrap();
        let back: EnvironmentPlan = serde_json::from_str(&j).unwrap();
        assert_eq!(plan, back);
    }

    #[test]
    fn version_at_least_handles_degenerate_inputs() {
        assert!(version_at_least("22.3", "22"));
        assert!(!version_at_least("21", "22"));
        assert!(version_at_least("3.12.1", "3.10"));
        assert!(version_at_least("1", "1")); // equal counts
    }

    #[test]
    fn tool_kind_label_is_lowercase_and_stable() {
        assert_eq!(ToolKind::Java.label(), "java");
        assert_eq!(ToolKind::BuildEssential.label(), "build-essential");
        assert_eq!(format!("{}", ToolKind::Node), "node");
    }

    #[tokio::test]
    async fn bounded_project_detection_builds_real_node_and_rust_plan() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(
            root.path().join("package.json"),
            r#"{"engines":{"node":">=22"},"packageManager":"pnpm@9.1.0"}"#,
        )
        .unwrap();
        std::fs::write(
            root.path().join("Cargo.toml"),
            "[package]\nname = \"fixture\"\nrust-version = \"1.88\"\n",
        )
        .unwrap();
        let managed = root.path().join("managed");
        let report = inspect_environment(root.path(), &managed).await.unwrap();
        assert!(report
            .plan
            .required_tools
            .iter()
            .any(|tool| tool.kind == ToolKind::Node && tool.min_version.as_deref() == Some("22")));
        assert!(report
            .plan
            .required_tools
            .iter()
            .any(|tool| tool.kind == ToolKind::Pnpm));
        assert!(
            report
                .plan
                .required_tools
                .iter()
                .any(|tool| tool.kind == ToolKind::Rust
                    && tool.min_version.as_deref() == Some("1.88"))
        );
        assert!(report
            .plan
            .required_tools
            .iter()
            .any(|tool| tool.kind == ToolKind::Git));
        assert_eq!(report.ready, report.plan.all_required_satisfied());
    }

    #[cfg(unix)]
    #[test]
    fn manifest_symlink_is_rejected_instead_of_reading_external_content() {
        use std::os::unix::fs::symlink;
        let root = tempfile::tempdir().unwrap();
        let external = tempfile::tempdir().unwrap();
        std::fs::write(external.path().join("package.json"), "{}").unwrap();
        symlink(
            external.path().join("package.json"),
            root.path().join("package.json"),
        )
        .unwrap();
        assert!(matches!(
            detect_project_requirements(root.path()),
            Err(EnvironmentError::UnsafeManifest(_))
        ));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn verification_records_real_exit_and_output() {
        let checks = vec![EnvironmentCheck {
            kind: ToolKind::Git,
            program: PathBuf::from("/usr/bin/printf"),
            arguments: vec!["doctor-evidence".into()],
            expected_output_contains: "doctor-evidence".into(),
            timeout_secs: 2,
        }];
        let evidence = run_environment_checks(&checks).await;
        assert_eq!(evidence[0].result, CheckResult::Passed);
        assert_eq!(evidence[0].exit_code, Some(0));
        assert!(evidence[0].stdout_tail.contains("doctor-evidence"));
        assert!(evidence[0].checked_at.is_some());
    }
}
