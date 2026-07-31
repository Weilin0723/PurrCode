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
//! evidence. `apply_human_authority` is a pure function over (grant, decision),
//! so every bypass is reproducible from the record.

use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{DomainError, JudgmentDecision};

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

    /// Whether this grant covers running `program` for `capability`.
    fn covers(&self, program: Option<&str>, capability: Option<GrantCapability>) -> bool {
        match &self.mode {
            AuthorityMode::Governed => false,
            AuthorityMode::Unrestricted => true,
            AuthorityMode::Elevated {
                capabilities,
                allowed_programs,
            } => {
                let capability_ok = capability.is_none_or(|needed| capabilities.contains(&needed));
                let program_ok = program
                    .is_none_or(|name| allowed_programs.iter().any(|allowed| allowed == name));
                capability_ok && program_ok
            }
        }
    }
}

/// The outcome of consulting a grant about a PawGate decision.
#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct AuthorityOutcome {
    pub decision: JudgmentDecision,
    /// True when the grant changed the decision. Recorded as evidence; a
    /// bypass that is not visible in the record did not happen.
    pub overridden: bool,
    /// The grant consulted, when one applied.
    pub grant_id: Option<GrantId>,
}

/// Apply a human authority grant to a PawGate decision.
///
/// Pure by design: (grant, decision, time) in, outcome out, so every bypass is
/// reproducible from durable evidence. The rules:
///
/// * no grant, inactive grant, or Governed mode → the decision stands;
/// * a covering grant converts `RequireApproval` into allow-with-the-same-
///   constraints (the scope PawGate computed still bounds execution) and
///   converts a policy `Deny` the same way under Unrestricted;
/// * `Deny` under Elevated stands — elevation skips approvals for enumerated
///   tools, it does not convert refusals;
/// * `ModifyAction`/`Replan` always stand: they are advice about correctness,
///   not authority, and overriding them would change *what* runs rather than
///   *whether* the human's chosen action runs.
pub fn apply_human_authority(
    grant: Option<&HumanAuthorityGrant>,
    decision: JudgmentDecision,
    program: Option<&str>,
    capability: Option<GrantCapability>,
    now: DateTime<Utc>,
) -> AuthorityOutcome {
    let Some(grant) = grant else {
        return AuthorityOutcome {
            decision,
            overridden: false,
            grant_id: None,
        };
    };
    if !grant.is_active(now) || !grant.covers(program, capability) {
        return AuthorityOutcome {
            decision,
            overridden: false,
            grant_id: Some(grant.grant_id),
        };
    }
    let unrestricted = matches!(grant.mode, AuthorityMode::Unrestricted);
    let (decision, overridden) = match decision {
        JudgmentDecision::RequireApproval { constraints, .. } => (
            // The human pre-approved this scope; the constraints PawGate
            // computed still bound the execution.
            JudgmentDecision::AllowWithConstraints(constraints),
            true,
        ),
        JudgmentDecision::Deny { reason } if unrestricted => (
            // Unrestricted means the human decided; PawGate's refusal is
            // recorded via `overridden`, not enforced.
            JudgmentDecision::Allow,
            {
                let _ = reason;
                true
            },
        ),
        other => (other, false),
    };
    AuthorityOutcome {
        decision,
        overridden,
        grant_id: Some(grant.grant_id),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ActionConstraints;
    use std::path::PathBuf;

    fn human() -> HumanIdentity {
        HumanIdentity {
            subject: "user@example.test".into(),
            channel: AuthenticationChannel::LocalTui,
        }
    }

    fn constraints() -> ActionConstraints {
        ActionConstraints::read_only(PathBuf::from("/repo"))
    }

    fn require_approval() -> JudgmentDecision {
        JudgmentDecision::RequireApproval {
            reason: "policy requires approval".into(),
            constraints: constraints(),
        }
    }

    fn deny() -> JudgmentDecision {
        JudgmentDecision::Deny {
            reason: "policy refuses this program".into(),
        }
    }

    fn unrestricted() -> HumanAuthorityGrant {
        HumanAuthorityGrant::issue(
            AuthorityMode::Unrestricted,
            human(),
            Utc::now(),
            chrono::Duration::hours(8),
        )
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
    fn without_a_grant_pawgate_decisions_stand() {
        let outcome =
            apply_human_authority(None, require_approval(), Some("cargo"), None, Utc::now());
        assert!(!outcome.overridden);
        assert!(matches!(
            outcome.decision,
            JudgmentDecision::RequireApproval { .. }
        ));
    }

    #[test]
    fn unrestricted_never_vetoes_and_every_bypass_is_recorded() {
        let grant = unrestricted();
        for decision in [require_approval(), deny()] {
            let outcome =
                apply_human_authority(Some(&grant), decision, Some("anything"), None, Utc::now());
            assert!(
                outcome.overridden,
                "the bypass must be visible in the record"
            );
            assert_eq!(outcome.grant_id, Some(grant.grant_id));
            assert!(matches!(
                outcome.decision,
                JudgmentDecision::Allow | JudgmentDecision::AllowWithConstraints(_)
            ));
        }
    }

    #[test]
    fn a_bypassed_approval_keeps_the_computed_constraints() {
        let grant = unrestricted();
        let outcome = apply_human_authority(
            Some(&grant),
            require_approval(),
            Some("cargo"),
            None,
            Utc::now(),
        );
        match outcome.decision {
            JudgmentDecision::AllowWithConstraints(kept) => {
                assert_eq!(kept, constraints(), "scope still bounds execution");
            }
            other => panic!("expected constraints to survive the bypass, got {other:?}"),
        }
    }

    #[test]
    fn elevated_skips_approval_only_for_listed_programs() {
        let grant = elevated(&["cargo", "npm"]);
        let covered = apply_human_authority(
            Some(&grant),
            require_approval(),
            Some("cargo"),
            Some(GrantCapability::ExecuteTests),
            Utc::now(),
        );
        assert!(covered.overridden);

        let uncovered = apply_human_authority(
            Some(&grant),
            require_approval(),
            Some("curl"),
            Some(GrantCapability::ExecuteTests),
            Utc::now(),
        );
        assert!(
            !uncovered.overridden,
            "an unlisted program falls back to Governed"
        );
    }

    #[test]
    fn elevated_never_converts_a_deny() {
        let grant = elevated(&["cargo"]);
        let outcome = apply_human_authority(Some(&grant), deny(), Some("cargo"), None, Utc::now());
        assert!(!outcome.overridden);
        assert!(matches!(outcome.decision, JudgmentDecision::Deny { .. }));
    }

    #[test]
    fn advice_decisions_are_never_overridden() {
        let grant = unrestricted();
        for advice in [
            JudgmentDecision::ModifyAction {
                reason: "wrong file".into(),
            },
            JudgmentDecision::Replan {
                reason: "plan drifted".into(),
            },
        ] {
            let outcome =
                apply_human_authority(Some(&grant), advice.clone(), None, None, Utc::now());
            assert!(
                !outcome.overridden,
                "authority changes whether, not what: {advice:?}"
            );
            assert_eq!(outcome.decision, advice);
        }
    }

    #[test]
    fn expired_and_revoked_grants_never_apply() {
        let mut expired = unrestricted();
        expired.expires_at = Utc::now() - chrono::Duration::minutes(1);
        let outcome = apply_human_authority(
            Some(&expired),
            require_approval(),
            Some("cargo"),
            None,
            Utc::now(),
        );
        assert!(!outcome.overridden, "an expired grant is no grant");

        let mut revoked = unrestricted();
        revoked.revoked = true;
        let outcome = apply_human_authority(
            Some(&revoked),
            require_approval(),
            Some("cargo"),
            None,
            Utc::now(),
        );
        assert!(!outcome.overridden, "a revoked grant is no grant");
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

    #[test]
    fn governed_mode_is_a_no_op_grant() {
        let grant = HumanAuthorityGrant::issue(
            AuthorityMode::Governed,
            human(),
            Utc::now(),
            chrono::Duration::hours(1),
        );
        let outcome = apply_human_authority(
            Some(&grant),
            require_approval(),
            Some("cargo"),
            None,
            Utc::now(),
        );
        assert!(!outcome.overridden);
    }
}
