//! Governed web research with domain policy, content sanitization, and evidence caching.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;
use thiserror::Error;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
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

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceTrust {
    #[default]
    UntrustedExternal,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FetchedPage {
    pub url: String,
    pub content_digest: String,
    pub content: String,
    pub content_type: String,
    pub retrieved_at: DateTime<Utc>,
    pub truncated: bool,
    #[serde(default)]
    pub trust: EvidenceTrust,
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
    #[serde(default)]
    pub trust: EvidenceTrust,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct DomainPolicy {
    pub allow_list: Vec<String>,
    pub deny_list: Vec<String>,
    pub approval_required: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
pub enum PublicWebAction {
    Search { query: SearchQuery },
    Fetch { url: String },
}

/// Evidence supplied only after the daemon has verified and consumed a durable PawGate record.
///
/// The research execution boundary rechecks the exact operation, expiry, approved domains, and
/// in-process single-use state before any DNS resolution or provider call.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PublicWebAuthorization {
    authorization_id: String,
    exact_action: PublicWebAction,
    approved_domains: Vec<String>,
    expires_at: DateTime<Utc>,
}

impl PublicWebAuthorization {
    pub fn new(
        authorization_id: impl Into<String>,
        exact_action: PublicWebAction,
        approved_domains: Vec<String>,
        expires_at: DateTime<Utc>,
    ) -> Result<Self, ResearchError> {
        let authorization_id = authorization_id.into();
        if authorization_id.trim().is_empty() {
            return Err(ResearchError::InvalidAuthorization(
                "durable authorization id is empty".into(),
            ));
        }
        if expires_at <= Utc::now() {
            return Err(ResearchError::AuthorizationExpired);
        }
        match &exact_action {
            PublicWebAction::Search { query } => validate_search_query(query)?,
            PublicWebAction::Fetch { url } => {
                validate_public_url(url)?;
            }
        }
        let mut normalized_domains = Vec::new();
        for domain in approved_domains {
            let domain = domain.trim().trim_end_matches('.').to_ascii_lowercase();
            if domain.is_empty()
                || domain.contains('/')
                || domain.contains('*')
                || domain.parse::<IpAddr>().is_ok()
            {
                return Err(ResearchError::InvalidAuthorization(
                    "approved domains must be exact DNS names".into(),
                ));
            }
            if !normalized_domains.contains(&domain) {
                normalized_domains.push(domain);
            }
        }
        Ok(Self {
            authorization_id,
            exact_action,
            approved_domains: normalized_domains,
            expires_at,
        })
    }

    pub fn authorization_id(&self) -> &str {
        &self.authorization_id
    }

    fn verify(&self, action: &PublicWebAction) -> Result<(), ResearchError> {
        if self.expires_at <= Utc::now() {
            return Err(ResearchError::AuthorizationExpired);
        }
        if self.exact_action != *action {
            return Err(ResearchError::AuthorizationMismatch);
        }
        Ok(())
    }

    fn domain_is_approved(&self, domain: &str) -> bool {
        self.approved_domains
            .iter()
            .any(|approved| approved == domain)
    }
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
    #[error("public web endpoint returned HTTP {0}")]
    HttpStatus(u16),
    #[error("search disabled in local-only mode")]
    LocalOnlyMode,
    #[error("cache miss")]
    CacheMiss,
    #[error("search query contains private or credential-like data")]
    PrivateQuery,
    #[error("URL is not an allowed public HTTP(S) target: {0}")]
    UnsafeUrl(String),
    #[error("public DNS resolution failed closed: {0}")]
    DnsResolution(String),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("cache I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("public web research requires an exact durable authorization")]
    PublicWebPermissionRequired,
    #[error("public web research authorization has expired")]
    AuthorizationExpired,
    #[error("public web research authorization does not match this exact operation")]
    AuthorizationMismatch,
    #[error("public web research authorization was already consumed")]
    AuthorizationConsumed,
    #[error("invalid public web authorization: {0}")]
    InvalidAuthorization(String),
    #[error("invalid search request: {0}")]
    InvalidSearch(String),
}

const MAX_PAGE_BYTES: u64 = 2 * 1024 * 1024; // 2 MB
const MAX_SEARCH_RESPONSE_BYTES: u64 = 1024 * 1024; // 1 MB
const MAX_EXCERPT_CHARS: usize = 2000;
const MAX_TITLE_CHARS: usize = 256;
const MAX_URL_CHARS: usize = 2048;
const MAX_SEARCH_RESULTS: u32 = 20;
const CACHE_TTL: Duration = Duration::from_secs(3600); // 1 hour

pub struct ResearchEngine {
    policy: DomainPolicy,
    cache: HashMap<String, EvidenceRecord>,
    cache_path: Option<PathBuf>,
    search_provider: Box<dyn SearchProvider>,
    consumed_authorizations: Mutex<HashSet<String>>,
}

#[async_trait::async_trait]
pub trait SearchProvider: Send + Sync {
    async fn search(&self, query: &SearchQuery) -> Result<Vec<SearchResult>, ResearchError>;
}

pub struct WebSearchProvider {
    endpoint: String,
}

impl WebSearchProvider {
    pub fn new(endpoint: &str) -> Self {
        Self {
            endpoint: endpoint.trim_end_matches('/').to_string(),
        }
    }
}

#[async_trait::async_trait]
impl SearchProvider for WebSearchProvider {
    async fn search(&self, query: &SearchQuery) -> Result<Vec<SearchResult>, ResearchError> {
        let url = format!("{}/search", self.endpoint);
        let (domain, addresses) = resolve_public_url(&url).await?;
        let client = pinned_client(&domain, &addresses)?;
        let mut resp = client.post(&url).json(query).send().await?;
        if !resp.status().is_success() {
            return Err(ResearchError::HttpStatus(resp.status().as_u16()));
        }
        if let Some(length) = resp.content_length()
            && length > MAX_SEARCH_RESPONSE_BYTES
        {
            return Err(ResearchError::PageTooLarge(length));
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = resp.chunk().await? {
            if bytes.len().saturating_add(chunk.len()) > MAX_SEARCH_RESPONSE_BYTES as usize {
                return Err(ResearchError::PageTooLarge(
                    bytes.len().saturating_add(chunk.len()) as u64,
                ));
            }
            bytes.extend_from_slice(&chunk);
        }
        let results: Vec<SearchResult> = serde_json::from_slice(&bytes)?;
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
            policy,
            cache: HashMap::new(),
            cache_path: None,
            search_provider,
            consumed_authorizations: Mutex::new(HashSet::new()),
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
        validate_search_query(query)?;
        Err(ResearchError::PublicWebPermissionRequired)
    }

    pub async fn search_authorized(
        &mut self,
        query: &SearchQuery,
        local_only: bool,
        authorization: &PublicWebAuthorization,
    ) -> Result<Vec<EvidenceRecord>, ResearchError> {
        if local_only {
            return Err(ResearchError::LocalOnlyMode);
        }
        validate_search_query(query)?;
        let action = PublicWebAction::Search {
            query: query.clone(),
        };
        self.verify_and_consume(authorization, &action)?;

        let results = self.search_provider.search(query).await?;
        let mut evidence = Vec::new();

        for result in results.into_iter().take(query.max_results as usize) {
            let (domain, _) = resolve_public_url(&result.url).await?;
            enforce_domain_policy(&self.policy, &domain, Some(authorization))?;

            let record = EvidenceRecord {
                id: format!("ev-{}", sha256_hex(&result.url)),
                query: query.query.clone(),
                url: result.url.clone(),
                title: sanitize_external_text(&result.title, MAX_TITLE_CHARS),
                content_digest: sha256_hex(&result.snippet),
                excerpt: sanitize_external_text(&result.snippet, MAX_EXCERPT_CHARS),
                retrieved_at: Utc::now(),
                trust: EvidenceTrust::UntrustedExternal,
            };

            self.cache.insert(record.id.clone(), record.clone());
            evidence.push(record);
        }

        self.persist_cache()?;

        Ok(evidence)
    }

    pub async fn fetch_page(&self, url: &str) -> Result<FetchedPage, ResearchError> {
        validate_public_url(url)?;
        Err(ResearchError::PublicWebPermissionRequired)
    }

    pub async fn fetch_page_authorized(
        &self,
        url: &str,
        authorization: &PublicWebAuthorization,
    ) -> Result<FetchedPage, ResearchError> {
        validate_public_url(url)?;
        let action = PublicWebAction::Fetch {
            url: url.to_owned(),
        };
        self.verify_and_consume(authorization, &action)?;
        let (domain, addresses) = resolve_public_url(url).await?;
        enforce_domain_policy(&self.policy, &domain, Some(authorization))?;

        let client = pinned_client(&domain, &addresses)?;
        let mut resp = client.get(url).send().await?;
        if resp.status().is_redirection() {
            return Err(ResearchError::UnsafeUrl(format!(
                "redirects require a separately validated request: {url}"
            )));
        }
        if !resp.status().is_success() {
            return Err(ResearchError::HttpStatus(resp.status().as_u16()));
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

        let mut bytes = Vec::new();
        while let Some(chunk) = resp.chunk().await? {
            if bytes.len().saturating_add(chunk.len()) > MAX_PAGE_BYTES as usize {
                return Err(ResearchError::PageTooLarge(
                    bytes.len().saturating_add(chunk.len()) as u64,
                ));
            }
            bytes.extend_from_slice(&chunk);
        }

        let content = String::from_utf8_lossy(&bytes);
        let truncated = content.chars().count() > MAX_EXCERPT_CHARS;
        let excerpt = sanitize_external_text(&content, MAX_EXCERPT_CHARS);

        Ok(FetchedPage {
            url: url.to_string(),
            content_digest: sha256_hex(&bytes),
            content: excerpt,
            content_type,
            retrieved_at: Utc::now(),
            truncated,
            trust: EvidenceTrust::UntrustedExternal,
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

    fn verify_and_consume(
        &self,
        authorization: &PublicWebAuthorization,
        action: &PublicWebAction,
    ) -> Result<(), ResearchError> {
        authorization.verify(action)?;
        let mut consumed = self.consumed_authorizations.lock().map_err(|_| {
            ResearchError::InvalidAuthorization("authorization state lock is poisoned".into())
        })?;
        if !consumed.insert(authorization.authorization_id.clone()) {
            return Err(ResearchError::AuthorizationConsumed);
        }
        Ok(())
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

fn enforce_domain_policy(
    policy: &DomainPolicy,
    domain: &str,
    authorization: Option<&PublicWebAuthorization>,
) -> Result<(), ResearchError> {
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
        && !authorization.is_some_and(|grant| grant.domain_is_approved(domain))
    {
        return Err(ResearchError::DomainRequiresApproval(domain.to_owned()));
    }
    Ok(())
}

fn validate_public_url(raw: &str) -> Result<String, ResearchError> {
    if raw.chars().count() > MAX_URL_CHARS {
        return Err(ResearchError::UnsafeUrl(
            "URL exceeds the public research length limit".into(),
        ));
    }
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
    if let Ok(address) = host.parse::<IpAddr>()
        && !is_public_address(address)
    {
        return Err(ResearchError::UnsafeUrl(raw.to_owned()));
    }
    Ok(host)
}

fn is_public_address(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(v4) => {
            let octets = v4.octets();
            !(octets[0] == 0
                || v4.is_private()
                || v4.is_loopback()
                || v4.is_link_local()
                || v4.is_broadcast()
                || v4.is_documentation()
                || v4.is_unspecified()
                || v4.is_multicast()
                || (octets[0] == 100 && (64..=127).contains(&octets[1]))
                || (octets[0] == 192 && octets[1] == 0 && octets[2] == 0)
                || (octets[0] == 198 && (octets[1] == 18 || octets[1] == 19))
                || octets[0] >= 240)
        }
        IpAddr::V6(v6) => {
            let segments = v6.segments();
            v6.to_ipv4_mapped()
                .map(|mapped| is_public_address(IpAddr::V4(mapped)))
                .unwrap_or_else(|| {
                    !(v6.is_loopback()
                        || v6.is_unspecified()
                        || v6.is_multicast()
                        || (segments[0] & 0xfe00) == 0xfc00
                        || (segments[0] & 0xffc0) == 0xfe80
                        || (segments[0] & 0xffc0) == 0xfec0
                        || (segments[0] == 0x2001 && segments[1] == 0x0db8))
                })
        }
    }
}

fn validate_resolved_addresses(
    raw: &str,
    addresses: Vec<SocketAddr>,
) -> Result<Vec<SocketAddr>, ResearchError> {
    if addresses.is_empty() || addresses.iter().any(|entry| !is_public_address(entry.ip())) {
        return Err(ResearchError::UnsafeUrl(raw.to_owned()));
    }
    Ok(addresses)
}

async fn resolve_public_url(raw: &str) -> Result<(String, Vec<SocketAddr>), ResearchError> {
    let domain = validate_public_url(raw)?;
    let url = reqwest::Url::parse(raw).map_err(|_| ResearchError::UnsafeUrl(raw.to_owned()))?;
    let port = url.port_or_known_default().ok_or_else(|| {
        ResearchError::DnsResolution("HTTP(S) URL does not have a resolvable port".into())
    })?;
    let addresses = tokio::net::lookup_host((domain.as_str(), port))
        .await
        .map_err(|error| ResearchError::DnsResolution(error.to_string()))?
        .collect::<Vec<_>>();
    Ok((domain, validate_resolved_addresses(raw, addresses)?))
}

fn pinned_client(domain: &str, addresses: &[SocketAddr]) -> Result<reqwest::Client, ResearchError> {
    Ok(reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .connect_timeout(Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::none())
        .resolve_to_addrs(domain, addresses)
        .build()?)
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

fn validate_search_query(query: &SearchQuery) -> Result<(), ResearchError> {
    validate_query(&query.query)?;
    if query.max_results == 0 || query.max_results > MAX_SEARCH_RESULTS {
        return Err(ResearchError::InvalidSearch(format!(
            "max_results must be between 1 and {MAX_SEARCH_RESULTS}"
        )));
    }
    Ok(())
}

fn sha256_hex(data: impl AsRef<[u8]>) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    format!("{:x}", hasher.finalize())
}

fn sanitize_external_text(input: &str, maximum_chars: usize) -> String {
    input
        .chars()
        .filter(|character| *character == '\n' || *character == '\t' || !character.is_control())
        .take(maximum_chars)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct CountingSearchProvider {
        calls: Arc<AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl SearchProvider for CountingSearchProvider {
        async fn search(&self, _query: &SearchQuery) -> Result<Vec<SearchResult>, ResearchError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(Vec::new())
        }
    }

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

    #[test]
    fn external_text_is_bounded_and_control_characters_are_removed() {
        let sanitized = sanitize_external_text("safe\u{1b}[31m\ntext", 9);
        assert_eq!(sanitized, "safe[31m\n");
        assert!(sanitized.chars().count() <= 9);
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
        assert!(enforce_domain_policy(&policy, "docs.example.com", None).is_ok());
        assert!(matches!(
            enforce_domain_policy(&policy, "approval.example", None),
            Err(ResearchError::DomainRequiresApproval(_))
        ));
        assert!(matches!(
            enforce_domain_policy(&policy, "notexample.com", None),
            Err(ResearchError::DomainDenied(_))
        ));
        let authorization = PublicWebAuthorization::new(
            "approved-domain-action",
            PublicWebAction::Fetch {
                url: "https://approval.example/source".into(),
            },
            vec!["approval.example".into()],
            Utc::now() + chrono::Duration::minutes(5),
        )
        .unwrap();
        assert!(enforce_domain_policy(&policy, "approval.example", Some(&authorization)).is_ok());
    }

    #[tokio::test]
    async fn public_web_search_requires_exact_single_use_authorization_before_provider_call() {
        let calls = Arc::new(AtomicUsize::new(0));
        let mut engine = ResearchEngine::new(
            Box::new(CountingSearchProvider {
                calls: calls.clone(),
            }),
            DomainPolicy::default(),
        );
        let query = SearchQuery {
            query: "github pull request MCP".into(),
            max_results: 5,
        };

        assert!(matches!(
            engine.search(&query, false).await,
            Err(ResearchError::PublicWebPermissionRequired)
        ));
        assert_eq!(calls.load(Ordering::SeqCst), 0);

        let authorization = PublicWebAuthorization::new(
            "durable-search-action",
            PublicWebAction::Search {
                query: query.clone(),
            },
            Vec::new(),
            Utc::now() + chrono::Duration::minutes(5),
        )
        .unwrap();
        assert!(
            engine
                .search_authorized(&query, false, &authorization)
                .await
                .unwrap()
                .is_empty()
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(matches!(
            engine
                .search_authorized(&query, false, &authorization)
                .await,
            Err(ResearchError::AuthorizationConsumed)
        ));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn changed_or_oversized_search_is_denied_before_external_call() {
        let calls = Arc::new(AtomicUsize::new(0));
        let mut engine = ResearchEngine::new(
            Box::new(CountingSearchProvider {
                calls: calls.clone(),
            }),
            DomainPolicy::default(),
        );
        let query = SearchQuery {
            query: "safe public capability".into(),
            max_results: 5,
        };
        let authorization = PublicWebAuthorization::new(
            "exact-query-action",
            PublicWebAction::Search {
                query: query.clone(),
            },
            Vec::new(),
            Utc::now() + chrono::Duration::minutes(5),
        )
        .unwrap();
        let changed = SearchQuery {
            query: "different capability".into(),
            max_results: 5,
        };
        assert!(matches!(
            engine
                .search_authorized(&changed, false, &authorization)
                .await,
            Err(ResearchError::AuthorizationMismatch)
        ));
        assert_eq!(calls.load(Ordering::SeqCst), 0);

        let oversized = SearchQuery {
            query: "safe public capability".into(),
            max_results: MAX_SEARCH_RESULTS + 1,
        };
        assert!(matches!(
            engine.search(&oversized, false).await,
            Err(ResearchError::InvalidSearch(_))
        ));
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn fetch_without_public_permission_does_not_resolve_or_request() {
        let engine = ResearchEngine::new(Box::new(StubSearchProvider), DomainPolicy::default());
        assert!(matches!(
            engine.fetch_page("https://example.com/source").await,
            Err(ResearchError::PublicWebPermissionRequired)
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
    fn dns_answers_fail_closed_when_any_address_is_not_public() {
        let public: SocketAddr = "93.184.216.34:443".parse().unwrap();
        let loopback: SocketAddr = "127.0.0.1:443".parse().unwrap();
        let mapped_loopback: SocketAddr = "[::ffff:127.0.0.1]:443".parse().unwrap();
        let carrier_nat: SocketAddr = "100.64.0.1:443".parse().unwrap();
        assert!(validate_resolved_addresses("https://example.com", vec![public]).is_ok());
        assert!(validate_resolved_addresses("https://example.com", Vec::new()).is_err());
        assert!(
            validate_resolved_addresses("https://rebind.example", vec![public, loopback]).is_err()
        );
        assert!(
            validate_resolved_addresses("https://mapped.example", vec![mapped_loopback]).is_err()
        );
        assert!(validate_resolved_addresses("https://cgnat.example", vec![carrier_nat]).is_err());
    }

    #[tokio::test]
    async fn dns_resolution_rejects_localhost_before_http() {
        assert!(matches!(
            resolve_public_url("http://localhost:8080/private").await,
            Err(ResearchError::UnsafeUrl(_))
        ));
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
            trust: EvidenceTrust::UntrustedExternal,
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
            trust: EvidenceTrust::UntrustedExternal,
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
