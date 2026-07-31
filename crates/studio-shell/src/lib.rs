//! Authenticated local web shell for PurrCode Studio.
//!
//! The browser never receives the daemon bearer token.  It authenticates to
//! this loopback-only shell with an HttpOnly cookie and the shell forwards
//! `/api/v1/*` requests to the daemon with the bearer token attached.  The
//! daemon remains the only execution path.

use anyhow::{bail, Context, Result};
use axum::body::{Body, Bytes};
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path as AxumPath, State};
use axum::http::header::{
    ACCEPT, AUTHORIZATION, CACHE_CONTROL, CONTENT_SECURITY_POLICY, CONTENT_TYPE, COOKIE, ORIGIN,
    REFERRER_POLICY, SET_COOKIE, X_CONTENT_TYPE_OPTIONS,
};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{any, get};
use axum::Router;
use chrono::{DateTime, Utc};
use futures::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::Mutex;
use url::Url;
use uuid::Uuid;

const INDEX_HTML: &str = include_str!("../assets/index.html");
const APP_CSS: &str = include_str!("../assets/app.css");
const APP_JS: &str = include_str!("../assets/app.js");
const TERM_JS: &str = include_str!("../assets/term.js");
const SESSION_COOKIE: &str = "purrcode_studio";

/// Configuration for one Studio shell process.
#[derive(Clone, Debug)]
pub struct StudioConfig {
    pub bind: SocketAddr,
    pub daemon_url: Url,
    pub daemon_token: String,
    pub repository: PathBuf,
    pub expected_daemon_api_version: u32,
    pub expected_studio_api_version: u32,
}

/// Safe startup information.  `bootstrap_url` contains a one-time secret and
/// must only be handed to the local browser process or printed to its owner.
#[derive(Clone, Debug, Serialize)]
pub struct StudioReport {
    pub bind: SocketAddr,
    pub bootstrap_url: Url,
    pub started_at: DateTime<Utc>,
}

#[derive(Clone)]
struct StudioState {
    client: reqwest::Client,
    daemon_url: Url,
    daemon_token: Arc<str>,
    repository: PathBuf,
    origin: Arc<str>,
    session_secret: Arc<str>,
    session_digest: [u8; 32],
    bootstrap_digest: Arc<Mutex<Option<[u8; 32]>>>,
    daemon_api_version: u32,
    studio_api_version: u32,
}

#[derive(Debug, Deserialize)]
struct DaemonHealth {
    product: String,
    daemon_api_version: u32,
    studio_api_version: u32,
}

#[derive(Serialize)]
struct BrowserConfig<'a> {
    product: &'static str,
    repository: &'a std::path::Path,
    daemon_api_version: u32,
    studio_api_version: u32,
}

/// Validate the daemon, bind the loopback Studio server, and return its
/// future.  The caller owns the lifetime and may run it in the foreground or
/// spawn it under a supervised process.
pub async fn bind_and_report(
    config: StudioConfig,
) -> Result<(
    StudioReport,
    impl std::future::Future<Output = std::io::Result<()>>,
)> {
    if !config.bind.ip().is_loopback() {
        bail!(
            "Studio shell refuses non-loopback bind {}; expose the daemon through an authenticated TLS reverse proxy instead",
            config.bind.ip()
        );
    }
    if config.daemon_token.trim().len() < 32 {
        bail!("daemon token is missing or invalid");
    }
    validate_daemon_url(&config.daemon_url)?;

    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(3))
        .timeout(std::time::Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    verify_compatibility(&client, &config).await?;

    let listener = TcpListener::bind(config.bind)
        .await
        .with_context(|| format!("bind Studio shell at {}", config.bind))?;
    let bind = listener.local_addr()?;
    let origin = format!("http://{bind}");
    let bootstrap_secret = random_secret();
    let session_secret = random_secret();
    let state = Arc::new(StudioState {
        client,
        daemon_url: normalize_base_url(config.daemon_url),
        daemon_token: Arc::from(config.daemon_token.trim()),
        repository: config.repository,
        origin: Arc::from(origin.as_str()),
        session_digest: digest(&session_secret),
        session_secret: Arc::from(session_secret),
        bootstrap_digest: Arc::new(Mutex::new(Some(digest(&bootstrap_secret)))),
        daemon_api_version: config.expected_daemon_api_version,
        studio_api_version: config.expected_studio_api_version,
    });
    let app = router(state);
    let bootstrap_url = Url::parse(&format!("{origin}/bootstrap/{bootstrap_secret}"))?;
    let report = StudioReport {
        bind,
        bootstrap_url,
        started_at: Utc::now(),
    };
    let server = async move { axum::serve(listener, app).await };
    Ok((report, server))
}

fn router(state: Arc<StudioState>) -> Router {
    Router::new()
        .route("/bootstrap/{token}", get(bootstrap))
        .route("/", get(index))
        .route("/app.css", get(styles))
        .route("/app.js", get(script))
        .route("/term.js", get(terminal_script))
        .route("/studio/config", get(browser_config))
        .route("/studio/terminals/{id}/stream", get(terminal_socket))
        .route("/api/{*path}", any(proxy))
        .fallback(not_found)
        .with_state(state)
}

async fn terminal_socket(
    State(state): State<Arc<StudioState>>,
    AxumPath(id): AxumPath<String>,
    headers: HeaderMap,
    upgrade: WebSocketUpgrade,
) -> Response {
    if !authorized(&state, &headers) {
        return secured_text(StatusCode::UNAUTHORIZED, "Studio authentication required");
    }
    if !same_origin(&state, &headers) {
        return secured_text(StatusCode::FORBIDDEN, "cross-origin request rejected");
    }
    if Uuid::parse_str(&id).is_err() {
        return secured_text(StatusCode::BAD_REQUEST, "terminal ID is not a UUID");
    }
    upgrade
        .on_upgrade(move |socket| run_terminal_socket(socket, state, id))
        .into_response()
}

/// Stream a terminal to one Studio client.
///
/// Each tick forwards only the bytes produced since the last one (PRD §24.7).
/// The previous implementation re-fetched and re-sent the entire transcript
/// every 80ms, which grows with the session and makes the client redraw output
/// it already has; `since` makes an idle terminal cost an empty frame.
async fn run_terminal_socket(socket: WebSocket, state: Arc<StudioState>, id: String) {
    let (mut sender, mut receiver) = socket.split();
    let mut interval = tokio::time::interval(std::time::Duration::from_millis(40));
    let mut since: u64 = 0;
    let endpoint = match state.daemon_url.join(&format!("v1/terminals/{id}/output")) {
        Ok(endpoint) => endpoint,
        Err(_) => return,
    };
    loop {
        tokio::select! {
            _ = interval.tick() => {
                let response = state.client.get(endpoint.clone())
                    .query(&[("since", since)])
                    .bearer_auth(state.daemon_token.as_ref()).send().await;
                let Ok(response) = response else { continue };
                let Ok(body) = response.json::<serde_json::Value>().await else { continue };
                let next = body["chunk"]["next_offset"].as_u64().unwrap_or(since);
                let empty = body["chunk"]["bytes"].as_array().is_none_or(|b| b.is_empty());
                since = next;
                // An idle terminal still needs the occasional frame so the
                // client learns when the process exits, but not 25 a second.
                if empty && body["alive"].as_bool().unwrap_or(true) { continue; }
                let Ok(text) = serde_json::to_string(&body) else { continue };
                if sender.send(Message::Text(text.into())).await.is_err() { break; }
            }
            incoming = receiver.next() => {
                let Some(Ok(message)) = incoming else { break; };
                match message {
                    Message::Text(body) => {
                        let Ok(input) = serde_json::from_str::<TerminalSocketInput>(&body) else { continue; };
                        let Ok(endpoint) = state.daemon_url.join(&format!("v1/terminals/{id}/input")) else { continue; };
                        let _ = state.client.post(endpoint)
                            .bearer_auth(state.daemon_token.as_ref())
                            .json(&input).send().await;
                    }
                    Message::Close(_) => break,
                    Message::Ping(payload) => {
                        if sender.send(Message::Pong(payload)).await.is_err() { break; }
                    }
                    _ => {}
                }
            }
        }
    }
}

#[derive(Serialize, Deserialize)]
struct TerminalSocketInput {
    generation: u64,
    input: String,
}

async fn verify_compatibility(client: &reqwest::Client, config: &StudioConfig) -> Result<()> {
    let url = normalize_base_url(config.daemon_url.clone()).join("v1/health")?;
    let response = client
        .get(url)
        .bearer_auth(config.daemon_token.trim())
        .send()
        .await
        .context("connect to authenticated PurrCode daemon")?;
    if response.status() != reqwest::StatusCode::OK {
        bail!("daemon health check returned HTTP {}", response.status());
    }
    let health: DaemonHealth = response
        .json()
        .await
        .context("decode daemon compatibility response")?;
    if health.product != "purrcode" {
        bail!("connected service is not PurrCode");
    }
    if health.daemon_api_version != config.expected_daemon_api_version {
        bail!(
            "daemon API version {} is incompatible with Studio version {}",
            health.daemon_api_version,
            config.expected_daemon_api_version
        );
    }
    if health.studio_api_version != config.expected_studio_api_version {
        bail!(
            "Studio API version {} is incompatible with client version {}",
            health.studio_api_version,
            config.expected_studio_api_version
        );
    }
    Ok(())
}

fn validate_daemon_url(url: &Url) -> Result<()> {
    if !matches!(url.scheme(), "http" | "https") {
        bail!("daemon URL must use http or https");
    }
    if !url.username().is_empty() || url.password().is_some() {
        bail!("daemon URL must not contain credentials");
    }
    if url.query().is_some() || url.fragment().is_some() {
        bail!("daemon URL must not contain a query or fragment");
    }
    Ok(())
}

fn normalize_base_url(mut url: Url) -> Url {
    if !url.path().ends_with('/') {
        let path = format!("{}/", url.path());
        url.set_path(&path);
    }
    url
}

async fn bootstrap(
    State(state): State<Arc<StudioState>>,
    AxumPath(token): AxumPath<String>,
) -> Response {
    let supplied = digest(&token);
    let mut expected = state.bootstrap_digest.lock().await;
    let valid = expected
        .as_ref()
        .is_some_and(|value| constant_time_eq(value, &supplied));
    if !valid {
        return secured_text(StatusCode::UNAUTHORIZED, "invalid or expired Studio link");
    }
    *expected = None;
    let cookie = format!(
        "{SESSION_COOKIE}={}; HttpOnly; SameSite=Strict; Path=/",
        state.session_secret
    );
    let mut response = Redirect::to("/").into_response();
    response.headers_mut().insert(
        SET_COOKIE,
        HeaderValue::from_str(&cookie).expect("generated cookie is valid ASCII"),
    );
    apply_security_headers(response.headers_mut());
    response
}

/// Shown to a person who reaches Studio without a session.
///
/// The bootstrap link is single-use by design, so this page is reached by
/// ordinary means: bookmarking the bare address, opening it in a second
/// browser, clearing cookies, or restarting the shell. Answering that with
/// `Studio authentication required` and nothing else is a dead end — the
/// reader is left unable to tell a malfunction from a security property, and is
/// given no way forward.
///
/// Unstyled on purpose: the CSP is `default-src 'self'` with no inline styles,
/// and `/app.css` needs the very session this reader does not have. Semantic
/// HTML renders readably without it.
const UNAUTHORIZED_HTML: &str = r#"<!doctype html>
<html lang="en">
<head><meta charset="utf-8"><title>PurrCode Studio - session required</title></head>
<body>
<h1>This browser has no Studio session</h1>
<p>Studio opens through a one-time link that is exchanged for a session cookie
and then immediately invalidated. That link has already been used, or this
browser never had it.</p>
<p>This is how Studio is meant to work, not a fault: the link is single-use so
that a copied URL cannot be replayed by anyone else.</p>
<h2>To get back in</h2>
<p>Run this in your repository and open the link it prints:</p>
<pre>purrcode studio</pre>
<p>Your session, conversation and terminals are held by the daemon, not by this
browser, so nothing is lost by opening a new link.</p>
</body>
</html>"#;

async fn index(State(state): State<Arc<StudioState>>, headers: HeaderMap) -> Response {
    if !authorized(&state, &headers) {
        return secured_asset(
            StatusCode::UNAUTHORIZED,
            "text/html; charset=utf-8",
            UNAUTHORIZED_HTML,
        );
    }
    secured_asset(StatusCode::OK, "text/html; charset=utf-8", INDEX_HTML)
}

async fn styles(State(state): State<Arc<StudioState>>, headers: HeaderMap) -> Response {
    if !authorized(&state, &headers) {
        return secured_text(StatusCode::UNAUTHORIZED, "Studio authentication required");
    }
    secured_asset(StatusCode::OK, "text/css; charset=utf-8", APP_CSS)
}

async fn script(State(state): State<Arc<StudioState>>, headers: HeaderMap) -> Response {
    if !authorized(&state, &headers) {
        return secured_text(StatusCode::UNAUTHORIZED, "Studio authentication required");
    }
    secured_asset(
        StatusCode::OK,
        "application/javascript; charset=utf-8",
        APP_JS,
    )
}

async fn terminal_script(State(state): State<Arc<StudioState>>, headers: HeaderMap) -> Response {
    if !authorized(&state, &headers) {
        return secured_text(StatusCode::UNAUTHORIZED, "Studio authentication required");
    }
    secured_asset(
        StatusCode::OK,
        "application/javascript; charset=utf-8",
        TERM_JS,
    )
}

async fn browser_config(State(state): State<Arc<StudioState>>, headers: HeaderMap) -> Response {
    if !authorized(&state, &headers) {
        return secured_text(StatusCode::UNAUTHORIZED, "Studio authentication required");
    }
    let body = serde_json::to_vec(&BrowserConfig {
        product: "purrcode",
        repository: &state.repository,
        daemon_api_version: state.daemon_api_version,
        studio_api_version: state.studio_api_version,
    })
    .expect("browser configuration is serializable");
    secured_bytes(StatusCode::OK, "application/json", body)
}

async fn proxy(
    State(state): State<Arc<StudioState>>,
    AxumPath(path): AxumPath<String>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if !authorized(&state, &headers) {
        return secured_text(StatusCode::UNAUTHORIZED, "Studio authentication required");
    }
    if !valid_proxy_path(&path) {
        return secured_text(StatusCode::NOT_FOUND, "unknown daemon API path");
    }
    if is_mutating(&method) && !same_origin(&state, &headers) {
        return secured_text(StatusCode::FORBIDDEN, "cross-origin request rejected");
    }

    let mut target = match state.daemon_url.join(&path) {
        Ok(target) => target,
        Err(_) => return secured_text(StatusCode::BAD_REQUEST, "invalid daemon API path"),
    };
    target.set_query(uri.query());
    let mut request = state
        .client
        .request(method, target)
        .header(AUTHORIZATION, format!("Bearer {}", state.daemon_token));
    for name in [CONTENT_TYPE, ACCEPT] {
        if let Some(value) = headers.get(&name) {
            request = request.header(name, value);
        }
    }
    if headers
        .get(ACCEPT)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.contains("text/event-stream"))
    {
        request = request.timeout(std::time::Duration::from_secs(60 * 60));
    }
    if !body.is_empty() {
        request = request.body(body);
    }
    let upstream = match request.send().await {
        Ok(response) => response,
        Err(_) => {
            return secured_text(
                StatusCode::BAD_GATEWAY,
                "PurrCode daemon is unavailable; the run remains durable",
            )
        }
    };
    let status = upstream.status();
    let content_type = upstream.headers().get(CONTENT_TYPE).cloned();
    let cache_control = upstream.headers().get(CACHE_CONTROL).cloned();
    let mut response = Response::new(Body::from_stream(upstream.bytes_stream()));
    *response.status_mut() = status;
    if let Some(value) = content_type {
        response.headers_mut().insert(CONTENT_TYPE, value);
    }
    if let Some(value) = cache_control {
        response.headers_mut().insert(CACHE_CONTROL, value);
    } else {
        response
            .headers_mut()
            .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    }
    response
}

async fn not_found() -> Response {
    secured_text(StatusCode::NOT_FOUND, "not found")
}

fn authorized(state: &StudioState, headers: &HeaderMap) -> bool {
    let supplied = headers
        .get(COOKIE)
        .and_then(|value| value.to_str().ok())
        .and_then(|cookies| {
            cookies.split(';').find_map(|cookie| {
                let (name, value) = cookie.trim().split_once('=')?;
                (name == SESSION_COOKIE).then_some(value)
            })
        });
    supplied.is_some_and(|value| {
        let supplied = digest(value);
        constant_time_eq(&state.session_digest, &supplied)
    })
}

fn same_origin(state: &StudioState, headers: &HeaderMap) -> bool {
    headers
        .get(ORIGIN)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|origin| origin == state.origin.as_ref())
}

fn is_mutating(method: &Method) -> bool {
    !matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS)
}

fn valid_proxy_path(path: &str) -> bool {
    (path == "v1" || path.starts_with("v1/"))
        && path.split('/').all(|segment| {
            !segment.is_empty()
                && segment != "."
                && segment != ".."
                && segment
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        })
}

fn random_secret() -> String {
    format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple())
}

fn digest(value: &str) -> [u8; 32] {
    *blake3::hash(value.as_bytes()).as_bytes()
}

fn constant_time_eq(left: &[u8; 32], right: &[u8; 32]) -> bool {
    left.iter()
        .zip(right)
        .fold(0_u8, |difference, (left, right)| {
            difference | (left ^ right)
        })
        == 0
}

fn secured_asset(status: StatusCode, content_type: &'static str, body: &'static str) -> Response {
    secured_bytes(status, content_type, body.as_bytes().to_vec())
}

fn secured_text(status: StatusCode, body: &'static str) -> Response {
    secured_bytes(
        status,
        "text/plain; charset=utf-8",
        body.as_bytes().to_vec(),
    )
}

fn secured_bytes(status: StatusCode, content_type: &'static str, body: Vec<u8>) -> Response {
    let mut response = (status, body).into_response();
    response
        .headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static(content_type));
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    apply_security_headers(response.headers_mut());
    response
}

fn apply_security_headers(headers: &mut HeaderMap) {
    headers.insert(
        CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(
            "default-src 'self'; connect-src 'self'; img-src 'self' data:; object-src 'none'; base-uri 'none'; frame-ancestors 'none'; form-action 'self'",
        ),
    );
    headers.insert(X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    headers.insert(REFERRER_POLICY, HeaderValue::from_static("no-referrer"));
    headers.insert(
        "permissions-policy",
        HeaderValue::from_static("camera=(), microphone=(), geolocation=()"),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::extract::State as TestState;
    use axum::http::header::AUTHORIZATION as TEST_AUTHORIZATION;
    use axum::response::sse::{Event as TestEvent, Sse as TestSse};
    use axum::routing::get as test_get;
    use axum::{Json, Router as TestRouter};
    use serde_json::json;

    async fn fake_health(headers: HeaderMap) -> Response {
        if headers
            .get(TEST_AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            != Some("Bearer daemon-secret-that-is-long-enough")
        {
            return StatusCode::UNAUTHORIZED.into_response();
        }
        Json(json!({
            "product": "purrcode",
            "daemon_api_version": 2,
            "studio_api_version": 1,
            "status": "ok"
        }))
        .into_response()
    }

    async fn fake_sessions(
        TestState(counter): TestState<Arc<Mutex<u32>>>,
        headers: HeaderMap,
    ) -> Response {
        if headers
            .get(TEST_AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            != Some("Bearer daemon-secret-that-is-long-enough")
        {
            return StatusCode::UNAUTHORIZED.into_response();
        }
        *counter.lock().await += 1;
        Json(json!([])).into_response()
    }

    async fn fake_start(headers: HeaderMap) -> Response {
        if headers
            .get(TEST_AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            != Some("Bearer daemon-secret-that-is-long-enough")
        {
            return StatusCode::UNAUTHORIZED.into_response();
        }
        (StatusCode::ACCEPTED, Json(json!({"id": "run-1"}))).into_response()
    }

    async fn fake_stream(headers: HeaderMap) -> Response {
        if headers
            .get(TEST_AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            != Some("Bearer daemon-secret-that-is-long-enough")
        {
            return StatusCode::UNAUTHORIZED.into_response();
        }
        TestSse::new(futures::stream::iter([Ok::<_, std::convert::Infallible>(
            TestEvent::default()
                .event("durable_audit")
                .data("{\"kind\":\"durable_audit\"}"),
        )]))
        .into_response()
    }

    async fn start_pair() -> (StudioReport, tokio::task::JoinHandle<()>, Arc<Mutex<u32>>) {
        let counter = Arc::new(Mutex::new(0));
        let daemon = TestRouter::new()
            .route("/v1/health", test_get(fake_health))
            .route("/v1/sessions", test_get(fake_sessions).post(fake_start))
            .route("/v1/sessions/run-1/events/stream", test_get(fake_stream))
            .with_state(counter.clone());
        let daemon_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let daemon_bind = daemon_listener.local_addr().unwrap();
        let daemon_task = tokio::spawn(async move {
            axum::serve(daemon_listener, daemon).await.unwrap();
        });
        let (report, server) = bind_and_report(StudioConfig {
            bind: "127.0.0.1:0".parse().unwrap(),
            daemon_url: Url::parse(&format!("http://{daemon_bind}/")).unwrap(),
            daemon_token: "daemon-secret-that-is-long-enough".into(),
            repository: PathBuf::from("/example/repository"),
            expected_daemon_api_version: 2,
            expected_studio_api_version: 1,
        })
        .await
        .unwrap();
        let studio_task = tokio::spawn(async move {
            server.await.unwrap();
        });
        let joined = tokio::spawn(async move {
            let _ = tokio::join!(daemon_task, studio_task);
        });
        (report, joined, counter)
    }

    async fn authenticate(client: &reqwest::Client, report: &StudioReport) -> String {
        let response = client
            .get(report.bootstrap_url.clone())
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::SEE_OTHER);
        response
            .headers()
            .get(reqwest::header::SET_COOKIE)
            .unwrap()
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_owned()
    }

    #[tokio::test]
    async fn shell_requires_cookie_and_bootstrap_is_single_use() {
        let (report, task, _) = start_pair().await;
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap();
        let root = client
            .get(format!("http://{}/", report.bind))
            .send()
            .await
            .unwrap();
        assert_eq!(root.status(), reqwest::StatusCode::UNAUTHORIZED);

        let cookie = authenticate(&client, &report).await;
        let replay = client
            .get(report.bootstrap_url.clone())
            .send()
            .await
            .unwrap();
        assert_eq!(replay.status(), reqwest::StatusCode::UNAUTHORIZED);
        let root = client
            .get(format!("http://{}/", report.bind))
            .header(reqwest::header::COOKIE, cookie)
            .send()
            .await
            .unwrap();
        assert_eq!(root.status(), reqwest::StatusCode::OK);
        assert_eq!(
            root.headers()
                .get(reqwest::header::X_CONTENT_TYPE_OPTIONS)
                .unwrap(),
            "nosniff"
        );
        task.abort();
    }

    #[tokio::test]
    async fn proxy_hides_daemon_token_and_rejects_cross_origin_mutation() {
        let (report, task, counter) = start_pair().await;
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap();
        let cookie = authenticate(&client, &report).await;
        let sessions = client
            .get(format!("http://{}/api/v1/sessions", report.bind))
            .header(reqwest::header::COOKIE, &cookie)
            .send()
            .await
            .unwrap();
        assert_eq!(sessions.status(), reqwest::StatusCode::OK);
        assert_eq!(*counter.lock().await, 1);

        let denied = client
            .post(format!("http://{}/api/v1/sessions", report.bind))
            .header(reqwest::header::COOKIE, &cookie)
            .header(reqwest::header::ORIGIN, "https://attacker.invalid")
            .json(&json!({"objective": "x", "repository": "/tmp"}))
            .send()
            .await
            .unwrap();
        assert_eq!(denied.status(), reqwest::StatusCode::FORBIDDEN);

        let accepted = client
            .post(format!("http://{}/api/v1/sessions", report.bind))
            .header(reqwest::header::COOKIE, &cookie)
            .header(reqwest::header::ORIGIN, format!("http://{}", report.bind))
            .json(&json!({"objective": "x", "repository": "/tmp"}))
            .send()
            .await
            .unwrap();
        assert_eq!(accepted.status(), reqwest::StatusCode::ACCEPTED);

        let streamed = client
            .get(format!(
                "http://{}/api/v1/sessions/run-1/events/stream",
                report.bind
            ))
            .header(reqwest::header::COOKIE, &cookie)
            .header(reqwest::header::ACCEPT, "text/event-stream")
            .send()
            .await
            .unwrap();
        assert_eq!(streamed.status(), reqwest::StatusCode::OK);
        assert!(streamed.text().await.unwrap().contains("durable_audit"));
        task.abort();
    }

    #[tokio::test]
    async fn public_bind_and_incompatible_daemon_fail_closed() {
        let result = bind_and_report(StudioConfig {
            bind: "0.0.0.0:0".parse().unwrap(),
            daemon_url: Url::parse("http://127.0.0.1:7377/").unwrap(),
            daemon_token: "daemon-secret-that-is-long-enough".into(),
            repository: PathBuf::from("/tmp"),
            expected_daemon_api_version: 2,
            expected_studio_api_version: 1,
        })
        .await;
        let error = match result {
            Ok(_) => panic!("public Studio bind must fail closed"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("non-loopback"));
    }

    #[test]
    fn proxy_path_cannot_escape_the_versioned_api() {
        assert!(valid_proxy_path("v1/sessions/123/events"));
        assert!(!valid_proxy_path("v1/../health"));
        assert!(!valid_proxy_path("v1//sessions"));
        assert!(!valid_proxy_path("internal/metrics"));
        assert!(!valid_proxy_path("v1/%2e%2e/health"));
    }

    #[test]
    fn embedded_studio_is_session_first() {
        // PRD §24.3: sessions, one conversation, a composer, a hidden drawer.
        for surface in [
            r#"aria-label="Sessions""#,
            r#"id="session-list""#,
            r#"id="conversation""#,
            r#"id="composer""#,
            r#"id="context-drawer""#,
            r#"id="settings-modal""#,
        ] {
            assert!(INDEX_HTML.contains(surface), "missing {surface}");
        }
        // PRD §24.4: no dashboard shell and no unfinished enterprise pages.
        for forbidden in [
            "Agent Factory",
            "Deployments",
            "Agent Runs",
            "Workspaces",
            r#"class="nav-item""#,
        ] {
            assert!(
                !INDEX_HTML.contains(forbidden),
                "{forbidden} must not appear in the primary navigation"
            );
        }
        // The drawer opens contextually rather than owning permanent space.
        assert!(INDEX_HTML.contains(r#"id="context-drawer" class="context-drawer hidden""#));
        assert!(!INDEX_HTML.contains("<script>"));
        assert!(!INDEX_HTML.contains("<style>"));
        assert!(APP_JS.contains("/messages"));
        assert!(APP_JS.contains("/events"));
        assert!(APP_JS.contains("/diff"));
        assert!(APP_CSS.contains("user-select: text"));
        assert!(APP_CSS.contains("@media (max-width: 800px)"));
    }

    #[test]
    fn embedded_studio_can_see_and_change_the_active_model() {
        // PRD §10.1 requires the active model to be visible at all times, and
        // §36 fails the release if the primary UI cannot select one. The
        // settings modal existed as markup with nothing wired to it.
        assert!(INDEX_HTML.contains(r#"id="settings-open""#));
        assert!(INDEX_HTML.contains(r#"id="model-choices""#));
        assert!(APP_JS.contains("#settings-open\").addEventListener"));
        assert!(APP_JS.contains("/api/v1/models"));
        assert!(APP_JS.contains("/api/v1/providers"));
        // The canonical role name, not the alias the old config used.
        assert!(APP_JS.contains(r#"role: "coding_worker""#));
        // The header and the composer read one value, so they cannot disagree.
        assert!(APP_JS.contains("#model-info\").textContent = label"));
        assert!(APP_JS.contains("#composer-model\").textContent = label"));
    }

    #[test]
    fn embedded_studio_lets_the_user_choose_task_and_permission_modes() {
        // PRD §11, §12, §16. Studio previously printed "Build" and "Ask" as
        // static text, which reads as a setting the user cannot reach.
        for control in [
            r#"<select id="composer-mode""#,
            r#"<select id="composer-permission""#,
            r#"<select id="settings-permission""#,
            r#"<select id="settings-default-mode""#,
        ] {
            assert!(INDEX_HTML.contains(control), "missing {control}");
        }
        // Ask and Plan must reach the daemon as a constraint, not as a hint it
        // might infer from the objective's wording.
        assert!(APP_JS.contains(r#"READ_ONLY_MODES = ["ask", "plan"]"#));
        assert!(APP_JS.contains("plan_only: READ_ONLY_MODES.includes(state.taskMode)"));
        assert!(APP_JS.contains("authority_mode: AUTHORITY_MODES[state.permission]"));
        // Full Access invites a larger reading than it deserves, so the text
        // states what it does not grant.
        assert!(APP_JS.contains("It grants no new ones"));
    }

    #[test]
    fn studio_settings_covers_every_prd_24_6_section() {
        for section in [
            "Models and providers",
            "Permissions",
            "Appearance",
            "Repository defaults",
            "Terminal",
            "Advanced",
            "Experimental",
        ] {
            assert!(
                INDEX_HTML.contains(&format!("<h3>{section}</h3>")),
                "settings is missing the {section} section"
            );
        }
        // With every section present the dialog is taller than a short
        // viewport, so it must scroll inside itself and keep Close reachable.
        assert!(APP_CSS.contains("max-height: 85vh"));
        assert!(APP_CSS.contains(".modal-header { position: sticky"));
    }

    #[test]
    fn studio_reads_activity_from_the_daemon_not_from_raw_events() {
        // Observed in a real session: the activity list read "Submodules
        // Prepared" and "Model Request Started" because the client title-cased
        // raw event names. That is internal vocabulary on the main surface
        // (PRD §15.1) and a second, divergent reading of the run from the one
        // the Workbench shows (§31).
        assert!(APP_JS.contains("/activity"));
        assert!(APP_JS.contains("/validation"));
        assert!(
            !APP_JS.contains("function eventTitle"),
            "no client may invent its own labels for durable events"
        );
        assert!(
            !APP_JS.contains(r#"split("_")"#),
            "an event name split on underscores is a raw event name"
        );
        // Status reaches the reader as a word, not only as a glyph or colour.
        assert!(APP_JS.contains("activity-state"));
    }

    #[test]
    fn studio_shows_the_plan_a_paused_run_asks_you_to_review() {
        assert!(APP_JS.contains("function renderPlan"));
        assert!(APP_JS.contains("summary?.plan"));
        // It renders every step, not a truncated summary of the first one.
        assert!(APP_JS.contains("state.plan\n    .map((step)"));
        assert!(APP_CSS.contains(".plan-steps"));
    }

    #[test]
    fn a_model_change_reports_each_scope_it_actually_changed() {
        // Observed while testing: the repository default changed, the header
        // followed it, and the open session kept running on the old model — but
        // the only message was an unconditional "Model set to ...". A user
        // cannot act on a success that may not have happened.
        assert!(APP_JS.contains("for this session and new work"));
        assert!(APP_JS.contains("This session kept its model"));
        assert!(APP_JS.contains("for new work in this repository"));
        assert!(
            !APP_JS.contains("`Model set to ${id}`"),
            "one message for two scopes cannot be truthful about either"
        );
        // The header reads the session's model when a session is open.
        assert!(APP_JS.contains("state.selectedSession?.selected_model"));
    }

    #[test]
    fn a_reviewed_plan_can_be_turned_into_work_in_one_action() {
        // A plan-only run pauses saying it is "ready for review", and the
        // composer refuses follow-ups on a paused session. Without an action on
        // the plan itself the only way forward was to start a new session and
        // describe the work again — the runtime has continued from the plan on
        // resume all along, but no client offered it.
        assert!(APP_JS.contains("Build this plan"));
        assert!(APP_JS.contains("/resume"));
        // It says what it will do and that nothing has happened yet.
        assert!(APP_JS.contains("Nothing has been changed yet"));
        // A refused follow-up points at that action instead of dead-ending.
        assert!(APP_JS.contains("Use Build this plan to continue it"));
        // Bound by delegation: the plan block is re-rendered on every refresh.
        assert!(APP_JS.contains(r#"event.target.id === "build-plan""#));
    }

    #[test]
    fn embedded_studio_terminal_is_emulated_and_incremental() {
        // PRD §24.7: a real emulator, not a stripped log.
        assert!(APP_JS.contains("new WebSocket"));
        assert!(APP_JS.contains("/studio/terminals/"));
        assert!(
            APP_JS.contains(r#"import { Terminal, measure } from "/term.js""#),
            "the Studio must drive the terminal emulator"
        );
        assert!(
            !APP_JS.contains(r"\x1b(?:[@-_]|\[[0-?]*[ -/]*[@-~])"),
            "escape sequences must be interpreted, not stripped"
        );
        assert!(INDEX_HTML.contains(r#"<script type="module" src="/app.js">"#));
        for capability in [
            "class Terminal",
            "applyStyle",
            "eraseDisplay",
            "scrollUp",
            "resize(",
            "keyToBytes",
        ] {
            assert!(TERM_JS.contains(capability), "emulator lacks {capability}");
        }
        // The shell must not re-send the whole transcript on a timer.
        assert!(
            APP_JS.contains("chunk.next_offset") || APP_JS.contains(r#"frame.chunk"#),
            "the client must consume incremental chunks"
        );
    }

    #[tokio::test]
    async fn a_browser_without_a_session_is_told_how_to_get_one() {
        // Reached by ordinary means — a bookmarked bare URL, a second browser,
        // cleared cookies, a restarted shell. A bare "authentication required"
        // leaves the reader unable to tell a malfunction from a security
        // property, and with nothing to do about either.
        let (report, server, _) = start_pair().await;
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap();
        let response = client
            .get(format!("http://{}/", report.bind))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::UNAUTHORIZED);
        assert!(response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .is_some_and(|value| value.to_str().unwrap_or_default().contains("text/html")));
        let body = response.text().await.unwrap();
        // It explains, and it says exactly what to run.
        assert!(body.contains("single-use"));
        assert!(body.contains("purrcode studio"));
        assert!(body.contains("not a fault"));
        // And it reassures that recovering costs nothing.
        assert!(body.contains("nothing is lost"));
        // Unstyled on purpose: /app.css needs the session this reader lacks.
        assert!(!body.contains("app.css"));
        server.abort();
    }

    #[tokio::test]
    async fn terminal_script_requires_the_session_cookie() {
        let (report, server, _) = start_pair().await;
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap();
        let url = format!("http://{}/term.js", report.bind);
        let unauthorized = client.get(&url).send().await.unwrap();
        assert_eq!(unauthorized.status(), reqwest::StatusCode::UNAUTHORIZED);
        let cookie = authenticate(&client, &report).await;
        let authorized = client
            .get(&url)
            .header(reqwest::header::COOKIE, &cookie)
            .send()
            .await
            .unwrap();
        assert_eq!(authorized.status(), reqwest::StatusCode::OK);
        assert!(authorized.text().await.unwrap().contains("class Terminal"));
        server.abort();
    }
}
