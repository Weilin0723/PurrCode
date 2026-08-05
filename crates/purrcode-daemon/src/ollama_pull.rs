//! Exact-authorized, shell-free Ollama model pulling.
//!
//! Pulling mutates Ollama's external model store and needs network access, so it intentionally
//! does not use Claw's repository-only, no-network command backend. This dedicated adapter keeps
//! the same trust invariant: the exact executable, executable digest, arguments, working
//! directory, and constraints are durably authorized, then independently rechecked and consumed
//! immediately before process creation.

use chrono::Utc;
use purrcode_ninelives::SessionStore;
use purrcode_runtime_core::{
    ActionConstraints, ActionId, ApprovalAuthority, Authorization, CommandAction, ProposedAction,
    SessionEvent, SessionId, ValidationStatus,
};
use serde::Serialize;
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;
use tokio::sync::{Mutex, watch};

const APPROVED_EXECUTABLE_DIGEST: &str = "PURRCODE_APPROVED_EXECUTABLE_BLAKE3";
const MAX_EXECUTABLE_BYTES: u64 = 256 * 1024 * 1024;
pub(crate) const MAX_PULL_OUTPUT_BYTES: usize = 1024 * 1024;
pub(crate) const PULL_TIMEOUT_SECONDS: u64 = 3600;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PullPhase {
    Queued,
    Pulling,
    Verifying,
    Cancelling,
    Cancelled,
    Failed,
    Completed,
}

impl PullPhase {
    pub(crate) fn terminal(self) -> bool {
        matches!(self, Self::Cancelled | Self::Failed | Self::Completed)
    }
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct PullProgress {
    pub action_id: ActionId,
    pub model: String,
    pub phase: PullPhase,
    pub message: String,
    pub captured_output_bytes: usize,
    pub truncated: bool,
    pub exit_code: Option<i32>,
}

impl PullProgress {
    pub(crate) fn queued(action_id: ActionId, model: String) -> Self {
        Self {
            action_id,
            model,
            phase: PullPhase::Queued,
            message: "Approved Ollama pull is queued".into(),
            captured_output_bytes: 0,
            truncated: false,
            exit_code: None,
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct PullOutcome {
    pub exit_code: Option<i32>,
    pub cancelled: bool,
    pub truncated: bool,
}

pub(crate) fn validate_model_name(model: &str) -> Result<(), String> {
    if model.is_empty()
        || model.len() > 256
        || !model
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "._:/-".contains(character))
    {
        return Err("model name contains unsupported characters".into());
    }
    Ok(())
}

pub(crate) fn resolve_ollama_program() -> Result<(PathBuf, String), String> {
    let search_path = std::env::var_os("PATH")
        .ok_or_else(|| "PATH is unavailable; cannot locate Ollama".to_owned())?;
    resolve_program_in_path("ollama", search_path)
}

fn resolve_program_in_path(
    program: &str,
    search_path: OsString,
) -> Result<(PathBuf, String), String> {
    for directory in std::env::split_paths(&search_path) {
        let candidate = directory.join(program);
        if !candidate.is_file() {
            continue;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let executable = candidate
                .metadata()
                .map_err(|error| format!("could not inspect Ollama executable: {error}"))?
                .permissions()
                .mode()
                & 0o111
                != 0;
            if !executable {
                continue;
            }
        }
        let canonical = candidate
            .canonicalize()
            .map_err(|error| format!("could not canonicalize Ollama executable: {error}"))?;
        let digest = executable_digest(&canonical)?;
        return Ok((canonical, digest));
    }
    Err("Ollama executable was not found on PATH".into())
}

fn executable_digest(path: &Path) -> Result<String, String> {
    let metadata = path
        .metadata()
        .map_err(|error| format!("could not inspect approved executable: {error}"))?;
    if !metadata.is_file() || metadata.len() > MAX_EXECUTABLE_BYTES {
        return Err(format!(
            "approved executable must be a regular file no larger than {MAX_EXECUTABLE_BYTES} bytes"
        ));
    }
    let mut file = std::fs::File::open(path)
        .map_err(|error| format!("could not open approved executable: {error}"))?;
    let mut hasher = blake3::Hasher::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| format!("could not hash approved executable: {error}"))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hasher.finalize().to_hex().to_string())
}

pub(crate) fn proposed_pull(
    session_id: SessionId,
    model: &str,
    program: PathBuf,
    program_digest: String,
    working_directory: PathBuf,
) -> Result<(ActionId, ProposedAction, ActionConstraints, Authorization), String> {
    validate_model_name(model)?;
    if !program.is_absolute() || !working_directory.is_absolute() {
        return Err("pull executable and working directory must be absolute".into());
    }
    let mut environment = BTreeMap::new();
    environment.insert(APPROVED_EXECUTABLE_DIGEST.into(), program_digest);
    let action = ProposedAction::Command(CommandAction {
        program,
        arguments: vec!["pull".into(), model.into()],
        working_directory: working_directory.clone(),
        environment,
    });
    let constraints = ActionConstraints {
        working_directory,
        network: true,
        timeout_seconds: PULL_TIMEOUT_SECONDS,
        maximum_output_bytes: MAX_PULL_OUTPUT_BYTES,
        allowed_write_globs: Vec::new(),
        maximum_changed_files: 0,
    };
    let action_id = ActionId::new();
    let action_digest = action
        .digest(&constraints)
        .map_err(|error| error.to_string())?;
    let authorization = Authorization {
        action_id,
        session_id,
        action_digest,
        constraints: constraints.clone(),
        authorized_at: Utc::now(),
        approved_by: ApprovalAuthority::Human,
    };
    Ok((action_id, action, constraints, authorization))
}

pub(crate) struct PullAdapter;

impl PullAdapter {
    pub(crate) async fn execute(
        store: Arc<Mutex<SessionStore>>,
        session_id: SessionId,
        action_id: ActionId,
        action: ProposedAction,
        constraints: ActionConstraints,
        cancellation: watch::Receiver<bool>,
        progress: watch::Sender<PullProgress>,
    ) -> Result<PullOutcome, String> {
        let (command, model, approved_digest) = validate_exact_action(&action, &constraints)?;
        let action_digest = action
            .digest(&constraints)
            .map_err(|error| error.to_string())?;
        {
            let mut store = store.lock().await;
            let authorization = store
                .consume_authorization(action_id, &action_digest)
                .map_err(|error| format!("exact pull authorization is unavailable: {error}"))?;
            if authorization.session_id != session_id
                || authorization.constraints != constraints
                || authorization.approved_by != ApprovalAuthority::Human
            {
                return Err("pull authorization did not match the exact execution request".into());
            }
        }

        let observed_digest = executable_digest(&command.program)?;
        if observed_digest != approved_digest {
            return Err("Ollama executable changed after approval; pull was not started".into());
        }
        {
            let mut store = store.lock().await;
            store
                .append(session_id, &SessionEvent::ExecutionStarted { action_id })
                .map_err(|error| error.to_string())?;
        }
        let _ = progress.send(PullProgress {
            action_id,
            model: model.clone(),
            phase: PullPhase::Queued,
            message: "Authorization consumed; starting Ollama pull".into(),
            captured_output_bytes: 0,
            truncated: false,
            exit_code: None,
        });

        let mut process = Command::new(&command.program);
        process
            .args(&command.arguments)
            .current_dir(&command.working_directory)
            .env_clear()
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        #[cfg(unix)]
        process.process_group(0);
        let mut child = match process.spawn() {
            Ok(child) => child,
            Err(error) => {
                record_terminal(
                    &store,
                    session_id,
                    action_id,
                    None,
                    false,
                    ValidationStatus::Failed,
                    "Ollama pull process could not be started",
                )
                .await?;
                let _ = progress.send(PullProgress {
                    action_id,
                    model,
                    phase: PullPhase::Failed,
                    message: "Ollama pull process could not be started".into(),
                    captured_output_bytes: 0,
                    truncated: false,
                    exit_code: None,
                });
                return Err(format!(
                    "could not spawn approved Ollama executable: {error}"
                ));
            }
        };
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "approved Ollama process did not expose stdout".to_owned())?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| "approved Ollama process did not expose stderr".to_owned())?;
        let captured = Arc::new(AtomicUsize::new(0));
        let truncated = Arc::new(AtomicBool::new(false));
        let stdout_task = tokio::spawn(read_progress(
            stdout,
            ProgressContext {
                action_id,
                model: model.clone(),
                source: "stdout",
                maximum_bytes: constraints.maximum_output_bytes,
                captured: captured.clone(),
                truncated: truncated.clone(),
                progress: progress.clone(),
            },
        ));
        let stderr_task = tokio::spawn(read_progress(
            stderr,
            ProgressContext {
                action_id,
                model: model.clone(),
                source: "stderr",
                maximum_bytes: constraints.maximum_output_bytes,
                captured: captured.clone(),
                truncated: truncated.clone(),
                progress: progress.clone(),
            },
        ));

        let deadline =
            tokio::time::Instant::now() + Duration::from_secs(constraints.timeout_seconds);
        let (exit_code, cancelled, terminal_message) = loop {
            if *cancellation.borrow() {
                let _ = progress.send(PullProgress {
                    action_id,
                    model: model.clone(),
                    phase: PullPhase::Cancelling,
                    message: "Cancelling Ollama pull".into(),
                    captured_output_bytes: captured.load(Ordering::Relaxed),
                    truncated: truncated.load(Ordering::Relaxed),
                    exit_code: None,
                });
                child
                    .kill()
                    .await
                    .map_err(|error| format!("could not terminate Ollama pull: {error}"))?;
                break (None, true, "Ollama pull cancelled");
            }
            if let Some(status) = child
                .try_wait()
                .map_err(|error| format!("could not inspect Ollama pull: {error}"))?
            {
                let code = status.code();
                break (
                    code,
                    false,
                    if status.success() {
                        "Ollama pull completed"
                    } else {
                        "Ollama pull failed"
                    },
                );
            }
            if tokio::time::Instant::now() >= deadline {
                child.kill().await.map_err(|error| {
                    format!("could not terminate timed-out Ollama pull: {error}")
                })?;
                break (None, false, "Ollama pull exceeded its approved timeout");
            }
            let _ = cancellation.has_changed();
            tokio::time::sleep(Duration::from_millis(100)).await;
        };
        stdout_task
            .await
            .map_err(|error| format!("Ollama stdout reader failed: {error}"))??;
        stderr_task
            .await
            .map_err(|error| format!("Ollama stderr reader failed: {error}"))??;

        let succeeded = exit_code == Some(0) && !cancelled;
        let was_truncated = truncated.load(Ordering::Relaxed);
        record_terminal(
            &store,
            session_id,
            action_id,
            exit_code,
            was_truncated,
            if succeeded {
                ValidationStatus::Passed
            } else {
                ValidationStatus::Failed
            },
            terminal_message,
        )
        .await?;
        let phase = if cancelled {
            PullPhase::Cancelled
        } else if succeeded {
            PullPhase::Verifying
        } else {
            PullPhase::Failed
        };
        let _ = progress.send(PullProgress {
            action_id,
            model,
            phase,
            message: if succeeded {
                "Ollama pull process completed; verifying installed model metadata".into()
            } else {
                terminal_message.into()
            },
            captured_output_bytes: captured
                .load(Ordering::Relaxed)
                .min(constraints.maximum_output_bytes),
            truncated: was_truncated,
            exit_code,
        });
        Ok(PullOutcome {
            exit_code,
            cancelled,
            truncated: was_truncated,
        })
    }
}

fn validate_exact_action<'a>(
    action: &'a ProposedAction,
    constraints: &ActionConstraints,
) -> Result<(&'a CommandAction, String, String), String> {
    let ProposedAction::Command(command) = action else {
        return Err("approved pull action is not a native command".into());
    };
    if command.program.is_relative()
        || command.working_directory != constraints.working_directory
        || command.arguments.len() != 2
        || command.arguments.first().map(String::as_str) != Some("pull")
        || !constraints.network
        || constraints.timeout_seconds == 0
        || constraints.timeout_seconds > PULL_TIMEOUT_SECONDS
        || constraints.maximum_output_bytes == 0
        || constraints.maximum_output_bytes > MAX_PULL_OUTPUT_BYTES
        || !constraints.allowed_write_globs.is_empty()
        || constraints.maximum_changed_files != 0
    {
        return Err("approved pull action or constraints are not supported".into());
    }
    let model = command
        .arguments
        .get(1)
        .cloned()
        .ok_or_else(|| "approved pull omitted the model".to_owned())?;
    validate_model_name(&model)?;
    if command.environment.len() != 1 {
        return Err("approved pull action contained unexpected environment entries".into());
    }
    let digest = command
        .environment
        .get(APPROVED_EXECUTABLE_DIGEST)
        .filter(|digest| digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .cloned()
        .ok_or_else(|| "approved pull omitted a valid executable digest".to_owned())?;
    Ok((command, model, digest))
}

pub(crate) fn validate_pull_action(
    action: &ProposedAction,
    constraints: &ActionConstraints,
) -> Result<String, String> {
    validate_exact_action(action, constraints).map(|(_, model, _)| model)
}

struct ProgressContext {
    action_id: ActionId,
    model: String,
    source: &'static str,
    maximum_bytes: usize,
    captured: Arc<AtomicUsize>,
    truncated: Arc<AtomicBool>,
    progress: watch::Sender<PullProgress>,
}

async fn read_progress<R: AsyncRead + Unpin>(
    mut reader: R,
    context: ProgressContext,
) -> Result<(), String> {
    let ProgressContext {
        action_id,
        model,
        source,
        maximum_bytes,
        captured,
        truncated,
        progress,
    } = context;
    let mut buffer = [0_u8; 8192];
    loop {
        let read = reader
            .read(&mut buffer)
            .await
            .map_err(|error| format!("Ollama {source} read failed: {error}"))?;
        if read == 0 {
            return Ok(());
        }
        let previous = captured.fetch_add(read, Ordering::Relaxed);
        if previous.saturating_add(read) > maximum_bytes {
            truncated.store(true, Ordering::Relaxed);
        }
        let allowed = maximum_bytes.saturating_sub(previous).min(read);
        if allowed == 0 {
            continue;
        }
        let message = sanitize_progress(&String::from_utf8_lossy(&buffer[..allowed]));
        if message.is_empty() {
            continue;
        }
        let _ = progress.send(PullProgress {
            action_id,
            model: model.clone(),
            phase: PullPhase::Pulling,
            message: format!("{source}: {message}"),
            captured_output_bytes: previous.saturating_add(allowed).min(maximum_bytes),
            truncated: truncated.load(Ordering::Relaxed),
            exit_code: None,
        });
    }
}

fn sanitize_progress(value: &str) -> String {
    let mut output = String::with_capacity(value.len().min(2048));
    let mut escape = false;
    for character in value.chars() {
        if escape {
            if character.is_ascii_alphabetic() {
                escape = false;
            }
            continue;
        }
        if character == '\u{1b}' {
            escape = true;
            continue;
        }
        if (character.is_control() && !matches!(character, '\n' | '\r' | '\t'))
            || output.len() >= 2048
        {
            continue;
        }
        output.push(character);
    }
    output.trim().to_owned()
}

async fn record_terminal(
    store: &Arc<Mutex<SessionStore>>,
    session_id: SessionId,
    action_id: ActionId,
    exit_code: Option<i32>,
    truncated: bool,
    validation: ValidationStatus,
    evidence: &str,
) -> Result<(), String> {
    let mut store = store.lock().await;
    store
        .append(
            session_id,
            &SessionEvent::ExecutionFinished {
                action_id,
                exit_code,
                truncated,
                sandbox_level: Some("dedicated_network_native_action".into()),
                sandbox_backend: Some("exact-argv-credential-scrubbed".into()),
            },
        )
        .map_err(|error| error.to_string())?;
    store
        .append(
            session_id,
            &SessionEvent::ValidationRecorded {
                action_id,
                status: validation,
                evidence: evidence.into(),
            },
        )
        .map_err(|error| error.to_string())?;
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use purrcode_runtime_core::SessionEvent;
    use tempfile::tempdir;

    #[cfg(unix)]
    fn executable(directory: &Path, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = directory.join("ollama-fixture");
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        let mut permissions = path.metadata().unwrap().permissions();
        permissions.set_mode(0o700);
        std::fs::set_permissions(&path, permissions).unwrap();
        path.canonicalize().unwrap()
    }

    fn authorized(
        store: &mut SessionStore,
        repository: &Path,
        program: PathBuf,
        model: &str,
    ) -> (SessionId, ActionId, ProposedAction, ActionConstraints) {
        let session_id = SessionId::new();
        store
            .append(
                session_id,
                &SessionEvent::SessionCreated {
                    objective: "pull fixture".into(),
                    repository: repository.into(),
                    authority_mode: Default::default(),
                },
            )
            .unwrap();
        let digest = executable_digest(&program).unwrap();
        let (action_id, action, constraints, authorization) =
            proposed_pull(session_id, model, program, digest, repository.into()).unwrap();
        store
            .append(
                session_id,
                &SessionEvent::ActionProposed {
                    action_id,
                    action: action.clone(),
                },
            )
            .unwrap();
        store
            .append(
                session_id,
                &SessionEvent::JudgmentRecorded {
                    action_id,
                    decision: purrcode_runtime_core::JudgmentDecision::RequireApproval {
                        reason: "test".into(),
                        constraints: constraints.clone(),
                    },
                },
            )
            .unwrap();
        store.authorize(&authorization).unwrap();
        (session_id, action_id, action, constraints)
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn exact_authorization_is_consumed_once_and_progress_is_bounded() {
        let temporary = tempdir().unwrap();
        let repository = temporary.path().canonicalize().unwrap();
        let program = executable(&repository, "printf '\\033[31mpulling\\033[0m\\n'; exit 0");
        let mut store = SessionStore::in_memory().unwrap();
        let (session_id, action_id, action, constraints) =
            authorized(&mut store, &repository, program, "small:latest");
        let store = Arc::new(Mutex::new(store));
        let (_cancel_tx, cancel_rx) = watch::channel(false);
        let (progress_tx, progress_rx) =
            watch::channel(PullProgress::queued(action_id, "small:latest".into()));
        let outcome = PullAdapter::execute(
            store.clone(),
            session_id,
            action_id,
            action.clone(),
            constraints.clone(),
            cancel_rx,
            progress_tx,
        )
        .await
        .unwrap();
        assert_eq!(outcome.exit_code, Some(0));
        assert!(!outcome.cancelled);
        let final_progress = progress_rx.borrow().clone();
        assert_eq!(final_progress.phase, PullPhase::Verifying);
        assert!(final_progress.message.contains("verifying"));
        assert!(!final_progress.message.contains('\u{1b}'));

        let (_cancel_tx, cancel_rx) = watch::channel(false);
        let (progress_tx, _progress_rx) =
            watch::channel(PullProgress::queued(action_id, "small:latest".into()));
        let repeated = PullAdapter::execute(
            store,
            session_id,
            action_id,
            action,
            constraints,
            cancel_rx,
            progress_tx,
        )
        .await;
        assert!(repeated.is_err());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn mutation_after_approval_fails_before_process_start() {
        let temporary = tempdir().unwrap();
        let repository = temporary.path().canonicalize().unwrap();
        let program = executable(&repository, "exit 0");
        let marker = repository.join("should-not-exist");
        let mut store = SessionStore::in_memory().unwrap();
        let (session_id, action_id, action, constraints) =
            authorized(&mut store, &repository, program.clone(), "small:latest");
        std::fs::write(
            &program,
            format!("#!/bin/sh\ntouch '{}'\n", marker.display()),
        )
        .unwrap();
        let store = Arc::new(Mutex::new(store));
        let (_cancel_tx, cancel_rx) = watch::channel(false);
        let (progress_tx, _progress_rx) =
            watch::channel(PullProgress::queued(action_id, "small:latest".into()));
        let result = PullAdapter::execute(
            store,
            session_id,
            action_id,
            action,
            constraints,
            cancel_rx,
            progress_tx,
        )
        .await;
        assert!(result.is_err());
        assert!(!marker.exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn cancellation_terminates_the_pull_and_records_failure() {
        let temporary = tempdir().unwrap();
        let repository = temporary.path().canonicalize().unwrap();
        let program = executable(
            &repository,
            "while true; do printf 'pulling\\n'; sleep 1; done",
        );
        let mut store = SessionStore::in_memory().unwrap();
        let (session_id, action_id, action, constraints) =
            authorized(&mut store, &repository, program, "small:latest");
        let store = Arc::new(Mutex::new(store));
        let (cancel_tx, cancel_rx) = watch::channel(false);
        let (progress_tx, progress_rx) =
            watch::channel(PullProgress::queued(action_id, "small:latest".into()));
        let task = tokio::spawn(PullAdapter::execute(
            store,
            session_id,
            action_id,
            action,
            constraints,
            cancel_rx,
            progress_tx,
        ));
        tokio::time::sleep(Duration::from_millis(150)).await;
        cancel_tx.send(true).unwrap();
        let outcome = task.await.unwrap().unwrap();
        assert!(outcome.cancelled);
        assert_eq!(progress_rx.borrow().phase, PullPhase::Cancelled);
    }
}
