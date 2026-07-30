//! Environment detection and toolchain-provisioning contracts (PRD §9).
//!
//! This crate fixes the data shapes for *what the project needs* and *what the
//! host has*, and the actions that bridge the two. The actual detection
//! (`which <tool>`, parsing manifest files) and the managed-install downloads
//! land in PR5; here we only pin the typed contract the orchestrator, the UI
//! inspector, and the doctor command agree on — so a missing-JDK plan and a
//! successful auto-install report the same shapes the studio renders.
//!
//! `EnvironmentPlan` is the central type (PRD §9.2). It is plain serializable
//! data: required tools the project declares, tools detected on the host,
//! the missing delta, the bounded install actions chosen by the strategy in
//! PRD §9.3, and the validation actions that prove each install worked.

use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
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

#[derive(Clone, Debug, Error, JsonSchema, PartialEq, Serialize, Deserialize)]
pub enum EnvironmentError {
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
}
