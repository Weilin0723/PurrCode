//! Registry adapters, candidate discovery, provenance verification, and ranking.

use async_trait::async_trait;
use purrcode_runtime_core::QualificationStatus;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use thiserror::Error;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SearchQuery {
    pub capability: String,
    pub keywords: Vec<String>,
    pub platform: String,
    pub purrcode_version: String,
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
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RankedCandidate {
    pub manifest: CandidateManifest,
    pub score: f64,
    pub signals: BTreeMap<String, f64>,
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
}

#[async_trait]
pub trait RegistryAdapter: Send + Sync {
    fn name(&self) -> &str;
    async fn search(&self, query: &SearchQuery) -> Result<Vec<CandidateManifest>, RegistryError>;
    async fn fetch_manifest(&self, candidate_id: &str) -> Result<CandidateManifest, RegistryError>;
}

pub struct RegistryEngine {
    adapters: Vec<Box<dyn RegistryAdapter>>,
}

impl RegistryEngine {
    pub fn new(adapters: Vec<Box<dyn RegistryAdapter>>) -> Self {
        Self { adapters }
    }

    pub async fn search(&self, query: &SearchQuery) -> Result<Vec<RankedCandidate>, RegistryError> {
        validate_search_query(query)?;
        let mut all_candidates: Vec<(CandidateManifest, &str)> = Vec::new();

        for adapter in &self.adapters {
            match adapter.search(query).await {
                Ok(candidates) => {
                    for c in candidates {
                        all_candidates.push((c, adapter.name()));
                    }
                }
                Err(e) => {
                    eprintln!(
                        "[skill-registry] adapter {} search failed: {}",
                        adapter.name(),
                        e
                    );
                }
            }
        }

        if all_candidates.is_empty() {
            return Err(RegistryError::NoCandidates);
        }

        let ranked = self.rank(all_candidates);
        Ok(ranked)
    }

    pub async fn fetch_manifest(
        &self,
        candidate_id: &str,
    ) -> Result<CandidateManifest, RegistryError> {
        for adapter in &self.adapters {
            if let Ok(manifest) = adapter.fetch_manifest(candidate_id).await {
                return Ok(manifest);
            }
        }
        Err(RegistryError::NoCandidates)
    }

    fn rank(&self, candidates: Vec<(CandidateManifest, &str)>) -> Vec<RankedCandidate> {
        let mut ranked: Vec<RankedCandidate> = candidates
            .into_iter()
            .map(|(manifest, source)| {
                let mut signals = BTreeMap::new();

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

                let total: f64 = signals.values().sum();
                let score = total / signals.len() as f64;

                RankedCandidate {
                    manifest,
                    score,
                    signals,
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
    client: reqwest::Client,
    base_url: String,
}

impl OfficialRegistryAdapter {
    pub fn new(base_url: &str) -> Self {
        Self {
            client: reqwest::Client::new(),
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
        let resp = self.client.post(&url).json(_query).send().await?;
        let candidates: Vec<CandidateManifest> = resp.json().await?;
        Ok(candidates)
    }

    async fn fetch_manifest(&self, candidate_id: &str) -> Result<CandidateManifest, RegistryError> {
        let url = format!("{}/v1/skills/{}", self.base_url, candidate_id);
        let resp = self.client.get(&url).send().await?;
        let manifest: CandidateManifest = resp.json().await?;
        Ok(manifest)
    }
}

pub struct GitHubRegistryAdapter {
    client: reqwest::Client,
}

impl Default for GitHubRegistryAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl GitHubRegistryAdapter {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::new(),
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
            .get(&url)
            .header("User-Agent", "PurrCode/0.1")
            .header("Accept", "application/vnd.github.v3+json")
            .send()
            .await?;

        let body: serde_json::Value = resp.json().await?;
        let items = body["items"].as_array().map_or_else(Vec::new, |items| {
            items
                .iter()
                .filter_map(|item| {
                    let name = item["name"].as_str()?;
                    let full_name = item["full_name"].as_str()?;
                    let description = item["description"].as_str().unwrap_or("");
                    let _pushed_at = item["pushed_at"].as_str().unwrap_or("");

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
                    })
                })
                .collect()
        });

        Ok(items)
    }

    async fn fetch_manifest(&self, candidate_id: &str) -> Result<CandidateManifest, RegistryError> {
        let path = candidate_id.strip_prefix("github:").unwrap_or(candidate_id);
        let url = format!("https://api.github.com/repos/{path}/contents/manifest.toml");

        let resp = self
            .client
            .get(&url)
            .header("User-Agent", "PurrCode/0.1")
            .send()
            .await?;

        if !resp.status().is_success() {
            return Err(RegistryError::Adapter(format!(
                "manifest not found at {path}"
            )));
        }

        Err(RegistryError::Adapter("binary content not decoded".into()))
    }
}

fn urlencoding(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' | '.' | '~' => c.to_string(),
            ' ' => "+".into(),
            other => format!("%{:02X}", other as u8),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

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
        };
        assert!(Qualifier::validate_manifest(&m).is_ok());
    }

    #[test]
    fn limited_permissions_scoring() {
        let no_perms = CandidateManifest::default_for_test();
        assert_eq!(limited_permissions_score(&no_perms), 1.0);

        let mut write_only = CandidateManifest::default_for_test();
        write_only
            .permissions
            .insert("write".into(), vec!["**/*.rs".into()]);
        assert_eq!(limited_permissions_score(&write_only), 0.6);

        let mut full_access = CandidateManifest::default_for_test();
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
            }
        }
    }
}
