//! Capability-based specialist routing (v1.4 §PR5).
//!
//! There are no hard-coded specialist classes in v1.4. A capability is an
//! intent (`security_review`, `write_tests`), the v1.3 [`CapabilityRegistry`] is
//! the only place that knows who can satisfy it, and this module picks one and
//! records why. That is what lets a user drop
//! `.purrcode/agents/security-reviewer.yaml` into a repository and have the
//! delegation planner use it with no Rust change.
//!
//! When nothing can satisfy a capability the answer is an explicit
//! *unavailable* — never a crash, and never a silent fallback to a
//! general-purpose agent that would run a security review it is not equipped
//! for.

use purrcode_runtime_core::delegation::{DelegationError, RoutingDecision};
use purrcode_runtime_core::{
    AgentDescriptor, CapabilityId, CapabilityProvider, CapabilityRegistry, ToolCeiling,
};

/// The specialist a capability resolved to, with the authority it brings.
#[derive(Clone, Debug)]
pub struct RoutedSpecialist {
    pub profile_name: String,
    pub profile_digest: String,
    /// The profile's ceiling. One of the three terms in the authority
    /// intersection — never the final authority on its own.
    pub ceiling: ToolCeiling,
    pub decision: RoutingDecision,
}

/// How to choose when several profiles can satisfy a capability.
#[derive(Clone, Debug)]
pub struct RoutingPolicy {
    /// Profiles that may never be selected by the planner (e.g. one the user
    /// disabled). Deny beats rank.
    pub denied_profiles: Vec<String>,
    /// A profile to fall back to when no provider declares the capability.
    /// `None` means "refuse", which is the safe default for a specialist task.
    pub fallback_profile: Option<String>,
}

impl Default for RoutingPolicy {
    fn default() -> Self {
        Self {
            denied_profiles: Vec::new(),
            // No implicit fallback: routing `security_review` to whatever agent
            // happens to exist is worse than telling the user nothing can do it.
            fallback_profile: None,
        }
    }
}

/// Resolve one capability to a specialist.
///
/// Ranking is the registry's — Project > User > Builtin, then explicit
/// priority, then id — so two runs of the same repository choose the same
/// specialist. Everything considered is recorded in the returned
/// [`RoutingDecision`], including the candidates that lost.
pub fn route(
    registry: &CapabilityRegistry,
    capability: &CapabilityId,
    policy: &RoutingPolicy,
) -> Result<RoutedSpecialist, DelegationError> {
    let providers = registry.resolve(capability);
    let mut candidates: Vec<&str> = providers
        .iter()
        .filter_map(|provider| match provider {
            CapabilityProvider::Agent { name, .. } => Some(name.as_str()),
            // A skill, command or tool can satisfy a capability *inside* a
            // worker, but it cannot BE a worker: a delegation needs an agent
            // profile to supply a ceiling, a model role and a system prompt.
            _ => None,
        })
        .collect();
    candidates.retain(|name| !policy.denied_profiles.iter().any(|denied| denied == name));

    let selected = candidates.first().copied().or(policy
        .fallback_profile
        .as_deref()
        .filter(|name| registry.agent(name).is_some()));

    let Some(name) = selected else {
        return Err(DelegationError::CapabilityUnavailable {
            capability: capability.to_string(),
        });
    };

    // A provider index can name a profile the registry no longer holds (a
    // deleted file, a profile rejected by admission). That is an unavailable
    // capability, not a panic.
    let Some(descriptor) = registry.agent(name) else {
        return Err(DelegationError::CapabilityUnavailable {
            capability: capability.to_string(),
        });
    };

    let from_fallback = candidates.first().copied() != Some(name);
    let reason = if from_fallback {
        format!("no profile declares `{capability}`; using the configured fallback `{name}`")
    } else {
        format!(
            "`{name}` is the highest-ranked {:?}-layer provider of `{capability}` \
             among {} candidate(s)",
            descriptor.layer(),
            candidates.len()
        )
    };

    Ok(RoutedSpecialist {
        profile_name: descriptor.name().to_owned(),
        profile_digest: descriptor.descriptor_digest().to_owned(),
        ceiling: descriptor.ceiling().clone(),
        decision: RoutingDecision {
            capability: capability.to_string(),
            chosen_profile: descriptor.name().to_owned(),
            profile_digest: descriptor.descriptor_digest().to_owned(),
            model_role: descriptor.model_role().cloned(),
            alternatives: candidates
                .into_iter()
                .filter(|candidate| *candidate != name)
                .map(str::to_owned)
                .collect(),
            reason,
        },
    })
}

/// How many distinct capabilities in `wanted` have at least one agent provider.
/// Feeds `DelegationSignals::available_specialist_capabilities`, so the
/// classifier's "are specialists available?" signal is measured rather than
/// assumed.
pub fn available_specialist_count(registry: &CapabilityRegistry, wanted: &[CapabilityId]) -> u32 {
    wanted
        .iter()
        .filter(|capability| {
            registry
                .resolve(capability)
                .iter()
                .any(|provider| matches!(provider, CapabilityProvider::Agent { .. }))
        })
        .count() as u32
}

/// The descriptor for a named profile, if the registry still holds it.
pub fn profile<'a>(registry: &'a CapabilityRegistry, name: &str) -> Option<&'a AgentDescriptor> {
    registry.agent(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use purrcode_runtime_core::{
        AgentProfile, ApprovalPolicy, ExtensionLayer, FilesystemScope, NetworkScope,
        PermissionRequest, SideEffectClass, ToolPolicy,
    };
    use std::collections::BTreeSet;

    fn capability(raw: &str) -> CapabilityId {
        CapabilityId::parse(raw).unwrap()
    }

    fn workspace_ceiling() -> ToolCeiling {
        ToolCeiling {
            maximum_side_effect: SideEffectClass::Destructive,
            maximum_network: NetworkScope::Any,
            maximum_filesystem: FilesystemScope::maximum(),
            minimum_approval: ApprovalPolicy::ByClass,
            denied_tool_ids: BTreeSet::new(),
        }
    }

    fn agent(name: &str, capabilities: &[&str], layer: ExtensionLayer) -> AgentProfile {
        AgentProfile {
            name: name.into(),
            description: format!("{name} specialist"),
            capabilities: capabilities.iter().map(|c| capability(c)).collect(),
            model_role: None,
            system_prompt: None,
            tools: ToolPolicy::default(),
            permissions: PermissionRequest::default(),
            context: Default::default(),
            skills: Default::default(),
            priority: 0,
            layer,
        }
    }

    fn registry() -> CapabilityRegistry {
        let mut registry = CapabilityRegistry::new();
        let ceiling = workspace_ceiling();
        registry.admit_agent(
            agent(
                "builtin-reviewer",
                &["security_review"],
                ExtensionLayer::Builtin,
            ),
            &ceiling,
        );
        registry.admit_agent(
            agent(
                "project-security-reviewer",
                &["security_review"],
                ExtensionLayer::Project,
            ),
            &ceiling,
        );
        registry.admit_agent(
            agent("backend", &["implement_backend"], ExtensionLayer::User),
            &ceiling,
        );
        registry
    }

    #[test]
    fn a_project_profile_outranks_a_builtin_and_the_choice_is_explained() {
        let registry = registry();
        let routed = route(
            &registry,
            &capability("security_review"),
            &RoutingPolicy::default(),
        )
        .unwrap();
        assert_eq!(routed.profile_name, "project-security-reviewer");
        assert_eq!(routed.decision.alternatives, vec!["builtin-reviewer"]);
        assert!(routed.decision.reason.contains("Project"));
        assert!(!routed.profile_digest.is_empty());
    }

    #[test]
    fn a_user_defined_profile_is_routable_without_a_rust_change() {
        // The §PR5 acceptance: dropping a YAML profile in `.purrcode/agents/`
        // is enough for the planner to use it.
        let mut registry = CapabilityRegistry::new();
        registry.admit_agent(
            agent(
                "perf-auditor",
                &["performance_review"],
                ExtensionLayer::Project,
            ),
            &workspace_ceiling(),
        );
        let routed = route(
            &registry,
            &capability("performance_review"),
            &RoutingPolicy::default(),
        )
        .unwrap();
        assert_eq!(routed.profile_name, "perf-auditor");
    }

    #[test]
    fn a_missing_capability_is_unavailable_not_a_crash() {
        // §PR5 acceptance: "Delete profile → fallback or explicit unavailable
        // state. Never crash."
        let registry = registry();
        let error = route(
            &registry,
            &capability("database_migration"),
            &RoutingPolicy::default(),
        )
        .expect_err("nothing provides this capability");
        assert!(matches!(
            error,
            DelegationError::CapabilityUnavailable { ref capability }
                if capability == "database_migration"
        ));
    }

    #[test]
    fn a_configured_fallback_is_used_and_labelled_as_such() {
        let registry = registry();
        let routed = route(
            &registry,
            &capability("documentation"),
            &RoutingPolicy {
                fallback_profile: Some("backend".into()),
                ..RoutingPolicy::default()
            },
        )
        .unwrap();
        assert_eq!(routed.profile_name, "backend");
        assert!(routed.decision.reason.contains("fallback"));
    }

    #[test]
    fn a_fallback_naming_a_deleted_profile_is_still_unavailable() {
        let registry = registry();
        let error = route(
            &registry,
            &capability("documentation"),
            &RoutingPolicy {
                fallback_profile: Some("profile-that-was-deleted".into()),
                ..RoutingPolicy::default()
            },
        )
        .expect_err("a fallback that does not exist cannot be routed to");
        assert!(matches!(
            error,
            DelegationError::CapabilityUnavailable { .. }
        ));
    }

    #[test]
    fn a_denied_profile_is_never_selected() {
        let registry = registry();
        let routed = route(
            &registry,
            &capability("security_review"),
            &RoutingPolicy {
                denied_profiles: vec!["project-security-reviewer".into()],
                ..RoutingPolicy::default()
            },
        )
        .unwrap();
        assert_eq!(routed.profile_name, "builtin-reviewer");
    }

    #[test]
    fn specialist_availability_is_measured_from_the_registry() {
        let registry = registry();
        let wanted = [
            capability("security_review"),
            capability("implement_backend"),
            capability("database_migration"),
        ];
        assert_eq!(available_specialist_count(&registry, &wanted), 2);
    }

    #[test]
    fn routing_is_deterministic() {
        let registry = registry();
        let first = route(
            &registry,
            &capability("security_review"),
            &RoutingPolicy::default(),
        )
        .unwrap();
        let second = route(
            &registry,
            &capability("security_review"),
            &RoutingPolicy::default(),
        )
        .unwrap();
        assert_eq!(first.profile_name, second.profile_name);
        assert_eq!(first.decision.alternatives, second.decision.alternatives);
    }
}
