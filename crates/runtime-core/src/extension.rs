//! User-authored extension descriptors (v1.3 §4.3): agents, skills, commands,
//! hooks.
//!
//! Everything in this module is a *claim*. [`AgentProfile`] is untrusted YAML
//! a user writes in `.purrcode/agents/`; it becomes an [`AgentDescriptor`]
//! only through `CapabilityRegistry::admit_agent`, which applies the workspace
//! [`crate::ToolCeiling`]. The same no-public-constructor rule that governs
//! [`crate::ToolDescriptor`] governs agents: a descriptor that was not
//! restricted cannot be constructed.

use super::capability::{AdmissionDiagnostic, CapabilityId, DiagnosticSeverity, ExtensionLayer};
use super::tool::{
    DescriptorOrigin, FilesystemScope, NetworkScope, SideEffectClass, ToolCeiling, ToolDescriptor,
    ToolId,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

/// A configured model role, reconciled across the role vocabularies
/// (provider-gateway `canonical_model_role` vs `model-selection::ModelRole`).
///
/// The architecture doc places this in `model-selection` (PR4), but
/// runtime-core cannot depend on model-selection, so the vocabulary lives here
/// and PR4 makes model-selection's `ModelRole` map onto it.
#[derive(Clone, Debug, Eq, Hash, JsonSchema, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct ModelRoleName(String);

impl<'de> Deserialize<'de> for ModelRoleName {
    /// A role that is not recognized is a hard deserialization error, NOT a
    /// silent `coding_worker` fallback. The v1.2 `model_for` fallback is the
    /// defect PR4 closes; a profile that names an unknown role must fail at
    /// load, where the diagnostic is visible.
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        Self::parse(&raw).map_err(serde::de::Error::custom)
    }
}

impl ModelRoleName {
    /// Parses any of the accepted role spellings. Legacy aliases
    /// (`fast_router`, `scout`) are accepted and canonicalized.
    pub fn parse(raw: &str) -> Result<Self, super::DomainError> {
        let canonical = match raw {
            "coding_worker" => "coding_worker",
            "planner" => "planner",
            "judge" => "judge",
            "summarizer" => "summarizer",
            "embedding" => "embedding",
            "router" | "fast_router" => "fast_router",
            "utility" => "utility",
            "reviewer" => "reviewer",
            "scout" => "scout",
            "" => {
                return Err(super::DomainError::InvalidBounds {
                    reason: "model role must not be empty".into(),
                });
            }
            other => {
                // Unknown roles are rejected loudly rather than silently
                // falling back to coding_worker — the v1.2 fallback in
                // `model_for` is exactly the defect PR4 closes.
                return Err(super::DomainError::InvalidBounds {
                    reason: format!("`{other}` is not a recognized model role"),
                });
            }
        };
        Ok(Self(canonical.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ModelRoleName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Byte cap for a profile-supplied system prompt. Mirrors the project-context
/// instruction cap spirit: a repository file must not be able to inject an
/// unbounded prompt.
pub const MAX_SYSTEM_PROMPT_BYTES: usize = 256 * 1024;

/// What a user writes in `.purrcode/agents/<name>.yaml`. Untrusted input.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AgentProfile {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub capabilities: BTreeSet<CapabilityId>,
    /// Which configured model role this agent's `coding_worker` route resolves
    /// to. Validated against the reconciled ModelRole vocabulary, NOT silently
    /// fallen back to `coding_worker` as `model_for` does today.
    #[serde(default)]
    pub model_role: Option<ModelRoleName>,
    /// Replaces `DEFAULT_DEVELOPER_INSTRUCTIONS`
    /// (agent-runtime/src/context.rs:788) for this agent. Byte-capped at load;
    /// gets its own ContextLedgerSection.
    #[serde(default)]
    pub system_prompt: Option<String>,
    #[serde(default)]
    pub tools: ToolPolicy,
    #[serde(default)]
    pub permissions: PermissionRequest,
    #[serde(default)]
    pub context: ContextPolicy,
    #[serde(default)]
    pub skills: SkillPolicy,
    #[serde(default)]
    pub priority: i32,
    /// Trust tier, set by the loader (extension-config) from the directory the
    /// file came from. Skipped from YAML: a user cannot declare their own tier.
    #[serde(default, skip)]
    pub layer: ExtensionLayer,
}

/// The admitted, restricted form. Same no-public-constructor rule as
/// `ToolDescriptor`: only `CapabilityRegistry::admit_agent` produces one.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AgentDescriptor {
    name: String,
    layer: ExtensionLayer,
    description: String,
    capabilities: BTreeSet<CapabilityId>,
    model_role: Option<ModelRoleName>,
    system_prompt: Option<String>,
    /// Post-restriction. Explicit tri-state — there is deliberately no
    /// "empty set means everything" case (see [`ToolSelection`]).
    tool_selection: ToolSelection,
    /// Post-restriction. This is what PawGate is handed, not `permissions`.
    ceiling: ToolCeiling,
    context: ContextPolicy,
    allowed_skills: BTreeSet<String>,
    priority: i32,
    descriptor_digest: String,
}

impl AgentDescriptor {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn layer(&self) -> ExtensionLayer {
        self.layer
    }

    pub fn description(&self) -> &str {
        &self.description
    }

    pub fn capabilities(&self) -> &BTreeSet<CapabilityId> {
        &self.capabilities
    }

    pub fn model_role(&self) -> Option<&ModelRoleName> {
        self.model_role.as_ref()
    }

    pub fn system_prompt(&self) -> Option<&str> {
        self.system_prompt.as_deref()
    }

    pub fn tool_selection(&self) -> &ToolSelection {
        &self.tool_selection
    }

    pub fn ceiling(&self) -> &ToolCeiling {
        &self.ceiling
    }

    pub fn context(&self) -> &ContextPolicy {
        &self.context
    }

    pub fn allowed_skills(&self) -> &BTreeSet<String> {
        &self.allowed_skills
    }

    pub fn priority(&self) -> i32 {
        self.priority
    }

    pub fn descriptor_digest(&self) -> &str {
        &self.descriptor_digest
    }
}

impl Default for AgentDescriptor {
    /// The BUILT-IN main agent, used when a session runs with no named profile.
    /// Its tool selection is [`ToolSelection::AllAdmitted`]: the built-in agent
    /// is bounded by the workspace policy itself, which is exactly the ceiling
    /// every descriptor in the registry was already admitted against.
    ///
    /// This variant is unreachable from YAML — `AgentProfile::restrict` never
    /// produces it — so a user-authored profile can never widen itself to
    /// "every registered tool" by leaving `tools:` blank.
    fn default() -> Self {
        let ceiling = ToolCeiling {
            maximum_side_effect: SideEffectClass::Read,
            maximum_network: NetworkScope::None,
            maximum_filesystem: FilesystemScope::WorktreeRead,
            minimum_approval: super::tool::ApprovalPolicy::ByClass,
            denied_tool_ids: BTreeSet::new(),
        };
        let mut descriptor = AgentDescriptor {
            name: "main".into(),
            layer: ExtensionLayer::Builtin,
            description: String::new(),
            capabilities: BTreeSet::new(),
            model_role: None,
            system_prompt: None,
            tool_selection: ToolSelection::AllAdmitted,
            ceiling,
            context: ContextPolicy::default(),
            allowed_skills: BTreeSet::new(),
            priority: 0,
            descriptor_digest: String::new(),
        };
        descriptor.descriptor_digest = blake3::hash(&serde_json::to_vec(&descriptor).unwrap())
            .to_hex()
            .to_string();
        descriptor
    }
}

impl AgentProfile {
    /// The restriction lattice for an agent, applied on admission.
    ///
    /// `permissions` is a REQUEST, never a grant: every field can only lower
    /// the effective value relative to the workspace ceiling. Capability axes
    /// intersect (min); the approval axis takes the max (friction only grows).
    pub fn restrict(self, ceiling: &ToolCeiling) -> (AgentDescriptor, Vec<AdmissionDiagnostic>) {
        let mut diagnostics = Vec::new();

        // ── Filesystem ceiling ──
        let requested_fs = match self.permissions.write {
            Some(true) => FilesystemScope::maximum(),
            Some(false) => FilesystemScope::WorktreeRead,
            None => FilesystemScope::maximum(),
        };
        let effective_fs = ceiling.maximum_filesystem.intersect(&requested_fs);
        if effective_fs != requested_fs {
            diagnostics.push(AdmissionDiagnostic {
                source_path: None,
                subject: self.name.clone(),
                severity: DiagnosticSeverity::Restricted,
                message: "filesystem permission request clamped to the workspace ceiling".into(),
                restricted_fields: vec![(
                    "permissions.write".into(),
                    format!("{:?}", self.permissions.write),
                    format!("{effective_fs:?}"),
                )],
            });
        }

        // ── Network ceiling ──
        let requested_net = self
            .permissions
            .network
            .clone()
            .unwrap_or(NetworkScope::Any);
        let effective_net = ceiling.maximum_network.meet(&requested_net);
        if effective_net != requested_net {
            diagnostics.push(AdmissionDiagnostic {
                source_path: None,
                subject: self.name.clone(),
                severity: DiagnosticSeverity::Restricted,
                message: "network permission request clamped to the workspace ceiling".into(),
                restricted_fields: vec![(
                    "permissions.network".into(),
                    format!("{requested_net:?}"),
                    format!("{effective_net:?}"),
                )],
            });
        }

        // ── Side-effect ceiling ──
        let requested_side = self
            .permissions
            .maximum_side_effect
            .unwrap_or(SideEffectClass::Destructive);
        let effective_side = ceiling.maximum_side_effect.min(requested_side);
        if effective_side != requested_side {
            diagnostics.push(AdmissionDiagnostic {
                source_path: None,
                subject: self.name.clone(),
                severity: DiagnosticSeverity::Restricted,
                message: "maximum_side_effect request clamped to the workspace ceiling".into(),
                restricted_fields: vec![(
                    "permissions.maximum_side_effect".into(),
                    format!("{requested_side:?}"),
                    format!("{effective_side:?}"),
                )],
            });
        }

        // ── Approval ceiling: friction can only grow ──
        let requested_approval = self
            .permissions
            .approval
            .unwrap_or(super::tool::ApprovalPolicy::PreAuthorized);
        let effective_approval = ceiling.minimum_approval.max(requested_approval);
        if effective_approval != requested_approval {
            diagnostics.push(AdmissionDiagnostic {
                source_path: None,
                subject: self.name.clone(),
                severity: DiagnosticSeverity::Restricted,
                message: "approval request raised to the workspace minimum friction".into(),
                restricted_fields: vec![(
                    "permissions.approval".into(),
                    format!("{requested_approval:?}"),
                    format!("{effective_approval:?}"),
                )],
            });
        }

        // ── Tool selection: an explicit tri-state, NOT a set that means
        // "everything" when empty. Patterns (literal ids as well as globs) are
        // kept verbatim and resolved against the admitted registry by
        // `CapabilityRegistry::for_agent`, so `allow: [native:*]` selects the
        // native namespace and nothing else — never "every registered tool".
        //
        // Deny always beats allow, and the workspace ceiling's deny list is
        // folded in here as well so a profile cannot re-admit a workspace-denied
        // id by naming it. ──
        let mut validate = |patterns: &[String], field: &str| -> Vec<String> {
            patterns
                .iter()
                .filter(|pattern| {
                    if ToolPattern::is_valid(pattern) {
                        true
                    } else {
                        diagnostics.push(AdmissionDiagnostic {
                            source_path: None,
                            subject: self.name.clone(),
                            severity: DiagnosticSeverity::Rejected,
                            message: format!(
                                "`{field}` entry `{pattern}` is not a valid tool id or glob and was dropped"
                            ),
                            restricted_fields: vec![(
                                format!("tools.{field}"),
                                pattern.to_string(),
                                "dropped".into(),
                            )],
                        });
                        false
                    }
                })
                .cloned()
                .collect()
        };
        let deny: Vec<String> = validate(&self.tools.deny, "deny")
            .into_iter()
            .chain(ceiling.denied_tool_ids.iter().cloned())
            .collect();
        let allow = validate(&self.tools.allow, "allow");
        let tool_selection = if allow.is_empty() {
            ToolSelection::InheritSafeDefaults { deny }
        } else {
            ToolSelection::Explicit { allow, deny }
        };

        // ── Allowed skills: deny beats allow. ──
        let denied_skills: BTreeSet<&str> = self.skills.deny.iter().map(String::as_str).collect();
        let allowed_skills: BTreeSet<String> = self
            .skills
            .allow
            .into_iter()
            .filter(|skill| !denied_skills.contains(skill.as_str()))
            .collect();

        // ── System prompt byte cap ──
        let system_prompt = self.system_prompt.filter(|p| {
            if p.len() > MAX_SYSTEM_PROMPT_BYTES {
                diagnostics.push(AdmissionDiagnostic {
                    source_path: None,
                    subject: self.name.clone(),
                    severity: DiagnosticSeverity::Restricted,
                    message: "system_prompt exceeds the byte cap and was dropped".into(),
                    restricted_fields: vec![(
                        "system_prompt".into(),
                        format!("{} bytes", p.len()),
                        format!("{MAX_SYSTEM_PROMPT_BYTES} bytes max"),
                    )],
                });
                false
            } else {
                true
            }
        });

        let effective_ceiling = ToolCeiling {
            maximum_side_effect: effective_side,
            maximum_network: effective_net,
            maximum_filesystem: effective_fs,
            minimum_approval: effective_approval,
            denied_tool_ids: ceiling.denied_tool_ids.clone(),
        };

        let mut descriptor = AgentDescriptor {
            name: self.name,
            layer: self.layer,
            description: self.description,
            capabilities: self.capabilities,
            model_role: self.model_role,
            system_prompt,
            tool_selection,
            ceiling: effective_ceiling,
            context: self.context,
            allowed_skills,
            priority: self.priority,
            descriptor_digest: String::new(),
        };
        descriptor.descriptor_digest = blake3::hash(&serde_json::to_vec(&descriptor).unwrap())
            .to_hex()
            .to_string();
        (descriptor, diagnostics)
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct ToolPolicy {
    /// Glob patterns over ToolId strings. Omitting `allow` selects the
    /// workspace SAFE DEFAULT (built-in, read-only, no-network native tools) —
    /// it does NOT select every registered tool. A written `allow` selects
    /// exactly the admitted tools matching it. Either way the result is
    /// INTERSECTED with the ceiling — never unioned.
    #[serde(default)]
    pub allow: Vec<String>,
    /// Always applied, always wins over `allow`. Deny beats trust — the same
    /// ordering `McpServerConfig::denies`/`trusts` uses.
    #[serde(default)]
    pub deny: Vec<String>,
}

/// How an agent profile selects tools out of the admitted registry.
///
/// The v1.2 shape was a single `BTreeSet<ToolId>` whose EMPTY value was read as
/// "admit everything" by both the model manifest and normalization. That made
/// `allow: [native:*]` — a pattern the old restriction discarded — silently
/// mean *more* capability than writing nothing at all: native **plus** MCP plus
/// every future provider. This enum removes the ambiguity: the three cases are
/// distinct values, and patterns are carried through to be resolved against the
/// registry rather than dropped.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ToolSelection {
    /// The BUILT-IN main agent: every admitted tool, bounded by the workspace
    /// ceiling the registry already applied. `AgentProfile::restrict` never
    /// produces this, so no user-authored file can reach it.
    AllAdmitted,
    /// `tools.allow` was omitted. The safe default — built-in, read-only,
    /// no-network, non-writing native tools — minus `deny`.
    InheritSafeDefaults { deny: Vec<String> },
    /// `tools.allow` was written. Exactly the admitted tools matching one of
    /// `allow`, minus `deny`.
    Explicit {
        allow: Vec<String>,
        deny: Vec<String>,
    },
}

impl ToolSelection {
    /// Whether this selection admits an already-admitted descriptor. Deny is
    /// evaluated first and always wins.
    pub fn admits(&self, descriptor: &ToolDescriptor) -> bool {
        let id = descriptor.id().as_str();
        match self {
            ToolSelection::AllAdmitted => true,
            ToolSelection::InheritSafeDefaults { deny } => {
                !matches_any(deny, id) && is_safe_default(descriptor)
            }
            ToolSelection::Explicit { allow, deny } => {
                !matches_any(deny, id) && matches_any(allow, id)
            }
        }
    }

    /// The patterns as written, for `GET /v1/agents` and diagnostics.
    pub fn describe(&self) -> serde_json::Value {
        match self {
            ToolSelection::AllAdmitted => serde_json::json!({ "kind": "all_admitted" }),
            ToolSelection::InheritSafeDefaults { deny } => serde_json::json!({
                "kind": "inherit_safe_defaults",
                "deny": deny,
            }),
            ToolSelection::Explicit { allow, deny } => serde_json::json!({
                "kind": "explicit",
                "allow": allow,
                "deny": deny,
            }),
        }
    }
}

/// The workspace SAFE DEFAULT: what an agent gets when it writes no `allow`.
///
/// Built-in origin (compiled in, not authored by a repository or a remote
/// server), observes only, reaches no network, and cannot write the worktree.
/// Deliberately conservative: omitting `tools:` must never hand an agent an MCP
/// server, a skill script, or `native:command`.
pub fn is_safe_default(descriptor: &ToolDescriptor) -> bool {
    descriptor.origin() == DescriptorOrigin::Builtin
        && descriptor.side_effect_class() == SideEffectClass::Read
        && descriptor.network_scope() == &NetworkScope::None
        && !matches!(
            descriptor.filesystem_scope(),
            FilesystemScope::Worktree { .. }
        )
}

fn matches_any(patterns: &[String], id: &str) -> bool {
    patterns
        .iter()
        .any(|pattern| ToolPattern::matches(pattern, id))
}

/// A glob over tool-id strings. `*` deliberately spans `/` so `mcp:*` covers
/// `mcp:<server>/<tool>`; the id alphabet has no path semantics.
pub struct ToolPattern;

impl ToolPattern {
    /// A pattern is valid if it either parses as a tool id (a literal) or
    /// compiles as a glob. An entry that is neither is dropped with a
    /// diagnostic rather than silently matching nothing.
    pub fn is_valid(pattern: &str) -> bool {
        if pattern.is_empty() {
            return false;
        }
        if ToolId::parse(pattern).is_some() {
            return true;
        }
        // A glob must still be namespaced: `*` on its own would let a profile
        // re-widen itself to everything, which is the defect this type exists
        // to close.
        let namespaced = ["native:", "mcp:", "skill:"]
            .iter()
            .any(|prefix| pattern.starts_with(prefix));
        namespaced && Self::compile(pattern).is_some()
    }

    pub fn matches(pattern: &str, id: &str) -> bool {
        if pattern == id {
            return true;
        }
        Self::compile(pattern).is_some_and(|matcher| matcher.is_match(id))
    }

    fn compile(pattern: &str) -> Option<globset::GlobMatcher> {
        globset::GlobBuilder::new(pattern)
            .literal_separator(false)
            .build()
            .ok()
            .map(|glob| glob.compile_matcher())
    }
}

/// A REQUEST, not a grant. Every field can only lower the effective value.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct PermissionRequest {
    #[serde(default)]
    pub write: Option<bool>,
    #[serde(default)]
    pub network: Option<NetworkScope>,
    #[serde(default)]
    pub maximum_side_effect: Option<SideEffectClass>,
    /// May only INCREASE friction. `Some(PreAuthorized)` from a Project layer
    /// is rejected with a Restricted diagnostic.
    #[serde(default)]
    pub approval: Option<super::tool::ApprovalPolicy>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct ContextPolicy {
    #[serde(default)]
    pub auto: Option<bool>,
    #[serde(default)]
    pub project_memory: Option<bool>,
    /// Clamped with `min()` against the controls' maximum_input_tokens.
    #[serde(default)]
    pub maximum_input_tokens: Option<u64>,
    /// Auto-attached references, e.g. `["@diff"]`. Acceptance-test step 7.
    #[serde(default)]
    pub references: Vec<String>,
    #[serde(default)]
    pub graph_expansion: Option<bool>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct SkillPolicy {
    #[serde(default)]
    pub allow: Vec<String>,
    #[serde(default)]
    pub deny: Vec<String>,
}

/// Skills 2.0 — instructions + context requirements + allowed tools +
/// structured outputs + optional scripts + validation.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SkillDescriptor {
    pub skill_id: String,
    pub version: String,
    pub layer: ExtensionLayer,
    pub description: String,
    pub capabilities: BTreeSet<CapabilityId>,
    /// The SKILL.md body. Rendered as an UNTRUSTED-framed PinnedSection —
    /// this is what finally gives mcp-host/src/lib.rs:61 `instructions` a
    /// reader.
    pub instructions: String,
    /// What must be in context before the skill runs. Missing requirements are
    /// a hard error, not a silent no-op.
    #[serde(default)]
    pub requires_context: Vec<ContextRequirement>,
    /// Tool ids the skill may use. INTERSECTED with the invoking agent's set.
    #[serde(default)]
    pub allowed_tools: BTreeSet<ToolId>,
    /// JSON Schema of the structured result. Acceptance-test step 11.
    #[serde(default)]
    pub output_schema: Option<serde_json::Value>,
    /// Derived from the presence of scripts/ ON DISK (the digest already
    /// covers the tree) — NOT from a self-asserted boolean. A skill with no
    /// scripts is FORBIDDEN from declaring an entrypoint.
    #[serde(default)]
    pub entrypoints: BTreeMap<String, PathBuf>,
    #[serde(default)]
    pub validation: Option<SkillValidation>,
    pub content_digest: String,
    pub descriptor_digest: String,
    pub priority: i32,
}

impl SkillDescriptor {
    pub fn skill_id(&self) -> &str {
        &self.skill_id
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ContextRequirement {
    /// e.g. `@diff`
    Reference {
        spec: String,
    },
    Capability {
        id: CapabilityId,
    },
    Tool {
        id: ToolId,
    },
    ProjectMemory {
        memory_kind: String,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SkillValidation {
    /// Entrypoint key run in the Claw sandbox after the skill produces output.
    pub entrypoint: String,
    #[serde(default)]
    pub arguments: Vec<String>,
    #[serde(default = "default_validation_timeout")]
    pub timeout_seconds: u64,
    /// Real JSON Schema validation — NOT the top-level-key-presence check.
    #[serde(default)]
    pub expected_output_schema: Option<serde_json::Value>,
}

fn default_validation_timeout() -> u64 {
    60
}

/// Dynamic slash commands. Note the ABSENCE of a `path` field: a project file
/// can NEVER declare a daemon route (§2.3 / §10).
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CommandDescriptor {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub group: String,
    pub layer: ExtensionLayer,
    pub execution: CommandExecutionSpec,
    #[serde(default)]
    pub capabilities: BTreeSet<CapabilityId>,
    /// Auto-attached references. Previewed in the composer BEFORE send, so the
    /// green-chip honesty contract (project_context.rs:161-171) holds.
    #[serde(default)]
    pub references: Vec<String>,
    #[serde(default)]
    pub priority: i32,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CommandExecutionSpec {
    /// Built-in only. `path` is a &'static route owned by the daemon; project
    /// files cannot construct this variant (the loader rejects `kind: daemon`).
    Daemon {
        method: String,
        path: String,
    },
    Client,
    Prompt {
        prompt: String,
    },
    /// NEW. Runs `prompt` as the named agent. Published to clients as
    /// `Daemon { method: "POST", path: "/v1/sessions/{id}/commands/<name>" }`
    /// so v1.2 IDEs dispatch it correctly with zero client changes (§10).
    Agent {
        agent: String,
        prompt: String,
    },
}

/// Hooks are governed: Trigger -> PawGate -> Authorization -> Execution ->
/// Evidence.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct HookDescriptor {
    pub id: String,
    pub layer: ExtensionLayer,
    pub trigger: HookTrigger,
    /// Optional glob filter over the paths that fired the trigger.
    #[serde(default)]
    pub path_filter: Vec<String>,
    /// What the hook runs. A hook can only invoke a REGISTERED tool — it can
    /// never name an arbitrary program. This is what keeps it inside PawGate.
    pub action: HookAction,
    /// If true, a hook failure fails the turn. If false, it records evidence
    /// and continues. Default false — a hook must not be able to wedge a
    /// session.
    #[serde(default)]
    pub blocking: bool,
    #[serde(default = "default_hook_timeout")]
    pub timeout_seconds: u64,
    pub descriptor_digest: String,
}

fn default_hook_timeout() -> u64 {
    30
}

#[derive(
    Clone, Copy, Debug, Eq, JsonSchema, Ord, PartialEq, PartialOrd, Deserialize, Serialize,
)]
#[serde(rename_all = "snake_case")]
pub enum HookTrigger {
    BeforeWrite,
    AfterWrite,
    AfterAgentComplete,
    BeforeCommit,
    AfterValidation,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum HookAction {
    /// Invoke a registered tool with these arguments. Goes through
    /// `Policy::evaluate_tool` exactly like a model-proposed invocation.
    Tool {
        tool_id: ToolId,
        arguments: serde_json::Value,
    },
    /// Run a named capability (which resolves to an agent/skill/command).
    Capability { id: CapabilityId },
}

impl std::fmt::Display for HookTrigger {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = match self {
            HookTrigger::BeforeWrite => "before_write",
            HookTrigger::AfterWrite => "after_write",
            HookTrigger::AfterAgentComplete => "after_agent_complete",
            HookTrigger::BeforeCommit => "before_commit",
            HookTrigger::AfterValidation => "after_validation",
        };
        f.write_str(name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ApprovalPolicy;

    fn ceiling() -> ToolCeiling {
        ToolCeiling {
            maximum_side_effect: SideEffectClass::Destructive,
            maximum_network: NetworkScope::Any,
            maximum_filesystem: FilesystemScope::maximum(),
            minimum_approval: ApprovalPolicy::ByClass,
            denied_tool_ids: BTreeSet::new(),
        }
    }

    fn reviewer_profile() -> AgentProfile {
        AgentProfile {
            name: "reviewer".into(),
            description: "read-only reviewer".into(),
            capabilities: [capability("code_review")].into_iter().collect(),
            model_role: Some(ModelRoleName::parse("reviewer").unwrap()),
            system_prompt: None,
            tools: ToolPolicy {
                allow: vec!["native:read_file".into()],
                deny: vec![],
            },
            permissions: PermissionRequest {
                write: Some(false),
                network: Some(NetworkScope::None),
                maximum_side_effect: Some(SideEffectClass::Read),
                approval: None,
            },
            context: ContextPolicy::default(),
            skills: SkillPolicy::default(),
            priority: 0,
            layer: ExtensionLayer::Project,
        }
    }

    fn capability(raw: &str) -> CapabilityId {
        CapabilityId::parse(raw).unwrap()
    }

    #[test]
    fn write_request_is_clamped_to_ceiling() {
        // §9: a project agent requesting write against a WorktreeRead ceiling
        // is admitted WorktreeRead with one Restricted diagnostic.
        let ceiling = ToolCeiling {
            maximum_filesystem: FilesystemScope::WorktreeRead,
            ..ceiling()
        };
        let profile = AgentProfile {
            permissions: PermissionRequest {
                write: Some(true),
                ..PermissionRequest::default()
            },
            ..reviewer_profile()
        };
        let (descriptor, diagnostics) = profile.restrict(&ceiling);
        assert_eq!(
            descriptor.ceiling().maximum_filesystem,
            FilesystemScope::WorktreeRead
        );
        let restricted = diagnostics
            .iter()
            .find(|d| d.severity == DiagnosticSeverity::Restricted)
            .expect("one Restricted diagnostic");
        assert_eq!(restricted.restricted_fields[0].0, "permissions.write");
    }

    #[test]
    fn write_false_drops_filesystem_to_read() {
        let (descriptor, _) = reviewer_profile().restrict(&ceiling());
        assert_eq!(
            descriptor.ceiling().maximum_filesystem,
            FilesystemScope::WorktreeRead
        );
        // Read side-effect ceiling survives.
        assert_eq!(
            descriptor.ceiling().maximum_side_effect,
            SideEffectClass::Read
        );
    }

    #[test]
    fn approval_request_cannot_lower_friction() {
        // A Project agent asking for PreAuthorized against a ByClass minimum
        // is admitted ByClass, not PreAuthorized.
        let profile = AgentProfile {
            permissions: PermissionRequest {
                approval: Some(ApprovalPolicy::PreAuthorized),
                ..PermissionRequest::default()
            },
            ..reviewer_profile()
        };
        let (descriptor, diagnostics) = profile.restrict(&ceiling());
        assert_eq!(
            descriptor.ceiling().minimum_approval,
            ApprovalPolicy::ByClass
        );
        assert!(
            diagnostics.iter().any(|d| d
                .restricted_fields
                .iter()
                .any(|(f, ..)| f == "permissions.approval")),
            "the approval downgrade must be surfaced"
        );
    }

    #[test]
    fn ceiling_denied_ids_join_the_selection_deny_list() {
        let ceiling = ToolCeiling {
            denied_tool_ids: BTreeSet::from(["native:read_file".into()]),
            ..ceiling()
        };
        let (descriptor, _) = reviewer_profile().restrict(&ceiling);
        // The profile asked for `native:read_file`; the workspace denies it, so
        // the selection carries the deny and can never re-admit it.
        match descriptor.tool_selection() {
            ToolSelection::Explicit { allow, deny } => {
                assert_eq!(allow, &vec!["native:read_file".to_string()]);
                assert!(deny.contains(&"native:read_file".to_string()));
            }
            other => panic!("expected an explicit selection, got {other:?}"),
        }
    }

    #[test]
    fn glob_allow_entries_are_carried_not_discarded() {
        // v1.3 closure: a glob is a real selection, not a dropped entry. The
        // old behaviour discarded it, leaving an EMPTY allowlist that both the
        // manifest and normalization read as "admit everything" — the opposite
        // of what the author wrote.
        let profile = AgentProfile {
            tools: ToolPolicy {
                allow: vec!["native:read_file".into(), "native:*".into()],
                deny: vec![],
            },
            ..reviewer_profile()
        };
        let (descriptor, _) = profile.restrict(&ceiling());
        match descriptor.tool_selection() {
            ToolSelection::Explicit { allow, .. } => {
                assert_eq!(
                    allow,
                    &vec!["native:read_file".to_string(), "native:*".to_string()]
                );
            }
            other => panic!("expected an explicit selection, got {other:?}"),
        }
    }

    #[test]
    fn omitted_tools_inherits_safe_defaults_not_everything() {
        let profile = AgentProfile {
            tools: ToolPolicy::default(),
            ..reviewer_profile()
        };
        let (descriptor, _) = profile.restrict(&ceiling());
        assert!(matches!(
            descriptor.tool_selection(),
            ToolSelection::InheritSafeDefaults { .. }
        ));
    }

    #[test]
    fn a_bare_wildcard_is_rejected_with_a_diagnostic() {
        // `*` would let a profile re-widen itself to every provider, which is
        // exactly the semantic this type exists to remove.
        let profile = AgentProfile {
            tools: ToolPolicy {
                allow: vec!["*".into(), "not-a-namespace:thing".into()],
                deny: vec![],
            },
            ..reviewer_profile()
        };
        let (descriptor, diagnostics) = profile.restrict(&ceiling());
        assert!(
            matches!(
                descriptor.tool_selection(),
                ToolSelection::InheritSafeDefaults { .. }
            ),
            "every entry was invalid, so nothing was selected explicitly"
        );
        assert_eq!(
            diagnostics
                .iter()
                .filter(|d| d.severity == DiagnosticSeverity::Rejected)
                .count(),
            2,
            "both invalid entries must be surfaced, not silently dropped"
        );
    }

    #[test]
    fn tool_pattern_namespaces_are_not_cross_matched() {
        assert!(ToolPattern::matches("native:*", "native:read_file"));
        assert!(!ToolPattern::matches("native:*", "mcp:github/create_issue"));
        assert!(!ToolPattern::matches("native:*", "skill:rust-review/lint"));
        // `*` spans `/` so an mcp glob covers `server/tool`.
        assert!(ToolPattern::matches("mcp:*", "mcp:github/create_issue"));
        assert!(ToolPattern::matches(
            "mcp:github/*",
            "mcp:github/create_issue"
        ));
        assert!(!ToolPattern::matches(
            "mcp:github/*",
            "mcp:gitlab/create_issue"
        ));
        assert!(ToolPattern::is_valid("native:{read_file,list}"));
        assert!(!ToolPattern::is_valid("*"));
        assert!(!ToolPattern::is_valid(""));
    }

    #[test]
    fn skills_deny_beats_allow() {
        let profile = AgentProfile {
            skills: SkillPolicy {
                allow: vec!["rust-review".into(), "security-review".into()],
                deny: vec!["security-review".into()],
            },
            ..reviewer_profile()
        };
        let (descriptor, _) = profile.restrict(&ceiling());
        assert_eq!(
            descriptor.allowed_skills(),
            &BTreeSet::from(["rust-review".to_string()])
        );
    }

    #[test]
    fn system_prompt_over_cap_is_dropped_with_diagnostic() {
        let profile = AgentProfile {
            system_prompt: Some("x".repeat(MAX_SYSTEM_PROMPT_BYTES + 1)),
            ..reviewer_profile()
        };
        let (descriptor, diagnostics) = profile.restrict(&ceiling());
        assert_eq!(descriptor.system_prompt(), None);
        assert!(
            diagnostics
                .iter()
                .any(|d| d.message.contains("system_prompt")),
            "oversized prompt must be diagnosed"
        );
    }

    #[test]
    fn model_role_validation() {
        assert!(ModelRoleName::parse("reviewer").is_ok());
        assert!(ModelRoleName::parse("coding_worker").is_ok());
        assert_eq!(
            ModelRoleName::parse("fast_router").unwrap().as_str(),
            "fast_router"
        );
        assert!(ModelRoleName::parse("").is_err());
        assert!(ModelRoleName::parse("not-a-role").is_err());
    }

    #[test]
    fn builtin_default_agent_selects_every_admitted_tool() {
        // The BUILT-IN agent (no named profile) is bounded by the workspace
        // policy the registry already applied, so it selects everything. What
        // matters is that `restrict` cannot produce this variant — only the
        // Default impl can.
        let descriptor = AgentDescriptor::default();
        assert_eq!(descriptor.tool_selection(), &ToolSelection::AllAdmitted);
        assert!(descriptor.system_prompt().is_none());
        let (from_yaml, _) = reviewer_profile().restrict(&ceiling());
        assert_ne!(
            from_yaml.tool_selection(),
            &ToolSelection::AllAdmitted,
            "a YAML-authored profile can never reach AllAdmitted"
        );
    }

    #[test]
    fn descriptor_digest_changes_with_permissions() {
        let (a, _) = reviewer_profile().restrict(&ceiling());
        let (b, _) = AgentProfile {
            permissions: PermissionRequest {
                write: Some(true),
                ..PermissionRequest::default()
            },
            ..reviewer_profile()
        }
        .restrict(&ceiling());
        assert_ne!(a.descriptor_digest(), b.descriptor_digest());
    }

    #[test]
    fn command_and_hook_descriptors_serialize() {
        let command = CommandDescriptor {
            name: "release-check".into(),
            description: "release checklist".into(),
            group: "release".into(),
            layer: ExtensionLayer::Project,
            execution: CommandExecutionSpec::Agent {
                agent: "reviewer".into(),
                prompt: "Review the release candidate.".into(),
            },
            capabilities: [capability("release")].into_iter().collect(),
            references: vec!["@diff".into()],
            priority: 0,
        };
        let hook = HookDescriptor {
            id: "h1".into(),
            layer: ExtensionLayer::Project,
            trigger: HookTrigger::BeforeWrite,
            path_filter: vec![],
            action: HookAction::Tool {
                tool_id: ToolId::native("fmt"),
                arguments: serde_json::json!({"check": true}),
            },
            blocking: false,
            timeout_seconds: 30,
            descriptor_digest: "abc".into(),
        };
        let command_json = serde_json::to_value(&command).unwrap();
        assert_eq!(command_json["execution"]["kind"], "agent");
        assert_eq!(command_json["references"][0], "@diff");
        let hook_json = serde_json::to_value(&hook).unwrap();
        assert_eq!(hook_json["trigger"], "before_write");
        assert_eq!(hook_json["action"]["kind"], "tool");
        assert_eq!(hook_json["blocking"], false);
    }

    #[test]
    fn tool_id_parse_rejects_unknown_namespaces() {
        assert!(ToolId::parse("native:read_file").is_some());
        assert!(ToolId::parse("mcp:github/create_issue").is_some());
        assert!(ToolId::parse("skill:rust-review/lint").is_some());
        assert!(ToolId::parse("random:thing").is_none());
        assert!(ToolId::parse("").is_none());
    }
}
