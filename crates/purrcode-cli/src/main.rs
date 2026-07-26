use anyhow::{bail, Context, Result};
use chrono::Utc;
use clap::{Parser, Subcommand};
use directories::ProjectDirs;
use futures::future::join_all;
use purrcode_claw::{sandbox_capability, ToolRuntime};
use purrcode_codex_bridge::{CodexBridge, CodexBridgeConfig};
use purrcode_daemon::{bind_and_report, DaemonConfig};
use purrcode_golden_suite::GoldenCatalog;
use purrcode_mcp_host::{
    discover_skills, install_skill, uninstall_skill, verify_installed_skill, McpHost,
    McpServerConfig,
};
use purrcode_ninelives::SessionStore;
use purrcode_pawgate::{resolve_policy_path, Policy};
use purrcode_provider_gateway::{
    delete_keychain_credential, qualify_model, set_keychain_credential, AppConfig,
    JudgmentRuntimeConfig, ModelId, ModelsConfig, PrivacyConfig, PrivacyMode, ProviderConfig,
    ProviderRouter,
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
use tracing_subscriber::EnvFilter;
use zeroize::Zeroize;

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
    /// Start an isolated, resumable native-agent session.
    Run {
        objective: String,
        #[arg(long)]
        repository: Option<PathBuf>,
    },
    /// Create a durable repository-aware plan without executing or modifying files.
    Plan {
        objective: String,
        #[arg(long)]
        repository: Option<PathBuf>,
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
    /// Check the local persistence subsystem.
    Doctor,
    /// Resume the latest or selected session.
    Resume { session: Option<String> },
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
    /// Roll back all agent-owned changes inside the isolated worktree.
    Rollback { session: Option<String> },
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
    /// Store or remove provider secrets in the operating-system credential store.
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
}

#[derive(Subcommand)]
enum CredentialCommand {
    /// Prompt securely for a provider API key and configure that provider to use it.
    Set {
        provider: String,
        #[arg(long)]
        name: Option<String>,
    },
    /// Remove a named secret from the operating-system credential store.
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
enum BenchmarkCommand {
    Audit {
        #[arg(long)]
        catalog: Option<PathBuf>,
    },
    Baseline {
        #[arg(long)]
        catalog: Option<PathBuf>,
    },
    /// Drive coding fixtures through the running daemon and score real agent outcomes.
    Live {
        #[arg(long)]
        catalog: Option<PathBuf>,
        #[arg(long)]
        max_tasks: Option<usize>,
        /// Whole-task deadline for each benchmark case.
        #[arg(long, default_value_t = 300)]
        timeout_seconds: u64,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn")),
        )
        .without_time()
        .init();
    let cli = Cli::parse();
    let config_path = cli.config.unwrap_or(default_config_path()?);
    let daemon_url = cli.daemon_url;
    let daemon_token = default_daemon_token_path()?;
    let requested_database = cli.database;
    let Some(command) = cli.command else {
        if !config_path.is_file() {
            bail!("PurrCode is not initialized; run `purrcode init`");
        }
        let database = requested_database
            .clone()
            .unwrap_or(default_database_path()?);
        ensure_daemon_started(&config_path, &database, &daemon_url, &daemon_token, false).await?;
        let current = std::env::current_dir()?.canonicalize()?;
        let repository = if is_git_repository(&current) {
            current
        } else {
            let workspace = default_managed_workspace_path()?;
            if !workspace.join(".git").is_dir() {
                bail!(
                    "current directory is not a Git repository and managed workspace is missing; run `purrcode init`"
                );
            }
            workspace
        };
        purrcode_tui::run(TuiConfig {
            daemon_url,
            token_file: daemon_token,
            repository,
        })
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
        Command::Run {
            objective,
            repository,
        } => {
            let repository = canonical_repository(repository)?;
            let result = daemon_json(
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
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
        Command::Plan {
            objective,
            repository,
        } => {
            let repository = canonical_repository(repository)?;
            let result = daemon_json(
                reqwest::Method::POST,
                &daemon_url,
                "/v1/sessions",
                Some(serde_json::json!({
                    "objective": objective,
                    "repository": repository,
                    "plan_only": true
                })),
                &daemon_token,
            )
            .await?;
            println!("{}", serde_json::to_string_pretty(&result)?);
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
                },
            )?;
            store.append(
                session_id,
                &SessionEvent::ActionProposed {
                    action_id,
                    action: action.clone(),
                },
            )?;
            let decision = policy.evaluate(&action, &repository);
            store.append(
                session_id,
                &SessionEvent::JudgmentRecorded {
                    action_id,
                    decision: decision.clone(),
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
        Command::Doctor => {
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
        }
        Command::Resume { session } => {
            let session = resolve_daemon_session(&daemon_url, &daemon_token, session).await?;
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
        Command::Rollback { session } => {
            let session_id = resolve_session_id(&store, session)?;
            let worktree = session_worktree_from_store(&store, session_id)?;
            RepositoryEngine::rollback_all(&worktree).await?;
            store.append(
                session_id,
                &SessionEvent::WorktreeDispositionRecorded {
                    strategy: "rollback_all".into(),
                    detail: "agent-owned worktree changes rolled back".into(),
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
                    let router = ProviderRouter::from_config(&config)?;
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
            ProviderCommand::Doctor => {
                let config = load_app_config(&config_path)?;
                let router = ProviderRouter::from_config(&config)?;
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
                let stored = set_keychain_credential(&credential_name, &secret);
                secret.zeroize();
                stored?;
                if let Err(error) = config.save(&config_path) {
                    let _ = delete_keychain_credential(&credential_name);
                    return Err(error.into());
                }
                println!("credential stored in the operating-system credential store");
                println!("provider: {provider}");
                println!("config: {}", config_path.display());
            }
            CredentialCommand::Delete { name } => {
                delete_keychain_credential(&name)?;
                println!("credential `{name}` removed from the operating-system credential store");
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
                let repository = canonical_repository(repository)?;
                let root = repository.join(".purrcode/skills");
                let source = source
                    .canonicalize()
                    .with_context(|| format!("resolve skill package {}", source.display()))?;
                let installed = install_skill(&source, &root)?;
                println!("{}", serde_json::to_string_pretty(&installed)?);
                println!("installation: {}", root.join(&installed.name).display());
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
                },
            )?;
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
                    decision,
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
                    if let Ok(events) = store.events(*sid) {
                        for event in &events {
                            match event {
                                SessionEvent::ResearchSearchPerformed {
                                    query,
                                    url,
                                    content_digest,
                                    excerpt,
                                } => {
                                    all_events.push(ResearchEvent {
                                        event_type: "ResearchSearchPerformed".into(),
                                        timestamp: Utc::now(),
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
                                        timestamp: Utc::now(),
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
                                        timestamp: Utc::now(),
                                        session_id: *sid,
                                        data: if redacted {
                                            serde_json::json!({ "skill_id": skill_id })
                                        } else {
                                            serde_json::json!({ "skill_id": skill_id, "tool_name": tool_name })
                                        },
                                    });
                                }
                                SessionEvent::SkillInstallApproved { skill_id, scope } => {
                                    all_events.push(ResearchEvent {
                                        event_type: "SkillInstallApproved".into(),
                                        timestamp: Utc::now(),
                                        session_id: *sid,
                                        data: if redacted {
                                            serde_json::json!({ "skill_id": skill_id })
                                        } else {
                                            serde_json::json!({ "skill_id": skill_id, "scope": scope })
                                        },
                                    });
                                }
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
        Command::Benchmark { command } => {
            let catalog_path = match &command {
                BenchmarkCommand::Audit { catalog }
                | BenchmarkCommand::Baseline { catalog }
                | BenchmarkCommand::Live { catalog, .. } => {
                    catalog.clone().unwrap_or_else(default_golden_catalog_path)
                }
            };
            let catalog = GoldenCatalog::load(&catalog_path)?;
            let root = catalog_path.parent().unwrap_or_else(|| Path::new("."));
            match command {
                BenchmarkCommand::Audit { .. } => {
                    println!("{}", serde_json::to_string_pretty(&catalog.audit(root)?)?);
                }
                BenchmarkCommand::Baseline { .. } => {
                    let report = catalog.run_baselines(root).await;
                    println!("{}", serde_json::to_string_pretty(&report)?);
                    if report.failed > 0 {
                        bail!("{} golden baseline cases failed", report.failed);
                    }
                }
                BenchmarkCommand::Live {
                    max_tasks,
                    timeout_seconds,
                    ..
                } => {
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
                    println!("{}", serde_json::to_string_pretty(&report)?);
                    if report.failed > 0 {
                        bail!("{} live golden cases failed", report.failed);
                    }
                }
            }
        }
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
    status: String,
    elapsed_ms: u128,
    changed_paths: Vec<PathBuf>,
    missing_expected_paths: Vec<PathBuf>,
    forbidden_changed_paths: Vec<PathBuf>,
    validation: String,
    model_calls: usize,
    approvals: usize,
    detail: String,
}

#[derive(Debug, Serialize)]
struct LiveBenchmarkReport {
    cases: Vec<LiveBenchmarkCase>,
    passed: usize,
    failed: usize,
    accuracy_percent: f64,
    safety_percent: f64,
    median_latency_ms: u128,
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
    let tasks: Vec<_> = catalog
        .tasks
        .iter()
        .filter(|task| task.category == "coding" && task.fixture.is_some())
        .take(max_tasks.unwrap_or(usize::MAX))
        .collect();
    if tasks.is_empty() {
        bail!("live benchmark selected no coding fixtures");
    }
    let mut cases = Vec::new();
    for task in tasks {
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
        let mut approvals = 0;
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
            .count();
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
        let passed = view.status_code == "completed"
            && missing_expected_paths.is_empty()
            && forbidden_changed_paths.is_empty()
            && validation == "passed";
        cases.push(LiveBenchmarkCase {
            id: task.id.clone(),
            status: if passed { "passed" } else { "failed" }.into(),
            elapsed_ms: started.elapsed().as_millis(),
            changed_paths,
            missing_expected_paths,
            forbidden_changed_paths,
            validation,
            model_calls,
            approvals,
            detail: terminal_detail,
        });
    }
    let passed = cases.iter().filter(|case| case.status == "passed").count();
    let failed = cases.len() - passed;
    let safe = cases
        .iter()
        .filter(|case| case.forbidden_changed_paths.is_empty())
        .count();
    let mut latencies: Vec<_> = cases.iter().map(|case| case.elapsed_ms).collect();
    latencies.sort_unstable();
    Ok(LiveBenchmarkReport {
        accuracy_percent: passed as f64 * 100.0 / cases.len() as f64,
        safety_percent: safe as f64 * 100.0 / cases.len() as f64,
        median_latency_ms: latencies[latencies.len() / 2],
        passed,
        failed,
        cases,
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
    if let Ok(response) = client.get("http://127.0.0.1:11434/api/tags").send().await {
        if response.status().is_success() {
            let value: serde_json::Value = response.json().await?;
            discovered_models = value["models"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|model| model["name"].as_str().map(str::to_owned))
                .collect();
            if !discovered_models.is_empty() {
                provider_name = Some("ollama".to_owned());
                provider_config = Some(ProviderConfig::Ollama {
                    base_url: url::Url::parse("http://127.0.0.1:11434/v1/")?,
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
    let (provider_name, provider_config) = provider_name.zip(provider_config).context(
        "no local model server was discovered; start Ollama or LM Studio with at least one model",
    )?;
    discovered_models.sort();
    discovered_models.dedup();
    let coder = format!("{provider_name}/{}", discovered_models[0]);
    let judge = format!(
        "{provider_name}/{}",
        discovered_models.get(1).unwrap_or(&discovered_models[0])
    );
    let allow_same_model = coder == judge;
    let mut roles = BTreeMap::new();
    for role in ["router", "summarizer", "planner", "coder", "reviewer"] {
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
            "warning: only one model was found; coder/judge separation is reduced until a second model is installed"
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
    if daemon_is_ready(daemon_url, token_file).await {
        if announce {
            println!("daemon: already ready at {daemon_url}");
        }
        return Ok(());
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
        if daemon_is_ready(daemon_url, token_file).await {
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

async fn daemon_is_ready(daemon_url: &str, token_file: &Path) -> bool {
    if !token_file.is_file() {
        return false;
    }
    daemon_json(
        reqwest::Method::GET,
        daemon_url,
        "/v1/health",
        None,
        token_file,
    )
    .await
    .is_ok()
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
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    use std::io::Write;
    temporary.write_all(bytes)?;
    temporary.as_file().sync_all()?;
    temporary
        .persist_noclobber(path)
        .map_err(|error| anyhow::anyhow!("persist {}: {}", path.display(), error.error))?;
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
}
