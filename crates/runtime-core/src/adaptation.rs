//! v1.0 Adaptive worklow orchestration and efficiency contracts (PRD §9–12).
//!
//! These domain types are serializable, provider-independent, and live in
//! runtime-core so every crate can agree on the same vocabulary. The daemon
//! owns the live decision engine; clients consume typed summaries through the
//! v1.0 presentation API.

use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
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

#[derive(Clone, Copy, Debug, Default, Eq, Hash, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowProfile {
    #[default]
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

impl std::fmt::Display for WorkflowProfile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WorkflowProfile::Direct => write!(f, "Direct"),
            WorkflowProfile::Standard => write!(f, "Standard"),
            WorkflowProfile::Ultra => write!(f, "Ultra"),
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

impl Default for WorkflowPlanId {
    fn default() -> Self {
        Self::new()
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

impl Default for WorkflowLaneId {
    fn default() -> Self {
        Self::new()
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

#[derive(Clone, Copy, Debug, Default, Eq, Hash, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchPolicy {
    #[default]
    Off,
    Auto,
    Always,
}

impl std::fmt::Display for SearchPolicy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SearchPolicy::Off => write!(f, "Off"),
            SearchPolicy::Auto => write!(f, "Auto"),
            SearchPolicy::Always => write!(f, "Always"),
        }
    }
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

#[derive(Clone, Copy, Debug, Default, Eq, Hash, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BudgetProfileKind {
    Economy,
    #[default]
    Balanced,
    MaxQuality,
    Custom,
}

impl BudgetProfileKind {
    pub fn default_budget() -> BudgetConstraints {
        BudgetConstraints::default()
    }
}

impl std::fmt::Display for BudgetProfileKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BudgetProfileKind::Economy => write!(f, "Economy"),
            BudgetProfileKind::Balanced => write!(f, "Balanced"),
            BudgetProfileKind::MaxQuality => write!(f, "MaxQuality"),
            BudgetProfileKind::Custom => write!(f, "Custom"),
        }
    }
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

impl Default for RequestId {
    fn default() -> Self {
        Self::new()
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
    /// Total wall-clock the model calls took, summed across every usage record
    /// (first token to completion). Exposed so a client can show a session
    /// latency figure without re-deriving it from timestamps it does not have.
    pub total_latency_ms: u64,
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

impl Default for CredentialId {
    fn default() -> Self {
        Self::new()
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

impl Default for RouteDecisionId {
    fn default() -> Self {
        Self::new()
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

impl Default for MCPServerId {
    fn default() -> Self {
        Self::new()
    }
}

// ── Product controls (PRD §9.7, §10.5) ─────────────────────────────────

#[derive(Clone, Copy, Debug, Default, Eq, Hash, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowControl {
    #[default]
    Auto,
    Direct,
    Standard,
    Ultra,
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelRoutingControl {
    Fixed,
    #[default]
    Auto,
    Economy,
    Quality,
}

impl ModelRoutingControl {
    pub fn as_routing_profile(self) -> RoutingProfile {
        match self {
            Self::Fixed => RoutingProfile::Fixed,
            Self::Auto => RoutingProfile::Auto,
            Self::Economy => RoutingProfile::Economy,
            Self::Quality => RoutingProfile::Quality,
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Fixed => "Fixed",
            Self::Auto => "Auto",
            Self::Economy => "Economy",
            Self::Quality => "Quality",
        }
    }

    pub fn parse(input: &str) -> Option<Self> {
        match input.trim().to_ascii_lowercase().as_str() {
            "fixed" | "pinned" => Some(Self::Fixed),
            "auto" => Some(Self::Auto),
            "economy" | "cheap" => Some(Self::Economy),
            "quality" | "best" => Some(Self::Quality),
            _ => None,
        }
    }
}

impl WorkflowControl {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Auto => "Auto",
            Self::Direct => "Direct",
            Self::Standard => "Standard",
            Self::Ultra => "Ultra",
        }
    }

    pub fn parse(input: &str) -> Option<Self> {
        match input.trim().to_ascii_lowercase().as_str() {
            "auto" => Some(Self::Auto),
            "direct" => Some(Self::Direct),
            "standard" => Some(Self::Standard),
            "ultra" | "purrcode ultra" => Some(Self::Ultra),
            _ => None,
        }
    }

    /// The profile a forced control pins to. `Auto` returns `None`, which is
    /// what lets the classifier choose (PRD §9.7).
    pub const fn forced_profile(self) -> Option<WorkflowProfile> {
        match self {
            Self::Auto => None,
            Self::Direct => Some(WorkflowProfile::Direct),
            Self::Standard => Some(WorkflowProfile::Standard),
            Self::Ultra => Some(WorkflowProfile::Ultra),
        }
    }
}

impl SearchPolicy {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::Auto => "Auto",
            Self::Always => "Always",
        }
    }

    pub fn parse(input: &str) -> Option<Self> {
        match input.trim().to_ascii_lowercase().as_str() {
            "off" | "none" | "never" => Some(Self::Off),
            "auto" => Some(Self::Auto),
            "always" | "on" => Some(Self::Always),
            _ => None,
        }
    }

    /// Whether the runtime may reach the network for research at all.
    ///
    /// This is the single gate PRD §11.1 requires: under `Off` no online search
    /// and no network research MCP tool may run, whatever the model asks for.
    pub const fn permits_network_research(self) -> bool {
        !matches!(self, Self::Off)
    }
}

impl BudgetProfileKind {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Economy => "Economy",
            Self::Balanced => "Balanced",
            Self::MaxQuality => "Max Quality",
            Self::Custom => "Custom",
        }
    }

    pub fn parse(input: &str) -> Option<Self> {
        match input
            .trim()
            .to_ascii_lowercase()
            .replace(['-', '_'], " ")
            .as_str()
        {
            "economy" => Some(Self::Economy),
            "balanced" => Some(Self::Balanced),
            "max quality" | "maxquality" | "quality" => Some(Self::MaxQuality),
            "custom" => Some(Self::Custom),
            _ => None,
        }
    }

    /// The concrete ceiling each named profile carries.
    ///
    /// `Custom` has no built-in ceiling — its constraints come from the user's
    /// own configuration, so returning a default here would silently invent a
    /// limit the user never set.
    pub fn constraints(self) -> BudgetConstraints {
        match self {
            Self::Economy => BudgetConstraints {
                maximum_total_tokens: Some(120_000),
                maximum_model_calls: Some(24),
                maximum_search_requests: Some(2),
                maximum_mcp_calls: Some(8),
                maximum_wall_time_seconds: Some(600),
                ..BudgetConstraints::default()
            },
            Self::Balanced => BudgetConstraints {
                maximum_total_tokens: Some(600_000),
                maximum_model_calls: Some(80),
                maximum_search_requests: Some(8),
                maximum_mcp_calls: Some(40),
                maximum_wall_time_seconds: Some(2_400),
                ..BudgetConstraints::default()
            },
            Self::MaxQuality => BudgetConstraints {
                maximum_total_tokens: Some(2_400_000),
                maximum_model_calls: Some(240),
                maximum_search_requests: Some(24),
                maximum_mcp_calls: Some(120),
                maximum_wall_time_seconds: Some(7_200),
                ..BudgetConstraints::default()
            },
            Self::Custom => BudgetConstraints::default(),
        }
    }
}

// ── Task and permission modes (PRD §8.1, §8.3) ─────────────────────────

/// What the user is asking PurrCode to do.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskMode {
    #[default]
    Ask,
    Plan,
    Build,
    Review,
}

impl TaskMode {
    pub const ALL: &'static [Self] = &[Self::Ask, Self::Plan, Self::Build, Self::Review];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Ask => "Ask",
            Self::Plan => "Plan",
            Self::Build => "Build",
            Self::Review => "Review",
        }
    }

    pub fn parse(input: &str) -> Option<Self> {
        match input.trim().to_ascii_lowercase().as_str() {
            "ask" => Some(Self::Ask),
            "plan" => Some(Self::Plan),
            "build" => Some(Self::Build),
            "review" => Some(Self::Review),
            _ => None,
        }
    }

    /// True for the modes that must not write to the repository.
    pub const fn read_only(self) -> bool {
        matches!(self, Self::Ask | Self::Plan | Self::Review)
    }
}

impl std::fmt::Display for TaskMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

/// How much PurrCode may do before asking.
///
/// Full Access permits everything the PurrCode process and its connected
/// identities can already do. It creates no new operating-system, network,
/// GitHub or cloud permission, and it does not disable identity, action
/// binding, effect tracking, validation or recovery (PRD §8.3, §29).
#[derive(Clone, Copy, Debug, Default, Eq, Hash, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionMode {
    Ask,
    /// PawGate auto-approves governed actions without a prompt (the default).
    #[default]
    Auto,
    FullAccess,
}

impl PermissionMode {
    pub const ALL: &'static [Self] = &[Self::Ask, Self::Auto, Self::FullAccess];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Ask => "Ask",
            Self::Auto => "Auto",
            Self::FullAccess => "Full Access",
        }
    }

    pub fn parse(input: &str) -> Option<Self> {
        match input
            .trim()
            .to_ascii_lowercase()
            .replace(['-', '_'], " ")
            .as_str()
        {
            "ask" | "governed" => Some(Self::Ask),
            "auto" | "elevated" => Some(Self::Auto),
            "full access" | "fullaccess" | "unrestricted" => Some(Self::FullAccess),
            _ => None,
        }
    }

    /// The authority vocabulary the runtime persists.
    pub const fn authority_mode(self) -> &'static str {
        match self {
            Self::Ask => "governed",
            Self::Auto => "elevated",
            Self::FullAccess => "unrestricted",
        }
    }
}

impl std::fmt::Display for PermissionMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

// ── Execution style (PRD §8.2) ─────────────────────────────────────────

/// How much of each stage the user wants to steer.
///
/// This is not a permission mode: `Autonomous` does not grant authority, it
/// only says PurrCode should keep going to completion rather than stopping to
/// confirm each stage. PawGate still gates every effect.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, JsonSchema, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionStyle {
    Collaborative,
    #[default]
    Autonomous,
}

impl ExecutionStyle {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Collaborative => "Collaborative",
            Self::Autonomous => "Autonomous",
        }
    }

    pub fn parse(input: &str) -> Option<Self> {
        match input.trim().to_ascii_lowercase().as_str() {
            "collaborative" | "guided" => Some(Self::Collaborative),
            "autonomous" | "auto" => Some(Self::Autonomous),
            _ => None,
        }
    }

    /// True when PurrCode should pause after each major stage for direction.
    pub const fn pauses_between_stages(self) -> bool {
        matches!(self, Self::Collaborative)
    }
}

impl std::fmt::Display for ExecutionStyle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

// ── Workflow plan (PRD §9.4) ───────────────────────────────────────────

/// The bounded execution shape chosen for one task.
///
/// A plan is produced before any lane runs and is durable, so "why did it fan
/// out?" has an answer that does not depend on re-running the classifier.
#[derive(Clone, Debug, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct WorkflowPlan {
    pub plan_id: WorkflowPlanId,
    pub profile: WorkflowProfile,
    pub objective: String,
    pub lanes: Vec<WorkflowLane>,
    pub dependencies: Vec<WorkflowDependency>,
    pub budgets: WorkflowBudgets,
    pub search_policy: SearchPolicy,
    pub completion_condition: CompletionCondition,
}

impl WorkflowBudgets {
    /// PRD §9.2 default limits. Ultra is bounded by construction; Direct and
    /// Standard are narrower still.
    pub fn for_profile(profile: WorkflowProfile) -> Self {
        match profile {
            WorkflowProfile::Direct => Self {
                max_active_lanes: 1,
                max_depth: 1,
                max_repair_cycles: 3,
                max_parallel_writers_per_scope: 1,
            },
            WorkflowProfile::Standard => Self {
                max_active_lanes: 2,
                max_depth: 2,
                max_repair_cycles: 3,
                max_parallel_writers_per_scope: 1,
            },
            WorkflowProfile::Ultra => Self {
                max_active_lanes: 5,
                max_depth: 2,
                max_repair_cycles: 4,
                max_parallel_writers_per_scope: 1,
            },
        }
    }
}

/// How a lane is going. Distinct from `ActivityStatus`: a lane can be `Blocked`
/// on a dependency without needing a person.
#[derive(
    Clone, Copy, Debug, Eq, Hash, JsonSchema, Ord, PartialEq, PartialOrd, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowLaneStatus {
    Pending,
    Running,
    Done,
    Failed,
    Skipped,
    Cancelled,
}

impl WorkflowLaneStatus {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::Done => "done",
            Self::Failed => "failed",
            Self::Skipped => "skipped",
            Self::Cancelled => "cancelled",
        }
    }

    pub const fn glyph(self, unicode: bool) -> &'static str {
        match (self, unicode) {
            (Self::Done, true) => "✓",
            (Self::Done, false) => "[x]",
            (Self::Running, true) => "●",
            (Self::Running, false) => "[>]",
            (Self::Pending, true) => "○",
            (Self::Pending, false) => "[ ]",
            (Self::Failed, true) => "✗",
            (Self::Failed, false) => "[f]",
            (Self::Skipped, _) => "[-]",
            (Self::Cancelled, true) => "⊘",
            (Self::Cancelled, false) => "[c]",
        }
    }

    pub const fn finished(self) -> bool {
        !matches!(self, Self::Pending | Self::Running)
    }
}

// ── Workflow artifacts (PRD §9.6) ──────────────────────────────────────

/// What one workflow hands to another.
///
/// Lanes never exchange whole conversations — that is how a bounded fan-out
/// turns into N copies of the same context. They exchange typed, sized,
/// digest-identified artifacts.
#[derive(
    Clone, Copy, Debug, Eq, Hash, JsonSchema, Ord, PartialEq, PartialOrd, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactKind {
    RepositoryMap,
    TaskPlan,
    SymbolList,
    ApiContract,
    ResearchFindings,
    PatchSummary,
    FailureSummary,
    ValidationEvidence,
}

/// How far an artifact's contents may be trusted.
///
/// Search results and MCP output are `Untrusted` by construction: they are
/// external input, not instructions (PRD §11.5).
#[derive(
    Clone, Copy, Debug, Eq, Hash, JsonSchema, Ord, PartialEq, PartialOrd, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum TrustClass {
    /// Produced by the runtime from the local repository.
    RepositoryEvidence,
    /// Produced by a model inside a lane.
    ModelDerived,
    /// Came from outside the machine. Never executed without inspection.
    Untrusted,
}

#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct WorkflowArtifact {
    pub artifact_id: Uuid,
    pub kind: ArtifactKind,
    /// The lane that produced it, when a lane did.
    #[serde(default)]
    pub source_lane: Option<WorkflowLaneId>,
    pub source: String,
    pub created_at: DateTime<Utc>,
    /// The file or module scope the artifact describes.
    pub scope: Vec<String>,
    pub size_bytes: u64,
    pub trust: TrustClass,
    pub digest: String,
    /// The bounded summary a downstream lane actually reads.
    pub summary: String,
}

// ── Research decision (PRD §11.4) ──────────────────────────────────────

/// A pointer to something already checked locally.
///
/// The point of recording these is that "we searched" has to be justified by
/// what was looked at first — PRD §11.3 forbids searching merely because a task
/// is long or a model says it is unsure.
#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct EvidenceRef {
    pub kind: String,
    pub locator: String,
    #[serde(default)]
    pub digest: Option<String>,
}

#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct ResearchDecision {
    pub search_policy: SearchPolicy,
    #[serde(default)]
    pub trigger: Option<ResearchTrigger>,
    pub local_evidence_checked: Vec<EvidenceRef>,
    pub query_budget: u32,
    pub token_budget: u64,
    pub allowed_sources: Vec<SourcePolicy>,
}

impl ResearchDecision {
    /// The decision a policy makes on its own, before any trigger is seen.
    pub fn refused(policy: SearchPolicy) -> Self {
        Self {
            search_policy: policy,
            trigger: None,
            local_evidence_checked: Vec::new(),
            query_budget: 0,
            token_budget: 0,
            allowed_sources: Vec::new(),
        }
    }

    /// True when this decision actually authorises a network search.
    ///
    /// `Off` can never authorise one, and `Auto` needs an evidence-based
    /// trigger — a policy alone is not a reason to go online.
    pub fn permits_search(&self) -> bool {
        match self.search_policy {
            SearchPolicy::Off => false,
            SearchPolicy::Auto => self.trigger.is_some() && self.query_budget > 0,
            SearchPolicy::Always => self.query_budget > 0,
        }
    }
}

// ── Session controls (PRD §9.7, §10.5, §11.1, §12.3) ───────────────────

/// The adaptive controls a user sets per session.
///
/// These live on the session, not in a client, so the TUI and the IDE cannot
/// disagree about whether search is off (PRD §7).
#[derive(Clone, Debug, Default, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct SessionControls {
    pub workflow: WorkflowControl,
    pub routing: ModelRoutingControl,
    pub budget_profile: BudgetProfileKind,
    pub execution_style: ExecutionStyle,
    /// What the user is asking for. Shared here rather than kept per client so
    /// the TUI and the IDE cannot disagree about the mode of one session
    /// (PRD §7).
    #[serde(default = "legacy_task_mode")]
    pub task_mode: TaskMode,
    /// How much PurrCode may do without asking. This is a *request* from an
    /// authenticated human; the runtime maps it onto its authority mode, and
    /// the model can never set it (PRD §8.3). Legacy sessions recorded before
    /// permission modes existed keep `Ask`; new code defaults to `Auto`.
    #[serde(default = "legacy_permission_mode")]
    pub permission_mode: PermissionMode,
    /// `None` means "follow the workflow profile's default" (PRD §11.2). An
    /// explicit value is a user override and always wins.
    #[serde(default)]
    pub search_policy: Option<SearchPolicy>,
    /// Only meaningful when `budget_profile` is `Custom`.
    #[serde(default)]
    pub custom_budget: Option<BudgetConstraints>,
}

// Sessions written before task mode existed were implementation sessions.
// Preserve that replay meaning while making newly constructed controls use
// the safer Ask default. New clients always persist the explicit effective
// mode, so this function is only a legacy event migration boundary.
fn legacy_task_mode() -> TaskMode {
    TaskMode::Build
}

fn legacy_permission_mode() -> PermissionMode {
    PermissionMode::Ask
}

impl SessionControls {
    /// The search policy actually in force for a chosen profile.
    pub fn effective_search_policy(&self, profile: WorkflowProfile) -> SearchPolicy {
        self.search_policy
            .unwrap_or_else(|| profile.default_search_policy())
    }

    pub fn effective_budget(&self) -> BudgetConstraints {
        match (self.budget_profile, &self.custom_budget) {
            (BudgetProfileKind::Custom, Some(custom)) => custom.clone(),
            (kind, _) => kind.constraints(),
        }
    }
}

// ── Adaptive decision engine (PRD §9.3–9.5, §10.4, §12.3) ─────────────

/// Repository facts used to choose a workflow. The classifier deliberately
/// receives inspection evidence rather than the prompt alone; callers can
/// persist this value alongside the decision for an explainable route.
#[derive(Clone, Debug, Default, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct TaskEvidence {
    pub affected_module_count: usize,
    pub language_count: usize,
    pub requested_artifact_count: usize,
    pub migration_required: bool,
    pub external_api_dependency: bool,
    pub uncertainty_from_inspection: bool,
    pub baseline_test_failure: bool,
    pub multiple_deliverables: bool,
    pub user_quality_high: bool,
    pub inspection_complete: bool,
}

impl TaskEvidence {
    /// A conservative seed for callers that have not yet inspected a
    /// repository. It is intentionally only a hint; an incomplete inspection
    /// cannot select Direct unless the user explicitly forces it.
    pub fn from_objective(objective: &str) -> Self {
        let normalized = objective.to_ascii_lowercase();
        let migration_required = ["migrat", "upgrade", "port", "move from"]
            .iter()
            .any(|word| normalized.contains(word));
        let external_api_dependency = ["api", "sdk", "dependency", "package", "library"]
            .iter()
            .any(|word| normalized.contains(word));
        let multiple_deliverables = [" and ", " plus ", " as well as ", ";"]
            .iter()
            .any(|word| normalized.contains(word));
        Self {
            requested_artifact_count: usize::from(multiple_deliverables) + 1,
            migration_required,
            external_api_dependency,
            multiple_deliverables,
            inspection_complete: false,
            ..Self::default()
        }
    }

    fn signals(&self) -> Vec<ComplexitySignal> {
        let mut signals = Vec::new();
        if self.affected_module_count > 0 {
            signals.push(ComplexitySignal::AffectedModuleCount(
                self.affected_module_count,
            ));
        }
        if self.language_count > 0 {
            signals.push(ComplexitySignal::LanguageCount(self.language_count));
        }
        if self.requested_artifact_count > 0 {
            signals.push(ComplexitySignal::RequestedArtifactCount(
                self.requested_artifact_count,
            ));
        }
        if self.migration_required {
            signals.push(ComplexitySignal::MigrationRequired);
        }
        if self.external_api_dependency {
            signals.push(ComplexitySignal::ExternalApiDependency);
        }
        if self.uncertainty_from_inspection {
            signals.push(ComplexitySignal::UncertaintyFromInspection);
        }
        if self.baseline_test_failure {
            signals.push(ComplexitySignal::BaselineTestFailure);
        }
        if self.multiple_deliverables {
            signals.push(ComplexitySignal::MultipleDeliverables);
        }
        if self.user_quality_high {
            signals.push(ComplexitySignal::UserQualityHigh);
        }
        signals
    }
}

/// Classify a task and select its default search and budget policies.
///
/// Forced controls are applied after evidence is scored, so `Direct` really
/// does prohibit fan-out and `SearchPolicy::Off` survives an Ultra selection.
pub fn classify_task(evidence: &TaskEvidence, controls: &SessionControls) -> ComplexityDecision {
    let signals = evidence.signals();
    let score = usize::from(evidence.affected_module_count >= 2) * 2
        + usize::from(evidence.language_count >= 2)
        + usize::from(evidence.requested_artifact_count >= 3)
        + usize::from(evidence.migration_required) * 2
        + usize::from(evidence.external_api_dependency)
        + usize::from(evidence.uncertainty_from_inspection) * 2
        + usize::from(evidence.baseline_test_failure)
        + usize::from(evidence.multiple_deliverables)
        + usize::from(evidence.user_quality_high);
    let complexity = if !evidence.inspection_complete && score == 0 {
        TaskComplexity::Unknown
    } else if score >= 4 {
        TaskComplexity::Complex
    } else if score >= 2 {
        TaskComplexity::Moderate
    } else {
        TaskComplexity::Simple
    };
    let inferred = match complexity {
        TaskComplexity::Simple => WorkflowProfile::Direct,
        TaskComplexity::Moderate => WorkflowProfile::Standard,
        TaskComplexity::Complex | TaskComplexity::Unknown => WorkflowProfile::Ultra,
    };
    let selected_workflow = controls.workflow.forced_profile().unwrap_or(inferred);
    ComplexityDecision {
        complexity,
        evidence: signals,
        selected_workflow,
        selected_search_policy: controls.effective_search_policy(selected_workflow),
        selected_budget: controls.budget_profile,
    }
}

fn lane(
    kind: WorkflowLaneKind,
    objective: &str,
    allowed_tools: &[&str],
    read_scope: &[&str],
    write_scope: &[&str],
    token_budget: u64,
    dependencies: &[WorkflowLaneId],
) -> WorkflowLane {
    WorkflowLane {
        lane_id: WorkflowLaneId::new(),
        kind,
        objective: objective.to_owned(),
        allowed_tools: allowed_tools
            .iter()
            .map(|value| (*value).to_owned())
            .collect(),
        read_scope: read_scope.iter().map(|value| (*value).to_owned()).collect(),
        write_scope: write_scope
            .iter()
            .map(|value| (*value).to_owned())
            .collect(),
        token_budget,
        wall_time_budget_seconds: 600,
        completion_condition: "recorded evidence satisfies the lane objective".to_owned(),
        dependencies: dependencies.to_vec(),
    }
}

/// Build the bounded, typed lane graph used by the daemon coordinator.
pub fn build_workflow_plan(
    objective: impl Into<String>,
    decision: &ComplexityDecision,
) -> Result<WorkflowPlan, String> {
    let objective = objective.into();
    let profile = decision.selected_workflow;
    let per_lane_budget = match decision.selected_budget {
        BudgetProfileKind::Economy => 24_000,
        BudgetProfileKind::Balanced => 64_000,
        BudgetProfileKind::MaxQuality => 128_000,
        BudgetProfileKind::Custom => 64_000,
    };
    let mut lanes = Vec::new();
    let mut dependencies = Vec::new();
    match profile {
        WorkflowProfile::Direct => {
            // Direct is deliberately a single bounded lane. Validation for a
            // mutating direct run is owned by the agent's validation runtime,
            // so it must not be represented as a second lane that exceeds the
            // profile's max-active-lanes budget.
            let implementation = lane(
                WorkflowLaneKind::Implementation,
                &objective,
                &["repository_read", "write_file", "command"],
                &["repository"],
                &["repository"],
                per_lane_budget,
                &[],
            );
            lanes.push(implementation);
        }
        WorkflowProfile::Standard => {
            let implementation = lane(
                WorkflowLaneKind::Implementation,
                &objective,
                &["repository_read", "write_file", "command"],
                &["repository"],
                &["repository"],
                per_lane_budget,
                &[],
            );
            let implementation_id = implementation.lane_id;
            let validation = lane(
                WorkflowLaneKind::Validation,
                "Independently validate the implementation and report failures",
                &["repository_read", "command"],
                &["repository"],
                &[],
                per_lane_budget / 2,
                &[implementation_id],
            );
            dependencies.push(WorkflowDependency {
                from: implementation_id,
                to: validation.lane_id,
                artifact_kind: "patch_summary".to_owned(),
            });
            lanes.extend([implementation, validation]);
        }
        WorkflowProfile::Ultra => {
            let analysis = lane(
                WorkflowLaneKind::RepositoryAnalysis,
                "Map affected modules, symbols, build systems, and risks",
                &["repository_read"],
                &["repository"],
                &[],
                per_lane_budget / 2,
                &[],
            );
            let analysis_id = analysis.lane_id;
            let research = if decision.selected_search_policy.permits_network_research() {
                Some(lane(
                    WorkflowLaneKind::Research,
                    "Research only evidence-triggered external facts",
                    &["external_tool"],
                    &["repository"],
                    &[],
                    per_lane_budget / 2,
                    &[analysis_id],
                ))
            } else {
                None
            };
            let research_id = research.as_ref().map(|value| value.lane_id);
            let implementation_dependencies = research_id.into_iter().collect::<Vec<_>>();
            let implementation = lane(
                WorkflowLaneKind::Implementation,
                &objective,
                &["repository_read", "write_file", "command"],
                &["repository"],
                &["repository"],
                per_lane_budget,
                &implementation_dependencies,
            );
            let implementation_id = implementation.lane_id;
            let validation = lane(
                WorkflowLaneKind::Validation,
                "Run progressive validation and produce failure evidence",
                &["repository_read", "command"],
                &["repository"],
                &[],
                per_lane_budget / 2,
                &[implementation_id],
            );
            let review = lane(
                WorkflowLaneKind::Review,
                "Independently review the merged diff and validation evidence",
                &["repository_read"],
                &["repository"],
                &[],
                per_lane_budget / 2,
                &[validation.lane_id],
            );
            dependencies.extend([
                WorkflowDependency {
                    from: analysis_id,
                    to: implementation_id,
                    artifact_kind: "repository_map".to_owned(),
                },
                WorkflowDependency {
                    from: implementation_id,
                    to: validation.lane_id,
                    artifact_kind: "patch_summary".to_owned(),
                },
                WorkflowDependency {
                    from: validation.lane_id,
                    to: review.lane_id,
                    artifact_kind: "validation_evidence".to_owned(),
                },
            ]);
            lanes.push(analysis);
            if let Some(research) = research {
                dependencies.push(WorkflowDependency {
                    from: research.lane_id,
                    to: implementation_id,
                    artifact_kind: "research_findings".to_owned(),
                });
                lanes.push(research);
            }
            lanes.extend([implementation, validation, review]);
        }
    }
    let plan = WorkflowPlan {
        plan_id: WorkflowPlanId::new(),
        profile,
        objective,
        lanes,
        dependencies,
        budgets: WorkflowBudgets::for_profile(profile),
        search_policy: decision.selected_search_policy,
        completion_condition: CompletionCondition::AllLanesDone,
    };
    validate_workflow_plan(&plan).map(|()| plan)
}

/// Validate the two safety properties that cannot be left to the coordinator:
/// bounded lane count and exclusive ownership of overlapping write scopes.
pub fn validate_workflow_plan(plan: &WorkflowPlan) -> Result<(), String> {
    if plan.lanes.is_empty() {
        return Err("workflow plan must contain at least one lane".to_owned());
    }
    if plan.lanes.len() > plan.budgets.max_active_lanes as usize {
        return Err(format!(
            "workflow has {} lanes but the profile permits {}",
            plan.lanes.len(),
            plan.budgets.max_active_lanes
        ));
    }
    let ids: BTreeSet<_> = plan.lanes.iter().map(|lane| lane.lane_id).collect();
    if ids.len() != plan.lanes.len() {
        return Err("workflow lane IDs must be unique".to_owned());
    }
    for lane in &plan.lanes {
        if lane
            .dependencies
            .iter()
            .any(|dependency| !ids.contains(dependency))
        {
            return Err(format!("lane {:?} has an unknown dependency", lane.lane_id));
        }
    }
    let mut owners: BTreeMap<String, WorkflowLaneId> = BTreeMap::new();
    for lane in &plan.lanes {
        for scope in &lane.write_scope {
            if let Some(owner) = owners.insert(scope.clone(), lane.lane_id) {
                return Err(format!(
                    "write scope `{scope}` is owned by both {owner:?} and {:?}",
                    lane.lane_id
                ));
            }
        }
    }
    Ok(())
}

/// A model/provider candidate supplied by the provider gateway. Raw secrets do
/// not appear here; routing only handles secure credential references.
#[derive(Clone, Debug, PartialEq)]
pub struct RouteCandidate {
    pub provider_id: String,
    pub model_id: String,
    pub credential: CredentialProfile,
    pub capabilities: ModelCapabilities,
    pub privacy_class: PrivacyClass,
    pub healthy: bool,
    pub expected_cost: Option<f64>,
    pub expected_latency_ms: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RouteRequest {
    pub profile: RoutingProfile,
    pub pinned_provider: Option<String>,
    pub pinned_model: Option<String>,
    pub required_context_tokens: u64,
    pub require_tool_calling: bool,
    pub require_structured_output: bool,
    pub privacy: PrivacyClass,
    pub remaining_cost: Option<f64>,
}

impl Default for RouteRequest {
    fn default() -> Self {
        Self {
            profile: RoutingProfile::Auto,
            pinned_provider: None,
            pinned_model: None,
            required_context_tokens: 0,
            require_tool_calling: false,
            require_structured_output: false,
            privacy: PrivacyClass::Local,
            remaining_cost: None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RouteError {
    NoQualifiedRoute,
    FixedRouteUnavailable,
    PrivacyBoundary,
    BudgetExceeded,
}

impl std::fmt::Display for RouteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::NoQualifiedRoute => "no qualified model route is available",
            Self::FixedRouteUnavailable => {
                "the fixed provider/model/credential route is unavailable"
            }
            Self::PrivacyBoundary => "the requested route crosses the configured privacy boundary",
            Self::BudgetExceeded => "all qualified routes exceed the remaining budget",
        })
    }
}

/// Pick the smallest qualified route allowed by the user's policy. Fixed mode
/// never silently changes provider, model, or credential.
pub fn select_route(
    request: &RouteRequest,
    candidates: &[RouteCandidate],
) -> Result<ModelRouteDecision, RouteError> {
    let mut eligible = candidates
        .iter()
        .filter(|candidate| candidate.healthy)
        .filter(|candidate| {
            request
                .pinned_provider
                .as_deref()
                .is_none_or(|provider| provider == candidate.provider_id)
                && request
                    .pinned_model
                    .as_deref()
                    .is_none_or(|model| model == candidate.model_id)
        })
        .filter(|candidate| {
            candidate.capabilities.context_capacity_tokens >= request.required_context_tokens
        })
        .filter(|candidate| !request.require_tool_calling || candidate.capabilities.tool_calling)
        .filter(|candidate| {
            !request.require_structured_output || candidate.capabilities.structured_output
        })
        .filter(|candidate| candidate.capabilities.coding_qualified)
        .filter(|candidate| privacy_allows(request.privacy, candidate.privacy_class))
        .filter(|candidate| {
            candidate.credential.allowed_models.is_empty()
                || candidate
                    .credential
                    .allowed_models
                    .iter()
                    .any(|pattern| model_pattern_matches(pattern, &candidate.model_id))
        })
        .filter(|candidate| candidate.credential.enabled)
        .filter(|candidate| {
            request.remaining_cost.is_none_or(|remaining| {
                candidate.expected_cost.is_none_or(|cost| cost <= remaining)
            })
        })
        .collect::<Vec<_>>();
    if eligible.is_empty() {
        if request.profile == RoutingProfile::Fixed {
            return Err(
                if candidates.iter().any(|candidate| {
                    request
                        .pinned_provider
                        .as_deref()
                        .is_none_or(|provider| provider == candidate.provider_id)
                        && request
                            .pinned_model
                            .as_deref()
                            .is_none_or(|model| model == candidate.model_id)
                }) {
                    RouteError::FixedRouteUnavailable
                } else {
                    RouteError::NoQualifiedRoute
                },
            );
        }
        if request.remaining_cost.is_some() {
            return Err(RouteError::BudgetExceeded);
        }
        return Err(
            if candidates.iter().any(|candidate| {
                candidate.healthy && !privacy_allows(request.privacy, candidate.privacy_class)
            }) {
                RouteError::PrivacyBoundary
            } else {
                RouteError::NoQualifiedRoute
            },
        );
    }
    eligible.sort_by(|left, right| {
        let by_profile = match request.profile {
            RoutingProfile::Quality => right
                .capabilities
                .context_capacity_tokens
                .cmp(&left.capabilities.context_capacity_tokens),
            RoutingProfile::Economy => left
                .expected_cost
                .unwrap_or(f64::INFINITY)
                .total_cmp(&right.expected_cost.unwrap_or(f64::INFINITY)),
            RoutingProfile::Fixed | RoutingProfile::Auto => {
                left.credential.priority.cmp(&right.credential.priority)
            }
        };
        by_profile
            .then_with(|| left.expected_latency_ms.cmp(&right.expected_latency_ms))
            .then_with(|| left.model_id.cmp(&right.model_id))
    });
    let selected = eligible.remove(0);
    let mut reasons = vec![
        RouteReason::CodingQualified,
        RouteReason::SufficientContextCapacity,
        RouteReason::ProviderHealthy,
        RouteReason::PrivacyOK,
        RouteReason::BudgetRemaining,
    ];
    if request.require_tool_calling {
        reasons.push(RouteReason::ToolCallingRequired);
    }
    if request.require_structured_output {
        reasons.push(RouteReason::StructuredOutputRequired);
    }
    if request.profile == RoutingProfile::Fixed {
        reasons.push(RouteReason::UserPinned);
    }
    Ok(ModelRouteDecision {
        decision_id: RouteDecisionId::new(),
        workflow_lane_id: None,
        provider_id: selected.provider_id.clone(),
        model_id: selected.model_id.clone(),
        credential_id: selected.credential.credential_id,
        reasons,
        expected_capabilities: selected.capabilities.clone(),
        privacy_class: selected.privacy_class,
        budget_snapshot: BudgetSnapshot {
            remaining_tokens: None,
            remaining_cost: request.remaining_cost,
            remaining_calls: None,
            rate_limits: Vec::new(),
        },
    })
}

fn privacy_allows(requested: PrivacyClass, candidate: PrivacyClass) -> bool {
    match requested {
        PrivacyClass::Local => candidate == PrivacyClass::Local,
        PrivacyClass::ApprovedRemote => matches!(
            candidate,
            PrivacyClass::Local | PrivacyClass::ApprovedRemote
        ),
        PrivacyClass::UnrestrictedRemote => true,
    }
}

fn model_pattern_matches(pattern: &str, model: &str) -> bool {
    pattern == "*"
        || pattern == model
        || pattern
            .strip_suffix('*')
            .is_some_and(|prefix| model.starts_with(prefix))
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BudgetExhaustion {
    InputTokens,
    OutputTokens,
    TotalTokens,
    EstimatedCost,
    ModelCalls,
    SearchRequests,
    McpCalls,
    WallTime,
}

impl std::fmt::Display for BudgetExhaustion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::InputTokens => "input tokens",
            Self::OutputTokens => "output tokens",
            Self::TotalTokens => "total tokens",
            Self::EstimatedCost => "estimated cost",
            Self::ModelCalls => "model calls",
            Self::SearchRequests => "search requests",
            Self::McpCalls => "MCP calls",
            Self::WallTime => "wall time",
        })
    }
}

/// In-memory ledger used by the daemon before a `UsageRecorded` event is
/// appended. It is also useful for token-regression tests without a provider.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UsageLedger {
    pub records: Vec<UsageRecord>,
    /// Tokens consumed per provider, so failover can prefer providers that
    /// still have room under a per-provider cap.
    per_provider: BTreeMap<String, u64>,
    /// Model calls per provider.
    provider_calls: BTreeMap<String, u32>,
}

impl UsageLedger {
    /// Build a ledger from existing records, populating the per-provider
    /// accounting by folding them through `record`.
    pub fn from_records(records: Vec<UsageRecord>) -> Self {
        let budget = BudgetConstraints::default();
        let mut ledger = UsageLedger::default();
        for record in records {
            let _ = ledger.record(&budget, record);
        }
        ledger
    }

    pub fn can_record(
        &self,
        budget: &BudgetConstraints,
        record: &UsageRecord,
    ) -> Result<(), BudgetExhaustion> {
        let input = self.input_tokens().saturating_add(record.input_tokens);
        let output = self.output_tokens().saturating_add(record.output_tokens);
        let total = input.saturating_add(output);
        let calls = self.records.len() as u32 + 1;
        let searches = self
            .search_requests()
            .saturating_add(record.search_requests);
        let mcp = self.mcp_calls().saturating_add(record.mcp_calls);
        let cost =
            self.estimated_cost().unwrap_or_default() + record.estimated_cost.unwrap_or_default();
        if budget
            .maximum_input_tokens
            .is_some_and(|limit| input > limit)
        {
            return Err(BudgetExhaustion::InputTokens);
        }
        if budget
            .maximum_output_tokens
            .is_some_and(|limit| output > limit)
        {
            return Err(BudgetExhaustion::OutputTokens);
        }
        if budget
            .maximum_total_tokens
            .is_some_and(|limit| total > limit)
        {
            return Err(BudgetExhaustion::TotalTokens);
        }
        if budget
            .maximum_estimated_cost
            .is_some_and(|limit| cost > limit)
        {
            return Err(BudgetExhaustion::EstimatedCost);
        }
        if budget
            .maximum_model_calls
            .is_some_and(|limit| calls > limit)
        {
            return Err(BudgetExhaustion::ModelCalls);
        }
        if budget
            .maximum_search_requests
            .is_some_and(|limit| searches > limit)
        {
            return Err(BudgetExhaustion::SearchRequests);
        }
        if budget.maximum_mcp_calls.is_some_and(|limit| mcp > limit) {
            return Err(BudgetExhaustion::McpCalls);
        }
        Ok(())
    }

    pub fn record(
        &mut self,
        budget: &BudgetConstraints,
        record: UsageRecord,
    ) -> Result<(), BudgetExhaustion> {
        self.can_record(budget, &record)?;
        *self
            .per_provider
            .entry(record.provider_id.clone())
            .or_insert(0) += record.input_tokens + record.output_tokens;
        *self
            .provider_calls
            .entry(record.provider_id.clone())
            .or_insert(0) += 1;
        self.records.push(record);
        Ok(())
    }

    /// Total tokens recorded for one provider.
    pub fn provider_tokens(&self, provider: &str) -> u64 {
        self.per_provider.get(provider).copied().unwrap_or(0)
    }

    /// Model calls recorded for one provider.
    pub fn provider_call_count(&self, provider: &str) -> u32 {
        self.provider_calls.get(provider).copied().unwrap_or(0)
    }

    pub fn input_tokens(&self) -> u64 {
        self.records.iter().map(|record| record.input_tokens).sum()
    }
    pub fn output_tokens(&self) -> u64 {
        self.records.iter().map(|record| record.output_tokens).sum()
    }
    pub fn search_requests(&self) -> u32 {
        self.records
            .iter()
            .map(|record| record.search_requests)
            .sum()
    }
    pub fn mcp_calls(&self) -> u32 {
        self.records.iter().map(|record| record.mcp_calls).sum()
    }
    pub fn estimated_cost(&self) -> Option<f64> {
        let mut total = 0.0;
        let mut observed = false;
        for record in &self.records {
            if let Some(cost) = record.estimated_cost {
                total += cost;
                observed = true;
            }
        }
        observed.then_some(total)
    }

    pub fn summary(&self, validated_changes: usize) -> UsageSummary {
        let input = self.input_tokens();
        let output = self.output_tokens();
        let total = input.saturating_add(output);
        UsageSummary {
            total_tokens: total,
            input_tokens: input,
            output_tokens: output,
            cache_read_tokens: self
                .records
                .iter()
                .map(|record| record.cache_read_tokens)
                .sum(),
            cache_write_tokens: self
                .records
                .iter()
                .map(|record| record.cache_write_tokens)
                .sum(),
            search_requests: self.search_requests(),
            mcp_calls: self.mcp_calls(),
            estimated_total_cost: self.estimated_cost(),
            model_call_count: self.records.len() as u32,
            tokens_per_validated_change: (validated_changes > 0)
                .then_some(total as f64 / validated_changes as f64),
            context_selection_ratio: None,
            retry_token_share: None,
            total_latency_ms: self.records.iter().map(|record| record.latency_ms).sum(),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::SessionId;

    use super::*;

    #[test]
    fn a_forced_workflow_pins_the_profile_and_auto_leaves_it_to_the_classifier() {
        assert_eq!(WorkflowControl::Auto.forced_profile(), None);
        assert_eq!(
            WorkflowControl::Direct.forced_profile(),
            Some(WorkflowProfile::Direct)
        );
        assert_eq!(
            WorkflowControl::Ultra.forced_profile(),
            Some(WorkflowProfile::Ultra)
        );
    }

    #[test]
    fn direct_defaults_to_no_search_and_the_others_to_auto() {
        assert_eq!(
            WorkflowProfile::Direct.default_search_policy(),
            SearchPolicy::Off
        );
        assert_eq!(
            WorkflowProfile::Standard.default_search_policy(),
            SearchPolicy::Auto
        );
        assert_eq!(
            WorkflowProfile::Ultra.default_search_policy(),
            SearchPolicy::Auto
        );
    }

    #[test]
    fn a_user_search_setting_overrides_the_profile_default() {
        let mut controls = SessionControls::default();
        assert_eq!(
            controls.effective_search_policy(WorkflowProfile::Standard),
            SearchPolicy::Auto
        );
        controls.search_policy = Some(SearchPolicy::Off);
        // "Do not search online" has to survive an Ultra classification.
        assert_eq!(
            controls.effective_search_policy(WorkflowProfile::Ultra),
            SearchPolicy::Off
        );
    }

    #[test]
    fn search_off_can_never_authorise_a_search() {
        let mut decision = ResearchDecision::refused(SearchPolicy::Off);
        decision.trigger = Some(ResearchTrigger::ExplicitUserRequest);
        decision.query_budget = 5;
        assert!(
            !decision.permits_search(),
            "Off must be enforced even with a trigger and a budget"
        );
        assert!(!SearchPolicy::Off.permits_network_research());
    }

    #[test]
    fn auto_needs_a_trigger_not_just_a_policy() {
        let mut decision = ResearchDecision::refused(SearchPolicy::Auto);
        decision.query_budget = 3;
        assert!(
            !decision.permits_search(),
            "Auto without evidence must not search"
        );
        decision.trigger = Some(ResearchTrigger::UnknownExternalApi);
        assert!(decision.permits_search());
    }

    #[test]
    fn always_still_respects_its_query_budget() {
        let mut decision = ResearchDecision::refused(SearchPolicy::Always);
        assert!(!decision.permits_search(), "Always is not unbounded");
        decision.query_budget = 1;
        assert!(decision.permits_search());
    }

    #[test]
    fn ultra_stays_inside_the_prd_9_2_limits() {
        let budgets = WorkflowBudgets::for_profile(WorkflowProfile::Ultra);
        assert_eq!(budgets.max_active_lanes, 5);
        assert_eq!(budgets.max_depth, 2);
        assert_eq!(budgets.max_parallel_writers_per_scope, 1);
    }

    #[test]
    fn direct_cannot_fan_out_at_all() {
        assert_eq!(
            WorkflowBudgets::for_profile(WorkflowProfile::Direct).max_active_lanes,
            1
        );
    }

    #[test]
    fn named_budget_profiles_carry_real_ceilings() {
        for kind in [
            BudgetProfileKind::Economy,
            BudgetProfileKind::Balanced,
            BudgetProfileKind::MaxQuality,
        ] {
            let constraints = kind.constraints();
            assert!(
                constraints.maximum_total_tokens.is_some(),
                "{kind:?} has no token ceiling"
            );
            assert!(constraints.maximum_model_calls.is_some(), "{kind:?}");
        }
        // Economy must actually be cheaper than Max Quality, or the control is
        // decorative.
        assert!(
            BudgetProfileKind::Economy
                .constraints()
                .maximum_total_tokens
                < BudgetProfileKind::MaxQuality
                    .constraints()
                    .maximum_total_tokens
        );
    }

    #[test]
    fn a_custom_budget_never_invents_a_limit_the_user_did_not_set() {
        let controls = SessionControls {
            budget_profile: BudgetProfileKind::Custom,
            ..SessionControls::default()
        };
        assert_eq!(controls.effective_budget(), BudgetConstraints::default());
    }

    #[test]
    fn economy_routing_is_not_silently_treated_as_fixed() {
        assert_eq!(
            ModelRoutingControl::Economy.as_routing_profile(),
            RoutingProfile::Economy
        );
        assert_eq!(
            ModelRoutingControl::Fixed.as_routing_profile(),
            RoutingProfile::Fixed
        );
    }

    #[test]
    fn controls_parse_the_words_a_user_actually_types() {
        assert_eq!(
            WorkflowControl::parse("ULTRA"),
            Some(WorkflowControl::Ultra)
        );
        assert_eq!(SearchPolicy::parse(" off "), Some(SearchPolicy::Off));
        assert_eq!(
            BudgetProfileKind::parse("max-quality"),
            Some(BudgetProfileKind::MaxQuality)
        );
        assert_eq!(
            ExecutionStyle::parse("collaborative"),
            Some(ExecutionStyle::Collaborative)
        );
        assert_eq!(WorkflowControl::parse("swarm"), None);
    }

    #[test]
    fn lane_status_has_a_monochrome_glyph_and_a_word() {
        for status in [
            WorkflowLaneStatus::Pending,
            WorkflowLaneStatus::Running,
            WorkflowLaneStatus::Done,
            WorkflowLaneStatus::Failed,
            WorkflowLaneStatus::Skipped,
            WorkflowLaneStatus::Cancelled,
        ] {
            assert!(!status.label().is_empty());
            assert!(status.glyph(false).is_ascii());
        }
        assert!(!WorkflowLaneStatus::Running.finished());
        assert!(WorkflowLaneStatus::Cancelled.finished());
    }

    #[test]
    fn session_controls_round_trip() {
        let controls = SessionControls {
            workflow: WorkflowControl::Ultra,
            routing: ModelRoutingControl::Quality,
            budget_profile: BudgetProfileKind::Economy,
            execution_style: ExecutionStyle::Collaborative,
            task_mode: TaskMode::Review,
            permission_mode: PermissionMode::FullAccess,
            search_policy: Some(SearchPolicy::Off),
            custom_budget: None,
        };
        let encoded = serde_json::to_string(&controls).unwrap();
        assert_eq!(
            serde_json::from_str::<SessionControls>(&encoded).unwrap(),
            controls
        );
    }

    #[test]
    fn controls_written_before_task_and_permission_modes_existed_still_load() {
        // A session recorded by an older build must keep replaying, so the two
        // new fields have to be optional on the wire.
        let older = r#"{"workflow":"direct","routing":"auto","budget_profile":"balanced","execution_style":"autonomous"}"#;
        let controls: SessionControls = serde_json::from_str(older).unwrap();
        assert_eq!(controls.task_mode, TaskMode::Build);
        assert_eq!(controls.permission_mode, PermissionMode::Ask);
    }

    #[test]
    fn new_controls_default_to_ask_without_rewriting_legacy_sessions() {
        assert_eq!(SessionControls::default().task_mode, TaskMode::Ask);
    }

    #[test]
    fn new_controls_default_to_auto_permission_while_legacy_stays_ask() {
        // New code auto-approves governed actions (the product default).
        assert_eq!(
            SessionControls::default().permission_mode,
            PermissionMode::Auto
        );
        // A session recorded before permission modes existed must keep
        // replaying as governed, not silently upgrade to Auto.
        let older = r#"{"workflow":"direct","routing":"auto","budget_profile":"balanced","execution_style":"autonomous"}"#;
        let controls: SessionControls = serde_json::from_str(older).unwrap();
        assert_eq!(controls.permission_mode, PermissionMode::Ask);
    }

    #[test]
    fn the_read_only_task_modes_are_the_ones_that_must_not_write() {
        assert!(TaskMode::Ask.read_only());
        assert!(TaskMode::Plan.read_only());
        assert!(TaskMode::Review.read_only());
        assert!(!TaskMode::Build.read_only());
    }

    #[test]
    fn permission_modes_map_onto_the_runtime_authority_vocabulary() {
        assert_eq!(PermissionMode::Ask.authority_mode(), "governed");
        assert_eq!(PermissionMode::Auto.authority_mode(), "elevated");
        assert_eq!(PermissionMode::FullAccess.authority_mode(), "unrestricted");
        // Both vocabularies must parse, because the daemon has persisted the
        // authority words since v0.9.
        assert_eq!(
            PermissionMode::parse("unrestricted"),
            Some(PermissionMode::FullAccess)
        );
        assert_eq!(
            PermissionMode::parse("Full Access"),
            Some(PermissionMode::FullAccess)
        );
        assert_eq!(PermissionMode::parse("root"), None);
    }

    #[test]
    fn evidence_selects_the_smallest_sufficient_workflow() {
        let controls = SessionControls::default();
        let simple = TaskEvidence {
            inspection_complete: true,
            requested_artifact_count: 1,
            ..TaskEvidence::default()
        };
        assert_eq!(
            classify_task(&simple, &controls).selected_workflow,
            WorkflowProfile::Direct
        );
        let moderate = TaskEvidence {
            inspection_complete: true,
            affected_module_count: 2,
            language_count: 1,
            ..simple.clone()
        };
        assert_eq!(
            classify_task(&moderate, &controls).selected_workflow,
            WorkflowProfile::Standard
        );
        let complex = TaskEvidence {
            inspection_complete: true,
            affected_module_count: 8,
            migration_required: true,
            external_api_dependency: true,
            ..simple
        };
        assert_eq!(
            classify_task(&complex, &controls).selected_workflow,
            WorkflowProfile::Ultra
        );
    }

    #[test]
    fn ultra_plan_is_bounded_and_search_off_has_no_research_lane() {
        let decision = ComplexityDecision {
            complexity: TaskComplexity::Complex,
            evidence: vec![ComplexitySignal::MigrationRequired],
            selected_workflow: WorkflowProfile::Ultra,
            selected_search_policy: SearchPolicy::Off,
            selected_budget: BudgetProfileKind::Balanced,
        };
        let plan = build_workflow_plan("migrate the API", &decision).unwrap();
        assert_eq!(plan.profile, WorkflowProfile::Ultra);
        assert!(plan.lanes.len() <= 5);
        assert!(
            !plan
                .lanes
                .iter()
                .any(|lane| lane.kind == WorkflowLaneKind::Research)
        );
        assert!(validate_workflow_plan(&plan).is_ok());
    }

    #[test]
    fn direct_plan_is_bounded_without_panicking() {
        let decision = ComplexityDecision {
            complexity: TaskComplexity::Simple,
            evidence: vec![ComplexitySignal::RequestedArtifactCount(1)],
            selected_workflow: WorkflowProfile::Direct,
            selected_search_policy: SearchPolicy::Off,
            selected_budget: BudgetProfileKind::Balanced,
        };
        let plan = build_workflow_plan("say hello", &decision).unwrap();
        assert_eq!(plan.lanes.len(), 1);
        assert!(validate_workflow_plan(&plan).is_ok());
    }

    #[test]
    fn overlapping_writer_scopes_are_rejected() {
        let decision = ComplexityDecision {
            complexity: TaskComplexity::Complex,
            evidence: Vec::new(),
            selected_workflow: WorkflowProfile::Ultra,
            selected_search_policy: SearchPolicy::Off,
            selected_budget: BudgetProfileKind::Balanced,
        };
        let mut plan = build_workflow_plan("task", &decision).unwrap();
        plan.lanes[2].write_scope = vec!["repository".into()];
        assert!(
            validate_workflow_plan(&plan)
                .unwrap_err()
                .contains("write scope")
        );
    }

    #[test]
    fn fixed_routing_does_not_fallback_and_budget_is_enforced() {
        let credential = CredentialProfile {
            credential_id: CredentialId::new(),
            provider_id: "local".into(),
            label: "primary".into(),
            secret_reference: SecretReference("keychain://purrcode/primary".into()),
            allowed_models: vec!["coder".into()],
            priority: 1,
            enabled: true,
        };
        let candidate = RouteCandidate {
            provider_id: "local".into(),
            model_id: "coder".into(),
            credential: credential.clone(),
            capabilities: ModelCapabilities {
                tool_calling: true,
                structured_output: true,
                coding_qualified: true,
                context_capacity_tokens: 32_000,
            },
            privacy_class: PrivacyClass::Local,
            healthy: true,
            expected_cost: Some(0.02),
            expected_latency_ms: 20,
        };
        let decision = select_route(
            &RouteRequest {
                profile: RoutingProfile::Fixed,
                pinned_provider: Some("remote".into()),
                pinned_model: Some("other".into()),
                ..RouteRequest::default()
            },
            &[candidate],
        );
        assert!(matches!(decision, Err(RouteError::NoQualifiedRoute)));
        let record = UsageRecord {
            request_id: RequestId::new(),
            session_id: SessionId::new(),
            workflow_lane_id: None,
            provider_id: "local".into(),
            model_id: "coder".into(),
            credential_id: credential.credential_id.0.to_string(),
            input_tokens: 10,
            output_tokens: 5,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            tool_result_tokens: 0,
            search_requests: 0,
            mcp_calls: 0,
            estimated_cost: Some(0.02),
            latency_ms: 20,
            recorded_at: Utc::now(),
        };
        let mut ledger = UsageLedger::default();
        let budget = BudgetConstraints {
            maximum_total_tokens: Some(10),
            ..BudgetConstraints::default()
        };
        assert_eq!(
            ledger.record(&budget, record).unwrap_err(),
            BudgetExhaustion::TotalTokens
        );
    }

    #[test]
    fn ledger_accounts_tokens_per_provider() {
        let mut ledger = UsageLedger::default();
        let budget = BudgetConstraints::default();
        let record = |provider: &str, input: u64, output: u64| UsageRecord {
            request_id: RequestId::new(),
            session_id: SessionId::new(),
            workflow_lane_id: None,
            provider_id: provider.into(),
            model_id: "coder".into(),
            credential_id: "daemon-managed".into(),
            input_tokens: input,
            output_tokens: output,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            tool_result_tokens: 0,
            search_requests: 0,
            mcp_calls: 0,
            estimated_cost: None,
            latency_ms: 1,
            recorded_at: Utc::now(),
        };
        ledger.record(&budget, record("deepseek", 100, 20)).unwrap();
        ledger.record(&budget, record("deepseek", 50, 10)).unwrap();
        ledger.record(&budget, record("openai", 200, 30)).unwrap();
        assert_eq!(ledger.provider_tokens("deepseek"), 180);
        assert_eq!(ledger.provider_call_count("deepseek"), 2);
        assert_eq!(ledger.provider_tokens("openai"), 230);
        assert_eq!(ledger.provider_call_count("openai"), 1);
        assert_eq!(ledger.provider_tokens("missing"), 0);

        // Rebuilding from records reproduces the per-provider accounting.
        let rebuilt = UsageLedger::from_records(ledger.records.clone());
        assert_eq!(rebuilt.provider_tokens("deepseek"), 180);
        assert_eq!(rebuilt.provider_call_count("openai"), 1);
    }
}
