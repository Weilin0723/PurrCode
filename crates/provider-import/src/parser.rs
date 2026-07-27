use super::*;
use serde_json::{Map, Value};
use tree_sitter::{Language, Parser};
use url::Url;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InputFormat {
    Python,
    JavaScript,
    Curl,
    Dotenv,
    Json,
    Yaml,
    Toml,
}

pub(crate) fn parse_provider(
    input: &str,
    format_hint: Option<InputFormat>,
    secret_detected: bool,
) -> Result<ProviderImportCandidate, ImportError> {
    let format = format_hint.unwrap_or_else(|| detect_format(input));
    let mut candidate = empty_candidate(input, secret_detected);
    match format {
        InputFormat::Python => parse_syntax_source(
            input,
            tree_sitter_python::LANGUAGE.into(),
            "python",
            &mut candidate,
        )?,
        InputFormat::JavaScript => parse_syntax_source(
            input,
            tree_sitter_javascript::LANGUAGE.into(),
            "javascript",
            &mut candidate,
        )?,
        InputFormat::Curl => parse_curl(input, &mut candidate),
        InputFormat::Dotenv => parse_dotenv(input, &mut candidate),
        InputFormat::Json => {
            let value: Value =
                serde_json::from_str(input).map_err(|error| ImportError::Malformed {
                    format: "json".into(),
                    message: error.to_string(),
                })?;
            extract_structured(&value, input, &mut candidate);
        }
        InputFormat::Yaml => {
            let value: serde_yaml::Value =
                serde_yaml::from_str(input).map_err(|error| ImportError::Malformed {
                    format: "yaml".into(),
                    message: error.to_string(),
                })?;
            let value = serde_json::to_value(value).map_err(|error| ImportError::Malformed {
                format: "yaml".into(),
                message: error.to_string(),
            })?;
            extract_structured(&value, input, &mut candidate);
        }
        InputFormat::Toml => {
            let value: toml::Value =
                toml::from_str(input).map_err(|error| ImportError::Malformed {
                    format: "toml".into(),
                    message: error.to_string(),
                })?;
            let value = serde_json::to_value(value).map_err(|error| ImportError::Malformed {
                format: "toml".into(),
                message: error.to_string(),
            })?;
            extract_structured(&value, input, &mut candidate);
        }
    }
    sanitize_candidate(&mut candidate);
    finalize_candidate(&mut candidate);
    if candidate.base_url.is_none()
        && candidate.model_id.is_none()
        && candidate.auth.is_none()
        && candidate.provider_kind == ProviderKind::Unknown
    {
        return Err(ImportError::Unsupported);
    }
    Ok(candidate)
}

fn detect_format(input: &str) -> InputFormat {
    let trimmed = input.trim_start();
    let lower = trimmed.to_ascii_lowercase();
    if lower.starts_with("curl ") {
        return InputFormat::Curl;
    }
    if trimmed.starts_with('{') || trimmed.starts_with('[') {
        return InputFormat::Json;
    }
    if lower.contains("from openai import") || lower.contains("openai(") {
        return InputFormat::Python;
    }
    if lower.contains("new openai") || lower.contains("require(") || lower.contains("from 'openai'")
    {
        return InputFormat::JavaScript;
    }
    if input
        .lines()
        .any(|line| line.trim_start().starts_with("export "))
        || input.lines().all(|line| {
            line.trim().is_empty() || line.trim_start().starts_with('#') || line.contains('=')
        })
    {
        return InputFormat::Dotenv;
    }
    if input
        .lines()
        .any(|line| line.trim_start().starts_with('[') && line.trim_end().ends_with(']'))
    {
        return InputFormat::Toml;
    }
    InputFormat::Yaml
}

fn empty_candidate(input: &str, secret_detected: bool) -> ProviderImportCandidate {
    let auth = secret_detected
        .then(|| extracted(AuthReference::SecretDetected, Confidence::High, None, true));
    ProviderImportCandidate {
        provider_kind: ProviderKind::Unknown,
        suggested_name: "Imported provider".into(),
        base_url: None,
        model_id: None,
        auth,
        api_mode: None,
        defaults: BTreeMap::new(),
        custom_headers: BTreeMap::new(),
        extra_body: None,
        is_local: None,
        warnings: if secret_detected {
            vec![warning(
                "secret_redacted",
                "Secret-like source values were removed before review.",
            )]
        } else {
            Vec::new()
        },
        redacted_source: input.to_owned(),
    }
}

fn parse_syntax_source(
    input: &str,
    language: Language,
    format: &str,
    candidate: &mut ProviderImportCandidate,
) -> Result<(), ImportError> {
    let mut parser = Parser::new();
    parser
        .set_language(&language)
        .map_err(|error| ImportError::Malformed {
            format: format.into(),
            message: error.to_string(),
        })?;
    let tree = parser
        .parse(input, None)
        .ok_or_else(|| ImportError::Malformed {
            format: format.into(),
            message: "parser returned no syntax tree".into(),
        })?;
    if tree.root_node().has_error() {
        candidate.warnings.push(warning(
            "syntax_error",
            "The source contains syntax errors; only safe static literals were considered.",
        ));
    }
    let literal_spans = collect_literal_spans(tree.root_node());
    extract_static_literal(
        input,
        &["base_url", "baseURL"],
        &literal_spans,
        &mut candidate.base_url,
    );
    extract_static_literal(
        input,
        &["model", "model_id", "modelId"],
        &literal_spans,
        &mut candidate.model_id,
    );
    extract_environment_auth(input, candidate);
    for key in [
        "temperature",
        "top_p",
        "max_tokens",
        "seed",
        "stream",
        "timeout",
    ] {
        extract_default_literal(input, key, &literal_spans, candidate);
    }
    if input.contains("responses.create") || input.contains("responses.create(") {
        candidate.api_mode = Some(extracted(
            ApiMode::Responses,
            Confidence::High,
            find_span(input, "responses.create"),
            false,
        ));
    } else if input.contains("chat.completions") {
        candidate.api_mode = Some(extracted(
            ApiMode::ChatCompletions,
            Confidence::High,
            find_span(input, "chat.completions"),
            false,
        ));
    }
    if contains_dynamic_provider_field(input) {
        candidate.warnings.push(warning(
            "dynamic_expression",
            "Dynamic provider expressions were not evaluated and require manual confirmation.",
        ));
    }
    Ok(())
}

fn extract_static_literal(
    input: &str,
    keys: &[&str],
    literal_spans: &[SourceSpan],
    target: &mut Option<Extracted<String>>,
) {
    for key in keys {
        let pattern = format!(
            r#"(?m)\b{}\b\s*[:=]\s*[\"'](?P<value>[^\"']+)[\"']"#,
            regex::escape(key)
        );
        let regex = Regex::new(&pattern).expect("escaped static field regex");
        if let Some(value) = regex
            .captures(input)
            .and_then(|captures| captures.name("value"))
        {
            if !span_is_syntax_literal(value.start(), value.end(), literal_spans) {
                continue;
            }
            *target = Some(extracted(
                value.as_str().to_owned(),
                Confidence::High,
                Some(SourceSpan {
                    start: value.start(),
                    end: value.end(),
                }),
                false,
            ));
            return;
        }
    }
}

fn extract_environment_auth(input: &str, candidate: &mut ProviderImportCandidate) {
    let regex = Regex::new(r#"(?i)(?:api[_-]?key|apiKey|token)\s*[:=]\s*(?:os\.(?:getenv|environ\.get)\s*\(\s*[\"']|process\.env\.|\$\{?)(?P<name>[A-Z][A-Z0-9_]{2,})"#).unwrap();
    if let Some(name) = regex
        .captures(input)
        .and_then(|captures| captures.name("name"))
    {
        candidate.auth = Some(extracted(
            AuthReference::Environment(name.as_str().to_owned()),
            Confidence::High,
            Some(SourceSpan {
                start: name.start(),
                end: name.end(),
            }),
            false,
        ));
    }
}

fn extract_default_literal(
    input: &str,
    key: &str,
    literal_spans: &[SourceSpan],
    candidate: &mut ProviderImportCandidate,
) {
    let pattern = format!(
        r#"(?m)\b{}\b\s*[:=]\s*(?P<value>true|false|-?[0-9]+(?:\.[0-9]+)?)"#,
        regex::escape(key)
    );
    let regex = Regex::new(&pattern).unwrap();
    let Some(value) = regex
        .captures(input)
        .and_then(|captures| captures.name("value"))
    else {
        return;
    };
    if !span_is_syntax_literal(value.start(), value.end(), literal_spans) {
        return;
    }
    let parsed = match value.as_str() {
        "true" => Value::Bool(true),
        "false" => Value::Bool(false),
        number => number
            .parse::<i64>()
            .map(Value::from)
            .or_else(|_| number.parse::<f64>().map(Value::from))
            .unwrap_or(Value::String(number.into())),
    };
    candidate.defaults.insert(
        key.to_owned(),
        extracted(
            parsed,
            Confidence::High,
            Some(SourceSpan {
                start: value.start(),
                end: value.end(),
            }),
            false,
        ),
    );
}

fn collect_literal_spans(root: tree_sitter::Node<'_>) -> Vec<SourceSpan> {
    fn visit(node: tree_sitter::Node<'_>, output: &mut Vec<SourceSpan>) {
        if matches!(
            node.kind(),
            "string"
                | "string_content"
                | "string_fragment"
                | "integer"
                | "float"
                | "true"
                | "false"
        ) {
            output.push(SourceSpan {
                start: node.start_byte(),
                end: node.end_byte(),
            });
        }
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            visit(child, output);
        }
    }
    let mut output = Vec::new();
    visit(root, &mut output);
    output
}

fn span_is_syntax_literal(start: usize, end: usize, literals: &[SourceSpan]) -> bool {
    literals
        .iter()
        .any(|literal| start >= literal.start && end <= literal.end)
}

fn parse_dotenv(input: &str, candidate: &mut ProviderImportCandidate) {
    for line in input.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = line.strip_prefix("export ").unwrap_or(line);
        let Some((raw_key, raw_value)) = line.split_once('=') else {
            candidate.warnings.push(warning(
                "malformed_line",
                "A dotenv line without '=' was ignored.",
            ));
            continue;
        };
        let key = raw_key.trim().to_ascii_uppercase();
        let value = unquote(raw_value.trim());
        match key.as_str() {
            "OPENAI_BASE_URL" | "BASE_URL" | "API_BASE" => {
                candidate.base_url = Some(high_string(input, value))
            }
            "OPENAI_MODEL" | "MODEL" | "MODEL_ID" => {
                candidate.model_id = Some(high_string(input, value))
            }
            "OPENAI_API_KEY" | "API_KEY" | "TOKEN" => {
                candidate.auth = if let Some(variable) = dotenv_environment_reference(value) {
                    Some(extracted(
                        AuthReference::Environment(variable),
                        Confidence::High,
                        find_span(input, raw_key.trim()),
                        false,
                    ))
                } else {
                    Some(extracted(
                        AuthReference::SecretDetected,
                        Confidence::High,
                        find_span(input, value),
                        true,
                    ))
                };
            }
            _ => {}
        }
    }
}

fn parse_curl(input: &str, candidate: &mut ProviderImportCandidate) {
    let tokens = shell_like_tokens(input);
    let mut index = 0;
    while index < tokens.len() {
        match tokens[index].as_str() {
            "curl" | "-X" | "--request" => {}
            "-H" | "--header" => {
                if let Some(header) = tokens.get(index + 1) {
                    if let Some((name, value)) = header.split_once(':') {
                        if is_sensitive_header(name) || value.contains(REDACTION_TOKEN) {
                            candidate.auth = Some(extracted(
                                AuthReference::SecretDetected,
                                Confidence::High,
                                find_span(input, value.trim()),
                                true,
                            ));
                        } else {
                            candidate
                                .custom_headers
                                .insert(name.trim().to_owned(), high_string(input, value.trim()));
                        }
                    }
                    index += 1;
                }
            }
            "-d" | "--data" | "--data-raw" => {
                if let Some(body) = tokens.get(index + 1) {
                    if let Ok(value) = serde_json::from_str::<Value>(body) {
                        extract_structured(&value, input, candidate);
                    }
                    index += 1;
                }
            }
            token if token.starts_with("http://") || token.starts_with("https://") => {
                let base = endpoint_base(token);
                candidate.base_url = Some(high_string(input, &base));
                if token.contains("/chat/completions") {
                    candidate.api_mode = Some(extracted(
                        ApiMode::ChatCompletions,
                        Confidence::High,
                        find_span(input, token),
                        false,
                    ));
                }
                if token.contains("/responses") {
                    candidate.api_mode = Some(extracted(
                        ApiMode::Responses,
                        Confidence::High,
                        find_span(input, token),
                        false,
                    ));
                }
                if token.contains("/api/chat") {
                    candidate.api_mode = Some(extracted(
                        ApiMode::OllamaNative,
                        Confidence::High,
                        find_span(input, token),
                        false,
                    ));
                }
            }
            _ => {}
        }
        index += 1;
    }
}

fn extract_structured(value: &Value, input: &str, candidate: &mut ProviderImportCandidate) {
    let Some(object) = value.as_object() else {
        return;
    };
    visit_object(object, input, candidate);
}

fn visit_object(object: &Map<String, Value>, input: &str, candidate: &mut ProviderImportCandidate) {
    for (key, value) in object {
        let normalized = key.to_ascii_lowercase().replace('-', "_");
        match normalized.as_str() {
            "base_url" | "baseurl" | "api_base" | "endpoint" if value.is_string() => {
                candidate.base_url = Some(high_string(input, value.as_str().unwrap_or_default()))
            }
            "model" | "model_id" | "modelid" if value.is_string() => {
                candidate.model_id = Some(high_string(input, value.as_str().unwrap_or_default()))
            }
            "api_key_env" | "token_env" if value.is_string() => {
                candidate.auth = Some(extracted(
                    AuthReference::Environment(value.as_str().unwrap_or_default().to_owned()),
                    Confidence::High,
                    find_span(input, value.as_str().unwrap_or_default()),
                    false,
                ))
            }
            "api_key" | "token" | "authorization" if value.is_string() => {
                candidate.auth = Some(extracted(
                    AuthReference::SecretDetected,
                    Confidence::High,
                    find_span(input, value.as_str().unwrap_or_default()),
                    true,
                ))
            }
            "api_mode" | "mode" if value.is_string() => {
                let mode = match value
                    .as_str()
                    .unwrap_or_default()
                    .to_ascii_lowercase()
                    .replace('-', "_")
                    .as_str()
                {
                    "responses" => Some(ApiMode::Responses),
                    "chat_completions" | "openai_compatible" => Some(ApiMode::ChatCompletions),
                    "ollama_native" | "native" => Some(ApiMode::OllamaNative),
                    _ => None,
                };
                if let Some(mode) = mode {
                    candidate.api_mode = Some(extracted(mode, Confidence::High, None, false));
                }
            }
            "temperature" | "top_p" | "max_tokens" | "seed" | "stream" | "timeout" => {
                candidate.defaults.insert(
                    normalized,
                    extracted(value.clone(), Confidence::High, None, false),
                );
            }
            "extra_body" => {
                if contains_redaction(value) || contains_sensitive_key(value) {
                    candidate.warnings.push(warning(
                        "sensitive_extra_body_omitted",
                        "An extra-body value contained authentication material and was omitted.",
                    ));
                    candidate.auth = Some(extracted(
                        AuthReference::SecretDetected,
                        Confidence::High,
                        None,
                        true,
                    ));
                } else {
                    candidate.extra_body =
                        Some(extracted(value.clone(), Confidence::High, None, false));
                }
            }
            "headers" | "custom_headers" => {
                if let Some(headers) = value.as_object() {
                    for (name, value) in headers {
                        if let Some(value) = value.as_str() {
                            if is_sensitive_header(name) || value.contains(REDACTION_TOKEN) {
                                candidate.auth = Some(extracted(
                                    AuthReference::SecretDetected,
                                    Confidence::High,
                                    find_span(input, value),
                                    true,
                                ));
                            } else {
                                candidate
                                    .custom_headers
                                    .insert(name.clone(), high_string(input, value));
                            }
                        }
                    }
                }
            }
            _ => {}
        }
        if let Some(nested) = value.as_object() {
            visit_object(nested, input, candidate);
        }
    }
}

fn sanitize_candidate(candidate: &mut ProviderImportCandidate) {
    let Some(base) = candidate.base_url.as_mut() else {
        return;
    };
    let Ok(mut url) = Url::parse(&base.value) else {
        if base.value.contains(REDACTION_TOKEN) {
            candidate.base_url = None;
            candidate.warnings.push(warning(
                "credentialed_url_requires_review",
                "Credentials were removed from the URL; enter a credential-free base URL.",
            ));
        }
        return;
    };
    let had_userinfo = !url.username().is_empty() || url.password().is_some();
    let had_query_or_fragment = url.query().is_some() || url.fragment().is_some();
    if had_userinfo {
        let _ = url.set_username("");
        let _ = url.set_password(None);
        candidate.warnings.push(warning(
            "url_credentials_removed",
            "URL credentials were removed and must be resolved through a secret reference.",
        ));
        candidate.auth = Some(extracted(
            AuthReference::SecretDetected,
            Confidence::High,
            None,
            true,
        ));
    }
    if had_query_or_fragment {
        url.set_query(None);
        url.set_fragment(None);
        candidate.warnings.push(warning(
            "url_query_removed",
            "URL query and fragment components were removed from the provider base URL.",
        ));
    }
    base.value = url.to_string();
    base.requires_confirmation |= had_userinfo || had_query_or_fragment;
}

fn finalize_candidate(candidate: &mut ProviderImportCandidate) {
    let base = candidate
        .base_url
        .as_ref()
        .map(|value| value.value.to_ascii_lowercase())
        .unwrap_or_default();
    candidate.provider_kind =
        if base.contains("127.0.0.1:11434") || base.contains("localhost:11434") {
            ProviderKind::Ollama
        } else if base.contains("127.0.0.1:1234") || base.contains("localhost:1234") {
            ProviderKind::LmStudio
        } else if base.contains("api.openai.com") {
            ProviderKind::OpenAi
        } else if !base.is_empty() {
            ProviderKind::OpenAiCompatible
        } else {
            ProviderKind::Unknown
        };
    candidate.suggested_name = match candidate.provider_kind {
        ProviderKind::Ollama => "Ollama",
        ProviderKind::LmStudio => "LM Studio",
        ProviderKind::OpenAi => "OpenAI",
        ProviderKind::OpenAiCompatible if base.contains("nvidia") => "NVIDIA NIM",
        ProviderKind::OpenAiCompatible => "OpenAI-compatible",
        ProviderKind::Unknown => "Imported provider",
    }
    .into();
    if !base.is_empty() {
        let local =
            base.contains("localhost") || base.contains("127.0.0.1") || base.contains("[::1]");
        candidate.is_local = Some(extracted(
            local,
            Confidence::High,
            candidate
                .base_url
                .as_ref()
                .and_then(|value| value.source_span),
            false,
        ));
    }
    if candidate.api_mode.is_none() {
        candidate.api_mode = Some(extracted(ApiMode::Unknown, Confidence::Low, None, true));
    }
}

fn endpoint_base(url: &str) -> String {
    for suffix in ["/chat/completions", "/responses", "/models", "/api/chat"] {
        if let Some(base) = url.strip_suffix(suffix) {
            return base.to_owned();
        }
    }
    url.to_owned()
}

fn dotenv_environment_reference(value: &str) -> Option<String> {
    let value = value
        .strip_prefix("${")
        .and_then(|value| value.strip_suffix('}'))
        .or_else(|| value.strip_prefix('$'))?;
    validate_environment_reference(value)
        .ok()
        .map(|()| value.to_owned())
}

fn is_sensitive_header(name: &str) -> bool {
    let normalized = name.trim().to_ascii_lowercase().replace('_', "-");
    matches!(
        normalized.as_str(),
        "authorization"
            | "proxy-authorization"
            | "api-key"
            | "x-api-key"
            | "x-auth-token"
            | "x-access-token"
            | "cookie"
            | "set-cookie"
    ) || ["auth", "credential", "key", "password", "secret", "token"]
        .iter()
        .any(|marker| normalized.contains(marker))
}

fn contains_redaction(value: &Value) -> bool {
    match value {
        Value::String(value) => value.contains(REDACTION_TOKEN),
        Value::Array(values) => values.iter().any(contains_redaction),
        Value::Object(values) => values.values().any(contains_redaction),
        Value::Null | Value::Bool(_) | Value::Number(_) => false,
    }
}

fn contains_sensitive_key(value: &Value) -> bool {
    match value {
        Value::Array(values) => values.iter().any(contains_sensitive_key),
        Value::Object(values) => values
            .iter()
            .any(|(key, value)| is_sensitive_key(key) || contains_sensitive_key(value)),
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => false,
    }
}

fn is_sensitive_key(key: &str) -> bool {
    matches!(
        key.to_ascii_lowercase().replace('-', "_").as_str(),
        "api_key"
            | "token"
            | "access_token"
            | "refresh_token"
            | "auth_token"
            | "credential"
            | "client_secret"
            | "private_key"
            | "cookie"
            | "password"
            | "authorization"
    )
}

fn shell_like_tokens(input: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut quote = None;
    for character in input.chars() {
        if let Some(expected) = quote {
            if character == expected {
                quote = None;
            } else {
                current.push(character);
            }
        } else if character == '\'' || character == '"' {
            quote = Some(character);
        } else if character.is_whitespace() || character == '\\' {
            if !current.is_empty() {
                tokens.push(std::mem::take(&mut current));
            }
        } else {
            current.push(character);
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

fn unquote(value: &str) -> &str {
    value
        .strip_prefix('"')
        .and_then(|v| v.strip_suffix('"'))
        .or_else(|| value.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')))
        .unwrap_or(value)
}
fn contains_dynamic_provider_field(input: &str) -> bool {
    let regex =
        Regex::new(r#"(?m)\b(?:base_url|baseURL|model|model_id)\b\s*[:=]\s*(?P<value>[^\s,})]+)"#)
            .expect("static dynamic-field regex");
    let dynamic = regex.captures_iter(input).any(|captures| {
        captures
            .name("value")
            .is_some_and(|value| !value.as_str().starts_with(['\'', '"']))
    });
    dynamic
}
fn warning(code: &str, message: &str) -> ImportWarning {
    ImportWarning {
        code: code.into(),
        message: message.into(),
        source_span: None,
    }
}
fn extracted<T>(
    value: T,
    confidence: Confidence,
    source_span: Option<SourceSpan>,
    requires_confirmation: bool,
) -> Extracted<T> {
    Extracted {
        value,
        confidence,
        source_span,
        requires_confirmation,
    }
}
fn high_string(input: &str, value: &str) -> Extracted<String> {
    extracted(
        value.to_owned(),
        Confidence::High,
        find_span(input, value),
        false,
    )
}
fn find_span(input: &str, needle: &str) -> Option<SourceSpan> {
    input.find(needle).map(|start| SourceSpan {
        start,
        end: start + needle.len(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn python_ast_import_extracts_nvidia_profile_without_secret_leak() {
        let source = include_str!("../tests/fixtures/provider.py");
        let candidate = import_provider(source, Some(InputFormat::Python)).unwrap();
        assert_eq!(candidate.suggested_name, "NVIDIA NIM");
        assert_eq!(candidate.model_id.unwrap().value, "z-ai/glm-5.2");
        assert_eq!(candidate.defaults["max_tokens"].value, 16384);
        assert!(!candidate.redacted_source.contains("nvapi-fixture-secret"));
    }

    #[test]
    fn all_declarative_formats_normalize_the_same_core_fields() {
        for (format, source) in [
            (
                InputFormat::Json,
                include_str!("../tests/fixtures/provider.json"),
            ),
            (
                InputFormat::Yaml,
                include_str!("../tests/fixtures/provider.yaml"),
            ),
            (
                InputFormat::Toml,
                include_str!("../tests/fixtures/provider.toml"),
            ),
            (
                InputFormat::Dotenv,
                include_str!("../tests/fixtures/provider.env"),
            ),
        ] {
            let candidate = import_provider(source, Some(format)).unwrap();
            assert_eq!(
                candidate.base_url.unwrap().value,
                "http://127.0.0.1:11434/v1"
            );
            assert_eq!(candidate.model_id.unwrap().value, "qwen2.5-coder");
            assert_eq!(candidate.provider_kind, ProviderKind::Ollama);
        }
    }

    #[test]
    fn curl_and_javascript_are_parse_only_and_extract_static_literals() {
        let curl = include_str!("../tests/fixtures/provider.sh");
        let curl_candidate = import_provider(curl, Some(InputFormat::Curl)).unwrap();
        assert_eq!(curl_candidate.model_id.unwrap().value, "fixture-model");
        assert_eq!(
            curl_candidate.api_mode.unwrap().value,
            ApiMode::ChatCompletions
        );

        let js = include_str!("../tests/fixtures/provider.js");
        let js_candidate = import_provider(js, Some(InputFormat::JavaScript)).unwrap();
        assert_eq!(
            js_candidate.base_url.unwrap().value,
            "http://127.0.0.1:1234/v1"
        );
        assert_eq!(js_candidate.provider_kind, ProviderKind::LmStudio);
    }

    #[test]
    fn ollama_native_curl_is_distinct_from_openai_compatible_mode() {
        let source = r#"curl http://127.0.0.1:11434/api/chat -d '{"model":"qwen","stream":true}'"#;
        let candidate = import_provider(source, Some(InputFormat::Curl)).unwrap();
        assert_eq!(candidate.provider_kind, ProviderKind::Ollama);
        assert_eq!(candidate.base_url.unwrap().value, "http://127.0.0.1:11434/");
        assert_eq!(candidate.api_mode.unwrap().value, ApiMode::OllamaNative);
    }

    #[test]
    fn dotenv_variable_indirection_remains_an_environment_reference() {
        let source = "BASE_URL=https://example.com/v1\nMODEL=test\nAPI_KEY=$PROVIDER_API_KEY";
        let candidate = import_provider(source, Some(InputFormat::Dotenv)).unwrap();
        assert_eq!(
            candidate.auth.unwrap().value,
            AuthReference::Environment("PROVIDER_API_KEY".into())
        );
    }

    #[test]
    fn malformed_dynamic_source_returns_safe_partial_findings_and_warnings() {
        let source = include_str!("../tests/fixtures/malformed.txt");
        let candidate = import_provider(source, Some(InputFormat::Python)).unwrap();
        assert_eq!(candidate.base_url.unwrap().value, "https://example.com/v1");
        assert!(candidate.model_id.is_none());
        assert!(candidate
            .warnings
            .iter()
            .any(|warning| warning.code == "syntax_error"));
        assert!(candidate
            .warnings
            .iter()
            .any(|warning| warning.code == "dynamic_expression"));
    }
}
