//! Pure, evidence-driven local-model recommendation.
//!
//! This module never probes a provider, pulls a model, or infers model quality from its name.
//! Callers must provide observed Ollama metadata, a resource snapshot, and real qualification
//! evidence. Missing evidence is preserved in the result instead of being replaced by optimistic
//! defaults.

#![allow(clippy::collapsible_if)]

pub use crate::local_models::ResourceSnapshot;
use purrcode_provider_gateway::QualificationReport;
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::collections::BTreeSet;

const KIB: u64 = 1024;
const MIB: u64 = 1024 * KIB;
const GIB: u64 = 1024 * MIB;
const HIGH_SWAP_BYTES: u64 = 2 * GIB;
const ELEVATED_SWAP_BYTES: u64 = 512 * MIB;
const MINIMUM_QUALIFICATION_BASIS_POINTS: u16 = 8_000;
const MINIMUM_RECOMMENDATION_SCORE: u16 = 700;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityObservation {
    Verified,
    Failed,
    NotTested,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct OllamaMetadataEvidence {
    pub parameter_count: Option<u64>,
    pub quantization: Option<String>,
    pub quantization_bits: Option<u8>,
    pub context_length_tokens: Option<u64>,
    pub size_bytes: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct QualificationEvidence {
    pub passed_cases: u16,
    pub total_cases: u16,
    pub structured_json: CapabilityObservation,
    pub tool_calling: CapabilityObservation,
    pub mean_latency_ms: Option<u64>,
    pub estimated_tokens_per_second_milli: Option<u64>,
    pub maximum_reliable_context_tokens: Option<u64>,
}

impl QualificationEvidence {
    /// Captures the current provider qualification report without upgrading untested native tool
    /// calling into a pass. `tool_calling` must come from a separate observed probe.
    pub fn from_report(report: &QualificationReport, tool_calling: CapabilityObservation) -> Self {
        let passed_cases = report
            .cases
            .iter()
            .filter(|case| case.passed)
            .count()
            .min(u16::MAX as usize) as u16;
        let total_cases = report.cases.len().min(u16::MAX as usize) as u16;
        let structured_json = report
            .cases
            .iter()
            .find(|case| case.name == "structured-output")
            .map(|case| {
                if case.passed {
                    CapabilityObservation::Verified
                } else {
                    CapabilityObservation::Failed
                }
            })
            .unwrap_or(CapabilityObservation::NotTested);
        Self {
            passed_cases,
            total_cases,
            structured_json,
            tool_calling,
            mean_latency_ms: finite_nonnegative_u64(report.mean_latency_ms),
            estimated_tokens_per_second_milli: report
                .estimated_tokens_per_second
                .and_then(|value| finite_nonnegative_u64(value * 1_000.0)),
            // `qualify_model` always runs the reliable-context ladder. `None` therefore means
            // that the first rung failed, rather than that the probe was skipped.
            maximum_reliable_context_tokens: Some(
                report.maximum_reliable_context_tokens.unwrap_or_default(),
            ),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ModelEvidence {
    pub model: String,
    pub installed: bool,
    pub currently_loaded: bool,
    pub metadata: OllamaMetadataEvidence,
    pub qualification: Option<QualificationEvidence>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CodingAssessment {
    Strong,
    Usable,
    Weak,
    Failed,
    Unverified,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum QualificationAssessment {
    Verified,
    Failed,
    Incomplete,
    Missing,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct QualificationCard {
    pub assessment: QualificationAssessment,
    pub coding: CodingAssessment,
    pub passed_cases: Option<u16>,
    pub total_cases: Option<u16>,
    pub accuracy_basis_points: Option<u16>,
    pub structured_json: CapabilityObservation,
    pub tool_calling: CapabilityObservation,
    pub mean_latency_ms: Option<u64>,
    pub estimated_tokens_per_second_milli: Option<u64>,
    pub maximum_reliable_context_tokens: Option<u64>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MissingEvidenceCode {
    ModelSize,
    ParameterCount,
    Quantization,
    QuantizationBits,
    MetadataContext,
    Qualification,
    QualificationLatency,
    ReliableContext,
    StructuredJsonProbe,
    ToolCallingProbe,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RiskCode {
    AggressiveQuantization,
    ContextCapped,
    ElevatedMemoryPressure,
    HighLatency,
    HighMemoryPressure,
    HighSwapPressure,
    LowMemorySystem,
    ToolCallingFailed,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BlockerCode {
    EmptyModelIdentity,
    InvalidMetadata,
    InvalidQualification,
    MemoryBudgetExceeded,
    MemoryEstimateUnavailable,
    NotInstalled,
    QualificationBelowThreshold,
    QualificationLatencyMissing,
    QualificationMissing,
    RecommendationScoreBelowThreshold,
    ReliableContextBelowMinimum,
    ReliableContextMissing,
    ResourceGovernorDisabled,
    StructuredJsonFailed,
    StructuredJsonNotTested,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceNotice<T> {
    pub code: T,
    pub detail: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelRecommendationStatus {
    Recommended,
    EligibleAlternative,
    NotRecommended,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ModelQualificationCard {
    pub model: String,
    pub installed: bool,
    pub currently_loaded: bool,
    pub parameter_count: Option<u64>,
    pub quantization: Option<String>,
    pub quantization_bits: Option<u8>,
    pub metadata_context_tokens: Option<u64>,
    pub observed_size_bytes: Option<u64>,
    pub estimated_runtime_memory_bytes: Option<u64>,
    pub recommended_context_tokens: u64,
    pub qualification: QualificationCard,
    pub score: Option<u16>,
    pub status: ModelRecommendationStatus,
    pub risks: Vec<EvidenceNotice<RiskCode>>,
    pub missing_evidence: Vec<EvidenceNotice<MissingEvidenceCode>>,
    pub not_recommended_reasons: Vec<EvidenceNotice<BlockerCode>>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RecommendedLifecycle {
    UnloadAfterRequest,
    IdleTimeout,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConstraintReason {
    ConservativeDefault,
    ElevatedMemoryPressure,
    HighMemoryPressure,
    HighSwapPressure,
    LowMemorySystem,
    QualificationContextLimit,
    ResourceGovernorLimit,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RecommendationConstraints {
    pub single_model_mode: bool,
    pub context_limit_tokens: u64,
    pub maximum_parallel_requests: usize,
    pub allow_separate_local_judge: bool,
    pub lifecycle: RecommendedLifecycle,
    pub idle_timeout_seconds: Option<u64>,
    pub reasons: Vec<ConstraintReason>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NoRecommendationReason {
    EvidenceIncomplete,
    HardwareBudgetInsufficient,
    NoCandidates,
    NoEligibleModel,
    NoInstalledModels,
    QualificationInsufficient,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum RecommendationOutcome {
    Recommended {
        model: String,
        score: u16,
    },
    NoRecommendation {
        reasons: Vec<NoRecommendationReason>,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RecommendationReport {
    pub outcome: RecommendationOutcome,
    pub constraints: RecommendationConstraints,
    pub resource_risks: Vec<EvidenceNotice<RiskCode>>,
    pub cards: Vec<ModelQualificationCard>,
}

pub fn recommend_local_models(
    resources: &ResourceSnapshot,
    candidates: &[ModelEvidence],
) -> RecommendationReport {
    let pressure = MemoryPressure::from_snapshot(resources);
    let hardware_context_limit = hardware_context_limit(resources, pressure);
    let resource_risks = resource_risks(resources, pressure);
    let mut cards = candidates
        .iter()
        .map(|candidate| evaluate_candidate(resources, candidate, pressure, hardware_context_limit))
        .collect::<Vec<_>>();
    cards.sort_by(compare_cards);

    let primary_index = cards
        .iter()
        .position(|card| card.not_recommended_reasons.is_empty());
    for (index, card) in cards.iter_mut().enumerate() {
        card.status = if Some(index) == primary_index {
            ModelRecommendationStatus::Recommended
        } else if card.not_recommended_reasons.is_empty() {
            ModelRecommendationStatus::EligibleAlternative
        } else {
            ModelRecommendationStatus::NotRecommended
        };
    }

    let outcome = primary_index.map_or_else(
        || RecommendationOutcome::NoRecommendation {
            reasons: no_recommendation_reasons(candidates, &cards),
        },
        |index| RecommendationOutcome::Recommended {
            model: cards[index].model.clone(),
            score: cards[index].score.unwrap_or_default(),
        },
    );
    let primary = primary_index.and_then(|index| cards.get(index));
    let constraints = constraints(resources, pressure, hardware_context_limit, primary);
    RecommendationReport {
        outcome,
        constraints,
        resource_risks,
        cards,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MemoryPressure {
    Normal,
    Elevated,
    High,
}

impl MemoryPressure {
    fn from_snapshot(resources: &ResourceSnapshot) -> Self {
        if resources.total_memory_bytes == 0
            || ratio_at_least(
                resources
                    .total_memory_bytes
                    .saturating_sub(resources.available_memory_bytes),
                resources.total_memory_bytes,
                90,
            )
            || high_swap(resources)
            || resources.memory_pressure.eq_ignore_ascii_case("high")
        {
            Self::High
        } else if ratio_at_least(
            resources
                .total_memory_bytes
                .saturating_sub(resources.available_memory_bytes),
            resources.total_memory_bytes,
            75,
        ) || resources.used_swap_bytes >= ELEVATED_SWAP_BYTES
            || resources.memory_pressure.eq_ignore_ascii_case("elevated")
        {
            Self::Elevated
        } else {
            Self::Normal
        }
    }
}

fn evaluate_candidate(
    resources: &ResourceSnapshot,
    candidate: &ModelEvidence,
    pressure: MemoryPressure,
    hardware_context_limit: u64,
) -> ModelQualificationCard {
    let mut missing_evidence = Vec::new();
    let mut risks = Vec::new();
    let mut blockers = Vec::new();

    if candidate.model.trim().is_empty() {
        push_blocker(
            &mut blockers,
            BlockerCode::EmptyModelIdentity,
            "model identity is empty",
        );
    }
    if !candidate.installed {
        push_blocker(
            &mut blockers,
            BlockerCode::NotInstalled,
            "model is not installed and pull is outside this recommendation core",
        );
    }

    validate_metadata(candidate, &mut blockers);
    collect_metadata_gaps(candidate, &mut missing_evidence);
    collect_quantization_risk(candidate, &mut risks);

    let qualification = qualification_card(
        candidate.qualification.as_ref(),
        &mut missing_evidence,
        &mut risks,
        &mut blockers,
    );
    let recommended_context_tokens = recommended_context(
        candidate,
        &qualification,
        hardware_context_limit,
        &mut risks,
    );
    let estimated_runtime_memory_bytes =
        estimate_runtime_memory(&candidate.metadata, recommended_context_tokens);
    if estimated_runtime_memory_bytes.is_none() {
        push_blocker(
            &mut blockers,
            BlockerCode::MemoryEstimateUnavailable,
            "observed size or a valid parameter-count/quantization pair is required",
        );
    }
    if resources.maximum_local_inference_requests == 0 {
        push_blocker(
            &mut blockers,
            BlockerCode::ResourceGovernorDisabled,
            "resource governor permits zero local inference requests",
        );
    }
    if let Some(estimated) = estimated_runtime_memory_bytes {
        if !fits_memory_budget(resources, estimated, candidate.currently_loaded, pressure) {
            push_blocker(
                &mut blockers,
                BlockerCode::MemoryBudgetExceeded,
                format!(
                    "estimated runtime memory {} MiB exceeds the conservative observed budget",
                    estimated / MIB
                ),
            );
        }
    }

    let score = candidate.qualification.as_ref().and_then(|evidence| {
        valid_qualification(evidence).then(|| {
            recommendation_score(
                resources,
                candidate,
                evidence,
                estimated_runtime_memory_bytes,
                recommended_context_tokens,
            )
        })
    });
    if blockers.is_empty() && score.is_some_and(|score| score < MINIMUM_RECOMMENDATION_SCORE) {
        push_blocker(
            &mut blockers,
            BlockerCode::RecommendationScoreBelowThreshold,
            format!(
                "evidence score {} is below the minimum {}",
                score.unwrap_or_default(),
                MINIMUM_RECOMMENDATION_SCORE
            ),
        );
    }

    ModelQualificationCard {
        model: candidate.model.clone(),
        installed: candidate.installed,
        currently_loaded: candidate.currently_loaded,
        parameter_count: candidate.metadata.parameter_count,
        quantization: candidate.metadata.quantization.clone(),
        quantization_bits: candidate.metadata.quantization_bits,
        metadata_context_tokens: candidate.metadata.context_length_tokens,
        observed_size_bytes: candidate.metadata.size_bytes,
        estimated_runtime_memory_bytes,
        recommended_context_tokens,
        qualification,
        score,
        status: ModelRecommendationStatus::NotRecommended,
        risks,
        missing_evidence,
        not_recommended_reasons: blockers,
    }
}

fn validate_metadata(candidate: &ModelEvidence, blockers: &mut Vec<EvidenceNotice<BlockerCode>>) {
    let invalid = candidate.metadata.parameter_count == Some(0)
        || candidate.metadata.size_bytes == Some(0)
        || candidate.metadata.context_length_tokens == Some(0)
        || candidate
            .metadata
            .quantization_bits
            .is_some_and(|bits| bits == 0 || bits > 32);
    if invalid {
        push_blocker(
            blockers,
            BlockerCode::InvalidMetadata,
            "metadata contains a zero or out-of-range observed value",
        );
    }
}

fn collect_metadata_gaps(
    candidate: &ModelEvidence,
    missing: &mut Vec<EvidenceNotice<MissingEvidenceCode>>,
) {
    if candidate.metadata.size_bytes.is_none() {
        push_missing(
            missing,
            MissingEvidenceCode::ModelSize,
            "Ollama did not report model size",
        );
    }
    if candidate.metadata.parameter_count.is_none() {
        push_missing(
            missing,
            MissingEvidenceCode::ParameterCount,
            "Ollama did not report parameter count",
        );
    }
    if candidate.metadata.quantization.is_none() {
        push_missing(
            missing,
            MissingEvidenceCode::Quantization,
            "Ollama did not report quantization label",
        );
    }
    if candidate.metadata.quantization_bits.is_none() {
        push_missing(
            missing,
            MissingEvidenceCode::QuantizationBits,
            "quantization bit width was not observed",
        );
    }
    if candidate.metadata.context_length_tokens.is_none() {
        push_missing(
            missing,
            MissingEvidenceCode::MetadataContext,
            "Ollama did not report model context length",
        );
    }
}

fn collect_quantization_risk(candidate: &ModelEvidence, risks: &mut Vec<EvidenceNotice<RiskCode>>) {
    if candidate
        .metadata
        .quantization_bits
        .is_some_and(|bits| bits <= 3)
    {
        push_risk(
            risks,
            RiskCode::AggressiveQuantization,
            "observed quantization is 3-bit or lower; rely on qualification, not model identity",
        );
    }
}

fn qualification_card(
    evidence: Option<&QualificationEvidence>,
    missing: &mut Vec<EvidenceNotice<MissingEvidenceCode>>,
    risks: &mut Vec<EvidenceNotice<RiskCode>>,
    blockers: &mut Vec<EvidenceNotice<BlockerCode>>,
) -> QualificationCard {
    let Some(evidence) = evidence else {
        push_missing(
            missing,
            MissingEvidenceCode::Qualification,
            "no completed model qualification was supplied",
        );
        push_blocker(
            blockers,
            BlockerCode::QualificationMissing,
            "a verified recommendation requires real qualification evidence",
        );
        return QualificationCard {
            assessment: QualificationAssessment::Missing,
            coding: CodingAssessment::Unverified,
            passed_cases: None,
            total_cases: None,
            accuracy_basis_points: None,
            structured_json: CapabilityObservation::NotTested,
            tool_calling: CapabilityObservation::NotTested,
            mean_latency_ms: None,
            estimated_tokens_per_second_milli: None,
            maximum_reliable_context_tokens: None,
        };
    };

    let valid = valid_qualification(evidence);
    let accuracy = valid.then(|| qualification_accuracy(evidence));
    if !valid {
        push_blocker(
            blockers,
            BlockerCode::InvalidQualification,
            "qualification totals are empty or inconsistent",
        );
    } else if accuracy.is_some_and(|score| score < MINIMUM_QUALIFICATION_BASIS_POINTS) {
        push_blocker(
            blockers,
            BlockerCode::QualificationBelowThreshold,
            format!(
                "qualification accuracy {} basis points is below {}",
                accuracy.unwrap_or_default(),
                MINIMUM_QUALIFICATION_BASIS_POINTS
            ),
        );
    }

    match evidence.structured_json {
        CapabilityObservation::Verified => {}
        CapabilityObservation::Failed => push_blocker(
            blockers,
            BlockerCode::StructuredJsonFailed,
            "structured JSON qualification failed",
        ),
        CapabilityObservation::NotTested => {
            push_missing(
                missing,
                MissingEvidenceCode::StructuredJsonProbe,
                "structured JSON was not tested",
            );
            push_blocker(
                blockers,
                BlockerCode::StructuredJsonNotTested,
                "PurrCode requires verified structured JSON for native turns",
            );
        }
    }
    match evidence.tool_calling {
        CapabilityObservation::Verified => {}
        CapabilityObservation::Failed => push_risk(
            risks,
            RiskCode::ToolCallingFailed,
            "native tool calling failed; structured atomic actions may still be usable",
        ),
        CapabilityObservation::NotTested => push_missing(
            missing,
            MissingEvidenceCode::ToolCallingProbe,
            "native tool calling was not tested",
        ),
    }
    if evidence.mean_latency_ms.is_none() {
        push_missing(
            missing,
            MissingEvidenceCode::QualificationLatency,
            "qualification latency was not observed",
        );
        push_blocker(
            blockers,
            BlockerCode::QualificationLatencyMissing,
            "a verified recommendation requires observed qualification latency",
        );
    } else if evidence
        .mean_latency_ms
        .is_some_and(|latency| latency > 30_000)
    {
        push_risk(
            risks,
            RiskCode::HighLatency,
            "mean qualification latency exceeded 30 seconds",
        );
    }
    if evidence.maximum_reliable_context_tokens.is_none() {
        push_missing(
            missing,
            MissingEvidenceCode::ReliableContext,
            "maximum reliable context was not observed",
        );
        push_blocker(
            blockers,
            BlockerCode::ReliableContextMissing,
            "a verified recommendation requires an observed reliable-context probe",
        );
    } else if evidence
        .maximum_reliable_context_tokens
        .is_some_and(|tokens| tokens < 4_096)
    {
        push_blocker(
            blockers,
            BlockerCode::ReliableContextBelowMinimum,
            "observed reliable context is below the 4096-token coding minimum",
        );
    }

    let assessment = if !valid {
        QualificationAssessment::Incomplete
    } else if evidence.structured_json == CapabilityObservation::Failed
        || accuracy.is_some_and(|score| score < MINIMUM_QUALIFICATION_BASIS_POINTS)
        || evidence
            .maximum_reliable_context_tokens
            .is_some_and(|tokens| tokens < 4_096)
    {
        QualificationAssessment::Failed
    } else if evidence.structured_json == CapabilityObservation::NotTested
        || evidence.mean_latency_ms.is_none()
        || evidence.maximum_reliable_context_tokens.is_none()
    {
        QualificationAssessment::Incomplete
    } else {
        QualificationAssessment::Verified
    };
    QualificationCard {
        assessment,
        coding: accuracy.map_or(CodingAssessment::Unverified, coding_assessment),
        passed_cases: Some(evidence.passed_cases),
        total_cases: Some(evidence.total_cases),
        accuracy_basis_points: accuracy,
        structured_json: evidence.structured_json,
        tool_calling: evidence.tool_calling,
        mean_latency_ms: evidence.mean_latency_ms,
        estimated_tokens_per_second_milli: evidence.estimated_tokens_per_second_milli,
        maximum_reliable_context_tokens: evidence.maximum_reliable_context_tokens,
    }
}

fn valid_qualification(evidence: &QualificationEvidence) -> bool {
    evidence.total_cases > 0 && evidence.passed_cases <= evidence.total_cases
}

fn qualification_accuracy(evidence: &QualificationEvidence) -> u16 {
    ((u32::from(evidence.passed_cases) * 10_000) / u32::from(evidence.total_cases)) as u16
}

fn coding_assessment(accuracy: u16) -> CodingAssessment {
    match accuracy {
        9_000..=10_000 => CodingAssessment::Strong,
        8_000..=8_999 => CodingAssessment::Usable,
        1..=7_999 => CodingAssessment::Weak,
        0 => CodingAssessment::Failed,
        _ => CodingAssessment::Unverified,
    }
}

fn recommended_context(
    candidate: &ModelEvidence,
    qualification: &QualificationCard,
    hardware_limit: u64,
    risks: &mut Vec<EvidenceNotice<RiskCode>>,
) -> u64 {
    let mut limit = hardware_limit;
    if let Some(metadata_limit) = candidate
        .metadata
        .context_length_tokens
        .filter(|value| *value > 0)
    {
        limit = limit.min(metadata_limit);
    }
    if let Some(reliable_limit) = qualification
        .maximum_reliable_context_tokens
        .filter(|value| *value > 0)
    {
        limit = limit.min(reliable_limit);
    } else {
        limit = limit.min(8_192);
    }
    if limit < hardware_limit {
        push_risk(
            risks,
            RiskCode::ContextCapped,
            format!("context is capped at {limit} tokens by observed evidence"),
        );
    }
    limit
}

fn estimate_runtime_memory(metadata: &OllamaMetadataEvidence, context_tokens: u64) -> Option<u64> {
    let base = metadata.size_bytes.filter(|size| *size > 0).or_else(|| {
        let parameters = metadata.parameter_count.filter(|count| *count > 0)?;
        let bits = u64::from(
            metadata
                .quantization_bits
                .filter(|bits| *bits > 0 && *bits <= 32)?,
        );
        parameters.checked_mul(bits)?.checked_add(7)?.checked_div(8)
    })?;
    let runtime = base.checked_mul(5)?.checked_div(4)?;
    let bytes_per_context_token = if metadata.parameter_count.unwrap_or_default() > 8_000_000_000 {
        128 * KIB
    } else {
        64 * KIB
    };
    runtime.checked_add(context_tokens.checked_mul(bytes_per_context_token)?)
}

fn fits_memory_budget(
    resources: &ResourceSnapshot,
    estimated: u64,
    currently_loaded: bool,
    pressure: MemoryPressure,
) -> bool {
    if resources.total_memory_bytes == 0
        || resources.available_memory_bytes == 0
        || ratio_at_least(estimated, resources.total_memory_bytes, 70)
    {
        return false;
    }
    let reclaimable = if currently_loaded {
        resources.loaded_model_bytes.min(estimated)
    } else {
        0
    };
    let available_budget = resources
        .available_memory_bytes
        .saturating_add(reclaimable)
        .min(resources.total_memory_bytes);
    let allowed_percent = match pressure {
        MemoryPressure::High => 60,
        MemoryPressure::Elevated => 70,
        MemoryPressure::Normal if resources.low_memory => 75,
        MemoryPressure::Normal => 85,
    };
    !ratio_at_least(estimated, available_budget, allowed_percent + 1)
}

fn recommendation_score(
    resources: &ResourceSnapshot,
    candidate: &ModelEvidence,
    qualification: &QualificationEvidence,
    estimated_memory: Option<u64>,
    recommended_context: u64,
) -> u16 {
    let accuracy = qualification_accuracy(qualification);
    let mut score = u32::from(accuracy) * 650 / 10_000;
    if qualification.structured_json == CapabilityObservation::Verified {
        score += 100;
    }
    if qualification.tool_calling == CapabilityObservation::Verified {
        score += 50;
    }
    score += match qualification.mean_latency_ms {
        Some(0..=2_000) => 100,
        Some(2_001..=10_000) => 60,
        Some(10_001..=30_000) => 25,
        _ => 0,
    };
    score += match qualification.maximum_reliable_context_tokens {
        Some(context) if context >= recommended_context => 50,
        Some(context) if context >= 4_096 => 25,
        _ => 0,
    };
    if estimated_memory.is_some_and(|memory| {
        !ratio_at_least(memory, resources.total_memory_bytes, 40)
            && memory <= resources.available_memory_bytes
    }) {
        score += 50;
    } else if estimated_memory.is_some() {
        score += 20;
    }
    if candidate
        .metadata
        .quantization_bits
        .is_some_and(|bits| bits <= 3)
    {
        score = score.saturating_sub(75);
    }
    score.min(1_000) as u16
}

fn constraints(
    resources: &ResourceSnapshot,
    pressure: MemoryPressure,
    hardware_context_limit: u64,
    primary: Option<&ModelQualificationCard>,
) -> RecommendationConstraints {
    let low_memory = low_memory(resources);
    let high_swap = high_swap(resources);
    let conservative = low_memory || pressure != MemoryPressure::Normal || high_swap;
    let context_limit_tokens = primary
        .map(|card| card.recommended_context_tokens)
        .unwrap_or(hardware_context_limit);
    let estimated = primary.and_then(|card| card.estimated_runtime_memory_bytes);
    let memory_parallelism = estimated
        .and_then(|estimated| resources.available_memory_bytes.checked_div(estimated))
        .unwrap_or(1)
        .max(1)
        .min(usize::MAX as u64) as usize;
    let maximum_parallel_requests = if resources.maximum_local_inference_requests == 0 {
        0
    } else if conservative || primary.is_none() {
        1
    } else {
        resources
            .maximum_local_inference_requests
            .min(memory_parallelism)
            .max(1)
    };
    let allow_separate_local_judge = primary.is_some()
        && !conservative
        && resources.allow_separate_local_judge
        && estimated.is_some_and(|memory| {
            memory.checked_mul(2).is_some_and(|two_models| {
                !ratio_at_least(two_models, resources.total_memory_bytes, 60)
            })
        });
    let lifecycle = if conservative {
        RecommendedLifecycle::UnloadAfterRequest
    } else {
        RecommendedLifecycle::IdleTimeout
    };
    let mut reasons = BTreeSet::new();
    if conservative || primary.is_none() {
        reasons.insert(ConstraintReason::ConservativeDefault);
    }
    if low_memory {
        reasons.insert(ConstraintReason::LowMemorySystem);
    }
    if high_swap {
        reasons.insert(ConstraintReason::HighSwapPressure);
    }
    match pressure {
        MemoryPressure::Normal => {}
        MemoryPressure::Elevated => {
            reasons.insert(ConstraintReason::ElevatedMemoryPressure);
        }
        MemoryPressure::High => {
            reasons.insert(ConstraintReason::HighMemoryPressure);
        }
    }
    if context_limit_tokens < hardware_context_limit {
        reasons.insert(ConstraintReason::QualificationContextLimit);
    }
    if maximum_parallel_requests < resources.maximum_local_inference_requests.max(1) {
        reasons.insert(ConstraintReason::ResourceGovernorLimit);
    }
    RecommendationConstraints {
        single_model_mode: !allow_separate_local_judge,
        context_limit_tokens,
        maximum_parallel_requests,
        allow_separate_local_judge,
        lifecycle,
        idle_timeout_seconds: (lifecycle == RecommendedLifecycle::IdleTimeout).then_some(300),
        reasons: reasons.into_iter().collect(),
    }
}

fn hardware_context_limit(resources: &ResourceSnapshot, pressure: MemoryPressure) -> u64 {
    if resources.total_memory_bytes <= 8 * GIB || pressure == MemoryPressure::High {
        8_192
    } else if low_memory(resources) || pressure == MemoryPressure::Elevated {
        16_384
    } else {
        32_768
    }
}

fn low_memory(resources: &ResourceSnapshot) -> bool {
    resources.low_memory
        || resources.total_memory_bytes == 0
        || resources.total_memory_bytes <= 16 * GIB
}

fn high_swap(resources: &ResourceSnapshot) -> bool {
    resources.used_swap_bytes >= HIGH_SWAP_BYTES
        || (resources.used_swap_bytes >= ELEVATED_SWAP_BYTES
            && ratio_at_least(resources.used_swap_bytes, resources.total_swap_bytes, 50))
}

fn resource_risks(
    resources: &ResourceSnapshot,
    pressure: MemoryPressure,
) -> Vec<EvidenceNotice<RiskCode>> {
    let mut risks = Vec::new();
    if low_memory(resources) {
        push_risk(
            &mut risks,
            RiskCode::LowMemorySystem,
            "observed physical memory requires single-model conservative operation",
        );
    }
    if high_swap(resources) {
        push_risk(
            &mut risks,
            RiskCode::HighSwapPressure,
            "observed swap use requires one request and unload-after-request",
        );
    }
    match pressure {
        MemoryPressure::Normal => {}
        MemoryPressure::Elevated => push_risk(
            &mut risks,
            RiskCode::ElevatedMemoryPressure,
            "observed available memory or swap indicates elevated pressure",
        ),
        MemoryPressure::High => push_risk(
            &mut risks,
            RiskCode::HighMemoryPressure,
            "observed available memory or swap indicates high pressure",
        ),
    }
    risks
}

fn compare_cards(left: &ModelQualificationCard, right: &ModelQualificationCard) -> Ordering {
    let left_eligible = left.not_recommended_reasons.is_empty();
    let right_eligible = right.not_recommended_reasons.is_empty();
    right_eligible
        .cmp(&left_eligible)
        .then_with(|| {
            right
                .score
                .unwrap_or_default()
                .cmp(&left.score.unwrap_or_default())
        })
        .then_with(|| left.model.cmp(&right.model))
}

fn no_recommendation_reasons(
    candidates: &[ModelEvidence],
    cards: &[ModelQualificationCard],
) -> Vec<NoRecommendationReason> {
    if candidates.is_empty() {
        return vec![NoRecommendationReason::NoCandidates];
    }
    let mut reasons = BTreeSet::new();
    if candidates.iter().all(|candidate| !candidate.installed) {
        reasons.insert(NoRecommendationReason::NoInstalledModels);
    }
    if cards.iter().any(|card| {
        card.not_recommended_reasons.iter().any(|reason| {
            matches!(
                reason.code,
                BlockerCode::MemoryBudgetExceeded | BlockerCode::ResourceGovernorDisabled
            )
        })
    }) {
        reasons.insert(NoRecommendationReason::HardwareBudgetInsufficient);
    }
    if cards.iter().any(|card| {
        card.not_recommended_reasons.iter().any(|reason| {
            matches!(
                reason.code,
                BlockerCode::QualificationBelowThreshold
                    | BlockerCode::StructuredJsonFailed
                    | BlockerCode::ReliableContextBelowMinimum
                    | BlockerCode::RecommendationScoreBelowThreshold
            )
        })
    }) {
        reasons.insert(NoRecommendationReason::QualificationInsufficient);
    }
    if cards.iter().any(|card| {
        !card.missing_evidence.is_empty()
            || card.not_recommended_reasons.iter().any(|reason| {
                matches!(
                    reason.code,
                    BlockerCode::MemoryEstimateUnavailable
                        | BlockerCode::QualificationLatencyMissing
                        | BlockerCode::QualificationMissing
                        | BlockerCode::ReliableContextMissing
                        | BlockerCode::StructuredJsonNotTested
                )
            })
    }) {
        reasons.insert(NoRecommendationReason::EvidenceIncomplete);
    }
    if reasons.is_empty() {
        reasons.insert(NoRecommendationReason::NoEligibleModel);
    }
    reasons.into_iter().collect()
}

fn ratio_at_least(numerator: u64, denominator: u64, percent: u64) -> bool {
    denominator == 0 || u128::from(numerator) * 100 >= u128::from(denominator) * u128::from(percent)
}

fn finite_nonnegative_u64(value: f64) -> Option<u64> {
    (value.is_finite() && value >= 0.0).then(|| value.round().min(u64::MAX as f64) as u64)
}

fn push_missing(
    notices: &mut Vec<EvidenceNotice<MissingEvidenceCode>>,
    code: MissingEvidenceCode,
    detail: impl Into<String>,
) {
    notices.push(EvidenceNotice {
        code,
        detail: detail.into(),
    });
}

fn push_risk(
    notices: &mut Vec<EvidenceNotice<RiskCode>>,
    code: RiskCode,
    detail: impl Into<String>,
) {
    notices.push(EvidenceNotice {
        code,
        detail: detail.into(),
    });
}

fn push_blocker(
    notices: &mut Vec<EvidenceNotice<BlockerCode>>,
    code: BlockerCode,
    detail: impl Into<String>,
) {
    if notices.iter().all(|notice| notice.code != code) {
        notices.push(EvidenceNotice {
            code,
            detail: detail.into(),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use purrcode_provider_gateway::{
        ModelId, QualificationCaseResult as ProviderCase, QualificationReport as ProviderReport,
    };

    fn resources(
        total_gib: u64,
        available_gib: u64,
        used_swap_gib: u64,
        pressure: &'static str,
        maximum_requests: usize,
        allow_judge: bool,
    ) -> ResourceSnapshot {
        ResourceSnapshot {
            total_memory_bytes: total_gib * GIB,
            available_memory_bytes: available_gib * GIB,
            loaded_model_bytes: 0,
            total_swap_bytes: 16 * GIB,
            used_swap_bytes: used_swap_gib * GIB,
            low_memory: total_gib <= 16,
            memory_pressure: pressure,
            maximum_local_inference_requests: maximum_requests,
            allow_separate_local_judge: allow_judge,
        }
    }

    fn qualified(passed: u16, total: u16) -> QualificationEvidence {
        QualificationEvidence {
            passed_cases: passed,
            total_cases: total,
            structured_json: CapabilityObservation::Verified,
            tool_calling: CapabilityObservation::NotTested,
            mean_latency_ms: Some(2_500),
            estimated_tokens_per_second_milli: Some(12_000),
            maximum_reliable_context_tokens: Some(16_384),
        }
    }

    fn candidate(model: &str, parameters: u64, size_gib: u64) -> ModelEvidence {
        ModelEvidence {
            model: model.into(),
            installed: true,
            currently_loaded: false,
            metadata: OllamaMetadataEvidence {
                parameter_count: Some(parameters),
                quantization: Some("Q4_K_M".into()),
                quantization_bits: Some(4),
                context_length_tokens: Some(32_768),
                size_bytes: Some(size_gib * GIB),
            },
            qualification: Some(qualified(7, 7)),
        }
    }

    fn blocker(card: &ModelQualificationCard, code: BlockerCode) -> bool {
        card.not_recommended_reasons
            .iter()
            .any(|reason| reason.code == code)
    }

    #[test]
    fn low_memory_recommends_a_verified_small_model_with_conservative_constraints() {
        let report = recommend_local_models(
            &resources(8, 5, 0, "normal", 4, true),
            &[candidate("identity-only", 3_000_000_000, 2)],
        );
        assert!(matches!(
            report.outcome,
            RecommendationOutcome::Recommended { ref model, .. } if model == "identity-only"
        ));
        assert_eq!(report.constraints.context_limit_tokens, 8_192);
        assert_eq!(report.constraints.maximum_parallel_requests, 1);
        assert!(report.constraints.single_model_mode);
        assert!(!report.constraints.allow_separate_local_judge);
        assert_eq!(
            report.constraints.lifecycle,
            RecommendedLifecycle::UnloadAfterRequest
        );
    }

    #[test]
    fn large_model_is_rejected_on_eight_gib_and_smaller_verified_model_wins() {
        let report = recommend_local_models(
            &resources(8, 6, 0, "normal", 1, false),
            &[
                candidate("large", 7_000_000_000, 5),
                candidate("small", 3_000_000_000, 2),
            ],
        );
        assert!(matches!(
            report.outcome,
            RecommendationOutcome::Recommended { ref model, .. } if model == "small"
        ));
        let large = report
            .cards
            .iter()
            .find(|card| card.model == "large")
            .unwrap();
        assert!(blocker(large, BlockerCode::MemoryBudgetExceeded));
        assert_eq!(large.status, ModelRecommendationStatus::NotRecommended);
    }

    #[test]
    fn high_swap_is_deterministically_single_request_and_unload_after_request() {
        let report = recommend_local_models(
            &resources(32, 20, 4, "normal", 8, true),
            &[candidate("small", 3_000_000_000, 2)],
        );
        assert_eq!(report.constraints.maximum_parallel_requests, 1);
        assert!(report.constraints.single_model_mode);
        assert!(!report.constraints.allow_separate_local_judge);
        assert_eq!(
            report.constraints.lifecycle,
            RecommendedLifecycle::UnloadAfterRequest
        );
        assert!(
            report
                .resource_risks
                .iter()
                .any(|risk| risk.code == RiskCode::HighSwapPressure)
        );
    }

    #[test]
    fn missing_qualification_is_explicitly_not_recommended() {
        let mut model = candidate("unqualified", 3_000_000_000, 2);
        model.qualification = None;
        let report = recommend_local_models(&resources(32, 24, 0, "normal", 2, true), &[model]);
        assert!(matches!(
            report.outcome,
            RecommendationOutcome::NoRecommendation { ref reasons }
                if reasons.contains(&NoRecommendationReason::EvidenceIncomplete)
        ));
        assert!(blocker(&report.cards[0], BlockerCode::QualificationMissing));
        assert_eq!(
            report.cards[0].qualification.assessment,
            QualificationAssessment::Missing
        );
    }

    #[test]
    fn incomplete_or_weak_qualification_cannot_be_recommended() {
        let mut missing_latency = candidate("missing-latency", 3_000_000_000, 2);
        missing_latency
            .qualification
            .as_mut()
            .unwrap()
            .mean_latency_ms = None;
        let mut missing_context = candidate("missing-context", 3_000_000_000, 2);
        missing_context
            .qualification
            .as_mut()
            .unwrap()
            .maximum_reliable_context_tokens = None;
        let mut weak = candidate("weak", 3_000_000_000, 2);
        weak.qualification = Some(qualified(5, 7));
        let report = recommend_local_models(
            &resources(32, 24, 0, "normal", 2, true),
            &[missing_latency, missing_context, weak],
        );
        assert!(matches!(
            report.outcome,
            RecommendationOutcome::NoRecommendation { .. }
        ));
        let latency = report
            .cards
            .iter()
            .find(|card| card.model == "missing-latency")
            .unwrap();
        assert!(blocker(latency, BlockerCode::QualificationLatencyMissing));
        let context = report
            .cards
            .iter()
            .find(|card| card.model == "missing-context")
            .unwrap();
        assert!(blocker(context, BlockerCode::ReliableContextMissing));
        let weak = report
            .cards
            .iter()
            .find(|card| card.model == "weak")
            .unwrap();
        assert!(blocker(weak, BlockerCode::QualificationBelowThreshold));
    }

    #[test]
    fn noninstalled_invalid_and_governor_disabled_inputs_fail_closed() {
        let mut noninstalled = candidate("not-installed", 3_000_000_000, 2);
        noninstalled.installed = false;
        let report =
            recommend_local_models(&resources(32, 24, 0, "normal", 0, true), &[noninstalled]);
        assert!(blocker(&report.cards[0], BlockerCode::NotInstalled));
        assert!(blocker(
            &report.cards[0],
            BlockerCode::ResourceGovernorDisabled
        ));
        assert_eq!(report.constraints.maximum_parallel_requests, 0);
        assert!(matches!(
            report.outcome,
            RecommendationOutcome::NoRecommendation { ref reasons }
                if reasons.contains(&NoRecommendationReason::NoInstalledModels)
                    && reasons.contains(&NoRecommendationReason::HardwareBudgetInsufficient)
        ));

        let mut invalid = candidate("invalid", 3_000_000_000, 2);
        invalid.qualification = Some(qualified(8, 7));
        invalid.metadata.context_length_tokens = Some(0);
        let invalid_report =
            recommend_local_models(&resources(32, 24, 0, "normal", 1, true), &[invalid]);
        assert!(blocker(
            &invalid_report.cards[0],
            BlockerCode::InvalidMetadata
        ));
        assert!(blocker(
            &invalid_report.cards[0],
            BlockerCode::InvalidQualification
        ));
    }

    #[test]
    fn parameter_and_quantization_can_bound_memory_when_size_is_missing() {
        let mut model = candidate("metadata-estimate", 3_000_000_000, 2);
        model.metadata.size_bytes = None;
        let report = recommend_local_models(&resources(32, 24, 0, "normal", 2, true), &[model]);
        assert!(matches!(
            report.outcome,
            RecommendationOutcome::Recommended { .. }
        ));
        assert!(report.cards[0].estimated_runtime_memory_bytes.is_some());
        assert!(
            report.cards[0]
                .missing_evidence
                .iter()
                .any(|gap| gap.code == MissingEvidenceCode::ModelSize)
        );
        assert!(!blocker(
            &report.cards[0],
            BlockerCode::MemoryEstimateUnavailable
        ));
    }

    #[test]
    fn model_name_does_not_change_evidence_score_or_quality() {
        let report = recommend_local_models(
            &resources(32, 24, 0, "normal", 2, true),
            &[
                candidate("impressive-coder-name", 3_000_000_000, 2),
                candidate("plain", 3_000_000_000, 2),
            ],
        );
        let impressive = report
            .cards
            .iter()
            .find(|card| card.model == "impressive-coder-name")
            .unwrap();
        let plain = report
            .cards
            .iter()
            .find(|card| card.model == "plain")
            .unwrap();
        assert_eq!(impressive.score, plain.score);
        assert_eq!(impressive.qualification.coding, plain.qualification.coding);
    }

    #[test]
    fn structured_json_failure_blocks_even_perfect_case_accuracy() {
        let mut model = candidate("structured-failure", 3_000_000_000, 2);
        model.qualification.as_mut().unwrap().structured_json = CapabilityObservation::Failed;
        let report = recommend_local_models(&resources(32, 24, 0, "normal", 2, true), &[model]);
        assert!(blocker(&report.cards[0], BlockerCode::StructuredJsonFailed));
        assert!(matches!(
            report.outcome,
            RecommendationOutcome::NoRecommendation { .. }
        ));
    }

    #[test]
    fn invalid_or_missing_memory_metadata_fails_closed() {
        let mut model = candidate("missing-memory", 3_000_000_000, 2);
        model.metadata.parameter_count = None;
        model.metadata.quantization_bits = None;
        model.metadata.size_bytes = None;
        let report = recommend_local_models(&resources(32, 24, 0, "normal", 2, true), &[model]);
        assert!(blocker(
            &report.cards[0],
            BlockerCode::MemoryEstimateUnavailable
        ));
        assert!(
            report.cards[0]
                .missing_evidence
                .iter()
                .any(|gap| gap.code == MissingEvidenceCode::ModelSize)
        );
    }

    #[test]
    fn aggressive_quantization_is_a_risk_and_never_a_quality_bonus() {
        let normal = candidate("normal", 3_000_000_000, 2);
        let mut aggressive = normal.clone();
        aggressive.model = "aggressive".into();
        aggressive.metadata.quantization = Some("Q3".into());
        aggressive.metadata.quantization_bits = Some(3);
        let report = recommend_local_models(
            &resources(32, 24, 0, "normal", 2, true),
            &[normal, aggressive],
        );
        let normal = report
            .cards
            .iter()
            .find(|card| card.model == "normal")
            .unwrap();
        let aggressive = report
            .cards
            .iter()
            .find(|card| card.model == "aggressive")
            .unwrap();
        assert!(
            aggressive
                .risks
                .iter()
                .any(|risk| risk.code == RiskCode::AggressiveQuantization)
        );
        assert!(aggressive.score < normal.score);
    }

    #[test]
    fn observed_reliable_context_caps_the_recommendation() {
        let mut model = candidate("limited-context", 3_000_000_000, 2);
        model
            .qualification
            .as_mut()
            .unwrap()
            .maximum_reliable_context_tokens = Some(4_096);
        let report = recommend_local_models(&resources(64, 48, 0, "normal", 4, true), &[model]);
        assert_eq!(report.cards[0].recommended_context_tokens, 4_096);
        assert_eq!(report.constraints.context_limit_tokens, 4_096);
        assert!(
            report.cards[0]
                .risks
                .iter()
                .any(|risk| risk.code == RiskCode::ContextCapped)
        );
    }

    #[test]
    fn ample_observed_budget_can_allow_bounded_parallelism_and_a_local_judge() {
        let report = recommend_local_models(
            &resources(64, 48, 0, "normal", 4, true),
            &[candidate("small", 3_000_000_000, 2)],
        );
        assert!(report.constraints.maximum_parallel_requests > 1);
        assert!(report.constraints.allow_separate_local_judge);
        assert!(!report.constraints.single_model_mode);
        assert_eq!(
            report.constraints.lifecycle,
            RecommendedLifecycle::IdleTimeout
        );
        assert_eq!(report.constraints.idle_timeout_seconds, Some(300));
    }

    #[test]
    fn provider_report_adapter_preserves_observed_failures_and_latency() {
        let provider_report = ProviderReport {
            model: ModelId::parse("local/fixture").unwrap(),
            cases: vec![
                ProviderCase {
                    name: "structured-output".into(),
                    passed: false,
                    latency_ms: 10,
                    detail: "failed".into(),
                },
                ProviderCase {
                    name: "patch-generation".into(),
                    passed: true,
                    latency_ms: 20,
                    detail: "passed".into(),
                },
            ],
            accuracy: 0.5,
            mean_latency_ms: 15.0,
            estimated_tokens_per_second: Some(2.5),
            maximum_reliable_context_tokens: Some(4_096),
            recommended_roles: Vec::new(),
            not_recommended_roles: vec!["simple-coding".into()],
        };
        let evidence =
            QualificationEvidence::from_report(&provider_report, CapabilityObservation::NotTested);
        assert_eq!(evidence.passed_cases, 1);
        assert_eq!(evidence.total_cases, 2);
        assert_eq!(evidence.structured_json, CapabilityObservation::Failed);
        assert_eq!(evidence.mean_latency_ms, Some(15));
        assert_eq!(evidence.estimated_tokens_per_second_milli, Some(2_500));
    }

    #[test]
    fn recommendation_cards_are_json_serializable_with_explicit_reasons() {
        let report = recommend_local_models(&resources(8, 1, 4, "high", 1, false), &[]);
        let value = serde_json::to_value(&report).unwrap();
        assert_eq!(value["outcome"]["status"], "no_recommendation");
        assert_eq!(value["constraints"]["lifecycle"], "unload_after_request");
        assert!(value["resource_risks"].as_array().unwrap().len() >= 2);
    }
}
