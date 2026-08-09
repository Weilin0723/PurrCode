//! The traceability record (v1.3 §4.4). One per authorized invocation, for
//! EVERY provider.
//!
//! Answers the five questions the product owner named:
//! - *can it be called* — the descriptor (via `tool_id` + `descriptor_digest`)
//! - *why it was allowed* — the [`JudgmentDecision`] + [`ApprovalAuthority`]
//! - *what exact scope was authorized* — the [`ActionConstraints`] plus the
//!   effective network/filesystem scopes
//! - *what happened* — the [`ExecutionOutcome`]
//! - *what evidence came back* — `structured_output`, validated against the
//!   descriptor/skill `output_schema`

use crate::{ActionId, ApprovalAuthority, HookTrigger, SessionId, TurnId};
use crate::{FilesystemScope, NetworkScope, ToolId, ToolProvider};
use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// One authorized tool invocation, as durable evidence.
#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct ExecutionEvidence {
    pub action_id: ActionId,
    pub session_id: SessionId,
    pub turn_id: Option<TurnId>,
    pub tool_id: ToolId,
    pub provider: ToolProvider,
    pub descriptor_digest: String,
    /// WHY it was allowed.
    pub decision: super::JudgmentDecision,
    pub approved_by: ApprovalAuthority,
    /// WHAT SCOPE was authorized.
    pub constraints: super::ActionConstraints,
    pub effective_network_scope: NetworkScope,
    pub effective_filesystem_scope: FilesystemScope,
    /// WHO asked. `Hook { hook_id }` is what makes a hook auditable.
    pub initiator: EvidenceInitiator,
    /// WHAT HAPPENED.
    pub outcome: ExecutionOutcome,
    /// WHAT CAME BACK, validated against descriptor/skill `output_schema`.
    pub structured_output: Option<serde_json::Value>,
    /// Drives evidence-bundle redaction. Replaces the event-type-keyed table
    /// at evidence-bundle/src/lib.rs:123 for tool actions.
    pub redaction_class: RedactionClass,
    pub started_at: DateTime<Utc>,
    pub finished_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EvidenceInitiator {
    Model {
        turn_id: TurnId,
    },
    Human,
    Hook {
        hook_id: String,
        trigger: HookTrigger,
    },
    Command {
        name: String,
    },
    Skill {
        skill_id: String,
    },
}

#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ExecutionOutcome {
    Succeeded {
        exit_code: Option<i32>,
        truncated: bool,
        affected_paths: Vec<PathBuf>,
    },
    Failed {
        reason: String,
    },
    DeniedByPolicy {
        reason: String,
    },
    Cancelled,
    TimedOut,
}

#[derive(Clone, Copy, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RedactionClass {
    /// Safe to export verbatim.
    Public,
    /// Arguments and output redacted unless the bundle is marked sensitive.
    /// DEFAULT for every non-Builtin descriptor.
    Arguments,
    /// Everything but ids redacted.
    Full,
}

impl RedactionClass {
    pub fn for_origin(origin: super::DescriptorOrigin) -> Self {
        match origin {
            super::DescriptorOrigin::Builtin => RedactionClass::Public,
            super::DescriptorOrigin::User
            | super::DescriptorOrigin::Project
            | super::DescriptorOrigin::RemoteDiscovery => RedactionClass::Arguments,
        }
    }
}
