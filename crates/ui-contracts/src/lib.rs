//! UI action registry and API-compatibility contract (PRD §4, §10, §24 PR1).
//!
//! The Studio UI is a *client* of authenticated runtime APIs (PRD §3.3): it
//! never implements a separate execution path. Every interaction the UI emits
//! is a typed [`StudioAction`] in this registry, so the daemon can validate,
//! authorize (PawGate), and execute it through the same trusted path the CLI
//! uses. The UI conversely reads typed [`StudioEvent`]s from the durable run.
//!
//! This crate fixes the *shape* of the UI↔daemon boundary in PR1 so PR2 (the
//! Studio shell and web server) and PR3 (the Workbench screens) have a stable
//! target to build against.

use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub use purrcode_authority_contracts::{
    AutonomyGrant, Capability, GrantId, HumanAuthorityMode, PersistScope,
};
pub use purrcode_terminal_runtime::TerminalAction;
pub use purrcode_workspace_contracts::{RunId, WorkspaceId};

/// PurrCode public UI/runtime protocol version.
///
/// PR1 introduces the v0.8 surfaces; the daemon advertises this number on
/// `/health` (mirroring `purrcode_daemon::DAEMON_API_VERSION`) and the Studio
/// shell refuses to drive a daemon whose major version does not match. Bump on
/// any breaking change to the typed [`StudioAction`] / [`StudioEvent`] set.
pub const STUDIO_API_VERSION: u32 = 1;

/// Names of the Studio primary screens (PRD §10.1). The registry references
/// these so a screen can request exactly the events/actions it can render.
#[derive(
    Clone, Copy, Debug, Eq, Hash, JsonSchema, Ord, PartialEq, PartialOrd, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum StudioScreen {
    Home,
    Workspaces,
    AgentRuns,
    Workbench,
    Terminals,
    DiffReview,
    Validation,
    AgentFactory,
    Deployments,
    Evidence,
    Settings,
}

/// A typed action the Studio may request the daemon perform (PRD §4).
///
/// All actions are *requests*: the daemon still routes each privileged one
/// through PawGate. The UI never holds a credential or spawns its own
/// execution path (AGENTS.md: "TUI/UI actions route through daemon API; no UI
/// component creates a second execution path").
#[derive(Clone, Debug, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(tag = "action", content = "data", rename_all = "snake_case")]
pub enum StudioAction {
    /// Create or open a workspace (PRD §18.2).
    CreateWorkspace {
        repository: purrcode_workspace_contracts::RepositoryReference,
        objective: String,
    },
    OpenWorkspace {
        id: WorkspaceId,
    },
    CloseWorkspace {
        id: WorkspaceId,
    },
    /// Submit one goal as a continuous run (PRD §3.2, §18.3).
    StartRun {
        workspace_id: WorkspaceId,
        objective: String,
    },
    CancelRun {
        run_id: RunId,
    },
    ResumeRun {
        run_id: RunId,
    },
    /// Take/return terminal control (PRD §12.1, §18.4).
    TakeTerminalControl {
        terminal_id: purrcode_workspace_contracts::TerminalId,
    },
    ReturnTerminalControl {
        terminal_id: purrcode_workspace_contracts::TerminalId,
    },
    /// Issue a typed terminal action (PRD §12).
    Terminal {
        action: TerminalAction,
    },
    /// Record an autonomy grant once (PRD §15.3, §18.6).
    RecordGrant {
        grant: AutonomyGrant,
        scope: PersistScope,
    },
    RevokeGrant {
        grant_id: GrantId,
    },
}

/// A typed event the Studio renders (PRD §10.2-§10.5). The durable run emits
/// these; the UI maps them onto its screens.
#[derive(Clone, Debug, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", content = "data", rename_all = "snake_case")]
pub enum StudioEvent {
    WorkspaceOpened {
        id: WorkspaceId,
        status: purrcode_workspace_contracts::WorkspaceStatus,
    },
    RunCreated {
        run_id: RunId,
        workspace_id: WorkspaceId,
        objective: String,
    },
    RunProgress {
        run_id: RunId,
        agent: String,
        activity: String,
        state: purrcode_workspace_contracts::RunStatus,
    },
    RunCompleted {
        run_id: RunId,
        status: purrcode_workspace_contracts::RunStatus,
    },
    TerminalOutput {
        terminal_id: purrcode_workspace_contracts::TerminalId,
        owner: purrcode_terminal_runtime::TerminalOwner,
        bytes: Vec<u8>,
    },
    OwnershipChanged {
        terminal_id: purrcode_workspace_contracts::TerminalId,
        owner: purrcode_terminal_runtime::TerminalOwner,
        generation: purrcode_terminal_runtime::OwnershipGeneration,
    },
    ValidationUpdated {
        run_id: RunId,
        stage: String,
        passed: bool,
    },
    GrantRecorded {
        grant_id: GrantId,
    },
    GrantRevoked {
        grant_id: GrantId,
    },
    AuthorityBanner {
        mode: HumanAuthorityMode,
        subject: String,
        capabilities: Vec<Capability>,
    },
}

/// Version advertised alongside the above. Kept separate so a client can read
/// it before declaring which screens it can drive.
#[derive(Clone, Copy, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct StudioProtocolVersion {
    pub studio_api_version: u32,
}

impl Default for StudioProtocolVersion {
    fn default() -> Self {
        Self {
            studio_api_version: STUDIO_API_VERSION,
        }
    }
}

/// Compatibility check: a daemon's advertised major must equal the client's.
pub fn is_compatible(client: u32, daemon: u32) -> bool {
    client == daemon
}

#[derive(Clone, Debug, Error, JsonSchema, PartialEq, Serialize, Deserialize)]
pub enum UiContractError {
    #[error("API version mismatch: client {client}, daemon {daemon}")]
    VersionMismatch { client: u32, daemon: u32 },
    #[error("unknown studio action variant: {0}")]
    UnknownAction(String),
}

/// Timestamp helper for callers that want a stable `DateTime<Utc>` for events
/// but do not want to pull chrono directly.
pub fn now_utc() -> DateTime<Utc> {
    Utc::now()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn studio_action_round_trips_tagged() {
        let a = StudioAction::StartRun {
            workspace_id: WorkspaceId::new(),
            objective: "migrate to java 21".into(),
        };
        let j = serde_json::to_string(&a).unwrap();
        assert!(j.contains("\"action\":\"start_run\""));
        let back: StudioAction = serde_json::from_str(&j).unwrap();
        assert_eq!(a, back);
    }

    #[test]
    fn studio_event_round_trips_terminal_output() {
        let e = StudioEvent::TerminalOutput {
            terminal_id: purrcode_workspace_contracts::TerminalId::new(),
            owner: purrcode_terminal_runtime::TerminalOwner::Human,
            bytes: b"$ cargo build\n".to_vec(),
        };
        let j = serde_json::to_string(&e).unwrap();
        assert!(j.contains("\"event\":\"terminal_output\""));
        let back: StudioEvent = serde_json::from_str(&j).unwrap();
        assert_eq!(e, back);
    }

    #[test]
    fn ownership_changed_event_carries_generation() {
        let e = StudioEvent::OwnershipChanged {
            terminal_id: purrcode_workspace_contracts::TerminalId::new(),
            owner: purrcode_terminal_runtime::TerminalOwner::Agent {
                role: purrcode_terminal_runtime::AgentRoleLabel::new("Build Agent"),
            },
            generation: purrcode_terminal_runtime::OwnershipGeneration(2),
        };
        let j = serde_json::to_string(&e).unwrap();
        assert!(j.contains("\"generation\":2"));
    }

    #[test]
    fn screen_registry_covers_prd_10_1() {
        // PRD §10.1 names eleven screens.
        let all = [
            StudioScreen::Home,
            StudioScreen::Workspaces,
            StudioScreen::AgentRuns,
            StudioScreen::Workbench,
            StudioScreen::Terminals,
            StudioScreen::DiffReview,
            StudioScreen::Validation,
            StudioScreen::AgentFactory,
            StudioScreen::Deployments,
            StudioScreen::Evidence,
            StudioScreen::Settings,
        ];
        assert_eq!(all.len(), 11);
    }

    #[test]
    fn version_compatibility_requires_exact_match() {
        assert!(is_compatible(STUDIO_API_VERSION, STUDIO_API_VERSION));
        assert!(!is_compatible(STUDIO_API_VERSION, STUDIO_API_VERSION + 1));
    }

    #[test]
    fn ui_contract_error_displays_mismatch() {
        let e = UiContractError::VersionMismatch {
            client: 1,
            daemon: 2,
        };
        assert!(e.to_string().contains("client 1"));
        assert!(e.to_string().contains("daemon 2"));
    }

    #[test]
    fn record_grant_action_carries_scope() {
        let a = StudioAction::RecordGrant {
            // Grant internals are exercised in authority-contracts; here we only
            // verify the StudioAction shape retains scope.
            grant: AutonomyGrant {
                grant_id: GrantId::new(),
                human_subject: purrcode_authority_contracts::HumanSubject {
                    subject_id: "s".into(),
                    identity_claims_digest: "d".into(),
                    display_name: None,
                },
                mode: HumanAuthorityMode::Governed,
                agent_scope: None,
                workspace_scope: None,
                azure_scope: None,
                capabilities: Default::default(),
                issued_at: now_utc(),
                expires_at: None,
                identity_claims_digest: "d".into(),
            },
            scope: PersistScope::Workspace,
        };
        let j = serde_json::to_string(&a).unwrap();
        assert!(j.contains("\"scope\":\"workspace\""));
        let back: StudioAction = serde_json::from_str(&j).unwrap();
        assert_eq!(a, back);
    }
}
