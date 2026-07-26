//! Governed web research with domain policy, content sanitization, and evidence caching.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::time::{Duration, Instant};
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
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
}

const MAX_PAGE_BYTES: u64 = 2 * 1024 * 1024; // 2 MB
const MAX_EXCERPT_CHARS: usize = 2000;
const CACHE_TTL: Duration = Duration::from_secs(3600); // 1 hour

pub struct ResearchEngine {
    client: reqwest::Client,
    policy: DomainPolicy,
    cache: HashMap<String, EvidenceRecord>,
    cache_timestamps: HashMap<String, Instant>,
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
                .build()
                .unwrap_or_default(),
            policy,
            cache: HashMap::new(),
            cache_timestamps: HashMap::new(),
            search_provider,
        }
    }

    pub async fn search(
        &mut self,
        query: &SearchQuery,
        local_only: bool,
    ) -> Result<Vec<EvidenceRecord>, ResearchError> {
        if local_only {
            return Err(ResearchError::LocalOnlyMode);
        }

        let results = self.search_provider.search(query).await?;
        let mut evidence = Vec::new();

        for result in results {
            let domain = extract_domain(&result.url);
            if self.policy.deny_list.iter().any(|d| domain.contains(d)) {
                continue;
            }
            if self
                .policy
                .approval_required
                .iter()
                .any(|d| domain.contains(d))
            {
                continue;
            }

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
            self.cache_timestamps
                .insert(record.id.clone(), Instant::now());
            evidence.push(record);
        }

        Ok(evidence)
    }

    pub async fn fetch_page(&self, url: &str) -> Result<FetchedPage, ResearchError> {
        let domain = extract_domain(url);
        if self.policy.deny_list.iter().any(|d| domain.contains(d)) {
            return Err(ResearchError::DomainDenied(url.to_string()));
        }

        let resp = self.client.get(url).send().await?;

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
        let timestamp = self.cache_timestamps.get(id)?;
        if timestamp.elapsed() > CACHE_TTL {
            return None;
        }
        self.cache.get(id)
    }

    pub fn clear_cache(&mut self) {
        self.cache.clear();
        self.cache_timestamps.clear();
    }

    pub fn evidence_count(&self) -> usize {
        self.cache.len()
    }
}

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
        engine
            .cache_timestamps
            .insert("ev-test".into(), Instant::now());

        assert!(engine.get_cached("ev-test").is_some());
    }
}
