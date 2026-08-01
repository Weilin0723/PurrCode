//! v1.0 Adaptive worklow orchestration and efficiency contracts (PRD §9–12).
//!
//! These domain types are serializable, provider-independent, and live in
//! runtime-core so every crate can agree on the same vocabulary. The daemon
//! owns the live decision engine; clients consume typed summaries through the
//! v1.0 presentation API.

use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ── Task complexity classification (PRD §9.3) ──────────────────────────

#[derive(Clone, Copy, Debug, Eq, Hash, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskComplexity {
    Simple,
    Moderate,
    Complex,
    Unknown,
}

#[derive(Clone, Debug, Eq, Hash, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ComplexitySignal {
    AffectedModuleCount(usize),
    LanguageCount(usize),
    RequestedArtifactCount(usize),
    MigrationRequired,
    ExternalApiDependency,
    UncertaintyFromInspection,
    BaselineTestFailure,
    MultipleDeliverables,
    UserQualityHigh,
}

#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct ComplexityDecision {
    pub complexity: TaskComplexity,
    pub evidence: Vec<ComplexitySignal>,
    pub selected_workflow: WorkflowProfile,
    pub selected_search_policy: SearchPolicy,
    pub selected_budget: BudgetProfileKind,
}

// ── Workflow profiles (PRD §9.2) ───────────────────────────────────────

#[derive(Clone, Copy, Debug, Eq, Hash, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowProfile {
    Direct,
    Standard,
    Ultra,
}

impl WorkflowProfile {
    pub fn default_search_policy(self) -> SearchPolicy {
        match self {
            Self::Direct => SearchPolicy::Off,
            Self::Standard | Self::Ultra => SearchPolicy::Auto,
        }
    }
}

#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct WorkflowPlanId(pub Uuid);

impl WorkflowPlanId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowLaneKind {
    RepositoryAnalysis,
    Planning,
    Research,
    Implementation,
    Validation,
    Review,
    GitHubDelivery,
    Recovery,
}

#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct WorkflowLane {
    pub lane_id: WorkflowLaneId,
    pub kind: WorkflowLaneKind,
    pub objective: String,
    pub allowed_tools: Vec<String>,
    pub read_scope: Vec<String>,
    pub write_scope: Vec<String>,
    pub token_budget: u64,
    pub wall_time_budget_seconds: u64,
    pub completion_condition: String,
    pub dependencies: Vec<WorkflowLaneId>,
}

#[derive(
    Clone, Copy, Debug, Eq, Hash, JsonSchema, Ord, PartialEq, PartialOrd, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct WorkflowLaneId(pub Uuid);

impl WorkflowLaneId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct WorkflowDependency {
    pub from: WorkflowLaneId,
    pub to: WorkflowLaneId,
    pub artifact_kind: String,
}

#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct WorkflowBudgets {
    pub max_active_lanes: u32,
    pub max_depth: u32,
    pub max_repair_cycles: u32,
    pub max_parallel_writers_per_scope: u32,
}

#[derive(Clone, Copy, Debug, Eq, Hash, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompletionCondition {
    AllLanesDone,
    CoordinatorSatisfied,
}

// ── Search policy (PRD §11) ────────────────────────────────────────────

#[derive(Clone, Copy, Debug, Eq, Hash, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchPolicy {
    Off,
    Auto,
    Always,
}

#[derive(Clone, Copy, Debug, Eq, Hash, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResearchTrigger {
    ExplicitUserRequest,
    MissingDependency,
    UnknownExternalApi,
    RepeatedValidationFailure,
    SecurityAdvisory,
    ConflictingLocalDocs,
    UnverifiedLocalFact,
}

#[derive(Clone, Copy, Debug, Eq, Hash, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourcePolicy {
    OfficialDocumentation,
    PrimaryRepo,
    Standards,
    ReleaseNotes,
    PeerReviewed,
    Secondary,
}

// ── Budgets (PRD §12.3) ────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, Eq, Hash, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BudgetProfileKind {
    Economy,
    Balanced,
    MaxQuality,
    Custom,
}

#[derive(Clone, Debug, JsonSchema, Default, PartialEq, Serialize, Deserialize)]
pub struct BudgetConstraints {
    pub maximum_input_tokens: Option<u64>,
    pub maximum_output_tokens: Option<u64>,
    pub maximum_total_tokens: Option<u64>,
    pub maximum_estimated_cost: Option<f64>,
    pub maximum_model_calls: Option<u32>,
    pub maximum_search_requests: Option<u32>,
    pub maximum_mcp_calls: Option<u32>,
    pub maximum_wall_time_seconds: Option<u64>,
}

// ── Usage ledger (PRD §12.2) ───────────────────────────────────────────

#[derive(Clone, Copy, Debug, Eq, Hash, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RequestId(pub Uuid);

impl RequestId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

#[derive(Clone, Debug, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct UsageRecord {
    pub request_id: RequestId,
    pub session_id: crate::SessionId,
    pub workflow_lane_id: Option<WorkflowLaneId>,
    pub provider_id: String,
    pub model_id: String,
    pub credential_id: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub tool_result_tokens: u64,
    pub search_requests: u32,
    pub mcp_calls: u32,
    pub estimated_cost: Option<f64>,
    pub latency_ms: u64,
    pub recorded_at: DateTime<Utc>,
}

#[derive(Clone, Debug, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct UsageSummary {
    pub total_tokens: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub search_requests: u32,
    pub mcp_calls: u32,
    pub estimated_total_cost: Option<f64>,
    pub model_call_count: u32,
    pub tokens_per_validated_change: Option<f64>,
    pub context_selection_ratio: Option<f64>,
    pub retry_token_share: Option<f64>,
}

// ── Credential pools (PRD §10) ─────────────────────────────────────────

#[derive(Clone, Debug, Eq, Hash, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SecretReference(pub String);

#[derive(Clone, Debug, Eq, Hash, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct CredentialProfile {
    pub credential_id: CredentialId,
    pub provider_id: String,
    pub label: String,
    pub secret_reference: SecretReference,
    pub allowed_models: Vec<String>,
    pub priority: u16,
    pub enabled: bool,
}

#[derive(Clone, Copy, Debug, Eq, Hash, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CredentialId(pub Uuid);

impl CredentialId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CredentialSelectionStrategy {
    Fixed(CredentialId),
    Priority,
    Weighted,
    LowestObservedCost,
    HighestRemainingBudget,
    HealthAware,
}

#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct CredentialPool {
    pub provider_id: String,
    pub credentials: Vec<CredentialProfile>,
    pub strategy: CredentialSelectionStrategy,
}

// ── Model routing (PRD §10.4) ──────────────────────────────────────────

#[derive(Clone, Copy, Debug, Eq, Hash, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoutingProfile {
    Fixed,
    Auto,
    Economy,
    Quality,
}

#[derive(Clone, Debug, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct ModelRouteDecision {
    pub decision_id: RouteDecisionId,
    pub workflow_lane_id: Option<WorkflowLaneId>,
    pub provider_id: String,
    pub model_id: String,
    pub credential_id: CredentialId,
    pub reasons: Vec<RouteReason>,
    pub expected_capabilities: ModelCapabilities,
    pub privacy_class: PrivacyClass,
    pub budget_snapshot: BudgetSnapshot,
}

#[derive(Clone, Copy, Debug, Eq, Hash, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RouteDecisionId(pub Uuid);

impl RouteDecisionId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RouteReason {
    ToolCallingRequired,
    StructuredOutputRequired,
    CodingQualified,
    SufficientContextCapacity,
    TaskComplexity(TaskComplexity),
    LowestLatency,
    BudgetRemaining,
    ProviderHealthy,
    RateLimitOk,
    PrivacyOK,
    UserPinned,
    LocalResourceAvailable,
}

#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct ModelCapabilities {
    pub tool_calling: bool,
    pub structured_output: bool,
    pub coding_qualified: bool,
    pub context_capacity_tokens: u64,
}

#[derive(Clone, Copy, Debug, Eq, Hash, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrivacyClass {
    Local,
    ApprovedRemote,
    UnrestrictedRemote,
}

#[derive(Clone, Debug, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct BudgetSnapshot {
    pub remaining_tokens: Option<u64>,
    pub remaining_cost: Option<f64>,
    pub remaining_calls: Option<u32>,
    pub rate_limits: Vec<RateLimitInfo>,
}

#[derive(Clone, Debug, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct RateLimitInfo {
    pub name: String,
    pub remaining: u32,
    pub reset_seconds: u64,
}

// ── MCP (PRD §11.6–11.9) ───────────────────────────────────────────────

#[derive(Clone, Copy, Debug, Eq, Hash, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MCPTransport {
    Stdio,
    Http,
    Sse,
}

#[derive(Clone, Copy, Debug, Eq, Hash, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MCPScope {
    Project,
    User,
    Repository,
}

#[derive(Clone, Copy, Debug, Eq, Hash, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MCPTrustClass {
    Trusted,
    Verified,
    Unverified,
}

#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct MCPServerProfile {
    pub server_id: MCPServerId,
    pub name: String,
    pub transport: MCPTransport,
    pub scope: MCPScope,
    pub capabilities: Vec<String>,
    pub credential_reference: Option<SecretReference>,
    pub trust: MCPTrustClass,
    pub enabled: bool,
    pub max_response_bytes: u64,
    pub max_response_tokens: u64,
    pub max_records: u32,
    pub timeout_seconds: u64,
}

#[derive(Clone, Copy, Debug, Eq, Hash, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MCPServerId(pub Uuid);

impl MCPServerId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

// ── Product controls (PRD §9.7, §10.5) ─────────────────────────────────

#[derive(Clone, Copy, Debug, Eq, Hash, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowControl {
    Auto,
    Direct,
    Standard,
    Ultra,
}

#[derive(Clone, Copy, Debug, Eq, Hash, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelRoutingControl {
    Fixed,
    Auto,
    Economy,
    Quality,
}

impl ModelRoutingControl {
    pub fn as_routing_profile(self) -> RoutingProfile {
        match self {
            Self::Fixed => RoutingProfile::Fixed,
            Self::Auto => RoutingProfile::Auto,
            Self::Economy => RoutingProfile::Fixed,
            Self::Quality => RoutingProfile::Quality,
        }
    }
}
