//! Registry adapters, candidate discovery, provenance verification, and ranking.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use purrcode_runtime_core::QualificationStatus;
use purrcode_skill_store::{InstalledCapabilityResolution, SkillStore};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::sync::Mutex;
use thiserror::Error;

const MAX_REGISTRY_RESPONSE_BYTES: usize = 1024 * 1024;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SearchQuery {
    pub capability: String,
    pub keywords: Vec<String>,
    pub platform: String,
    pub purrcode_version: String,
}

/// A source reference that cannot move after the user approves a download.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ImmutableSourcePin {
    /// The full commit object ID is the immutable reference.
    GitCommit { commit: String, archive_url: String },
    /// Release assets are pinned by their exact SHA-256 digest.
    ReleaseAsset {
        release: String,
        archive_url: String,
        sha256: String,
    },
}

impl ImmutableSourcePin {
    pub fn archive_url(&self) -> &str {
        match self {
            Self::GitCommit { archive_url, .. } | Self::ReleaseAsset { archive_url, .. } => {
                archive_url
            }
        }
    }

    pub fn immutable_reference(&self) -> &str {
        match self {
            Self::GitCommit { commit, .. } => commit,
            Self::ReleaseAsset { sha256, .. } => sha256,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct MaintenanceMetadata {
    #[serde(default)]
    pub observed_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub last_push_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub last_release_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub archived: Option<bool>,
    #[serde(default)]
    pub stars: Option<u64>,
    #[serde(default)]
    pub open_issues: Option<u64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CandidateManifest {
    pub candidate_id: String,
    pub name: String,
    pub version: String,
    pub publisher: Option<String>,
    pub source_type: String,
    pub source_url: Option<String>,
    pub description: String,
    pub signature_status: String,
    #[serde(default)]
    pub signature: Option<String>,
    #[serde(default)]
    pub publisher_public_key: Option<String>,
    pub permissions: BTreeMap<String, Vec<String>>,
    pub network_access: Option<String>,
    pub dependencies: Vec<String>,
    pub license: Option<String>,
    pub content_digest: Option<String>,
    pub file_count: u32,
    pub reported_size_bytes: u64,
    pub detected_platforms: Vec<String>,
    pub min_purrcode_version: Option<String>,
    /// `false` means empty permission fields are unknown, not an assertion of no permissions.
    #[serde(default)]
    pub permissions_reviewed: bool,
    /// Search results may be mutable. Download cannot be authorized until inspection adds a pin.
    #[serde(default)]
    pub immutable_source: Option<ImmutableSourcePin>,
    #[serde(default)]
    pub maintenance: MaintenanceMetadata,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RiskLevel {
    Low,
    Medium,
    High,
    Critical,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RiskFinding {
    pub code: String,
    pub detail: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RiskAssessment {
    pub level: RiskLevel,
    pub score: u8,
    pub findings: Vec<RiskFinding>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SourceReview {
    pub source_type: String,
    pub source_url: Option<String>,
    pub publisher: Option<String>,
    pub signature_status: String,
    pub immutable_source: Option<ImmutableSourcePin>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PermissionReview {
    pub inspected: bool,
    pub declared: BTreeMap<String, Vec<String>>,
    pub network_access: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CandidateReview {
    pub source: SourceReview,
    pub permissions: PermissionReview,
    pub maintenance: MaintenanceMetadata,
    pub risk: RiskAssessment,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RankedCandidate {
    pub manifest: CandidateManifest,
    pub score: f64,
    pub signals: BTreeMap<String, f64>,
    pub review: CandidateReview,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProvenanceReport {
    pub candidate_id: String,
    pub content_digest: Option<String>,
    pub signature_valid: Option<bool>,
    pub publisher_matches_source: bool,
    pub repository_exists: bool,
    pub age_days: Option<i64>,
    pub release_recency_days: Option<i64>,
    pub test_discovered: bool,
    pub documentation_score: f64,
}

/// Evidence supplied by the daemon after it verifies a durable PawGate authorization.
///
/// This crate validates the exact query, expiry, and approved adapters again immediately before
/// calling an adapter. Constructing this value is not a substitute for the daemon's durable
/// authorization and single-use consumption.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ExternalSearchAuthorization {
    authorization_id: String,
    exact_query: SearchQuery,
    approved_adapters: Vec<String>,
    expires_at: DateTime<Utc>,
}

impl ExternalSearchAuthorization {
    pub fn new(
        authorization_id: impl Into<String>,
        exact_query: SearchQuery,
        approved_adapters: Vec<String>,
        expires_at: DateTime<Utc>,
    ) -> Result<Self, RegistryError> {
        let authorization_id = authorization_id.into();
        if authorization_id.trim().is_empty() {
            return Err(RegistryError::InvalidAuthorization(
                "durable authorization id is empty".into(),
            ));
        }
        if approved_adapters.is_empty()
            || approved_adapters
                .iter()
                .any(|adapter| adapter.trim().is_empty())
        {
            return Err(RegistryError::InvalidAuthorization(
                "at least one exact adapter must be approved".into(),
            ));
        }
        if expires_at <= Utc::now() {
            return Err(RegistryError::AuthorizationExpired);
        }
        validate_search_query(&exact_query)?;
        Ok(Self {
            authorization_id,
            exact_query,
            approved_adapters,
            expires_at,
        })
    }

    pub fn authorization_id(&self) -> &str {
        &self.authorization_id
    }

    fn verify(&self, query: &SearchQuery) -> Result<(), RegistryError> {
        if self.expires_at <= Utc::now() {
            return Err(RegistryError::AuthorizationExpired);
        }
        if self.exact_query != *query {
            return Err(RegistryError::AuthorizationMismatch);
        }
        Ok(())
    }

    fn allows_adapter(&self, adapter: &str) -> bool {
        self.approved_adapters
            .iter()
            .any(|approved| approved == adapter)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CandidateInspectionAuthorization {
    authorization_id: String,
    exact_candidate_id: String,
    approved_adapter: String,
    expires_at: DateTime<Utc>,
}

impl CandidateInspectionAuthorization {
    pub fn new(
        authorization_id: impl Into<String>,
        exact_candidate_id: impl Into<String>,
        approved_adapter: impl Into<String>,
        expires_at: DateTime<Utc>,
    ) -> Result<Self, RegistryError> {
        let authorization_id = authorization_id.into();
        let exact_candidate_id = exact_candidate_id.into();
        let approved_adapter = approved_adapter.into();
        if authorization_id.trim().is_empty()
            || exact_candidate_id.trim().is_empty()
            || approved_adapter.trim().is_empty()
        {
            return Err(RegistryError::InvalidAuthorization(
                "authorization id, candidate id, and adapter are required".into(),
            ));
        }
        if expires_at <= Utc::now() {
            return Err(RegistryError::AuthorizationExpired);
        }
        Ok(Self {
            authorization_id,
            exact_candidate_id,
            approved_adapter,
            expires_at,
        })
    }

    fn verify(&self, candidate_id: &str, adapter: &str) -> Result<(), RegistryError> {
        if self.expires_at <= Utc::now() {
            return Err(RegistryError::AuthorizationExpired);
        }
        if self.exact_candidate_id != candidate_id || self.approved_adapter != adapter {
            return Err(RegistryError::AuthorizationMismatch);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "resolution", rename_all = "snake_case")]
pub enum DiscoveryOutcome {
    Installed {
        installed: InstalledCapabilityResolution,
        external_search_avoided: bool,
    },
    External {
        installed: InstalledCapabilityResolution,
        candidates: Vec<RankedCandidate>,
        external_search_avoided: bool,
    },
}

pub trait InstalledCapabilitySource {
    fn resolve_installed(
        &self,
        capability: &str,
    ) -> Result<InstalledCapabilityResolution, RegistryError>;
}

impl InstalledCapabilitySource for SkillStore {
    fn resolve_installed(
        &self,
        capability: &str,
    ) -> Result<InstalledCapabilityResolution, RegistryError> {
        self.resolve_installed_capability(capability)
            .map_err(|error| RegistryError::InstalledLookup(error.to_string()))
    }
}

#[derive(Debug, Error)]
pub enum RegistryError {
    #[error("adapter error: {0}")]
    Adapter(String),
    #[error("network error: {0}")]
    Network(#[from] reqwest::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("no candidates found")]
    NoCandidates,
    #[error("invalid manifest: {0}")]
    InvalidManifest(String),
    #[error("signature verification failed: {0}")]
    InvalidSignature(String),
    #[error("search query contains private or credential-like data")]
    UnsafeQuery,
    #[error("public web research requires an exact durable authorization")]
    PublicWebPermissionRequired,
    #[error("public web research authorization has expired")]
    AuthorizationExpired,
    #[error("public web research authorization does not match this exact request")]
    AuthorizationMismatch,
    #[error("invalid public web authorization: {0}")]
    InvalidAuthorization(String),
    #[error("installed skill lookup failed: {0}")]
    InstalledLookup(String),
    #[error("candidate source is not pinned to an immutable commit or release digest")]
    UnpinnedSource,
    #[error("invalid immutable source pin: {0}")]
    InvalidSourcePin(String),
    #[error("invalid candidate workflow transition: {0}")]
    InvalidTransition(String),
    #[error("downloaded content digest mismatch: expected {expected}, observed {observed}")]
    DigestMismatch { expected: String, observed: String },
    #[error("download and install must use distinct durable authorizations")]
    ReusedAuthorization,
    #[error("public web authorization was already consumed")]
    AuthorizationConsumed,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ExactActionAuthorization {
    pub authorization_id: String,
    pub action_digest: String,
}

impl ExactActionAuthorization {
    pub fn new(
        authorization_id: impl Into<String>,
        action_digest: impl Into<String>,
    ) -> Result<Self, RegistryError> {
        let authorization_id = authorization_id.into();
        let action_digest = action_digest.into();
        if authorization_id.trim().is_empty() || action_digest.trim().is_empty() {
            return Err(RegistryError::InvalidAuthorization(
                "authorization id and exact action digest are required".into(),
            ));
        }
        Ok(Self {
            authorization_id,
            action_digest,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "stage", rename_all = "snake_case")]
pub enum CandidateWorkflowStage {
    Discovered,
    DownloadAuthorized {
        action_digest: String,
    },
    Downloaded {
        archive_digest: String,
        package_digest: String,
    },
    StaticQualified,
    DynamicQualificationRunning,
    DynamicQualified {
        status: QualificationStatus,
    },
    StaticRejected {
        status: QualificationStatus,
    },
    DynamicRejected {
        status: QualificationStatus,
    },
    IntegrityBlocked {
        status: QualificationStatus,
    },
    InstallAuthorized {
        action_digest: String,
        package_digest: String,
    },
    Installed {
        package_digest: String,
    },
}

/// Pure fail-closed workflow used by the daemon around its PawGate/Claw adapters.
///
/// This type performs no network, process, archive, or filesystem operation. Its purpose is to
/// make download and install independent exact-authorized transitions and to make every dynamic
/// qualification outcome explicit and testable.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CandidateWorkflow {
    pub candidate_id: String,
    pub immutable_source: ImmutableSourcePin,
    pub stage: CandidateWorkflowStage,
    download_authorization_id: Option<String>,
    package_digest: Option<String>,
}

impl CandidateWorkflow {
    pub fn new(
        candidate_id: impl Into<String>,
        immutable_source: ImmutableSourcePin,
    ) -> Result<Self, RegistryError> {
        let candidate_id = candidate_id.into();
        if candidate_id.trim().is_empty() {
            return Err(RegistryError::InvalidManifest(
                "candidate id is empty".into(),
            ));
        }
        validate_source_pin(&immutable_source)?;
        Ok(Self {
            candidate_id,
            immutable_source,
            stage: CandidateWorkflowStage::Discovered,
            download_authorization_id: None,
            package_digest: None,
        })
    }

    pub fn authorize_download(
        &mut self,
        authorization: ExactActionAuthorization,
    ) -> Result<(), RegistryError> {
        if !matches!(self.stage, CandidateWorkflowStage::Discovered) {
            return Err(RegistryError::InvalidTransition(
                "download may be authorized only from discovered".into(),
            ));
        }
        self.download_authorization_id = Some(authorization.authorization_id);
        self.stage = CandidateWorkflowStage::DownloadAuthorized {
            action_digest: authorization.action_digest,
        };
        Ok(())
    }

    pub fn record_download(
        &mut self,
        observed_archive_sha256: &str,
        package_digest: &str,
    ) -> Result<(), RegistryError> {
        if !matches!(
            self.stage,
            CandidateWorkflowStage::DownloadAuthorized { .. }
        ) {
            return Err(RegistryError::InvalidTransition(
                "download evidence requires a separately authorized download".into(),
            ));
        }
        if let Err(error) = validate_hex_digest(observed_archive_sha256, 64, "archive SHA-256")
            .and_then(|_| validate_hex_digest(package_digest, 64, "package content digest"))
        {
            self.stage = CandidateWorkflowStage::IntegrityBlocked {
                status: QualificationStatus::Blocked,
            };
            return Err(error);
        }
        if let ImmutableSourcePin::ReleaseAsset { sha256, .. } = &self.immutable_source {
            if !sha256.eq_ignore_ascii_case(observed_archive_sha256) {
                let error = RegistryError::DigestMismatch {
                    expected: sha256.clone(),
                    observed: observed_archive_sha256.to_owned(),
                };
                self.stage = CandidateWorkflowStage::IntegrityBlocked {
                    status: QualificationStatus::Blocked,
                };
                return Err(error);
            }
        }
        self.stage = CandidateWorkflowStage::Downloaded {
            archive_digest: observed_archive_sha256.to_ascii_lowercase(),
            package_digest: package_digest.to_ascii_lowercase(),
        };
        self.package_digest = Some(package_digest.to_ascii_lowercase());
        Ok(())
    }

    pub fn record_static_qualification(
        &mut self,
        status: QualificationStatus,
    ) -> Result<(), RegistryError> {
        if !matches!(self.stage, CandidateWorkflowStage::Downloaded { .. }) {
            return Err(RegistryError::InvalidTransition(
                "static qualification requires a verified download".into(),
            ));
        }
        if qualification_passed(&status) {
            self.stage = CandidateWorkflowStage::StaticQualified;
        } else {
            self.stage = CandidateWorkflowStage::StaticRejected { status };
        }
        Ok(())
    }

    pub fn start_dynamic_qualification(&mut self) -> Result<(), RegistryError> {
        if !matches!(self.stage, CandidateWorkflowStage::StaticQualified) {
            return Err(RegistryError::InvalidTransition(
                "dynamic qualification requires static qualification".into(),
            ));
        }
        self.stage = CandidateWorkflowStage::DynamicQualificationRunning;
        Ok(())
    }

    pub fn record_dynamic_qualification(
        &mut self,
        status: QualificationStatus,
    ) -> Result<(), RegistryError> {
        if !matches!(
            self.stage,
            CandidateWorkflowStage::DynamicQualificationRunning
        ) {
            return Err(RegistryError::InvalidTransition(
                "dynamic result requires a running dynamic qualification".into(),
            ));
        }
        self.stage = if qualification_passed(&status) {
            CandidateWorkflowStage::DynamicQualified { status }
        } else {
            CandidateWorkflowStage::DynamicRejected { status }
        };
        Ok(())
    }

    pub fn authorize_install(
        &mut self,
        authorization: ExactActionAuthorization,
        package_digest: &str,
    ) -> Result<(), RegistryError> {
        if !matches!(
            &self.stage,
            CandidateWorkflowStage::DynamicQualified { status }
                if qualification_passed(status)
        ) {
            return Err(RegistryError::InvalidTransition(
                "install authorization requires a passing dynamic qualification".into(),
            ));
        }
        if self.download_authorization_id.as_deref()
            == Some(authorization.authorization_id.as_str())
        {
            return Err(RegistryError::ReusedAuthorization);
        }
        validate_hex_digest(package_digest, 64, "package content digest")?;
        let expected_package_digest = self.downloaded_package_digest().ok_or_else(|| {
            RegistryError::InvalidTransition("download evidence is missing".into())
        })?;
        if expected_package_digest != package_digest.to_ascii_lowercase() {
            let error = RegistryError::DigestMismatch {
                expected: expected_package_digest,
                observed: package_digest.to_owned(),
            };
            self.stage = CandidateWorkflowStage::IntegrityBlocked {
                status: QualificationStatus::Blocked,
            };
            return Err(error);
        }
        self.stage = CandidateWorkflowStage::InstallAuthorized {
            action_digest: authorization.action_digest,
            package_digest: package_digest.to_ascii_lowercase(),
        };
        Ok(())
    }

    pub fn record_installed(&mut self, observed_package_digest: &str) -> Result<(), RegistryError> {
        if let Err(error) =
            validate_hex_digest(observed_package_digest, 64, "package content digest")
        {
            self.stage = CandidateWorkflowStage::IntegrityBlocked {
                status: QualificationStatus::Blocked,
            };
            return Err(error);
        }
        let CandidateWorkflowStage::InstallAuthorized { package_digest, .. } = &self.stage else {
            return Err(RegistryError::InvalidTransition(
                "installation requires a separate exact install authorization".into(),
            ));
        };
        if package_digest != &observed_package_digest.to_ascii_lowercase() {
            let error = RegistryError::DigestMismatch {
                expected: package_digest.clone(),
                observed: observed_package_digest.to_owned(),
            };
            self.stage = CandidateWorkflowStage::IntegrityBlocked {
                status: QualificationStatus::Blocked,
            };
            return Err(error);
        }
        self.stage = CandidateWorkflowStage::Installed {
            package_digest: observed_package_digest.to_ascii_lowercase(),
        };
        Ok(())
    }

    pub fn is_installable(&self) -> bool {
        matches!(
            &self.stage,
            CandidateWorkflowStage::DynamicQualified { status }
                if qualification_passed(status)
        )
    }

    fn downloaded_package_digest(&self) -> Option<String> {
        self.package_digest.clone()
    }
}

#[async_trait]
pub trait RegistryAdapter: Send + Sync {
    fn name(&self) -> &str;
    async fn search(&self, query: &SearchQuery) -> Result<Vec<CandidateManifest>, RegistryError>;
    async fn fetch_manifest(&self, candidate_id: &str) -> Result<CandidateManifest, RegistryError>;
}

pub struct RegistryEngine {
    adapters: Vec<Box<dyn RegistryAdapter>>,
    consumed_authorizations: Mutex<HashSet<String>>,
}

impl RegistryEngine {
    pub fn new(adapters: Vec<Box<dyn RegistryAdapter>>) -> Self {
        Self {
            adapters,
            consumed_authorizations: Mutex::new(HashSet::new()),
        }
    }

    pub async fn search(&self, query: &SearchQuery) -> Result<Vec<RankedCandidate>, RegistryError> {
        validate_search_query(query)?;
        Err(RegistryError::PublicWebPermissionRequired)
    }

    pub async fn search_authorized(
        &self,
        query: &SearchQuery,
        authorization: &ExternalSearchAuthorization,
    ) -> Result<Vec<RankedCandidate>, RegistryError> {
        validate_search_query(query)?;
        authorization.verify(query)?;
        self.consume_authorization(authorization.authorization_id())?;
        let mut all_candidates: Vec<(CandidateManifest, &str)> = Vec::new();
        let mut failed_adapters = Vec::new();

        for adapter in &self.adapters {
            if !authorization.allows_adapter(adapter.name()) {
                continue;
            }
            match adapter.search(query).await {
                Ok(candidates) => {
                    for c in candidates {
                        all_candidates.push((c, adapter.name()));
                    }
                }
                Err(_) => failed_adapters.push(adapter.name()),
            }
        }

        if all_candidates.is_empty() {
            if !failed_adapters.is_empty() {
                return Err(RegistryError::Adapter(format!(
                    "approved adapters failed: {}",
                    failed_adapters.join(", ")
                )));
            }
            return Err(RegistryError::NoCandidates);
        }

        let ranked = self.rank(all_candidates);
        Ok(ranked)
    }

    /// Search installed qualified skills first and call no registry adapter when one matches.
    pub async fn discover_installed_first<S: InstalledCapabilitySource>(
        &self,
        installed_source: &S,
        query: &SearchQuery,
        authorization: Option<&ExternalSearchAuthorization>,
    ) -> Result<DiscoveryOutcome, RegistryError> {
        validate_search_query(query)?;
        let installed = installed_source.resolve_installed(&query.capability)?;
        if installed.external_search_avoided {
            return Ok(DiscoveryOutcome::Installed {
                installed,
                external_search_avoided: true,
            });
        }
        let authorization = authorization.ok_or(RegistryError::PublicWebPermissionRequired)?;
        let candidates = self.search_authorized(query, authorization).await?;
        Ok(DiscoveryOutcome::External {
            installed,
            candidates,
            external_search_avoided: false,
        })
    }

    pub async fn fetch_manifest(
        &self,
        _candidate_id: &str,
    ) -> Result<CandidateManifest, RegistryError> {
        Err(RegistryError::PublicWebPermissionRequired)
    }

    pub async fn fetch_manifest_authorized(
        &self,
        candidate_id: &str,
        authorization: &CandidateInspectionAuthorization,
    ) -> Result<CandidateManifest, RegistryError> {
        authorization.verify(candidate_id, &authorization.approved_adapter)?;
        self.consume_authorization(&authorization.authorization_id)?;
        let adapter = self
            .adapters
            .iter()
            .find(|adapter| adapter.name() == authorization.approved_adapter)
            .ok_or(RegistryError::AuthorizationMismatch)?;
        adapter.fetch_manifest(candidate_id).await
    }

    fn consume_authorization(&self, authorization_id: &str) -> Result<(), RegistryError> {
        let mut consumed = self.consumed_authorizations.lock().map_err(|_| {
            RegistryError::InvalidAuthorization("authorization state lock is poisoned".into())
        })?;
        if !consumed.insert(authorization_id.to_owned()) {
            return Err(RegistryError::AuthorizationConsumed);
        }
        Ok(())
    }

    fn rank(&self, candidates: Vec<(CandidateManifest, &str)>) -> Vec<RankedCandidate> {
        let mut ranked: Vec<RankedCandidate> = candidates
            .into_iter()
            .map(|(manifest, source)| {
                let mut signals = BTreeMap::new();
                let review = review_candidate(&manifest);

                signals.insert("source_trust".into(), source_trust_score(source));
                signals.insert(
                    "signature_valid".into(),
                    if manifest.signature_status == "verified" {
                        1.0
                    } else {
                        0.0
                    },
                );
                signals.insert(
                    "has_description".into(),
                    if manifest.description.is_empty() {
                        0.0
                    } else {
                        1.0
                    },
                );
                signals.insert(
                    "has_tests".into(),
                    if manifest.detected_platforms.is_empty() {
                        0.0
                    } else {
                        0.5
                    },
                );
                signals.insert(
                    "has_license".into(),
                    if manifest.license.is_some() { 1.0 } else { 0.0 },
                );
                signals.insert(
                    "limited_permissions".into(),
                    limited_permissions_score(&manifest),
                );
                signals.insert(
                    "has_publisher".into(),
                    if manifest.publisher.is_some() {
                        1.0
                    } else {
                        0.0
                    },
                );
                signals.insert(
                    "immutable_source".into(),
                    if manifest.immutable_source.is_some() {
                        1.0
                    } else {
                        0.0
                    },
                );
                signals.insert(
                    "risk_control".into(),
                    1.0 - f64::from(review.risk.score) / 100.0,
                );

                let total: f64 = signals.values().sum();
                let score = total / signals.len() as f64;

                RankedCandidate {
                    manifest,
                    score,
                    signals,
                    review,
                }
            })
            .collect();

        ranked.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        ranked
    }
}

fn validate_search_query(query: &SearchQuery) -> Result<(), RegistryError> {
    let combined = format!("{} {}", query.capability, query.keywords.join(" "));
    let lower = combined.to_ascii_lowercase();
    let unsafe_query = combined.len() > 512
        || combined.lines().count() > 3
        || combined.contains("/Users/")
        || combined.contains("C:\\Users\\")
        || [
            "api_key=",
            "apikey=",
            "authorization:",
            "bearer ",
            "password=",
            "secret=",
        ]
        .iter()
        .any(|marker| lower.contains(marker));
    if unsafe_query {
        Err(RegistryError::UnsafeQuery)
    } else {
        Ok(())
    }
}

fn source_trust_score(source: &str) -> f64 {
    match source {
        "official_registry" => 1.0,
        "org_registry" => 0.9,
        "github" => 0.5,
        "web_discovery" => 0.2,
        _ => 0.3,
    }
}

fn limited_permissions_score(manifest: &CandidateManifest) -> f64 {
    if !manifest.permissions_reviewed {
        return 0.0;
    }
    let has_write = manifest
        .permissions
        .get("write")
        .is_some_and(|w| !w.is_empty());
    let has_secrets = manifest
        .permissions
        .get("secrets")
        .is_some_and(|s| !s.is_empty());
    let has_network = manifest.network_access.is_some();

    match (has_write, has_secrets, has_network) {
        (false, false, false) => 1.0,
        (true, false, false) => 0.6,
        (true, true, _) => 0.0,
        (false, false, true) => 0.7,
        _ => 0.3,
    }
}

pub fn review_candidate(manifest: &CandidateManifest) -> CandidateReview {
    let mut score = 0_u16;
    let mut findings = Vec::new();
    let mut add_finding = |points: u16, code: &str, detail: &str| {
        score = score.saturating_add(points);
        findings.push(RiskFinding {
            code: code.into(),
            detail: detail.into(),
        });
    };

    if manifest.immutable_source.is_none() {
        add_finding(
            35,
            "mutable_source",
            "source is not pinned to an immutable commit or release digest",
        );
    }
    if !manifest.permissions_reviewed {
        add_finding(
            30,
            "permissions_unknown",
            "permission declarations have not been inspected",
        );
    }
    if manifest
        .permissions
        .get("write")
        .is_some_and(|entries| entries.iter().any(|entry| entry == "**/*" || entry == "*"))
    {
        add_finding(
            25,
            "broad_write",
            "declared write access covers the full workspace",
        );
    }
    if manifest
        .permissions
        .get("secrets")
        .is_some_and(|entries| !entries.is_empty())
    {
        add_finding(
            20,
            "secret_access",
            "candidate requests one or more secret references",
        );
    }
    if let Some(network) = manifest.network_access.as_deref() {
        add_finding(
            if network == "any" || network == "*" {
                20
            } else {
                8
            },
            "network_access",
            "candidate requests outbound network access",
        );
    }
    if manifest.signature_status != "verified" {
        add_finding(
            10,
            "signature_unverified",
            "publisher signature has not been verified",
        );
    }
    if manifest.maintenance.archived == Some(true) {
        add_finding(25, "archived", "source repository is archived");
    }
    if manifest.maintenance.observed_at.is_none() {
        add_finding(
            5,
            "maintenance_unknown",
            "maintenance metadata has not been observed",
        );
    }

    let score = score.min(100) as u8;
    let level = match score {
        0..=19 => RiskLevel::Low,
        20..=44 => RiskLevel::Medium,
        45..=69 => RiskLevel::High,
        _ => RiskLevel::Critical,
    };
    CandidateReview {
        source: SourceReview {
            source_type: manifest.source_type.clone(),
            source_url: manifest.source_url.clone(),
            publisher: manifest.publisher.clone(),
            signature_status: manifest.signature_status.clone(),
            immutable_source: manifest.immutable_source.clone(),
        },
        permissions: PermissionReview {
            inspected: manifest.permissions_reviewed,
            declared: manifest.permissions.clone(),
            network_access: manifest.network_access.clone(),
        },
        maintenance: manifest.maintenance.clone(),
        risk: RiskAssessment {
            level,
            score,
            findings,
        },
    }
}

fn qualification_passed(status: &QualificationStatus) -> bool {
    matches!(
        status,
        QualificationStatus::Qualified | QualificationStatus::QualifiedWithConstraints
    )
}

fn validate_hex_digest(value: &str, length: usize, label: &str) -> Result<(), RegistryError> {
    if value.len() != length || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(RegistryError::InvalidSourcePin(format!(
            "{label} must contain exactly {length} hexadecimal characters"
        )));
    }
    Ok(())
}

pub fn validate_source_pin(pin: &ImmutableSourcePin) -> Result<(), RegistryError> {
    let archive_url = reqwest::Url::parse(pin.archive_url())
        .map_err(|_| RegistryError::InvalidSourcePin("archive URL is invalid".into()))?;
    if archive_url.scheme() != "https"
        || archive_url.host_str().is_none()
        || !archive_url.username().is_empty()
        || archive_url.password().is_some()
    {
        return Err(RegistryError::InvalidSourcePin(
            "archive URL must be public HTTPS without credentials".into(),
        ));
    }
    match pin {
        ImmutableSourcePin::GitCommit { commit, .. } => {
            if !matches!(commit.len(), 40 | 64)
                || !commit.bytes().all(|byte| byte.is_ascii_hexdigit())
            {
                return Err(RegistryError::InvalidSourcePin(
                    "Git commit must be a full 40- or 64-character object ID".into(),
                ));
            }
            if !archive_url.path().contains(commit) {
                return Err(RegistryError::InvalidSourcePin(
                    "Git archive URL does not contain the approved commit".into(),
                ));
            }
        }
        ImmutableSourcePin::ReleaseAsset {
            release, sha256, ..
        } => {
            if release.trim().is_empty() {
                return Err(RegistryError::InvalidSourcePin(
                    "release identifier is empty".into(),
                ));
            }
            validate_hex_digest(sha256, 64, "release asset SHA-256")?;
        }
    }
    Ok(())
}

#[allow(dead_code)]
fn default_platform() -> String {
    if cfg!(target_os = "macos") {
        "macos".into()
    } else if cfg!(target_os = "linux") {
        "linux".into()
    } else {
        "windows".into()
    }
}

pub struct Qualifier;

impl Qualifier {
    pub fn verify_signature(manifest: &CandidateManifest) -> Result<(), RegistryError> {
        let digest = manifest.content_digest.as_deref().ok_or_else(|| {
            RegistryError::InvalidSignature("publisher content digest is missing".into())
        })?;
        let public_key = manifest.publisher_public_key.as_deref().ok_or_else(|| {
            RegistryError::InvalidSignature("publisher public key is missing".into())
        })?;
        let signature = manifest.signature.as_deref().ok_or_else(|| {
            RegistryError::InvalidSignature("publisher signature is missing".into())
        })?;
        Self::verify_digest_signature(digest, public_key, signature)
    }

    pub fn verify_digest_signature(
        digest: &str,
        public_key: &str,
        signature: &str,
    ) -> Result<(), RegistryError> {
        use base64::Engine;

        let key_bytes = hex::decode(public_key)
            .map_err(|_| RegistryError::InvalidSignature("public key is not hex".into()))?;
        let key_array: [u8; 32] = key_bytes.try_into().map_err(|_| {
            RegistryError::InvalidSignature("public key must contain 32 bytes".into())
        })?;
        let key = ed25519_dalek::VerifyingKey::from_bytes(&key_array)
            .map_err(|_| RegistryError::InvalidSignature("public key is invalid".into()))?;
        let signature_bytes = base64::engine::general_purpose::STANDARD
            .decode(signature)
            .map_err(|_| RegistryError::InvalidSignature("signature is not base64".into()))?;
        let signature = ed25519_dalek::Signature::from_slice(&signature_bytes).map_err(|_| {
            RegistryError::InvalidSignature("signature must contain 64 bytes".into())
        })?;
        key.verify_strict(digest.as_bytes(), &signature)
            .map_err(|_| RegistryError::InvalidSignature("signature does not match digest".into()))
    }

    pub fn validate_manifest(manifest: &CandidateManifest) -> Result<(), RegistryError> {
        if manifest.name.is_empty() {
            return Err(RegistryError::InvalidManifest("name is empty".into()));
        }
        if manifest.version.is_empty() {
            return Err(RegistryError::InvalidManifest("version is empty".into()));
        }
        if manifest.name.len() > 128 {
            return Err(RegistryError::InvalidManifest("name too long".into()));
        }
        for ch in manifest.name.chars() {
            if !ch.is_ascii_alphanumeric() && ch != '-' && ch != '_' && ch != '.' {
                return Err(RegistryError::InvalidManifest(format!(
                    "invalid character in name: {ch}"
                )));
            }
        }
        Ok(())
    }

    pub fn validate_downloadable_source(
        manifest: &CandidateManifest,
    ) -> Result<&ImmutableSourcePin, RegistryError> {
        let pin = manifest
            .immutable_source
            .as_ref()
            .ok_or(RegistryError::UnpinnedSource)?;
        validate_source_pin(pin)?;
        Ok(pin)
    }

    pub fn evaluate_compatibility(
        manifest: &CandidateManifest,
        platform: &str,
        purrcode_version: &str,
    ) -> QualificationStatus {
        if !manifest.detected_platforms.is_empty()
            && !manifest.detected_platforms.contains(&platform.to_string())
        {
            return QualificationStatus::Incompatible;
        }

        if let Some(min_ver) = &manifest.min_purrcode_version {
            if min_ver.as_str() > purrcode_version {
                return QualificationStatus::Incompatible;
            }
        }

        if manifest
            .permissions
            .get("write")
            .is_some_and(|w| w.contains(&"**/*".into()))
        {
            return QualificationStatus::QualifiedWithConstraints;
        }

        QualificationStatus::Qualified
    }
}

// ── Built-in adapter stubs ───────────────────────────────────────

pub struct OfficialRegistryAdapter {
    client: Result<reqwest::Client, String>,
    base_url: String,
}

impl OfficialRegistryAdapter {
    pub fn new(base_url: &str) -> Self {
        Self {
            client: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .connect_timeout(std::time::Duration::from_secs(10))
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .map_err(|error| error.to_string()),
            base_url: base_url.trim_end_matches('/').to_string(),
        }
    }
}

#[async_trait]
impl RegistryAdapter for OfficialRegistryAdapter {
    fn name(&self) -> &str {
        "official_registry"
    }

    async fn search(&self, _query: &SearchQuery) -> Result<Vec<CandidateManifest>, RegistryError> {
        let url = format!("{}/v1/skills/search", self.base_url);
        let client = self
            .client
            .as_ref()
            .map_err(|error| RegistryError::Adapter(format!("HTTP client unavailable: {error}")))?;
        let resp = client.post(&url).json(_query).send().await?;
        decode_bounded_json(resp).await
    }

    async fn fetch_manifest(&self, candidate_id: &str) -> Result<CandidateManifest, RegistryError> {
        let url = format!("{}/v1/skills/{}", self.base_url, candidate_id);
        let client = self
            .client
            .as_ref()
            .map_err(|error| RegistryError::Adapter(format!("HTTP client unavailable: {error}")))?;
        let resp = client.get(&url).send().await?;
        decode_bounded_json(resp).await
    }
}

pub struct GitHubRegistryAdapter {
    client: Result<reqwest::Client, String>,
}

impl Default for GitHubRegistryAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl GitHubRegistryAdapter {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .connect_timeout(std::time::Duration::from_secs(10))
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .map_err(|error| error.to_string()),
        }
    }
}

#[async_trait]
impl RegistryAdapter for GitHubRegistryAdapter {
    fn name(&self) -> &str {
        "github"
    }

    async fn search(&self, query: &SearchQuery) -> Result<Vec<CandidateManifest>, RegistryError> {
        let q = format!("{} purrcode skill", query.keywords.join(" "));
        let url = format!(
            "https://api.github.com/search/repositories?q={}&sort=updated&per_page=10",
            urlencoding(&q)
        );

        let resp = self
            .client
            .as_ref()
            .map_err(|error| RegistryError::Adapter(format!("HTTP client unavailable: {error}")))?
            .get(&url)
            .header("User-Agent", "PurrCode/0.1")
            .header("Accept", "application/vnd.github.v3+json")
            .send()
            .await?;

        let body: serde_json::Value = decode_bounded_json(resp).await?;
        let items = body["items"].as_array().map_or_else(Vec::new, |items| {
            items
                .iter()
                .filter_map(|item| {
                    let name = item["name"].as_str()?;
                    let full_name = item["full_name"].as_str()?;
                    let description = item["description"].as_str().unwrap_or("");
                    let pushed_at = item["pushed_at"]
                        .as_str()
                        .and_then(|timestamp| timestamp.parse::<DateTime<Utc>>().ok());

                    Some(CandidateManifest {
                        candidate_id: format!("github:{full_name}"),
                        name: name.to_string(),
                        version: "0.1.0".into(),
                        publisher: Some(
                            item["owner"]["login"]
                                .as_str()
                                .unwrap_or("unknown")
                                .to_string(),
                        ),
                        source_type: "github".into(),
                        source_url: Some(item["html_url"].as_str()?.to_string()),
                        description: description.to_string(),
                        signature_status: "unavailable".into(),
                        signature: None,
                        publisher_public_key: None,
                        permissions: BTreeMap::new(),
                        network_access: None,
                        dependencies: Vec::new(),
                        license: item["license"]["spdx_id"].as_str().map(|s| s.to_string()),
                        content_digest: None,
                        file_count: 0,
                        reported_size_bytes: item["size"].as_u64().unwrap_or(0),
                        detected_platforms: Vec::new(),
                        min_purrcode_version: None,
                        permissions_reviewed: false,
                        immutable_source: None,
                        maintenance: MaintenanceMetadata {
                            observed_at: Some(Utc::now()),
                            last_push_at: pushed_at,
                            last_release_at: None,
                            archived: item["archived"].as_bool(),
                            stars: item["stargazers_count"].as_u64(),
                            open_issues: item["open_issues_count"].as_u64(),
                        },
                    })
                })
                .collect()
        });

        Ok(items)
    }

    async fn fetch_manifest(&self, candidate_id: &str) -> Result<CandidateManifest, RegistryError> {
        let path = candidate_id.strip_prefix("github:").unwrap_or(candidate_id);
        validate_github_repository(path)?;
        let client = self
            .client
            .as_ref()
            .map_err(|error| RegistryError::Adapter(format!("HTTP client unavailable: {error}")))?;
        let repository: serde_json::Value = decode_bounded_json(
            client
                .get(format!("https://api.github.com/repos/{path}"))
                .header("User-Agent", "PurrCode/0.1")
                .header("Accept", "application/vnd.github.v3+json")
                .send()
                .await?,
        )
        .await?;
        let default_branch = repository["default_branch"]
            .as_str()
            .ok_or_else(|| RegistryError::Adapter("repository default branch is missing".into()))?;
        let commit: serde_json::Value = decode_bounded_json(
            client
                .get(format!(
                    "https://api.github.com/repos/{path}/commits/{}",
                    urlencoding(default_branch)
                ))
                .header("User-Agent", "PurrCode/0.1")
                .header("Accept", "application/vnd.github.v3+json")
                .send()
                .await?,
        )
        .await?;
        let commit = commit["sha"]
            .as_str()
            .ok_or_else(|| RegistryError::Adapter("repository commit is missing".into()))?;
        if !matches!(commit.len(), 40 | 64) || !commit.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(RegistryError::Adapter(
                "repository returned an invalid commit object id".into(),
            ));
        }
        let manifest: serde_json::Value = decode_bounded_json(
            client
                .get(format!(
                    "https://api.github.com/repos/{path}/contents/manifest.toml?ref={commit}"
                ))
                .header("User-Agent", "PurrCode/0.1")
                .header("Accept", "application/vnd.github.v3+json")
                .send()
                .await?,
        )
        .await?;
        let reported_manifest_size = manifest["size"].as_u64().unwrap_or(u64::MAX);
        if manifest["type"].as_str() != Some("file") || reported_manifest_size > 256 * 1024 {
            return Err(RegistryError::Adapter(
                "manifest.toml is missing or exceeds 256 KiB".into(),
            ));
        }
        use base64::Engine;
        let encoded_manifest = manifest["content"]
            .as_str()
            .ok_or_else(|| RegistryError::Adapter("manifest.toml content is missing".into()))?;
        let decoded_manifest = base64::engine::general_purpose::STANDARD
            .decode(
                encoded_manifest
                    .bytes()
                    .filter(|byte| !byte.is_ascii_whitespace())
                    .collect::<Vec<_>>(),
            )
            .map_err(|_| RegistryError::Adapter("manifest.toml is not valid base64".into()))?;
        if decoded_manifest.len() > 256 * 1024 || decoded_manifest.is_empty() {
            return Err(RegistryError::Adapter(
                "manifest.toml is empty or exceeds 256 KiB".into(),
            ));
        }
        let name = repository["name"]
            .as_str()
            .ok_or_else(|| RegistryError::Adapter("repository name is missing".into()))?;
        let full_name = repository["full_name"].as_str().unwrap_or(path);
        let owner = repository["owner"]["login"].as_str().map(str::to_owned);
        let description = repository["description"].as_str().unwrap_or("");
        let pushed_at = repository["pushed_at"]
            .as_str()
            .and_then(|timestamp| timestamp.parse::<DateTime<Utc>>().ok());
        let short_commit = commit.chars().take(12).collect::<String>();

        Ok(CandidateManifest {
            candidate_id: format!("github:{full_name}"),
            name: name.to_owned(),
            version: format!("git-{short_commit}"),
            publisher: owner,
            source_type: "github".into(),
            source_url: repository["html_url"].as_str().map(str::to_owned),
            description: description.to_owned(),
            signature_status: "unavailable".into(),
            signature: None,
            publisher_public_key: None,
            permissions: BTreeMap::new(),
            network_access: None,
            dependencies: Vec::new(),
            license: repository["license"]["spdx_id"].as_str().map(str::to_owned),
            content_digest: None,
            file_count: 0,
            reported_size_bytes: repository["size"]
                .as_u64()
                .unwrap_or(0)
                .saturating_mul(1024),
            detected_platforms: Vec::new(),
            min_purrcode_version: None,
            permissions_reviewed: false,
            immutable_source: Some(ImmutableSourcePin::GitCommit {
                commit: commit.to_ascii_lowercase(),
                archive_url: format!("https://codeload.github.com/{path}/zip/{commit}"),
            }),
            maintenance: MaintenanceMetadata {
                observed_at: Some(Utc::now()),
                last_push_at: pushed_at,
                last_release_at: None,
                archived: repository["archived"].as_bool(),
                stars: repository["stargazers_count"].as_u64(),
                open_issues: repository["open_issues_count"].as_u64(),
            },
        })
    }
}

fn validate_github_repository(path: &str) -> Result<(), RegistryError> {
    let parts = path.split('/').collect::<Vec<_>>();
    if parts.len() != 2
        || parts.iter().any(|part| {
            part.is_empty()
                || matches!(*part, "." | "..")
                || part.len() > 100
                || !part.chars().all(|character| {
                    character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
                })
        })
    {
        return Err(RegistryError::InvalidManifest(
            "GitHub candidate must be an owner/repository identity".into(),
        ));
    }
    Ok(())
}

fn urlencoding(s: &str) -> String {
    s.bytes()
        .map(|byte| match byte {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                char::from(byte).to_string()
            }
            b' ' => "+".into(),
            other => format!("%{other:02X}"),
        })
        .collect()
}

async fn decode_bounded_json<T: DeserializeOwned>(
    mut response: reqwest::Response,
) -> Result<T, RegistryError> {
    if !response.status().is_success() {
        return Err(RegistryError::Adapter(format!(
            "registry returned HTTP {}",
            response.status()
        )));
    }
    if response
        .content_length()
        .is_some_and(|length| length > MAX_REGISTRY_RESPONSE_BYTES as u64)
    {
        return Err(RegistryError::Adapter(
            "registry response exceeds 1 MiB".into(),
        ));
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if bytes.len().saturating_add(chunk.len()) > MAX_REGISTRY_RESPONSE_BYTES {
            return Err(RegistryError::Adapter(
                "registry response exceeds 1 MiB".into(),
            ));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(serde_json::from_slice(&bytes)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use purrcode_skill_store::{SkillRecord, SkillScope};
    use std::future::Future;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::task::{Context, Poll, Waker};

    fn block_on<F: Future>(future: F) -> F::Output {
        let mut context = Context::from_waker(Waker::noop());
        let mut future = Box::pin(future);
        loop {
            match future.as_mut().poll(&mut context) {
                Poll::Ready(output) => return output,
                Poll::Pending => std::thread::yield_now(),
            }
        }
    }

    struct CountingAdapter {
        calls: Arc<AtomicUsize>,
        candidate: CandidateManifest,
    }

    #[async_trait]
    impl RegistryAdapter for CountingAdapter {
        fn name(&self) -> &str {
            "counting"
        }

        async fn search(
            &self,
            _query: &SearchQuery,
        ) -> Result<Vec<CandidateManifest>, RegistryError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(vec![self.candidate.clone()])
        }

        async fn fetch_manifest(
            &self,
            _candidate_id: &str,
        ) -> Result<CandidateManifest, RegistryError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(self.candidate.clone())
        }
    }

    struct MockInstalled {
        resolution: InstalledCapabilityResolution,
    }

    impl InstalledCapabilitySource for MockInstalled {
        fn resolve_installed(
            &self,
            _capability: &str,
        ) -> Result<InstalledCapabilityResolution, RegistryError> {
            Ok(self.resolution.clone())
        }
    }

    #[test]
    fn validate_manifest_rejects_empty_name() {
        let m = CandidateManifest {
            candidate_id: "x".into(),
            name: "".into(),
            version: "1.0".into(),
            publisher: None,
            source_type: "test".into(),
            source_url: None,
            description: "".into(),
            signature_status: "unavailable".into(),
            signature: None,
            publisher_public_key: None,
            permissions: BTreeMap::new(),
            network_access: None,
            dependencies: Vec::new(),
            license: None,
            content_digest: None,
            file_count: 0,
            reported_size_bytes: 0,
            detected_platforms: Vec::new(),
            min_purrcode_version: None,
            permissions_reviewed: false,
            immutable_source: None,
            maintenance: MaintenanceMetadata::default(),
        };
        assert!(Qualifier::validate_manifest(&m).is_err());
    }

    #[test]
    fn validate_manifest_accepts_valid() {
        let m = CandidateManifest {
            candidate_id: "test-skill".into(),
            name: "test-skill".into(),
            version: "1.0.0".into(),
            publisher: Some("example".into()),
            source_type: "registry".into(),
            source_url: None,
            description: "A test skill".into(),
            signature_status: "unavailable".into(),
            signature: None,
            publisher_public_key: None,
            permissions: {
                let mut p = BTreeMap::new();
                p.insert("read".into(), vec!["**/*.tf".into()]);
                p
            },
            network_access: Some("registry.terraform.io".into()),
            dependencies: Vec::new(),
            license: Some("Apache-2.0".into()),
            content_digest: Some("sha256:abc".into()),
            file_count: 5,
            reported_size_bytes: 1024,
            detected_platforms: vec!["macos".into(), "linux".into()],
            min_purrcode_version: Some("0.1.0".into()),
            permissions_reviewed: true,
            immutable_source: Some(ImmutableSourcePin::GitCommit {
                commit: "a".repeat(40),
                archive_url: format!(
                    "https://codeload.github.com/example/test/zip/{}",
                    "a".repeat(40)
                ),
            }),
            maintenance: MaintenanceMetadata {
                observed_at: Some(Utc::now()),
                ..Default::default()
            },
        };
        assert!(Qualifier::validate_manifest(&m).is_ok());
    }

    #[test]
    fn limited_permissions_scoring() {
        let mut no_perms = CandidateManifest::default_for_test();
        no_perms.permissions_reviewed = true;
        assert_eq!(limited_permissions_score(&no_perms), 1.0);

        let mut write_only = no_perms.clone();
        write_only
            .permissions
            .insert("write".into(), vec!["**/*.rs".into()]);
        assert_eq!(limited_permissions_score(&write_only), 0.6);

        let mut full_access = no_perms;
        full_access
            .permissions
            .insert("write".into(), vec!["**/*".into()]);
        full_access
            .permissions
            .insert("secrets".into(), vec!["*".into()]);
        full_access.network_access = Some("any".into());
        assert_eq!(limited_permissions_score(&full_access), 0.0);
    }

    #[test]
    fn publisher_signature_is_verified_against_declared_digest() {
        use base64::Engine;
        use ed25519_dalek::{Signer, SigningKey};

        let signing_key = SigningKey::from_bytes(&[7_u8; 32]);
        let mut manifest = CandidateManifest::default_for_test();
        manifest.content_digest = Some("digest-value".into());
        manifest.publisher_public_key = Some(hex::encode(signing_key.verifying_key().as_bytes()));
        manifest.signature = Some(
            base64::engine::general_purpose::STANDARD
                .encode(signing_key.sign(b"digest-value").to_bytes()),
        );
        assert!(Qualifier::verify_signature(&manifest).is_ok());
        manifest.content_digest = Some("tampered".into());
        assert!(Qualifier::verify_signature(&manifest).is_err());
    }

    #[test]
    fn registry_query_rejects_private_paths_and_credentials() {
        let unsafe_path = SearchQuery {
            capability: "error in /Users/alice/private-repository".into(),
            keywords: Vec::new(),
            platform: "macos".into(),
            purrcode_version: "0.1.0".into(),
        };
        assert!(matches!(
            validate_search_query(&unsafe_path),
            Err(RegistryError::UnsafeQuery)
        ));
        let safe = SearchQuery {
            capability: "terraform-schema-inspection".into(),
            keywords: vec!["terraform".into()],
            platform: "macos".into(),
            purrcode_version: "0.1.0".into(),
        };
        assert!(validate_search_query(&safe).is_ok());
    }

    #[test]
    fn github_candidate_identity_is_strictly_bounded() {
        assert!(validate_github_repository("owner/repository").is_ok());
        assert!(validate_github_repository("../escape").is_err());
        assert!(validate_github_repository("owner/repository/extra").is_err());
        assert!(validate_github_repository("owner/repo?ref=main").is_err());
    }

    #[test]
    fn public_search_permission_is_explicit_and_exact() {
        let calls = Arc::new(AtomicUsize::new(0));
        let engine = RegistryEngine::new(vec![Box::new(CountingAdapter {
            calls: calls.clone(),
            candidate: CandidateManifest::default_for_test(),
        })]);
        let query = safe_query();
        assert!(matches!(
            block_on(engine.search(&query)),
            Err(RegistryError::PublicWebPermissionRequired)
        ));
        assert_eq!(calls.load(Ordering::SeqCst), 0);

        let authorization = ExternalSearchAuthorization::new(
            "durable-search-action",
            query.clone(),
            vec!["counting".into()],
            Utc::now() + chrono::Duration::minutes(5),
        )
        .unwrap();
        assert_eq!(
            block_on(engine.search_authorized(&query, &authorization))
                .unwrap()
                .len(),
            1
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(matches!(
            block_on(engine.search_authorized(&query, &authorization)),
            Err(RegistryError::AuthorizationConsumed)
        ));
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        let mut changed = query;
        changed.keywords.push("different".into());
        assert!(matches!(
            block_on(engine.search_authorized(&changed, &authorization)),
            Err(RegistryError::AuthorizationMismatch)
        ));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn manifest_inspection_has_its_own_exact_public_web_permission() {
        let calls = Arc::new(AtomicUsize::new(0));
        let engine = RegistryEngine::new(vec![Box::new(CountingAdapter {
            calls: calls.clone(),
            candidate: CandidateManifest::default_for_test(),
        })]);
        assert!(matches!(
            block_on(engine.fetch_manifest("github:example/skill")),
            Err(RegistryError::PublicWebPermissionRequired)
        ));
        assert_eq!(calls.load(Ordering::SeqCst), 0);

        let authorization = CandidateInspectionAuthorization::new(
            "inspect-action",
            "github:example/skill",
            "counting",
            Utc::now() + chrono::Duration::minutes(5),
        )
        .unwrap();
        assert!(
            block_on(engine.fetch_manifest_authorized("github:other/skill", &authorization,))
                .is_err()
        );
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert!(block_on(
            engine.fetch_manifest_authorized("github:example/skill", &authorization,)
        )
        .is_ok());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn installed_qualified_skill_avoids_every_external_adapter() {
        let calls = Arc::new(AtomicUsize::new(0));
        let engine = RegistryEngine::new(vec![Box::new(CountingAdapter {
            calls: calls.clone(),
            candidate: CandidateManifest::default_for_test(),
        })]);
        let installed = MockInstalled {
            resolution: InstalledCapabilityResolution {
                capability: "github-pr".into(),
                qualified_matches: vec![qualified_skill_record()],
                non_invocable_matches: Vec::new(),
                external_search_avoided: true,
            },
        };
        let outcome =
            block_on(engine.discover_installed_first(&installed, &safe_query(), None)).unwrap();
        assert!(matches!(
            outcome,
            DiscoveryOutcome::Installed {
                external_search_avoided: true,
                ..
            }
        ));
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn unqualified_local_match_does_not_bypass_web_permission() {
        let calls = Arc::new(AtomicUsize::new(0));
        let engine = RegistryEngine::new(vec![Box::new(CountingAdapter {
            calls: calls.clone(),
            candidate: CandidateManifest::default_for_test(),
        })]);
        let mut non_invocable = qualified_skill_record();
        non_invocable.qualification_status = QualificationStatus::Unverified;
        let installed = MockInstalled {
            resolution: InstalledCapabilityResolution {
                capability: "github-pr".into(),
                qualified_matches: Vec::new(),
                non_invocable_matches: vec![non_invocable],
                external_search_avoided: false,
            },
        };
        assert!(matches!(
            block_on(engine.discover_installed_first(&installed, &safe_query(), None)),
            Err(RegistryError::PublicWebPermissionRequired)
        ));
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn candidate_review_exposes_unknown_permissions_maintenance_source_and_risk() {
        let mut candidate = CandidateManifest::default_for_test();
        candidate.source_type = "github".into();
        candidate.source_url = Some("https://github.com/example/skill".into());
        candidate.publisher = Some("example".into());
        candidate.signature_status = "unavailable".into();
        candidate.permissions_reviewed = false;
        candidate.maintenance.archived = Some(true);
        candidate.maintenance.observed_at = Some(Utc::now());

        let review = review_candidate(&candidate);
        assert!(!review.permissions.inspected);
        assert!(review.source.immutable_source.is_none());
        assert_eq!(review.maintenance.archived, Some(true));
        assert!(matches!(
            review.risk.level,
            RiskLevel::High | RiskLevel::Critical
        ));
        assert!(review
            .risk
            .findings
            .iter()
            .any(|finding| finding.code == "permissions_unknown"));
        assert!(review
            .risk
            .findings
            .iter()
            .any(|finding| finding.code == "mutable_source"));
    }

    #[test]
    fn download_requires_a_full_commit_or_release_asset_digest() {
        let mut candidate = CandidateManifest::default_for_test();
        assert!(matches!(
            Qualifier::validate_downloadable_source(&candidate),
            Err(RegistryError::UnpinnedSource)
        ));
        candidate.immutable_source = Some(ImmutableSourcePin::GitCommit {
            commit: "main".into(),
            archive_url: "https://codeload.github.com/example/skill/zip/main".into(),
        });
        assert!(matches!(
            Qualifier::validate_downloadable_source(&candidate),
            Err(RegistryError::InvalidSourcePin(_))
        ));

        let commit = "a".repeat(40);
        candidate.immutable_source = Some(ImmutableSourcePin::GitCommit {
            archive_url: format!("https://codeload.github.com/example/skill/zip/{commit}"),
            commit,
        });
        assert!(Qualifier::validate_downloadable_source(&candidate).is_ok());
    }

    #[test]
    fn dynamic_qualification_all_non_passing_states_fail_closed() {
        for status in [
            QualificationStatus::Unverified,
            QualificationStatus::Failed,
            QualificationStatus::Blocked,
            QualificationStatus::Outdated,
            QualificationStatus::Incompatible,
        ] {
            let mut workflow = workflow_waiting_for_dynamic();
            workflow
                .record_dynamic_qualification(status.clone())
                .unwrap();
            assert_eq!(
                workflow.stage,
                CandidateWorkflowStage::DynamicRejected { status }
            );
            assert!(!workflow.is_installable());
            assert!(matches!(
                workflow.authorize_install(install_authorization(), &"b".repeat(64)),
                Err(RegistryError::InvalidTransition(_))
            ));
        }
    }

    #[test]
    fn passing_dynamic_qualification_still_requires_a_distinct_exact_install_approval() {
        for status in [
            QualificationStatus::Qualified,
            QualificationStatus::QualifiedWithConstraints,
        ] {
            let mut workflow = workflow_waiting_for_dynamic();
            workflow.record_dynamic_qualification(status).unwrap();
            assert!(workflow.is_installable());
            assert!(matches!(
                workflow.authorize_install(
                    ExactActionAuthorization::new("download-auth", "install-action").unwrap(),
                    &"b".repeat(64),
                ),
                Err(RegistryError::ReusedAuthorization)
            ));
            workflow
                .authorize_install(install_authorization(), &"b".repeat(64))
                .unwrap();
            workflow.record_installed(&"b".repeat(64)).unwrap();
            assert!(matches!(
                workflow.stage,
                CandidateWorkflowStage::Installed { .. }
            ));
        }
    }

    #[test]
    fn release_download_digest_mismatch_blocks_the_workflow() {
        let mut workflow = CandidateWorkflow::new(
            "release-skill",
            ImmutableSourcePin::ReleaseAsset {
                release: "v1.0.0".into(),
                archive_url: "https://example.com/releases/v1.0.0/skill.zip".into(),
                sha256: "a".repeat(64),
            },
        )
        .unwrap();
        workflow
            .authorize_download(
                ExactActionAuthorization::new("download-auth", "download-action").unwrap(),
            )
            .unwrap();
        assert!(matches!(
            workflow.record_download(&"c".repeat(64), &"b".repeat(64)),
            Err(RegistryError::DigestMismatch { .. })
        ));
        assert_eq!(
            workflow.stage,
            CandidateWorkflowStage::IntegrityBlocked {
                status: QualificationStatus::Blocked
            }
        );
    }

    fn safe_query() -> SearchQuery {
        SearchQuery {
            capability: "github-pr".into(),
            keywords: vec!["pull request reader".into()],
            platform: "macos".into(),
            purrcode_version: "0.5.0".into(),
        }
    }

    fn qualified_skill_record() -> SkillRecord {
        SkillRecord {
            skill_id: "github-pr-reader".into(),
            version: "1.0.0".into(),
            scope: SkillScope::Repository,
            source_type: "local".into(),
            source_location: None,
            publisher: Some("trusted".into()),
            content_digest: "b".repeat(64),
            signature_status: "verified".into(),
            installed_at: Utc::now(),
            approved_permissions: serde_json::json!({"read": ["**/*"]}),
            qualification_status: QualificationStatus::Qualified,
            last_used_at: None,
            successful_uses: 0,
            failed_uses: 0,
            pinned: true,
        }
    }

    fn workflow_waiting_for_dynamic() -> CandidateWorkflow {
        let commit = "a".repeat(40);
        let mut workflow = CandidateWorkflow::new(
            "github-pr-reader",
            ImmutableSourcePin::GitCommit {
                archive_url: format!("https://codeload.github.com/example/skill/zip/{commit}"),
                commit,
            },
        )
        .unwrap();
        workflow
            .authorize_download(
                ExactActionAuthorization::new("download-auth", "download-action").unwrap(),
            )
            .unwrap();
        workflow
            .record_download(&"a".repeat(64), &"b".repeat(64))
            .unwrap();
        workflow
            .record_static_qualification(QualificationStatus::Qualified)
            .unwrap();
        workflow.start_dynamic_qualification().unwrap();
        workflow
    }

    fn install_authorization() -> ExactActionAuthorization {
        ExactActionAuthorization::new("install-auth", "install-action").unwrap()
    }

    impl CandidateManifest {
        fn default_for_test() -> Self {
            Self {
                candidate_id: String::new(),
                name: String::new(),
                version: String::new(),
                publisher: None,
                source_type: String::new(),
                source_url: None,
                description: String::new(),
                signature_status: String::new(),
                signature: None,
                publisher_public_key: None,
                permissions: BTreeMap::new(),
                network_access: None,
                dependencies: Vec::new(),
                license: None,
                content_digest: None,
                file_count: 0,
                reported_size_bytes: 0,
                detected_platforms: Vec::new(),
                min_purrcode_version: None,
                permissions_reviewed: false,
                immutable_source: None,
                maintenance: MaintenanceMetadata::default(),
            }
        }
    }
}
