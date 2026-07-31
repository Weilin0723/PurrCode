//! Automatic model selection contracts (PRD §7).
//!
//! PurrCode must not make the user pick a model by hand: after Managed Identity
//! resolves and deployments are enumerated, the runtime tests authentication,
//! runs bounded qualification probes, and assigns the best qualified
//! deployments to runtime roles (PRD §7.1). This crate pins the contract for
//! that process — roles, deployments, qualification criteria, the selection
//! policy, and the cached result `ModelSelectionResult` — without depending on
//! `provider-gateway` (which owns the live HTTP path) or tokio. A deployment is
//! identified here by opaque provider/model strings so the live runtime plugs in
//! the real `ModelId`; the ranking logic and role assignment are pure.

pub mod candidate;

pub use candidate::{
    rank, select_coder, select_judge, ModelCandidate, ModelPurpose, SelectionBudget, SizePreference,
};

use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::time::Duration;
use thiserror::Error;
use uuid::Uuid;

/// A runtime model role (PRD §7.2). One deployment may serve multiple roles
/// (e.g. the same deployment covers CodingWorker and Judge in reduced-
/// independence mode).
#[derive(
    Clone, Copy, Debug, Eq, Hash, JsonSchema, Ord, PartialEq, PartialOrd, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum ModelRole {
    CodingWorker,
    Planner,
    Judge,
    Summarizer,
    Embedding,
    FastRouter,
}

impl ModelRole {
    /// Canonical role string matching the existing loose-string role scheme in
    /// the daemon (`coding_worker`, `planner`, `judge`, …). Adding a role here
    /// and canonicalizing the string lets the existing provider-gateway code
    /// migrate incrementally.
    pub fn as_str(self) -> &'static str {
        match self {
            ModelRole::CodingWorker => "coding_worker",
            ModelRole::Planner => "planner",
            ModelRole::Judge => "judge",
            ModelRole::Summarizer => "summarizer",
            ModelRole::Embedding => "embedding",
            ModelRole::FastRouter => "fast_router",
        }
    }
}

impl std::fmt::Display for ModelRole {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Stable identifier for a PurrCode-managed model deployment / endpoint (PRD
/// §6.2). The runtime assigns these during enumeration; the cache is keyed by
/// this id so re-qualification is stable across reconnects.
#[derive(
    Clone, Copy, Debug, Eq, Hash, JsonSchema, Ord, PartialEq, PartialOrd, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct DeploymentId(pub Uuid);

impl DeploymentId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for DeploymentId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for DeploymentId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Where a deployment's endpoint came from (drives the PRD §6.2 discovery
/// ordering and the §6.1 "do not prompt" fallback).
#[derive(
    Clone, Copy, Debug, Eq, JsonSchema, Ord, PartialEq, PartialOrd, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum EndpointSource {
    /// Discovered via Foundry project deployment-list API (PRD §6.1 path 1).
    FoundryProject,
    /// Discovered via subscription-level Cognitive Services / Foundry enumeration
    /// (PRD §6.1 path 2).
    AzureSubscription,
    /// Azure OpenAI endpoint.
    AzureOpenAi,
    /// Azure API Management AI gateway endpoint.
    AzureApimGateway,
    /// OpenAI-compatible endpoint.
    OpenAiCompatible,
    /// Manually pasted as a one-time fallback (PRD §6.2 endpoint fallback).
    ManualFallback,
}

/// Privacy/endpoint classification (PRD §7.3 "privacy and endpoint
/// classification"). Drives whether a deployment can serve a role under a
/// local-only or data-residency policy.
#[derive(
    Clone, Copy, Debug, Eq, JsonSchema, Ord, PartialEq, PartialOrd, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum EndpointClassification {
    /// Local-only model (Ollama / LM Studio loopback).
    Local,
    /// Azure-hosted, accessed with Managed Identity / Entra.
    Azure,
    /// Remote third-party (OpenAI, generic OpenAI-compatible).
    RemoteThirdParty,
}

/// A deployment the scheduler may route to (PRD §7.1). Identified by opaque
/// provider/model strings so the live `provider-gateway::ModelId` plugs in
/// without this crate depending on it.
#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct ModelDeployment {
    pub deployment_id: DeploymentId,
    pub provider: String,
    pub model: String,
    /// When known, the deployment name on the hosting account.
    #[serde(default)]
    pub deployment_name: Option<String>,
    pub endpoint_source: EndpointSource,
    pub classification: EndpointClassification,
    /// Roles this deployment is willing to serve; qualification narrows this.
    #[serde(default)]
    pub declared_roles: Vec<ModelRole>,
    /// Observed qualification (None until probed).
    #[serde(default)]
    pub qualification: Option<QualificationReport>,
}

/// The criteria the qualification probe evaluates (PRD §7.3).
#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct QualificationCriteria {
    pub tool_call_compatible: bool,
    pub structured_output_reliable: bool,
    pub coding_accuracy: Option<u8>,
    pub context_capacity_tokens: Option<u32>,
    pub observed_latency: Option<DurationSerde>,
    pub rate_limit_per_minute: Option<u32>,
    pub streaming_supported: bool,
    pub error_stable: bool,
    /// True when cost metadata was observed (PRD §7.3 "cost metadata when
    /// available"). Absent cost is not a disqualifier.
    pub cost_observed: bool,
}

/// Result of probing a deployment against the criteria (PRD §7.3, §7.4).
#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct QualificationReport {
    pub criteria: QualificationCriteria,
    /// Overall pass/fail per the qualification gate (`Unverified` means the
    /// probe could not complete — never reported as qualified).
    pub status: QualificationStatus,
    pub measurement_latency: DurationSerde,
    /// Free text evidence for the report card (PRD §3.5 evidence-based).
    #[serde(default)]
    pub notes: String,
}

#[derive(
    Clone, Copy, Debug, Eq, JsonSchema, Ord, PartialEq, PartialOrd, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum QualificationStatus {
    Qualified,
    Failed,
    /// Probe could not complete (endpoint down, no auth, timeout). Reused from
    /// runtime-core semantics: unavailable is never qualified.
    Unverified,
}

/// Preference order for ranking (PRD §7.4).
#[derive(
    Clone, Copy, Debug, Eq, JsonSchema, Ord, PartialEq, PartialOrd, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum SelectionPreference {
    Qualified,
    ToolCompatible,
    CodingQuality,
    Latency,
    Cost,
}

/// The automatic selection policy (PRD §7.4).
#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct ModelSelectionPolicy {
    pub mode: SelectionMode,
    pub preference_order: Vec<SelectionPreference>,
    /// Re-qualify after this many hours (PRD §7.4 `requalify_after_hours`).
    #[serde(default = "default_requalify_hours")]
    pub requalify_after_hours: u32,
}

fn default_requalify_hours() -> u32 {
    168
}

impl Default for ModelSelectionPolicy {
    fn default() -> Self {
        // PRD §7.4 example policy.
        Self {
            mode: SelectionMode::Automatic,
            preference_order: vec![
                SelectionPreference::Qualified,
                SelectionPreference::ToolCompatible,
                SelectionPreference::CodingQuality,
                SelectionPreference::Latency,
                SelectionPreference::Cost,
            ],
            requalify_after_hours: 168,
        }
    }
}

#[derive(
    Clone, Copy, Debug, Eq, JsonSchema, Ord, PartialEq, PartialOrd, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum SelectionMode {
    Automatic,
    ManualFallback,
}

/// The cached selection result: a role → deployment map plus the selection
/// timestamp (PRD §7.5 "Selected automatically … Change in Settings → Models").
#[derive(Clone, Debug, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct ModelSelectionResult {
    pub selected_at: DateTime<Utc>,
    /// Role → DeploymentId. Every required role must be present or the result is
    /// considered incomplete and the runtime must surface it per §7.5.
    pub assignments: BTreeMap<ModelRole, DeploymentId>,
    #[serde(default)]
    pub rationale: String,
}

#[derive(Clone, Debug, Error, JsonSchema, PartialEq, Serialize, Deserialize)]
pub enum ModelSelectionError {
    #[error("no deployment is accessible")]
    NoDeploymentAccessible,
    #[error("every deployment failed qualification")]
    AllFailedQualification,
    #[error("no deployment qualifies for role {0}")]
    NoQualifiedDeployment(String),
    #[error("selecting a model would violate an explicit organization policy")]
    PolicyViolation,
}

/// Serde-friendly Duration shim (kept dependency-free of tokio/humantime).
#[derive(Clone, Copy, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct DurationSerde {
    pub secs: u64,
    pub nanos: u32,
}

impl DurationSerde {
    pub fn from_duration(d: Duration) -> Self {
        Self {
            secs: d.as_secs(),
            nanos: d.subsec_nanos(),
        }
    }
    pub fn to_duration(self) -> Duration {
        Duration::new(self.secs, self.nanos)
    }
}

impl From<Duration> for DurationSerde {
    fn from(d: Duration) -> Self {
        Self::from_duration(d)
    }
}
impl From<DurationSerde> for Duration {
    fn from(d: DurationSerde) -> Self {
        d.to_duration()
    }
}

/// The roles the runtime must assign before it can proceed without prompting
/// (PRD §7.5). `FastRouter` and `Embedding` may be absent when no deployment
/// qualifies, but `CodingWorker`, `Planner`, and `Judge` are mandatory.
pub const MANDATORY_ROLES: &[ModelRole] = &[
    ModelRole::CodingWorker,
    ModelRole::Planner,
    ModelRole::Judge,
];

/// Rank one deployment against `policy.preference_order`. Lower sorts better.
///
/// Each preference contributes one comparison component, in the order the
/// policy lists them, so reordering the policy actually reorders the result.
/// The `DeploymentId` is the final component: it breaks remaining ties into a
/// total order, which is what keeps the cached selection stable across
/// re-qualification, but it is now the *last* word rather than the only one.
fn preference_key(
    deployment: &ModelDeployment,
    policy: &ModelSelectionPolicy,
) -> (Vec<i64>, DeploymentId) {
    let criteria = deployment.qualification.as_ref().map(|q| &q.criteria);
    let components = policy
        .preference_order
        .iter()
        .map(|preference| match preference {
            SelectionPreference::Qualified => i64::from(!matches!(
                deployment.qualification.as_ref().map(|q| q.status),
                Some(QualificationStatus::Qualified)
            )),
            SelectionPreference::ToolCompatible => {
                i64::from(!criteria.is_some_and(|c| c.tool_call_compatible))
            }
            // Negated: higher accuracy must sort earlier.
            SelectionPreference::CodingQuality => {
                -i64::from(criteria.and_then(|c| c.coding_accuracy).unwrap_or(0))
            }
            SelectionPreference::Latency => {
                deployment.qualification.as_ref().map_or(i64::MAX, |q| {
                    q.measurement_latency.to_duration().as_millis() as i64
                })
            }
            // Absent cost metadata is not a disqualifier (PRD §7.3), only a
            // tie-break against a deployment whose cost is known.
            SelectionPreference::Cost => i64::from(!criteria.is_some_and(|c| c.cost_observed)),
        })
        .collect();
    (components, deployment.deployment_id)
}

/// Select role assignments automatically from a set of probed deployments,
/// per the PRD §7.4 preference order. Pure: no I/O. Returns an error only when
/// the policy is unsatisfiable (PRD §7.5 "Ask only when necessary").
///
/// Strategy: filter to `Qualified` deployments that declare the role, then take
/// the best one under [`preference_key`].
pub fn select_models(
    deployments: &[ModelDeployment],
    policy: &ModelSelectionPolicy,
) -> Result<ModelSelectionResult, ModelSelectionError> {
    if deployments.is_empty() {
        return Err(ModelSelectionError::NoDeploymentAccessible);
    }
    if deployments.iter().all(|d| {
        matches!(
            d.qualification.as_ref().map(|q| q.status),
            Some(QualificationStatus::Failed)
        )
    }) {
        return Err(ModelSelectionError::AllFailedQualification);
    }

    let mut assignments: BTreeMap<ModelRole, DeploymentId> = BTreeMap::new();
    let mut rationale_parts: Vec<String> = Vec::new();

    for role in MANDATORY_ROLES {
        let chosen = deployments
            .iter()
            .filter(|d| {
                d.declared_roles.contains(role)
                    && matches!(
                        d.qualification.as_ref().map(|q| q.status),
                        Some(QualificationStatus::Qualified)
                    )
            })
            .min_by_key(|d| preference_key(d, policy));

        match chosen {
            Some(d) => {
                assignments.insert(*role, d.deployment_id);
            }
            None => {
                return Err(ModelSelectionError::NoQualifiedDeployment(role.to_string()));
            }
        }
    }

    // Optional roles: fill when qualified, do not fail otherwise.
    for role in [
        ModelRole::Summarizer,
        ModelRole::FastRouter,
        ModelRole::Embedding,
    ] {
        let chosen = deployments
            .iter()
            .filter(|d| {
                d.declared_roles.contains(&role)
                    && matches!(
                        d.qualification.as_ref().map(|q| q.status),
                        Some(QualificationStatus::Qualified)
                    )
            })
            .min_by_key(|d| preference_key(d, policy));
        if let Some(d) = chosen {
            assignments.insert(role, d.deployment_id);
            rationale_parts.push(format!("{} assigned to {}/{}", role, d.provider, d.model));
        }
    }

    // Deterministic result timestamp is supplied by the caller; selection itself
    // is pure. The caller stamps `selected_at`.
    rationale_parts.push("Selected automatically".into());
    Ok(ModelSelectionResult {
        selected_at: Utc::now(),
        assignments,
        rationale: rationale_parts.join("\n"),
    })
}

/// True when the cached selection is stale relative to `requalify_after_hours`
/// (PRD §7.4). A pure predicate the scheduler consults before re-probing.
pub fn selection_is_stale(
    selected_at: DateTime<Utc>,
    now: DateTime<Utc>,
    policy: &ModelSelectionPolicy,
) -> bool {
    (now - selected_at).num_hours() as u32 >= policy.requalify_after_hours
}

#[cfg(test)]
mod tests {
    use super::*;

    fn deploy(
        provider: &str,
        model: &str,
        roles: &[ModelRole],
        status: QualificationStatus,
    ) -> ModelDeployment {
        ModelDeployment {
            deployment_id: DeploymentId::new(),
            provider: provider.into(),
            model: model.into(),
            deployment_name: Some(format!("{provider}-{model}")),
            endpoint_source: EndpointSource::AzureOpenAi,
            classification: EndpointClassification::Azure,
            declared_roles: roles.to_vec(),
            qualification: Some(QualificationReport {
                criteria: QualificationCriteria {
                    tool_call_compatible: true,
                    structured_output_reliable: true,
                    coding_accuracy: Some(90),
                    context_capacity_tokens: Some(200_000),
                    observed_latency: Some(DurationSerde::from_duration(Duration::from_millis(
                        500,
                    ))),
                    rate_limit_per_minute: Some(100),
                    streaming_supported: true,
                    error_stable: true,
                    cost_observed: true,
                },
                status,
                measurement_latency: DurationSerde::from_duration(Duration::from_millis(500)),
                notes: "ok".into(),
            }),
        }
    }

    #[test]
    fn role_strings_match_existing_daemon_roles() {
        assert_eq!(ModelRole::CodingWorker.as_str(), "coding_worker");
        assert_eq!(ModelRole::FastRouter.as_str(), "fast_router");
        assert_eq!(ModelRole::Judge.as_str(), "judge");
    }

    #[test]
    fn no_deployments_errors_no_accessible() {
        let policy = ModelSelectionPolicy::default();
        assert_eq!(
            select_models(&[], &policy).unwrap_err(),
            ModelSelectionError::NoDeploymentAccessible
        );
    }

    #[test]
    fn all_failed_qualification_errors() {
        let policy = ModelSelectionPolicy::default();
        let ds = vec![deploy(
            "p",
            "m",
            &[ModelRole::CodingWorker],
            QualificationStatus::Failed,
        )];
        assert_eq!(
            select_models(&ds, &policy).unwrap_err(),
            ModelSelectionError::AllFailedQualification
        );
    }

    #[test]
    fn missing_role_errors_no_qualified_deployment() {
        let policy = ModelSelectionPolicy::default();
        // Only covers CodingWorker; Planner and Judge are missing.
        let ds = vec![deploy(
            "p",
            "m",
            &[ModelRole::CodingWorker],
            QualificationStatus::Qualified,
        )];
        assert!(matches!(
            select_models(&ds, &policy),
            Err(ModelSelectionError::NoQualifiedDeployment(_))
        ));
    }

    #[test]
    fn full_role_coverage_assigns_mandatory_and_optional() {
        let policy = ModelSelectionPolicy::default();
        let ds = vec![deploy(
            "az",
            "gpt-x",
            &[
                ModelRole::CodingWorker,
                ModelRole::Planner,
                ModelRole::Judge,
                ModelRole::Summarizer,
            ],
            QualificationStatus::Qualified,
        )];
        let res = select_models(&ds, &policy).unwrap();
        assert!(res.assignments.contains_key(&ModelRole::CodingWorker));
        assert!(res.assignments.contains_key(&ModelRole::Planner));
        assert!(res.assignments.contains_key(&ModelRole::Judge));
        assert!(res.assignments.contains_key(&ModelRole::Summarizer));
        assert!(res.rationale.contains("Selected automatically"));
        // FastRouter/Embedding were not declared, so they should be absent.
        assert!(!res.assignments.contains_key(&ModelRole::FastRouter));
    }

    #[test]
    fn unverified_is_never_qualified_and_treated_as_missing() {
        let policy = ModelSelectionPolicy::default();
        let ds = vec![deploy(
            "p",
            "m",
            &[ModelRole::CodingWorker],
            QualificationStatus::Unverified,
        )];
        // Unverified is not Failed and not Qualified → no qualified deployment.
        assert!(matches!(
            select_models(&ds, &policy),
            Err(ModelSelectionError::NoQualifiedDeployment(_))
        ));
    }

    #[test]
    fn selection_is_stale_after_requalify_window() {
        let policy = ModelSelectionPolicy {
            mode: SelectionMode::Automatic,
            preference_order: vec![],
            requalify_after_hours: 168,
        };
        let selected = Utc::now();
        assert!(!selection_is_stale(selected, selected, &policy));
        let later = selected + chrono::Duration::hours(169);
        assert!(selection_is_stale(selected, later, &policy));
    }

    #[test]
    fn preference_order_decides_between_two_qualified_deployments() {
        // Both qualify and both call tools. One is more accurate, the other is
        // faster, so the policy's ordering is the only thing that can choose.
        let mut accurate = deploy(
            "p",
            "accurate",
            &[
                ModelRole::CodingWorker,
                ModelRole::Planner,
                ModelRole::Judge,
            ],
            QualificationStatus::Qualified,
        );
        let mut fast = accurate.clone();
        fast.deployment_id = DeploymentId::new();
        fast.model = "fast".into();
        if let Some(report) = accurate.qualification.as_mut() {
            report.criteria.coding_accuracy = Some(95);
            report.measurement_latency = DurationSerde::from_duration(Duration::from_secs(4));
        }
        if let Some(report) = fast.qualification.as_mut() {
            report.criteria.coding_accuracy = Some(60);
            report.measurement_latency = DurationSerde::from_duration(Duration::from_millis(50));
        }
        let deployments = vec![accurate.clone(), fast.clone()];

        let quality_first = ModelSelectionPolicy::default();
        let chosen = select_models(&deployments, &quality_first)
            .unwrap()
            .assignments[&ModelRole::CodingWorker];
        assert_eq!(chosen, accurate.deployment_id);

        let latency_first = ModelSelectionPolicy {
            preference_order: vec![
                SelectionPreference::Qualified,
                SelectionPreference::Latency,
                SelectionPreference::CodingQuality,
            ],
            ..ModelSelectionPolicy::default()
        };
        let chosen = select_models(&deployments, &latency_first)
            .unwrap()
            .assignments[&ModelRole::CodingWorker];
        assert_eq!(
            chosen, fast.deployment_id,
            "reordering the policy must reorder the selection"
        );
    }

    #[test]
    fn default_policy_matches_prd_example() {
        let p = ModelSelectionPolicy::default();
        assert_eq!(p.requalify_after_hours, 168);
        assert_eq!(p.preference_order.len(), 5);
        assert!(matches!(
            p.preference_order[0],
            SelectionPreference::Qualified
        ));
    }
}
