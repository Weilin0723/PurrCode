//! Human authority grants.
//!
//! The principle these types encode: **PawGate advises and records; it never
//! overrides a decision inside a valid human grant.** A human who wants the
//! agent to have everything gets everything the process identity can do — and
//! what no grant can ever remove is typing, attribution, recording, and secret
//! redaction.
//!
//! Equally load-bearing is the inverse: **the model can never mint or widen a
//! grant.** A grant is constructed only from an authenticated human identity,
//! carries an expiry, is revocable, and digests so its exact scope is durable
//! evidence. The session's permission mode is applied to PawGate decisions in
//! the agent loop (`apply_permission_mode`, agent-runtime), keeping the bypass
//! logic next to the decision it changes.

use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::DomainError;

#[derive(
    Clone, Copy, Debug, Eq, Hash, JsonSchema, Ord, PartialEq, PartialOrd, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct GrantId(pub Uuid);

impl GrantId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for GrantId {
    fn default() -> Self {
        Self::new()
    }
}

/// The authenticated human a grant is attributed to.
///
/// This is deliberately not a free-form string on the grant mode: every grant
/// carries who issued it and through which authenticated channel.
#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct HumanIdentity {
    /// Stable subject identifier (e.g. an Entra object id, or `local`).
    pub subject: String,
    /// The channel that authenticated the human.
    pub channel: AuthenticationChannel,
}

#[derive(Clone, Copy, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthenticationChannel {
    /// Interactive confirmation in the local TUI.
    LocalTui,
    /// Entra ID authenticated API call.
    EntraId,
    /// Service-to-service token (automation).
    ServiceToken,
}

/// Named capabilities an elevated grant may enumerate.
#[derive(Clone, Copy, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GrantCapability {
    ExecuteBuildTools,
    ExecuteTests,
    InstallProjectDependencies,
    ManageBackgroundServices,
    NetworkAccess,
}

#[derive(Clone, Debug, Default, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum AuthorityMode {
    /// Normal policy plus exact-action approval. The default, always.
    #[default]
    Governed,
    /// The listed capabilities and programs skip per-action approval;
    /// everything else falls back to Governed.
    Elevated {
        capabilities: Vec<GrantCapability>,
        /// Program basenames, e.g. `cargo`, `npm`.
        allowed_programs: Vec<String>,
    },
    /// No policy veto and no repeated approvals for anything the process
    /// identity can do.
    Unrestricted,
}

#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct HumanAuthorityGrant {
    pub grant_id: GrantId,
    pub mode: AuthorityMode,
    pub granted_by: HumanIdentity,
    pub granted_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    /// Set exactly once, by the human channel. A revoked grant never applies.
    pub revoked: bool,
}

impl HumanAuthorityGrant {
    /// Construct a grant. There is intentionally no way to build one from a
    /// model response: callers must supply an authenticated [`HumanIdentity`],
    /// and the daemon only invokes this from its human-authenticated routes.
    pub fn issue(
        mode: AuthorityMode,
        granted_by: HumanIdentity,
        now: DateTime<Utc>,
        valid_for: chrono::Duration,
    ) -> Self {
        Self {
            grant_id: GrantId::new(),
            mode,
            granted_by,
            granted_at: now,
            expires_at: now + valid_for,
            revoked: false,
        }
    }

    /// Digest over the grant's exact scope, recorded as evidence at issuance
    /// and at every use.
    pub fn digest(&self) -> Result<String, DomainError> {
        let canonical = serde_json::to_vec(self)?;
        Ok(blake3::hash(&canonical).to_hex().to_string())
    }

    pub fn is_active(&self, now: DateTime<Utc>) -> bool {
        !self.revoked && now < self.expires_at
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn human() -> HumanIdentity {
        HumanIdentity {
            subject: "user@example.test".into(),
            channel: AuthenticationChannel::LocalTui,
        }
    }

    fn elevated(programs: &[&str]) -> HumanAuthorityGrant {
        HumanAuthorityGrant::issue(
            AuthorityMode::Elevated {
                capabilities: vec![
                    GrantCapability::ExecuteBuildTools,
                    GrantCapability::ExecuteTests,
                ],
                allowed_programs: programs.iter().map(|p| (*p).to_owned()).collect(),
            },
            human(),
            Utc::now(),
            chrono::Duration::hours(8),
        )
    }

    #[test]
    fn a_grant_digest_binds_its_exact_scope() {
        let narrow = elevated(&["cargo"]);
        let wide = HumanAuthorityGrant {
            mode: AuthorityMode::Elevated {
                capabilities: vec![
                    GrantCapability::ExecuteBuildTools,
                    GrantCapability::ExecuteTests,
                ],
                allowed_programs: vec!["cargo".into(), "curl".into()],
            },
            ..narrow.clone()
        };
        assert_ne!(
            narrow.digest().unwrap(),
            wide.digest().unwrap(),
            "widening the scope must change the digest — a widened grant is a different grant"
        );
    }

    #[test]
    fn issuance_requires_a_human_identity_and_carries_expiry() {
        let grant = HumanAuthorityGrant::issue(
            AuthorityMode::Governed,
            human(),
            Utc::now(),
            chrono::Duration::hours(1),
        );
        assert_eq!(grant.granted_by.subject, "user@example.test");
        assert!(grant.expires_at > grant.granted_at);
        assert!(grant.is_active(Utc::now()));
        assert!(!grant.is_active(grant.expires_at + chrono::Duration::seconds(1)));
    }
}
