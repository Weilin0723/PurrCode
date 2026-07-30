//! Terminal contracts for PurrCode v0.8.
//!
//! These are the typed terminal *actions* and *ownership* contracts introduced
//! by PRD §12. Every terminal interaction is a typed [`TerminalAction`] rather
//! than a shell string (AGENTS.md: "Avoid shell strings. Spawn a program with
//! an explicit argument vector."), ownership is tracked with a monotonically
//! increasing generation so stale agent input after a human takeover is
//! rejected (PRD §12.1), and the data carries no I/O yet.
//!
//! The PTY backend itself (Linux PTY / macOS PTY / Windows ConPTY) lands in PR4.
//! This crate only fixes the contract every caller — the REST surface, the
//! WebSocket stream, the Studio UI, and the agent loop — agrees on, so the
//! backend can be developed and tested against it in isolation.

use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::Duration;
use thiserror::Error;
use uuid::Uuid;

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
    StartTerminal(StartTerminalAction),
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
    #[error("input for terminal {id} is stale: generation {claimed} < current {current}")]
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
        assert!(validate_working_directory(PathBuf::from("/abs/wk").as_path()).is_ok());
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

    #[test]
    fn secluded_empty_program_error() {
        let e = TerminalError::EmptyProgram;
        assert_eq!(e.to_string(), "program path is empty");
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
        };
        assert!(matches!(snap.owner, TerminalOwner::Agent { .. }));
        assert_eq!(snap.generation, OwnershipGeneration(2));
    }
}
