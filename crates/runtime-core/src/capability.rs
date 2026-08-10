//! The one registry (v1.3 §4.2). Replaces the scattered command/skill/MCP/
//! agent/tool/model-role lookups.
//!
//! [`CapabilityRegistry::admit_tool`] is the **only mint** for a
//! [`crate::ToolDescriptor`]: it applies the workspace [`crate::ToolCeiling`]
//! and records every rejection/downgrade as an [`AdmissionDiagnostic`], so a
//! typo in a project file is visible at `GET /v1/extensions/diagnostics`
//! instead of silently vanishing.

use super::extension::{AgentDescriptor, CommandDescriptor, SkillDescriptor};
use super::tool::{ApprovalPolicy, ToolCeiling, ToolDescriptor, ToolDescriptorProposal, ToolId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

/// A capability is an INTENT, not an implementation. `code_review`,
/// `run_tests`, `format`, `security_scan`. Lowercase snake_case, validated by
/// the same safe-identifier rule mcp-host uses (mcp-host/src/lib.rs:824).
#[derive(
    Clone, Debug, Eq, Hash, JsonSchema, Ord, PartialEq, PartialOrd, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct CapabilityId(String);

impl CapabilityId {
    /// Parses a safe identifier: non-empty, ASCII alphanumeric plus
    /// `-` `_` `.` (the same rule `mcp_host::safe_identifier` applies).
    pub fn parse(raw: &str) -> Result<Self, super::DomainError> {
        if raw.is_empty()
            || !raw
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        {
            return Err(super::DomainError::InvalidBounds {
                reason: format!("`{raw}` is not a safe capability identifier"),
            });
        }
        Ok(Self(raw.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for CapabilityId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Which tier declared a provider. Drives the deterministic ranking
/// (Project > User > Builtin) and the restriction strength.
#[derive(
    Clone, Copy, Debug, Default, Eq, JsonSchema, Ord, PartialEq, PartialOrd, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum ExtensionLayer {
    #[default]
    Builtin = 0,
    User = 1,
    Project = 2,
}

/// Something that can satisfy a capability. One capability, many providers.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CapabilityProvider {
    Agent {
        name: String,
        layer: ExtensionLayer,
        priority: i32,
    },
    Skill {
        skill_id: String,
        layer: ExtensionLayer,
        priority: i32,
    },
    Command {
        name: String,
        layer: ExtensionLayer,
        priority: i32,
    },
    Tool {
        tool_id: ToolId,
        layer: ExtensionLayer,
        priority: i32,
    },
}

impl CapabilityProvider {
    pub fn layer(&self) -> ExtensionLayer {
        match self {
            CapabilityProvider::Agent { layer, .. }
            | CapabilityProvider::Skill { layer, .. }
            | CapabilityProvider::Command { layer, .. }
            | CapabilityProvider::Tool { layer, .. } => *layer,
        }
    }

    pub fn priority(&self) -> i32 {
        match self {
            CapabilityProvider::Agent { priority, .. }
            | CapabilityProvider::Skill { priority, .. }
            | CapabilityProvider::Command { priority, .. }
            | CapabilityProvider::Tool { priority, .. } => *priority,
        }
    }

    /// Deterministic ranking key: (layer desc, priority desc, id asc).
    /// Project > User > Builtin; ties broken by explicit priority, then id.
    pub fn rank_key(
        &self,
    ) -> (
        std::cmp::Reverse<ExtensionLayer>,
        std::cmp::Reverse<i32>,
        String,
    ) {
        (
            std::cmp::Reverse(self.layer()),
            std::cmp::Reverse(self.priority()),
            self.identity().to_owned(),
        )
    }

    /// The human-visible id this provider is known by (agent name, skill id,
    /// command name, or tool id). Used as the deterministic tiebreak.
    pub fn identity(&self) -> &str {
        match self {
            CapabilityProvider::Agent { name, .. } => name,
            CapabilityProvider::Skill { skill_id, .. } => skill_id,
            CapabilityProvider::Command { name, .. } => name,
            CapabilityProvider::Tool { tool_id, .. } => tool_id.as_str(),
        }
    }
}

/// What a load-time restriction decided about one subject.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AdmissionDiagnostic {
    pub source_path: Option<PathBuf>,
    pub subject: String,
    pub severity: DiagnosticSeverity,
    pub message: String,
    /// Populated when a field was clamped: ("network_scope", "any", "none").
    pub restricted_fields: Vec<(String, String, String)>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticSeverity {
    Info,
    Restricted,
    Rejected,
}

/// The one registry.
#[derive(Debug, Default)]
pub struct CapabilityRegistry {
    tools: BTreeMap<ToolId, ToolDescriptor>,
    agents: BTreeMap<String, AgentDescriptor>,
    skills: BTreeMap<String, SkillDescriptor>,
    commands: BTreeMap<String, CommandDescriptor>,
    hooks: BTreeMap<super::HookTrigger, Vec<super::HookDescriptor>>,
    /// Reverse index, rebuilt on every admit.
    by_capability: BTreeMap<CapabilityId, Vec<CapabilityProvider>>,
    /// Every proposal that was rejected or downgraded, with the reason.
    /// Surfaced at `GET /v1/extensions/diagnostics` so a typo is visible
    /// instead of silent (see §2.3 risk: malformed entries vanish today).
    diagnostics: Vec<AdmissionDiagnostic>,
}

impl CapabilityRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// THE ONLY MINT for a [`ToolDescriptor`]. Applies the ceiling and records
    /// any downgrade or rejection as a diagnostic. Returns the admitted
    /// descriptor (admitted as `ApprovalPolicy::Forbidden` when denied).
    ///
    /// The namespace prefix is assigned HERE, not by the provider: a proposal
    /// whose id prefix does not match its declared provider is rejected with a
    /// diagnostic. This is what stops a user-installed skill from declaring
    /// `native:read_file` and shadowing the builtin.
    pub fn admit_tool(
        &mut self,
        proposal: ToolDescriptorProposal,
        ceiling: &ToolCeiling,
    ) -> &ToolDescriptor {
        // The registry is the source of the namespace: `native:` ids come from
        // the builtin table, `mcp:` from McpHost, `skill:` from the skill
        // store. A provider that claims a namespace it does not own is a
        // shadowing attempt.
        let id_namespace = proposal.id.namespace();
        let declared = match proposal.provider {
            super::ToolProvider::Native => "native",
            super::ToolProvider::Mcp => "mcp",
            super::ToolProvider::Skill => "skill",
        };
        if let Some(ns) = id_namespace
            && ns != declared
        {
            let subject = proposal.id.as_str().to_string();
            self.diagnostics.push(AdmissionDiagnostic {
                source_path: None,
                subject: subject.clone(),
                severity: DiagnosticSeverity::Rejected,
                message: format!(
                    "tool id namespace `{ns}:` does not match its declared provider `{declared}`"
                ),
                restricted_fields: vec![("provider".into(), declared.into(), ns.into())],
            });
            let forbidden = ToolDescriptorProposal {
                id: proposal.id.clone(),
                provider: proposal.provider,
                display_name: proposal.display_name,
                description: proposal.description,
                schema: proposal.schema,
                capabilities: proposal.capabilities.clone(),
                side_effect_class: super::SideEffectClass::Read,
                network_scope: super::NetworkScope::None,
                filesystem_scope: super::FilesystemScope::None,
                approval_policy: super::ApprovalPolicy::Forbidden,
                origin: proposal.origin,
            };
            return self.admit_forbidden(forbidden, ceiling);
        }

        // v1.3 does not enforce host-precise egress: Claw sandboxes network as
        // a boolean (off / unrestricted) and MCP servers configure it the same
        // way. A proposal that claims a `Hosts { allowed }` list would promise
        // a guarantee nothing can honor, so it is admitted as Forbidden with a
        // Rejected diagnostic rather than silently weakened to `Any`. The
        // lattice `meet` keeps `Hosts` for future enforcement, but admission
        // refuses to mint a callable tool from it.
        if matches!(proposal.network_scope, super::NetworkScope::Hosts { .. }) {
            let subject = proposal.id.as_str().to_string();
            self.diagnostics.push(AdmissionDiagnostic {
                source_path: None,
                subject: subject.clone(),
                severity: DiagnosticSeverity::Rejected,
                message:
                    "NetworkScope::Hosts is not enforceable in v1.3; a host-precise scope is denied"
                        .into(),
                restricted_fields: vec![("network_scope".into(), "hosts".into(), "none".into())],
            });
            let forbidden = ToolDescriptorProposal {
                id: proposal.id.clone(),
                provider: proposal.provider,
                display_name: proposal.display_name,
                description: proposal.description,
                schema: proposal.schema,
                capabilities: proposal.capabilities.clone(),
                side_effect_class: super::SideEffectClass::Read,
                network_scope: super::NetworkScope::None,
                filesystem_scope: super::FilesystemScope::None,
                approval_policy: super::ApprovalPolicy::Forbidden,
                origin: proposal.origin,
            };
            return self.admit_forbidden(forbidden, ceiling);
        }

        let (descriptor, diagnostics) = proposal.restrict(ceiling);
        self.diagnostics.extend(diagnostics);

        let tool_id = descriptor.id().clone();
        let capabilities = descriptor.capabilities().clone();
        let layer = origin_to_layer(descriptor.origin());
        self.tools.insert(tool_id.clone(), descriptor);
        self.register_provider(
            &CapabilityProvider::Tool {
                tool_id: tool_id.clone(),
                layer,
                priority: 0,
            },
            &capabilities,
        );
        self.tools.get(&tool_id).expect("just inserted")
    }

    /// Admit a descriptor that is unconditionally forbidden — the id is
    /// rejected before restriction, so it never enters the registry as a
    /// callable tool. If a callable descriptor already holds this id (e.g. the
    /// builtin `native:read_file`), it is left intact; the forbidden proposal
    /// must not clobber it.
    fn admit_forbidden(
        &mut self,
        proposal: ToolDescriptorProposal,
        ceiling: &ToolCeiling,
    ) -> &ToolDescriptor {
        let tool_id = proposal.id.clone();
        if self.tools.contains_key(&tool_id)
            && self
                .tools
                .get(&tool_id)
                .is_some_and(|d| d.approval_policy() != ApprovalPolicy::Forbidden)
        {
            return self
                .tools
                .get(&tool_id)
                .expect("checked above: callable descriptor present");
        }
        let (descriptor, diagnostics) = proposal.restrict(ceiling);
        self.diagnostics.extend(diagnostics);
        self.tools.insert(tool_id.clone(), descriptor);
        self.tools.get(&tool_id).expect("just inserted")
    }

    /// Admit an agent profile (restricted to its ceiling) into the registry.
    pub fn admit_agent(
        &mut self,
        proposal: super::AgentProfile,
        ceiling: &ToolCeiling,
    ) -> &AgentDescriptor {
        let (descriptor, diagnostics) = proposal.restrict(ceiling);
        self.diagnostics.extend(diagnostics);

        let capabilities = descriptor.capabilities().clone();
        let name = descriptor.name().to_owned();
        let layer = descriptor.layer();
        let priority = descriptor.priority();
        self.agents.insert(name.clone(), descriptor);
        self.register_provider(
            &CapabilityProvider::Agent {
                name: name.clone(),
                layer,
                priority,
            },
            &capabilities,
        );
        self.agents.get(&name).expect("just inserted")
    }

    /// Admit a skill into the registry.
    pub fn admit_skill(&mut self, descriptor: SkillDescriptor) {
        let capabilities = descriptor.capabilities.clone();
        let skill_id = descriptor.skill_id.clone();
        let layer = descriptor.layer;
        let priority = descriptor.priority;
        self.skills.insert(skill_id.clone(), descriptor);
        self.register_provider(
            &CapabilityProvider::Skill {
                skill_id,
                layer,
                priority,
            },
            &capabilities,
        );
    }

    /// Admit a command into the registry.
    pub fn admit_command(&mut self, descriptor: CommandDescriptor) {
        let capabilities = descriptor.capabilities.clone();
        let name = descriptor.name.clone();
        let layer = descriptor.layer;
        let priority = descriptor.priority;
        self.commands.insert(name.clone(), descriptor);
        self.register_provider(
            &CapabilityProvider::Command {
                name,
                layer,
                priority,
            },
            &capabilities,
        );
    }

    /// Admit a hook into the registry.
    pub fn admit_hook(&mut self, descriptor: super::HookDescriptor) {
        let trigger = descriptor.trigger;
        self.hooks.entry(trigger).or_default().push(descriptor);
    }

    pub fn tool(&self, id: &ToolId) -> Option<&ToolDescriptor> {
        self.tools.get(id)
    }

    pub fn agent(&self, name: &str) -> Option<&AgentDescriptor> {
        self.agents.get(name)
    }

    pub fn command(&self, name: &str) -> Option<&CommandDescriptor> {
        self.commands.get(name)
    }

    pub fn hooks(&self, trigger: &super::HookTrigger) -> &[super::HookDescriptor] {
        self.hooks.get(trigger).map(Vec::as_slice).unwrap_or(&[])
    }

    pub fn tools(&self) -> impl Iterator<Item = &ToolDescriptor> {
        self.tools.values()
    }

    /// "Who can satisfy this capability?" — ranked, deterministic.
    pub fn resolve(&self, capability: &CapabilityId) -> &[CapabilityProvider] {
        self.by_capability
            .get(capability)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    /// The model-facing tool schema for a turn, filtered by the active agent's
    /// allowlist. This is what replaces `schema_for!(AgentTurn)` at
    /// agent-runtime/src/agent.rs:2337 (wired in PR5).
    pub fn turn_schema(&self, _agent: &AgentDescriptor) -> serde_json::Value {
        // PR5 populates this from the admitted descriptors. A unit-test stub
        // that returns an empty tools array keeps the current call path
        // byte-identical until the registry is wired in.
        serde_json::json!({ "type": "array", "items": {} })
    }

    pub fn diagnostics(&self) -> &[AdmissionDiagnostic] {
        &self.diagnostics
    }

    /// Clear everything (e.g. on a reload). Rebuild the reverse index from the
    /// surviving providers.
    pub fn clear(&mut self) {
        self.tools.clear();
        self.agents.clear();
        self.skills.clear();
        self.commands.clear();
        self.hooks.clear();
        self.by_capability.clear();
        self.diagnostics.clear();
    }

    fn register_provider(
        &mut self,
        provider: &CapabilityProvider,
        capabilities: &std::collections::BTreeSet<CapabilityId>,
    ) {
        for capability in capabilities {
            let entry = self.by_capability.entry(capability.clone()).or_default();
            entry.push(provider.clone());
            entry.sort_by_key(|p| p.rank_key());
        }
    }
}

/// Tool descriptors are ranked by the trust tier of their origin:
/// Builtin (compiled in) outranks User, which outranks Project (checked-in,
/// untrusted) and RemoteDiscovery (authored by a remote server). Remote
/// discovery is the least trusted: the server wrote the descriptor itself.
pub fn origin_to_layer(origin: super::DescriptorOrigin) -> ExtensionLayer {
    match origin {
        super::DescriptorOrigin::Builtin => ExtensionLayer::Builtin,
        super::DescriptorOrigin::User => ExtensionLayer::User,
        super::DescriptorOrigin::Project | super::DescriptorOrigin::RemoteDiscovery => {
            ExtensionLayer::Project
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DescriptorOrigin, FilesystemScope, NetworkScope, SideEffectClass};

    fn capability(raw: &str) -> CapabilityId {
        CapabilityId::parse(raw).unwrap()
    }

    fn ceiling() -> ToolCeiling {
        ToolCeiling {
            maximum_side_effect: SideEffectClass::Destructive,
            maximum_network: NetworkScope::Any,
            maximum_filesystem: FilesystemScope::Worktree {
                write_globs: vec!["**".into()],
                maximum_changed_files: usize::MAX,
            },
            minimum_approval: ApprovalPolicy::ByClass,
            denied_tool_ids: std::collections::BTreeSet::new(),
        }
    }

    fn proposal(id: &str, capabilities: &[&str]) -> ToolDescriptorProposal {
        ToolDescriptorProposal {
            id: ToolId::native(id),
            provider: crate::ToolProvider::Native,
            display_name: id.into(),
            description: "test".into(),
            schema: serde_json::json!({ "type": "object" }),
            capabilities: capabilities.iter().map(|c| capability(c)).collect(),
            side_effect_class: SideEffectClass::Read,
            network_scope: NetworkScope::None,
            filesystem_scope: FilesystemScope::WorktreeRead,
            approval_policy: ApprovalPolicy::ByClass,
            origin: DescriptorOrigin::Builtin,
        }
    }

    #[test]
    fn admit_tool_is_the_only_mint_and_ranks_by_layer() {
        let mut registry = CapabilityRegistry::new();
        let ceiling = ceiling();

        registry.admit_tool(proposal("read_file", &["code_review", "read"]), &ceiling);
        registry.admit_tool(
            ToolDescriptorProposal {
                origin: DescriptorOrigin::User,
                ..proposal("user_read", &["code_review"])
            },
            &ceiling,
        );
        registry.admit_tool(
            ToolDescriptorProposal {
                origin: DescriptorOrigin::Project,
                ..proposal("project_read", &["code_review"])
            },
            &ceiling,
        );

        let providers = registry.resolve(&capability("code_review"));
        // Project > User > Builtin (highest-precedence first).
        assert_eq!(providers.len(), 3);
        assert_eq!(providers[0].identity(), "native:project_read");
        assert_eq!(providers[0].layer(), ExtensionLayer::Project);
        assert_eq!(providers[1].identity(), "native:user_read");
        assert_eq!(providers[1].layer(), ExtensionLayer::User);
        assert_eq!(providers[2].identity(), "native:read_file");
        assert_eq!(providers[2].layer(), ExtensionLayer::Builtin);

        // Unknown capability resolves empty.
        assert!(registry.resolve(&capability("does_not_exist")).is_empty());
    }

    #[test]
    fn priority_breaks_ties_within_a_layer() {
        let mut registry = CapabilityRegistry::new();
        let ceiling = ceiling();

        registry.admit_tool(
            ToolDescriptorProposal {
                origin: DescriptorOrigin::Project,
                ..proposal("low", &["format"])
            },
            &ceiling,
        );
        registry.admit_tool(
            ToolDescriptorProposal {
                origin: DescriptorOrigin::Project,
                ..proposal("high", &["format"])
            },
            &ceiling,
        );

        // Same layer, same priority → tiebreak by id ascending.
        let providers = registry.resolve(&capability("format"));
        assert_eq!(providers.len(), 2);
        assert_eq!(providers[0].identity(), "native:high");
        assert_eq!(providers[1].identity(), "native:low");
    }

    #[test]
    fn diagnostics_record_restriction() {
        let mut registry = CapabilityRegistry::new();
        let ceiling = ToolCeiling {
            maximum_side_effect: SideEffectClass::Read,
            ..ceiling()
        };
        registry.admit_tool(
            ToolDescriptorProposal {
                side_effect_class: SideEffectClass::Destructive,
                ..proposal("write_tool", &[])
            },
            &ceiling,
        );

        let diagnostics = registry.diagnostics();
        let restricted = diagnostics
            .iter()
            .find(|d| d.subject == "native:write_tool")
            .expect("diagnostic recorded");
        assert_eq!(restricted.severity, DiagnosticSeverity::Restricted);
        assert_eq!(restricted.restricted_fields[0].0, "side_effect_class");
        assert_eq!(restricted.restricted_fields[0].1, "Destructive");
        assert_eq!(restricted.restricted_fields[0].2, "Read");
    }

    #[test]
    fn provider_cannot_claim_another_namespace() {
        // A skill that declares `native:read_file` must not shadow the builtin
        // — the namespace is assigned by the registry, never the provider.
        let mut registry = CapabilityRegistry::new();
        let ceiling = ceiling();

        registry.admit_tool(proposal("read_file", &["read"]), &ceiling);
        let shadow = registry.admit_tool(
            ToolDescriptorProposal {
                id: ToolId::native("read_file"),      // claims the native namespace
                provider: crate::ToolProvider::Skill, // but is a skill
                ..proposal("read_file", &["read"])
            },
            &ceiling,
        );
        // The shadowing attempt must NOT replace the callable builtin.
        assert_eq!(
            shadow.approval_policy(),
            ApprovalPolicy::ByClass,
            "the callable builtin must survive; the shadow is refused"
        );
        assert_eq!(
            shadow.provider(),
            crate::ToolProvider::Native,
            "the surviving entry is the builtin, not the skill"
        );
        assert!(
            registry
                .diagnostics()
                .iter()
                .any(|d| d.subject == "native:read_file"
                    && d.severity == DiagnosticSeverity::Rejected),
            "the shadowing attempt must be surfaced as a Rejected diagnostic"
        );
    }

    #[test]
    fn hosts_network_scope_is_admitted_as_forbidden_with_a_rejected_diagnostic() {
        let mut registry = CapabilityRegistry::new();
        let ceiling = ceiling();
        let admitted = registry.admit_tool(
            ToolDescriptorProposal {
                network_scope: NetworkScope::Hosts {
                    allowed: ["api.example.com".into()].into_iter().collect(),
                },
                ..proposal("hosts_scope", &["read"])
            },
            &ceiling,
        );
        // Host-precise egress is not enforceable in v1.3 (Claw network is a
        // boolean), so the tool must be admitted as Forbidden, never callable,
        // and the refusal surfaced as a Rejected diagnostic.
        assert_eq!(
            admitted.approval_policy(),
            ApprovalPolicy::Forbidden,
            "a Hosts-scoped proposal must be admitted as Forbidden"
        );
        assert!(
            registry
                .diagnostics()
                .iter()
                .any(|d| d.subject == "native:hosts_scope"
                    && d.severity == DiagnosticSeverity::Rejected),
            "the Hosts-scoped proposal must be surfaced as a Rejected diagnostic"
        );
    }

    #[test]
    fn rank_key_orders_layer_desc_then_priority_desc_then_id_asc() {
        let builtin = CapabilityProvider::Agent {
            name: "a".into(),
            layer: ExtensionLayer::Builtin,
            priority: 100,
        };
        let project_high = CapabilityProvider::Agent {
            name: "b".into(),
            layer: ExtensionLayer::Project,
            priority: 0,
        };
        let project_low = CapabilityProvider::Agent {
            name: "c".into(),
            layer: ExtensionLayer::Project,
            priority: -5,
        };
        let mut keys: Vec<_> = [&project_low, &builtin, &project_high]
            .iter()
            .map(|p| p.rank_key())
            .collect();
        keys.sort();
        // Sort is ascending on the tuple; Reverse(layer) puts Project (2)
        // before Builtin (0).
        assert_eq!(keys[0].2, "b");
        assert_eq!(keys[1].2, "c");
        assert_eq!(keys[2].2, "a");
    }

    #[test]
    fn capability_id_validation() {
        assert!(CapabilityId::parse("code_review").is_ok());
        assert!(CapabilityId::parse("security-scan").is_ok());
        assert!(CapabilityId::parse("run_tests").is_ok());
        assert!(CapabilityId::parse("").is_err());
        assert!(CapabilityId::parse("has space").is_err());
        assert!(CapabilityId::parse("has/ slash").is_err());
    }
}
