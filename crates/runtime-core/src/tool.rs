//! The uniform tool contract (v1.3 §4.1).
//!
//! Every callable tool — native, MCP, skill, or future plugin — is one
//! [`ToolDescriptor`]. There is **no public constructor and no public field**;
//! the only way to obtain a descriptor is [`crate::CapabilityRegistry::admit_tool`],
//! which applies the workspace ceiling via [`ToolDescriptorProposal::restrict`].
//! That is the structural argument of the v1.3 permission-ceiling proof (§9.2):
//! a descriptor that was not restricted cannot be constructed.
//!
//! The lattice operators are copied verbatim from `SignedPolicyPack::restrict`
//! (purrcode-pawgate §9.1): allow-sets intersect, deny-sets union, numeric
//! budgets take the min, permission booleans AND. On the four new axes below,
//! capability axes take the min/intersection (capability shrinks) and the
//! approval axis takes the max (friction grows). A project file can always ask
//! for *more* approval; it can never ask for less.

use super::capability::{AdmissionDiagnostic, CapabilityId, DiagnosticSeverity};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::PathBuf;

/// Stable, namespaced identity of a callable tool.
///
/// Format: `<provider-namespace>:<name>` — e.g. `native:read_file`,
/// `mcp:github/create_issue`, `skill:rust-review/lint`. The namespace prefix is
/// assigned by the registry, never by the provider, which is what stops a
/// user-installed skill from shadowing `purrcode.skill-registry`.
#[derive(
    Clone, Debug, Eq, Hash, JsonSchema, Ord, PartialEq, PartialOrd, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct ToolId(String);

impl ToolId {
    pub fn native(name: &str) -> Self {
        Self(format!("native:{name}"))
    }

    pub fn mcp(server_id: &str, tool: &str) -> Self {
        Self(format!("mcp:{server_id}/{tool}"))
    }

    pub fn skill(skill_id: &str, tool: &str) -> Self {
        Self(format!("skill:{skill_id}/{tool}"))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Parses an id string with one of the three known namespaces. Used when
    /// loading allowlists from config; registry-admitted ids always carry a
    /// known prefix.
    pub fn parse(raw: &str) -> Option<Self> {
        if raw.starts_with("native:") || raw.starts_with("mcp:") || raw.starts_with("skill:") {
            Some(Self(raw.to_owned()))
        } else {
            None
        }
    }

    /// The provider namespace prefix. Registered ids always carry one of the
    /// three prefixes; the fallback is unreachable for any registry-admitted
    /// descriptor.
    pub fn provider(&self) -> ToolProvider {
        if self.0.starts_with("mcp:") {
            ToolProvider::Mcp
        } else if self.0.starts_with("skill:") {
            ToolProvider::Skill
        } else {
            ToolProvider::Native
        }
    }

    /// The namespace segment of the id (`"native"`, `"mcp"`, `"skill"`), or
    /// `None` for an id with no known prefix. Used by the registry to verify
    /// that a provider does not claim a namespace it does not own.
    pub fn namespace(&self) -> Option<&'static str> {
        if self.0.starts_with("native:") {
            Some("native")
        } else if self.0.starts_with("mcp:") {
            Some("mcp")
        } else if self.0.starts_with("skill:") {
            Some("skill")
        } else {
            None
        }
    }

    /// Splits an `mcp:` id into its `(server_id, tool_name)` pair. Returns
    /// `None` for non-MCP ids. Used by the daemon executor to find the server
    /// config and the tool name for `McpHost::call_authorized`.
    pub fn mcp_parts(&self) -> Option<(&str, &str)> {
        let rest = self.0.strip_prefix("mcp:")?;
        let (server, tool) = rest.split_once('/')?;
        if server.is_empty() || tool.is_empty() {
            return None;
        }
        Some((server, tool))
    }
}

impl std::fmt::Display for ToolId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Where a tool executes. The one place provider survives after admission:
/// `claw::ToolRuntime::execute` dispatches on this alone.
#[derive(
    Clone, Copy, Debug, Eq, Hash, JsonSchema, Ord, PartialEq, PartialOrd, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum ToolProvider {
    /// Executed by Claw in the sandbox.
    Native,
    /// Executed by McpHost over stdio or HTTP.
    Mcp,
    /// Executed by the skill runtime (script entrypoint) — or purely
    /// instructional, in which case there is no tool at all.
    Skill,
}

/// What a tool is capable of doing to the world. Total order: the lattice meet
/// in `ToolDescriptor::restrict` is `min`, so restriction can only move DOWN.
#[derive(
    Clone, Copy, Debug, Eq, JsonSchema, Ord, PartialEq, PartialOrd, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum SideEffectClass {
    /// Observes only. Eligible for the batch read path and for the
    /// `requires_contextual_judgment` skip.
    Read = 0,
    /// Mutates files inside the authorized filesystem scope.
    Write = 1,
    /// Spawns a process or otherwise has effects Claw cannot enumerate.
    Execute = 2,
    /// Irreversible or externally visible (delete, publish, POST, pay).
    Destructive = 3,
}

/// Network reach. Lattice meet: `None` is the floor, `Hosts` is tighter than
/// `Any`, and two `Hosts` scopes intersect on their allowed host sets.
#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum NetworkScope {
    /// Maps to `ActionConstraints.network = false`. The only value Claw accepts
    /// today.
    None,
    /// Only these hosts. Enforced by the provider adapter, NOT by Claw — see
    /// the UNVERIFIED note in the architecture doc §9.5.
    Hosts { allowed: BTreeSet<String> },
    /// Unrestricted egress. Requires `ApprovalPolicy::AlwaysAsk` or higher.
    Any,
}

impl NetworkScope {
    pub fn none() -> Self {
        NetworkScope::None
    }

    /// The lattice meet. Capability shrinks: `None` ∩ anything = `None`.
    pub fn meet(&self, other: &NetworkScope) -> NetworkScope {
        use NetworkScope::*;
        match (self, other) {
            (None, _) | (_, None) => None,
            (Any, Any) => Any,
            (Hosts { allowed: a }, Hosts { allowed: b }) => Hosts {
                allowed: a.intersection(b).cloned().collect(),
            },
            (Hosts { allowed: a }, Any) | (Any, Hosts { allowed: a }) => {
                Hosts { allowed: a.clone() }
            }
        }
    }
}

/// Filesystem reach. Lattice meet by containment: `None` < `WorktreeRead` <
/// `Worktree`; two `Worktree` scopes intersect on their write globs and take
/// the min changed-file budget.
#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FilesystemScope {
    /// No filesystem access at all. maximum_changed_files = 0, no read grant.
    None,
    /// Read-only inside the session worktree.
    WorktreeRead,
    /// Read + write inside the session worktree, confined to these globs.
    /// This is the ONLY variant that can produce a non-empty
    /// `ActionConstraints.allowed_write_globs`.
    Worktree {
        write_globs: Vec<String>,
        maximum_changed_files: usize,
    },
}

impl FilesystemScope {
    pub fn none() -> Self {
        FilesystemScope::None
    }

    /// The most permissive scope a proposal can request — unbounded worktree
    /// write. Everything narrows from here.
    pub fn maximum() -> Self {
        FilesystemScope::Worktree {
            write_globs: vec!["**".into()],
            maximum_changed_files: usize::MAX,
        }
    }

    /// The lattice meet. Capability shrinks: `None` ∩ anything = `None`.
    ///
    /// Glob intersection is exact-string: two overlapping globs that are not
    /// literally equal are treated as disjoint (only globs present in both
    /// survive). This is conservative — it narrows more than a glob-solver
    /// would — which is the direction the ceiling proof requires.
    pub fn intersect(&self, other: &FilesystemScope) -> FilesystemScope {
        use FilesystemScope::*;
        match (self, other) {
            (None, _) | (_, None) => None,
            (WorktreeRead, WorktreeRead) => WorktreeRead,
            (WorktreeRead, Worktree { .. }) | (Worktree { .. }, WorktreeRead) => WorktreeRead,
            (
                Worktree {
                    write_globs: a,
                    maximum_changed_files: ma,
                },
                Worktree {
                    write_globs: b,
                    maximum_changed_files: mb,
                },
            ) => {
                let mut left = a.clone();
                let mut right = b.clone();
                left.sort();
                right.sort();
                let mut left_iter = left.into_iter();
                let mut right_iter = right.into_iter();
                let mut l = left_iter.next();
                let mut r = right_iter.next();
                let mut intersection = Vec::new();
                while let (Some(x), Some(y)) = (&l, &r) {
                    match x.cmp(y) {
                        std::cmp::Ordering::Less => l = left_iter.next(),
                        std::cmp::Ordering::Greater => r = right_iter.next(),
                        std::cmp::Ordering::Equal => {
                            intersection.push(x.clone());
                            l = left_iter.next();
                            r = right_iter.next();
                        }
                    }
                }
                Worktree {
                    write_globs: intersection,
                    maximum_changed_files: (*ma).min(*mb),
                }
            }
        }
        .collapse_empty_write_globs()
    }

    /// Narrow this scope to a requested subset of paths (v1.4 §5.3).
    ///
    /// This is **not** [`Self::intersect`]. Intersect combines two independent
    /// *ceilings* and is deliberately exact-string, so `src/**` ∩ `**` is empty.
    /// A delegation's `allowed_paths` is not a ceiling — it is a request to work
    /// inside a subset of what the ceiling already permits — so each requested
    /// pattern survives when some ceiling glob *covers* it, and the surviving
    /// patterns (never the ceiling's own, wider ones) become the new write set.
    ///
    /// The result can only be narrower: a requested pattern the ceiling does not
    /// cover is dropped, and a request that survives nothing collapses to
    /// `WorktreeRead`. `maximum_changed_files` is carried through unchanged;
    /// callers clamp it separately from the delegation budget.
    pub fn narrow_to_paths<'a, I>(&self, requested: I) -> FilesystemScope
    where
        I: IntoIterator<Item = &'a str>,
    {
        match self {
            FilesystemScope::None => FilesystemScope::None,
            FilesystemScope::WorktreeRead => FilesystemScope::WorktreeRead,
            FilesystemScope::Worktree {
                write_globs,
                maximum_changed_files,
            } => {
                let surviving: Vec<String> = requested
                    .into_iter()
                    .filter(|candidate| {
                        write_globs
                            .iter()
                            .any(|permitted| glob_covers(permitted, candidate))
                    })
                    .map(str::to_owned)
                    .collect();
                FilesystemScope::Worktree {
                    write_globs: surviving,
                    maximum_changed_files: *maximum_changed_files,
                }
                .collapse_empty_write_globs()
            }
        }
    }

    /// Clamp the changed-file budget down. Never raises it.
    pub fn with_changed_file_limit(self, limit: usize) -> FilesystemScope {
        match self {
            FilesystemScope::Worktree {
                write_globs,
                maximum_changed_files,
            } => FilesystemScope::Worktree {
                write_globs,
                maximum_changed_files: maximum_changed_files.min(limit),
            }
            .collapse_empty_write_globs(),
            other => other,
        }
    }

    /// True when `self` permits no more than `bound` — a genuine containment
    /// predicate, unlike `intersect(bound) == self`, which is exact-string on
    /// globs and would call `src/**` wider than `**`.
    pub fn is_within(&self, bound: &FilesystemScope) -> bool {
        use FilesystemScope::*;
        match (self, bound) {
            (None, _) => true,
            (_, None) => false,
            (WorktreeRead, WorktreeRead | Worktree { .. }) => true,
            (Worktree { .. }, WorktreeRead) => false,
            (
                Worktree {
                    write_globs: inner,
                    maximum_changed_files: inner_max,
                },
                Worktree {
                    write_globs: outer,
                    maximum_changed_files: outer_max,
                },
            ) => {
                inner_max <= outer_max
                    && inner.iter().all(|candidate| {
                        outer
                            .iter()
                            .any(|permitted| glob_covers(permitted, candidate))
                    })
            }
        }
    }

    /// The write globs this scope permits, or an empty slice for a scope that
    /// permits no writes.
    pub fn write_globs(&self) -> &[String] {
        match self {
            FilesystemScope::Worktree { write_globs, .. } => write_globs,
            _ => &[],
        }
    }

    /// A `Worktree` scope whose glob intersection produced no surviving globs
    /// permits no writes — collapse it to `WorktreeRead` so the descriptor
    /// reads honestly instead of carrying an empty glob list.
    fn collapse_empty_write_globs(self) -> FilesystemScope {
        match self {
            FilesystemScope::Worktree {
                write_globs,
                maximum_changed_files,
            } if write_globs.is_empty() || maximum_changed_files == 0 => {
                FilesystemScope::WorktreeRead
            }
            other => other,
        }
    }
}

/// Does write glob `outer` permit everything `inner` permits?
///
/// Deliberately syntactic and conservative — a full glob-containment solver is
/// undecidable in the general case, and the safe direction for a permission
/// check is to answer "no" when unsure. Three rules cover every scope PurrCode
/// actually writes: the universal glob, an exact match, and a directory prefix
/// glob (`src/**` covers `src/auth/**` and `src/auth/mod.rs`). Anything else is
/// refused, which can only narrow authority, never widen it.
pub fn glob_covers(outer: &str, inner: &str) -> bool {
    if outer == "**" || outer == inner {
        return true;
    }
    match outer.strip_suffix("**") {
        // `src/**` → prefix `src/`; `**` alone was handled above.
        Some(prefix) if !prefix.is_empty() => {
            inner.starts_with(prefix) && !inner[prefix.len()..].is_empty()
        }
        _ => false,
    }
}

/// True when network scope `inner` reaches no further than `outer`.
pub fn network_is_within(inner: &NetworkScope, outer: &NetworkScope) -> bool {
    use NetworkScope::*;
    match (inner, outer) {
        (None, _) => true,
        (_, None) => false,
        (Hosts { allowed }, Hosts { allowed: permitted }) => allowed.is_subset(permitted),
        (Hosts { .. }, Any) => true,
        (Any, Any) => true,
        (Any, Hosts { .. }) => false,
    }
}

/// Who has to say yes. Total order; the lattice join is `max` (more friction
/// wins), so restriction can only move UP.
#[derive(
    Clone, Copy, Debug, Eq, JsonSchema, Ord, PartialEq, PartialOrd, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalPolicy {
    /// Policy may allow deterministically with constraints. Replaces the
    /// `trusted_tools` bypass at purrcode-daemon/src/lib.rs:3863-3876.
    PreAuthorized = 0,
    /// Deterministic allow for Read; approval for anything else.
    ByClass = 1,
    /// Always RequireApproval, regardless of side effect class.
    AlwaysAsk = 2,
    /// Never authorizable in this workspace. Produced by restriction when a
    /// proposal exceeds the ceiling on any axis.
    Forbidden = 3,
}

impl ApprovalPolicy {
    pub fn always_ask() -> Self {
        ApprovalPolicy::AlwaysAsk
    }
}

/// Provenance. `Builtin` descriptors skip restriction (they ARE the ceiling
/// source); everything else is restricted on admission.
#[derive(
    Clone, Copy, Debug, Eq, JsonSchema, Ord, PartialEq, PartialOrd, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum DescriptorOrigin {
    Builtin,
    /// From `~/.purrcode/` — the user's own machine-level declaration.
    User,
    /// From `<repo>/.purrcode/` — checked-in, therefore untrusted content.
    Project,
    /// Reported by a remote MCP server. Least trusted: the server authored it.
    RemoteDiscovery,
}

/// The uniform tool contract. **No public constructor and no public field.**
///
/// The only way to obtain one is [`crate::CapabilityRegistry::admit_tool`],
/// which applies `restrict`. Its `Deserialize` impl is a read-back path that
/// **re-verifies the digest**: a serialized descriptor whose
/// `descriptor_digest` does not match a recomputation over the carried fields
/// fails to deserialize, so a forged or tampered digest can never reach the
/// runtime as an admitted descriptor.
#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize)]
pub struct ToolDescriptor {
    id: ToolId,
    provider: ToolProvider,
    display_name: String,
    description: String,
    /// JSON Schema for `arguments`. Validated in `Policy::evaluate_tool`,
    /// BEFORE `SessionStore::authorize`.
    schema: serde_json::Value,
    /// Capabilities this tool claims to satisfy.
    capabilities: BTreeSet<CapabilityId>,
    side_effect_class: SideEffectClass,
    network_scope: NetworkScope,
    filesystem_scope: FilesystemScope,
    approval_policy: ApprovalPolicy,
    /// Where this descriptor came from. Drives restriction strength and
    /// evidence redaction.
    origin: DescriptorOrigin,
    /// blake3 over the canonical serialization of every field above. Enters
    /// `ProposedAction::digest` so a registry refresh invalidates outstanding
    /// authorizations.
    descriptor_digest: String,
}

impl ToolDescriptor {
    pub fn id(&self) -> &ToolId {
        &self.id
    }

    pub fn provider(&self) -> ToolProvider {
        self.provider
    }

    pub fn display_name(&self) -> &str {
        &self.display_name
    }

    pub fn description(&self) -> &str {
        &self.description
    }

    pub fn schema(&self) -> &serde_json::Value {
        &self.schema
    }

    pub fn capabilities(&self) -> &BTreeSet<CapabilityId> {
        &self.capabilities
    }

    pub fn side_effect_class(&self) -> SideEffectClass {
        self.side_effect_class
    }

    pub fn network_scope(&self) -> &NetworkScope {
        &self.network_scope
    }

    pub fn filesystem_scope(&self) -> &FilesystemScope {
        &self.filesystem_scope
    }

    pub fn approval_policy(&self) -> ApprovalPolicy {
        self.approval_policy
    }

    pub fn origin(&self) -> DescriptorOrigin {
        self.origin
    }

    pub fn descriptor_digest(&self) -> &str {
        &self.descriptor_digest
    }

    /// Strip this descriptor of every capability and mark it `Forbidden`. Used
    /// by [`crate::CapabilityRegistry::for_agent`] when an agent's ceiling
    /// cannot support what the tool needs: the descriptor stays visible for a
    /// precise refusal message, but it is not callable and claims nothing.
    pub(crate) fn forbid(self) -> ToolDescriptor {
        ToolDescriptor {
            side_effect_class: SideEffectClass::Read,
            network_scope: NetworkScope::None,
            filesystem_scope: FilesystemScope::None,
            approval_policy: ApprovalPolicy::Forbidden,
            descriptor_digest: String::new(),
            ..self
        }
        .recompute_digest()
    }

    /// Re-restrict an already-admitted descriptor against a (possibly tighter)
    /// ceiling. Idempotent against the ceiling it was admitted with. This is the
    /// per-agent intersection `CapabilityRegistry::for_agent` applies.
    pub(crate) fn restrict(
        self,
        ceiling: &ToolCeiling,
    ) -> (ToolDescriptor, Vec<AdmissionDiagnostic>) {
        let proposal = ToolDescriptorProposal {
            id: self.id,
            provider: self.provider,
            display_name: self.display_name,
            description: self.description,
            schema: self.schema,
            capabilities: self.capabilities,
            side_effect_class: self.side_effect_class,
            network_scope: self.network_scope,
            filesystem_scope: self.filesystem_scope,
            approval_policy: self.approval_policy,
            origin: self.origin,
        };
        proposal.restrict(ceiling)
    }

    fn recompute_digest(mut self) -> Self {
        // The digest is over the canonical serialization of every field EXCEPT
        // the digest itself: blank it first, hash, then store. This makes the
        // digest a pure function of the carried fields, so a read-back can
        // re-derive it and verify — and a forged digest can never survive.
        self.descriptor_digest.clear();
        let canonical = serde_json::to_vec(&self).expect("ToolDescriptor is always serializable");
        self.descriptor_digest = blake3::hash(&canonical).to_hex().to_string();
        self
    }

    /// Re-derive the digest from the carried fields and compare to the stored
    /// one. Returns true iff the stored digest is genuine (computed over these
    /// exact fields).
    fn digest_is_genuine(&self) -> bool {
        let mut blanked = self.clone();
        blanked.descriptor_digest.clear();
        let canonical =
            serde_json::to_vec(&blanked).expect("ToolDescriptor is always serializable");
        blake3::hash(&canonical).to_hex().to_string() == self.descriptor_digest
    }
}

/// Read-back path. Deserializing a `ToolDescriptor` is permitted ONLY to load
/// an already-admitted descriptor (e.g. from `tool_descriptor_pins`), and the
/// digest is re-verified: if the payload's `descriptor_digest` was not computed
/// over the exact carried fields, deserialization fails. A forged or stale
/// digest can never be admitted this way.
impl<'de> Deserialize<'de> for ToolDescriptor {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct Wire {
            id: ToolId,
            provider: ToolProvider,
            display_name: String,
            description: String,
            schema: serde_json::Value,
            #[serde(default)]
            capabilities: BTreeSet<CapabilityId>,
            side_effect_class: SideEffectClass,
            network_scope: NetworkScope,
            filesystem_scope: FilesystemScope,
            approval_policy: ApprovalPolicy,
            origin: DescriptorOrigin,
            descriptor_digest: String,
        }
        let wire = Wire::deserialize(deserializer)?;
        let descriptor = ToolDescriptor {
            id: wire.id,
            provider: wire.provider,
            display_name: wire.display_name,
            description: wire.description,
            schema: wire.schema,
            capabilities: wire.capabilities,
            side_effect_class: wire.side_effect_class,
            network_scope: wire.network_scope,
            filesystem_scope: wire.filesystem_scope,
            approval_policy: wire.approval_policy,
            origin: wire.origin,
            descriptor_digest: wire.descriptor_digest,
        };
        if !descriptor.digest_is_genuine() {
            return Err(serde::de::Error::custom(
                "ToolDescriptor digest does not match the carried fields; refusing forged descriptor",
            ));
        }
        Ok(descriptor)
    }
}

/// An unrestricted claim. This is what providers produce. It can never be
/// executed; it must pass through [`crate::CapabilityRegistry::admit_tool`]
/// first.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ToolDescriptorProposal {
    pub id: ToolId,
    pub provider: ToolProvider,
    pub display_name: String,
    pub description: String,
    pub schema: serde_json::Value,
    #[serde(default)]
    pub capabilities: BTreeSet<CapabilityId>,
    pub side_effect_class: SideEffectClass,
    #[serde(default = "NetworkScope::none")]
    pub network_scope: NetworkScope,
    #[serde(default = "FilesystemScope::none")]
    pub filesystem_scope: FilesystemScope,
    #[serde(default = "ApprovalPolicy::always_ask")]
    pub approval_policy: ApprovalPolicy,
    pub origin: DescriptorOrigin,
}

impl ToolDescriptorProposal {
    /// The restriction lattice, applied on admission. TOTAL: rebuilds every
    /// field, so a new descriptor field cannot be forgotten (the struct literal
    /// will not compile). The digest is computed over the RESTRICTED values,
    /// never the proposed ones.
    pub(crate) fn restrict(
        self,
        ceiling: &ToolCeiling,
    ) -> (ToolDescriptor, Vec<AdmissionDiagnostic>) {
        let mut diagnostics = Vec::new();

        let denied = ceiling.denied_tool_ids.contains(self.id.as_str());
        let side_effect_class = self.side_effect_class.min(ceiling.maximum_side_effect);
        let network_scope = self.network_scope.meet(&ceiling.maximum_network);
        let filesystem_scope = self.filesystem_scope.intersect(&ceiling.maximum_filesystem);
        let approval_policy = if denied {
            ApprovalPolicy::Forbidden
        } else {
            self.approval_policy.max(ceiling.minimum_approval)
        };

        let subject = self.id.as_str().to_string();
        if side_effect_class != self.side_effect_class {
            diagnostics.push(AdmissionDiagnostic {
                source_path: None,
                subject: subject.clone(),
                severity: DiagnosticSeverity::Restricted,
                message: "side_effect_class clamped down to the workspace ceiling".into(),
                restricted_fields: vec![(
                    "side_effect_class".into(),
                    format!("{:?}", self.side_effect_class),
                    format!("{side_effect_class:?}"),
                )],
            });
        }
        if network_scope != self.network_scope {
            diagnostics.push(AdmissionDiagnostic {
                source_path: None,
                subject: subject.clone(),
                severity: DiagnosticSeverity::Restricted,
                message: "network_scope clamped down to the workspace ceiling".into(),
                restricted_fields: vec![(
                    "network_scope".into(),
                    format!("{:?}", self.network_scope),
                    format!("{network_scope:?}"),
                )],
            });
        }
        if filesystem_scope != self.filesystem_scope {
            diagnostics.push(AdmissionDiagnostic {
                source_path: None,
                subject: subject.clone(),
                severity: DiagnosticSeverity::Restricted,
                message: "filesystem_scope clamped down to the workspace ceiling".into(),
                restricted_fields: vec![(
                    "filesystem_scope".into(),
                    format!("{:?}", self.filesystem_scope),
                    format!("{filesystem_scope:?}"),
                )],
            });
        }
        if approval_policy != self.approval_policy {
            let severity = if denied {
                DiagnosticSeverity::Rejected
            } else {
                DiagnosticSeverity::Restricted
            };
            let message = if denied {
                "tool id is denied by the workspace ceiling".into()
            } else {
                "approval_policy raised to the workspace minimum friction".into()
            };
            diagnostics.push(AdmissionDiagnostic {
                source_path: None,
                subject: subject.clone(),
                severity,
                message,
                restricted_fields: vec![(
                    "approval_policy".into(),
                    format!("{:?}", self.approval_policy),
                    format!("{approval_policy:?}"),
                )],
            });
        }

        let descriptor = ToolDescriptor {
            id: self.id,
            provider: self.provider,
            display_name: self.display_name,
            description: self.description,
            schema: self.schema,
            capabilities: self.capabilities,
            side_effect_class,
            network_scope,
            filesystem_scope,
            approval_policy,
            origin: self.origin,
            descriptor_digest: String::new(),
        }
        .recompute_digest();

        (descriptor, diagnostics)
    }
}

/// The workspace-wide upper bound. Derived from `purrcode_pawgate::Policy` by
/// `Policy::tool_ceiling()` — pawgate stays the authority, runtime-core stays
/// pure lattice math with no crypto beyond the digest and no I/O.
#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct ToolCeiling {
    pub maximum_side_effect: SideEffectClass,
    pub maximum_network: NetworkScope,
    pub maximum_filesystem: FilesystemScope,
    /// Minimum friction. A proposal may only ask for MORE approval, never less.
    pub minimum_approval: ApprovalPolicy,
    /// Tools whose ids match are Forbidden outright. Deny beats everything.
    pub denied_tool_ids: BTreeSet<String>,
}

impl ToolCeiling {
    /// The most permissive ceiling that can exist: unbounded worktree write,
    /// unrestricted egress, destructive side effects, no denied ids. Every
    /// narrower ceiling is a `meet` away from this one, so it is the identity
    /// element callers fold over.
    pub fn maximum() -> Self {
        Self {
            maximum_side_effect: SideEffectClass::Destructive,
            maximum_network: NetworkScope::Any,
            maximum_filesystem: FilesystemScope::maximum(),
            minimum_approval: ApprovalPolicy::PreAuthorized,
            denied_tool_ids: BTreeSet::new(),
        }
    }
    /// A ceiling that authorizes nothing: no side effects beyond reading, no
    /// egress, no filesystem, and every proposal forbidden.
    pub fn nothing() -> Self {
        Self {
            maximum_side_effect: SideEffectClass::Read,
            maximum_network: NetworkScope::None,
            maximum_filesystem: FilesystemScope::None,
            minimum_approval: ApprovalPolicy::Forbidden,
            denied_tool_ids: BTreeSet::new(),
        }
    }

    /// The lattice meet of two ceilings — the v1.4 delegation invariant
    /// *authority can only shrink* in one function.
    ///
    /// Every capability axis takes the tighter value (min / lattice meet) and
    /// the approval axis takes the higher friction (max), exactly as
    /// [`ToolDescriptorProposal::restrict`] does for a single descriptor. Deny
    /// sets union: a tool denied by *either* input stays denied, because a deny
    /// is the one direction a combination must never be able to undo.
    ///
    /// `meet` is commutative, associative and idempotent, which is what lets a
    /// delegation fold `workspace ∩ parent ∩ profile ∩ task` in any order and
    /// still be unable to produce authority none of the inputs had.
    pub fn meet(&self, other: &ToolCeiling) -> ToolCeiling {
        ToolCeiling {
            maximum_side_effect: self.maximum_side_effect.min(other.maximum_side_effect),
            maximum_network: self.maximum_network.meet(&other.maximum_network),
            maximum_filesystem: self.maximum_filesystem.intersect(&other.maximum_filesystem),
            minimum_approval: self.minimum_approval.max(other.minimum_approval),
            denied_tool_ids: self
                .denied_tool_ids
                .union(&other.denied_tool_ids)
                .cloned()
                .collect(),
        }
    }

    /// True when `self` grants no more than `bound` on every axis. This is the
    /// predicate the delegation contract asserts before a worker may run.
    ///
    /// Containment, not `meet(bound) == self`: glob intersection is
    /// exact-string, so the equality form would report `src/**` as wider than
    /// `**`. Deny sets are the one axis where *more* is narrower, so `bound`'s
    /// denials must all be present in `self`.
    pub fn is_within(&self, bound: &ToolCeiling) -> bool {
        self.maximum_side_effect <= bound.maximum_side_effect
            && network_is_within(&self.maximum_network, &bound.maximum_network)
            && self.maximum_filesystem.is_within(&bound.maximum_filesystem)
            && self.minimum_approval >= bound.minimum_approval
            && bound.denied_tool_ids.is_subset(&self.denied_tool_ids)
    }

    /// True when the ceiling permits writing to the filesystem at all.
    pub fn permits_write(&self) -> bool {
        self.maximum_side_effect >= SideEffectClass::Write
            && matches!(self.maximum_filesystem, FilesystemScope::Worktree { .. })
    }
}

/// An invocation of a registered tool. Replaces the five provider-shaped
/// variants over time; `ExternalTool` is kept during migration (§10).
#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct ToolInvocation {
    pub tool_id: ToolId,
    pub arguments: serde_json::Value,
    pub working_directory: PathBuf,
    /// Binds the invocation to the exact descriptor that was judged.
    pub descriptor_digest: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn capability(raw: &str) -> CapabilityId {
        CapabilityId::parse(raw).unwrap()
    }

    fn permissive_ceiling() -> ToolCeiling {
        ToolCeiling {
            maximum_side_effect: SideEffectClass::Destructive,
            maximum_network: NetworkScope::Any,
            maximum_filesystem: FilesystemScope::Worktree {
                write_globs: vec!["**".into()],
                maximum_changed_files: usize::MAX,
            },
            minimum_approval: ApprovalPolicy::ByClass,
            denied_tool_ids: BTreeSet::new(),
        }
    }

    fn read_proposal(id: &str) -> ToolDescriptorProposal {
        ToolDescriptorProposal {
            id: ToolId::native(id),
            provider: ToolProvider::Native,
            display_name: id.into(),
            description: "test tool".into(),
            schema: serde_json::json!({ "type": "object" }),
            capabilities: BTreeSet::new(),
            side_effect_class: SideEffectClass::Read,
            network_scope: NetworkScope::None,
            filesystem_scope: FilesystemScope::WorktreeRead,
            approval_policy: ApprovalPolicy::ByClass,
            origin: DescriptorOrigin::Builtin,
        }
    }

    fn admit(proposal: ToolDescriptorProposal, ceiling: &ToolCeiling) -> ToolDescriptor {
        proposal.restrict(ceiling).0
    }

    #[test]
    fn restrict_is_idempotent() {
        let ceiling = permissive_ceiling();
        let proposal = read_proposal("read_file");
        let once = admit(proposal, &ceiling);
        let twice = once.clone().restrict(&ceiling).0;
        assert_eq!(
            once, twice,
            "re-restricting against the same ceiling is a no-op"
        );
    }

    #[test]
    fn restrict_is_monotone_decreasing_on_all_axes() {
        let ceiling = ToolCeiling {
            maximum_side_effect: SideEffectClass::Write,
            maximum_network: NetworkScope::Hosts {
                allowed: ["api.example.com".into()].into_iter().collect(),
            },
            maximum_filesystem: FilesystemScope::Worktree {
                write_globs: vec!["src/**".into()],
                maximum_changed_files: 2,
            },
            minimum_approval: ApprovalPolicy::AlwaysAsk,
            denied_tool_ids: BTreeSet::from(["native:blocked".into()]),
        };

        // Read proposal that asks for everything: nothing survives untouched.
        let proposal = ToolDescriptorProposal {
            side_effect_class: SideEffectClass::Destructive,
            network_scope: NetworkScope::Any,
            filesystem_scope: FilesystemScope::Worktree {
                write_globs: vec!["**".into()],
                maximum_changed_files: usize::MAX,
            },
            approval_policy: ApprovalPolicy::PreAuthorized,
            ..read_proposal("anything")
        };
        let (descriptor, _) = proposal.restrict(&ceiling);
        assert!(
            descriptor.side_effect_class() <= ceiling.maximum_side_effect,
            "side_effect must be clamped down"
        );
        assert_eq!(
            descriptor.network_scope(),
            &NetworkScope::Hosts {
                allowed: ["api.example.com".into()].into_iter().collect()
            },
            "network meet keeps the tighter scope"
        );
        assert_eq!(
            descriptor.filesystem_scope(),
            &FilesystemScope::WorktreeRead,
            "glob intersect collapses to WorktreeRead when nothing survives"
        );
        assert!(
            descriptor.approval_policy() >= ceiling.minimum_approval,
            "approval friction must be raised, never lowered"
        );
    }

    #[test]
    fn restrict_is_monotone_over_generated_proposal_ceiling_pairs() {
        // §8 PR1 test 2: property-tested over generated combinations. A
        // deterministic sweep over every proposal axis × every ceiling axis
        // (2^4 × 2^4 = 256 pairs) asserts the four monotone invariants.
        let side_effects = [
            SideEffectClass::Read,
            SideEffectClass::Write,
            SideEffectClass::Execute,
            SideEffectClass::Destructive,
        ];
        let networks = [
            NetworkScope::None,
            NetworkScope::Hosts {
                allowed: ["api.example.com".into()].into_iter().collect(),
            },
            NetworkScope::Any,
        ];
        let filesystems = [
            FilesystemScope::None,
            FilesystemScope::WorktreeRead,
            FilesystemScope::Worktree {
                write_globs: vec!["src/**".into()],
                maximum_changed_files: 3,
            },
            FilesystemScope::maximum(),
        ];
        let approvals = [
            ApprovalPolicy::PreAuthorized,
            ApprovalPolicy::ByClass,
            ApprovalPolicy::AlwaysAsk,
            ApprovalPolicy::Forbidden,
        ];

        for &proposed_side in &side_effects {
            for proposed_net in &networks {
                for proposed_fs in &filesystems {
                    for &proposed_approval in &approvals {
                        for &ceiling_side in &side_effects {
                            for ceiling_net in &networks {
                                for ceiling_fs in &filesystems {
                                    for &ceiling_approval in &approvals {
                                        let ceiling = ToolCeiling {
                                            maximum_side_effect: ceiling_side,
                                            maximum_network: ceiling_net.clone(),
                                            maximum_filesystem: ceiling_fs.clone(),
                                            minimum_approval: ceiling_approval,
                                            denied_tool_ids: BTreeSet::from([
                                                "native:blocked".into()
                                            ]),
                                        };
                                        let proposal = ToolDescriptorProposal {
                                            id: ToolId::native("probe"),
                                            provider: ToolProvider::Native,
                                            display_name: "probe".into(),
                                            description: "probe".into(),
                                            schema: serde_json::json!({ "type": "object" }),
                                            capabilities: BTreeSet::new(),
                                            side_effect_class: proposed_side,
                                            network_scope: proposed_net.clone(),
                                            filesystem_scope: proposed_fs.clone(),
                                            approval_policy: proposed_approval,
                                            origin: DescriptorOrigin::Builtin,
                                        };
                                        let (descriptor, _) = proposal.restrict(&ceiling);
                                        assert!(
                                            descriptor.side_effect_class()
                                                <= ceiling.maximum_side_effect,
                                            "side_effect must never exceed the ceiling"
                                        );
                                        assert!(
                                            filesystem_contained(
                                                descriptor.filesystem_scope(),
                                                &ceiling.maximum_filesystem,
                                            ),
                                            "filesystem must never exceed the ceiling"
                                        );
                                        assert!(
                                            descriptor.approval_policy()
                                                >= ceiling.minimum_approval,
                                            "approval must never drop below the ceiling minimum"
                                        );
                                        assert!(
                                            network_contained(
                                                descriptor.network_scope(),
                                                &ceiling.maximum_network,
                                            ),
                                            "network must never exceed the ceiling"
                                        );
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    /// `FilesystemScope` containment: `a` is no wider than `b`.
    fn filesystem_contained(a: &FilesystemScope, b: &FilesystemScope) -> bool {
        use FilesystemScope::*;
        match (a, b) {
            (None, _) => true,
            (WorktreeRead, WorktreeRead) | (WorktreeRead, Worktree { .. }) => true,
            (Worktree { .. }, WorktreeRead) | (Worktree { .. }, None) => false,
            (Worktree { .. }, Worktree { .. }) => true,
            (WorktreeRead, None) => false,
        }
    }

    /// `NetworkScope` containment: `a` is no wider than `b`.
    fn network_contained(a: &NetworkScope, b: &NetworkScope) -> bool {
        use NetworkScope::*;
        match (a, b) {
            (None, _) => true,
            (Hosts { .. }, Any) | (Hosts { .. }, Hosts { .. }) => true,
            (Hosts { .. }, None) => false,
            (Any, Any) => true,
            (Any, _) => false,
        }
    }

    #[test]
    fn denied_tool_id_is_forbidden_regardless_of_other_fields() {
        let ceiling = ToolCeiling {
            denied_tool_ids: BTreeSet::from(["native:secret-leak".into()]),
            ..permissive_ceiling()
        };
        let proposal = ToolDescriptorProposal {
            approval_policy: ApprovalPolicy::PreAuthorized,
            ..read_proposal("secret-leak")
        };
        let (descriptor, diagnostics) = proposal.restrict(&ceiling);
        assert_eq!(descriptor.approval_policy(), ApprovalPolicy::Forbidden);
        assert!(
            diagnostics
                .iter()
                .any(|d| d.severity == DiagnosticSeverity::Rejected),
            "a denied id is Rejected, not merely Restricted"
        );
    }

    #[test]
    fn project_layer_preauthorized_is_raised_to_minimum_and_diagnosed() {
        let ceiling = ToolCeiling {
            minimum_approval: ApprovalPolicy::ByClass,
            ..permissive_ceiling()
        };
        let proposal = ToolDescriptorProposal {
            approval_policy: ApprovalPolicy::PreAuthorized,
            origin: DescriptorOrigin::Project,
            ..read_proposal("project_thing")
        };
        let (descriptor, diagnostics) = proposal.restrict(&ceiling);
        assert_eq!(descriptor.approval_policy(), ApprovalPolicy::ByClass);
        let restricted = diagnostics
            .iter()
            .find(|d| d.severity == DiagnosticSeverity::Restricted)
            .expect("one Restricted diagnostic");
        assert_eq!(restricted.restricted_fields[0].0, "approval_policy");
        assert_eq!(restricted.restricted_fields[0].1, "PreAuthorized");
        assert_eq!(restricted.restricted_fields[0].2, "ByClass");
    }

    #[test]
    fn digest_changes_when_any_field_changes() {
        let ceiling = permissive_ceiling();
        let a = admit(read_proposal("read_file"), &ceiling);
        let b = admit(
            ToolDescriptorProposal {
                description: "different description".into(),
                ..read_proposal("read_file")
            },
            &ceiling,
        );
        assert_ne!(a.descriptor_digest(), b.descriptor_digest());
    }

    #[test]
    fn digest_is_stable_under_literal_field_reordering() {
        let ceiling = permissive_ceiling();
        // Canonical serialization: the digest must depend only on the field
        // VALUES, never on the order the struct literal was written. Two
        // descriptors with identical values built through different literal
        // orderings must hash identically.
        let a = admit(
            ToolDescriptorProposal {
                id: ToolId::native("stable"),
                display_name: "stable".into(),
                description: "desc".into(),
                ..read_proposal("stable")
            },
            &ceiling,
        );
        let b = admit(
            ToolDescriptorProposal {
                description: "desc".into(),
                id: ToolId::native("stable"),
                display_name: "stable".into(),
                ..read_proposal("stable")
            },
            &ceiling,
        );
        assert_eq!(a.descriptor_digest(), b.descriptor_digest());
        assert_eq!(
            serde_json::to_vec(&a).unwrap(),
            serde_json::to_vec(&b).unwrap(),
            "serde field order is declaration order, so equal values serialize identically"
        );
    }

    #[test]
    fn deserialize_rejects_forged_digest() {
        // The read-back path must refuse a descriptor whose stored digest was
        // not computed over the carried fields (§9.2 structural invariant).
        let ceiling = permissive_ceiling();
        let descriptor = admit(read_proposal("read_file"), &ceiling);
        let mut json = serde_json::to_value(&descriptor).unwrap();

        // Tamper with a field; the stored digest no longer matches.
        json["description"] = serde_json::json!("tampered");
        let result: Result<ToolDescriptor, _> = serde_json::from_value(json);
        assert!(
            result.is_err(),
            "a forged descriptor must fail to deserialize"
        );

        // An untouched round-trip succeeds.
        let clean = serde_json::to_value(&descriptor).unwrap();
        let back: ToolDescriptor = serde_json::from_value(clean).unwrap();
        assert_eq!(back, descriptor);
    }

    #[test]
    fn network_hosts_meet_intersects_allowed_hosts() {
        let hosts = |h: &[&str]| NetworkScope::Hosts {
            allowed: h.iter().map(|s| s.to_string()).collect(),
        };
        let a = hosts(&["a.example.com", "b.example.com"]);
        let b = hosts(&["b.example.com", "c.example.com"]);
        let meet = a.meet(&b);
        assert_eq!(meet, hosts(&["b.example.com"]));
        assert_eq!(a.meet(&NetworkScope::None), NetworkScope::None);
        assert_eq!(NetworkScope::Any.meet(&a), a);
        assert_eq!(
            NetworkScope::Any.meet(&NetworkScope::Any),
            NetworkScope::Any
        );
    }

    #[test]
    fn filesystem_intersect_is_conservative() {
        let w = |globs: &[&str], n: usize| FilesystemScope::Worktree {
            write_globs: globs.iter().map(|s| s.to_string()).collect(),
            maximum_changed_files: n,
        };
        assert_eq!(
            w(&["src/**", "tests/**"], 5).intersect(&w(&["tests/**", "docs/**"], 3)),
            w(&["tests/**"], 3)
        );
        assert_eq!(
            w(&["src/**"], 5).intersect(&FilesystemScope::WorktreeRead),
            FilesystemScope::WorktreeRead
        );
        assert_eq!(
            FilesystemScope::None.intersect(&w(&["src/**"], 5)),
            FilesystemScope::None
        );
    }

    #[test]
    fn ceiling_meet_is_commutative_associative_and_only_shrinks() {
        // The v1.4 §5.3 fold relies on all three properties: without them,
        // `workspace ∩ parent ∩ profile` would depend on the order the terms
        // happened to be written in.
        let a = ToolCeiling {
            maximum_side_effect: SideEffectClass::Write,
            maximum_network: NetworkScope::Any,
            maximum_filesystem: FilesystemScope::maximum(),
            minimum_approval: ApprovalPolicy::ByClass,
            denied_tool_ids: BTreeSet::from(["native:a".to_string()]),
        };
        let b = ToolCeiling {
            maximum_side_effect: SideEffectClass::Destructive,
            maximum_network: NetworkScope::None,
            maximum_filesystem: FilesystemScope::WorktreeRead,
            minimum_approval: ApprovalPolicy::AlwaysAsk,
            denied_tool_ids: BTreeSet::from(["native:b".to_string()]),
        };
        let c = ToolCeiling {
            maximum_side_effect: SideEffectClass::Read,
            ..permissive_ceiling()
        };

        assert_eq!(a.meet(&b), b.meet(&a), "meet must be commutative");
        assert_eq!(
            a.meet(&b).meet(&c),
            a.meet(&b.meet(&c)),
            "meet must be associative"
        );
        assert_eq!(a.meet(&a), a, "meet must be idempotent");

        let met = a.meet(&b);
        assert!(met.is_within(&a) && met.is_within(&b));
        // Deny sets union: a tool either side denied stays denied.
        assert!(met.denied_tool_ids.contains("native:a"));
        assert!(met.denied_tool_ids.contains("native:b"));
        // Friction only rises.
        assert_eq!(met.minimum_approval, ApprovalPolicy::AlwaysAsk);
        // `nothing()` is the absorbing element.
        assert!(
            ToolCeiling::nothing()
                .meet(&a)
                .is_within(&ToolCeiling::nothing())
        );
        assert_eq!(ToolCeiling::maximum().meet(&a), a);
    }

    #[test]
    fn glob_coverage_is_conservative_but_handles_directory_prefixes() {
        assert!(glob_covers("**", "src/auth/**"));
        assert!(glob_covers("src/**", "src/auth/**"));
        assert!(glob_covers("src/**", "src/auth/token.rs"));
        assert!(glob_covers("src/auth/**", "src/auth/token.rs"));
        assert!(glob_covers("src/auth/token.rs", "src/auth/token.rs"));
        // Not covered: a sibling directory, a parent directory, or a prefix
        // that only matches as a string rather than as a path segment.
        assert!(!glob_covers("src/auth/**", "src/payments/billing.rs"));
        assert!(!glob_covers("src/auth/**", "src/**"));
        assert!(!glob_covers("src/**", "src"));
        assert!(!glob_covers("src/a*.rs", "src/auth.rs"));
    }

    #[test]
    fn narrow_to_paths_narrows_where_intersect_would_wrongly_empty() {
        let ceiling = FilesystemScope::maximum();
        // Exact-string intersect treats `**` and `src/auth/**` as disjoint…
        assert_eq!(
            ceiling.intersect(&FilesystemScope::Worktree {
                write_globs: vec!["src/auth/**".into()],
                maximum_changed_files: 10,
            }),
            FilesystemScope::WorktreeRead
        );
        // …but narrowing keeps the requested subset, which is what a delegated
        // path scope means.
        let narrowed = ceiling.narrow_to_paths(["src/auth/**"]);
        assert_eq!(narrowed.write_globs(), ["src/auth/**"]);
        assert!(narrowed.is_within(&ceiling));

        // A request the ceiling does not cover is dropped, never granted.
        let bounded = FilesystemScope::Worktree {
            write_globs: vec!["src/auth/**".into()],
            maximum_changed_files: 10,
        };
        assert_eq!(
            bounded.narrow_to_paths(["src/payments/**"]),
            FilesystemScope::WorktreeRead,
            "an uncovered request must survive as no write authority at all"
        );
        assert_eq!(
            bounded
                .narrow_to_paths(["src/auth/token.rs", "src/payments/**"])
                .write_globs(),
            ["src/auth/token.rs"],
        );
        // Read-only stays read-only whatever is requested.
        assert_eq!(
            FilesystemScope::WorktreeRead.narrow_to_paths(["src/**"]),
            FilesystemScope::WorktreeRead
        );
    }

    #[test]
    fn filesystem_containment_is_a_real_predicate() {
        let wide = FilesystemScope::maximum();
        let narrow = FilesystemScope::Worktree {
            write_globs: vec!["src/auth/**".into()],
            maximum_changed_files: 5,
        };
        assert!(narrow.is_within(&wide));
        assert!(!wide.is_within(&narrow));
        assert!(FilesystemScope::WorktreeRead.is_within(&narrow));
        assert!(!narrow.is_within(&FilesystemScope::WorktreeRead));
        assert!(FilesystemScope::None.is_within(&FilesystemScope::None));
        // A larger changed-file budget is wider even with the same globs.
        let looser = FilesystemScope::Worktree {
            write_globs: vec!["src/auth/**".into()],
            maximum_changed_files: 50,
        };
        assert!(narrow.is_within(&looser));
        assert!(!looser.is_within(&narrow));
    }

    #[test]
    fn tool_id_namespacing() {
        assert_eq!(ToolId::native("read_file").provider(), ToolProvider::Native);
        assert_eq!(
            ToolId::mcp("github", "create_issue").provider(),
            ToolProvider::Mcp
        );
        assert_eq!(
            ToolId::skill("rust-review", "lint").provider(),
            ToolProvider::Skill
        );
        assert_eq!(ToolId::native("x").as_str(), "native:x");
        assert_eq!(ToolId::mcp("gh", "i").as_str(), "mcp:gh/i");
        assert_eq!(ToolId::skill("s", "t").as_str(), "skill:s/t");
    }

    #[test]
    fn canonical_serialization_is_deterministic() {
        let ceiling = permissive_ceiling();
        let a = admit(read_proposal("canon"), &ceiling);
        let b = admit(read_proposal("canon"), &ceiling);
        assert_eq!(a, b);
        assert_eq!(a.descriptor_digest(), b.descriptor_digest());

        // serde output itself is deterministic (BTreeMap-backed, no preserve_order).
        let bytes_a = serde_json::to_vec(&a).unwrap();
        let bytes_b = serde_json::to_vec(&b).unwrap();
        assert_eq!(bytes_a, bytes_b);
    }

    #[test]
    fn capabilities_round_trip_through_proposal() {
        let mut capabilities = BTreeSet::new();
        capabilities.insert(capability("code_review"));
        capabilities.insert(capability("rust"));
        let ceiling = permissive_ceiling();
        let descriptor = admit(
            ToolDescriptorProposal {
                capabilities: capabilities.clone(),
                ..read_proposal("review")
            },
            &ceiling,
        );
        assert_eq!(descriptor.capabilities(), &capabilities);
    }

    #[test]
    fn serialized_descriptor_deserializes_back_with_private_fields() {
        let ceiling = permissive_ceiling();
        let descriptor = admit(read_proposal("roundtrip"), &ceiling);
        let bytes = serde_json::to_vec(&descriptor).unwrap();
        let back: ToolDescriptor = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(back, descriptor);
        assert_eq!(back.descriptor_digest(), descriptor.descriptor_digest());
    }

    #[test]
    fn example_from_architecture_doc_compiles() {
        // Smoke: the §4.1 shape (ToolId constructors, provider) is usable.
        let _map: BTreeMap<ToolId, ToolDescriptor> = BTreeMap::new();
        let _invocation = ToolInvocation {
            tool_id: ToolId::native("read_file"),
            arguments: serde_json::json!({}),
            working_directory: PathBuf::from("/repo"),
            descriptor_digest: "abc".into(),
        };
    }
}
