//! Terminal contracts for PurrCode v0.8.
//!
//! These are the typed terminal *actions* and *ownership* contracts introduced
//! by PRD §12. Every terminal interaction is a typed [`TerminalAction`] rather
//! than a shell string (AGENTS.md: "Avoid shell strings. Spawn a program with
//! an explicit argument vector."), ownership is tracked with a monotonically
//! increasing generation so stale agent input after a human takeover is
//! rejected (PRD §12.1). The runtime executes actions through a native PTY
//! (Unix PTY or Windows ConPTY), keeps bounded transcripts, and excludes
//! provider credentials from child environments.

use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, VecDeque};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use thiserror::Error;
use uuid::Uuid;

pub mod emulator;

pub use emulator::{
    CellAttrs, CursorShape, CursorState, GridPoint, KeyInput, KeyModifiers, Selection,
    TerminalCell, TerminalColor, TerminalContextSummary, TerminalEmulator, TerminalKey,
};
pub use purrcode_workspace_contracts::{TerminalId, WorkspaceId};

/// Monotonically increasing ownership generation.
///
/// Each [`TerminalOwner`] transition (Agent → Human → Agent, PRD §12.1)
/// increments this counter. An input action carries the generation of the owner
/// that issued it; the runtime rejects any input whose generation is older than
/// the current generation, so a delayed agent input cannot race a human takeover
/// and land in a terminal the human now controls.
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    Eq,
    Hash,
    JsonSchema,
    Ord,
    PartialEq,
    PartialOrd,
    Serialize,
    Deserialize,
)]
#[serde(transparent)]
pub struct OwnershipGeneration(pub u64);

impl OwnershipGeneration {
    pub const INITIAL: OwnershipGeneration = OwnershipGeneration(0);

    pub fn next(self) -> Self {
        OwnershipGeneration(
            self.0
                .checked_add(1)
                .expect("ownership generation overflow"),
        )
    }

    pub fn is_stale(self, current: OwnershipGeneration) -> bool {
        self < current
    }
}

impl std::fmt::Display for OwnershipGeneration {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Who controls a terminal right now (PRD §12.1).
///
/// `Agent` records which agent role owns it so the activity inspector can show
/// "Current agent". `Shared` is reserved for cooperative input and is not used
/// by the default human-takeover flow.
#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "data", rename_all = "snake_case")]
pub enum TerminalOwner {
    Human,
    Agent {
        /// PurrCode agent role label ("Supervisor", "Build Agent", …).
        role: AgentRoleLabel,
    },
    Shared,
}

/// A short, human-readable agent role label. Kept as a string so new roles do
/// not require a contract change; the supervisor assigns these (PRD §10.5).
#[derive(
    Clone, Debug, Eq, Hash, JsonSchema, Ord, PartialEq, PartialOrd, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct AgentRoleLabel(pub String);

impl AgentRoleLabel {
    pub fn new(role: impl Into<String>) -> Self {
        Self(role.into())
    }
}

impl std::fmt::Display for AgentRoleLabel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

// ---------------------------------------------------------------------------
// Typed terminal actions (PRD §12).
// ---------------------------------------------------------------------------

/// Start a one-shot command in a fresh PTY and capture its outcome.
#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct ExecuteCommandAction {
    /// Program to run. Never a shell string; an explicit argv element.
    pub program: PathBuf,
    /// Explicit argument vector.
    #[serde(default)]
    pub arguments: Vec<String>,
    /// Absolute working directory inside the workspace worktree.
    pub working_directory: PathBuf,
    #[serde(default)]
    pub environment: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    pub timeout: Option<DurationSerde>,
}

/// Open a long-lived interactive terminal that can be attached/detached and
/// taken over (PRD §10.4, §12.1). Backed by a login shell by default but may
/// host a long-running process such as a dev server (PRD §12.2).
#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct StartTerminalAction {
    /// Optional program; defaults to the user's preferred/login shell.
    #[serde(default)]
    pub program: Option<PathBuf>,
    #[serde(default)]
    pub arguments: Vec<String>,
    pub working_directory: PathBuf,
    #[serde(default)]
    pub environment: std::collections::BTreeMap<String, String>,
    /// Initial rows/cols; zero means let the backend pick a default.
    #[serde(default)]
    pub initial_size: TerminalSize,
    /// Optional owning agent role (takeover starts with Human otherwise).
    #[serde(default)]
    pub owner: Option<TerminalOwner>,
    /// If set, the terminal is registered as a managed background process
    /// (PRD §12.2) with readiness/health/lifecycle metadata.
    #[serde(default)]
    pub background: Option<ManagedProcessSpec>,
}

/// Send raw bytes (keystrokes, pasted text) to a live terminal.
///
/// `owner_generation` must equal the terminal's current generation or the input
/// is rejected as stale (PRD §12.1).
#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct SendTerminalInputAction {
    pub terminal_id: TerminalId,
    pub owner_generation: OwnershipGeneration,
    /// Raw bytes to write to the PTY master side.
    pub input: Vec<u8>,
}

/// Resize a live terminal's PTY window.
#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct ResizeTerminalAction {
    pub terminal_id: TerminalId,
    pub size: TerminalSize,
}

/// Inspect the foreground process tree of a terminal (PGID, exit status if
/// dead, child PIDs). Used by the UI "stop process" affordance and recovery.
#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct InspectProcessAction {
    pub terminal_id: TerminalId,
}

/// Block until a terminal's process exits or the deadline passes.
#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct WaitForProcessAction {
    pub terminal_id: TerminalId,
    #[serde(default)]
    pub timeout: Option<DurationSerde>,
}

/// Stop (SIGTERM then SIGKILL after grace) the process group of a terminal
/// (PRD §10.4: "terminate a process"). Does not delete the terminal record.
#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct StopProcessAction {
    pub terminal_id: TerminalId,
    /// Grace period before escalating to SIGKILL.
    #[serde(default)]
    pub grace: Option<DurationSerde>,
}

/// A client (re)attaches to a terminal's stream (PRD §11.3 reconnect).
#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct AttachTerminalAction {
    pub terminal_id: TerminalId,
    /// Maximum bytes of historical transcript to replay on attach (bounded).
    #[serde(default)]
    pub replay_bytes: usize,
}

/// Detach a client without terminating the process (PRD §11.3: "close the UI
/// while the run continues").
#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct DetachTerminalAction {
    pub terminal_id: TerminalId,
}

/// The complete typed terminal action set (PRD §12).
#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(tag = "action", content = "data", rename_all = "snake_case")]
pub enum TerminalAction {
    ExecuteCommand(ExecuteCommandAction),
    StartTerminal(Box<StartTerminalAction>),
    SendInput(SendTerminalInputAction),
    ResizeTerminal(ResizeTerminalAction),
    InspectProcess(InspectProcessAction),
    WaitForProcess(WaitForProcessAction),
    StopProcess(StopProcessAction),
    AttachTerminal(AttachTerminalAction),
    DetachTerminal(DetachTerminalAction),
}

// ---------------------------------------------------------------------------
// Terminal geometry and background process specs.
// ---------------------------------------------------------------------------

/// PTY window size in rows/columns.
#[derive(Clone, Copy, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct TerminalSize {
    pub rows: u16,
    pub cols: u16,
}

impl Default for TerminalSize {
    fn default() -> Self {
        // A safe portable default; the UI sends the real size on attach.
        Self { rows: 24, cols: 80 }
    }
}

/// Managed background-process lifecycle (PRD §12.2).
///
/// Long-running processes (dev server, database, message broker, mock server)
/// declare how they're probed and shut down so the terminal runtime can surface
/// readiness and restart them without human hand-holding.
#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct ManagedProcessSpec {
    /// Human-readable label shown in the Activity panel ("Build", "Tests").
    pub label: String,
    /// Probe that reports readiness (e.g. "HTTP 200 on :8080/health").
    #[serde(default)]
    pub readiness: Option<ReadinessProbe>,
    /// Probe that reports liveness once ready.
    #[serde(default)]
    pub health: Option<HealthProbe>,
    /// How to stop the process group.
    #[serde(default)]
    pub shutdown: Option<ShutdownMethod>,
    #[serde(default)]
    pub restart_policy: RestartPolicy,
    #[serde(default)]
    pub log_policy: LogPolicy,
    /// Optional resource limits (bytes / wall time). Bounded by the daemon.
    #[serde(default)]
    pub resource_limits: Option<ResourceLimits>,
}

#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "data", rename_all = "snake_case")]
pub enum ReadinessProbe {
    TcpConnect {
        host: String,
        port: u16,
    },
    HttpGet {
        url: String,
        expected_status: Option<u16>,
    },
    CommandExitZero {
        program: PathBuf,
        #[serde(default)]
        arguments: Vec<String>,
    },
}

#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "data", rename_all = "snake_case")]
pub enum HealthProbe {
    HttpGet {
        url: String,
        expected_status: Option<u16>,
    },
    CommandExitZero {
        program: PathBuf,
        #[serde(default)]
        arguments: Vec<String>,
    },
}

#[derive(Clone, Copy, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShutdownMethod {
    /// Send SIGTERM to the process group and wait for exit.
    GracefulTermination,
    /// Send SIGTERM, then SIGKILL after the grace window.
    GracefulThenKill,
    /// Invoke a stop command (e.g. a process manager) before escalation.
    Command,
}

#[derive(
    Clone, Copy, Debug, Default, Eq, JsonSchema, Ord, PartialEq, PartialOrd, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum RestartPolicy {
    #[default]
    Never,
    OnFailure,
    Always,
}

#[derive(Clone, Copy, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LogPolicy {
    RingBuffer { max_bytes: usize },
    FileOnly,
    InheritStdio,
}

impl Default for LogPolicy {
    fn default() -> Self {
        LogPolicy::RingBuffer {
            max_bytes: 256 * 1024,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct ResourceLimits {
    pub max_memory_bytes: Option<u64>,
    pub max_wall_time: Option<DurationSerde>,
}

// ---------------------------------------------------------------------------
// Outcomes.
// ---------------------------------------------------------------------------

/// Result of executing a one-shot command (PRD §12 supports bounded output and
/// timeouts; outcomes reuse the runtime's evidence-first discipline).
#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct CommandOutcome {
    pub exit_code: Option<i32>,
    /// Stdout/stderr, capped to a configured bound.
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    /// True when the process was killed by the timeout.
    pub timed_out: bool,
    #[serde(default)]
    pub duration: Option<DurationSerde>,
}

/// Snapshot of a live terminal returned by `GET /v1/terminals/{id}`.
#[derive(Clone, Debug, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct TerminalSnapshot {
    pub terminal_id: TerminalId,
    pub workspace_id: WorkspaceId,
    pub owner: TerminalOwner,
    pub generation: OwnershipGeneration,
    pub alive: bool,
    /// Frontend process group id (i32 for portability with ConPTY).
    pub process_group: Option<i32>,
    /// Bounded transcript tail for reconnect replay.
    pub transcript_tail: Vec<u8>,
    #[serde(default)]
    pub last_seen_at: Option<DateTime<Utc>>,
    /// The directory the process was started in.
    ///
    /// PurrCode runs sessions in isolated worktrees, so a client that cannot
    /// name this cannot tell the user whether `git status` describes their
    /// checkout or the agent's (PRD §14).
    #[serde(default)]
    pub working_directory: Option<PathBuf>,
    /// The program behind the terminal, so a client can say "zsh" rather than
    /// "a shell" and offer a useful action when it is missing.
    #[serde(default)]
    pub shell: Option<PathBuf>,
    /// Set once the process is gone. `Some(0)` is a clean exit, which is a
    /// different thing to report than a crash, and both differ from "still
    /// running" (PRD §4).
    #[serde(default)]
    pub exit_code: Option<i32>,
}

// ---------------------------------------------------------------------------
// Id helper + errors.
// ---------------------------------------------------------------------------

/// Generate a new [`TerminalId`]. Convenience so callers need not import uuid.
pub fn new_terminal_id() -> TerminalId {
    TerminalId(Uuid::new_v4())
}

#[derive(Clone, Debug, Error, JsonSchema, PartialEq, Serialize, Deserialize)]
pub enum TerminalError {
    #[error("terminal {id} not found")]
    NotFound { id: TerminalId },
    #[error("terminal {id} is already owned by {owner:?} (generation {generation})")]
    AlreadyOwned {
        id: TerminalId,
        owner: TerminalOwner,
        generation: OwnershipGeneration,
    },
    #[error("input for terminal {id} has generation {claimed}; current generation is {current}")]
    StaleInput {
        id: TerminalId,
        claimed: OwnershipGeneration,
        current: OwnershipGeneration,
    },
    #[error("terminal {id} process exited")]
    Exited { id: TerminalId },
    #[error("command timed out")]
    Timeout,
    #[error("working directory is not absolute: {0}")]
    RelativeWorkingDirectory(String),
    #[error("program path is empty")]
    EmptyProgram,
    #[error("terminal backend failed: {0}")]
    Backend(String),
    #[error("terminal environment key is not allowed: {0}")]
    UnsafeEnvironment(String),
    #[error("terminal runtime lock is poisoned")]
    LockPoisoned,
}

/// Reject absolute/non-absolute working-directory mistakes before any PTY is
/// opened. Worktree working directories are always absolute under managed
/// storage; a relative path is a bug.
pub fn validate_working_directory(path: &std::path::Path) -> Result<(), TerminalError> {
    if path.as_os_str().is_empty() {
        return Err(TerminalError::RelativeWorkingDirectory(String::new()));
    }
    if !path.is_absolute() {
        return Err(TerminalError::RelativeWorkingDirectory(
            path.display().to_string(),
        ));
    }
    Ok(())
}

const DEFAULT_TRANSCRIPT_BYTES: usize = 256 * 1024;
const MAX_TRANSCRIPT_BYTES: usize = 4 * 1024 * 1024;
const MAX_REPLAY_BYTES: usize = 1024 * 1024;
/// How long a one-shot command waits for the PTY reader to finish after the
/// child exits. Bounded so a reader that never sees end-of-file cannot hang the
/// caller; exceeded only if the backend does not close the pipe.
const DRAIN_TIMEOUT: Duration = Duration::from_secs(2);

struct Transcript {
    bytes: VecDeque<u8>,
    maximum: usize,
    /// Total bytes ever written, including bytes the ring buffer discarded.
    /// Monotonic, so it is a stable cursor for incremental readers.
    produced: u64,
    last_seen_at: DateTime<Utc>,
}

impl Transcript {
    fn append(&mut self, chunk: &[u8]) {
        let skip = chunk.len().saturating_sub(self.maximum);
        self.bytes.extend(chunk[skip..].iter().copied());
        self.produced = self.produced.saturating_add(chunk.len() as u64);
        while self.bytes.len() > self.maximum {
            self.bytes.pop_front();
        }
        self.last_seen_at = Utc::now();
    }

    fn tail(&self, maximum: usize) -> Vec<u8> {
        let take = maximum.min(MAX_REPLAY_BYTES).min(self.bytes.len());
        self.bytes
            .iter()
            .skip(self.bytes.len() - take)
            .copied()
            .collect()
    }

    /// Bytes produced after `since`, plus the offset the caller should ask for
    /// next. `truncated` is true when the ring buffer already discarded part of
    /// the requested range, so a client can tell "here is what came next" from
    /// "output was lost, resynchronise".
    fn since(&self, since: u64) -> TerminalChunk {
        let retained_from = self.produced.saturating_sub(self.bytes.len() as u64);
        let start = since.max(retained_from);
        let skip = (start - retained_from) as usize;
        TerminalChunk {
            bytes: self.bytes.iter().skip(skip).copied().collect(),
            next_offset: self.produced,
            truncated: since < retained_from,
        }
    }
}

/// An incremental slice of terminal output (PRD §24.7). Clients append these
/// rather than re-reading the whole transcript on a timer.
#[derive(Clone, Debug, Default, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct TerminalChunk {
    pub bytes: Vec<u8>,
    /// Offset to pass as `since` on the next read.
    pub next_offset: u64,
    /// True when output between the requested offset and `bytes` was discarded.
    pub truncated: bool,
}

struct TerminalSession {
    workspace_id: WorkspaceId,
    working_directory: PathBuf,
    program: PathBuf,
    owner: TerminalOwner,
    generation: OwnershipGeneration,
    master: Box<dyn portable_pty::MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    child: Box<dyn portable_pty::Child + Send + Sync>,
    transcript: Arc<Mutex<Transcript>>,
    /// Set once the reader thread has read the PTY to its end.
    drained: Arc<AtomicBool>,
    exit_code: Option<i32>,
    attached_clients: usize,
}

/// Native cross-platform PTY manager shared by local, remote, and headless
/// surfaces. Agent callers must pass through PawGate before this boundary.
#[derive(Clone, Default)]
pub struct TerminalRuntime {
    sessions: Arc<Mutex<BTreeMap<TerminalId, TerminalSession>>>,
}

impl TerminalRuntime {
    /// Execute a bounded one-shot command in a PTY. A PTY exposes one merged
    /// output stream, returned in `stdout`; `stderr` is therefore empty.
    pub fn execute(
        &self,
        workspace_id: WorkspaceId,
        action: ExecuteCommandAction,
    ) -> Result<CommandOutcome, TerminalError> {
        if action.program.as_os_str().is_empty() {
            return Err(TerminalError::EmptyProgram);
        }
        let started_at = Instant::now();
        let terminal = self.start(
            workspace_id,
            StartTerminalAction {
                program: Some(action.program),
                arguments: action.arguments,
                working_directory: action.working_directory,
                environment: action.environment,
                initial_size: TerminalSize::default(),
                owner: Some(TerminalOwner::Agent {
                    role: AgentRoleLabel::new("Command"),
                }),
                background: Some(ManagedProcessSpec {
                    label: "one-shot command".into(),
                    readiness: None,
                    health: None,
                    shutdown: Some(ShutdownMethod::GracefulThenKill),
                    restart_policy: RestartPolicy::Never,
                    log_policy: LogPolicy::RingBuffer {
                        max_bytes: MAX_TRANSCRIPT_BYTES,
                    },
                    resource_limits: None,
                }),
            },
        )?;
        let wait = self.wait(WaitForProcessAction {
            terminal_id: terminal.terminal_id,
            timeout: action.timeout,
        });
        let timed_out = matches!(wait, Err(TerminalError::Timeout));
        if timed_out {
            self.stop(StopProcessAction {
                terminal_id: terminal.terminal_id,
                grace: Some(Duration::from_millis(250).into()),
            })?;
        } else {
            wait?;
        }
        let mut sessions = self
            .sessions
            .lock()
            .map_err(|_| TerminalError::LockPoisoned)?;
        let session = sessions
            .remove(&terminal.terminal_id)
            .ok_or(TerminalError::NotFound {
                id: terminal.terminal_id,
            })?;
        // The child has exited, but bytes it already wrote may still be in the
        // pipe. Give the reader a bounded moment to finish so the captured
        // output is the whole output — this is the evidence the action is
        // judged on. Dropping the master first makes the pending read return.
        drop(session.master);
        let deadline = Instant::now() + DRAIN_TIMEOUT;
        while !session.drained.load(Ordering::Acquire) && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(2));
        }
        let output = session
            .transcript
            .lock()
            .map_err(|_| TerminalError::LockPoisoned)?
            .tail(MAX_TRANSCRIPT_BYTES);
        Ok(CommandOutcome {
            exit_code: session.exit_code,
            stdout: output,
            stderr: Vec::new(),
            timed_out,
            duration: Some(started_at.elapsed().into()),
        })
    }

    pub fn start(
        &self,
        workspace_id: WorkspaceId,
        action: StartTerminalAction,
    ) -> Result<TerminalSnapshot, TerminalError> {
        validate_working_directory(&action.working_directory)?;
        let size = normalized_size(action.initial_size);
        let pair = portable_pty::native_pty_system()
            .openpty(to_pty_size(size))
            .map_err(backend)?;
        let (program, arguments) = match action.program.as_ref() {
            Some(program) if program.as_os_str().is_empty() => {
                return Err(TerminalError::EmptyProgram);
            }
            Some(program) => (program.clone(), action.arguments.clone()),
            // No program means "the user's shell", which must be started
            // interactively — see `interactive_shell_arguments`.
            None if action.arguments.is_empty() => {
                let shell = default_shell();
                let arguments = interactive_shell_arguments(&shell);
                (shell, arguments)
            }
            None => (default_shell(), action.arguments.clone()),
        };
        let mut command = portable_pty::CommandBuilder::new(&program);
        command.args(&arguments);
        command.cwd(&action.working_directory);
        apply_safe_environment(&mut command, &action.environment)?;
        let reader = pair.master.try_clone_reader().map_err(backend)?;
        let writer = pair.master.take_writer().map_err(backend)?;
        let child = pair.slave.spawn_command(command).map_err(backend)?;
        let process_group = process_group(&*pair.master, &*child);
        drop(pair.slave);

        let maximum = action
            .background
            .as_ref()
            .and_then(|background| match background.log_policy {
                LogPolicy::RingBuffer { max_bytes } => Some(max_bytes),
                _ => None,
            })
            .unwrap_or(DEFAULT_TRANSCRIPT_BYTES)
            .clamp(1, MAX_TRANSCRIPT_BYTES);
        let transcript = Arc::new(Mutex::new(Transcript {
            bytes: VecDeque::new(),
            maximum,
            produced: 0,
            last_seen_at: Utc::now(),
        }));
        let drained = Arc::new(AtomicBool::new(false));
        spawn_reader(reader, Arc::clone(&transcript), Arc::clone(&drained));
        let terminal_id = new_terminal_id();
        let owner = action.owner.unwrap_or(TerminalOwner::Human);
        let snapshot = TerminalSnapshot {
            terminal_id,
            workspace_id,
            owner: owner.clone(),
            generation: OwnershipGeneration::INITIAL,
            alive: true,
            process_group,
            transcript_tail: Vec::new(),
            last_seen_at: Some(Utc::now()),
            working_directory: Some(action.working_directory.clone()),
            shell: Some(program.clone()),
            exit_code: None,
        };
        self.sessions
            .lock()
            .map_err(|_| TerminalError::LockPoisoned)?
            .insert(
                terminal_id,
                TerminalSession {
                    workspace_id,
                    working_directory: action.working_directory.clone(),
                    program,
                    owner,
                    generation: OwnershipGeneration::INITIAL,
                    master: pair.master,
                    writer,
                    child,
                    transcript,
                    drained,
                    exit_code: None,
                    attached_clients: 0,
                },
            );
        Ok(snapshot)
    }

    pub fn send_input(&self, action: SendTerminalInputAction) -> Result<(), TerminalError> {
        let mut sessions = self
            .sessions
            .lock()
            .map_err(|_| TerminalError::LockPoisoned)?;
        let session = sessions
            .get_mut(&action.terminal_id)
            .ok_or(TerminalError::NotFound {
                id: action.terminal_id,
            })?;
        refresh_exit(session)?;
        if session.exit_code.is_some() {
            return Err(TerminalError::Exited {
                id: action.terminal_id,
            });
        }
        if action.owner_generation != session.generation {
            return Err(TerminalError::StaleInput {
                id: action.terminal_id,
                claimed: action.owner_generation,
                current: session.generation,
            });
        }
        session
            .writer
            .write_all(&action.input)
            .and_then(|_| session.writer.flush())
            .map_err(backend)
    }

    pub fn resize(&self, action: ResizeTerminalAction) -> Result<TerminalSnapshot, TerminalError> {
        let mut sessions = self
            .sessions
            .lock()
            .map_err(|_| TerminalError::LockPoisoned)?;
        let session = get_session(&mut sessions, action.terminal_id)?;
        session
            .master
            .resize(to_pty_size(normalized_size(action.size)))
            .map_err(backend)?;
        snapshot(action.terminal_id, session, 0)
    }

    pub fn attach(&self, action: AttachTerminalAction) -> Result<TerminalSnapshot, TerminalError> {
        let mut sessions = self
            .sessions
            .lock()
            .map_err(|_| TerminalError::LockPoisoned)?;
        let session = get_session(&mut sessions, action.terminal_id)?;
        session.attached_clients = session.attached_clients.saturating_add(1);
        snapshot(action.terminal_id, session, action.replay_bytes)
    }

    pub fn detach(&self, action: DetachTerminalAction) -> Result<TerminalSnapshot, TerminalError> {
        let mut sessions = self
            .sessions
            .lock()
            .map_err(|_| TerminalError::LockPoisoned)?;
        let session = get_session(&mut sessions, action.terminal_id)?;
        session.attached_clients = session.attached_clients.saturating_sub(1);
        snapshot(action.terminal_id, session, 0)
    }

    pub fn inspect(
        &self,
        terminal_id: TerminalId,
        replay_bytes: usize,
    ) -> Result<TerminalSnapshot, TerminalError> {
        let mut sessions = self
            .sessions
            .lock()
            .map_err(|_| TerminalError::LockPoisoned)?;
        let session = get_session(&mut sessions, terminal_id)?;
        snapshot(terminal_id, session, replay_bytes)
    }

    /// Output produced after `since`. This is the read path a live client uses:
    /// it transfers only new bytes, so a busy build does not re-send its whole
    /// transcript on every tick.
    pub fn read_since(
        &self,
        terminal_id: TerminalId,
        since: u64,
    ) -> Result<TerminalChunk, TerminalError> {
        let mut sessions = self
            .sessions
            .lock()
            .map_err(|_| TerminalError::LockPoisoned)?;
        let session = get_session(&mut sessions, terminal_id)?;
        let transcript = session
            .transcript
            .lock()
            .map_err(|_| TerminalError::LockPoisoned)?;
        Ok(transcript.since(since))
    }

    pub fn transfer_ownership(
        &self,
        terminal_id: TerminalId,
        owner: TerminalOwner,
    ) -> Result<TerminalSnapshot, TerminalError> {
        let mut sessions = self
            .sessions
            .lock()
            .map_err(|_| TerminalError::LockPoisoned)?;
        let session = get_session(&mut sessions, terminal_id)?;
        if session.owner != owner {
            session.owner = owner;
            session.generation = session.generation.next();
        }
        snapshot(terminal_id, session, 0)
    }

    pub fn wait(&self, action: WaitForProcessAction) -> Result<TerminalSnapshot, TerminalError> {
        let deadline = action.timeout.map(|d| Instant::now() + d.to_duration());
        loop {
            let snapshot = self.inspect(action.terminal_id, 0)?;
            if !snapshot.alive {
                return Ok(snapshot);
            }
            if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
                return Err(TerminalError::Timeout);
            }
            thread::sleep(Duration::from_millis(20));
        }
    }

    pub fn stop(&self, action: StopProcessAction) -> Result<TerminalSnapshot, TerminalError> {
        {
            let mut sessions = self
                .sessions
                .lock()
                .map_err(|_| TerminalError::LockPoisoned)?;
            let session = get_session(&mut sessions, action.terminal_id)?;
            refresh_exit(session)?;
            if session.exit_code.is_none() {
                terminate_process_group(session)?;
            }
        }
        let grace = action
            .grace
            .unwrap_or_else(|| Duration::from_secs(2).into())
            .to_duration();
        let graceful = self.wait(WaitForProcessAction {
            terminal_id: action.terminal_id,
            timeout: Some(grace.into()),
        });
        if !matches!(graceful, Err(TerminalError::Timeout)) {
            return graceful;
        }
        {
            let mut sessions = self
                .sessions
                .lock()
                .map_err(|_| TerminalError::LockPoisoned)?;
            kill_process_group(get_session(&mut sessions, action.terminal_id)?)?;
        }
        self.wait(WaitForProcessAction {
            terminal_id: action.terminal_id,
            timeout: Some(Duration::from_secs(2).into()),
        })
    }

    pub fn list(&self) -> Result<Vec<TerminalSnapshot>, TerminalError> {
        let ids: Vec<_> = self
            .sessions
            .lock()
            .map_err(|_| TerminalError::LockPoisoned)?
            .keys()
            .copied()
            .collect();
        ids.into_iter().map(|id| self.inspect(id, 0)).collect()
    }
}

fn get_session(
    sessions: &mut BTreeMap<TerminalId, TerminalSession>,
    id: TerminalId,
) -> Result<&mut TerminalSession, TerminalError> {
    sessions.get_mut(&id).ok_or(TerminalError::NotFound { id })
}

fn snapshot(
    id: TerminalId,
    session: &mut TerminalSession,
    replay_bytes: usize,
) -> Result<TerminalSnapshot, TerminalError> {
    refresh_exit(session)?;
    let transcript = session
        .transcript
        .lock()
        .map_err(|_| TerminalError::LockPoisoned)?;
    Ok(TerminalSnapshot {
        terminal_id: id,
        workspace_id: session.workspace_id,
        owner: session.owner.clone(),
        generation: session.generation,
        alive: session.exit_code.is_none(),
        process_group: process_group(&*session.master, &*session.child),
        transcript_tail: transcript.tail(replay_bytes),
        last_seen_at: Some(transcript.last_seen_at),
        working_directory: Some(session.working_directory.clone()),
        shell: Some(session.program.clone()),
        exit_code: session.exit_code,
    })
}

fn refresh_exit(session: &mut TerminalSession) -> Result<(), TerminalError> {
    if session.exit_code.is_none() {
        session.exit_code = session
            .child
            .try_wait()
            .map_err(backend)?
            .map(|status| status.exit_code() as i32);
    }
    Ok(())
}

/// Drain the PTY into `transcript` until it closes, then set `drained`.
///
/// The flag matters: a process can exit while bytes it already wrote are still
/// in the pipe. Reading the transcript the instant the child exits therefore
/// returns output that is short by whatever the reader had not yet appended —
/// and that output is the evidence an action is judged on, not just something a
/// test looks at.
fn spawn_reader(
    mut reader: Box<dyn Read + Send>,
    transcript: Arc<Mutex<Transcript>>,
    drained: Arc<AtomicBool>,
) {
    thread::spawn(move || {
        let mut chunk = [0_u8; 8192];
        loop {
            match reader.read(&mut chunk) {
                Ok(0) => break,
                Ok(read) => match transcript.lock() {
                    Ok(mut transcript) => transcript.append(&chunk[..read]),
                    Err(_) => break,
                },
                // A signal or a momentarily empty PTY is not the end of the
                // stream. Treating every error as terminal killed the reader on
                // the first EINTR and silently discarded the rest of the
                // process's output — including, sometimes, all of it.
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::Interrupted | std::io::ErrorKind::WouldBlock
                    ) =>
                {
                    thread::sleep(Duration::from_millis(1));
                }
                // Anything else — the PTY closed, the far end went away — ends
                // the stream for real.
                Err(_) => break,
            }
        }
        drained.store(true, Ordering::Release);
    });
}

fn apply_safe_environment(
    command: &mut portable_pty::CommandBuilder,
    requested: &BTreeMap<String, String>,
) -> Result<(), TerminalError> {
    command.env_clear();
    for key in [
        "PATH",
        "TMPDIR",
        "TEMP",
        "LANG",
        "LC_ALL",
        "TERM",
        "SystemRoot",
        "WINDIR",
    ] {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }
    for (key, value) in requested {
        if credential_like(key) {
            return Err(TerminalError::UnsafeEnvironment(key.clone()));
        }
        command.env(key, value);
    }
    Ok(())
}

fn credential_like(key: &str) -> bool {
    let key = key.to_ascii_uppercase();
    key.split(|character: char| !character.is_ascii_alphanumeric())
        .any(|segment| {
            matches!(
                segment,
                "KEY" | "TOKEN" | "SECRET" | "PASSWORD" | "CREDENTIAL" | "AUTHORIZATION"
            )
        })
}

fn normalized_size(size: TerminalSize) -> TerminalSize {
    TerminalSize {
        rows: if size.rows == 0 { 24 } else { size.rows },
        cols: if size.cols == 0 { 80 } else { size.cols },
    }
}

fn to_pty_size(size: TerminalSize) -> portable_pty::PtySize {
    portable_pty::PtySize {
        rows: size.rows,
        cols: size.cols,
        pixel_width: 0,
        pixel_height: 0,
    }
}

/// The arguments that make a shell an *interactive* shell.
///
/// This is not cosmetic. An interactive shell turns on job control, which puts
/// each command it runs into its own process group. Without it the command and
/// the shell share a group, so the `SIGINT` that a terminal delivers on Ctrl+C
/// reaches both — the user interrupts a build and their shell exits with it.
/// Terminal PRD §47 lists "Ctrl+C kills wrong process" as a release blocker.
///
/// Shells decide interactivity from their arguments as much as from whether
/// stdin is a terminal, and the ones that infer it do not all enable job
/// control when they do, so the flag is passed explicitly.
fn interactive_shell_arguments(shell: &std::path::Path) -> Vec<String> {
    let name = shell
        .file_name()
        .map(|name| name.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    match name.trim_end_matches(".exe") {
        // cmd and PowerShell are interactive when attached to a console and
        // have no equivalent flag; ConPTY handles the signal routing.
        "cmd" | "powershell" | "pwsh" => Vec::new(),
        _ => vec!["-i".to_owned()],
    }
}

fn default_shell() -> PathBuf {
    #[cfg(windows)]
    {
        PathBuf::from("cmd.exe")
    }
    #[cfg(not(windows))]
    {
        PathBuf::from(std::env::var_os("SHELL").unwrap_or_else(|| "/bin/sh".into()))
    }
}

fn process_group(
    _master: &dyn portable_pty::MasterPty,
    child: &dyn portable_pty::Child,
) -> Option<i32> {
    #[cfg(unix)]
    {
        _master
            .process_group_leader()
            .or_else(|| child.process_id().map(|id| id as i32))
    }
    #[cfg(not(unix))]
    {
        child.process_id().map(|id| id as i32)
    }
}

fn backend(error: impl std::fmt::Display) -> TerminalError {
    TerminalError::Backend(error.to_string())
}

fn terminate_process_group(session: &mut TerminalSession) -> Result<(), TerminalError> {
    #[cfg(unix)]
    if let Some(group) = session.master.process_group_leader() {
        // A negative pid addresses the complete foreground process group.
        if unsafe { libc::kill(-group, libc::SIGTERM) } == 0 {
            return Ok(());
        }
    }
    session.child.kill().map_err(backend)
}

fn kill_process_group(session: &mut TerminalSession) -> Result<(), TerminalError> {
    #[cfg(unix)]
    if let Some(group) = session.master.process_group_leader()
        && unsafe { libc::kill(-group, libc::SIGKILL) } == 0
    {
        return Ok(());
    }
    session.child.kill().map_err(backend)
}

// ---------------------------------------------------------------------------
// Serde-friendly Duration shim.
// ---------------------------------------------------------------------------

/// Serde representation of [`Duration`] as whole + fractional seconds, because
/// `std::time::Duration` has no Serialize impl and we keep this crate
/// dependency-free of `tokio`/`humantime`.
#[derive(Clone, Copy, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct DurationSerde {
    pub secs: u64,
    pub nanos: u32,
}

impl DurationSerde {
    pub fn from_duration(d: Duration) -> Self {
        Self {
            secs: d.as_secs(),
            nanos: d.subsec_nanos(),
        }
    }
    pub fn to_duration(self) -> Duration {
        Duration::new(self.secs, self.nanos)
    }
}

impl From<Duration> for DurationSerde {
    fn from(d: Duration) -> Self {
        Self::from_duration(d)
    }
}

impl From<DurationSerde> for Duration {
    fn from(d: DurationSerde) -> Self {
        d.to_duration()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ownership_generation_next_and_stale() {
        let g0 = OwnershipGeneration::INITIAL;
        let g1 = g0.next();
        assert_eq!(g1, OwnershipGeneration(1));
        // Stale means "older than current".
        assert!(g0.is_stale(g1));
        assert!(!g1.is_stale(g1));
    }

    #[test]
    fn terminal_action_round_trips_tagged() {
        let act = TerminalAction::SendInput(SendTerminalInputAction {
            terminal_id: TerminalId::new(),
            owner_generation: OwnershipGeneration(3),
            input: b"cargo test\n".to_vec(),
        });
        let j = serde_json::to_string(&act).unwrap();
        assert!(j.contains("\"action\":\"send_input\""));
        let back: TerminalAction = serde_json::from_str(&j).unwrap();
        assert_eq!(act, back);
    }

    #[test]
    fn start_terminal_defaults_to_safe_size_and_never_restart() {
        let act = StartTerminalAction {
            program: None,
            arguments: vec![],
            working_directory: PathBuf::from("/repo/wk"),
            environment: Default::default(),
            initial_size: TerminalSize::default(),
            owner: None,
            background: None,
        };
        assert_eq!(act.initial_size, (TerminalSize { rows: 24, cols: 80 }));
        assert!(act.owner.is_none());
        assert!(act.background.is_none());
    }

    #[test]
    fn validate_working_directory_rejects_relative() {
        assert!(validate_working_directory(PathBuf::from("rel").as_path()).is_err());
        assert!(validate_working_directory(PathBuf::from("").as_path()).is_err());
        assert!(validate_working_directory(&std::env::current_dir().unwrap()).is_ok());
    }

    #[test]
    fn duration_serde_round_trips() {
        let d = Duration::new(7, 500_000_000);
        let s: DurationSerde = d.into();
        assert_eq!(s.secs, 7);
        assert_eq!(s.nanos, 500_000_000);
        let back: Duration = s.into();
        assert_eq!(back, d);
    }

    fn transcript(maximum: usize) -> Transcript {
        Transcript {
            bytes: VecDeque::new(),
            maximum,
            produced: 0,
            last_seen_at: Utc::now(),
        }
    }

    #[test]
    fn incremental_reads_return_only_new_bytes() {
        let mut t = transcript(1024);
        t.append(b"hello ");
        let first = t.since(0);
        assert_eq!(first.bytes, b"hello ");
        assert_eq!(first.next_offset, 6);
        assert!(!first.truncated);

        t.append(b"world");
        let second = t.since(first.next_offset);
        assert_eq!(second.bytes, b"world");
        assert_eq!(second.next_offset, 11);
        assert!(!second.truncated);

        // Reading at the head yields nothing, not a repeat of the transcript.
        assert!(t.since(second.next_offset).bytes.is_empty());
    }

    #[test]
    fn a_reader_that_fell_behind_the_ring_buffer_is_told_output_was_lost() {
        let mut t = transcript(4);
        t.append(b"abcdefgh");
        // Only the last four bytes survive; offset 0 can no longer be served.
        let chunk = t.since(0);
        assert_eq!(chunk.bytes, b"efgh");
        assert_eq!(chunk.next_offset, 8);
        assert!(
            chunk.truncated,
            "a client must be able to tell lost output from continuous output"
        );
        assert!(!t.since(4).truncated);
    }

    #[test]
    fn produced_counts_bytes_the_ring_buffer_discarded() {
        let mut t = transcript(2);
        t.append(b"abcdef");
        assert_eq!(t.produced, 6);
        assert_eq!(t.bytes.len(), 2);
        assert_eq!(t.since(6).next_offset, 6);
    }

    #[test]
    fn secluded_empty_program_error() {
        let e = TerminalError::EmptyProgram;
        assert_eq!(e.to_string(), "program path is empty");
    }

    #[test]
    fn a_user_shell_is_started_interactively() {
        // Job control is what puts a command in its own process group, which is
        // what makes Ctrl+C interrupt the command rather than the shell.
        assert_eq!(
            interactive_shell_arguments(std::path::Path::new("/bin/zsh")),
            vec!["-i".to_owned()]
        );
        assert_eq!(
            interactive_shell_arguments(std::path::Path::new("/usr/local/bin/fish")),
            vec!["-i".to_owned()]
        );
        assert!(
            interactive_shell_arguments(std::path::Path::new("cmd.exe")).is_empty(),
            "cmd has no such flag and ConPTY routes the signal itself"
        );
        assert!(interactive_shell_arguments(std::path::Path::new("pwsh")).is_empty());
    }

    #[test]
    #[cfg(unix)]
    fn an_explicit_program_is_run_exactly_as_asked() {
        // The interactive default applies to "give me a shell", never to a
        // command the caller named: adding `-i` to `pytest` would be absurd.
        let directory = tempfile::tempdir().expect("temp dir");
        let runtime = TerminalRuntime::default();
        let started = runtime
            .start(
                WorkspaceId::new(),
                StartTerminalAction {
                    program: Some(PathBuf::from("/bin/cat")),
                    arguments: vec!["-u".to_owned()],
                    working_directory: directory.path().to_path_buf(),
                    environment: Default::default(),
                    initial_size: TerminalSize::default(),
                    owner: None,
                    background: None,
                },
            )
            .expect("start");
        assert_eq!(started.shell, Some(PathBuf::from("/bin/cat")));
        let _ = runtime.stop(StopProcessAction {
            terminal_id: started.terminal_id,
            grace: Some(Duration::from_millis(200).into()),
        });
    }

    #[test]
    fn terminal_snapshot_has_workspace_and_owner() {
        let snap = TerminalSnapshot {
            terminal_id: TerminalId::new(),
            workspace_id: WorkspaceId::new(),
            owner: TerminalOwner::Agent {
                role: AgentRoleLabel::new("Build Agent"),
            },
            generation: OwnershipGeneration(2),
            alive: true,
            process_group: Some(4242),
            transcript_tail: vec![],
            last_seen_at: None,
            working_directory: Some(PathBuf::from("/tmp/worktree")),
            shell: Some(PathBuf::from("/bin/zsh")),
            exit_code: None,
        };
        assert!(matches!(snap.owner, TerminalOwner::Agent { .. }));
        assert_eq!(snap.generation, OwnershipGeneration(2));
    }

    #[test]
    #[cfg(unix)]
    fn a_snapshot_names_the_directory_and_shell_it_started_in() {
        // PRD §14: a client that cannot say which tree the terminal sees cannot
        // stop the user from misreading `git status`.
        let directory = tempfile::tempdir().expect("temp dir");
        let runtime = TerminalRuntime::default();
        let started = runtime
            .start(
                WorkspaceId::new(),
                cat_action(directory.path(), TerminalOwner::Human),
            )
            .expect("start");
        assert_eq!(
            started.working_directory.as_deref(),
            Some(directory.path()),
            "the terminal must report where it actually is"
        );
        assert_eq!(started.shell, Some(PathBuf::from("/bin/cat")));
        assert_eq!(started.exit_code, None, "a live process has not exited");

        runtime
            .stop(StopProcessAction {
                terminal_id: started.terminal_id,
                grace: Some(Duration::from_millis(200).into()),
            })
            .expect("stop");
        let after = runtime.inspect(started.terminal_id, 0).expect("inspect");
        assert!(!after.alive);
        assert!(
            after.exit_code.is_some(),
            "a dead process must report how it died, not just that it is gone"
        );
    }

    #[cfg(unix)]
    fn cat_action(directory: &std::path::Path, owner: TerminalOwner) -> StartTerminalAction {
        StartTerminalAction {
            program: Some(PathBuf::from("/bin/cat")),
            arguments: vec![],
            working_directory: directory.to_path_buf(),
            environment: BTreeMap::new(),
            initial_size: TerminalSize {
                rows: 30,
                cols: 100,
            },
            owner: Some(owner),
            background: None,
        }
    }

    #[cfg(unix)]
    fn wait_for_text(runtime: &TerminalRuntime, id: TerminalId, expected: &str) {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let snapshot = runtime.inspect(id, 4096).unwrap();
            if String::from_utf8_lossy(&snapshot.transcript_tail).contains(expected) {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "PTY output did not contain {expected:?}"
            );
            thread::sleep(Duration::from_millis(20));
        }
    }

    #[cfg(unix)]
    #[test]
    fn real_pty_supports_input_replay_resize_detach_and_stop() {
        let directory = tempfile::tempdir().unwrap();
        let runtime = TerminalRuntime::default();
        let workspace_id = WorkspaceId::new();
        let started = runtime
            .start(
                workspace_id,
                cat_action(directory.path(), TerminalOwner::Human),
            )
            .unwrap();
        runtime
            .send_input(SendTerminalInputAction {
                terminal_id: started.terminal_id,
                owner_generation: started.generation,
                input: b"purrcode-pty-roundtrip\n".to_vec(),
            })
            .unwrap();
        wait_for_text(&runtime, started.terminal_id, "purrcode-pty-roundtrip");
        let attached = runtime
            .attach(AttachTerminalAction {
                terminal_id: started.terminal_id,
                replay_bytes: 4096,
            })
            .unwrap();
        assert!(
            String::from_utf8_lossy(&attached.transcript_tail).contains("purrcode-pty-roundtrip")
        );
        runtime
            .resize(ResizeTerminalAction {
                terminal_id: started.terminal_id,
                size: TerminalSize {
                    rows: 42,
                    cols: 132,
                },
            })
            .unwrap();
        let detached = runtime
            .detach(DetachTerminalAction {
                terminal_id: started.terminal_id,
            })
            .unwrap();
        assert!(detached.alive, "detach must not stop the process");
        let stopped = runtime
            .stop(StopProcessAction {
                terminal_id: started.terminal_id,
                grace: Some(Duration::from_secs(2).into()),
            })
            .unwrap();
        assert!(!stopped.alive);
    }

    #[cfg(unix)]
    #[test]
    fn takeover_rejects_delayed_agent_input() {
        let directory = tempfile::tempdir().unwrap();
        let runtime = TerminalRuntime::default();
        let started = runtime
            .start(
                WorkspaceId::new(),
                cat_action(
                    directory.path(),
                    TerminalOwner::Agent {
                        role: AgentRoleLabel::new("Build Agent"),
                    },
                ),
            )
            .unwrap();
        let taken = runtime
            .transfer_ownership(started.terminal_id, TerminalOwner::Human)
            .unwrap();
        assert_eq!(taken.generation, OwnershipGeneration(1));
        assert!(matches!(
            runtime.send_input(SendTerminalInputAction {
                terminal_id: started.terminal_id,
                owner_generation: started.generation,
                input: b"must-not-land\n".to_vec(),
            }),
            Err(TerminalError::StaleInput { .. })
        ));
        runtime
            .stop(StopProcessAction {
                terminal_id: started.terminal_id,
                grace: None,
            })
            .unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn child_environment_is_allowlisted_and_rejects_secret_keys() {
        let directory = tempfile::tempdir().unwrap();
        let runtime = TerminalRuntime::default();
        let mut action = cat_action(directory.path(), TerminalOwner::Human);
        action
            .environment
            .insert("NVIDIA_API_KEY".into(), "secret".into());
        assert_eq!(
            runtime.start(WorkspaceId::new(), action).unwrap_err(),
            TerminalError::UnsafeEnvironment("NVIDIA_API_KEY".into())
        );
        assert!(!credential_like("MONKEY_PATCH_MODE"));
        assert!(credential_like("PROVIDER_API_TOKEN"));
    }

    #[cfg(unix)]
    #[test]
    fn execute_returns_real_pty_output_and_exit_status() {
        let directory = tempfile::tempdir().unwrap();
        let outcome = TerminalRuntime::default()
            .execute(
                WorkspaceId::new(),
                ExecuteCommandAction {
                    program: PathBuf::from("/usr/bin/printf"),
                    arguments: vec!["terminal-command-evidence".into()],
                    working_directory: directory.path().to_path_buf(),
                    environment: BTreeMap::new(),
                    timeout: Some(Duration::from_secs(2).into()),
                },
            )
            .unwrap();
        assert_eq!(outcome.exit_code, Some(0));
        assert!(!outcome.timed_out);
        assert!(String::from_utf8_lossy(&outcome.stdout).contains("terminal-command-evidence"));
        assert!(outcome.stderr.is_empty());
    }

    #[test]
    fn a_transient_read_error_does_not_discard_the_rest_of_the_output() {
        /// A reader that fails the way a real PTY does under load: an EINTR
        /// between two chunks. Before the fix the first error ended the stream
        /// and everything after it was lost, which is how a command could exit
        /// zero with no captured output at all.
        struct Interrupted {
            chunks: Vec<&'static str>,
            index: usize,
            interrupted: bool,
        }
        impl Read for Interrupted {
            fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
                if self.index == 1 && !self.interrupted {
                    self.interrupted = true;
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::Interrupted,
                        "signal",
                    ));
                }
                match self.chunks.get(self.index) {
                    Some(chunk) => {
                        self.index += 1;
                        buffer[..chunk.len()].copy_from_slice(chunk.as_bytes());
                        Ok(chunk.len())
                    }
                    None => Ok(0),
                }
            }
        }

        let transcript = Arc::new(Mutex::new(Transcript {
            bytes: VecDeque::new(),
            maximum: 1024,
            produced: 0,
            last_seen_at: Utc::now(),
        }));
        let drained = Arc::new(AtomicBool::new(false));
        spawn_reader(
            Box::new(Interrupted {
                chunks: vec!["before-", "after"],
                index: 0,
                interrupted: false,
            }),
            Arc::clone(&transcript),
            Arc::clone(&drained),
        );
        let deadline = Instant::now() + Duration::from_secs(2);
        while !drained.load(Ordering::Acquire) && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(2));
        }
        assert!(drained.load(Ordering::Acquire), "reader never finished");
        let captured = String::from_utf8(transcript.lock().unwrap().tail(1024)).unwrap();
        assert_eq!(
            captured, "before-after",
            "output after a transient error was discarded"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_command_that_exits_immediately_after_writing_still_yields_all_of_it() {
        // A process can exit while the bytes it wrote are still in the pipe, so
        // reading the transcript the instant the child dies returns output short
        // by whatever the reader had not yet appended. That output is an
        // action's evidence, so truncating it silently weakens every judgment
        // made from it.
        //
        // This asserts the property — a command's whole output is captured —
        // rather than reproducing the race, which needs a loaded machine and is
        // not reliably triggerable here. It is a volume check that would catch a
        // reader wired up so that it never drains at all; the narrow timing
        // window is closed by construction in `execute`, not by this test.
        let directory = tempfile::tempdir().unwrap();
        let line = "0123456789abcdef".repeat(64);
        let expected = 400;
        let script = format!("for i in $(seq 1 {expected}); do printf '%s\\n' '{line}'; done");
        let outcome = TerminalRuntime::default()
            .execute(
                WorkspaceId::new(),
                ExecuteCommandAction {
                    program: PathBuf::from("/bin/sh"),
                    arguments: vec!["-c".into(), script],
                    working_directory: directory.path().to_path_buf(),
                    environment: BTreeMap::new(),
                    timeout: Some(Duration::from_secs(10).into()),
                },
            )
            .unwrap();
        assert_eq!(outcome.exit_code, Some(0));
        let observed = String::from_utf8_lossy(&outcome.stdout)
            .matches(line.as_str())
            .count();
        assert_eq!(
            observed, expected,
            "captured {observed} of {expected} lines; output was truncated before the reader drained"
        );
    }
}
