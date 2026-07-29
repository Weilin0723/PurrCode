//! Parse-only provider configuration detection and secret-safe source redaction.
//!
//! This crate never executes imported input. All parsers are bounded to [`DEFAULT_MAX_INPUT_BYTES`]
//! and return source spans so callers can present reviewed, editable candidates.

use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;
use std::sync::OnceLock;
use thiserror::Error;
use zeroize::Zeroizing;

mod parser;
pub use parser::InputFormat;

mod normalizer;
pub use normalizer::{normalize_candidate, normalize_resolved_import, NormalizedProviderProfile};

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
    OllamaNative,
    Unknown,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "value")]
pub enum AuthReference {
    Environment(String),
    SecretDetected,
    None,
}

/// A durable, non-secret reference produced after the user resolves imported authentication.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "value")]
pub enum SecretReference {
    Keychain(String),
    Environment(String),
}

impl SecretReference {
    pub fn as_config_reference(&self) -> &str {
        match self {
            Self::Keychain(reference) | Self::Environment(reference) => reference,
        }
    }

    fn validate(&self) -> Result<(), ImportError> {
        match self {
            Self::Keychain(reference) => {
                let credential_name = reference.strip_prefix("keychain:").ok_or_else(|| {
                    ImportError::AuthenticationUnresolved {
                        message: "keychain authentication must use a validated keychain reference"
                            .into(),
                    }
                })?;
                let canonical = purrcode_provider_gateway::keychain_reference(credential_name)
                    .map_err(|error| ImportError::AuthenticationUnresolved {
                        message: error.to_string(),
                    })?;
                if canonical != *reference {
                    return Err(ImportError::AuthenticationUnresolved {
                        message: "keychain authentication reference is not canonical".into(),
                    });
                }
                Ok(())
            }
            Self::Environment(variable) => validate_environment_reference(variable),
        }
    }
}

/// One statically extracted secret. Its value is never serializable or printable.
pub struct TransientSecret {
    kind: String,
    source_span: SourceSpan,
    value: Zeroizing<String>,
}

impl TransientSecret {
    pub fn kind(&self) -> &str {
        &self.kind
    }

    pub fn source_span(&self) -> SourceSpan {
        self.source_span
    }

    /// Explicitly exposes the secret to the credential-storage boundary.
    ///
    /// Callers must not log, serialize, clone, or persist the returned value.
    pub fn expose_secret(&self) -> &str {
        self.value.as_str()
    }
}

impl fmt::Debug for TransientSecret {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TransientSecret")
            .field("kind", &self.kind)
            .field("source_span", &self.source_span)
            .field("value", &REDACTION_TOKEN)
            .finish()
    }
}

/// A non-serializable collection of distinct transient secret values.
pub struct TransientSecrets(Vec<TransientSecret>);

impl TransientSecrets {
    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn iter(&self) -> impl ExactSizeIterator<Item = &TransientSecret> {
        self.0.iter()
    }

    pub fn single(&self) -> Result<&TransientSecret, ImportError> {
        if self.len() != 1 {
            return Err(ImportError::AuthenticationUnresolved {
                message: format!(
                    "import contains {} distinct secrets; each secret requires separate review",
                    self.len()
                ),
            });
        }
        Ok(&self.0[0])
    }
}

impl fmt::Debug for TransientSecrets {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TransientSecrets")
            .field("count", &self.len())
            .finish()
    }
}

/// Authentication resolution state for a secure provider import.
///
/// This type intentionally does not implement `Serialize` or `Clone`: transient values must stay
/// inside one zeroizing owner until an explicitly confirmed storage choice succeeds.
pub enum ImportedSecretState {
    None,
    DetectedTransient(TransientSecrets),
    AwaitingStorageChoice(TransientSecrets),
    Stored(SecretReference),
    EnvironmentReference(String),
    Discarded,
}

impl ImportedSecretState {
    pub fn transient_secrets(&self) -> Option<&TransientSecrets> {
        match self {
            Self::DetectedTransient(secrets) | Self::AwaitingStorageChoice(secrets) => {
                Some(secrets)
            }
            Self::None | Self::Stored(_) | Self::EnvironmentReference(_) | Self::Discarded => None,
        }
    }

    pub fn begin_storage_choice(&mut self) -> Result<(), ImportError> {
        if !matches!(self, Self::DetectedTransient(_)) {
            return Err(ImportError::AuthenticationUnresolved {
                message: "storage choice is available only for a newly detected secret".into(),
            });
        }
        let current = std::mem::replace(self, Self::None);
        let Self::DetectedTransient(secrets) = current else {
            unreachable!("state was checked before replacement");
        };
        *self = Self::AwaitingStorageChoice(secrets);
        Ok(())
    }

    /// Records that the credential-storage boundary successfully stored the one transient secret.
    pub fn confirm_keychain_stored(
        &mut self,
        credential_name: &str,
        confirmed: bool,
    ) -> Result<(), ImportError> {
        if !confirmed {
            return Err(ImportError::ConfirmationRequired);
        }
        let secrets = match self {
            Self::AwaitingStorageChoice(secrets) => secrets,
            _ => {
                return Err(ImportError::AuthenticationUnresolved {
                    message: "keychain storage was not awaiting confirmation".into(),
                })
            }
        };
        secrets.single()?;
        let reference =
            purrcode_provider_gateway::keychain_reference(credential_name).map_err(|error| {
                ImportError::AuthenticationUnresolved {
                    message: error.to_string(),
                }
            })?;
        let previous = std::mem::replace(self, Self::Stored(SecretReference::Keychain(reference)));
        drop(previous);
        Ok(())
    }

    /// Records that the user explicitly converted the transient value to an environment reference.
    pub fn confirm_environment_reference(
        &mut self,
        variable: &str,
        confirmed: bool,
    ) -> Result<(), ImportError> {
        if !confirmed {
            return Err(ImportError::ConfirmationRequired);
        }
        let secrets = match self {
            Self::AwaitingStorageChoice(secrets) => secrets,
            _ => {
                return Err(ImportError::AuthenticationUnresolved {
                    message: "environment conversion was not awaiting confirmation".into(),
                })
            }
        };
        secrets.single()?;
        validate_environment_reference(variable)?;
        let previous = std::mem::replace(self, Self::EnvironmentReference(variable.to_owned()));
        drop(previous);
        Ok(())
    }

    pub fn discard(&mut self) {
        let previous = std::mem::replace(self, Self::Discarded);
        drop(previous);
    }

    pub fn reference(&self) -> Option<SecretReference> {
        match self {
            Self::Stored(reference) => Some(reference.clone()),
            Self::EnvironmentReference(variable) => {
                Some(SecretReference::Environment(variable.clone()))
            }
            Self::None
            | Self::DetectedTransient(_)
            | Self::AwaitingStorageChoice(_)
            | Self::Discarded => None,
        }
    }
}

impl fmt::Debug for ImportedSecretState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::None => formatter.write_str("None"),
            Self::DetectedTransient(secrets) => formatter
                .debug_tuple("DetectedTransient")
                .field(secrets)
                .finish(),
            Self::AwaitingStorageChoice(secrets) => formatter
                .debug_tuple("AwaitingStorageChoice")
                .field(secrets)
                .finish(),
            Self::Stored(reference) => formatter.debug_tuple("Stored").field(reference).finish(),
            Self::EnvironmentReference(variable) => formatter
                .debug_tuple("EnvironmentReference")
                .field(variable)
                .finish(),
            Self::Discarded => formatter.write_str("Discarded"),
        }
    }
}

/// Safe provider fields plus the non-serializable authentication state.
pub struct ParsedProviderImport {
    pub candidate: ProviderImportCandidate,
    pub secret_state: ImportedSecretState,
}

impl ParsedProviderImport {
    pub fn validate_auth_resolved(&self) -> Result<(), ImportError> {
        match self.candidate.auth.as_ref().map(|auth| &auth.value) {
            Some(AuthReference::SecretDetected) => match &self.secret_state {
                ImportedSecretState::Stored(reference) => reference.validate(),
                ImportedSecretState::EnvironmentReference(variable) => {
                    validate_environment_reference(variable)
                }
                ImportedSecretState::DetectedTransient(_)
                | ImportedSecretState::AwaitingStorageChoice(_) => {
                    Err(ImportError::AuthenticationUnresolved {
                        message: "transient authentication is still awaiting a storage choice"
                            .into(),
                    })
                }
                ImportedSecretState::None | ImportedSecretState::Discarded => {
                    Err(ImportError::AuthenticationUnresolved {
                        message: "detected authentication must be stored or converted to an environment reference before save or test".into(),
                    })
                }
            },
            Some(AuthReference::Environment(expected)) => {
                validate_environment_reference(expected)?;
                match &self.secret_state {
                    ImportedSecretState::EnvironmentReference(actual) if actual == expected => {
                        validate_environment_reference(actual)
                    }
                    _ => Err(ImportError::AuthenticationUnresolved {
                        message: "imported environment authentication reference is inconsistent"
                            .into(),
                    }),
                }
            }
            Some(AuthReference::None) | None => match &self.secret_state {
                ImportedSecretState::None | ImportedSecretState::Discarded => Ok(()),
                _ => Err(ImportError::AuthenticationUnresolved {
                    message: "authentication state is inconsistent with the imported candidate"
                        .into(),
                }),
            },
        }
    }
}

impl fmt::Debug for ParsedProviderImport {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ParsedProviderImport")
            .field("candidate", &self.candidate)
            .field("secret_state", &self.secret_state)
            .finish()
    }
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
    #[error("provider authentication is unresolved: {message}")]
    AuthenticationUnresolved { message: String },
    #[error("credential storage requires explicit confirmation")]
    ConfirmationRequired,
}

/// Compatibility parser returning only the serializable, fully redacted review candidate.
///
/// Callers retain ownership of `input` and are responsible for clearing it. New interactive import
/// paths should use [`import_provider_secure`] so this crate owns and zeroizes the source.
pub fn import_provider(
    input: &str,
    format_hint: Option<InputFormat>,
) -> Result<ProviderImportCandidate, ImportError> {
    enforce_size(input, DEFAULT_MAX_INPUT_BYTES)?;
    let redacted = redact_source(input)?;
    parser::parse_provider(
        &redacted.display,
        format_hint,
        !redacted.findings.is_empty(),
    )
}

/// Consumes and zeroizes provider source while retaining only explicit transient secret owners.
pub fn import_provider_secure(
    input: String,
    format_hint: Option<InputFormat>,
) -> Result<ParsedProviderImport, ImportError> {
    let input = Zeroizing::new(input);
    enforce_size(input.as_str(), DEFAULT_MAX_INPUT_BYTES)?;
    let redacted = redact_source(input.as_str())?;
    let secrets = extract_transient_secrets(input.as_str(), &redacted.findings);
    let mut candidate = parser::parse_provider(
        &redacted.display,
        format_hint,
        !redacted.findings.is_empty(),
    )?;
    if !redacted.findings.is_empty() {
        // A serializable review candidate never retains source text from a secret-bearing import.
        // Structured safe fields and source spans remain available for review.
        candidate.redacted_source = REDACTION_TOKEN.to_owned();
    }
    let secret_state = if secrets.is_empty() {
        match candidate.auth.as_ref().map(|auth| &auth.value) {
            Some(AuthReference::Environment(variable)) => {
                validate_environment_reference(variable)?;
                ImportedSecretState::EnvironmentReference(variable.clone())
            }
            Some(AuthReference::SecretDetected) => {
                return Err(ImportError::AuthenticationUnresolved {
                    message:
                        "secret markers were detected but no safe static value could be extracted"
                            .into(),
                })
            }
            Some(AuthReference::None) | None => ImportedSecretState::None,
        }
    } else {
        candidate.auth = Some(Extracted {
            value: AuthReference::SecretDetected,
            confidence: Confidence::High,
            source_span: None,
            requires_confirmation: true,
        });
        ImportedSecretState::DetectedTransient(secrets)
    };
    Ok(ParsedProviderImport {
        candidate,
        secret_state,
    })
}

/// Classifies bounded, untrusted source without executing or evaluating it.
pub fn detect_content(input: &str) -> Result<ContentDetection, ImportError> {
    enforce_size(input, DEFAULT_MAX_INPUT_BYTES)?;
    let redacted = redact_source(input)?;
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
        if contains_ascii_case_insensitive(input, needle) {
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
    } else if log_markers
        .iter()
        .any(|marker| contains_ascii_case_insensitive(input, marker))
    {
        (ContentKind::Log, Confidence::High)
    } else if input.contains('\n')
        && code_markers
            .iter()
            .any(|marker| contains_ascii_case_insensitive(input, marker))
    {
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
    let findings = find_secret_findings(input);
    let mut display = String::with_capacity(input.len());
    let mut cursor = 0;
    for finding in &findings {
        display.push_str(&input[cursor..finding.span.start]);
        display.push_str(REDACTION_TOKEN);
        cursor = finding.span.end;
    }
    display.push_str(&input[cursor..]);
    Ok(RedactedSource { display, findings })
}

fn find_secret_findings(input: &str) -> Vec<SecretFinding> {
    let mut findings = Vec::new();
    for (kind, regex) in secret_patterns() {
        for captures in regex.captures_iter(input) {
            if let Some(secret) = captures.name("secret") {
                if kind.starts_with("named_secret") && looks_like_secret_reference(secret.as_str())
                {
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
    non_overlapping
}

fn extract_transient_secrets(input: &str, findings: &[SecretFinding]) -> TransientSecrets {
    let mut secrets: Vec<TransientSecret> = Vec::new();
    for finding in findings {
        let value = &input[finding.span.start..finding.span.end];
        if secrets
            .iter()
            .any(|existing| existing.expose_secret() == value)
        {
            continue;
        }
        secrets.push(TransientSecret {
            kind: finding.kind.clone(),
            source_span: finding.span,
            value: Zeroizing::new(value.to_owned()),
        });
    }
    TransientSecrets(secrets)
}

fn looks_like_secret_reference(value: &str) -> bool {
    starts_with_ascii_case_insensitive(value, "os.getenv(")
        || starts_with_ascii_case_insensitive(value, "os.environ")
        || starts_with_ascii_case_insensitive(value, "process.env.")
        || starts_with_ascii_case_insensitive(value, "get_secret(")
        || value.starts_with('$')
}

fn contains_ascii_case_insensitive(input: &str, needle: &str) -> bool {
    input
        .as_bytes()
        .windows(needle.len())
        .any(|window| window.eq_ignore_ascii_case(needle.as_bytes()))
}

fn starts_with_ascii_case_insensitive(input: &str, prefix: &str) -> bool {
    input
        .as_bytes()
        .get(..prefix.len())
        .is_some_and(|value| value.eq_ignore_ascii_case(prefix.as_bytes()))
}

fn enforce_size(input: &str, maximum_bytes: usize) -> Result<(), ImportError> {
    if input.len() > maximum_bytes {
        return Err(ImportError::InputTooLarge { maximum_bytes });
    }
    Ok(())
}

fn validate_environment_reference(variable: &str) -> Result<(), ImportError> {
    let valid = !variable.is_empty()
        && variable.len() <= 128
        && variable
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_uppercase() || byte == b'_')
        && variable
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_');
    if !valid {
        return Err(ImportError::AuthenticationUnresolved {
            message: "environment reference must match [A-Z_][A-Z0-9_]{0,127}".into(),
        });
    }
    Ok(())
}

fn secret_patterns() -> &'static [(&'static str, Regex)] {
    static PATTERNS: OnceLock<Vec<(&'static str, Regex)>> = OnceLock::new();
    PATTERNS.get_or_init(|| {
        [
            ("provider_key", r#"(?i)(?:nvapi-|sk-(?:proj-)?|AIza)[A-Za-z0-9_\-]{8,}"#),
            ("named_secret_double_quoted", r#"(?i)(?:api[_-]?key|access[_-]?token|refresh[_-]?token|auth[_-]?token|client[_-]?secret|private[_-]?key|credential|cookie|password|\btoken\b|\bsecret\b)\s*[\"']?\s*[:=]\s*\"(?P<secret>(?:\\.|[^\"\\])+?)\""#),
            ("named_secret_single_quoted", r#"(?i)(?:api[_-]?key|access[_-]?token|refresh[_-]?token|auth[_-]?token|client[_-]?secret|private[_-]?key|credential|cookie|password|\btoken\b|\bsecret\b)\s*[\"']?\s*[:=]\s*'(?P<secret>(?:\\.|[^'\\])+?)'"#),
            ("named_secret", r#"(?i)(?:api[_-]?key|access[_-]?token|refresh[_-]?token|auth[_-]?token|client[_-]?secret|private[_-]?key|credential|cookie|password|\btoken\b|\bsecret\b)\s*[\"']?\s*[:=]\s*[\"']?(?P<secret>[^\s\"',;}]+)"#),
            ("sensitive_header_double_quoted", r#"(?i)(?:authorization|proxy[_-]?authorization|x[_-]?api[_-]?key|x[_-]?(?:auth|access)[_-]?token|cookie)\s*[\"']?\s*[:=]\s*\"(?:(?:bearer|basic)\s+)?(?P<secret>(?:\\.|[^\"\\])+?)\""#),
            ("sensitive_header_single_quoted", r#"(?i)(?:authorization|proxy[_-]?authorization|x[_-]?api[_-]?key|x[_-]?(?:auth|access)[_-]?token|cookie)\s*[\"']?\s*[:=]\s*'(?:(?:bearer|basic)\s+)?(?P<secret>(?:\\.|[^'\\])+?)'"#),
            ("curl_sensitive_header", r#"(?i)(?:-H|--header)\s+[\"'](?:authorization|proxy[_-]?authorization|x[_-]?api[_-]?key|x[_-]?(?:auth|access)[_-]?token|cookie)\s*:\s*(?:(?:bearer|basic)\s+)?(?P<secret>[^\"'\r\n]+)[\"']"#),
            ("sensitive_header", r#"(?i)(?:authorization|proxy[_-]?authorization|x[_-]?api[_-]?key|x[_-]?(?:auth|access)[_-]?token|cookie)\s*[\"']?\s*[:=]\s*[\"']?(?:(?:bearer|basic)\s+)?(?P<secret>[^\s\"',;}]+)"#),
            ("authorization", r#"(?i)authorization\s*[:=]\s*(?:bearer|basic)?\s*(?P<secret>[A-Za-z0-9._~+\-/=]{8,})"#),
            ("bearer", r#"(?i)bearer\s+(?P<secret>[A-Za-z0-9._~+\-/=]{8,})"#),
            ("url_userinfo", r#"(?i)https?://(?P<secret>[^\s/@]+(?::[^\s/@]*)?)@"#),
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
    fn requests_post_sample_extracts_endpoint_model_and_api_mode() {
        let secret = "nvapi-parser-fixture-123456";
        let source = format!(
            r#"invoke_url = "https://integrate.api.nvidia.com/v1/chat/completions"
headers = {{"Authorization": "{secret}"}}
payload = {{"model": "minimaxai/minimax-m3", "stream": False}}
response = requests.post(invoke_url, headers=headers, json=payload)"#
        );
        let parsed = import_provider_secure(source, None).unwrap();
        assert_eq!(
            parsed
                .candidate
                .base_url
                .as_ref()
                .map(|value| value.value.as_str()),
            Some("https://integrate.api.nvidia.com/v1")
        );
        assert_eq!(
            parsed
                .candidate
                .model_id
                .as_ref()
                .map(|value| value.value.as_str()),
            Some("minimaxai/minimax-m3")
        );
        assert_eq!(
            parsed.candidate.api_mode.as_ref().map(|value| value.value),
            Some(ApiMode::ChatCompletions)
        );
        assert!(!serde_json::to_string(&parsed.candidate)
            .unwrap()
            .contains(secret));
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

    #[test]
    fn secure_import_keeps_secret_only_in_non_serializable_zeroizing_state() {
        let secret = "nvapi-transient-only-123456";
        let source = format!(
            "from openai import OpenAI\nclient = OpenAI(base_url=\"https://integrate.api.nvidia.com/v1\", api_key=\"{secret}\")\nclient.chat.completions.create(model=\"test-model\")"
        );
        let mut parsed = import_provider_secure(source, Some(InputFormat::Python)).unwrap();

        let candidate_json = serde_json::to_string(&parsed.candidate).unwrap();
        assert!(!candidate_json.contains(secret));
        assert!(!format!("{parsed:?}").contains(secret));
        let transient = parsed
            .secret_state
            .transient_secrets()
            .unwrap()
            .single()
            .unwrap();
        assert_eq!(transient.expose_secret(), secret);
        assert!(parsed.validate_auth_resolved().is_err());

        parsed.secret_state.begin_storage_choice().unwrap();
        assert_eq!(
            parsed
                .secret_state
                .confirm_keychain_stored("imported-provider", false),
            Err(ImportError::ConfirmationRequired)
        );
        assert_eq!(
            parsed
                .secret_state
                .transient_secrets()
                .unwrap()
                .single()
                .unwrap()
                .expose_secret(),
            secret
        );
        parsed
            .secret_state
            .confirm_keychain_stored("imported-provider", true)
            .unwrap();
        parsed.validate_auth_resolved().unwrap();
        assert_eq!(
            parsed.secret_state.reference(),
            Some(SecretReference::Keychain(
                "keychain:imported-provider".into()
            ))
        );
        assert!(!format!("{parsed:?}").contains(secret));
    }

    #[test]
    fn every_supported_static_format_has_the_same_non_leaking_secret_property() {
        let secret = "format-static-secret-123456";
        let cases = [
            (
                InputFormat::Python,
                format!(
                    "from openai import OpenAI\nclient = OpenAI(base_url=\"https://example.com/v1\", api_key=\"{secret}\")\nclient.chat.completions.create(model=\"fixture\")"
                ),
            ),
            (
                InputFormat::JavaScript,
                format!(
                    "import OpenAI from \"openai\";\nconst client = new OpenAI({{ baseURL: \"https://example.com/v1\", apiKey: \"{secret}\" }});\nclient.chat.completions.create({{ model: \"fixture\" }});"
                ),
            ),
            (
                InputFormat::Curl,
                format!(
                    "curl https://example.com/v1/chat/completions -H \"Authorization: Bearer {secret}\" -d '{{\"model\":\"fixture\"}}'"
                ),
            ),
            (
                InputFormat::Dotenv,
                format!(
                    "BASE_URL=https://example.com/v1\nMODEL=fixture\nAPI_KEY={secret}"
                ),
            ),
            (
                InputFormat::Json,
                format!(
                    r#"{{"base_url":"https://example.com/v1","model":"fixture","api_key":"{secret}"}}"#
                ),
            ),
            (
                InputFormat::Yaml,
                format!(
                    "base_url: https://example.com/v1\nmodel: fixture\napi_key: {secret}"
                ),
            ),
            (
                InputFormat::Toml,
                format!(
                    "base_url = \"https://example.com/v1\"\nmodel = \"fixture\"\napi_key = \"{secret}\""
                ),
            ),
        ];

        for (format, source) in cases {
            let parsed = import_provider_secure(source, Some(format)).unwrap();
            let encoded = serde_json::to_string(&parsed.candidate).unwrap();
            assert!(!encoded.contains(secret), "{format:?}");
            assert!(!format!("{parsed:?}").contains(secret), "{format:?}");
            assert_eq!(
                parsed
                    .secret_state
                    .transient_secrets()
                    .unwrap()
                    .single()
                    .unwrap()
                    .expose_secret(),
                secret,
                "{format:?}"
            );
        }
    }

    #[test]
    fn environment_reference_is_resolved_without_a_transient_value() {
        let source = r#"from openai import OpenAI
client = OpenAI(
    base_url="https://api.openai.com/v1",
    api_key=os.getenv("OPENAI_API_KEY"),
)
client.responses.create(model="test-model")
"#;
        let parsed = import_provider_secure(source.into(), Some(InputFormat::Python)).unwrap();
        assert!(parsed.secret_state.transient_secrets().is_none());
        assert_eq!(
            parsed.secret_state.reference(),
            Some(SecretReference::Environment("OPENAI_API_KEY".into()))
        );
        parsed.validate_auth_resolved().unwrap();
    }

    #[test]
    fn environment_candidate_rejects_an_inconsistent_resolution_state() {
        let source = r#"from openai import OpenAI
client = OpenAI(base_url="https://api.openai.com/v1", api_key=os.getenv("OPENAI_API_KEY"))
client.responses.create(model="test-model")
"#;
        let mut parsed = import_provider_secure(source.into(), Some(InputFormat::Python)).unwrap();
        parsed.secret_state = ImportedSecretState::Discarded;
        assert!(parsed.validate_auth_resolved().is_err());
    }

    #[test]
    fn forged_durable_references_do_not_bypass_resolution_validation() {
        let source = "BASE_URL=https://example.com/v1\nMODEL=test\nAPI_KEY=valid-secret-value";
        let mut parsed = import_provider_secure(source.into(), Some(InputFormat::Dotenv)).unwrap();
        parsed.secret_state =
            ImportedSecretState::Stored(SecretReference::Keychain("plaintext-value".into()));
        assert!(parsed.validate_auth_resolved().is_err());

        parsed.secret_state = ImportedSecretState::EnvironmentReference("not-valid".into());
        assert!(parsed.validate_auth_resolved().is_err());
    }

    #[test]
    fn dotenv_literal_is_a_transient_secret_not_an_environment_reference() {
        let source = "OPENAI_BASE_URL=https://example.com/v1\nOPENAI_MODEL=test\nOPENAI_API_KEY=dotenv-secret-value";
        let parsed = import_provider_secure(source.into(), Some(InputFormat::Dotenv)).unwrap();
        assert!(matches!(
            parsed.candidate.auth.as_ref().map(|auth| &auth.value),
            Some(AuthReference::SecretDetected)
        ));
        assert_eq!(
            parsed
                .secret_state
                .transient_secrets()
                .unwrap()
                .single()
                .unwrap()
                .expose_secret(),
            "dotenv-secret-value"
        );
    }

    #[test]
    fn quoted_dotenv_secret_is_owned_whole_and_never_serialized() {
        let secret = "secret value with spaces; punctuation, included";
        let source = format!(
            "OPENAI_BASE_URL=https://example.com/v1\nOPENAI_MODEL=test\nOPENAI_API_KEY=\"{secret}\""
        );
        let parsed = import_provider_secure(source, Some(InputFormat::Dotenv)).unwrap();
        assert_eq!(
            parsed
                .secret_state
                .transient_secrets()
                .unwrap()
                .single()
                .unwrap()
                .expose_secret(),
            secret
        );
        assert!(!serde_json::to_string(&parsed.candidate)
            .unwrap()
            .contains(secret));
        assert!(!format!("{parsed:?}").contains(secret));
    }

    #[test]
    fn credentialed_url_sensitive_headers_and_extra_body_never_enter_candidate() {
        let url_username = "private-user-identifier";
        let secrets = [
            "url-password-value",
            "query-secret-value",
            "header-secret-value",
            "nested-secret-value",
            "basic-auth-value",
            "cookie-secret-value",
        ];
        let source = format!(
            r#"{{
                "base_url": "https://{}:{}@example.com/v1?api_key={}",
                "model": "fixture-model",
                "headers": {{
                    "X-API-Key": "{}",
                    "Authorization": "Basic {}",
                    "Cookie": "session={}",
                    "Content-Type": "application/json"
                }},
            "extra_body": {{"nested": {{"password": "{}"}}}}
            }}"#,
            url_username, secrets[0], secrets[1], secrets[2], secrets[4], secrets[5], secrets[3]
        );
        let parsed = import_provider_secure(source, Some(InputFormat::Json)).unwrap();
        let encoded = serde_json::to_string(&parsed.candidate).unwrap();
        let debug = format!("{parsed:?}");
        assert!(!encoded.contains(url_username));
        assert!(!debug.contains(url_username));
        for secret in secrets {
            assert!(!encoded.contains(secret));
            assert!(!debug.contains(secret));
        }
        let base = parsed.candidate.base_url.unwrap().value;
        assert_eq!(base, "https://example.com/v1");
        assert!(!parsed.candidate.custom_headers.contains_key("X-API-Key"));
        assert_eq!(
            parsed.candidate.custom_headers["Content-Type"].value,
            "application/json"
        );
        assert!(parsed.candidate.extra_body.is_none());
        assert!(parsed
            .candidate
            .warnings
            .iter()
            .any(|warning| warning.code == "url_credentials_removed"));
        assert!(parsed
            .candidate
            .warnings
            .iter()
            .any(|warning| warning.code == "sensitive_extra_body_omitted"));
        assert_eq!(parsed.candidate.redacted_source, REDACTION_TOKEN);
    }

    #[test]
    fn multiple_distinct_secrets_fail_closed_and_remain_available_for_review() {
        let source = r#"{
            "base_url": "https://example.com/v1",
            "model": "fixture",
            "api_key": "first-secret-value",
            "extra_body": {"password": "second-secret-value"}
        }"#;
        let mut parsed = import_provider_secure(source.into(), Some(InputFormat::Json)).unwrap();
        assert_eq!(parsed.secret_state.transient_secrets().unwrap().len(), 2);
        parsed.secret_state.begin_storage_choice().unwrap();
        assert!(parsed
            .secret_state
            .confirm_environment_reference("PROVIDER_API_KEY", true)
            .is_err());
        assert_eq!(parsed.secret_state.transient_secrets().unwrap().len(), 2);
        assert!(parsed.validate_auth_resolved().is_err());
    }

    #[test]
    fn repeated_identical_secret_has_one_transient_owner() {
        let source = r#"{
            "base_url": "https://example.com/v1",
            "model": "fixture",
            "api_key": "same-secret-value",
            "headers": {"Authorization": "Bearer same-secret-value"}
        }"#;
        let parsed = import_provider_secure(source.into(), Some(InputFormat::Json)).unwrap();
        let secrets = parsed.secret_state.transient_secrets().unwrap();
        assert_eq!(secrets.len(), 1);
        assert_eq!(
            secrets.single().unwrap().expose_secret(),
            "same-secret-value"
        );
    }

    #[test]
    fn invalid_environment_reference_does_not_consume_transient_secret() {
        let source = "BASE_URL=https://example.com/v1\nMODEL=test\nAPI_KEY=valid-secret-value";
        let mut parsed = import_provider_secure(source.into(), Some(InputFormat::Dotenv)).unwrap();
        parsed.secret_state.begin_storage_choice().unwrap();
        assert!(parsed
            .secret_state
            .confirm_environment_reference("not-valid", true)
            .is_err());
        assert_eq!(
            parsed
                .secret_state
                .transient_secrets()
                .unwrap()
                .single()
                .unwrap()
                .expose_secret(),
            "valid-secret-value"
        );
    }

    #[test]
    fn secure_import_enforces_the_exact_256_kib_boundary() {
        let prefix = "BASE_URL=https://example.com/v1\nMODEL=test\nAPI_KEY=boundary-secret\n#";
        let mut source = prefix.to_owned();
        source.push_str(&"x".repeat(DEFAULT_MAX_INPUT_BYTES - prefix.len()));
        let parsed = import_provider_secure(source.clone(), Some(InputFormat::Dotenv)).unwrap();
        assert_eq!(parsed.candidate.model_id.unwrap().value, "test");
        source.push('x');
        assert_eq!(
            import_provider_secure(source, Some(InputFormat::Dotenv)).unwrap_err(),
            ImportError::InputTooLarge {
                maximum_bytes: DEFAULT_MAX_INPUT_BYTES
            }
        );
    }

    #[test]
    fn malformed_errors_and_compatibility_candidates_never_echo_secret_values() {
        let secret = "sk-malformed-secret-123456";
        let source = format!(
            r#"{{"base_url":"https://example.com/v1","api_key":"{secret}","model":"x","broken":"#
        );
        let error = import_provider_secure(source.clone(), Some(InputFormat::Json)).unwrap_err();
        assert!(!error.to_string().contains(secret));
        let compatibility_error = import_provider(&source, Some(InputFormat::Json)).unwrap_err();
        assert!(!compatibility_error.to_string().contains(secret));
    }
}
