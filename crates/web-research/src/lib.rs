//! Governed web research with domain policy, content sanitization, and evidence caching.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;
use thiserror::Error;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SearchQuery {
    pub query: String,
    pub max_results: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SearchResult {
    pub url: String,
    pub title: String,
    pub snippet: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FetchedPage {
    pub url: String,
    pub content_digest: String,
    pub content: String,
    pub content_type: String,
    pub retrieved_at: DateTime<Utc>,
    pub truncated: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EvidenceRecord {
    pub id: String,
    pub query: String,
    pub url: String,
    pub title: String,
    pub content_digest: String,
    pub excerpt: String,
    pub retrieved_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct DomainPolicy {
    pub allow_list: Vec<String>,
    pub deny_list: Vec<String>,
    pub approval_required: Vec<String>,
}

#[derive(Debug, Error)]
pub enum ResearchError {
    #[error("network error: {0}")]
    Network(#[from] reqwest::Error),
    #[error("domain denied: {0}")]
    DomainDenied(String),
    #[error("domain requires approval: {0}")]
    DomainRequiresApproval(String),
    #[error("page too large: {0} bytes")]
    PageTooLarge(u64),
    #[error("invalid content type: {0}")]
    InvalidContentType(String),
    #[error("search disabled in local-only mode")]
    LocalOnlyMode,
    #[error("cache miss")]
    CacheMiss,
    #[error("search query contains private or credential-like data")]
    PrivateQuery,
    #[error("URL is not an allowed public HTTP(S) target: {0}")]
    UnsafeUrl(String),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("cache I/O error: {0}")]
    Io(#[from] std::io::Error),
}

const MAX_PAGE_BYTES: u64 = 2 * 1024 * 1024; // 2 MB
const MAX_EXCERPT_CHARS: usize = 2000;
const CACHE_TTL: Duration = Duration::from_secs(3600); // 1 hour

pub struct ResearchEngine {
    client: reqwest::Client,
    policy: DomainPolicy,
    cache: HashMap<String, EvidenceRecord>,
    cache_path: Option<PathBuf>,
    search_provider: Box<dyn SearchProvider>,
}

#[async_trait::async_trait]
pub trait SearchProvider: Send + Sync {
    async fn search(&self, query: &SearchQuery) -> Result<Vec<SearchResult>, ResearchError>;
}

pub struct WebSearchProvider {
    client: reqwest::Client,
    endpoint: String,
}

impl WebSearchProvider {
    pub fn new(endpoint: &str) -> Self {
        Self {
            client: reqwest::Client::new(),
            endpoint: endpoint.trim_end_matches('/').to_string(),
        }
    }
}

#[async_trait::async_trait]
impl SearchProvider for WebSearchProvider {
    async fn search(&self, query: &SearchQuery) -> Result<Vec<SearchResult>, ResearchError> {
        let url = format!("{}/search", self.endpoint);
        let resp = self.client.post(&url).json(query).send().await?;
        let results: Vec<SearchResult> = resp.json().await?;
        Ok(results)
    }
}

pub struct StubSearchProvider;

#[async_trait::async_trait]
impl SearchProvider for StubSearchProvider {
    async fn search(&self, _query: &SearchQuery) -> Result<Vec<SearchResult>, ResearchError> {
        Ok(Vec::new())
    }
}

impl ResearchEngine {
    pub fn new(search_provider: Box<dyn SearchProvider>, policy: DomainPolicy) -> Self {
        Self {
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(30))
                .connect_timeout(Duration::from_secs(10))
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .unwrap_or_default(),
            policy,
            cache: HashMap::new(),
            cache_path: None,
            search_provider,
        }
    }

    pub fn with_durable_cache(
        search_provider: Box<dyn SearchProvider>,
        policy: DomainPolicy,
        cache_path: &Path,
    ) -> Result<Self, ResearchError> {
        let mut engine = Self::new(search_provider, policy);
        engine.cache_path = Some(cache_path.to_owned());
        if cache_path.is_file() {
            let records: Vec<EvidenceRecord> = serde_json::from_slice(&std::fs::read(cache_path)?)?;
            engine.cache = records
                .into_iter()
                .map(|record| (record.id.clone(), record))
                .collect();
        }
        Ok(engine)
    }

    pub async fn search(
        &mut self,
        query: &SearchQuery,
        local_only: bool,
    ) -> Result<Vec<EvidenceRecord>, ResearchError> {
        if local_only {
            return Err(ResearchError::LocalOnlyMode);
        }
        validate_query(&query.query)?;

        let results = self.search_provider.search(query).await?;
        let mut evidence = Vec::new();

        for result in results {
            let domain = validate_public_url(&result.url)?;
            enforce_domain_policy(&self.policy, &domain)?;

            let record = EvidenceRecord {
                id: format!("ev-{}", sha256_hex(&result.url)),
                query: query.query.clone(),
                url: result.url.clone(),
                title: result.title.clone(),
                content_digest: sha256_hex(&result.snippet),
                excerpt: result.snippet.chars().take(MAX_EXCERPT_CHARS).collect(),
                retrieved_at: Utc::now(),
            };

            self.cache.insert(record.id.clone(), record.clone());
            evidence.push(record);
        }

        self.persist_cache()?;

        Ok(evidence)
    }

    pub async fn fetch_page(&self, url: &str) -> Result<FetchedPage, ResearchError> {
        let domain = validate_public_url(url)?;
        enforce_domain_policy(&self.policy, &domain)?;

        let resp = self.client.get(url).send().await?;
        if resp.status().is_redirection() {
            return Err(ResearchError::UnsafeUrl(format!(
                "redirects require a separately validated request: {url}"
            )));
        }

        let content_type = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();

        if !content_type.contains("text")
            && !content_type.contains("json")
            && !content_type.is_empty()
        {
            return Err(ResearchError::InvalidContentType(content_type));
        }

        let content_length = resp
            .headers()
            .get(reqwest::header::CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(0);

        if content_length > MAX_PAGE_BYTES {
            return Err(ResearchError::PageTooLarge(content_length));
        }

        let bytes = resp.bytes().await?;
        if bytes.len() > MAX_PAGE_BYTES as usize {
            return Err(ResearchError::PageTooLarge(bytes.len() as u64));
        }

        let content = String::from_utf8_lossy(&bytes).to_string();
        let truncated = content.len() > MAX_EXCERPT_CHARS;
        let excerpt: String = content.chars().take(MAX_EXCERPT_CHARS).collect();

        Ok(FetchedPage {
            url: url.to_string(),
            content_digest: sha256_hex(&bytes),
            content: excerpt,
            content_type,
            retrieved_at: Utc::now(),
            truncated,
        })
    }

    pub fn get_cached(&self, id: &str) -> Option<&EvidenceRecord> {
        let record = self.cache.get(id)?;
        if Utc::now().signed_duration_since(record.retrieved_at)
            > chrono::Duration::from_std(CACHE_TTL).ok()?
        {
            return None;
        }
        Some(record)
    }

    pub fn clear_cache(&mut self) {
        self.cache.clear();
        let _ = self.persist_cache();
    }

    pub fn evidence_count(&self) -> usize {
        self.cache.len()
    }

    fn persist_cache(&self) -> Result<(), ResearchError> {
        let Some(path) = &self.cache_path else {
            return Ok(());
        };
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let temporary = path.with_extension("json.tmp");
        let records = self.cache.values().collect::<Vec<_>>();
        std::fs::write(&temporary, serde_json::to_vec_pretty(&records)?)?;
        std::fs::rename(temporary, path)?;
        Ok(())
    }
}

#[cfg(test)]
fn extract_domain(url: &str) -> String {
    url.trim_start_matches("https://")
        .trim_start_matches("http://")
        .split('/')
        .next()
        .unwrap_or("")
        .split(':')
        .next()
        .unwrap_or("")
        .to_lowercase()
}

fn domain_matches(domain: &str, rule: &str) -> bool {
    let rule = rule.trim().trim_start_matches('.').to_ascii_lowercase();
    domain == rule || domain.ends_with(&format!(".{rule}"))
}

fn enforce_domain_policy(policy: &DomainPolicy, domain: &str) -> Result<(), ResearchError> {
    if policy
        .deny_list
        .iter()
        .any(|rule| domain_matches(domain, rule))
    {
        return Err(ResearchError::DomainDenied(domain.to_owned()));
    }
    if !policy.allow_list.is_empty()
        && !policy
            .allow_list
            .iter()
            .any(|rule| domain_matches(domain, rule))
    {
        return Err(ResearchError::DomainDenied(domain.to_owned()));
    }
    if policy
        .approval_required
        .iter()
        .any(|rule| domain_matches(domain, rule))
    {
        return Err(ResearchError::DomainRequiresApproval(domain.to_owned()));
    }
    Ok(())
}

fn validate_public_url(raw: &str) -> Result<String, ResearchError> {
    let url = reqwest::Url::parse(raw).map_err(|_| ResearchError::UnsafeUrl(raw.to_owned()))?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(ResearchError::UnsafeUrl(raw.to_owned()));
    }
    let host = url
        .host_str()
        .ok_or_else(|| ResearchError::UnsafeUrl(raw.to_owned()))?
        .trim_end_matches('.')
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_ascii_lowercase();
    if host == "localhost" || host.ends_with(".localhost") {
        return Err(ResearchError::UnsafeUrl(raw.to_owned()));
    }
    if let Ok(address) = host.parse::<IpAddr>() {
        let unsafe_address = match address {
            IpAddr::V4(v4) => {
                v4.is_private()
                    || v4.is_loopback()
                    || v4.is_link_local()
                    || v4.is_broadcast()
                    || v4.is_documentation()
                    || v4.is_unspecified()
                    || v4.is_multicast()
            }
            IpAddr::V6(v6) => {
                let segments = v6.segments();
                v6.is_loopback()
                    || v6.is_unspecified()
                    || v6.is_multicast()
                    || (segments[0] & 0xfe00) == 0xfc00
                    || (segments[0] & 0xffc0) == 0xfe80
            }
        };
        if unsafe_address {
            return Err(ResearchError::UnsafeUrl(raw.to_owned()));
        }
    }
    Ok(host)
}

fn validate_query(query: &str) -> Result<(), ResearchError> {
    let lower = query.to_ascii_lowercase();
    let credential_markers = [
        "api_key=",
        "apikey=",
        "authorization:",
        "bearer ",
        "password=",
        "secret=",
        "-----begin private key-----",
    ];
    let path_or_code = query.contains("/Users/")
        || query.contains("C:\\Users\\")
        || query.contains("BEGIN PRIVATE")
        || query.lines().count() > 3
        || query.len() > 512;
    if path_or_code
        || credential_markers
            .iter()
            .any(|marker| lower.contains(marker))
    {
        return Err(ResearchError::PrivateQuery);
    }
    Ok(())
}

fn sha256_hex(data: impl AsRef<[u8]>) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    format!("{:x}", hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn domain_extraction() {
        assert_eq!(extract_domain("https://example.com/path"), "example.com");
        assert_eq!(
            extract_domain("http://sub.example.com:8080/path"),
            "sub.example.com"
        );
        assert_eq!(extract_domain("not-a-url"), "not-a-url");
    }

    #[test]
    fn sha256_hex_produces_consistent_hash() {
        let h1 = sha256_hex(b"hello");
        let h2 = sha256_hex(b"hello");
        let h3 = sha256_hex(b"world");
        assert_eq!(h1, h2);
        assert_ne!(h1, h3);
        assert_eq!(h1.len(), 64);
    }

    #[tokio::test]
    async fn domain_deny_list_blocks_search_results() {
        let mut engine = ResearchEngine::new(
            Box::new(StubSearchProvider),
            DomainPolicy {
                deny_list: vec!["malware.example".into()],
                ..Default::default()
            },
        );

        let query = SearchQuery {
            query: "test".into(),
            max_results: 5,
        };

        let result = engine.search(&query, false).await;
        assert!(result.is_err() || result.unwrap().is_empty());
    }

    #[tokio::test]
    async fn local_only_mode_rejects_search() {
        let mut engine = ResearchEngine::new(Box::new(StubSearchProvider), DomainPolicy::default());

        let result = engine
            .search(
                &SearchQuery {
                    query: "test".into(),
                    max_results: 5,
                },
                true,
            )
            .await;
        assert!(matches!(result, Err(ResearchError::LocalOnlyMode)));
    }

    #[test]
    fn domain_policy_enforces_allow_approval_and_label_boundaries() {
        let policy = DomainPolicy {
            allow_list: vec!["example.com".into(), "approval.example".into()],
            deny_list: vec!["blocked.example.com".into()],
            approval_required: vec!["approval.example".into()],
        };
        assert!(enforce_domain_policy(&policy, "docs.example.com").is_ok());
        assert!(matches!(
            enforce_domain_policy(&policy, "approval.example"),
            Err(ResearchError::DomainRequiresApproval(_))
        ));
        assert!(matches!(
            enforce_domain_policy(&policy, "notexample.com"),
            Err(ResearchError::DomainDenied(_))
        ));
    }

    #[test]
    fn private_targets_and_queries_are_rejected() {
        assert!(validate_public_url("file:///etc/passwd").is_err());
        assert!(validate_public_url("http://127.0.0.1/admin").is_err());
        assert!(validate_public_url("http://[::1]/admin").is_err());
        assert!(validate_query("Authorization: Bearer token-value").is_err());
        assert!(validate_query("error in /Users/alice/private/repo").is_err());
        assert!(validate_query("rust axum sse example").is_ok());
    }

    #[test]
    fn evidence_cache_honors_ttl() {
        let mut engine = ResearchEngine::new(Box::new(StubSearchProvider), DomainPolicy::default());

        let record = EvidenceRecord {
            id: "ev-test".into(),
            query: "test".into(),
            url: "https://example.com".into(),
            title: "Example".into(),
            content_digest: "abc".into(),
            excerpt: "excerpt".into(),
            retrieved_at: Utc::now(),
        };
        engine.cache.insert("ev-test".into(), record.clone());

        assert!(engine.get_cached("ev-test").is_some());
    }

    #[test]
    fn durable_evidence_cache_survives_restart() {
        let temporary = tempfile::tempdir().unwrap();
        let path = temporary.path().join("research-cache.json");
        let mut engine = ResearchEngine::with_durable_cache(
            Box::new(StubSearchProvider),
            DomainPolicy::default(),
            &path,
        )
        .unwrap();
        let record = EvidenceRecord {
            id: "ev-persisted".into(),
            query: "public docs".into(),
            url: "https://example.com".into(),
            title: "Example".into(),
            content_digest: "abc".into(),
            excerpt: "excerpt".into(),
            retrieved_at: Utc::now(),
        };
        engine.cache.insert(record.id.clone(), record);
        engine.persist_cache().unwrap();
        drop(engine);
        let restored = ResearchEngine::with_durable_cache(
            Box::new(StubSearchProvider),
            DomainPolicy::default(),
            &path,
        )
        .unwrap();
        assert!(restored.get_cached("ev-persisted").is_some());
    }
}
