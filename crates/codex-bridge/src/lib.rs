//! Isolated Codex CLI worker adapter.
//!
//! PurrCode judges the task and final diff. It does not claim action-level visibility into
//! Codex's internal reasoning or tool decisions.

use purrcode_repository_engine::{
    RepositoryEngine, RepositoryError, SessionWorktree, WorktreeEffects,
};
use purrcode_runtime_core::SessionId;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use thiserror::Error;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::time::{Duration, timeout};

const MAX_EVENT_COUNT: usize = 10_000;
const MAX_EVENT_BYTES: usize = 8 * 1024 * 1024;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct CodexBridgeConfig {
    pub enabled: bool,
    pub binary: PathBuf,
    pub execution_mode: String,
    pub timeout_seconds: u64,
    pub inherit_auth: bool,
    pub require_final_diff_judgment: bool,
    pub allow_active_tree_write: bool,
}

impl Default for CodexBridgeConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            binary: PathBuf::from("codex"),
            execution_mode: "worktree".into(),
            timeout_seconds: 3600,
            inherit_auth: true,
            require_final_diff_judgment: true,
            allow_active_tree_write: false,
        }
    }
}

impl CodexBridgeConfig {
    pub fn validate(&self) -> Result<(), CodexBridgeError> {
        if self.execution_mode != "worktree" {
            return Err(CodexBridgeError::UnsafeConfiguration(
                "execution_mode must be `worktree`".into(),
            ));
        }
        if self.allow_active_tree_write {
            return Err(CodexBridgeError::UnsafeConfiguration(
                "allow_active_tree_write must be false".into(),
            ));
        }
        if !self.require_final_diff_judgment {
            return Err(CodexBridgeError::UnsafeConfiguration(
                "require_final_diff_judgment must be true".into(),
            ));
        }
        if self.timeout_seconds == 0 {
            return Err(CodexBridgeError::UnsafeConfiguration(
                "timeout_seconds must be nonzero".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CodexDoctorReport {
    pub version: String,
    pub adapter: String,
    pub authenticated: bool,
    pub json_events: bool,
    pub noninteractive: bool,
    pub worktree_only: bool,
}

#[derive(Clone, Debug)]
pub struct CodexBridgeResult {
    pub session_id: SessionId,
    pub worktree: SessionWorktree,
    pub exit_code: Option<i32>,
    pub events: Vec<Value>,
    pub dropped_events: usize,
    pub effects: WorktreeEffects,
    pub requires_independent_diff_judgment: bool,
}

pub struct CodexBridge {
    config: CodexBridgeConfig,
}

impl CodexBridge {
    pub fn new(config: CodexBridgeConfig) -> Result<Self, CodexBridgeError> {
        config.validate()?;
        Ok(Self { config })
    }

    pub async fn doctor(&self) -> Result<CodexDoctorReport, CodexBridgeError> {
        let version = command_text(&self.config.binary, &["--version"], None, 30).await?;
        let adapter = adapter_for_version(&version)?;
        let help = command_text(&self.config.binary, &["exec", "--help"], None, 30).await?;
        let login = command_text(&self.config.binary, &["login", "status"], None, 30).await;
        let json_events = help.contains("--json");
        let noninteractive = help.contains("non-interactively") || help.contains("noninteractive");
        if !json_events || !noninteractive {
            return Err(CodexBridgeError::UnsupportedCli(
                "Codex exec must support noninteractive JSON events".into(),
            ));
        }
        Ok(CodexDoctorReport {
            version: version.trim().into(),
            adapter: adapter.into(),
            authenticated: login
                .as_deref()
                .is_ok_and(|output| output.to_ascii_lowercase().contains("logged in")),
            json_events,
            noninteractive,
            worktree_only: true,
        })
    }

    pub async fn run(
        &self,
        repository: &Path,
        objective: &str,
    ) -> Result<CodexBridgeResult, CodexBridgeError> {
        if !self.config.enabled {
            return Err(CodexBridgeError::Disabled);
        }
        if objective.trim().is_empty() {
            return Err(CodexBridgeError::EmptyObjective);
        }
        let session_id = SessionId::new();
        let worktree = RepositoryEngine::create_worktree(repository, session_id).await?;
        let mut process = Command::new(&self.config.binary);
        process
            .arg("exec")
            .arg("--json")
            .arg("--ephemeral")
            .arg("--sandbox")
            .arg("workspace-write")
            .arg("--cd")
            .arg(&worktree.path)
            .arg(objective)
            .current_dir(&worktree.path)
            .env_clear()
            .envs(codex_environment(self.config.inherit_auth))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut child = process.spawn()?;
        let stdout = child.stdout.take().ok_or(CodexBridgeError::MissingPipe)?;
        let stderr = child.stderr.take().ok_or(CodexBridgeError::MissingPipe)?;
        let stdout_task = tokio::spawn(collect_json_events(stdout));
        let stderr_task = tokio::spawn(collect_bounded_text(stderr));
        let status = timeout(
            Duration::from_secs(self.config.timeout_seconds),
            child.wait(),
        )
        .await
        .map_err(|_| CodexBridgeError::Timeout)??;
        let (events, dropped_events) = stdout_task.await??;
        let stderr = stderr_task.await??;
        if !status.success() {
            return Err(CodexBridgeError::WorkerFailed {
                exit_code: status.code(),
                stderr,
                worktree: worktree.path,
            });
        }
        let effects = RepositoryEngine::effects(&worktree).await?;
        Ok(CodexBridgeResult {
            session_id,
            worktree,
            exit_code: status.code(),
            events,
            dropped_events,
            effects,
            requires_independent_diff_judgment: true,
        })
    }
}

fn adapter_for_version(version: &str) -> Result<&'static str, CodexBridgeError> {
    let numeric = version
        .split_whitespace()
        .find(|part| {
            part.chars()
                .next()
                .is_some_and(|value| value.is_ascii_digit())
        })
        .ok_or_else(|| CodexBridgeError::UnsupportedCli("version is not parseable".into()))?;
    let mut parts = numeric.split(['.', '-']);
    let major = parts.next().and_then(|value| value.parse::<u64>().ok());
    let minor = parts.next().and_then(|value| value.parse::<u64>().ok());
    match (major, minor) {
        (Some(0), Some(minor)) if minor >= 100 => Ok("jsonl-v1"),
        (Some(major), _) if major >= 1 => Ok("jsonl-v1"),
        _ => Err(CodexBridgeError::UnsupportedCli(format!(
            "no versioned adapter for {version}"
        ))),
    }
}

async fn collect_json_events(
    stdout: tokio::process::ChildStdout,
) -> Result<(Vec<Value>, usize), CodexBridgeError> {
    let mut lines = BufReader::new(stdout).lines();
    let mut events = VecDeque::new();
    let mut bytes = 0;
    let mut dropped = 0;
    while let Some(line) = lines.next_line().await? {
        bytes += line.len();
        if bytes > MAX_EVENT_BYTES {
            return Err(CodexBridgeError::OutputLimit);
        }
        let event: Value = serde_json::from_str(&line)
            .map_err(|error| CodexBridgeError::InvalidEvent(error.to_string()))?;
        if events.len() == MAX_EVENT_COUNT {
            events.pop_front();
            dropped += 1;
        }
        events.push_back(event);
    }
    Ok((events.into(), dropped))
}

async fn collect_bounded_text(
    stderr: tokio::process::ChildStderr,
) -> Result<String, CodexBridgeError> {
    let mut lines = BufReader::new(stderr).lines();
    let mut output = String::new();
    while let Some(line) = lines.next_line().await? {
        if output.len() + line.len() + 1 > MAX_EVENT_BYTES {
            return Err(CodexBridgeError::OutputLimit);
        }
        output.push_str(&line);
        output.push('\n');
    }
    Ok(output)
}

async fn command_text(
    binary: &Path,
    arguments: &[&str],
    directory: Option<&Path>,
    timeout_seconds: u64,
) -> Result<String, CodexBridgeError> {
    let mut command = Command::new(binary);
    command
        .args(arguments)
        .env_clear()
        .envs(codex_environment(true))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    if let Some(directory) = directory {
        command.current_dir(directory);
    }
    let output = timeout(Duration::from_secs(timeout_seconds), command.output())
        .await
        .map_err(|_| CodexBridgeError::Timeout)??;
    if !output.status.success() {
        return Err(CodexBridgeError::WorkerFailed {
            exit_code: output.status.code(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            worktree: directory.unwrap_or_else(|| Path::new(".")).into(),
        });
    }
    let mut combined = String::from_utf8(output.stdout)?;
    if !output.stderr.is_empty() {
        combined.push_str(&String::from_utf8(output.stderr)?);
    }
    Ok(combined)
}

fn codex_environment(inherit_auth: bool) -> Vec<(String, String)> {
    let mut allowed = vec!["PATH", "TMPDIR", "LANG", "LC_ALL", "TERM"];
    if inherit_auth {
        allowed.extend(["HOME", "CODEX_HOME"]);
    }
    allowed
        .into_iter()
        .filter_map(|key| std::env::var(key).ok().map(|value| (key.into(), value)))
        .collect()
}

#[derive(Debug, Error)]
pub enum CodexBridgeError {
    #[error("Codex Bridge is disabled")]
    Disabled,
    #[error("Codex Bridge objective cannot be empty")]
    EmptyObjective,
    #[error("unsafe Codex Bridge configuration: {0}")]
    UnsafeConfiguration(String),
    #[error("unsupported Codex CLI: {0}")]
    UnsupportedCli(String),
    #[error("Codex worker timed out")]
    Timeout,
    #[error("Codex worker output exceeded its bound")]
    OutputLimit,
    #[error("Codex worker emitted invalid JSON event: {0}")]
    InvalidEvent(String),
    #[error("Codex worker failed with {exit_code:?} in retained worktree {worktree}: {stderr}")]
    WorkerFailed {
        exit_code: Option<i32>,
        stderr: String,
        worktree: PathBuf,
    },
    #[error("Codex process pipe was unavailable")]
    MissingPipe,
    #[error("Codex process I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("Codex output was not UTF-8: {0}")]
    Utf8(#[from] std::string::FromUtf8Error),
    #[error("Codex event collector failed: {0}")]
    Join(#[from] tokio::task::JoinError),
    #[error("worktree isolation failed: {0}")]
    Repository(#[from] RepositoryError),
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use std::process::Command as StdCommand;

    #[test]
    fn active_tree_write_can_never_be_enabled() {
        let config = CodexBridgeConfig {
            allow_active_tree_write: true,
            ..CodexBridgeConfig::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn unsupported_versions_fail_closed() {
        assert!(adapter_for_version("codex-cli 0.42.0").is_err());
        assert_eq!(
            adapter_for_version("codex-cli 0.145.0-alpha.30").unwrap(),
            "jsonl-v1"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn fake_worker_can_only_modify_isolated_worktree() {
        use std::os::unix::fs::PermissionsExt;
        let repository = tempfile::tempdir().unwrap();
        assert!(
            StdCommand::new("git")
                .args(["init", "-q"])
                .current_dir(repository.path())
                .status()
                .unwrap()
                .success()
        );
        std::fs::write(repository.path().join("tracked.txt"), "base").unwrap();
        assert!(
            StdCommand::new("git")
                .args(["add", "tracked.txt"])
                .current_dir(repository.path())
                .status()
                .unwrap()
                .success()
        );
        assert!(
            StdCommand::new("git")
                .args([
                    "-c",
                    "user.name=PurrCode",
                    "-c",
                    "user.email=test@local.invalid",
                    "commit",
                    "-q",
                    "-m",
                    "base",
                ])
                .current_dir(repository.path())
                .status()
                .unwrap()
                .success()
        );
        let fake = repository.path().join("fake-codex");
        std::fs::write(
            &fake,
            "#!/bin/sh\nprintf '{\"type\":\"done\"}\\n'\nprintf worker > worker.txt\n",
        )
        .unwrap();
        let mut permissions = std::fs::metadata(&fake).unwrap().permissions();
        permissions.set_mode(0o700);
        std::fs::set_permissions(&fake, permissions).unwrap();
        let bridge = CodexBridge::new(CodexBridgeConfig {
            enabled: true,
            binary: fake,
            ..CodexBridgeConfig::default()
        })
        .unwrap();
        let result = bridge
            .run(repository.path(), "make an isolated change")
            .await
            .unwrap();
        assert!(result.worktree.path.join("worker.txt").is_file());
        assert!(!repository.path().join("worker.txt").exists());
        assert_eq!(
            result.effects.changed_files,
            vec![PathBuf::from("worker.txt")]
        );
        assert!(result.requires_independent_diff_judgment);
    }
}
