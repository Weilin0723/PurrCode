//! Human authority and autonomy-grant contracts (PRD §15).
//!
//! PurrCode separates model generation from authorization: a model may propose
//! an action but can never authorize it. This crate pins the contract for
//! *human* authority — the three modes (Governed / Elevated / Unrestricted),
//! the persistent autonomy grant that removes repeated approvals, and the
//! model-restriction invariants that cannot be disabled even under
//! Unrestricted authority (PRD §15.4, §15.5).
//!
//! `AutonomyGrant` is durable value: the control plane stores it once and
//! reuses it; the runtime checks a proposed action against the grant instead of
//! re-prompting. The grant never carries an identity *secret* (PRD §5.1), only a
//! digest of the human subject's identity claims, and the model can never
//! manufacture, widen, or re-scope one (§15.5).

use blake3::Hasher;
use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use thiserror::Error;

pub use purrcode_workspace_contracts::{GrantId, WorkspaceId};

// ---------------------------------------------------------------------------
// Authority modes (PRD §15.1).
// ---------------------------------------------------------------------------

/// How much authority an authenticated human has granted to PurrCode.
///
/// `Governed` is the normal PawGate policy. `Elevated` persistently authorizes
/// a selected set of privileged capabilities. `Unrestricted` lets PurrCode use
/// every capability available to the current OS process identity, the
/// configured Managed Identity, and the current workspace — but §15.4 integrity
/// (typed action, digest, single-use execution identity, scope, evidence,
/// idempotency, terminal ownership) is *still* enforced. Unrestricted is not
/// privilege escalation: PurrCode cannot exceed Azure RBAC, Linux permissions,
/// or network controls.
#[derive(
    Clone, Copy, Debug, Default, Eq, JsonSchema, Ord, PartialEq, PartialOrd, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum HumanAuthorityMode {
    #[default]
    Governed,
    Elevated,
    Unrestricted,
}

impl HumanAuthorityMode {
    /// True when the mode permits all capabilities the process identity already
    /// holds. Even in this mode §15.4 integrity is preserved.
    pub fn is_unrestricted(self) -> bool {
        matches!(self, HumanAuthorityMode::Unrestricted)
    }
}

// ---------------------------------------------------------------------------
// Scopes and capability sets.
// ---------------------------------------------------------------------------

/// Stable identifier for a PurrCode agent / compiled production agent (PRD
/// §16.2). The Factory assigns these; grants may be scoped to one.
#[derive(
    Clone, Copy, Debug, Eq, Hash, JsonSchema, Ord, PartialEq, PartialOrd, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct AgentId(pub uuid::Uuid);

impl AgentId {
    pub fn new() -> Self {
        Self(uuid::Uuid::new_v4())
    }
}

impl Default for AgentId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for AgentId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Azure RBAC scope attached to a grant (subscription / resource group /
/// resource). Free-form ARM scope string per PRD §15.2.
#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct AzureResourceScope {
    /// ARM-style scope id, e.g. `/subscriptions/<id>/resourceGroups/<rg>`.
    pub scope_id: String,
}

/// A named capability string ("read_repository", "run_terminal_command",
/// "install_toolchain", "delete_files_in_workspace", …). Kept as a string so
/// new capabilities do not require a contract change; the capability registry
/// owns the canonical list.
#[derive(
    Clone, Debug, Eq, Hash, JsonSchema, Ord, PartialEq, PartialOrd, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct Capability(pub String);

impl Capability {
    pub fn new(name: impl Into<String>) -> Self {
        Self(name.into())
    }
}

impl std::fmt::Display for Capability {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// A set of capabilities. We use a sorted set so a grant's serialized form is
/// stable and digestible — the lifecycle of "remember for this run" vs "this
/// workspace" is a different field, but the capabilities themselves are
/// order-independent.
#[derive(Clone, Debug, Default, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CapabilitySet(pub BTreeSet<Capability>);

impl CapabilitySet {
    pub fn new() -> Self {
        Self(BTreeSet::new())
    }

    pub fn from_capabilities<I: IntoIterator<Item = Capability>>(iter: I) -> Self {
        Self(iter.into_iter().collect())
    }

    pub fn contains(&self, cap: &Capability) -> bool {
        self.0.contains(cap)
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn insert(&mut self, cap: Capability) -> bool {
        self.0.insert(cap)
    }

    pub fn grant_for(&self, mode: HumanAuthorityMode) -> CapabilitySet {
        match mode {
            // Elevated: exactly the listed capabilities. Governed: none here,
            // normal PawGate policy applies per-action.
            HumanAuthorityMode::Elevated | HumanAuthorityMode::Governed => self.clone(),
            // Unrestricted: the empty set here is intentionally meaningful — an
            // unrestricted grant does not enumerate capabilities, it permits all
            // the process identity already holds. We still keep §15.4 integrity.
            HumanAuthorityMode::Unrestricted => Self::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// Human subject.
// ---------------------------------------------------------------------------

/// Authenticated human the grant binds to (PRD §15.2).
///
/// `identity_claims_digest` is a BLAKE3 digest of the verified identity claims
/// (Entra issuer + audience + object id). It is held — not the claims
/// themselves, and never a token — so a grant can be re-bound on reconnect
/// without a credential round-trip, while still detecting a changed subject.
#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct HumanSubject {
    /// Stable subject id from the verified identity (e.g. Entra object id).
    pub subject_id: String,
    /// BLAKE3 hex digest of the canonicalized verified claims.
    pub identity_claims_digest: String,
    /// Optional display name for the UI authority banner.
    #[serde(default)]
    pub display_name: Option<String>,
}

impl HumanSubject {
    /// Compute the canonical identity-claims digest (PRD §20.1: redact token
    /// diagnostics, validate issuer + audience).
    ///
    /// `claims` is a list of `(issuer, audience, object_id)` tuples in a
    /// canonical sorted order. We hash the concatenation of these verbatim and
    /// return the hex digest; the caller keeps only this digest, never the
    /// claims, reducing the value of a stolen grant record.
    pub fn claims_digest<'a, I>(claims: I) -> String
    where
        I: IntoIterator<Item = (&'a str, &'a str, &'a str)>,
    {
        let mut sorted: Vec<(&str, &str, &str)> = claims.into_iter().collect();
        sorted.sort();
        let mut hasher = Hasher::new();
        for (issuer, audience, oid) in sorted {
            hasher.update(issuer.as_bytes());
            hasher.update(b"|");
            hasher.update(audience.as_bytes());
            hasher.update(b"|");
            hasher.update(oid.as_bytes());
            hasher.update(b"\n");
        }
        hasher.finalize().to_hex().to_string()
    }
}

// ---------------------------------------------------------------------------
// Autonomy grant (PRD §15.2).
// ---------------------------------------------------------------------------

/// A scoped, expiring human autonomy grant (PRD §15.2).
#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct AutonomyGrant {
    pub grant_id: GrantId,
    pub human_subject: HumanSubject,
    pub mode: HumanAuthorityMode,
    /// Scope to a single compiled agent, or None for all agents in the scope.
    #[serde(default)]
    pub agent_scope: Option<AgentId>,
    /// Scope to a single workspace, or None for all workspaces.
    #[serde(default)]
    pub workspace_scope: Option<WorkspaceId>,
    /// Scope to an Azure deployment profile / resource group, or None.
    #[serde(default)]
    pub azure_scope: Option<AzureResourceScope>,
    pub capabilities: CapabilitySet,
    pub issued_at: DateTime<Utc>,
    /// When set, the grant auto-expires and must not be honored after.
    #[serde(default)]
    pub expires_at: Option<DateTime<Utc>>,
    /// Must equal `human_subject.identity_claims_digest`. Kept duplicated on
    /// the grant so a stale-detect check is O(1) without re-deriving claims.
    pub identity_claims_digest: String,
}

/// How long to remember a grant (PRD §15.3 "Persist Once").
#[derive(
    Clone, Copy, Debug, Eq, JsonSchema, Ord, PartialEq, PartialOrd, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum PersistScope {
    /// Remember for a single exact action only.
    Action,
    /// Remember for the duration of one run.
    Run,
    /// Remember for the workspace's lifetime.
    Workspace,
    /// Remember for a compiled agent across all runs.
    Agent,
    /// Remember for an Azure deployment profile.
    AzureDeployment,
}

impl PersistScope {
    /// Ranking used to keep the most-durable applicable grant when several
    /// scopes match a request (we prefer the broadest durable one so the user
    /// is prompted least often).
    pub fn durability_rank(self) -> u8 {
        match self {
            PersistScope::Action => 1,
            PersistScope::Run => 2,
            PersistScope::Workspace => 3,
            PersistScope::Agent => 4,
            PersistScope::AzureDeployment => 5,
        }
    }
}

// ---------------------------------------------------------------------------
// Model restrictions (PRD §15.5). These are pure predicate guards — they
// cannot be disabled and the runtime calls them before honoring any grant.
// ---------------------------------------------------------------------------

/// Every way a model is forbidden from changing authority (PRD §15.5).
///
/// `model_self_authorization` and friends are the only constructor; mutating
/// paths route through `assert_*`, which always return an error — there is no
/// "allow" path. This guarantees a typed contract that the grant store, the
/// PawGate judgment, and the Factory all reference, so a future caller cannot
/// accidentally weaken the boundary.
#[derive(Clone, Copy, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
pub enum ForbiddenModelAction {
    /// A model attempting to create a grant.
    CreateGrant,
    /// A model attempting to widen a grant's capabilities.
    WidenGrant,
    /// A model attempting to change a grant's scope.
    ChangeScope,
    /// A model attempting to impersonate a human subject.
    ImpersonateHuman,
    /// A model attempting to escalate a Governed grant to Unrestricted.
    EscalateMode,
    /// A model attempting to hide that an override was used.
    HideOverride,
}

#[derive(Clone, Debug, Error, JsonSchema, PartialEq, Serialize, Deserialize)]
pub enum AuthorityError {
    #[error("a model may not perform {0:?}")]
    ForbiddenByModelPolicy(ForbiddenModelAction),
    #[error("grant {id} not found")]
    NotFound { id: GrantId },
    #[error("grant {id} has expired")]
    Expired { id: GrantId },
    #[error("identity claims digest mismatch for grant {id}")]
    IdentityMismatch { id: GrantId },
    #[error("grant {id} does not cover capability {capability}")]
    CapabilityNotCovered { id: GrantId, capability: Capability },
    #[error("grant {id} is scoped to a different workspace")]
    OutOfScope { id: GrantId },
}

/// A model can never create a grant. (PRD §15.5, rule 1.)
pub fn assert_model_may_not_create_grant() -> Result<(), AuthorityError> {
    Err(AuthorityError::ForbiddenByModelPolicy(
        ForbiddenModelAction::CreateGrant,
    ))
}

/// A model can never widen (add capabilities to) an existing grant. (§15.5,
/// rule 2.) The caller passes the grant id; this always fails.
pub fn assert_model_may_not_widen_grant(_id: GrantId) -> Result<(), AuthorityError> {
    Err(AuthorityError::ForbiddenByModelPolicy(
        ForbiddenModelAction::WidenGrant,
    ))
}

/// A model can never change a grant's scope. (§15.5, rule 3.)
pub fn assert_model_may_not_change_scope(_id: GrantId) -> Result<(), AuthorityError> {
    Err(AuthorityError::ForbiddenByModelPolicy(
        ForbiddenModelAction::ChangeScope,
    ))
}

/// A model can never impersonate a human; only a verified human subject flows
/// into `HumanSubject`. (§15.5, rule 4.)
pub fn assert_model_may_not_impersonate_human() -> Result<(), AuthorityError> {
    Err(AuthorityError::ForbiddenByModelPolicy(
        ForbiddenModelAction::ImpersonateHuman,
    ))
}

/// A model can never escalate a Governed grant to Unrestricted, or set a
/// grant's mode. (§15.5, rule 5.) The human chooses the mode; a model
/// suggestion cannot.
pub fn assert_model_may_not_escalate_mode() -> Result<(), AuthorityError> {
    Err(AuthorityError::ForbiddenByModelPolicy(
        ForbiddenModelAction::EscalateMode,
    ))
}

/// A model can never hide that an override was used; overrides are surfaced in
/// evidence. (§15.5, rule 6.)
pub fn assert_model_may_not_hide_override() -> Result<(), AuthorityError> {
    Err(AuthorityError::ForbiddenByModelPolicy(
        ForbiddenModelAction::HideOverride,
    ))
}

impl AutonomyGrant {
    /// True if the grant is currently valid (not expired). Lifetime checks are
    /// the runtime's responsibility; this is the pure predicate.
    pub fn is_active_at(&self, now: DateTime<Utc>) -> bool {
        match self.expires_at {
            None => true,
            Some(exp) => now < exp,
        }
    }

    /// True if the grant covers the given capability. Under Unrestricted mode
    /// the empty capability set means "all the process identity holds", but we
    /// still return true here only when asked about a capability that the
    /// process *already* possesses — that check lives in the policy layer; this
    /// predicate answers "is the grant's enumerated set relevant?".
    pub fn covers_capability(&self, cap: &Capability) -> bool {
        if self.mode.is_unrestricted() {
            return true;
        }
        self.capabilities.contains(cap)
    }

    /// True if the grant applies to the given workspace.
    pub fn applies_to_workspace(&self, ws: WorkspaceId) -> bool {
        match self.workspace_scope {
            None => true,
            Some(w) => w == ws,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn subject() -> HumanSubject {
        let digest = HumanSubject::claims_digest([("issuer", "aud", "obj-1")]);
        HumanSubject {
            subject_id: "obj-1".into(),
            identity_claims_digest: digest.clone(),
            display_name: Some("Alice".into()),
        }
    }

    #[test]
    fn model_restriction_guard_functions_all_fail_closed() {
        // No "allow" path exists for any of the model restrictions.
        assert!(matches!(
            assert_model_may_not_create_grant(),
            Err(AuthorityError::ForbiddenByModelPolicy(
                ForbiddenModelAction::CreateGrant
            ))
        ));
        assert!(matches!(
            assert_model_may_not_widen_grant(GrantId::new()),
            Err(AuthorityError::ForbiddenByModelPolicy(
                ForbiddenModelAction::WidenGrant
            ))
        ));
        assert!(matches!(
            assert_model_may_not_change_scope(GrantId::new()),
            Err(AuthorityError::ForbiddenByModelPolicy(
                ForbiddenModelAction::ChangeScope
            ))
        ));
        assert!(matches!(
            assert_model_may_not_impersonate_human(),
            Err(AuthorityError::ForbiddenByModelPolicy(
                ForbiddenModelAction::ImpersonateHuman
            ))
        ));
        assert!(matches!(
            assert_model_may_not_escalate_mode(),
            Err(AuthorityError::ForbiddenByModelPolicy(
                ForbiddenModelAction::EscalateMode
            ))
        ));
        assert!(matches!(
            assert_model_may_not_hide_override(),
            Err(AuthorityError::ForbiddenByModelPolicy(
                ForbiddenModelAction::HideOverride
            ))
        ));
    }

    #[test]
    fn identity_claims_digest_is_stable_and_order_independent() {
        let a =
            HumanSubject::claims_digest([("issuer", "aud", "obj-1"), ("issuer", "aud", "obj-2")]);
        let b =
            HumanSubject::claims_digest([("issuer", "aud", "obj-2"), ("issuer", "aud", "obj-1")]);
        // Sorting makes the digest order-independent.
        assert_eq!(a, b);
        let c = HumanSubject::claims_digest([("issuer", "aud", "obj-3")]);
        assert_ne!(a, c);
    }

    #[test]
    fn unrestricted_covers_any_capability_but_governed_is_enumerated() {
        let mut caps = CapabilitySet::new();
        caps.insert(Capability::new("run_terminal_command"));
        let sub = subject();
        let unrestricted = AutonomyGrant {
            grant_id: GrantId::new(),
            human_subject: sub.clone(),
            mode: HumanAuthorityMode::Unrestricted,
            agent_scope: None,
            workspace_scope: None,
            azure_scope: None,
            capabilities: CapabilitySet::new(),
            issued_at: Utc::now(),
            expires_at: None,
            identity_claims_digest: sub.identity_claims_digest.clone(),
        };
        assert!(unrestricted.covers_capability(&Capability::new("anything")));
        assert!(unrestricted.covers_capability(&Capability::new("run_terminal_command")));

        let governed = AutonomyGrant {
            grant_id: GrantId::new(),
            human_subject: sub,
            mode: HumanAuthorityMode::Governed,
            agent_scope: None,
            workspace_scope: None,
            azure_scope: None,
            capabilities: caps,
            issued_at: Utc::now(),
            expires_at: None,
            identity_claims_digest: "".into(),
        };
        assert!(governed.covers_capability(&Capability::new("run_terminal_command")));
        assert!(!governed.covers_capability(&Capability::new("delete_files")));
    }

    #[test]
    fn grant_expires_correctly() {
        let sub = subject();
        let now = Utc::now();
        let later = now + chrono::Duration::hours(1);
        let g = AutonomyGrant {
            grant_id: GrantId::new(),
            human_subject: sub,
            mode: HumanAuthorityMode::Governed,
            agent_scope: None,
            workspace_scope: None,
            azure_scope: None,
            capabilities: CapabilitySet::new(),
            issued_at: now,
            expires_at: Some(later),
            identity_claims_digest: "".into(),
        };
        assert!(g.is_active_at(now));
        assert!(!g.is_active_at(later + chrono::Duration::seconds(1)));
    }

    #[test]
    fn applies_to_workspace_respects_scope() {
        let sub = subject();
        let w1 = WorkspaceId::new();
        let w2 = WorkspaceId::new();
        let g1 = AutonomyGrant {
            grant_id: GrantId::new(),
            human_subject: sub.clone(),
            mode: HumanAuthorityMode::Governed,
            agent_scope: None,
            workspace_scope: Some(w1),
            azure_scope: None,
            capabilities: CapabilitySet::new(),
            issued_at: Utc::now(),
            expires_at: None,
            identity_claims_digest: sub.identity_claims_digest,
        };
        assert!(g1.applies_to_workspace(w1));
        assert!(!g1.applies_to_workspace(w2));
        // Unscoped workspace applies to all.
        let g2 = AutonomyGrant {
            workspace_scope: None,
            ..g1
        };
        assert!(g2.applies_to_workspace(w1));
        assert!(g2.applies_to_workspace(w2));
    }

    #[test]
    fn persist_scope_durability_ranks_most_durable_for_broadest() {
        assert!(
            PersistScope::AzureDeployment.durability_rank()
                > PersistScope::Workspace.durability_rank()
        );
        assert!(PersistScope::Workspace.durability_rank() > PersistScope::Action.durability_rank());
    }

    #[test]
    fn human_authority_mode_default_is_governed() {
        assert_eq!(HumanAuthorityMode::default(), HumanAuthorityMode::Governed);
        assert!(!HumanAuthorityMode::Governed.is_unrestricted());
        assert!(HumanAuthorityMode::Unrestricted.is_unrestricted());
    }

    #[test]
    fn capability_set_grant_for_unrestricted_yields_empty() {
        let cs = CapabilitySet::from_capabilities([Capability::new("x")]);
        let unrestricted = cs.grant_for(HumanAuthorityMode::Unrestricted);
        // Unrestricted does not enumerate capabilities.
        assert!(unrestricted.is_empty());
        let elevated = cs.grant_for(HumanAuthorityMode::Elevated);
        assert!(elevated.contains(&Capability::new("x")));
    }

    #[test]
    fn grant_round_trips_json() {
        let sub = subject();
        let g = AutonomyGrant {
            grant_id: GrantId::new(),
            human_subject: sub,
            mode: HumanAuthorityMode::Elevated,
            agent_scope: Some(AgentId::new()),
            workspace_scope: Some(WorkspaceId::new()),
            azure_scope: Some(AzureResourceScope {
                scope_id: "/subs/x".into(),
            }),
            capabilities: CapabilitySet::from_capabilities([Capability::new(
                "run_terminal_command",
            )]),
            issued_at: Utc::now(),
            expires_at: Some(Utc::now() + chrono::Duration::days(1)),
            identity_claims_digest: "abc".into(),
        };
        let j = serde_json::to_string(&g).unwrap();
        let back: AutonomyGrant = serde_json::from_str(&j).unwrap();
        assert_eq!(g, back);
    }
}
