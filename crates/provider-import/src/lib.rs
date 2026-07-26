//! Parse-only provider configuration detection and secret-safe source redaction.
//!
//! This crate never executes imported input. All parsers are bounded to [`DEFAULT_MAX_INPUT_BYTES`]
//! and return source spans so callers can present reviewed, editable candidates.

use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::OnceLock;
use thiserror::Error;

mod parser;
pub use parser::{import_provider, InputFormat};

mod normalizer;
pub use normalizer::{normalize_candidate, NormalizedProviderProfile};

pub const DEFAULT_MAX_INPUT_BYTES: usize = 256 * 1024;
pub const REDACTION_TOKEN: &str = "[REDACTED_SECRET]";

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContentKind {
    Prose,
    Code,
    Log,
    ProviderConfiguration,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Confidence {
    Low,
    Medium,
    High,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SourceSpan {
    pub start: usize,
    pub end: usize,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SecretFinding {
    pub kind: String,
    pub span: SourceSpan,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ContentDetection {
    pub kind: ContentKind,
    pub confidence: Confidence,
    pub secret_findings: Vec<SecretFinding>,
    pub provider_signals: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RedactedSource {
    pub display: String,
    pub findings: Vec<SecretFinding>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Extracted<T> {
    pub value: T,
    pub confidence: Confidence,
    pub source_span: Option<SourceSpan>,
    pub requires_confirmation: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProviderKind {
    OpenAi,
    OpenAiCompatible,
    Ollama,
    LmStudio,
    Unknown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ApiMode {
    Responses,
    ChatCompletions,
    Unknown,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "value")]
pub enum AuthReference {
    Environment(String),
    SecretDetected,
    None,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProviderImportCandidate {
    pub provider_kind: ProviderKind,
    pub suggested_name: String,
    pub base_url: Option<Extracted<String>>,
    pub model_id: Option<Extracted<String>>,
    pub auth: Option<Extracted<AuthReference>>,
    pub api_mode: Option<Extracted<ApiMode>>,
    pub defaults: BTreeMap<String, Extracted<serde_json::Value>>,
    pub custom_headers: BTreeMap<String, Extracted<String>>,
    pub extra_body: Option<Extracted<serde_json::Value>>,
    pub is_local: Option<Extracted<bool>>,
    pub warnings: Vec<ImportWarning>,
    pub redacted_source: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ImportWarning {
    pub code: String,
    pub message: String,
    pub source_span: Option<SourceSpan>,
}

#[derive(Debug, Error, Eq, PartialEq)]
pub enum ImportError {
    #[error("provider import exceeds the {maximum_bytes}-byte limit")]
    InputTooLarge { maximum_bytes: usize },
    #[error("input does not contain a supported provider configuration")]
    Unsupported,
    #[error("{format} input is malformed: {message}")]
    Malformed { format: String, message: String },
}

/// Classifies bounded, untrusted source without executing or evaluating it.
pub fn detect_content(input: &str) -> Result<ContentDetection, ImportError> {
    enforce_size(input, DEFAULT_MAX_INPUT_BYTES)?;
    let redacted = redact_source(input)?;
    let lower = input.to_ascii_lowercase();
    let mut signals = Vec::new();
    for (needle, label) in [
        ("base_url", "base_url"),
        ("api_key", "api_key"),
        ("openai(", "openai_sdk"),
        ("chat.completions", "chat_completions"),
        ("/v1/chat/completions", "chat_completions_endpoint"),
        ("ollama", "ollama"),
        ("lm studio", "lm_studio"),
    ] {
        if lower.contains(needle) {
            signals.push(label.to_owned());
        }
    }
    let provider = signals.len() >= 2 || (signals.len() == 1 && !redacted.findings.is_empty());
    let log_markers = [
        "traceback (most recent call last)",
        "error:",
        "warn ",
        "stack trace",
    ];
    let code_markers = [
        "def ",
        "const ",
        "function ",
        "curl ",
        "import ",
        "export ",
        "={",
    ];
    let (kind, confidence) = if provider {
        (ContentKind::ProviderConfiguration, Confidence::High)
    } else if log_markers.iter().any(|marker| lower.contains(marker)) {
        (ContentKind::Log, Confidence::High)
    } else if input.contains('\n') && code_markers.iter().any(|marker| lower.contains(marker)) {
        (ContentKind::Code, Confidence::Medium)
    } else {
        (ContentKind::Prose, Confidence::Medium)
    };
    Ok(ContentDetection {
        kind,
        confidence,
        secret_findings: redacted.findings,
        provider_signals: signals,
    })
}

/// Produces a display/context-safe representation. Findings never contain secret values.
pub fn redact_source(input: &str) -> Result<RedactedSource, ImportError> {
    enforce_size(input, DEFAULT_MAX_INPUT_BYTES)?;
    let mut findings = Vec::new();
    for (kind, regex) in secret_patterns() {
        for captures in regex.captures_iter(input) {
            if let Some(secret) = captures.name("secret") {
                if *kind == "named_secret" && looks_like_secret_reference(secret.as_str()) {
                    continue;
                }
                findings.push(SecretFinding {
                    kind: (*kind).to_owned(),
                    span: SourceSpan {
                        start: secret.start(),
                        end: secret.end(),
                    },
                });
            }
        }
    }
    findings.sort_by_key(|finding| (finding.span.start, finding.span.end));
    let mut non_overlapping: Vec<SecretFinding> = Vec::new();
    for finding in findings {
        if let Some(previous) = non_overlapping.last_mut() {
            if finding.span.start < previous.span.end {
                previous.span.end = previous.span.end.max(finding.span.end);
                if previous.kind != finding.kind {
                    previous.kind = "multiple_secret_signals".to_owned();
                }
                continue;
            }
            if finding.span == previous.span {
                continue;
            }
        }
        non_overlapping.push(finding);
    }
    let findings = non_overlapping;
    let mut display = input.to_owned();
    for finding in findings.iter().rev() {
        display.replace_range(finding.span.start..finding.span.end, REDACTION_TOKEN);
    }
    Ok(RedactedSource { display, findings })
}

fn looks_like_secret_reference(value: &str) -> bool {
    let normalized = value.to_ascii_lowercase();
    normalized.starts_with("os.getenv(")
        || normalized.starts_with("os.environ")
        || normalized.starts_with("process.env.")
        || normalized.starts_with("get_secret(")
        || value.starts_with('$')
}

fn enforce_size(input: &str, maximum_bytes: usize) -> Result<(), ImportError> {
    if input.len() > maximum_bytes {
        return Err(ImportError::InputTooLarge { maximum_bytes });
    }
    Ok(())
}

fn secret_patterns() -> &'static [(&'static str, Regex)] {
    static PATTERNS: OnceLock<Vec<(&'static str, Regex)>> = OnceLock::new();
    PATTERNS.get_or_init(|| {
        [
            ("provider_key", r#"(?i)(?:nvapi-|sk-(?:proj-)?|AIza)[A-Za-z0-9_\-]{8,}"#),
            ("named_secret", r#"(?i)(?:api[_-]?key|access[_-]?token|secret)\s*[:=]\s*[\"']?(?P<secret>[^\s\"',;}]+)"#),
            ("authorization", r#"(?i)authorization\s*[:=]\s*(?:bearer|basic)?\s*(?P<secret>[A-Za-z0-9._~+\-/=]{8,})"#),
            ("bearer", r#"(?i)bearer\s+(?P<secret>[A-Za-z0-9._~+\-/=]{8,})"#),
            ("url_credential", r#"https?://[^\s/@:]+:(?P<secret>[^\s/@]+)@"#),
        ]
        .into_iter()
        .map(|(kind, pattern)| {
            let wrapped = if pattern.contains("?P<secret>") { pattern.to_owned() } else { format!("(?P<secret>{pattern})") };
            (kind, Regex::new(&wrapped).expect("static secret regex must compile"))
        })
        .collect()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_sample_is_detected_and_secret_is_not_returned_in_metadata() {
        let input =
            "client = OpenAI(base_url=\"https://example.com/v1\", api_key=\"sk-example123456\")";
        let detection = detect_content(input).unwrap();
        assert_eq!(detection.kind, ContentKind::ProviderConfiguration);
        assert!(!detection.secret_findings.is_empty());
        let encoded = serde_json::to_string(&detection).unwrap();
        assert!(!encoded.contains("sk-example123456"));
        let redacted = redact_source(input).unwrap();
        assert!(!redacted.display.contains("sk-example123456"));
        assert!(redacted.display.contains(REDACTION_TOKEN));
    }

    #[test]
    fn authorization_headers_urls_and_nested_json_are_redacted() {
        for source in [
            "Authorization: Bearer token.that-is-secret",
            "https://user:password123@example.com/v1",
            r#"{"nested":{"api_key":"nvapi-secretvalue123"}}"#,
        ] {
            let redacted = redact_source(source).unwrap();
            assert_eq!(redacted.findings.len(), 1, "{source}");
            assert!(redacted.display.contains(REDACTION_TOKEN), "{source}");
        }
    }

    #[test]
    fn oversized_input_fails_closed() {
        let input = "x".repeat(DEFAULT_MAX_INPUT_BYTES + 1);
        assert!(matches!(
            detect_content(&input),
            Err(ImportError::InputTooLarge { .. })
        ));
    }

    #[test]
    fn environment_and_dynamic_secret_references_are_not_misclassified_as_values() {
        for source in [
            r#"api_key=os.getenv("OPENAI_API_KEY")"#,
            "api_key=process.env.OPENAI_API_KEY",
            "api_key=$OPENAI_API_KEY",
            "api_key=get_secret()",
        ] {
            assert!(
                redact_source(source).unwrap().findings.is_empty(),
                "{source}"
            );
        }
    }
}
