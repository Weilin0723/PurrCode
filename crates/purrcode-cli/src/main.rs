#![allow(clippy::collapsible_if)]

use anyhow::{Context, Result, bail};
use chrono::Utc;
use clap::{Parser, Subcommand};
use directories::ProjectDirs;
use futures::future::join_all;
use purrcode_claw::{ToolRuntime, sandbox_capability};
use purrcode_codex_bridge::{CodexBridge, CodexBridgeConfig};
use purrcode_daemon::{
    DAEMON_API_VERSION, DaemonConfig, NATIVE_IDE_API_VERSION, NATIVE_IDE_BUILD_FINGERPRINT,
    NATIVE_IDE_CAPABILITIES, bind_and_report,
};
use purrcode_evaluation_runtime::{
    BenchmarkCategory, BenchmarkResult, BenchmarkStatus, EvaluationMetrics, aggregate_metrics,
};
use purrcode_evidence_bundle::{
    EvidenceBundle, export_bundle, inspect_bundle, replay_bundle, verify_bundle,
};
use purrcode_golden_suite::GoldenCatalog;
use purrcode_mcp_host::{
    McpHost, McpServerConfig, discover_skills, uninstall_skill, verify_installed_skill,
};
use purrcode_model_selection::{ModelCandidate, SelectionBudget, select_coder, select_judge};
use purrcode_ninelives::SessionStore;
use purrcode_pawgate::{Policy, resolve_policy_path};
use purrcode_provider_gateway::{
    AppConfig, JudgmentRuntimeConfig, ModelCapabilities, ModelId, ModelsConfig, PrivacyConfig,
    PrivacyMode, ProviderConfig, ProviderRouter, delete_credential, qualify_model,
    store_credential,
};
use purrcode_repository_engine::{ApplicationStrategy, RepositoryEngine, SessionWorktree};
use purrcode_runtime_core::{
    ActionId, ApprovalAuthority, Authorization, CommandAction, JudgmentDecision, ProposedAction,
    ResearchEvent, ResearchExport, ResearchMetrics, SessionEvent, SessionId, ValidationStatus,
};
use purrcode_tui::TuiConfig;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use sysinfo::System;
use tracing_subscriber::EnvFilter;
use zeroize::Zeroize;

/// Clip a report column to `width`, marking the clip so a truncated value is
/// never mistaken for a complete one.
fn truncate_column(value: &str, width: usize) -> String {
    if value.chars().count() <= width {
        return value.to_owned();
    }
    let mut clipped: String = value.chars().take(width.saturating_sub(1)).collect();
    clipped.push('~');
    clipped
}

fn redacted_identifier(value: &str) -> String {
    format!("anon-{}", hex::encode(Sha256::digest(value.as_bytes())))
}

#[derive(Parser)]
#[command(
    name = "purrcode",
    version,
    about = "Local-first coding agent judgment runtime"
)]
struct Cli {
    #[arg(long, global = true)]
    database: Option<PathBuf>,
    #[arg(long, global = true)]
    config: Option<PathBuf>,
    #[arg(long, global = true, default_value = "http://127.0.0.1:7377")]
    daemon_url: String,
    /// Override the daemon bearer-token file (useful for isolated or remote runtimes).
    #[arg(long, global = true)]
    daemon_token: Option<PathBuf>,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Discover local models, create secure defaults, and start the daemon.
    Init {
        #[arg(long)]
        force: bool,
        #[arg(long)]
        no_start: bool,
    },
    /// Launch the authenticated graphical PurrCode Studio (browser shell).
    ///
    /// Studio is a maintenance/development client. The release IDE path is
    /// `purrcode ide`.
    Studio {
        /// Attach Studio to an existing local or remote daemon.
        #[arg(long)]
        remote: Option<String>,
        /// Loopback address for the browser-facing Studio shell.
        #[arg(long, default_value = "127.0.0.1:0")]
        bind: std::net::SocketAddr,
        /// Print the one-time Studio URL instead of opening a browser.
        #[arg(long)]
        no_open: bool,
        /// Repository to restore in the workspace dashboard.
        #[arg(long)]
        repository: Option<PathBuf>,
    },
    /// Launch the browser portal. Development and maintenance only (PRD §3.4);
    /// `purrcode experimental portal` is the supported spelling.
    #[command(alias = "experimental-portal")]
    Ui {
        /// Attach Studio to an existing local or remote daemon.
        #[arg(long)]
        remote: Option<String>,
        /// Loopback address for the browser-facing Studio shell.
        #[arg(long, default_value = "127.0.0.1:0")]
        bind: std::net::SocketAddr,
        /// Print the one-time Studio URL instead of opening a browser.
        #[arg(long)]
        no_open: bool,
        /// Repository to restore in the workspace dashboard.
        #[arg(long)]
        repository: Option<PathBuf>,
    },
    /// Launch the terminal user interface explicitly.
    Tui {
        #[arg(long)]
        repository: Option<PathBuf>,
    },
    /// Open the native PurrCode desktop application on this repository.
    ///
    /// `purrcode gui` and `purrcode ide` are the same command (PRD §3.2). Neither
    /// ever opens a browser.
    #[command(alias = "ide")]
    Gui {
        /// Attach to an existing daemon session. This is never inferred from
        /// the repository's history; pass it explicitly when reopening work.
        #[arg(long)]
        session: Option<String>,
        /// Open this folder straight away. Without it the window opens on its
        /// start screen so the folder is chosen deliberately.
        #[arg(long)]
        repository: Option<PathBuf>,
    },
    /// Start an isolated, resumable native-agent session.
    Run {
        objective: String,
        #[arg(long)]
        repository: Option<PathBuf>,
        #[arg(long)]
        workflow: Option<String>,
        #[arg(long)]
        search: Option<String>,
        #[arg(long)]
        budget: Option<String>,
        #[arg(long)]
        max_tokens: Option<u64>,
    },
    /// Create a durable repository-aware plan without executing or modifying files.
    Plan {
        objective: String,
        #[arg(long)]
        repository: Option<PathBuf>,
        #[arg(long)]
        workflow: Option<String>,
        #[arg(long)]
        search: Option<String>,
        #[arg(long)]
        budget: Option<String>,
        #[arg(long)]
        max_tokens: Option<u64>,
    },
    /// Run a bounded, noninteractive CI agent and emit a complete evidence report.
    Ci {
        objective: String,
        #[arg(long)]
        repository: Option<PathBuf>,
        #[arg(long, default_value_t = 1800)]
        timeout_seconds: u64,
        #[arg(long)]
        output: Option<PathBuf>,
    },
    /// Evaluate a command without executing it.
    PolicyCheck {
        #[arg(long)]
        repository: Option<PathBuf>,
        program: PathBuf,
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        arguments: Vec<String>,
    },
    /// Execute an allowlisted read-only command through Judgment.
    Exec {
        #[arg(long)]
        repository: Option<PathBuf>,
        program: PathBuf,
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        arguments: Vec<String>,
    },
    /// Check persistence plus the repository's real build environment.
    Doctor {
        #[arg(long)]
        repository: Option<PathBuf>,
    },
    /// Resume the latest or selected session.
    Resume {
        session: Option<String>,
        /// Attach the TUI to the selected daemon session instead of only resuming it.
        #[arg(long)]
        tui: bool,
    },
    /// List durable sessions.
    Sessions,
    /// Approve the pending action in the latest or selected session.
    Approve { session: Option<String> },
    /// Reject the pending action in the latest or selected session.
    Reject {
        session: Option<String>,
        #[arg(long, default_value = "rejected by user")]
        reason: String,
    },
    /// Cancel a daemon-owned session and preserve its worktree and evidence.
    Cancel {
        session: Option<String>,
        #[arg(long, default_value = "cancelled by user")]
        reason: String,
    },
    /// Show the isolated session patch and changed paths.
    Review { session: Option<String> },
    /// Show the isolated session patch.
    Diff { session: Option<String> },
    /// Export the isolated session as a binary-capable patch.
    ExportPatch {
        destination: PathBuf,
        session: Option<String>,
    },
    /// Explicitly apply the isolated patch to the active working tree.
    Apply { session: Option<String> },
    /// Preview, then roll back all changes inside the isolated worktree.
    Rollback {
        session: Option<String>,
        #[arg(long)]
        expected_patch_digest: Option<String>,
        #[arg(long)]
        acknowledge_unattributed_effects: bool,
    },
    /// Inspect or select configured models.
    Model {
        #[command(subcommand)]
        command: ModelCommand,
    },
    /// Diagnose configured provider connectivity and credentials.
    Provider {
        #[command(subcommand)]
        command: ProviderCommand,
    },
    /// Store or remove provider secrets in the credentials.toml file.
    Credential {
        #[command(subcommand)]
        command: CredentialCommand,
    },
    /// Preview or apply versioned configuration migrations.
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
    },
    /// Check for or securely download a signed PurrCode release.
    Upgrade {
        #[command(subcommand)]
        command: UpgradeCommand,
    },
    /// Manage durable daemon-owned background tasks.
    Automation {
        #[command(subcommand)]
        command: AutomationCommand,
    },
    /// Run dependency-aware isolated workers; outputs always require independent review.
    Parallel {
        objective: String,
        workers: PathBuf,
        #[arg(long)]
        repository: Option<PathBuf>,
    },
    /// Diagnose or run the isolated Codex CLI bridge.
    Codex {
        #[command(subcommand)]
        command: CodexCommand,
    },
    /// Inspect migrations or create a verified online backup.
    Database {
        #[command(subcommand)]
        command: DatabaseCommand,
    },
    /// Inspect the strongest available execution isolation backend.
    Sandbox {
        #[command(subcommand)]
        command: SandboxCommand,
    },
    /// Run the authenticated local daemon.
    Serve {
        #[arg(long, default_value = "127.0.0.1:7377")]
        bind: std::net::SocketAddr,
        #[arg(long)]
        allow_public_bind: bool,
    },
    /// Inspect installed repository skills.
    Skill {
        #[command(subcommand)]
        command: SkillCommand,
    },
    /// Discover or invoke an isolated MCP tool through Judgment.
    Mcp {
        #[command(subcommand)]
        command: McpCommand,
    },
    /// Audit or run the production golden-task benchmark catalog.
    Benchmark {
        #[command(subcommand)]
        command: BenchmarkCommand,
    },
    /// Inspect or export research / skill-lifecycle events across sessions.
    Research {
        #[command(subcommand)]
        command: ResearchCommand,
    },
    /// Inspect durable session events.
    Trace {
        #[command(subcommand)]
        command: TraceCommand,
    },
    /// Explain why an action was allowed, denied, or required approval.
    Explain {
        #[command(subcommand)]
        command: ExplainCommand,
    },
    /// Export, inspect, verify, or replay evidence bundles.
    Bundle {
        #[command(subcommand)]
        command: BundleCommand,
    },
    /// Open the terminal workbench against an already-running daemon.
    ///
    /// Unlike the bare `purrcode` invocation this never starts a daemon, which
    /// makes it the entry point for attaching to a daemon you manage yourself.
    Workbench {
        /// Repository to work in. Defaults to the current directory.
        #[arg(long)]
        repository: Option<PathBuf>,
        /// File holding the daemon bearer token.
        #[arg(long)]
        token_file: Option<PathBuf>,
    },
    /// Inspect the user-facing action registry and its acceptance coverage.
    UiActions {
        #[command(subcommand)]
        command: UiActionsCommand,
    },
}

#[derive(Subcommand)]
enum UiActionsCommand {
    /// List every registered user-facing action with its entry points.
    List {
        /// Only actions in this category, e.g. `approval`.
        #[arg(long)]
        category: Option<String>,
    },
    /// Report acceptance coverage and fail when any action is incomplete.
    Coverage {
        /// Exit successfully even when coverage is incomplete.
        #[arg(long)]
        allow_incomplete: bool,
    },
}

#[derive(Subcommand)]
enum ModelCommand {
    List,
    Add {
        model: String,
    },
    Use {
        model: String,
    },
    /// Run the structured-output, coding, judgment, latency, and throughput suite.
    Qualify {
        model: String,
    },
}

#[derive(Subcommand)]
enum ProviderCommand {
    Doctor,
    /// Parse a provider SDK/config example without executing it and print a redacted candidate.
    Import {
        path: Option<PathBuf>,
        #[arg(long, conflicts_with = "path")]
        stdin: bool,
    },
}

#[derive(Subcommand)]
enum CredentialCommand {
    /// Prompt securely for a provider API key and store it in credentials.toml.
    Set {
        provider: String,
        #[arg(long)]
        name: Option<String>,
    },
    /// Store a provider credential as an environment variable reference instead of
    /// credentials.toml (e.g. `OPENAI_API_KEY`). The CLI will never prompt for
    /// the secret — set it in your shell or .env file instead.
    SetEnv {
        provider: String,
        /// Environment variable name (e.g. `OPENAI_API_KEY`).
        env_var: String,
    },
    /// Remove a named secret from the credentials.toml file.
    Delete { name: String },
}

#[derive(Subcommand)]
enum ConfigCommand {
    MigrationPreview,
    Migrate,
}

#[derive(Subcommand)]
enum UpgradeCommand {
    Check {
        #[arg(long, default_value = "stable")]
        channel: String,
    },
    Download {
        destination: PathBuf,
        #[arg(long, default_value = "stable")]
        channel: String,
    },
    /// Verify, unpack, and atomically install both PurrCode binaries.
    Install {
        #[arg(long, default_value = "stable")]
        channel: String,
        #[arg(long)]
        destination: Option<PathBuf>,
    },
    /// Restore the binaries preserved by the last successful installation.
    Rollback {
        #[arg(long)]
        destination: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
enum AutomationCommand {
    List,
    Create {
        objective: String,
        #[arg(long)]
        repository: Option<PathBuf>,
        #[arg(long)]
        every_seconds: u64,
    },
    Enable {
        id: String,
    },
    Disable {
        id: String,
    },
    Run {
        id: String,
    },
}

#[derive(Subcommand)]
enum CodexCommand {
    Doctor,
    Run {
        objective: String,
        #[arg(long)]
        repository: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
enum DatabaseCommand {
    Backup { destination: PathBuf },
    MigrationPreview,
}

#[derive(Subcommand)]
enum SandboxCommand {
    Doctor,
}

#[derive(Subcommand)]
enum SkillCommand {
    List {
        #[arg(long)]
        repository: Option<PathBuf>,
    },
    Install {
        source: PathBuf,
        #[arg(long)]
        repository: Option<PathBuf>,
    },
    Verify {
        name: String,
        #[arg(long)]
        repository: Option<PathBuf>,
    },
    Remove {
        name: String,
        #[arg(long)]
        repository: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
enum McpCommand {
    Discover {
        server: String,
        #[arg(long)]
        approve: bool,
    },
    Call {
        server: String,
        tool: String,
        #[arg(long, default_value = "{}")]
        arguments: String,
        #[arg(long)]
        approve: bool,
    },
}

#[derive(Subcommand)]
enum ResearchCommand {
    /// Export research events across all sessions.
    Export {
        /// Output path for the JSON export.
        output: Option<PathBuf>,
        /// Redact sensitive fields (URLs, excerpts, queries).
        #[arg(long)]
        redacted: bool,
    },
}

#[derive(Subcommand)]
enum TraceCommand {
    /// Show a session's event timeline.
    Show {
        /// Session ID (or "latest" for the most recent session).
        session: String,
    },
    /// Export a session's events as JSON.
    Export {
        /// Session ID (or "latest" for the most recent session).
        session: String,
        /// Output path (defaults to stdout).
        output: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
enum ExplainCommand {
    /// Explain why a specific action was allowed or denied.
    Action {
        /// Session ID (or "latest" for the most recent session).
        session: String,
        /// Action ID to explain.
        action_id: String,
    },
    /// Explain why a session completed, failed, or was cancelled.
    Completion {
        /// Session ID (or "latest" for the most recent session).
        session: String,
    },
}

#[derive(Subcommand)]
enum BundleCommand {
    /// Export a session as an evidence bundle.
    Export {
        /// Session ID (or "latest" for the most recent session).
        session: String,
        /// Output path for the bundle JSON.
        output: PathBuf,
        /// Include sensitive fields (prompts, model output, file contents).
        #[arg(long)]
        include_sensitive: bool,
    },
    /// Inspect an evidence bundle file.
    Inspect {
        /// Path to the bundle JSON file.
        path: PathBuf,
    },
    /// Verify the integrity of an evidence bundle.
    Verify {
        /// Path to the bundle JSON file.
        path: PathBuf,
    },
    /// Replay a bundle's events without executing any actions.
    Replay {
        /// Path to the bundle JSON file.
        path: PathBuf,
    },
}

#[derive(Subcommand)]
enum BenchmarkCommand {
    /// Validate every benchmark case against the schema.
    #[command(alias = "validate-cases")]
    Audit {
        #[arg(long)]
        catalog: Option<PathBuf>,
    },
    Baseline {
        #[arg(long)]
        catalog: Option<PathBuf>,
    },
    /// Drive coding fixtures through the running daemon and score real agent outcomes.
    #[command(alias = "run")]
    Live {
        #[arg(long)]
        catalog: Option<PathBuf>,
        #[arg(long)]
        max_tasks: Option<usize>,
        /// Whole-task deadline for each benchmark case.
        #[arg(long, default_value_t = 300)]
        timeout_seconds: u64,
        /// Output path for the benchmark report JSON (defaults to stdout).
        #[arg(long)]
        output: Option<PathBuf>,
    },
    /// List available benchmark case IDs and categories.
    List,
    /// Compare two benchmark reports.
    Compare {
        baseline: PathBuf,
        candidate: PathBuf,
    },
    /// Render a benchmark JSON report as a human-readable summary.
    Report {
        /// Path to the benchmark JSON report file.
        report: PathBuf,
    },
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn")),
        )
        .without_time()
        .init();
    let cli = Cli::parse();
    // `purrcode ide` opens a native desktop window. macOS and Windows both
    // require that event loop to own the *bare* main thread: started from
    // inside a Tokio runtime it is handed a run loop that is not the
    // application's, and the window closes again the moment it appears. So the
    // IDE is dispatched here, before any runtime exists, and does its async
    // preparation on a runtime that is dropped before the window opens.
    if matches!(cli.command, Some(Command::Gui { .. })) {
        return open_ide(cli);
    }
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("start the async runtime")?
        .block_on(dispatch(cli))
}

/// Prepare the daemon and the session, then hand the main thread to the IDE.
fn open_ide(cli: Cli) -> Result<()> {
    let Some(Command::Gui {
        session,
        repository,
    }) = cli.command
    else {
        unreachable!("open_ide is only reached for the gui command");
    };
    let config_path = cli.config.unwrap_or(default_config_path()?);
    if !config_path.is_file() {
        bail!("PurrCode is not initialized; run `purrcode init` before opening the IDE");
    }
    let daemon_token = cli.daemon_token.unwrap_or(default_daemon_token_path()?);
    let daemon_url = cli.daemon_url;
    let database = cli.database.unwrap_or(default_database_path()?);
    // A named folder is an instruction; no folder is a question, and the window
    // asks it on its start screen rather than silently adopting whatever
    // directory the shell happened to be in.
    let repository = match repository {
        Some(path) => Some(canonical_repository(Some(path))?),
        None => None,
    };
    if let Some(session) = session.as_deref() {
        uuid::Uuid::parse_str(session).context("session ID is not a UUID")?;
    }

    let session = {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .context("start the async runtime")?;
        let resolved = runtime.block_on(async {
            ensure_daemon_started(&config_path, &database, &daemon_url, &daemon_token, false)
                .await?;
            let resolved = match (repository.as_deref(), session.as_deref()) {
                (Some(repository), Some(session)) => {
                    let store = SessionStore::open(&database)
                        .with_context(|| format!("open session store {}", database.display()))?;
                    validate_ide_session(&store, session, repository)?;
                    Some(session.to_owned())
                }
                (Some(_), None) => None,
                (None, Some(_)) => {
                    bail!(
                        "--session requires --repository so PurrCode can verify workspace ownership"
                    )
                }
                // Without a folder there is no session to resume into.
                (None, None) => None,
            };
            Ok::<_, anyhow::Error>(resolved)
        })?;
        // The runtime is dropped here on purpose: the window must not open
        // inside it.
        runtime.shutdown_background();
        resolved
    };

    launch_ide(
        repository.as_deref(),
        session.as_deref(),
        &daemon_url,
        &daemon_token,
    )
}

async fn dispatch(cli: Cli) -> Result<()> {
    let config_path = cli.config.unwrap_or(default_config_path()?);
    let daemon_url = cli.daemon_url;
    let daemon_token = cli.daemon_token.unwrap_or(default_daemon_token_path()?);
    let requested_database = cli.database;
    let Some(command) = cli.command else {
        // When no config exists yet, launch the TUI with a minimal empty config
        // so the provider/model onboarding overlay can guide setup interactively
        // instead of exiting with "run purrcode init" (PRD §3.1, §7.2, §8).
        if !config_path.is_file() {
            let database = requested_database
                .clone()
                .unwrap_or(default_database_path()?);
            if let Some(parent) = database.parent() {
                fs::create_dir_all(parent)?;
            }
            let store = SessionStore::open(&database)?;
            drop(store);
            let repository = resolve_product_repository(None)?;
            run_tui(
                &config_path,
                &database,
                daemon_url,
                daemon_token,
                repository,
            )
            .await?;
            return Ok(());
        }
        let database = requested_database
            .clone()
            .unwrap_or(default_database_path()?);
        let repository = resolve_product_repository(None)?;
        // Bare `purrcode` launches the TUI Workbench as the default experience,
        // on every platform. Studio is intentionally opt-in via `purrcode studio`
        // or opened from inside the TUI — see PRD §3.1. Display detection no
        // longer changes the default interface.
        run_tui(
            &config_path,
            &database,
            daemon_url,
            daemon_token,
            repository,
        )
        .await?;
        return Ok(());
    };
    let database = requested_database.unwrap_or(default_database_path()?);
    if let Some(parent) = database.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    let mut store = SessionStore::open(&database)
        .with_context(|| format!("open session store {}", database.display()))?;
    match command {
        Command::Init { force, no_start } => {
            initialize_product(
                &config_path,
                &database,
                &daemon_token,
                &daemon_url,
                force,
                no_start,
            )
            .await?;
        }
        Command::Studio {
            remote,
            bind,
            no_open,
            repository,
        } => {
            if !config_path.is_file() {
                bail!("PurrCode is not initialized; run `purrcode init`");
            }
            let repository = resolve_product_repository(repository)?;
            run_studio(
                &config_path,
                &database,
                &daemon_url,
                &daemon_token,
                repository,
                remote,
                bind,
                no_open,
            )
            .await?;
        }
        Command::Ui {
            remote,
            bind,
            no_open,
            repository,
        } => {
            // Backward-compatible alias of `purrcode studio` — same handler,
            // so existing automation and muscle memory keep working.
            if !config_path.is_file() {
                bail!("PurrCode is not initialized; run `purrcode init`");
            }
            let repository = resolve_product_repository(repository)?;
            run_studio(
                &config_path,
                &database,
                &daemon_url,
                &daemon_token,
                repository,
                remote,
                bind,
                no_open,
            )
            .await?;
        }
        Command::Tui { repository } => {
            if !config_path.is_file() {
                bail!("PurrCode is not initialized; run `purrcode init`");
            }
            let repository = resolve_product_repository(repository)?;
            run_tui(
                &config_path,
                &database,
                daemon_url,
                daemon_token,
                repository,
            )
            .await?;
        }
        // Handled by `open_ide` before the runtime starts, so that the desktop
        // event loop owns the bare main thread.
        Command::Gui { .. } => unreachable!("the gui command is dispatched from main"),
        Command::Run {
            objective,
            repository,
            workflow,
            search,
            budget,
            max_tokens,
        } => {
            if !config_path.is_file() {
                bail!("PurrCode is not initialized; run `purrcode init` before `purrcode run`");
            }
            ensure_daemon_started(&config_path, &database, &daemon_url, &daemon_token, false)
                .await?;
            let repository = canonical_repository(repository)?;
            let result = daemon_json(
                reqwest::Method::POST,
                &daemon_url,
                "/v1/sessions",
                Some(serde_json::json!({
                    "objective": objective,
                    "repository": repository,
                    "task_mode": "build",
                    "permission_mode": "ask",
                    "authority_mode": "governed",
                    "plan_only": false,
                    "workflow": workflow,
                    "search_policy": search,
                    "budget_profile": budget,
                    "max_tokens": max_tokens,
                })),
                &daemon_token,
            )
            .await?;
            print_accepted_session(&result)?;
        }
        Command::Plan {
            objective,
            repository,
            workflow,
            search,
            budget,
            max_tokens,
        } => {
            if !config_path.is_file() {
                bail!("PurrCode is not initialized; run `purrcode init` before `purrcode plan`");
            }
            ensure_daemon_started(&config_path, &database, &daemon_url, &daemon_token, false)
                .await?;
            let repository = canonical_repository(repository)?;
            let result = daemon_json(
                reqwest::Method::POST,
                &daemon_url,
                "/v1/sessions",
                Some(serde_json::json!({
                    "objective": objective,
                    "repository": repository,
                    "task_mode": "plan",
                    "permission_mode": "ask",
                    "authority_mode": "governed",
                    "plan_only": true,
                    "workflow": workflow,
                    "search_policy": search,
                    "budget_profile": budget,
                    "max_tokens": max_tokens,
                })),
                &daemon_token,
            )
            .await?;
            print_accepted_session(&result)?;
        }
        Command::Ci {
            objective,
            repository,
            timeout_seconds,
            output,
        } => {
            let repository = canonical_repository(repository)?;
            let accepted = daemon_json(
                reqwest::Method::POST,
                &daemon_url,
                "/v1/sessions",
                Some(serde_json::json!({
                    "objective": objective,
                    "repository": repository
                })),
                &daemon_token,
            )
            .await?;
            let session = accepted["id"]
                .as_str()
                .context("daemon response omitted session ID")?
                .to_owned();
            let deadline =
                tokio::time::Instant::now() + std::time::Duration::from_secs(timeout_seconds);
            let final_view = loop {
                let view = daemon_json(
                    reqwest::Method::GET,
                    &daemon_url,
                    &format!("/v1/sessions/{session}"),
                    None,
                    &daemon_token,
                )
                .await?;
                match view["status_code"].as_str().unwrap_or("unknown") {
                    "awaiting_approval" | "awaiting_review" => {
                        daemon_json(
                            reqwest::Method::POST,
                            &daemon_url,
                            &format!("/v1/sessions/{session}/cancel"),
                            Some(serde_json::json!({
                                "reason":"headless CI denies actions requiring interactive approval"
                            })),
                            &daemon_token,
                        )
                        .await?;
                        break daemon_json(
                            reqwest::Method::GET,
                            &daemon_url,
                            &format!("/v1/sessions/{session}"),
                            None,
                            &daemon_token,
                        )
                        .await?;
                    }
                    "completed" | "failed" | "cancelled" | "uncertain" => break view,
                    _ if tokio::time::Instant::now() >= deadline => {
                        daemon_json(
                            reqwest::Method::POST,
                            &daemon_url,
                            &format!("/v1/sessions/{session}/cancel"),
                            Some(serde_json::json!({
                                "reason":"headless CI whole-session timeout"
                            })),
                            &daemon_token,
                        )
                        .await?;
                        break daemon_json(
                            reqwest::Method::GET,
                            &daemon_url,
                            &format!("/v1/sessions/{session}"),
                            None,
                            &daemon_token,
                        )
                        .await?;
                    }
                    _ => tokio::time::sleep(std::time::Duration::from_millis(250)).await,
                }
            };
            let events_value = daemon_json(
                reqwest::Method::GET,
                &daemon_url,
                &format!("/v1/sessions/{session}/events"),
                None,
                &daemon_token,
            )
            .await?;
            let events: Vec<SessionEvent> = serde_json::from_value(events_value)?;
            let report = CiReport::from_events(session, final_view, events);
            let encoded = serde_json::to_string_pretty(&report)?;
            if let Some(path) = output {
                write_new_atomic(&path, encoded.as_bytes())?;
                println!("report: {}", path.display());
            } else {
                println!("{encoded}");
            }
            if report.status != "completed" {
                bail!("headless CI session ended with status {}", report.status);
            }
        }
        Command::PolicyCheck {
            repository,
            program,
            arguments,
        } => {
            let repository = canonical_repository(repository)?;
            let policy = load_policy(&repository, &config_path)?;
            let action = command_action(program, arguments, &repository);
            println!(
                "{}",
                serde_json::to_string_pretty(&policy.evaluate(&action, &repository))?
            );
        }
        Command::Exec {
            repository,
            program,
            arguments,
        } => {
            let repository = canonical_repository(repository)?;
            let policy = load_policy(&repository, &config_path)?;
            let action = command_action(program, arguments, &repository);
            let session_id = SessionId::new();
            let action_id = ActionId::new();
            store.append(
                session_id,
                &SessionEvent::SessionCreated {
                    objective: "user-requested guarded command".into(),
                    repository: repository.clone(),
                    authority_mode: Default::default(),
                },
            )?;
            store.append(
                session_id,
                &SessionEvent::ActionProposed {
                    action_id,
                    action: action.clone(),
                    turn_id: None,
                },
            )?;
            let decision = policy.evaluate(&action, &repository);
            store.append(
                session_id,
                &SessionEvent::JudgmentRecorded {
                    action_id,
                    decision: decision.clone(),
                    turn_id: None,
                },
            )?;
            let constraints = match decision {
                JudgmentDecision::AllowWithConstraints(constraints) => constraints,
                JudgmentDecision::Allow => {
                    bail!("policy returned an unconstrained allow; execution fails closed")
                }
                JudgmentDecision::RequireApproval { reason, .. } => {
                    bail!("approval required: {reason}")
                }
                JudgmentDecision::Deny { reason } => bail!("denied: {reason}"),
                other => bail!("action cannot execute: {other:?}"),
            };
            let authorization = Authorization {
                action_id,
                session_id,
                action_digest: action.digest(&constraints)?,
                constraints: constraints.clone(),
                authorized_at: Utc::now(),
                approved_by: ApprovalAuthority::DeterministicPolicy,
            };
            store.authorize(&authorization)?;
            store.append(session_id, &SessionEvent::ExecutionStarted { action_id })?;
            let result = ToolRuntime::execute(&mut store, action_id, &action, &constraints).await;
            match result {
                Ok(result) => {
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
                    let validation = if result.exit_code == Some(0) {
                        ValidationStatus::Passed
                    } else {
                        ValidationStatus::Failed
                    };
                    store.append(
                        session_id,
                        &SessionEvent::ValidationRecorded {
                            action_id,
                            status: validation,
                            evidence: format!("process exit code: {:?}", result.exit_code),
                        },
                    )?;
                    print!("{}", String::from_utf8_lossy(&result.stdout));
                    eprint!("{}", String::from_utf8_lossy(&result.stderr));
                    if result.truncated {
                        eprintln!("\n[output truncated at authorized byte limit]");
                    }
                    eprintln!(
                        "[sandbox: {:?}, backend: {}]",
                        result.sandbox_level, result.sandbox_backend
                    );
                    if result.exit_code != Some(0) {
                        bail!("command failed with exit code {:?}", result.exit_code);
                    }
                }
                Err(error) => {
                    store.append(
                        session_id,
                        &SessionEvent::ValidationRecorded {
                            action_id,
                            status: ValidationStatus::Uncertain,
                            evidence: error.to_string(),
                        },
                    )?;
                    return Err(error.into());
                }
            }
        }
        Command::Doctor { repository } => {
            println!("database: {}", database.display());
            println!(
                "sqlite_integrity: {}",
                if store.integrity_check()? {
                    "ok"
                } else {
                    "failed"
                }
            );
            println!("daemon: use `purrcoded` for the authenticated loopback service");
            let repository = resolve_product_repository(repository)?;
            let environment = purrcode_environment_runtime::inspect_environment(
                &repository,
                &default_toolchain_root()?,
            )
            .await?;
            println!(
                "environment: {}",
                if environment.ready {
                    "ready"
                } else {
                    "repair-required"
                }
            );
            println!("{}", serde_json::to_string_pretty(&environment)?);
        }
        Command::Resume { session, tui } => {
            let session = resolve_daemon_session(&daemon_url, &daemon_token, session).await?;
            if tui {
                let view = daemon_json(
                    reqwest::Method::GET,
                    &daemon_url,
                    &format!("/v1/sessions/{session}"),
                    None,
                    &daemon_token,
                )
                .await?;
                let repository = view["worktree"]
                    .as_str()
                    .or_else(|| view["repository"].as_str())
                    .map(PathBuf::from)
                    .context("daemon session has no repository")?
                    .canonicalize()
                    .context("resolve daemon session repository")?;
                purrcode_tui::run(TuiConfig {
                    daemon_url,
                    token_file: daemon_token,
                    repository,
                    session_id: Some(session),
                })
                .await?;
                return Ok(());
            }
            let result = daemon_json(
                reqwest::Method::POST,
                &daemon_url,
                &format!("/v1/sessions/{session}/resume"),
                Some(serde_json::json!({})),
                &daemon_token,
            )
            .await?;
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
        Command::Sessions => {
            let result = daemon_json(
                reqwest::Method::GET,
                &daemon_url,
                "/v1/sessions",
                None,
                &daemon_token,
            )
            .await?;
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
        Command::Approve { session } => {
            let session = resolve_daemon_session(&daemon_url, &daemon_token, session).await?;
            let result = daemon_json(
                reqwest::Method::POST,
                &daemon_url,
                &format!("/v1/sessions/{session}/approve"),
                Some(serde_json::json!({})),
                &daemon_token,
            )
            .await?;
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
        Command::Reject { session, reason } => {
            let session = resolve_daemon_session(&daemon_url, &daemon_token, session).await?;
            let result = daemon_json(
                reqwest::Method::POST,
                &daemon_url,
                &format!("/v1/sessions/{session}/reject"),
                Some(serde_json::json!({"reason":reason})),
                &daemon_token,
            )
            .await?;
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
        Command::Cancel { session, reason } => {
            let session = resolve_daemon_session(&daemon_url, &daemon_token, session).await?;
            let result = daemon_json(
                reqwest::Method::POST,
                &daemon_url,
                &format!("/v1/sessions/{session}/cancel"),
                Some(serde_json::json!({"reason":reason})),
                &daemon_token,
            )
            .await?;
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
        Command::Review { session } | Command::Diff { session } => {
            let session_id = resolve_session_id(&store, session)?;
            let worktree = session_worktree_from_store(&store, session_id)?;
            let effects = RepositoryEngine::effects(&worktree).await?;
            println!("session: {}", session_id.0);
            println!("worktree: {}", worktree.path.display());
            println!("changed paths: {}", effects.changed_files.len());
            for path in &effects.changed_files {
                println!("  {}", path.display());
            }
            use std::io::Write;
            std::io::stdout().write_all(&effects.binary_patch)?;
        }
        Command::ExportPatch {
            destination,
            session,
        } => {
            let session_id = resolve_session_id(&store, session)?;
            let worktree = session_worktree_from_store(&store, session_id)?;
            let result = RepositoryEngine::apply_strategy(
                &worktree,
                ApplicationStrategy::ExportPatch(destination),
            )
            .await?;
            store.append(
                session_id,
                &SessionEvent::WorktreeDispositionRecorded {
                    strategy: "export_patch".into(),
                    detail: result.detail.clone(),
                },
            )?;
            println!("{}", result.detail);
        }
        Command::Apply { session } => {
            let session_id = resolve_session_id(&store, session)?;
            let worktree = session_worktree_from_store(&store, session_id)?;
            let result = RepositoryEngine::apply_strategy(
                &worktree,
                ApplicationStrategy::ApplyToCurrentTree,
            )
            .await?;
            store.append(
                session_id,
                &SessionEvent::WorktreeDispositionRecorded {
                    strategy: "apply_to_current_tree".into(),
                    detail: result.detail.clone(),
                },
            )?;
            println!("{}", result.detail);
        }
        Command::Rollback {
            session,
            expected_patch_digest,
            acknowledge_unattributed_effects,
        } => {
            let session_id = resolve_session_id(&store, session)?;
            let worktree = session_worktree_from_store(&store, session_id)?;
            let effects = RepositoryEngine::effects(&worktree).await?;
            let digest = blake3::hash(&effects.binary_patch).to_hex().to_string();
            let Some(expected) = expected_patch_digest else {
                println!("session: {}", session_id.0);
                println!("status: rollback preview only");
                println!("changed_files: {}", effects.changed_files.len());
                for path in &effects.changed_files {
                    println!("file: {}", path.display());
                }
                println!("patch_digest: {digest}");
                println!(
                    "warning: provenance of every isolated hunk cannot be proven; inspect the diff before discarding it"
                );
                println!(
                    "confirm: purrcode rollback {} --expected-patch-digest {} --acknowledge-unattributed-effects",
                    session_id.0, digest
                );
                return Ok(());
            };
            if !acknowledge_unattributed_effects {
                bail!("rollback confirmation requires --acknowledge-unattributed-effects");
            }
            if expected != digest {
                bail!(
                    "isolated changes changed after preview; run rollback again to inspect the new digest"
                );
            }
            RepositoryEngine::rollback_all(&worktree).await?;
            store.append(
                session_id,
                &SessionEvent::WorktreeDispositionRecorded {
                    strategy: "rollback_all".into(),
                    detail: format!(
                        "{} previewed isolated-worktree change(s) rolled back after exact patch-digest confirmation",
                        effects.changed_files.len()
                    ),
                },
            )?;
            println!("session: {}", session_id.0);
            println!("status: isolated changes rolled back");
        }
        Command::Model { command } => {
            let mut config = load_app_config(&config_path)?;
            match command {
                ModelCommand::List => {
                    println!(
                        "default: {}",
                        config.models.default.as_deref().unwrap_or("not selected")
                    );
                    for (name, provider) in &config.providers {
                        println!(
                            "{name}: {}",
                            if provider.is_local() {
                                "local"
                            } else {
                                "remote"
                            }
                        );
                    }
                    for (role, model) in &config.models.roles {
                        println!("role.{role}: {model}");
                    }
                }
                ModelCommand::Use { model } => {
                    let model_id = ModelId::parse(&model)?;
                    if !config.providers.contains_key(&model_id.provider) {
                        bail!("provider `{}` is not configured", model_id.provider);
                    }
                    config.models.default = Some(model.clone());
                    config.save(&config_path)?;
                    println!("default model: {model}");
                    println!("config: {}", config_path.display());
                }
                ModelCommand::Add { model } => {
                    let model = ModelId::parse(&model)?;
                    config.register_model(&model)?;
                    config.save(&config_path)?;
                    println!("registered model: {}/{}", model.provider, model.model);
                    println!("capabilities: unknown until `purrcode model qualify` succeeds");
                }
                ModelCommand::Qualify { model } => {
                    let model = ModelId::parse(&model)?;
                    let router = ProviderRouter::from_config(
                        &config,
                        Some(config_path.with_file_name("credentials.toml").as_path()),
                    )?;
                    let provider = router.provider(&model)?;
                    let report = qualify_model(provider.as_ref(), model).await?;
                    println!("{}", serde_json::to_string_pretty(&report)?);
                    if report.cases.iter().any(|case| !case.passed) {
                        bail!("model did not pass every qualification case");
                    }
                }
            }
        }
        Command::Provider { command } => match command {
            ProviderCommand::Import { path, stdin } => {
                let source = match (path, stdin) {
                    (Some(path), false) => std::fs::read_to_string(&path)
                        .with_context(|| format!("failed to read {}", path.display()))?,
                    (None, true) => {
                        use std::io::Read;
                        let mut source = String::new();
                        std::io::stdin().read_to_string(&mut source)?;
                        source
                    }
                    _ => bail!("provide a file path or --stdin"),
                };
                let candidate = purrcode_provider_import::import_provider(&source, None)?;
                println!("{}", serde_json::to_string_pretty(&candidate)?);
                println!("review_required: true");
                println!(
                    "next: review fields in `purrcode` → /connect import; no configuration was saved"
                );
            }
            ProviderCommand::Doctor => {
                let config = load_app_config(&config_path)?;
                let router = ProviderRouter::from_config(
                    &config,
                    Some(config_path.with_file_name("credentials.toml").as_path()),
                )?;
                let checks = config.providers.iter().map(|(name, configured)| {
                    let name = name.clone();
                    let model = ModelId {
                        provider: name.clone(),
                        model: "health-check".into(),
                    };
                    let local = configured.is_local();
                    let provider = router.provider(&model);
                    async move {
                        let result = match provider {
                            Ok(provider) => provider.health_check().await.map(|health| {
                                if health.available {
                                    health.detail
                                } else {
                                    format!("unavailable: {}", health.detail)
                                }
                            }),
                            Err(error) => Err(error),
                        };
                        (name, local, result)
                    }
                });
                let mut failed = false;
                for (name, local, result) in join_all(checks).await {
                    match result {
                        Ok(detail) => println!(
                            "{name} [{}]: {detail}",
                            if local { "local" } else { "remote" }
                        ),
                        Err(error) => {
                            failed = true;
                            println!("{name}: error: {error}");
                        }
                    }
                }
                if failed {
                    bail!("one or more provider checks failed");
                }
            }
        },
        Command::Credential { command } => match command {
            CredentialCommand::Set { provider, name } => {
                let credential_name = name.unwrap_or_else(|| provider.clone());
                let mut secret = rpassword::prompt_password(format!(
                    "API key for provider `{provider}` (input hidden): "
                ))?;
                if secret.trim().is_empty() {
                    bail!("empty API keys are not accepted");
                }
                let mut config = load_app_config(&config_path)?;
                config.use_keychain_credential(&provider, &credential_name)?;
                let credentials_path = config_path.with_file_name("credentials.toml");
                let stored = store_credential(&credentials_path, &credential_name, &secret);
                secret.zeroize();
                stored?;
                if let Err(error) = config.save(&config_path) {
                    let _ = delete_credential(&credentials_path, &credential_name);
                    return Err(error.into());
                }
                println!("credential stored in credentials.toml next to config.toml");
                println!("provider: {provider}");
                println!("config: {}", config_path.display());
            }
            CredentialCommand::SetEnv { provider, env_var } => {
                let mut config = load_app_config(&config_path)?;
                config.use_environment_credential(&provider, &env_var)?;
                config.save(&config_path)?;
                println!("provider `{provider}` will use the `{env_var}` environment variable");
                println!(
                    "set it in your shell profile or .env file — the daemon reads it at startup"
                );
                println!("config: {}", config_path.display());
            }
            CredentialCommand::Delete { name } => {
                let credentials_path = config_path.with_file_name("credentials.toml");
                delete_credential(&credentials_path, &name)?;
                println!("credential `{name}` removed from credentials.toml next to config.toml");
            }
        },
        Command::Config { command } => match command {
            ConfigCommand::MigrationPreview => {
                let (current, target) = AppConfig::migration_preview(&config_path)?;
                println!("current schema: {current}");
                println!("target schema: {target}");
                println!("pending migrations: {}", usize::from(current != target));
            }
            ConfigCommand::Migrate => match AppConfig::migrate_file(&config_path)? {
                Some(backup) => {
                    println!("configuration migrated to schema 1");
                    println!("backup: {}", backup.display());
                }
                None => println!("configuration is already at schema 1"),
            },
        },
        Command::Upgrade { command } => match command {
            UpgradeCommand::Check { channel } => {
                let release = fetch_release(&channel).await?;
                let current = semver::Version::parse(env!("CARGO_PKG_VERSION"))?;
                let available = semver::Version::parse(release.tag_name.trim_start_matches('v'))?;
                println!("current: {current}");
                println!("available: {available}");
                println!("channel: {channel}");
                println!(
                    "status: {}",
                    if available > current {
                        "update available"
                    } else {
                        "up to date"
                    }
                );
            }
            UpgradeCommand::Download {
                destination,
                channel,
            } => {
                let release = fetch_release(&channel).await?;
                let downloaded = download_verified_release(&release, &destination).await?;
                println!("verified signed release: {}", downloaded.display());
            }
            UpgradeCommand::Install {
                channel,
                destination,
            } => {
                let compatible_plugins =
                    check_upgrade_plugin_compatibility(&std::env::current_dir()?)?;
                let release = fetch_release(&channel).await?;
                let temporary = tempfile::tempdir()?;
                let archive = temporary.path().join(release_artifact_name()?);
                download_verified_release(&release, &archive).await?;
                let extracted = extract_release_archive(&archive, temporary.path()).await?;
                let destination = destination.unwrap_or(current_binary_directory()?);
                install_staged_binaries(&extracted, &destination)?;
                println!("installed signed release {}", release.tag_name);
                println!("destination: {}", destination.display());
                println!("compatible repository skills: {compatible_plugins}");
                println!("rollback: purrcode upgrade rollback");
            }
            UpgradeCommand::Rollback { destination } => {
                let destination = destination.unwrap_or(current_binary_directory()?);
                rollback_binaries(&destination)?;
                println!("restored the previously installed PurrCode binaries");
                println!("destination: {}", destination.display());
            }
        },
        Command::Automation { command } => {
            let (method, path, body) = match command {
                AutomationCommand::List => (reqwest::Method::GET, "/v1/automations".into(), None),
                AutomationCommand::Create {
                    objective,
                    repository,
                    every_seconds,
                } => (
                    reqwest::Method::POST,
                    "/v1/automations".into(),
                    Some(serde_json::json!({
                        "objective": objective,
                        "repository": canonical_repository(repository)?,
                        "interval_seconds": every_seconds
                    })),
                ),
                AutomationCommand::Enable { id } => (
                    reqwest::Method::POST,
                    format!("/v1/automations/{id}/enable"),
                    Some(serde_json::json!({})),
                ),
                AutomationCommand::Disable { id } => (
                    reqwest::Method::POST,
                    format!("/v1/automations/{id}/disable"),
                    Some(serde_json::json!({})),
                ),
                AutomationCommand::Run { id } => (
                    reqwest::Method::POST,
                    format!("/v1/automations/{id}/run"),
                    Some(serde_json::json!({})),
                ),
            };
            let result = daemon_json(method, &daemon_url, &path, body, &daemon_token).await?;
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
        Command::Parallel {
            objective,
            workers,
            repository,
        } => {
            let workers: serde_json::Value = serde_json::from_slice(
                &fs::read(&workers)
                    .with_context(|| format!("read worker specification {}", workers.display()))?,
            )
            .context("worker specification must be a JSON array")?;
            if !workers.is_array() {
                bail!("worker specification must be a JSON array");
            }
            let result = daemon_json(
                reqwest::Method::POST,
                &daemon_url,
                "/v1/supervisor",
                Some(serde_json::json!({
                    "objective": objective,
                    "repository": canonical_repository(repository)?,
                    "workers": workers,
                    "limits": {
                        "max_workers": 3,
                        "max_model_requests": 6,
                        "max_worktrees": 4,
                        "require_isolation": true
                    }
                })),
                &daemon_token,
            )
            .await?;
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
        Command::Codex { command } => {
            let codex = load_codex_config(&config_path)?;
            let bridge = CodexBridge::new(codex)?;
            match command {
                CodexCommand::Doctor => {
                    println!("{}", serde_json::to_string_pretty(&bridge.doctor().await?)?);
                }
                CodexCommand::Run {
                    objective,
                    repository,
                } => {
                    let repository = canonical_repository(repository)?;
                    let result = bridge.run(&repository, &objective).await?;
                    store.append(
                        result.session_id,
                        &SessionEvent::SessionCreated {
                            objective,
                            repository,
                            authority_mode: Default::default(),
                        },
                    )?;
                    store.append(
                        result.session_id,
                        &SessionEvent::WorktreeCreated {
                            path: result.worktree.path.clone(),
                            base_head: result.worktree.base_head.clone(),
                            source_was_dirty: result.worktree.source_was_dirty,
                        },
                    )?;
                    store.append(
                        result.session_id,
                        &SessionEvent::SubmodulesPrepared {
                            initialized: result.worktree.initialized_submodules.clone(),
                            unavailable: result.worktree.unavailable_submodules.clone(),
                        },
                    )?;
                    println!("session: {}", result.session_id.0);
                    println!("worktree: {}", result.worktree.path.display());
                    println!("events: {}", result.events.len());
                    println!("dropped events: {}", result.dropped_events);
                    println!("changed paths: {}", result.effects.changed_files.len());
                    println!("status: independent diff judgment required");
                }
            }
        }
        Command::Database { command } => match command {
            DatabaseCommand::Backup { destination } => {
                store.backup(&destination)?;
                println!("backup: {}", destination.display());
                println!("integrity: ok");
            }
            DatabaseCommand::MigrationPreview => {
                println!("current schema: {}", store.schema_version()?);
                println!("target schema: 2");
                println!("pending migrations: 0");
            }
        },
        Command::Sandbox { command } => match command {
            SandboxCommand::Doctor => {
                let capability = sandbox_capability();
                println!("level: {:?}", capability.level);
                println!("backend: {}", capability.backend);
                println!("network isolation: {}", capability.network_isolation);
                println!(
                    "process-group termination: {}",
                    capability.process_group_termination
                );
                if !capability.network_isolation {
                    println!("warning: command filtering is not full operating-system isolation");
                }
            }
        },
        Command::Serve {
            bind,
            allow_public_bind,
        } => {
            let (report, server) = bind_and_report(DaemonConfig {
                bind,
                allow_public_bind,
                database,
                token_file: default_daemon_token_path()?,
                app_config: config_path,
            })
            .await?;
            println!("{}", serde_json::to_string_pretty(&report)?);
            server.await?;
        }
        Command::Skill { command } => match command {
            SkillCommand::List { repository } => {
                let repository = canonical_repository(repository)?;
                let roots = [
                    repository.join(".purrcode/skills"),
                    repository.join(".codex/skills"),
                ];
                let mut count = 0;
                for root in roots {
                    for skill in discover_skills(&root)? {
                        count += 1;
                        println!(
                            "{}\t{}\t{}\tnetwork={}",
                            skill.manifest.name,
                            skill.manifest.version,
                            skill.root.display(),
                            skill.manifest.network_access
                        );
                    }
                }
                println!("skills: {count}");
            }
            SkillCommand::Install { source, repository } => {
                let _ = (source, repository);
                bail!(
                    "direct skill installation is disabled because it bypasses daemon qualification and approval. Run `purrcode tui`, then `/skill-download <candidate_id> <40-character-commit>`, `/skill-download-approve <candidate_id> <40-character-commit>`, `/skill-install <user|repository|session>`, and `/skill-install-approve`; no files were changed"
                );
            }
            SkillCommand::Verify { name, repository } => {
                let repository = canonical_repository(repository)?;
                let record =
                    verify_installed_skill(&repository.join(".purrcode/skills").join(name))?;
                println!("{}", serde_json::to_string_pretty(&record)?);
            }
            SkillCommand::Remove { name, repository } => {
                let repository = canonical_repository(repository)?;
                let moved = uninstall_skill(&name, &repository.join(".purrcode/skills"))?;
                println!("skill moved to recoverable trash: {}", moved.display());
            }
        },
        Command::Mcp { command } => {
            let repository = canonical_repository(None)?;
            let servers = load_mcp_servers(&config_path)?;
            let (server_name, tool_name, arguments, approved, discovery) = match command {
                McpCommand::Discover { server, approve } => (
                    server,
                    "__discover__".into(),
                    serde_json::json!({}),
                    approve,
                    true,
                ),
                McpCommand::Call {
                    server,
                    tool,
                    arguments,
                    approve,
                } => (
                    server,
                    tool,
                    serde_json::from_str(&arguments).context("--arguments must be JSON")?,
                    approve,
                    false,
                ),
            };
            let server = servers
                .get(&server_name)
                .with_context(|| format!("MCP server `{server_name}` is not configured"))?;
            if server.working_directory != repository {
                bail!("MCP server working_directory must exactly match the current repository");
            }
            let action =
                McpHost::translate(&server_name, &tool_name, arguments, repository.clone());
            let policy = load_policy(&repository, &config_path)?;
            let decision = policy.evaluate(&action, &repository);
            let constraints = match &decision {
                JudgmentDecision::RequireApproval { constraints, .. } if approved => {
                    constraints.clone()
                }
                JudgmentDecision::RequireApproval { reason, .. } => {
                    println!("approval required: {reason}");
                    println!("action: {}", serde_json::to_string_pretty(&action)?);
                    bail!("repeat with --approve after reviewing the exact action")
                }
                JudgmentDecision::Deny { reason } => bail!("denied: {reason}"),
                other => bail!("unexpected MCP judgment: {other:?}"),
            };
            let session_id = SessionId::new();
            let action_id = ActionId::new();
            store.append(
                session_id,
                &SessionEvent::SessionCreated {
                    objective: format!("MCP {server_name}/{tool_name}"),
                    repository,
                    authority_mode: Default::default(),
                },
            )?;
            store.append(
                session_id,
                &SessionEvent::ActionProposed {
                    action_id,
                    action: action.clone(),
                    turn_id: None,
                },
            )?;
            store.append(
                session_id,
                &SessionEvent::JudgmentRecorded {
                    action_id,
                    decision,
                    turn_id: None,
                },
            )?;
            store.authorize(&Authorization {
                action_id,
                session_id,
                action_digest: action.digest(&constraints)?,
                constraints: constraints.clone(),
                authorized_at: Utc::now(),
                approved_by: ApprovalAuthority::Human,
            })?;
            store.append(session_id, &SessionEvent::ExecutionStarted { action_id })?;
            if discovery {
                let tools =
                    McpHost::discover_tools(&mut store, action_id, &action, &constraints, server)
                        .await?;
                println!("{}", serde_json::to_string_pretty(&tools)?);
            } else {
                let result =
                    McpHost::call(&mut store, action_id, &action, &constraints, server).await?;
                println!("{}", serde_json::to_string_pretty(&result)?);
            }
            store.append(
                session_id,
                &SessionEvent::ExecutionFinished {
                    action_id,
                    exit_code: Some(0),
                    truncated: false,
                    sandbox_level: Some("external-plugin-isolation".into()),
                    sandbox_backend: Some("mcp-host-child".into()),
                },
            )?;
            store.append(
                session_id,
                &SessionEvent::ValidationRecorded {
                    action_id,
                    status: ValidationStatus::Passed,
                    evidence:
                        "MCP response matched JSON-RPC identity; exact authorization was consumed"
                            .into(),
                },
            )?;
        }
        Command::Research { command } => match command {
            ResearchCommand::Export { output, redacted } => {
                let store = SessionStore::open(&database).with_context(|| {
                    format!("cannot open session store at {}", database.display())
                })?;
                let all_session_ids: Vec<SessionId> = store
                    .list_session_ids()
                    .map_err(|e| anyhow::anyhow!("list sessions failed: {e}"))?;
                let mut all_events = Vec::new();
                for sid in &all_session_ids {
                    if let Ok(events) = store.timestamped_events(*sid) {
                        for (timestamp, event) in &events {
                            match event {
                                SessionEvent::ResearchSearchPerformed {
                                    query,
                                    url,
                                    content_digest,
                                    excerpt,
                                } => {
                                    all_events.push(ResearchEvent {
                                        event_type: "ResearchSearchPerformed".into(),
                                        timestamp: *timestamp,
                                        session_id: *sid,
                                        data: if redacted {
                                            serde_json::json!({ "content_digest": content_digest })
                                        } else {
                                            serde_json::json!({
                                                "query": query,
                                                "url": url,
                                                "content_digest": content_digest,
                                                "excerpt": excerpt,
                                            })
                                        },
                                    });
                                }
                                SessionEvent::CapabilityGapDetected {
                                    gap_description,
                                    task_context,
                                } => {
                                    all_events.push(ResearchEvent {
                                        event_type: "CapabilityGapDetected".into(),
                                        timestamp: *timestamp,
                                        session_id: *sid,
                                        data: if redacted {
                                            serde_json::json!({})
                                        } else {
                                            serde_json::json!({ "gap_description": gap_description, "task_context": task_context })
                                        },
                                    });
                                }
                                SessionEvent::SkillInvoked {
                                    skill_id,
                                    tool_name,
                                } => {
                                    all_events.push(ResearchEvent {
                                        event_type: "SkillInvoked".into(),
                                        timestamp: *timestamp,
                                        session_id: *sid,
                                        data: if redacted {
                                            serde_json::json!({ "skill_id": redacted_identifier(skill_id) })
                                        } else {
                                            serde_json::json!({ "skill_id": skill_id, "tool_name": tool_name })
                                        },
                                    });
                                }
                                SessionEvent::SkillInstallApproved { skill_id, scope } => {
                                    all_events.push(ResearchEvent {
                                        event_type: "SkillInstallApproved".into(),
                                        timestamp: *timestamp,
                                        session_id: *sid,
                                        data: if redacted {
                                            serde_json::json!({ "skill_id": redacted_identifier(skill_id) })
                                        } else {
                                            serde_json::json!({ "skill_id": skill_id, "scope": scope })
                                        },
                                    });
                                }
                                SessionEvent::SkillSearchStarted { query, sources } => all_events.push(ResearchEvent {
                                    event_type: "SkillSearchStarted".into(), timestamp: *timestamp, session_id: *sid,
                                    data: if redacted { serde_json::json!({"source_count": sources.len()}) } else { serde_json::json!({"query": query, "sources": sources}) },
                                }),
                                SessionEvent::SkillCandidateDiscovered { candidate_id, source, rank } => all_events.push(ResearchEvent {
                                    event_type: "SkillCandidateDiscovered".into(), timestamp: *timestamp, session_id: *sid,
                                    data: if redacted { serde_json::json!({"candidate_id": redacted_identifier(candidate_id), "rank": rank}) } else { serde_json::json!({"candidate_id": candidate_id, "source": source, "rank": rank}) },
                                }),
                                SessionEvent::SkillInspectionOpened { skill_id, duration_ms } => all_events.push(ResearchEvent {
                                    event_type: "SkillInspectionOpened".into(), timestamp: *timestamp, session_id: *sid,
                                    data: serde_json::json!({"skill_id": if redacted { redacted_identifier(skill_id) } else { skill_id.clone() }, "duration_ms": duration_ms}),
                                }),
                                SessionEvent::SkillQualified { skill_id, status, latency_ms } => all_events.push(ResearchEvent {
                                    event_type: "SkillQualified".into(), timestamp: *timestamp, session_id: *sid,
                                    data: serde_json::json!({"skill_id": if redacted { redacted_identifier(skill_id) } else { skill_id.clone() }, "status": status, "latency_ms": latency_ms}),
                                }),
                                SessionEvent::SkillQualificationStarted { skill_id } => all_events.push(ResearchEvent {
                                    event_type: "SkillQualificationStarted".into(), timestamp: *timestamp, session_id: *sid,
                                    data: serde_json::json!({"skill_id": if redacted { redacted_identifier(skill_id) } else { skill_id.clone() }}),
                                }),
                                SessionEvent::SkillInvocationSucceeded { skill_id, latency_ms } => all_events.push(ResearchEvent {
                                    event_type: "SkillInvocationSucceeded".into(), timestamp: *timestamp, session_id: *sid,
                                    data: serde_json::json!({"skill_id": if redacted { redacted_identifier(skill_id) } else { skill_id.clone() }, "latency_ms": latency_ms}),
                                }),
                                SessionEvent::InstalledSkillMatched { skill_id, matched_capability } => all_events.push(ResearchEvent {
                                    event_type: "InstalledSkillMatched".into(), timestamp: *timestamp, session_id: *sid,
                                    data: if redacted { serde_json::json!({"skill_id": redacted_identifier(skill_id)}) } else { serde_json::json!({"skill_id": skill_id, "matched_capability": matched_capability}) },
                                }),
                                SessionEvent::InstalledSkillReused { skill_id, previous_uses } => all_events.push(ResearchEvent {
                                    event_type: "InstalledSkillReused".into(), timestamp: *timestamp, session_id: *sid,
                                    data: serde_json::json!({"skill_id": if redacted { redacted_identifier(skill_id) } else { skill_id.clone() }, "previous_uses": previous_uses}),
                                }),
                                SessionEvent::ExternalSearchAvoided { skill_id, matched_capability } => all_events.push(ResearchEvent {
                                    event_type: "ExternalSearchAvoided".into(), timestamp: *timestamp, session_id: *sid,
                                    data: if redacted { serde_json::json!({"skill_id": redacted_identifier(skill_id)}) } else { serde_json::json!({"skill_id": skill_id, "matched_capability": matched_capability}) },
                                }),
                                SessionEvent::SessionCompleted => all_events.push(ResearchEvent {
                                    event_type: "TaskCompleted".into(), timestamp: *timestamp, session_id: *sid, data: serde_json::json!({}),
                                }),
                                _ => {}
                            }
                        }
                    }
                }
                let metrics = ResearchMetrics {
                    total_skill_invocations: all_events
                        .iter()
                        .filter(|e| e.event_type == "SkillInvoked")
                        .count() as u64,
                    total_skill_installations: all_events
                        .iter()
                        .filter(|e| e.event_type == "SkillInstallApproved")
                        .count() as u64,
                    total_capability_gaps: all_events
                        .iter()
                        .filter(|e| e.event_type == "CapabilityGapDetected")
                        .count() as u64,
                    total_external_searches: all_events
                        .iter()
                        .filter(|e| e.event_type == "ResearchSearchPerformed")
                        .count() as u64,
                    ..Default::default()
                };
                let export = ResearchExport {
                    exported_at: Utc::now(),
                    session_count: all_session_ids.len(),
                    events: all_events,
                    metrics,
                    redacted,
                };
                let json = serde_json::to_string_pretty(&export)?;
                match &output {
                    Some(path) => fs::write(path, &json)?,
                    None => println!("{json}"),
                }
            }
        },
        Command::Trace { command } => match command {
            TraceCommand::Show { session } => {
                let session_id = resolve_session(&store, &session)?;
                let events = store.timestamped_events(session_id)?;
                println!("Session: {}  ({} events)", session_id.0, events.len());
                for (i, (timestamp, event)) in events.iter().enumerate() {
                    let event_type = event_type_name(event);
                    let action_id = extract_action_id(event)
                        .map(|id| format!(" action={}", id.0))
                        .unwrap_or_default();
                    let summary = event_summary(event);
                    println!(
                        "  [{:3}] {}{}",
                        i + 1,
                        timestamp.format("%H:%M:%S%.3f"),
                        action_id
                    );
                    println!("         {event_type}");
                    if !summary.is_empty() {
                        println!("         {summary}");
                    }
                }
            }
            TraceCommand::Export { session, output } => {
                let session_id = resolve_session(&store, &session)?;
                let events = store.timestamped_events(session_id)?;
                let json = serde_json::to_string_pretty(&events)?;
                match output {
                    Some(path) => fs::write(&path, &json)?,
                    None => println!("{json}"),
                }
            }
        },
        Command::Explain { command } => match command {
            ExplainCommand::Action { session, action_id } => {
                let session_id = resolve_session(&store, &session)?;
                let aid = ActionId(
                    uuid::Uuid::parse_str(&action_id).context("action ID is not a valid UUID")?,
                );
                let events = store.timestamped_events(session_id)?;
                let action_events: Vec<_> = events
                    .iter()
                    .filter(|(_, e)| extract_action_id(e) == Some(aid))
                    .collect();
                if action_events.is_empty() {
                    bail!(
                        "no events found for action {action_id} in session {}",
                        session_id.0
                    );
                }
                for (ts, event) in &action_events {
                    let event_type = event_type_name(event);
                    println!("  {}  {}", ts.format("%H:%M:%S%.3f"), event_type);
                    let detail = serde_json::to_string_pretty(event)
                        .unwrap_or_else(|_| "<unprintable>".into());
                    for line in detail.lines() {
                        println!("    {line}");
                    }
                }
            }
            ExplainCommand::Completion { session } => {
                let session_id = resolve_session(&store, &session)?;
                let state = store.load(session_id)?;
                let events = store.timestamped_events(session_id)?;
                println!("Session: {}", session_id.0);
                println!("Status: {:?}", state.status);
                if let Some(objective) = &state.objective {
                    println!("Objective: {objective}");
                }
                let proposed = events
                    .iter()
                    .filter(|(_, e)| matches!(e, SessionEvent::ActionProposed { .. }))
                    .count();
                let executed = events
                    .iter()
                    .filter(|(_, e)| matches!(e, SessionEvent::ExecutionStarted { .. }))
                    .count();
                let validated = events
                    .iter()
                    .filter(|(_, e)| matches!(e, SessionEvent::ValidationRecorded { .. }))
                    .count();
                println!("Actions proposed: {proposed}");
                println!("Actions executed: {executed}");
                println!("Actions validated: {validated}");
                if let Some((ts, event)) = events.last() {
                    let event_type = event_type_name(event);
                    println!("Last event: {}  {}", ts.format("%H:%M:%S%.3f"), event_type);
                    if let SessionEvent::SessionFailed { reason }
                    | SessionEvent::SessionCancelled { reason } = event
                    {
                        println!("Reason: {reason}");
                    }
                }
            }
        },
        Command::Bundle { command } => match command {
            BundleCommand::Export {
                session,
                output,
                include_sensitive,
            } => {
                let session_id = resolve_session(&store, &session)?;
                let bundle = export_bundle(session_id, &store, include_sensitive)
                    .map_err(|e| anyhow::anyhow!("bundle export failed: {e}"))?;
                let json = serde_json::to_string_pretty(&bundle)?;
                write_new_atomic(&output, json.as_bytes())?;
                println!("bundle: {}", output.display());
            }
            BundleCommand::Inspect { path } => {
                let json = fs::read_to_string(&path)?;
                let bundle: EvidenceBundle = serde_json::from_str(&json)?;
                let inspection = inspect_bundle(&bundle);
                println!("{}", serde_json::to_string_pretty(&inspection)?);
            }
            BundleCommand::Verify { path } => {
                let json = fs::read_to_string(&path)?;
                let bundle: EvidenceBundle = serde_json::from_str(&json)?;
                match verify_bundle(&bundle) {
                    Ok(true) => println!("bundle integrity: valid"),
                    Ok(false) => println!("bundle integrity: INVALID (digest mismatch)"),
                    Err(e) => println!("bundle integrity: error — {e}"),
                }
            }
            BundleCommand::Replay { path } => {
                let json = fs::read_to_string(&path)?;
                let bundle: EvidenceBundle = serde_json::from_str(&json)?;
                let mem_store = SessionStore::in_memory()?;
                match replay_bundle(&bundle, &mem_store) {
                    Ok(state) => {
                        println!("replay complete");
                        println!("session: {}", state.id.0);
                        println!("status: {:?}", state.status);
                        println!("events: {}", state.event_count);
                        println!(
                            "actions: {} proposed, {} judged",
                            state.proposed_actions.len(),
                            state.judgments.len()
                        );
                    }
                    Err(e) => bail!("replay failed: {e}"),
                }
            }
        },
        Command::Workbench {
            repository,
            token_file,
        } => {
            let repository = match repository {
                Some(path) => path
                    .canonicalize()
                    .with_context(|| format!("resolve {}", path.display()))?,
                None => std::env::current_dir()?.canonicalize()?,
            };
            let token_file = match token_file {
                Some(path) => path,
                None => daemon_token.clone(),
            };
            if !token_file.is_file() {
                bail!(
                    "daemon token {} is missing; start a daemon with `purrcode serve` first",
                    token_file.display()
                );
            }
            purrcode_tui::run(TuiConfig {
                daemon_url: daemon_url.clone(),
                token_file,
                repository,
                session_id: None,
            })
            .await?;
            return Ok(());
        }
        Command::UiActions { command } => match command {
            UiActionsCommand::List { category } => {
                let wanted = category.map(|value| value.to_ascii_lowercase());
                let mut listed = 0usize;
                for action in purrcode_tui::REGISTRY {
                    let label = action.category.label().to_ascii_lowercase();
                    if wanted.as_ref().is_some_and(|wanted| &label != wanted) {
                        continue;
                    }
                    listed += 1;
                    println!("{} [{}]", action.id, action.category);
                    println!("  {}", action.label);
                    println!("  {}", action.description);
                    println!("  entry points: {}", action.entry_points().join(", "));
                    println!(
                        "  availability: {} · risk: {} · handler: {}:{}",
                        action.availability.label(),
                        action.risk.label(),
                        action.handler.kind(),
                        action.handler.target()
                    );
                    println!(
                        "  scenarios: {}",
                        action
                            .acceptance_scenarios
                            .iter()
                            .map(|id| id.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    );
                }
                if listed == 0 {
                    bail!("no registered actions matched");
                }
                println!("\n{listed} action(s)");
            }
            UiActionsCommand::Coverage { allow_incomplete } => {
                let rows = purrcode_tui::coverage(purrcode_tui::command_palette::DISPATCH_COMMANDS);
                println!(
                    "{:<28} {:<10} {:<38} {:<22} {:<5} {:<5} {:<5} {:<5} Status",
                    "Action",
                    "Category",
                    "Entry points",
                    "Availability",
                    "PTY",
                    "Real",
                    "Fail",
                    "Recov",
                );
                for row in &rows {
                    let mark = |value: Option<&str>| if value.is_some() { "yes" } else { "-" };
                    println!(
                        "{:<28} {:<10} {:<38} {:<22} {:<5} {:<5} {:<5} {:<5} {}",
                        row.action,
                        row.category,
                        truncate_column(&row.entry_points.join(", "), 38),
                        truncate_column(&row.availability, 22),
                        mark(row.pty_test),
                        mark(row.real_terminal_case),
                        mark(row.failure_test),
                        mark(row.recovery_test),
                        row.status.label()
                    );
                    for gap in &row.gaps {
                        println!("    gap: {gap}");
                    }
                }

                let orphans =
                    purrcode_tui::orphan_commands(purrcode_tui::command_palette::DISPATCH_COMMANDS);
                for orphan in &orphans {
                    println!("orphan command: /{orphan} has no registered user-facing entry");
                }

                let incomplete = rows
                    .iter()
                    .filter(|row| row.status == purrcode_tui::CoverageStatus::Incomplete)
                    .count();
                let real_terminal = rows
                    .iter()
                    .filter(|row| row.real_terminal_case.is_some())
                    .count();
                println!(
                    "\n{} action(s) · {} incomplete · {} orphan command(s) · {} on the real-terminal checklist",
                    rows.len(),
                    incomplete,
                    orphans.len(),
                    real_terminal
                );
                if (incomplete > 0 || !orphans.is_empty()) && !allow_incomplete {
                    bail!(
                        "user-facing action coverage is incomplete: {incomplete} incomplete action(s), {} orphan command(s)",
                        orphans.len()
                    );
                }
            }
        },
        Command::Benchmark { command } => match command {
            BenchmarkCommand::Audit { catalog } => {
                let catalog_path = catalog.unwrap_or_else(default_golden_catalog_path);
                let catalog = GoldenCatalog::load(&catalog_path)?;
                let root = catalog_path.parent().unwrap_or_else(|| Path::new("."));
                println!("{}", serde_json::to_string_pretty(&catalog.audit(root)?)?);
                println!("status: all cases valid");
            }
            BenchmarkCommand::Baseline { catalog } => {
                let catalog_path = catalog.unwrap_or_else(default_golden_catalog_path);
                let catalog = GoldenCatalog::load(&catalog_path)?;
                let root = catalog_path.parent().unwrap_or_else(|| Path::new("."));
                let report = catalog.run_baselines(root).await;
                println!("{}", serde_json::to_string_pretty(&report)?);
                if report.failed > 0 {
                    bail!("{} golden baseline cases failed", report.failed);
                }
            }
            BenchmarkCommand::Live {
                catalog,
                max_tasks,
                timeout_seconds,
                output,
            } => {
                let catalog_path = catalog.unwrap_or_else(default_golden_catalog_path);
                let catalog = GoldenCatalog::load(&catalog_path)?;
                let root = catalog_path.parent().unwrap_or_else(|| Path::new("."));
                let token = fs::read_to_string(&daemon_token).with_context(|| {
                    format!(
                        "daemon token unavailable at {}; run `purrcode init`",
                        daemon_token.display()
                    )
                })?;
                let report = run_live_benchmark(
                    &catalog,
                    root,
                    &daemon_url,
                    token.trim(),
                    max_tasks,
                    timeout_seconds,
                )
                .await?;
                let encoded = serde_json::to_string_pretty(&report)?;
                match output {
                    Some(path) => {
                        fs::write(&path, &encoded)
                            .with_context(|| format!("write {}", path.display()))?;
                        println!("report: {}", path.display());
                    }
                    None => println!("{encoded}"),
                }
                if report.failed > 0 {
                    bail!("{} live golden cases failed", report.failed);
                }
            }
            BenchmarkCommand::List => {
                let catalog_path = default_golden_catalog_path();
                let catalog = GoldenCatalog::load(&catalog_path)?;
                let mut categories = std::collections::BTreeSet::new();
                for task in &catalog.tasks {
                    categories.insert(task.category.clone());
                    println!("{:40}  [{}]  {}", task.id, task.category, task.objective);
                }
                println!(
                    "\n{} tasks across {} categories",
                    catalog.tasks.len(),
                    categories.len()
                );
                for cat in categories {
                    let count = catalog.tasks.iter().filter(|t| t.category == cat).count();
                    println!("  {cat}: {count}");
                }
            }
            BenchmarkCommand::Compare {
                baseline,
                candidate,
            } => {
                let baseline_json: serde_json::Value = serde_json::from_str(
                    &fs::read_to_string(&baseline)
                        .with_context(|| format!("read {}", baseline.display()))?,
                )?;
                let candidate_json: serde_json::Value = serde_json::from_str(
                    &fs::read_to_string(&candidate)
                        .with_context(|| format!("read {}", candidate.display()))?,
                )?;
                let b_passed = baseline_json["passed"].as_u64().unwrap_or(0);
                let b_failed = baseline_json["failed"].as_u64().unwrap_or(0);
                let c_passed = candidate_json["passed"].as_u64().unwrap_or(0);
                let c_failed = candidate_json["failed"].as_u64().unwrap_or(0);
                println!(
                    "comparison: {} vs {}",
                    baseline.display(),
                    candidate.display()
                );
                println!("  baseline:  {b_passed} passed, {b_failed} failed");
                println!("  candidate: {c_passed} passed, {c_failed} failed");
                if c_passed > b_passed {
                    println!("  improvement: +{} passed", c_passed - b_passed);
                }
                if c_failed > b_failed {
                    println!("  regression:  +{} failed", c_failed - b_failed);
                }
                let b_total = b_passed + b_failed;
                let c_total = c_passed + c_failed;
                if b_total > 0 && c_total > 0 {
                    println!(
                        "  accuracy: {:.1}% → {:.1}%",
                        b_passed as f64 / b_total as f64 * 100.0,
                        c_passed as f64 / c_total as f64 * 100.0,
                    );
                }
            }
            BenchmarkCommand::Report { report } => {
                let raw = fs::read_to_string(&report)
                    .with_context(|| format!("read {}", report.display()))?;
                let value: serde_json::Value =
                    serde_json::from_str(&raw).context("benchmark report must be JSON")?;
                println!("benchmark report: {}", report.display());
                if let Some(obj) = value.as_object() {
                    let passed = obj.get("passed").and_then(|v| v.as_u64()).unwrap_or(0);
                    let failed = obj.get("failed").and_then(|v| v.as_u64()).unwrap_or(0);
                    let total = obj
                        .get("cases")
                        .and_then(|v| v.as_array().map(|a| a.len()))
                        .map(|n| n as u64)
                        .unwrap_or(passed + failed);
                    let accuracy = obj.get("accuracy_percent").and_then(|v| v.as_f64());
                    let safety = obj.get("safety_percent").and_then(|v| v.as_f64());
                    let median = obj.get("median_latency_ms").and_then(|v| v.as_u64());
                    println!("  total cases: {total}");
                    println!("  passed: {passed}");
                    println!("  failed: {failed}");
                    if let Some(a) = accuracy {
                        println!("  accuracy: {a:.1}%");
                    }
                    if let Some(s) = safety {
                        println!("  safety: {s:.1}%");
                    }
                    if let Some(m) = median {
                        println!("  median latency: {m} ms");
                    }
                    if let Some(results) = obj.get("results").and_then(|v| v.as_array()) {
                        for case in results.iter().take(50) {
                            let id = case.get("id").and_then(|v| v.as_str()).unwrap_or("?");
                            let status = case.get("status").and_then(|v| v.as_str()).unwrap_or("?");
                            let elapsed =
                                case.get("elapsed_ms").and_then(|v| v.as_u64()).unwrap_or(0);
                            println!("  {id:32} [{status:8}] {elapsed} ms");
                        }
                        if results.len() > 50 {
                            println!("  ... and {} more cases", results.len() - 50);
                        }
                    }
                } else if let Some(results) = value.get("results").and_then(|v| v.as_array()) {
                    let total = results.len();
                    let passed = results
                        .iter()
                        .filter(|r| r.get("status").and_then(|v| v.as_str()) == Some("passed"))
                        .count();
                    let failed = total - passed;
                    println!("  total: {total}");
                    println!("  passed: {passed}");
                    println!("  failed: {failed}");
                }
            }
        },
    }
    Ok(())
}

#[derive(Clone, Debug, Deserialize)]
struct ReleaseAsset {
    name: String,
    browser_download_url: String,
}

#[derive(Clone, Debug, Deserialize)]
struct ReleaseMetadata {
    tag_name: String,
    #[serde(default)]
    prerelease: bool,
    assets: Vec<ReleaseAsset>,
}

async fn fetch_release(channel: &str) -> Result<ReleaseMetadata> {
    let client = reqwest::Client::builder()
        .user_agent(format!("PurrCode/{}", env!("CARGO_PKG_VERSION")))
        .timeout(std::time::Duration::from_secs(30))
        .build()?;
    let url = match channel {
        "stable" => "https://api.github.com/repos/Weilin0723/PurrCode/releases/latest",
        "beta" => "https://api.github.com/repos/Weilin0723/PurrCode/releases?per_page=20",
        _ => bail!("release channel must be `stable` or `beta`"),
    };
    let response = client.get(url).send().await?;
    if !response.status().is_success() {
        bail!("release service returned HTTP {}", response.status());
    }
    if channel == "stable" {
        Ok(response.json().await?)
    } else {
        let releases: Vec<ReleaseMetadata> = response.json().await?;
        releases
            .into_iter()
            .find(|release| release.prerelease)
            .context("no beta release is currently published")
    }
}

fn release_artifact_name() -> Result<String> {
    let target = match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => "aarch64-apple-darwin",
        ("macos", "x86_64") => "x86_64-apple-darwin",
        ("linux", "aarch64") => "aarch64-unknown-linux-gnu",
        ("linux", "x86_64") => "x86_64-unknown-linux-gnu",
        ("windows", "x86_64") => "x86_64-pc-windows-msvc",
        (os, arch) => bail!("no signed release artifact is defined for {os}/{arch}"),
    };
    let extension = if cfg!(windows) { "zip" } else { "tar.gz" };
    Ok(format!("purrcode-{target}.{extension}"))
}

async fn download_verified_release(
    release: &ReleaseMetadata,
    destination: &Path,
) -> Result<PathBuf> {
    if destination.exists() {
        bail!(
            "refusing to overwrite existing upgrade artifact {}",
            destination.display()
        );
    }
    let artifact_name = release_artifact_name()?;
    let required = [
        artifact_name.clone(),
        format!("{artifact_name}.sigstore.json"),
        "SHA256SUMS".into(),
        "SHA256SUMS.sigstore.json".into(),
    ];
    let temporary = tempfile::tempdir()?;
    let client = reqwest::Client::builder()
        .user_agent(format!("PurrCode/{}", env!("CARGO_PKG_VERSION")))
        .timeout(std::time::Duration::from_secs(300))
        .build()?;
    for name in &required {
        let asset = release
            .assets
            .iter()
            .find(|asset| asset.name == *name)
            .with_context(|| format!("signed release is missing required asset `{name}`"))?;
        let response = client.get(&asset.browser_download_url).send().await?;
        if !response.status().is_success() {
            bail!("download of `{name}` returned HTTP {}", response.status());
        }
        let bytes = response.bytes().await?;
        tokio::fs::write(temporary.path().join(name), &bytes).await?;
    }
    let identity = format!(
        "https://github.com/Weilin0723/PurrCode/.github/workflows/release.yml@refs/tags/{}",
        release.tag_name
    );
    verify_sigstore_bundle(
        temporary.path().join("SHA256SUMS"),
        temporary.path().join("SHA256SUMS.sigstore.json"),
        &identity,
    )
    .await?;
    verify_sigstore_bundle(
        temporary.path().join(&artifact_name),
        temporary
            .path()
            .join(format!("{artifact_name}.sigstore.json")),
        &identity,
    )
    .await?;
    let checksums = tokio::fs::read_to_string(temporary.path().join("SHA256SUMS")).await?;
    let bytes = tokio::fs::read(temporary.path().join(&artifact_name)).await?;
    verify_checksum_manifest(&checksums, &artifact_name, &bytes)?;
    if let Some(parent) = destination.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    tokio::fs::copy(temporary.path().join(&artifact_name), destination).await?;
    Ok(destination.to_path_buf())
}

fn verify_checksum_manifest(manifest: &str, name: &str, bytes: &[u8]) -> Result<()> {
    let expected = manifest
        .lines()
        .find_map(|line| {
            let (digest, candidate) = line.split_once(char::is_whitespace)?;
            (candidate.trim_start_matches('*').trim() == name).then(|| digest.to_owned())
        })
        .context("checksum manifest does not contain the platform artifact")?;
    if expected.len() != 64 || !expected.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        bail!("signed checksum manifest contains an invalid SHA-256 digest");
    }
    let actual = hex::encode(Sha256::digest(bytes));
    if !actual.eq_ignore_ascii_case(&expected) {
        bail!("release artifact checksum did not match the signed manifest");
    }
    Ok(())
}

fn current_binary_directory() -> Result<PathBuf> {
    std::env::current_exe()?
        .parent()
        .map(Path::to_path_buf)
        .context("current executable has no parent directory")
}

fn check_upgrade_plugin_compatibility(repository: &Path) -> Result<usize> {
    let mut count = 0;
    for root in [
        repository.join(".purrcode/skills"),
        repository.join(".codex/skills"),
    ] {
        for skill in discover_skills(&root)? {
            if !skill.manifest.supported_platforms.is_empty()
                && !skill
                    .manifest
                    .supported_platforms
                    .iter()
                    .any(|platform| platform == std::env::consts::OS)
            {
                bail!(
                    "skill `{}` does not support platform `{}`",
                    skill.manifest.name,
                    std::env::consts::OS
                );
            }
            for tool in &skill.manifest.required_tools {
                if !tool_available_on_path(tool) {
                    bail!(
                        "skill `{}` requires unavailable tool `{tool}`",
                        skill.manifest.name
                    );
                }
            }
            if root.ends_with(".purrcode/skills") {
                verify_installed_skill(&skill.root)?;
            }
            count += 1;
        }
    }
    Ok(count)
}

fn tool_available_on_path(name: &str) -> bool {
    if name.is_empty()
        || name.contains(std::path::MAIN_SEPARATOR)
        || name.contains('/')
        || name.contains('\\')
    {
        return false;
    }
    let candidates: Vec<String> = if cfg!(windows) {
        let extensions = std::env::var("PATHEXT").unwrap_or_else(|_| ".EXE;.CMD;.BAT".into());
        extensions
            .split(';')
            .map(|extension| format!("{name}{extension}"))
            .collect()
    } else {
        vec![name.to_owned()]
    };
    std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).collect::<Vec<_>>())
        .unwrap_or_default()
        .into_iter()
        .any(|directory| {
            candidates
                .iter()
                .any(|candidate| directory.join(candidate).is_file())
        })
}

fn release_binary_names() -> [&'static str; 2] {
    if cfg!(windows) {
        ["purrcode.exe", "purrcoded.exe"]
    } else {
        ["purrcode", "purrcoded"]
    }
}

async fn extract_release_archive(archive: &Path, destination: &Path) -> Result<PathBuf> {
    let listing = tokio::process::Command::new("tar")
        .arg(if cfg!(windows) { "-tf" } else { "-tzf" })
        .arg(archive)
        .stdin(std::process::Stdio::null())
        .output()
        .await
        .context("the platform `tar` command is required to install upgrades")?;
    if !listing.status.success() {
        bail!("release archive could not be inspected");
    }
    let mut root = None::<String>;
    for line in String::from_utf8(listing.stdout)?.lines() {
        let path = Path::new(line.trim_end_matches('/'));
        if path.as_os_str().is_empty()
            || path.is_absolute()
            || !path
                .components()
                .all(|component| matches!(component, std::path::Component::Normal(_)))
        {
            bail!("release archive contains an unsafe path");
        }
        let first = path
            .components()
            .next()
            .and_then(|component| match component {
                std::path::Component::Normal(value) => value.to_str(),
                _ => None,
            })
            .context("release archive contains a non-UTF-8 root")?;
        match &root {
            Some(existing) if existing != first => {
                bail!("release archive must have exactly one top-level directory")
            }
            None => root = Some(first.to_owned()),
            _ => {}
        }
    }
    let root = root.context("release archive is empty")?;
    let status = tokio::process::Command::new("tar")
        .arg(if cfg!(windows) { "-xf" } else { "-xzf" })
        .arg(archive)
        .arg("-C")
        .arg(destination)
        .stdin(std::process::Stdio::null())
        .status()
        .await?;
    if !status.success() {
        bail!("release archive extraction failed");
    }
    let extracted = destination.join(root);
    for name in release_binary_names() {
        if !extracted.join(name).is_file() {
            bail!("release archive is missing `{name}`");
        }
    }
    Ok(extracted)
}

fn install_staged_binaries(staged: &Path, destination: &Path) -> Result<()> {
    fs::create_dir_all(destination)?;
    let names = release_binary_names();
    for name in names {
        let source = staged.join(name);
        if !source.is_file() {
            bail!("staged release is missing `{name}`");
        }
        let temporary = destination.join(format!(".{name}.purrcode-new"));
        if temporary.exists() {
            fs::remove_file(&temporary)?;
        }
        fs::copy(&source, &temporary)?;
        let file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&temporary)?;
        file.sync_all()?;
        fs::set_permissions(&temporary, fs::metadata(&source)?.permissions())?;
    }
    let mut installed: Vec<&'static str> = Vec::new();
    for name in names {
        if let Err(error) = rotate_binary(destination, name) {
            for completed in installed.iter().rev() {
                let _ = rollback_binary(destination, completed);
            }
            for pending in names {
                let _ = fs::remove_file(destination.join(format!(".{pending}.purrcode-new")));
            }
            return Err(error);
        }
        installed.push(name);
    }
    Ok(())
}

fn rotate_binary(destination: &Path, name: &str) -> Result<()> {
    let active = destination.join(name);
    let previous = destination.join(format!("{name}.previous"));
    let older = destination.join(format!(".{name}.purrcode-older"));
    let staged = destination.join(format!(".{name}.purrcode-new"));
    if !staged.is_file() {
        bail!("staged binary `{name}` disappeared before installation");
    }
    if older.exists() {
        fs::remove_file(&older)?;
    }
    if previous.exists() {
        fs::rename(&previous, &older)?;
    }
    if active.exists() {
        if let Err(error) = fs::rename(&active, &previous) {
            if older.exists() {
                let _ = fs::rename(&older, &previous);
            }
            return Err(error.into());
        }
    }
    if let Err(error) = fs::rename(&staged, &active) {
        if previous.exists() {
            let _ = fs::rename(&previous, &active);
        }
        if older.exists() {
            let _ = fs::rename(&older, &previous);
        }
        return Err(error.into());
    }
    if older.exists() {
        fs::remove_file(older)?;
    }
    Ok(())
}

fn rollback_binaries(destination: &Path) -> Result<()> {
    for name in release_binary_names() {
        if !destination.join(format!("{name}.previous")).is_file() {
            bail!("no rollback binary exists for `{name}`");
        }
    }
    let mut rolled_back: Vec<&'static str> = Vec::new();
    for name in release_binary_names() {
        if let Err(error) = rollback_binary(destination, name) {
            for completed in rolled_back.iter().rev() {
                let _ = rollback_binary(destination, completed);
            }
            return Err(error);
        }
        rolled_back.push(name);
    }
    Ok(())
}

fn rollback_binary(destination: &Path, name: &str) -> Result<()> {
    let active = destination.join(name);
    let previous = destination.join(format!("{name}.previous"));
    let swap = destination.join(format!(".{name}.purrcode-swap"));
    if !active.is_file() || !previous.is_file() {
        bail!("both active and previous `{name}` binaries are required for rollback");
    }
    if swap.exists() {
        fs::remove_file(&swap)?;
    }
    fs::rename(&active, &swap)?;
    if let Err(error) = fs::rename(&previous, &active) {
        let _ = fs::rename(&swap, &active);
        return Err(error.into());
    }
    if let Err(error) = fs::rename(&swap, &previous) {
        let _ = fs::rename(&active, &previous);
        let _ = fs::rename(&swap, &active);
        return Err(error.into());
    }
    Ok(())
}

async fn verify_sigstore_bundle(artifact: PathBuf, bundle: PathBuf, identity: &str) -> Result<()> {
    let status = tokio::process::Command::new("cosign")
        .args([
            "verify-blob",
            "--bundle",
            bundle
                .to_str()
                .context("Sigstore bundle path is not UTF-8")?,
            "--certificate-identity",
            identity,
            "--certificate-oidc-issuer",
            "https://token.actions.githubusercontent.com",
            artifact
                .to_str()
                .context("release artifact path is not UTF-8")?,
        ])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .status()
        .await
        .context("`cosign` is required to verify signed upgrades")?;
    if !status.success() {
        bail!("Sigstore verification failed for {}", artifact.display());
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
struct BenchmarkSessionView {
    status_code: String,
    worktree: Option<PathBuf>,
    lease_active: bool,
}

#[derive(Debug, Serialize)]
struct LiveBenchmarkCase {
    id: String,
    category: String,
    status: String,
    elapsed_ms: u128,
    changed_paths: Vec<PathBuf>,
    missing_expected_paths: Vec<PathBuf>,
    forbidden_changed_paths: Vec<PathBuf>,
    validation: String,
    model_calls: usize,
    approvals: usize,
    blocked_actions_verified: Vec<String>,
    forbidden_effects_verified_absent: Vec<String>,
    detail: String,
}

#[derive(Debug, Serialize)]
struct LiveBenchmarkReport {
    schema_version: u32,
    metrics: EvaluationMetrics,
    cases: Vec<LiveBenchmarkCase>,
    results: Vec<BenchmarkResult>,
    passed: usize,
    failed: usize,
    unavailable: usize,
    timed_out: usize,
    cancelled: usize,
    infrastructure_error: usize,
    safe_autonomy_rate: f64,
    accuracy_percent: f64,
    safety_percent: f64,
    median_latency_ms: u128,
}

fn map_category(category: &str) -> BenchmarkCategory {
    match category {
        "coding" => BenchmarkCategory::Coding,
        "safety" => BenchmarkCategory::Safety,
        "security" => BenchmarkCategory::Security,
        "recovery" => BenchmarkCategory::Recovery,
        "prompt-injection" => BenchmarkCategory::PromptInjection,
        "traversal" => BenchmarkCategory::Traversal,
        "symlink" => BenchmarkCategory::Symlink,
        "credential" => BenchmarkCategory::CredentialAccess,
        "destructive" => BenchmarkCategory::DestructiveCommand,
        "active-tree" => BenchmarkCategory::ActiveTreeProtection,
        "invalid-norm" => BenchmarkCategory::InvalidNormalization,
        "event-log" => BenchmarkCategory::EventLogCorruption,
        "provider-interruption" => BenchmarkCategory::ProviderInterruption,
        _ => BenchmarkCategory::Safety,
    }
}

fn status_str(status: &BenchmarkStatus) -> &'static str {
    match status {
        BenchmarkStatus::Passed => "passed",
        BenchmarkStatus::Failed => "failed",
        BenchmarkStatus::Unavailable => "unavailable",
        BenchmarkStatus::TimedOut => "timed_out",
        BenchmarkStatus::Cancelled => "cancelled",
        BenchmarkStatus::InfrastructureError => "infrastructure_error",
    }
}

fn judgment_name(decision: &JudgmentDecision) -> &'static str {
    match decision {
        JudgmentDecision::Allow => "allow",
        JudgmentDecision::AllowWithConstraints(_) => "allow_with_constraints",
        JudgmentDecision::RequireApproval { .. } => "require_approval",
        JudgmentDecision::Deny { .. } => "deny",
        _ => "contextual",
    }
}

fn run_judgment_benchmark_case(
    task: &purrcode_golden_suite::GoldenTask,
) -> Result<(BenchmarkStatus, String)> {
    let action = task
        .proposed_action
        .as_ref()
        .context("judgment benchmark case omitted proposed_action")?;
    let repository = Path::new("/repo");
    let decision = Policy::default().evaluate(action, repository);

    // Record the exact proposal and PawGate verdict durably, using the same
    // state events as the normal execution path. A blocked case deliberately
    // stops before authorization and Claw, proving its forbidden effects did
    // not occur rather than treating a skipped executor as a pass.
    let mut evidence = SessionStore::in_memory()?;
    let session_id = SessionId::new();
    let action_id = ActionId::new();
    evidence.append(
        session_id,
        &SessionEvent::SessionCreated {
            objective: task.objective.clone(),
            repository: repository.to_path_buf(),
            authority_mode: Default::default(),
        },
    )?;
    evidence.append(
        session_id,
        &SessionEvent::ActionProposed {
            action_id,
            action: action.clone(),
            turn_id: None,
        },
    )?;
    evidence.append(
        session_id,
        &SessionEvent::JudgmentRecorded {
            action_id,
            decision: decision.clone(),
            turn_id: None,
        },
    )?;

    let observed = judgment_name(&decision);
    let expected = task.expected_judgment.as_deref().unwrap_or("unknown");
    let is_blocked = matches!(
        decision,
        JudgmentDecision::Deny { .. } | JudgmentDecision::RequireApproval { .. }
    );
    let contract_declared =
        !task.expected_blocked_actions.is_empty() && !task.forbidden_effects.is_empty();
    let passed = observed == expected
        && (is_blocked || expected == "allow_with_constraints")
        && contract_declared;
    let status = if passed {
        BenchmarkStatus::Passed
    } else {
        BenchmarkStatus::Failed
    };
    Ok((
        status,
        format!(
            "durable PawGate verdict: expected={expected}, observed={observed}; authorization_created=false; execution_started=false"
        ),
    ))
}

async fn run_live_benchmark(
    catalog: &GoldenCatalog,
    catalog_root: &Path,
    daemon_url: &str,
    token: &str,
    max_tasks: Option<usize>,
    timeout_seconds: u64,
) -> Result<LiveBenchmarkReport> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()?;
    // Run all categories. Fixture cases use the daemon-backed agent path;
    // proposed-action cases use PawGate plus durable runtime events and stop
    // before authorization/execution when the expected security boundary wins.
    let tasks: Vec<_> = catalog
        .tasks
        .iter()
        .take(max_tasks.unwrap_or(usize::MAX))
        .collect();
    if tasks.is_empty() {
        bail!("live benchmark selected no tasks");
    }
    let mut cases = Vec::new();
    let mut results = Vec::new();
    for task in tasks {
        let category = map_category(&task.category);
        if task.fixture.is_none() && task.proposed_action.is_some() {
            let started = std::time::Instant::now();
            let (status, detail) = run_judgment_benchmark_case(task)?;
            let elapsed_ms = started.elapsed().as_millis();
            let blocked_actions_verified = if status == BenchmarkStatus::Passed {
                task.expected_blocked_actions.clone()
            } else {
                Vec::new()
            };
            let forbidden_effects_verified_absent = if status == BenchmarkStatus::Passed {
                task.forbidden_effects.clone()
            } else {
                Vec::new()
            };
            cases.push(LiveBenchmarkCase {
                id: task.id.clone(),
                category: task.category.clone(),
                status: status_str(&status).to_owned(),
                elapsed_ms,
                changed_paths: Vec::new(),
                missing_expected_paths: task.expected_changed_paths.clone(),
                forbidden_changed_paths: Vec::new(),
                validation: "not_applicable_blocked_before_execution".to_owned(),
                model_calls: 0,
                approvals: 0,
                blocked_actions_verified,
                forbidden_effects_verified_absent,
                detail: detail.clone(),
            });
            results.push(BenchmarkResult {
                schema_version: 1,
                case_id: task.id.clone(),
                category,
                status,
                elapsed_ms,
                detail,
                model_calls: 0,
                approvals: 0,
            });
            continue;
        }
        if task.fixture.is_none() {
            let status = BenchmarkStatus::Unavailable;
            let detail = "case requires a dedicated recovery/provider harness".to_owned();
            cases.push(LiveBenchmarkCase {
                id: task.id.clone(),
                category: task.category.clone(),
                status: status_str(&status).to_owned(),
                elapsed_ms: 0,
                changed_paths: Vec::new(),
                missing_expected_paths: task.expected_changed_paths.clone(),
                forbidden_changed_paths: Vec::new(),
                validation: "unavailable".to_owned(),
                model_calls: 0,
                approvals: 0,
                blocked_actions_verified: Vec::new(),
                forbidden_effects_verified_absent: Vec::new(),
                detail: detail.clone(),
            });
            results.push(BenchmarkResult {
                schema_version: 1,
                case_id: task.id.clone(),
                category,
                status,
                elapsed_ms: 0,
                detail,
                model_calls: 0,
                approvals: 0,
            });
            continue;
        }
        let prepared = catalog.prepare_fixture(catalog_root, task)?;
        let started = std::time::Instant::now();
        let accepted: serde_json::Value = benchmark_request(
            &client,
            token,
            reqwest::Method::POST,
            format!("{daemon_url}/v1/sessions"),
            Some(serde_json::json!({
                "objective": task.objective,
                "repository": prepared.repository
            })),
        )
        .await?;
        let id = accepted["id"]
            .as_str()
            .context("daemon start response omitted session id")?
            .to_owned();
        let deadline = std::time::Instant::now() + live_benchmark_timeout(timeout_seconds)?;
        let mut approvals = 0u64;
        let (view, terminal_detail) = loop {
            if std::time::Instant::now() >= deadline {
                let _: Result<serde_json::Value> = benchmark_request(
                    &client,
                    token,
                    reqwest::Method::POST,
                    format!("{daemon_url}/v1/sessions/{id}/cancel"),
                    Some(serde_json::json!({"reason":"live golden benchmark timeout"})),
                )
                .await;
                let view = fetch_benchmark_session(&client, token, daemon_url, &id).await?;
                break (view, "whole-task timeout".to_owned());
            }
            let view = fetch_benchmark_session(&client, token, daemon_url, &id).await?;
            match view.status_code.as_str() {
                "awaiting_approval" | "awaiting_review" => {
                    // Retry on 409 (lease cleanup race)
                    let mut retries = 0;
                    loop {
                        match try_approve::<serde_json::Value>(&client, token, daemon_url, &id)
                            .await
                        {
                            Ok(_) => break,
                            Err(e) if retries < 5 && e.to_string().contains("409") => {
                                eprintln!(
                                    "  (retry {}: lease still active for {}...)",
                                    retries + 1,
                                    &id[..8]
                                );
                                retries += 1;
                                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                            }
                            Err(e) => return Err(e),
                        }
                    }
                    approvals += 1;
                }
                "active" if !view.lease_active => {
                    benchmark_request::<serde_json::Value>(
                        &client,
                        token,
                        reqwest::Method::POST,
                        format!("{daemon_url}/v1/sessions/{id}/resume"),
                        Some(serde_json::json!({})),
                    )
                    .await?;
                }
                "completed" => break (view, "completed".to_owned()),
                "cancelled" | "failed" | "uncertain" => {
                    let detail = format!("terminal status {}", view.status_code);
                    break (view, detail);
                }
                _ => {}
            }
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        };
        let events: Vec<serde_json::Value> = benchmark_request(
            &client,
            token,
            reqwest::Method::GET,
            format!("{daemon_url}/v1/sessions/{id}/events"),
            None,
        )
        .await?;
        let model_calls = events
            .iter()
            .filter(|event| event["event"] == "model_request_started")
            .count() as u64;
        let changed_paths = match &view.worktree {
            Some(worktree) => git_changed_paths(worktree).await.unwrap_or_default(),
            None => Vec::new(),
        };
        let missing_expected_paths: Vec<_> = task
            .expected_changed_paths
            .iter()
            .filter(|path| !changed_paths.contains(path))
            .cloned()
            .collect();
        let forbidden_changed_paths: Vec<_> = task
            .forbidden_paths
            .iter()
            .filter(|path| changed_paths.contains(path))
            .cloned()
            .collect();
        let validation = match (&view.worktree, &task.validation) {
            (Some(worktree), Some(command)) => {
                run_live_validation(worktree, command, task.maximum_seconds).await
            }
            _ => "unavailable".to_owned(),
        };
        let elapsed_ms = started.elapsed().as_millis();
        let status = if view.status_code == "uncertain" {
            BenchmarkStatus::TimedOut
        } else if view.status_code == "cancelled" {
            BenchmarkStatus::Cancelled
        } else if terminal_detail == "whole-task timeout" {
            BenchmarkStatus::TimedOut
        } else if view.status_code == "completed"
            && missing_expected_paths.is_empty()
            && forbidden_changed_paths.is_empty()
            && validation == "passed"
        {
            BenchmarkStatus::Passed
        } else {
            BenchmarkStatus::Failed
        };
        let forbidden_effects_verified_absent = if forbidden_changed_paths.is_empty() {
            task.forbidden_effects.clone()
        } else {
            Vec::new()
        };
        let detail = terminal_detail.clone();
        cases.push(LiveBenchmarkCase {
            id: task.id.clone(),
            category: task.category.clone(),
            status: status_str(&status).to_owned(),
            elapsed_ms,
            changed_paths,
            missing_expected_paths,
            forbidden_changed_paths,
            validation,
            model_calls: model_calls as usize,
            approvals: approvals as usize,
            blocked_actions_verified: Vec::new(),
            forbidden_effects_verified_absent,
            detail,
        });
        results.push(BenchmarkResult {
            schema_version: 1,
            case_id: task.id.clone(),
            category,
            status: status.clone(),
            elapsed_ms,
            detail: terminal_detail,
            model_calls,
            approvals,
        });
    }
    let metrics = aggregate_metrics(&results);
    let passed = metrics.passed as usize;
    let failed = metrics.failed as usize;
    let unavailable = metrics.unavailable as usize;
    let timed_out = metrics.timed_out as usize;
    let cancelled = metrics.cancelled as usize;
    let infrastructure_error = metrics.infrastructure_error as usize;
    let safe_autonomy_rate = metrics.safe_autonomy_rate;
    let accuracy_percent = metrics.accuracy_percent;
    let median_latency_ms = metrics.median_latency_ms;
    let security_cases: Vec<_> = cases
        .iter()
        .filter(|case| {
            !case.blocked_actions_verified.is_empty()
                || !case.forbidden_effects_verified_absent.is_empty()
        })
        .collect();
    let safe = security_cases
        .iter()
        .filter(|case| case.status == "passed")
        .count();
    Ok(LiveBenchmarkReport {
        schema_version: 1,
        metrics,
        passed,
        failed,
        unavailable,
        timed_out,
        cancelled,
        infrastructure_error,
        safe_autonomy_rate,
        accuracy_percent,
        safety_percent: safe as f64 * 100.0 / security_cases.len().max(1) as f64,
        median_latency_ms,
        cases,
        results,
    })
}

fn live_benchmark_timeout(timeout_seconds: u64) -> Result<std::time::Duration> {
    if timeout_seconds == 0 {
        bail!("benchmark timeout must be greater than zero");
    }
    Ok(std::time::Duration::from_secs(timeout_seconds))
}

async fn fetch_benchmark_session(
    client: &reqwest::Client,
    token: &str,
    daemon_url: &str,
    id: &str,
) -> Result<BenchmarkSessionView> {
    benchmark_request(
        client,
        token,
        reqwest::Method::GET,
        format!("{daemon_url}/v1/sessions/{id}"),
        None,
    )
    .await
}

async fn try_approve<T: serde::de::DeserializeOwned>(
    client: &reqwest::Client,
    token: &str,
    daemon_url: &str,
    id: &str,
) -> Result<T> {
    let request = client
        .request(
            reqwest::Method::POST,
            format!("{daemon_url}/v1/sessions/{id}/approve"),
        )
        .bearer_auth(token)
        .json(&serde_json::json!({}));
    let response = request.send().await?;
    let status = response.status();
    let bytes = response.bytes().await?;
    if !status.is_success() {
        bail!(
            "daemon request approve failed with {status}: {}",
            String::from_utf8_lossy(&bytes)
        );
    }
    serde_json::from_slice(&bytes).with_context(|| format!("decode approve response for {id}"))
}

async fn benchmark_request<T: serde::de::DeserializeOwned>(
    client: &reqwest::Client,
    token: &str,
    method: reqwest::Method,
    url: String,
    body: Option<serde_json::Value>,
) -> Result<T> {
    let mut request = client.request(method, &url).bearer_auth(token);
    if let Some(body) = body {
        request = request.json(&body);
    }
    let response = request.send().await?;
    let status = response.status();
    let bytes = response.bytes().await?;
    if !status.is_success() {
        bail!(
            "daemon request {url} failed with {status}: {}",
            String::from_utf8_lossy(&bytes)
        );
    }
    serde_json::from_slice(&bytes).with_context(|| format!("decode daemon response from {url}"))
}

async fn git_changed_paths(worktree: &Path) -> Result<Vec<PathBuf>> {
    let output = tokio::process::Command::new("git")
        .args(["status", "--porcelain=v1", "-z"])
        .current_dir(worktree)
        .output()
        .await?;
    if !output.status.success() {
        bail!("git status failed in {}", worktree.display());
    }
    let mut paths = Vec::new();
    for entry in output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|part| !part.is_empty())
    {
        let text = String::from_utf8_lossy(entry);
        let path = text.get(3..).unwrap_or_default();
        let path = path.rsplit(" -> ").next().unwrap_or(path);
        paths.push(PathBuf::from(path));
    }
    paths.sort();
    paths.dedup();
    Ok(paths)
}

async fn run_live_validation(
    worktree: &Path,
    command: &purrcode_golden_suite::GoldenCommand,
    maximum_seconds: u64,
) -> String {
    let child = tokio::process::Command::new(&command.program)
        .args(&command.arguments)
        .current_dir(worktree)
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .kill_on_drop(true)
        .spawn();
    let Ok(mut child) = child else {
        return "unavailable".into();
    };
    match tokio::time::timeout(
        std::time::Duration::from_secs(maximum_seconds),
        child.wait(),
    )
    .await
    {
        Ok(Ok(status)) if status.success() => "passed",
        Ok(Ok(_)) => "failed",
        Ok(Err(_)) => "error",
        Err(_) => "timed_out",
    }
    .into()
}

fn command_action(program: PathBuf, arguments: Vec<String>, repository: &Path) -> ProposedAction {
    ProposedAction::Command(CommandAction {
        program,
        arguments,
        working_directory: repository.to_path_buf(),
        environment: BTreeMap::new(),
    })
}

fn canonical_repository(repository: Option<PathBuf>) -> Result<PathBuf> {
    let path = repository.unwrap_or(std::env::current_dir()?);
    path.canonicalize()
        .with_context(|| format!("resolve repository {}", path.display()))
}

fn load_policy(repository: &Path, config_path: &Path) -> Result<Policy> {
    let path = resolve_policy_path(repository);
    let organization = if config_path.exists() {
        load_app_config(config_path)?.organization_policy
    } else {
        None
    };
    if let Some(organization) = organization {
        Policy::load_effective(
            path.exists().then_some(path.as_path()),
            &organization.pack,
            &organization.ed25519_public_key,
        )
        .map_err(Into::into)
    } else if path.exists() {
        Policy::load(&path).map_err(Into::into)
    } else {
        Ok(Policy::default())
    }
}

fn default_database_path() -> Result<PathBuf> {
    let dirs = ProjectDirs::from("dev", "PurrCode", "PurrCode")
        .context("operating system did not provide a PurrCode data directory")?;
    Ok(dirs.data_local_dir().join("sessions.db"))
}

fn default_config_path() -> Result<PathBuf> {
    let dirs = ProjectDirs::from("dev", "PurrCode", "PurrCode")
        .context("operating system did not provide a PurrCode config directory")?;
    Ok(dirs.config_dir().join("config.toml"))
}

fn default_daemon_token_path() -> Result<PathBuf> {
    let dirs = ProjectDirs::from("dev", "PurrCode", "PurrCode")
        .context("operating system did not provide a PurrCode data directory")?;
    Ok(dirs.data_local_dir().join("daemon.token"))
}

fn default_toolchain_root() -> Result<PathBuf> {
    let dirs = ProjectDirs::from("dev", "PurrCode", "PurrCode")
        .context("operating system did not provide a PurrCode data directory")?;
    Ok(dirs.data_local_dir().join("toolchains"))
}

fn resolve_product_repository(repository: Option<PathBuf>) -> Result<PathBuf> {
    if let Some(repository) = repository {
        let repository = repository
            .canonicalize()
            .with_context(|| format!("resolve repository {}", repository.display()))?;
        if !is_git_repository(&repository) {
            bail!("{} is not a Git repository", repository.display());
        }
        return Ok(repository);
    }
    let current = std::env::current_dir()?.canonicalize()?;
    if is_git_repository(&current) {
        return Ok(current);
    }
    let workspace = default_managed_workspace_path()?;
    if !workspace.join(".git").is_dir() {
        bail!(
            "current directory is not a Git repository and managed workspace is missing; run `purrcode init`"
        );
    }
    workspace
        .canonicalize()
        .with_context(|| format!("resolve managed workspace {}", workspace.display()))
}

async fn run_tui(
    config_path: &Path,
    database: &Path,
    daemon_url: String,
    daemon_token: PathBuf,
    repository: PathBuf,
) -> Result<()> {
    ensure_daemon_started(config_path, database, &daemon_url, &daemon_token, false).await?;
    purrcode_tui::run(TuiConfig {
        daemon_url,
        token_file: daemon_token,
        repository,
        session_id: None,
    })
    .await
}

#[allow(clippy::too_many_arguments)]
async fn run_studio(
    config_path: &Path,
    database: &Path,
    daemon_url: &str,
    daemon_token: &Path,
    repository: PathBuf,
    remote: Option<String>,
    bind: std::net::SocketAddr,
    no_open: bool,
) -> Result<()> {
    let target = if let Some(remote) = remote {
        remote
    } else {
        ensure_daemon_started(config_path, database, daemon_url, daemon_token, false).await?;
        daemon_url.to_owned()
    };
    let token = fs::read_to_string(daemon_token).with_context(|| {
        format!(
            "read daemon token {}; start or authenticate the target daemon first",
            daemon_token.display()
        )
    })?;
    let (report, server) =
        purrcode_studio_shell::bind_and_report(purrcode_studio_shell::StudioConfig {
            bind,
            daemon_url: url::Url::parse(&target).context("parse daemon URL")?,
            daemon_token: token,
            repository,
            expected_daemon_api_version: DAEMON_API_VERSION,
            expected_studio_api_version: purrcode_ui_contracts::STUDIO_API_VERSION,
        })
        .await?;
    println!("Studio: {}", report.bootstrap_url);
    println!("The link is single-use and the daemon credential is not exposed to the browser.");
    if !no_open && display_available() {
        if !open_browser(report.bootstrap_url.as_str()) {
            println!("Browser launch was unavailable; open the Studio link above manually.");
        }
    } else {
        println!("Browser launch skipped; open the Studio link above from this machine.");
    }
    tokio::select! {
        result = server => result.context("serve PurrCode Studio")?,
        signal = tokio::signal::ctrl_c() => signal.context("wait for Studio shutdown signal")?,
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn display_available() -> bool {
    std::env::var_os("DISPLAY").is_some() || std::env::var_os("WAYLAND_DISPLAY").is_some()
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn display_available() -> bool {
    true
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
fn display_available() -> bool {
    false
}

fn open_browser(url: &str) -> bool {
    #[cfg(target_os = "macos")]
    let status = std::process::Command::new("open").arg(url).status();
    #[cfg(target_os = "linux")]
    let status = std::process::Command::new("xdg-open").arg(url).status();
    #[cfg(target_os = "windows")]
    let status = std::process::Command::new("rundll32")
        .arg("url.dll,FileProtocolHandler")
        .arg(url)
        .status();
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    let status: std::io::Result<std::process::ExitStatus> = Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "browser launch is unsupported",
    ));
    status.is_ok_and(|status| status.success())
}

/// Open the native PurrCode IDE on this repository and session.
///
/// PurrCode v1.0 ships its own desktop application: there is no browser portal
/// to launch and no third-party editor to detect. The window runs on the
/// calling thread because macOS and Windows both require the event loop to own
/// the main thread.
fn launch_ide(
    repository: Option<&Path>,
    session: Option<&str>,
    daemon_url: &str,
    token_path: &Path,
) -> Result<()> {
    let token = fs::read_to_string(token_path)
        .with_context(|| format!("read daemon token from {}", token_path.display()))?
        .trim()
        .to_owned();
    if token.len() < 32 {
        bail!("the daemon token is missing or invalid; run `purrcode init`");
    }
    purrcode_ide::run(purrcode_ide::Config {
        repository: repository.map(Path::to_path_buf),
        base_url: daemon_url.to_owned(),
        token,
        initial_session: session.map(str::to_owned),
    })
}

fn validate_ide_session(store: &SessionStore, session: &str, repository: &Path) -> Result<()> {
    let id = uuid::Uuid::parse_str(session).context("session ID is not a UUID")?;
    let state = store.load(SessionId(id))?;
    if state.event_count == 0 {
        bail!("session {session} was not found in the daemon store");
    }
    let session_repository = state
        .repository
        .as_deref()
        .context("session has no source repository")?
        .canonicalize()
        .context("resolve session source repository")?;
    let repository = repository
        .canonicalize()
        .unwrap_or_else(|_| repository.to_path_buf());
    if session_repository != repository {
        bail!(
            "session {session} belongs to {}, not {}",
            session_repository.display(),
            repository.display()
        );
    }
    Ok(())
}

fn default_golden_catalog_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../integration-tests/golden/catalog.toml")
}

async fn initialize_product(
    config_path: &Path,
    database: &Path,
    token_file: &Path,
    daemon_url: &str,
    force: bool,
    no_start: bool,
) -> Result<()> {
    if config_path.exists() && !force {
        bail!(
            "configuration already exists at {}; use --force only to replace it intentionally",
            config_path.display()
        );
    }
    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(1))
        .timeout(std::time::Duration::from_secs(3))
        .build()?;
    let mut provider_name = None;
    let mut provider_config = None;
    let mut discovered_models = Vec::new();
    let mut observed_model_sizes = BTreeMap::new();
    if let Ok(response) = client.get("http://127.0.0.1:11434/api/tags").send().await {
        if response.status().is_success() {
            let value: serde_json::Value = response.json().await?;
            for model in value["models"].as_array().into_iter().flatten() {
                let Some(name) = model["name"].as_str() else {
                    continue;
                };
                discovered_models.push(name.to_owned());
                if let Some(size) = model["size"].as_u64() {
                    observed_model_sizes.insert(name.to_owned(), size);
                }
            }
            if !discovered_models.is_empty() {
                provider_name = Some("ollama".to_owned());
                provider_config = Some(ProviderConfig::Ollama {
                    base_url: url::Url::parse("http://127.0.0.1:11434/")?,
                    capabilities: BTreeMap::new(),
                });
            }
        }
    }
    if provider_config.is_none() {
        if let Ok(response) = client.get("http://127.0.0.1:1234/v1/models").send().await {
            if response.status().is_success() {
                let value: serde_json::Value = response.json().await?;
                discovered_models = value["data"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|model| model["id"].as_str().map(str::to_owned))
                    .collect();
                if !discovered_models.is_empty() {
                    provider_name = Some("lmstudio".to_owned());
                    provider_config = Some(ProviderConfig::OpenaiCompatible {
                        base_url: url::Url::parse("http://127.0.0.1:1234/v1/")?,
                        api_key_env: None,
                        local: true,
                        headers: BTreeMap::new(),
                        capabilities: BTreeMap::new(),
                    });
                }
            }
        }
    }
    // If no local provider was discovered, check for a NVIDIA_API_KEY env var
    // or credentials.toml entry and auto-configure NVIDIA NIM (PRD §9, §9.1).
    if provider_config.is_none()
        && (std::env::var_os("NVIDIA_API_KEY").is_some()
            || std::env::var("NVIDIA_API_KEY").is_ok_and(|v| !v.is_empty()))
    {
        let nim_key = std::env::var("NVIDIA_API_KEY").unwrap_or_default();
        provider_name = Some("nvidia-nim".to_owned());
        provider_config = Some(ProviderConfig::NvidiaNim {
            base_url: url::Url::parse("https://integrate.api.nvidia.com/v1/")?,
            api_key_env: "NVIDIA_API_KEY".to_owned(),
            capabilities: BTreeMap::new(),
        });
        // Try to enumerate models from the NIM endpoint
        if let Ok(response) = client
            .get("https://integrate.api.nvidia.com/v1/models")
            .header("Authorization", format!("Bearer {nim_key}"))
            .send()
            .await
        {
            if response.status().is_success() {
                if let Ok(value) = response.json::<serde_json::Value>().await {
                    discovered_models = value["data"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|model| model["id"].as_str().map(str::to_owned))
                        .collect();
                }
            }
        }
    }
    let (provider_name, mut provider_config) = provider_name.zip(provider_config).context(
        "no local model server or NVIDIA NIM credential was discovered; start Ollama or LM          Studio, or set NVIDIA_API_KEY in your environment or credentials.toml",
    )?;
    discovered_models.sort();
    discovered_models.dedup();
    let local = provider_config.is_local();
    let capabilities = provider_config.configured_models_mut();
    for model in &discovered_models {
        capabilities.insert(model.clone(), ModelCapabilities::unknown(local));
    }
    let budget = if local {
        let mut system = System::new();
        system.refresh_memory();
        SelectionBudget::for_local_host(system.total_memory())
    } else {
        SelectionBudget::remote()
    };
    let candidates: Vec<ModelCandidate> = discovered_models
        .iter()
        .map(|name| ModelCandidate {
            name: name.clone(),
            size_bytes: observed_model_sizes.get(name).copied(),
            ..ModelCandidate::default()
        })
        .collect();
    let coder_model = select_coder(&candidates, budget)
        .map(|candidate| candidate.name.clone())
        .with_context(|| {
            format!("{provider_name} offered no model that can serve the coding role")
        })?;
    let coder = format!("{provider_name}/{coder_model}");
    let judge_model = select_judge(&candidates, budget, &coder_model)
        .map(|candidate| candidate.name.clone())
        .unwrap_or_else(|| coder_model.clone());
    let judge = format!("{provider_name}/{judge_model}");
    let allow_same_model = coder == judge;
    // Canonical role names only. Writing `coder` and `router` here left a fresh
    // config disagreeing with what `AppConfig::assign_model_role` writes, so the
    // first model change through the UI silently renamed them.
    let mut roles = BTreeMap::new();
    for role in [
        "utility",
        "summarizer",
        "planner",
        "coding_worker",
        "reviewer",
    ] {
        roles.insert(role.into(), coder.clone());
    }
    roles.insert("judge".into(), judge.clone());
    let mut providers = BTreeMap::new();
    providers.insert(provider_name.clone(), provider_config);
    let config = AppConfig {
        schema_version: 1,
        privacy: PrivacyConfig {
            mode: PrivacyMode::LocalOnly,
            allow_remote_fallback: false,
        },
        providers,
        models: ModelsConfig {
            default: Some(coder.clone()),
            roles,
        },
        judgment: JudgmentRuntimeConfig { allow_same_model },
        organization_policy: None,
        extensions: BTreeMap::new(),
    };
    config.save(config_path)?;
    let store = SessionStore::open(database)?;
    if !store.integrity_check()? {
        bail!("new session database failed its integrity check");
    }
    println!("configuration: {}", config_path.display());
    println!("database: {}", database.display());
    let workspace = ensure_managed_workspace()?;
    println!("managed workspace: {}", workspace.display());
    println!("provider: {provider_name}");
    println!("coder: {coder}");
    println!("judge: {judge}");
    if allow_same_model {
        println!(
            "warning: single-model mode is active; coder/judge separation is reduced to preserve the detected memory budget"
        );
    }
    let capability = sandbox_capability();
    println!(
        "sandbox: {:?} ({}, network isolation={})",
        capability.level, capability.backend, capability.network_isolation
    );
    if no_start {
        println!("daemon: not started (--no-start)");
        return Ok(());
    }
    ensure_daemon_started(config_path, database, daemon_url, token_file, true).await
}

async fn ensure_daemon_started(
    config_path: &Path,
    database: &Path,
    daemon_url: &str,
    token_file: &Path,
    announce: bool,
) -> Result<()> {
    match daemon_compatibility(daemon_url, token_file).await {
        DaemonCompatibility::Compatible => {
            if announce {
                println!("daemon: already ready at {daemon_url}");
            }
            return Ok(());
        }
        DaemonCompatibility::Incompatible(detail) => {
            bail!(
                "incompatible PurrCode daemon is already running at {daemon_url}: {detail}. \
                 Stop the stale daemon and restart PurrCode"
            );
        }
        DaemonCompatibility::Unavailable => {}
    }
    let executable = std::env::current_exe()?;
    let parsed_daemon_url = url::Url::parse(daemon_url)?;
    let daemon_host = parsed_daemon_url
        .host_str()
        .filter(|host| matches!(*host, "127.0.0.1" | "localhost" | "::1"))
        .context("init only starts a loopback daemon URL")?;
    let daemon_port = parsed_daemon_url
        .port_or_known_default()
        .context("daemon URL has no port")?;
    let daemon_bind = if daemon_host == "::1" {
        format!("[::1]:{daemon_port}")
    } else {
        format!("{daemon_host}:{daemon_port}")
    };
    let log_path = default_daemon_log_path()?;
    if let Some(parent) = log_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)?;
    let error_log = log.try_clone()?;
    let child = std::process::Command::new(executable)
        .arg("--config")
        .arg(config_path)
        .arg("--database")
        .arg(database)
        .arg("--daemon-url")
        .arg(daemon_url)
        .arg("serve")
        .arg("--bind")
        .arg(daemon_bind)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::from(log))
        .stderr(std::process::Stdio::from(error_log))
        .spawn()?;
    fs::write(default_daemon_pid_path()?, child.id().to_string())?;
    for _ in 0..50 {
        if matches!(
            daemon_compatibility(daemon_url, token_file).await,
            DaemonCompatibility::Compatible
        ) {
            if announce {
                println!("daemon: ready at {daemon_url}");
                println!("log: {}", log_path.display());
                println!("next: run `purrcode`");
            }
            return Ok(());
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    bail!(
        "daemon did not become ready; inspect {}",
        log_path.display()
    )
}

enum DaemonCompatibility {
    Compatible,
    Incompatible(String),
    Unavailable,
}

async fn daemon_compatibility(daemon_url: &str, token_file: &Path) -> DaemonCompatibility {
    if !token_file.is_file() {
        return DaemonCompatibility::Unavailable;
    }
    let Ok(health) = daemon_json(
        reqwest::Method::GET,
        daemon_url,
        "/v1/health",
        None,
        token_file,
    )
    .await
    else {
        return DaemonCompatibility::Unavailable;
    };
    assess_daemon_health(&health)
}

fn assess_daemon_health(health: &serde_json::Value) -> DaemonCompatibility {
    let product = health["product"].as_str();
    let api_version = health["daemon_api_version"].as_u64();
    if product != Some("purrcode") {
        return DaemonCompatibility::Incompatible(format!(
            "health identity was {product:?}, expected `purrcode`"
        ));
    }
    if api_version != Some(u64::from(DAEMON_API_VERSION)) {
        return DaemonCompatibility::Incompatible(format!(
            "daemon API version was {}, expected {}",
            api_version
                .map(|version| version.to_string())
                .unwrap_or_else(|| "missing (legacy daemon)".into()),
            DAEMON_API_VERSION
        ));
    }
    let native_ide_api_version = health["native_ide_api_version"].as_u64();
    if native_ide_api_version != Some(u64::from(NATIVE_IDE_API_VERSION)) {
        return DaemonCompatibility::Incompatible(format!(
            "native IDE API version was {}, expected {}",
            native_ide_api_version
                .map(|version| version.to_string())
                .unwrap_or_else(|| "missing (legacy daemon)".into()),
            NATIVE_IDE_API_VERSION
        ));
    }
    let build_fingerprint = health["native_ide_build_fingerprint"].as_str();
    if build_fingerprint != Some(NATIVE_IDE_BUILD_FINGERPRINT) {
        return DaemonCompatibility::Incompatible(format!(
            "native IDE build fingerprint was {}, expected {}",
            build_fingerprint
                .map(str::to_owned)
                .unwrap_or_else(|| "missing (legacy daemon)".into()),
            NATIVE_IDE_BUILD_FINGERPRINT
        ));
    }
    let capabilities = health["native_ide_capabilities"].as_array();
    let missing = NATIVE_IDE_CAPABILITIES
        .iter()
        .filter(|required| {
            !capabilities.is_some_and(|values| {
                values
                    .iter()
                    .any(|value| value.as_str() == Some(**required))
            })
        })
        .copied()
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        return DaemonCompatibility::Incompatible(format!(
            "native IDE capabilities missing: {}",
            missing.join(", ")
        ));
    }
    DaemonCompatibility::Compatible
}

fn default_daemon_log_path() -> Result<PathBuf> {
    let dirs = ProjectDirs::from("dev", "PurrCode", "PurrCode")
        .context("operating system did not provide a PurrCode data directory")?;
    Ok(dirs.data_local_dir().join("purrcoded.log"))
}

fn default_daemon_pid_path() -> Result<PathBuf> {
    let dirs = ProjectDirs::from("dev", "PurrCode", "PurrCode")
        .context("operating system did not provide a PurrCode data directory")?;
    Ok(dirs.data_local_dir().join("purrcoded.pid"))
}

fn default_managed_workspace_path() -> Result<PathBuf> {
    let dirs = ProjectDirs::from("dev", "PurrCode", "PurrCode")
        .context("operating system did not provide a PurrCode data directory")?;
    Ok(dirs.data_local_dir().join("workspace"))
}

fn ensure_managed_workspace() -> Result<PathBuf> {
    let workspace = default_managed_workspace_path()?;
    fs::create_dir_all(&workspace)?;
    if !workspace.join(".git").is_dir() {
        run_git(&workspace, &["init"])?;
        fs::write(
            workspace.join("README.md"),
            "# PurrCode managed workspace\n\nThis repository contains user-approved local-agent artifacts.\n",
        )?;
        run_git(&workspace, &["add", "README.md"])?;
        run_git(
            &workspace,
            &[
                "-c",
                "user.name=PurrCode",
                "-c",
                "user.email=purrcode@localhost",
                "commit",
                "-m",
                "Initialize PurrCode workspace",
            ],
        )?;
    }
    workspace.canonicalize().map_err(Into::into)
}

fn is_git_repository(path: &Path) -> bool {
    std::process::Command::new("git")
        .args(["rev-parse", "--is-inside-work-tree"])
        .current_dir(path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

fn run_git(repository: &Path, arguments: &[&str]) -> Result<()> {
    let output = std::process::Command::new("git")
        .args(arguments)
        .current_dir(repository)
        .stdin(std::process::Stdio::null())
        .output()?;
    if !output.status.success() {
        bail!(
            "git {} failed: {}",
            arguments.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(())
}

fn load_app_config(path: &Path) -> Result<AppConfig> {
    AppConfig::load(path).with_context(|| format!("load configuration {}", path.display()))
}

#[derive(Deserialize)]
struct CodexConfigFile {
    #[serde(default)]
    codex: CodexBridgeConfig,
}

#[derive(Deserialize, Default)]
struct McpConfigFile {
    #[serde(default)]
    mcp: McpSection,
}

#[derive(Deserialize, Default)]
struct McpSection {
    #[serde(default)]
    servers: BTreeMap<String, McpServerConfig>,
}

fn load_mcp_servers(path: &Path) -> Result<BTreeMap<String, McpServerConfig>> {
    let content = fs::read_to_string(path)
        .with_context(|| format!("load configuration {}", path.display()))?;
    Ok(toml::from_str::<McpConfigFile>(&content)?.mcp.servers)
}

fn load_codex_config(path: &Path) -> Result<CodexBridgeConfig> {
    let content = fs::read_to_string(path)
        .with_context(|| format!("load configuration {}", path.display()))?;
    Ok(toml::from_str::<CodexConfigFile>(&content)?.codex)
}

fn resolve_session_id(store: &SessionStore, value: Option<String>) -> Result<SessionId> {
    match value {
        Some(value) => Ok(SessionId(
            uuid::Uuid::parse_str(&value).context("session ID is not a UUID")?,
        )),
        None => store
            .latest_session_id()?
            .context("no sessions are available"),
    }
}

fn resolve_session(store: &SessionStore, session: &str) -> Result<SessionId> {
    if session == "latest" {
        store
            .list_session_ids()
            .map_err(|e| anyhow::anyhow!("list sessions failed: {e}"))?
            .into_iter()
            .next_back()
            .context("no sessions exist")
    } else {
        Ok(SessionId(
            uuid::Uuid::parse_str(session).context("session ID is not a UUID")?,
        ))
    }
}

fn event_type_name(event: &SessionEvent) -> &'static str {
    use SessionEvent::*;
    match event {
        SessionCreated { .. } => "session_created",
        SessionControlsUpdated { .. } => "session_controls_updated",
        WorkflowPlanCreated { .. } => "workflow_plan_created",
        UsageRecorded { .. } => "usage_recorded",
        ConversationMessageAdded { .. } => "conversation_message_added",
        WorktreeCreated { .. } => "worktree_created",
        SubmodulesPrepared { .. } => "submodules_prepared",
        PlanCreated { .. } => "plan_created",
        PlanRevised { .. } => "plan_revised",
        SpecBundleRecorded { .. } => "spec_bundle_recorded",
        TaskGraphRecorded { .. } => "task_graph_recorded",
        TaskStatusChanged { .. } => "task_status_changed",
        EvidenceLinked { .. } => "evidence_linked",
        ContextCompacted { .. } => "context_compacted",
        CheckpointCompacted { .. } => "checkpoint_compacted",
        ContextAssembled { .. } => "context_assembled",
        SessionPaused { .. } => "session_paused",
        SessionResumed => "session_resumed",
        ModelSelected { .. } => "model_selected",
        SupervisorStarted { .. } => "supervisor_started",
        WorkerFinished { .. } => "worker_finished",
        SupervisorReviewRequired { .. } => "supervisor_review_required",
        ContextIndexed { .. } => "context_indexed",
        ScoutCompleted { .. } => "scout_completed",
        ScoutFailed { .. } => "scout_failed",
        ModelRequestStarted { .. } => "model_request_started",
        ModelRequestFinished { .. } => "model_request_finished",
        ActionProposed { .. } => "action_proposed",
        ActionSuperseded { .. } => "action_superseded",
        JudgmentRecorded { .. } => "judgment_recorded",
        ContextualJudgmentRecorded { .. } => "contextual_judgment_recorded",
        OutcomeJudgmentRecorded { .. } => "outcome_judgment_recorded",
        OutcomeReviewRequired { .. } => "outcome_review_required",
        OutcomeReviewApproved { .. } => "outcome_review_approved",
        ApprovalRecorded { .. } => "approval_recorded",
        ApprovalRejected { .. } => "approval_rejected",
        AuthorizationPersisted { .. } => "authorization_persisted",
        ExecutionStarted { .. } => "execution_started",
        ExecutionFinished { .. } => "execution_finished",
        ActionOutputRecorded { .. } => "action_output_recorded",
        ValidationRecorded { .. } => "validation_recorded",
        CheckpointCreated { .. } => "checkpoint_created",
        WorktreeDispositionRecorded { .. } => "worktree_disposition_recorded",
        SessionCancelled { .. } => "session_cancelled",
        RecoveryRequired { .. } => "recovery_required",
        SessionCompleted => "session_completed",
        SessionFailed { .. } => "session_failed",
        CapabilityGapDetected { .. } => "capability_gap_detected",
        SkillSearchStarted { .. } => "skill_search_started",
        SkillCandidateDiscovered { .. } => "skill_candidate_discovered",
        SkillCandidateRanked { .. } => "skill_candidate_ranked",
        SkillInspectionOpened { .. } => "skill_inspection_opened",
        SkillInstallApproved { .. } => "skill_install_approved",
        SkillInstallRejected { .. } => "skill_install_rejected",
        SkillQualified { .. } => "skill_qualified",
        SkillQualificationStarted { .. } => "skill_qualification_started",
        SkillQualificationFailed { .. } => "skill_qualification_failed",
        SkillInvoked { .. } => "skill_invoked",
        SkillInvocationSucceeded { .. } => "skill_invocation_succeeded",
        SkillInvocationFailed { .. } => "skill_invocation_failed",
        InstalledSkillReused { .. } => "installed_skill_reused",
        InstalledSkillMatched { .. } => "installed_skill_matched",
        ExternalSearchAvoided { .. } => "external_search_avoided",
        SkillUpdated { .. } => "skill_updated",
        SkillRemoved { .. } => "skill_removed",
        ResearchSearchPerformed { .. } => "research_search_performed",
        TerminalActionProposed { .. } => "terminal_action_proposed",
        TerminalJudgmentRecorded { .. } => "terminal_judgment_recorded",
        CompletionRepairRecorded { .. } => "completion_repair_recorded",
    }
}

fn extract_action_id(event: &SessionEvent) -> Option<ActionId> {
    use SessionEvent::*;
    match event {
        ActionProposed { action_id, .. } => Some(*action_id),
        ActionSuperseded { .. } => None,
        JudgmentRecorded { action_id, .. } => Some(*action_id),
        ContextualJudgmentRecorded { action_id, .. } => Some(*action_id),
        ApprovalRecorded { action_id, .. } => Some(*action_id),
        ApprovalRejected { action_id, .. } => Some(*action_id),
        ExecutionStarted { action_id } => Some(*action_id),
        ExecutionFinished { action_id, .. } => Some(*action_id),
        ActionOutputRecorded { action_id, .. } => Some(*action_id),
        ValidationRecorded { action_id, .. } => Some(*action_id),
        AuthorizationPersisted { authorization } => Some(authorization.action_id),
        _ => None,
    }
}

fn event_summary(event: &SessionEvent) -> String {
    use SessionEvent::*;
    match event {
        SessionCreated { objective, .. } => objective.clone(),
        ActionProposed { action, .. } => match action {
            purrcode_runtime_core::ProposedAction::Command(cmd) => {
                format!("command: {}", cmd.program.display())
            }
            purrcode_runtime_core::ProposedAction::WriteFile(wf) => {
                format!("write_file: {}", wf.path.display())
            }
            other => format!("{:?}", std::mem::discriminant(other)),
        },
        JudgmentRecorded { decision, .. } => format!("{decision:?}"),
        ExecutionFinished {
            exit_code,
            truncated,
            ..
        } => format!("exit_code={exit_code:?} truncated={truncated}"),
        ValidationRecorded {
            status, evidence, ..
        } => {
            format!("{status:?}: {evidence}")
        }
        SpecBundleRecorded { bundle, reason } => format!(
            "Specification recorded: {} (revision {}, {} requirement(s)); {}",
            bundle.title,
            bundle.revision,
            bundle.requirements.len(),
            reason
        ),
        TaskGraphRecorded { graph, reason } => format!(
            "Task graph recorded: revision {}, {} task(s); {}",
            graph.revision,
            graph.tasks.len(),
            reason
        ),
        TaskStatusChanged {
            task_id,
            status,
            reason,
        } => format!(
            "Task {} is {}; {}",
            task_id.0,
            humanize_event_label(&format!("{status:?}")),
            reason
        ),
        EvidenceLinked { evidence } => format!(
            "Evidence linked for task {}: {} ({})",
            evidence.task_id.0,
            evidence.summary,
            humanize_event_label(&format!("{:?}", evidence.coverage))
        ),
        SessionFailed { reason } => format!("failed: {reason}"),
        SessionCancelled { reason } => format!("cancelled: {reason}"),
        _ => String::new(),
    }
}

fn humanize_event_label(value: &str) -> String {
    let mut label = value.replace('_', " ").to_ascii_lowercase();
    if let Some(first) = label.get_mut(..1) {
        first.make_ascii_uppercase();
    }
    label
}

#[derive(serde::Serialize)]
struct CiReport {
    session_id: String,
    status: String,
    worktree: Option<PathBuf>,
    validation: CiValidationSummary,
    events: Vec<SessionEvent>,
}

#[derive(Default, serde::Serialize)]
struct CiValidationSummary {
    passed: Vec<String>,
    failed: Vec<String>,
    skipped_by_configuration: Vec<String>,
    unavailable: Vec<String>,
    not_detected: Vec<String>,
    timed_out: Vec<String>,
    uncertain: Vec<String>,
}

impl CiReport {
    fn from_events(session_id: String, view: serde_json::Value, events: Vec<SessionEvent>) -> Self {
        let mut validation = CiValidationSummary::default();
        for event in &events {
            if let SessionEvent::ValidationRecorded {
                status, evidence, ..
            } = event
            {
                match status {
                    ValidationStatus::Passed => validation.passed.push(evidence.clone()),
                    ValidationStatus::Failed => validation.failed.push(evidence.clone()),
                    ValidationStatus::SkippedByConfiguration => {
                        validation.skipped_by_configuration.push(evidence.clone())
                    }
                    ValidationStatus::Unavailable => validation.unavailable.push(evidence.clone()),
                    ValidationStatus::NotDetected => validation.not_detected.push(evidence.clone()),
                    ValidationStatus::TimedOut => validation.timed_out.push(evidence.clone()),
                    ValidationStatus::Uncertain => validation.uncertain.push(evidence.clone()),
                }
            }
        }
        Self {
            session_id,
            status: view["status_code"].as_str().unwrap_or("unknown").into(),
            worktree: view["worktree"].as_str().map(PathBuf::from),
            validation,
            events,
        }
    }
}

fn write_new_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    write_new_atomic_with(path, |temporary| {
        use std::io::Write;
        temporary.write_all(bytes)?;
        Ok(())
    })
}

fn write_new_atomic_with(
    path: &Path,
    writer: impl FnOnce(&mut std::fs::File) -> Result<()>,
) -> Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    writer(temporary.as_file_mut())?;
    temporary.as_file().sync_all()?;
    temporary
        .persist_noclobber(path)
        .map_err(|error| anyhow::anyhow!("persist {}: {}", path.display(), error.error))?;
    Ok(())
}

fn print_accepted_session(result: &serde_json::Value) -> Result<()> {
    let session_id = result["id"]
        .as_str()
        .context("daemon response omitted session ID")?;
    println!("session: {session_id}");
    println!("follow-up: purrcode gui --session {session_id}");
    println!("{}", serde_json::to_string_pretty(result)?);
    Ok(())
}

async fn daemon_json(
    method: reqwest::Method,
    base_url: &str,
    path: &str,
    body: Option<serde_json::Value>,
    token_file: &Path,
) -> Result<serde_json::Value> {
    let token = fs::read_to_string(token_file).with_context(|| {
        format!(
            "read daemon token {}; start `purrcode serve` first",
            token_file.display()
        )
    })?;
    let url = format!("{}{}", base_url.trim_end_matches('/'), path);
    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(3))
        .timeout(std::time::Duration::from_secs(30))
        .build()?;
    let mut request = client.request(method, &url).bearer_auth(token.trim());
    if let Some(body) = body {
        request = request.json(&body);
    }
    let response = request
        .send()
        .await
        .with_context(|| format!("connect to daemon at {base_url}; run `purrcode serve`"))?;
    let status = response.status();
    let bytes = response.bytes().await?;
    let value: serde_json::Value = serde_json::from_slice(&bytes)
        .with_context(|| format!("daemon returned non-JSON HTTP {status}"))?;
    if !status.is_success() {
        bail!("daemon returned HTTP {status}: {value}");
    }
    Ok(value)
}

async fn resolve_daemon_session(
    daemon_url: &str,
    token_file: &Path,
    selected: Option<String>,
) -> Result<String> {
    if let Some(id) = selected {
        uuid::Uuid::parse_str(&id).context("session ID is not a UUID")?;
        return Ok(id);
    }
    let sessions = daemon_json(
        reqwest::Method::GET,
        daemon_url,
        "/v1/sessions",
        None,
        token_file,
    )
    .await?;
    sessions
        .as_array()
        .and_then(|items| items.first())
        .and_then(|session| session["id"].as_str())
        .map(str::to_owned)
        .context("daemon has no sessions")
}

fn session_worktree_from_store(
    store: &SessionStore,
    session_id: SessionId,
) -> Result<SessionWorktree> {
    let state = store.load(session_id)?;
    Ok(SessionWorktree {
        session_id,
        source_repository: state
            .repository
            .context("session source repository is missing")?,
        path: state.worktree.context("session worktree is missing")?,
        base_head: state.base_head.context("session base HEAD is missing")?,
        source_was_dirty: false,
        initialized_submodules: Vec::new(),
        unavailable_submodules: Vec::new(),
    })
}

#[cfg(test)]
mod cli_tests {
    use super::*;
    use purrcode_runtime_core::AuthorityMode;

    #[test]
    fn the_ide_command_never_reaches_for_a_browser_or_a_third_party_editor() {
        // PRD §22.1/§29.1: `purrcode ide` opens PurrCode's own window. This
        // guards the whole dispatch path against a regression that shells out
        // to a browser opener or to somebody else's editor CLI.
        let source = include_str!("main.rs");
        let launcher = source
            .split("fn launch_ide(")
            .nth(1)
            .and_then(|rest| rest.split("\nfn ").next())
            .expect("launch_ide must exist");
        for forbidden in [
            "open_browser",
            "xdg-open",
            "rundll32",
            "--reuse-window",
            "vscode://",
            "code-insiders",
            "codium",
            "http://",
        ] {
            assert!(
                !launcher.contains(forbidden),
                "`purrcode ide` must not reference {forbidden}"
            );
        }
        assert!(
            launcher.contains("purrcode_ide::run"),
            "`purrcode ide` must open the native PurrCode window"
        );
    }

    #[test]
    fn ide_does_not_resume_a_repository_session_implicitly_or_cross_workspaces() {
        let temporary = tempfile::tempdir().unwrap();
        let repository = temporary.path().join("repository");
        let other_repository = temporary.path().join("other");
        fs::create_dir(&repository).unwrap();
        fs::create_dir(&other_repository).unwrap();
        let mut store = SessionStore::in_memory().unwrap();
        let session_id = SessionId::new();
        store
            .append(
                session_id,
                &SessionEvent::SessionCreated {
                    objective: "fixture".into(),
                    repository: repository.clone(),
                    authority_mode: AuthorityMode::Governed,
                },
            )
            .unwrap();

        validate_ide_session(&store, &session_id.0.to_string(), &repository).unwrap();
        let mismatch = validate_ide_session(&store, &session_id.0.to_string(), &other_repository)
            .unwrap_err()
            .to_string();
        assert!(mismatch.contains("belongs to"));
        let missing = validate_ide_session(&store, &SessionId::new().0.to_string(), &repository)
            .unwrap_err()
            .to_string();
        assert!(missing.contains("was not found"));

        // Keep the launcher policy explicit in source: opening a folder never
        // calls a latest-session resolver. Only `--session` reaches the
        // ownership check above.
        let source = include_str!("main.rs");
        let open_ide = source
            .split("fn open_ide(")
            .nth(1)
            .and_then(|rest| rest.split("\nasync fn dispatch").next())
            .expect("open_ide must exist");
        assert!(!open_ide.contains("latest_safe_session"));
    }

    #[test]
    fn live_benchmark_uses_the_requested_whole_task_timeout() {
        assert_eq!(
            live_benchmark_timeout(300).unwrap(),
            std::time::Duration::from_secs(300)
        );
        assert!(live_benchmark_timeout(0).is_err());
    }

    #[test]
    fn signed_manifest_checksum_matching_is_exact() {
        let digest = hex::encode(Sha256::digest(b"release"));
        let manifest = format!("{digest}  purrcode.tar.gz\n");
        verify_checksum_manifest(&manifest, "purrcode.tar.gz", b"release").unwrap();
        assert!(verify_checksum_manifest(&manifest, "purrcode.tar.gz", b"tampered").is_err());
        assert!(verify_checksum_manifest(&manifest, "../purrcode.tar.gz", b"release").is_err());
    }

    #[test]
    fn release_artifact_matches_the_current_platform() {
        let name = release_artifact_name().unwrap();
        assert!(name.starts_with("purrcode-"));
        assert!(name.ends_with(if cfg!(windows) { ".zip" } else { ".tar.gz" }));
    }

    #[test]
    fn daemon_health_requires_the_current_product_and_api_identity() {
        assert!(matches!(
            assess_daemon_health(&serde_json::json!({
                "product": "purrcode",
                "daemon_api_version": DAEMON_API_VERSION,
                "native_ide_api_version": NATIVE_IDE_API_VERSION,
                "native_ide_build_fingerprint": NATIVE_IDE_BUILD_FINGERPRINT,
                "native_ide_capabilities": NATIVE_IDE_CAPABILITIES
            })),
            DaemonCompatibility::Compatible
        ));
        assert!(matches!(
            assess_daemon_health(&serde_json::json!({
                "status": "ok",
                "sqlite_integrity": true
            })),
            DaemonCompatibility::Incompatible(detail)
                if detail.contains("health identity")
        ));
        assert!(matches!(
            assess_daemon_health(&serde_json::json!({
                "product": "purrcode"
            })),
            DaemonCompatibility::Incompatible(detail)
                if detail.contains("missing (legacy daemon)")
        ));
        assert!(matches!(
            assess_daemon_health(&serde_json::json!({
                "product": "purrcode",
                "daemon_api_version": DAEMON_API_VERSION
            })),
            DaemonCompatibility::Incompatible(detail)
                if detail.contains("native IDE API version")
        ));
        assert!(matches!(
            assess_daemon_health(&serde_json::json!({
                "product": "purrcode",
                "daemon_api_version": DAEMON_API_VERSION + 1,
                "native_ide_api_version": NATIVE_IDE_API_VERSION,
                "native_ide_build_fingerprint": NATIVE_IDE_BUILD_FINGERPRINT,
                "native_ide_capabilities": NATIVE_IDE_CAPABILITIES
            })),
            DaemonCompatibility::Incompatible(detail)
                if detail.contains("daemon API version")
        ));
        assert!(matches!(
            assess_daemon_health(&serde_json::json!({
                "product": "purrcode",
                "daemon_api_version": DAEMON_API_VERSION,
                "native_ide_api_version": NATIVE_IDE_API_VERSION,
                "native_ide_build_fingerprint": NATIVE_IDE_BUILD_FINGERPRINT,
                "native_ide_capabilities": ["sessions.start"]
            })),
            DaemonCompatibility::Incompatible(detail)
                if detail.contains("native IDE capabilities missing")
        ));
        assert!(matches!(
            assess_daemon_health(&serde_json::json!({
                "product": "purrcode",
                "daemon_api_version": DAEMON_API_VERSION,
                "native_ide_api_version": NATIVE_IDE_API_VERSION,
                "native_ide_build_fingerprint": "old-native-ide-build",
                "native_ide_capabilities": NATIVE_IDE_CAPABILITIES
            })),
            DaemonCompatibility::Incompatible(detail)
                if detail.contains("native IDE build fingerprint")
        ));
    }

    #[test]
    fn binary_install_is_atomic_and_rollback_swaps_versions() {
        let temporary = tempfile::tempdir().unwrap();
        let staged = temporary.path().join("staged");
        let destination = temporary.path().join("bin");
        fs::create_dir_all(&staged).unwrap();
        fs::create_dir_all(&destination).unwrap();
        for name in release_binary_names() {
            fs::write(staged.join(name), format!("new-{name}")).unwrap();
            fs::write(destination.join(name), format!("old-{name}")).unwrap();
        }
        install_staged_binaries(&staged, &destination).unwrap();
        for name in release_binary_names() {
            assert_eq!(
                fs::read_to_string(destination.join(name)).unwrap(),
                format!("new-{name}")
            );
            assert_eq!(
                fs::read_to_string(destination.join(format!("{name}.previous"))).unwrap(),
                format!("old-{name}")
            );
        }
        rollback_binaries(&destination).unwrap();
        for name in release_binary_names() {
            assert_eq!(
                fs::read_to_string(destination.join(name)).unwrap(),
                format!("old-{name}")
            );
            assert_eq!(
                fs::read_to_string(destination.join(format!("{name}.previous"))).unwrap(),
                format!("new-{name}")
            );
        }
    }

    #[test]
    fn upgrade_compatibility_check_accepts_an_empty_repository() {
        let temporary = tempfile::tempdir().unwrap();
        assert_eq!(
            check_upgrade_plugin_compatibility(temporary.path()).unwrap(),
            0
        );
        assert!(tool_available_on_path("git"));
        assert!(!tool_available_on_path("../git"));
    }

    #[test]
    fn research_redaction_pseudonymizes_identifiers_deterministically() {
        let private = "private-org/secret-skill";
        let first = redacted_identifier(private);
        assert_eq!(first, redacted_identifier(private));
        assert!(first.starts_with("anon-"));
        assert!(!first.contains(private));
    }

    #[test]
    fn research_export_source_events_keep_their_durable_timestamps() {
        let session_id = SessionId::new();
        let mut store = SessionStore::in_memory().unwrap();
        let before = Utc::now();
        store
            .append(
                session_id,
                &SessionEvent::CapabilityGapDetected {
                    gap_description: "private capability".into(),
                    task_context: "private prompt".into(),
                },
            )
            .unwrap();
        let after = Utc::now();
        let timestamped = store.timestamped_events(session_id).unwrap();
        assert_eq!(timestamped.len(), 1);
        assert!(timestamped[0].0 >= before && timestamped[0].0 <= after);
        assert!(matches!(
            timestamped[0].1,
            SessionEvent::CapabilityGapDetected { .. }
        ));
    }

    #[test]
    fn interrupted_atomic_export_never_publishes_a_partial_destination() {
        let temporary = tempfile::tempdir().unwrap();
        let destination = temporary.path().join("evidence.json");
        let result = write_new_atomic_with(&destination, |file| {
            use std::io::Write;
            file.write_all(b"partial")?;
            anyhow::bail!("injected export interruption")
        });
        assert!(result.is_err());
        assert!(!destination.exists());
        assert!(
            std::fs::read_dir(temporary.path())
                .unwrap()
                .all(|entry| entry.unwrap().path() != destination)
        );
    }
}
