//! Bounded and redacted diagnostics for provider failures.

use regex::Regex;
use serde::{Deserialize, Serialize};
use std::error::Error as StdError;
use std::fmt;
use std::sync::LazyLock;

pub const MAX_PROVIDER_HTTP_REQUEST_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_PROVIDER_HTTP_BODY_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_PROVIDER_ERROR_BODY_BYTES: usize = 64 * 1024;
pub const MAX_PROVIDER_STREAM_FRAME_BYTES: usize = 256 * 1024;
pub const MAX_PROVIDER_DIAGNOSTIC_BYTES: usize = 2 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderApiMode {
    Responses,
    OpenaiCompatible,
    OllamaNative,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderErrorCategory {
    Dns,
    Tls,
    Authentication,
    HttpStatus,
    ContentType,
    Schema,
    StreamFraming,
    ApiModeMismatch,
    ModelNotFound,
    ContextTooLarge,
    OutOfMemory,
    Cancelled,
    Unreachable,
    Timeout,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ProviderDiagnostic {
    pub category: ProviderErrorCategory,
    pub summary: String,
    pub api_mode: ProviderApiMode,
    pub http_status: Option<u16>,
    pub excerpt: Option<String>,
    pub truncated: bool,
}

impl fmt::Display for ProviderDiagnostic {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.summary)?;
        if let Some(status) = self.http_status {
            write!(formatter, " (HTTP {status})")?;
        }
        if let Some(excerpt) = &self.excerpt {
            write!(formatter, ": {excerpt}")?;
        }
        Ok(())
    }
}

pub(crate) fn transport_diagnostic(
    error: &reqwest::Error,
    api_mode: ProviderApiMode,
) -> ProviderDiagnostic {
    let mut detail = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        detail.push_str(": ");
        detail.push_str(&cause.to_string());
        source = cause.source();
    }
    transport_text_diagnostic(&detail, error.is_timeout(), error.is_connect(), api_mode)
}

pub(crate) fn transport_text_diagnostic(
    detail: &str,
    timeout: bool,
    connect: bool,
    api_mode: ProviderApiMode,
) -> ProviderDiagnostic {
    let normalized = detail.to_ascii_lowercase();
    let (category, summary) = if normalized.contains("cancelled") || normalized.contains("canceled")
    {
        (
            ProviderErrorCategory::Cancelled,
            "Provider request was cancelled",
        )
    } else if timeout {
        (ProviderErrorCategory::Timeout, "Provider request timed out")
    } else if contains_any(
        &normalized,
        &[
            "dns",
            "failed to lookup address",
            "name or service not known",
            "nodename nor servname",
            "temporary failure in name resolution",
        ],
    ) {
        (
            ProviderErrorCategory::Dns,
            "Provider hostname could not be resolved",
        )
    } else if contains_any(
        &normalized,
        &["tls", "rustls", "certificate", "invalid peer", "handshake"],
    ) {
        (
            ProviderErrorCategory::Tls,
            "TLS negotiation with the provider failed",
        )
    } else if connect
        || contains_any(
            &normalized,
            &[
                "connection refused",
                "connection reset",
                "network is unreachable",
                "broken pipe",
            ],
        )
    {
        (
            ProviderErrorCategory::Unreachable,
            "Provider endpoint is unreachable",
        )
    } else {
        (
            ProviderErrorCategory::Unreachable,
            "Provider transport failed",
        )
    };
    diagnostic(
        category,
        summary,
        api_mode,
        None,
        Some(detail.as_bytes()),
        false,
    )
}

pub(crate) fn http_diagnostic(
    status: u16,
    body: &[u8],
    body_truncated: bool,
    api_mode: ProviderApiMode,
) -> ProviderDiagnostic {
    let normalized = String::from_utf8_lossy(body).to_ascii_lowercase();
    let category = if matches!(status, 401 | 403) {
        ProviderErrorCategory::Authentication
    } else if mode_mismatch_text(&normalized, api_mode) {
        ProviderErrorCategory::ApiModeMismatch
    } else if contains_any(
        &normalized,
        &[
            "model not found",
            "model does not exist",
            "unknown model",
            "pull model",
            "no such model",
        ],
    ) {
        ProviderErrorCategory::ModelNotFound
    } else if contains_any(
        &normalized,
        &[
            "context length",
            "context window",
            "too many tokens",
            "prompt too long",
            "input length",
            "maximum context",
        ],
    ) {
        ProviderErrorCategory::ContextTooLarge
    } else if contains_any(
        &normalized,
        &[
            "out of memory",
            "not enough memory",
            "requires more system memory",
            "memory allocation",
            "insufficient memory",
        ],
    ) {
        ProviderErrorCategory::OutOfMemory
    } else if normalized.contains("cancelled") || normalized.contains("canceled") {
        ProviderErrorCategory::Cancelled
    } else {
        ProviderErrorCategory::HttpStatus
    };
    let summary = match category {
        ProviderErrorCategory::Authentication => "Provider rejected authentication",
        ProviderErrorCategory::ApiModeMismatch => {
            "Provider response does not match the selected API mode"
        }
        ProviderErrorCategory::ModelNotFound => "Selected provider model was not found",
        ProviderErrorCategory::ContextTooLarge => "Provider context limit was exceeded",
        ProviderErrorCategory::OutOfMemory => "Provider ran out of memory",
        ProviderErrorCategory::Cancelled => "Provider request was cancelled",
        _ => "Provider returned an unsuccessful HTTP status",
    };
    diagnostic(
        category,
        summary,
        api_mode,
        Some(status),
        Some(body),
        body_truncated,
    )
}

pub(crate) fn content_type_diagnostic(
    observed: Option<&str>,
    expected: &str,
    streaming: bool,
    api_mode: ProviderApiMode,
) -> ProviderDiagnostic {
    let observed = observed.unwrap_or("<missing>");
    let normalized = observed.to_ascii_lowercase();
    let category = if (api_mode == ProviderApiMode::OllamaNative
        && normalized.starts_with("text/event-stream"))
        || (api_mode != ProviderApiMode::OllamaNative
            && (normalized.starts_with("application/x-ndjson")
                || normalized.starts_with("application/ndjson")))
    {
        ProviderErrorCategory::ApiModeMismatch
    } else {
        ProviderErrorCategory::ContentType
    };
    let summary = if category == ProviderErrorCategory::ApiModeMismatch {
        "Provider content type belongs to a different API mode"
    } else if streaming {
        "Provider stream returned an unsupported content type"
    } else {
        "Provider response returned an unsupported content type"
    };
    diagnostic(
        category,
        summary,
        api_mode,
        None,
        Some(format!("expected {expected}; observed {observed}").as_bytes()),
        false,
    )
}

pub(crate) fn schema_diagnostic(
    summary: &str,
    body: Option<&[u8]>,
    body_truncated: bool,
    api_mode: ProviderApiMode,
) -> ProviderDiagnostic {
    let category = body
        .and_then(|body| std::str::from_utf8(body).ok())
        .filter(|body| mode_mismatch_text(&body.to_ascii_lowercase(), api_mode))
        .map(|_| ProviderErrorCategory::ApiModeMismatch)
        .unwrap_or(ProviderErrorCategory::Schema);
    diagnostic(
        category,
        if category == ProviderErrorCategory::ApiModeMismatch {
            "Provider payload does not match the selected API mode"
        } else {
            summary
        },
        api_mode,
        None,
        body,
        body_truncated,
    )
}

pub(crate) fn stream_diagnostic(
    summary: &str,
    frame: Option<&[u8]>,
    frame_truncated: bool,
    api_mode: ProviderApiMode,
) -> ProviderDiagnostic {
    let category = frame
        .and_then(|frame| std::str::from_utf8(frame).ok())
        .filter(|frame| mode_mismatch_text(&frame.to_ascii_lowercase(), api_mode))
        .map(|_| ProviderErrorCategory::ApiModeMismatch)
        .unwrap_or(ProviderErrorCategory::StreamFraming);
    diagnostic(
        category,
        if category == ProviderErrorCategory::ApiModeMismatch {
            "Provider stream does not match the selected API mode"
        } else {
            summary
        },
        api_mode,
        None,
        frame,
        frame_truncated,
    )
}

pub(crate) fn cancelled_diagnostic(reason: &str, api_mode: ProviderApiMode) -> ProviderDiagnostic {
    diagnostic(
        ProviderErrorCategory::Cancelled,
        "Provider request was cancelled",
        api_mode,
        None,
        Some(reason.as_bytes()),
        false,
    )
}

pub(crate) fn request_too_large_diagnostic(
    actual: usize,
    api_mode: ProviderApiMode,
) -> ProviderDiagnostic {
    diagnostic(
        ProviderErrorCategory::ContextTooLarge,
        "Provider request body exceeded the gateway limit",
        api_mode,
        None,
        Some(
            format!("{actual} bytes exceeds the {MAX_PROVIDER_HTTP_REQUEST_BYTES} byte limit")
                .as_bytes(),
        ),
        false,
    )
}

pub(crate) fn response_too_large_diagnostic(
    limit: usize,
    api_mode: ProviderApiMode,
) -> ProviderDiagnostic {
    diagnostic(
        ProviderErrorCategory::Schema,
        "Provider response body exceeded the gateway limit",
        api_mode,
        None,
        Some(format!("response exceeded the {limit} byte limit").as_bytes()),
        true,
    )
}

fn diagnostic(
    category: ProviderErrorCategory,
    summary: &str,
    api_mode: ProviderApiMode,
    http_status: Option<u16>,
    excerpt: Option<&[u8]>,
    already_truncated: bool,
) -> ProviderDiagnostic {
    let (summary, summary_truncated) = truncate_utf8(summary, 256);
    let (excerpt, excerpt_truncated) = excerpt
        .map(redacted_excerpt)
        .map_or((None, false), |(value, truncated)| (Some(value), truncated));
    ProviderDiagnostic {
        category,
        summary,
        api_mode,
        http_status,
        excerpt,
        truncated: already_truncated || summary_truncated || excerpt_truncated,
    }
}

fn redacted_excerpt(bytes: &[u8]) -> (String, bool) {
    let decoded = String::from_utf8_lossy(bytes);
    let mut redacted = if let Ok(mut value) = serde_json::from_str::<serde_json::Value>(&decoded) {
        redact_json(&mut value);
        serde_json::to_string(&value).unwrap_or_else(|_| "[unavailable]".into())
    } else {
        decoded.into_owned()
    };
    for expression in [
        &*BEARER_SECRET,
        &*KEY_VALUE_SECRET,
        &*TOKEN_PREFIX_SECRET,
        &*URL_CREDENTIAL_SECRET,
    ] {
        redacted = expression
            .replace_all(&redacted, "${1}[REDACTED]")
            .into_owned();
    }
    let (redacted, truncated) = truncate_utf8(&redacted, MAX_PROVIDER_DIAGNOSTIC_BYTES);
    (redacted, truncated)
}

fn redact_json(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(map) => {
            for (key, value) in map {
                let normalized = key.to_ascii_lowercase();
                if contains_any(
                    &normalized,
                    &[
                        "authorization",
                        "api_key",
                        "apikey",
                        "token",
                        "secret",
                        "password",
                        "credential",
                        "cookie",
                    ],
                ) {
                    *value = serde_json::Value::String("[REDACTED]".into());
                } else {
                    redact_json(value);
                }
            }
        }
        serde_json::Value::Array(values) => {
            for value in values {
                redact_json(value);
            }
        }
        _ => {}
    }
}

fn mode_mismatch_text(text: &str, api_mode: ProviderApiMode) -> bool {
    match api_mode {
        ProviderApiMode::OllamaNative => contains_any(
            text,
            &[
                "\"choices\"",
                "\"object\":\"chat.completion",
                "\"output_text\"",
                "/v1/chat/completions",
            ],
        ),
        ProviderApiMode::Responses | ProviderApiMode::OpenaiCompatible => contains_any(
            text,
            &[
                "\"done\":",
                "\"done_reason\"",
                "\"prompt_eval_count\"",
                "\"eval_count\"",
            ],
        ),
    }
}

fn contains_any(text: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| text.contains(needle))
}

fn truncate_utf8(value: &str, maximum: usize) -> (String, bool) {
    if value.len() <= maximum {
        return (value.to_owned(), false);
    }
    const MARKER: &str = " [truncated]";
    let target = maximum.saturating_sub(MARKER.len());
    let mut boundary = target.min(value.len());
    while boundary > 0 && !value.is_char_boundary(boundary) {
        boundary -= 1;
    }
    let mut result = value[..boundary].to_owned();
    result.push_str(MARKER);
    (result, true)
}

static KEY_VALUE_SECRET: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?i)\b((?:authorization|api[-_ ]?key|token|secret|password|credential)\s*[:=]\s*["']?)[^"',\s}\]]+"#,
    )
    .expect("secret redaction regex is valid")
});
static BEARER_SECRET: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(bearer\s+)[A-Za-z0-9._~+/=-]+").expect("bearer regex is valid")
});
static TOKEN_PREFIX_SECRET: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(sk-)[A-Za-z0-9_-]{4,}").expect("token prefix regex is valid")
});
static URL_CREDENTIAL_SECRET: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(://)[^/\s:@]+:[^/\s@]+@").expect("URL credential regex is valid")
});

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transport_categories_cover_dns_tls_cancel_timeout_and_unreachable() {
        for (detail, timeout, connect, expected) in [
            ("dns lookup failed", false, true, ProviderErrorCategory::Dns),
            (
                "rustls invalid peer certificate",
                false,
                true,
                ProviderErrorCategory::Tls,
            ),
            (
                "operation canceled",
                false,
                false,
                ProviderErrorCategory::Cancelled,
            ),
            (
                "deadline elapsed",
                true,
                false,
                ProviderErrorCategory::Timeout,
            ),
            (
                "connection refused",
                false,
                true,
                ProviderErrorCategory::Unreachable,
            ),
        ] {
            assert_eq!(
                transport_text_diagnostic(detail, timeout, connect, ProviderApiMode::OllamaNative)
                    .category,
                expected
            );
        }
    }

    #[test]
    fn http_categories_cover_provider_specific_failure_modes() {
        for (status, body, mode, expected) in [
            (
                401,
                "invalid credentials",
                ProviderApiMode::OllamaNative,
                ProviderErrorCategory::Authentication,
            ),
            (
                404,
                "model not found; pull model first",
                ProviderApiMode::OllamaNative,
                ProviderErrorCategory::ModelNotFound,
            ),
            (
                400,
                "prompt exceeds maximum context window",
                ProviderApiMode::OllamaNative,
                ProviderErrorCategory::ContextTooLarge,
            ),
            (
                500,
                "runner failed: out of memory",
                ProviderApiMode::OllamaNative,
                ProviderErrorCategory::OutOfMemory,
            ),
            (
                499,
                "request cancelled by client",
                ProviderApiMode::OllamaNative,
                ProviderErrorCategory::Cancelled,
            ),
            (
                400,
                r#"{"choices":[]}"#,
                ProviderApiMode::OllamaNative,
                ProviderErrorCategory::ApiModeMismatch,
            ),
            (
                418,
                "ordinary provider error",
                ProviderApiMode::OllamaNative,
                ProviderErrorCategory::HttpStatus,
            ),
        ] {
            assert_eq!(
                http_diagnostic(status, body.as_bytes(), false, mode).category,
                expected
            );
        }
    }

    #[test]
    fn content_schema_stream_and_cancel_categories_are_explicit() {
        assert_eq!(
            content_type_diagnostic(
                Some("text/plain"),
                "application/json",
                false,
                ProviderApiMode::OllamaNative,
            )
            .category,
            ProviderErrorCategory::ContentType
        );
        assert_eq!(
            schema_diagnostic(
                "bad schema",
                Some(b"{}"),
                false,
                ProviderApiMode::OllamaNative,
            )
            .category,
            ProviderErrorCategory::Schema
        );
        assert_eq!(
            stream_diagnostic(
                "bad frame",
                Some(b"not-json"),
                false,
                ProviderApiMode::OllamaNative,
            )
            .category,
            ProviderErrorCategory::StreamFraming
        );
        assert_eq!(
            cancelled_diagnostic("user stopped", ProviderApiMode::OllamaNative).category,
            ProviderErrorCategory::Cancelled
        );
    }

    #[test]
    fn diagnostics_redact_json_text_bearer_tokens_and_url_credentials() {
        let body = br#"{"api_key":"sk-json-secret","detail":"Bearer token-value https://user:pass@example.com sk-plaintext"}"#;
        let diagnostic = http_diagnostic(401, body, false, ProviderApiMode::OpenaiCompatible);
        let rendered = diagnostic.to_string();

        assert_eq!(diagnostic.category, ProviderErrorCategory::Authentication);
        assert!(!rendered.contains("json-secret"));
        assert!(!rendered.contains("token-value"));
        assert!(!rendered.contains("user:pass"));
        assert!(!rendered.contains("plaintext"));
        assert!(rendered.contains("[REDACTED]"));
    }

    #[test]
    fn diagnostic_excerpt_is_strictly_bounded() {
        let body = vec![b'x'; MAX_PROVIDER_DIAGNOSTIC_BYTES * 2];
        let diagnostic = http_diagnostic(500, &body, true, ProviderApiMode::OllamaNative);
        let excerpt = diagnostic.excerpt.unwrap();
        assert!(excerpt.len() <= MAX_PROVIDER_DIAGNOSTIC_BYTES);
        assert!(diagnostic.truncated);
    }
}
