//! Terminal session contracts and typed terminal actions.
//!
//! These types are the shared vocabulary between the daemon, the terminal
//! runtime, the TUI and the remote API. They live in `runtime-core` for the
//! same reason `ProposedAction` does: one definition, digested and recorded the
//! same way everywhere, so no surface can invent a second meaning for "who owns
//! this terminal".
//!
//! Two properties here are load-bearing for safety:
//!
//! * **Ownership generations.** Every ownership transition increments a
//!   monotonic generation, and terminal input binds to the generation the
//!   sender observed. After a human takes control the agent's generation is
//!   stale, so its queued input is refused rather than raced.
//! * **Typed, digestable actions.** A terminal action digests together with its
//!   constraints exactly like a `ProposedAction`, so the existing exact-action
//!   approval machinery applies to terminals without modification.

use std::collections::BTreeMap;
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{ActionConstraints, DomainError};

#[derive(
    Clone, Copy, Debug, Eq, Hash, JsonSchema, Ord, PartialEq, PartialOrd, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct TerminalId(pub Uuid);

impl TerminalId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for TerminalId {
    fn default() -> Self {
        Self::new()
    }
}

/// Monotonic ownership generation.
///
/// Input carries the generation the sender observed; the runtime rejects input
/// whose generation is not current. This is what makes human takeover safe: the
/// transition itself invalidates every in-flight agent write.
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
    pub fn next(self) -> Self {
        Self(self.0.saturating_add(1))
    }
}

#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(tag = "owner", rename_all = "snake_case")]
pub enum TerminalOwner {
    Human,
    Agent { role: String },
    Shared,
}

impl TerminalOwner {
    pub fn describe(&self) -> String {
        match self {
            Self::Human => "human".to_owned(),
            Self::Agent { role } => format!("agent:{role}"),
            Self::Shared => "shared".to_owned(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalStatus {
    Starting,
    Running,
    WaitingForInput,
    Completed,
    Failed,
    Cancelled,
    Disconnected,
    RecoveryRequired,
}

impl TerminalStatus {
    /// True when the process may still be producing output.
    pub const fn is_live(self) -> bool {
        matches!(
            self,
            Self::Starting | Self::Running | Self::WaitingForInput | Self::Disconnected
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct TerminalDimensions {
    pub columns: u16,
    pub rows: u16,
}

impl Default for TerminalDimensions {
    fn default() -> Self {
        Self {
            columns: 120,
            rows: 30,
        }
    }
}

/// How transcripts are retained. Redaction is not optional: transcripts become
/// artifacts, and artifacts never carry raw secrets.
#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct TranscriptPolicy {
    /// Maximum bytes retained in the replayable transcript ring.
    pub maximum_bytes: usize,
    /// Whether the transcript is persisted beyond the session.
    pub persist: bool,
}

impl Default for TranscriptPolicy {
    fn default() -> Self {
        Self {
            maximum_bytes: 512 * 1024,
            persist: false,
        }
    }
}

/// One recorded ownership transition — durable evidence, never inferred.
#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct OwnershipTransition {
    pub generation: OwnershipGeneration,
    pub from: TerminalOwner,
    pub to: TerminalOwner,
    pub at: DateTime<Utc>,
}

/// Durable description of a terminal session.
#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct TerminalSessionRecord {
    pub terminal_id: TerminalId,
    pub owner: TerminalOwner,
    pub generation: OwnershipGeneration,
    pub status: TerminalStatus,
    pub dimensions: TerminalDimensions,
    pub program: PathBuf,
    pub arguments: Vec<String>,
    pub working_directory: PathBuf,
    pub created_at: DateTime<Utc>,
    pub last_activity_at: DateTime<Utc>,
    pub transcript_policy: TranscriptPolicy,
    pub transitions: Vec<OwnershipTransition>,
}

// ── Typed terminal actions ───────────────────────────────────────

/// Input a terminal accepts. Encoded rather than raw bytes so evidence can
/// describe an interrupt as an interrupt instead of an unprintable byte.
#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(tag = "input", rename_all = "snake_case")]
pub enum TerminalInput {
    Text {
        text: String,
    },
    /// Ctrl+C.
    Interrupt,
    /// End of input (Ctrl+D).
    EndOfInput,
}

impl TerminalInput {
    pub fn bytes(&self) -> Vec<u8> {
        match self {
            Self::Text { text } => text.clone().into_bytes(),
            Self::Interrupt => vec![0x03],
            Self::EndOfInput => vec![0x04],
        }
    }
}

#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct StartTerminalAction {
    pub program: PathBuf,
    pub arguments: Vec<String>,
    pub working_directory: PathBuf,
    #[serde(default)]
    pub environment: BTreeMap<String, String>,
    pub dimensions: TerminalDimensions,
    /// Terminate the process if it produces nothing for this long.
    pub idle_timeout_seconds: Option<u64>,
    /// Hard lifetime cap, after which the tree is terminated.
    pub maximum_lifetime_seconds: Option<u64>,
}

#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct SendTerminalInputAction {
    pub terminal_id: TerminalId,
    /// The generation the sender observed. Stale generations are refused.
    pub expected_generation: OwnershipGeneration,
    pub input: TerminalInput,
}

#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct ResizeTerminalAction {
    pub terminal_id: TerminalId,
    pub dimensions: TerminalDimensions,
}

#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct StopProcessAction {
    pub terminal_id: TerminalId,
    /// Seconds of grace between polite and forceful termination.
    pub grace_seconds: u64,
}

/// Typed terminal actions, digested with constraints exactly like
/// [`crate::ProposedAction`] so the approval machinery applies unchanged.
#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TerminalAction {
    StartTerminal(StartTerminalAction),
    SendInput(SendTerminalInputAction),
    Resize(ResizeTerminalAction),
    StopProcess(StopProcessAction),
    Attach { terminal_id: TerminalId },
    Detach { terminal_id: TerminalId },
}

impl TerminalAction {
    /// Digest over the exact action and its constraints, byte-compatible with
    /// [`crate::ProposedAction::digest`]'s scheme.
    pub fn digest(&self, constraints: &ActionConstraints) -> Result<String, DomainError> {
        let canonical = serde_json::to_vec(&(self, constraints))?;
        Ok(blake3::hash(&canonical).to_hex().to_string())
    }

    /// True when the action can cause new process activity, as opposed to
    /// observing or ending it. Read-only surfaces use this the same way they
    /// use `starts_execution` on UI actions.
    pub const fn starts_execution(&self) -> bool {
        matches!(self, Self::StartTerminal(_) | Self::SendInput(_))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn constraints() -> ActionConstraints {
        ActionConstraints::read_only(PathBuf::from("/repo"))
    }

    fn start_action() -> TerminalAction {
        TerminalAction::StartTerminal(StartTerminalAction {
            program: PathBuf::from("/bin/sh"),
            arguments: vec!["-c".into(), "cargo test".into()],
            working_directory: PathBuf::from("/repo"),
            environment: BTreeMap::new(),
            dimensions: TerminalDimensions::default(),
            idle_timeout_seconds: Some(600),
            maximum_lifetime_seconds: Some(3_600),
        })
    }

    #[test]
    fn generations_are_monotonic_and_saturating() {
        let first = OwnershipGeneration::default();
        assert_eq!(first.next(), OwnershipGeneration(1));
        assert_eq!(
            OwnershipGeneration(u64::MAX).next(),
            OwnershipGeneration(u64::MAX)
        );
    }

    #[test]
    fn a_terminal_action_digest_binds_the_action_and_the_constraints() {
        let action = start_action();
        let digest = action.digest(&constraints()).unwrap();
        assert_eq!(digest.len(), 64);
        // Same action, same constraints → same digest.
        assert_eq!(digest, action.digest(&constraints()).unwrap());
        // Different arguments → different digest.
        let other = TerminalAction::StartTerminal(StartTerminalAction {
            arguments: vec!["-c".into(), "cargo build".into()],
            ..match start_action() {
                TerminalAction::StartTerminal(inner) => inner,
                _ => unreachable!(),
            }
        });
        assert_ne!(digest, other.digest(&constraints()).unwrap());
        // Different constraints → different digest.
        let widened = ActionConstraints {
            network: true,
            ..constraints()
        };
        assert_ne!(digest, action.digest(&widened).unwrap());
    }

    #[test]
    fn stale_input_is_representable_and_detectable() {
        let current = OwnershipGeneration(3);
        let action = SendTerminalInputAction {
            terminal_id: TerminalId::new(),
            expected_generation: OwnershipGeneration(2),
            input: TerminalInput::Text {
                text: "npm test\n".into(),
            },
        };
        assert_ne!(action.expected_generation, current);
    }

    #[test]
    fn input_encodings_match_terminal_conventions() {
        assert_eq!(
            TerminalInput::Text {
                text: "ls\n".into()
            }
            .bytes(),
            b"ls\n"
        );
        assert_eq!(TerminalInput::Interrupt.bytes(), vec![0x03]);
        assert_eq!(TerminalInput::EndOfInput.bytes(), vec![0x04]);
    }

    #[test]
    fn only_start_and_input_actions_start_execution() {
        let id = TerminalId::new();
        assert!(start_action().starts_execution());
        assert!(TerminalAction::SendInput(SendTerminalInputAction {
            terminal_id: id,
            expected_generation: OwnershipGeneration(0),
            input: TerminalInput::Interrupt,
        })
        .starts_execution());
        for observer in [
            TerminalAction::Attach { terminal_id: id },
            TerminalAction::Detach { terminal_id: id },
            TerminalAction::Resize(ResizeTerminalAction {
                terminal_id: id,
                dimensions: TerminalDimensions::default(),
            }),
            TerminalAction::StopProcess(StopProcessAction {
                terminal_id: id,
                grace_seconds: 5,
            }),
        ] {
            assert!(
                !observer.starts_execution(),
                "{observer:?} observes or ends activity; it must stay usable read-only"
            );
        }
    }

    #[test]
    fn live_statuses_are_distinguished_from_terminal_ones() {
        for live in [
            TerminalStatus::Starting,
            TerminalStatus::Running,
            TerminalStatus::WaitingForInput,
            TerminalStatus::Disconnected,
        ] {
            assert!(live.is_live());
        }
        for done in [
            TerminalStatus::Completed,
            TerminalStatus::Failed,
            TerminalStatus::Cancelled,
            TerminalStatus::RecoveryRequired,
        ] {
            assert!(!done.is_live());
        }
    }

    #[test]
    fn contracts_round_trip_through_serde() {
        let record = TerminalSessionRecord {
            terminal_id: TerminalId::new(),
            owner: TerminalOwner::Agent {
                role: "build".into(),
            },
            generation: OwnershipGeneration(2),
            status: TerminalStatus::Running,
            dimensions: TerminalDimensions::default(),
            program: PathBuf::from("/bin/sh"),
            arguments: vec!["-c".into(), "./mvnw test".into()],
            working_directory: PathBuf::from("/workspace"),
            created_at: Utc::now(),
            last_activity_at: Utc::now(),
            transcript_policy: TranscriptPolicy::default(),
            transitions: vec![OwnershipTransition {
                generation: OwnershipGeneration(1),
                from: TerminalOwner::Agent {
                    role: "build".into(),
                },
                to: TerminalOwner::Human,
                at: Utc::now(),
            }],
        };
        let json = serde_json::to_string(&record).unwrap();
        let back: TerminalSessionRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(back, record);
    }
}
