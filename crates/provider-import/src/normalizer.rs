use super::*;
use purrcode_provider_gateway::ProviderConfig;
use std::collections::BTreeMap;
use url::Url;

#[derive(Clone, Debug)]
pub struct NormalizedProviderProfile {
    pub name: String,
    pub provider: ProviderConfig,
    pub model_id: Option<String>,
    pub api_mode: ApiMode,
    pub defaults: BTreeMap<String, serde_json::Value>,
}

pub fn normalize_candidate(
    candidate: &ProviderImportCandidate,
    confirmed_secret_environment: Option<&str>,
) -> Result<NormalizedProviderProfile, ImportError> {
    let base = candidate
        .base_url
        .as_ref()
        .ok_or_else(|| ImportError::Malformed {
            format: "provider profile".into(),
            message: "base URL requires confirmation".into(),
        })?;
    let base_url = Url::parse(&base.value).map_err(|error| ImportError::Malformed {
        format: "provider profile".into(),
        message: format!("invalid base URL: {error}"),
    })?;
    let auth_env = match candidate.auth.as_ref().map(|auth| &auth.value) {
        Some(AuthReference::Environment(name)) => Some(name.clone()),
        Some(AuthReference::SecretDetected) => confirmed_secret_environment.map(str::to_owned),
        Some(AuthReference::None) | None => None,
    };
    if candidate.auth.as_ref().is_some_and(|auth| {
        auth.value == AuthReference::SecretDetected && confirmed_secret_environment.is_none()
    }) {
        return Err(ImportError::Malformed {
            format: "provider profile".into(),
            message: "detected secret must be converted to a keychain or environment reference"
                .into(),
        });
    }
    let capabilities = BTreeMap::new();
    let provider = match candidate.provider_kind {
        ProviderKind::OpenAi => ProviderConfig::Openai {
            base_url,
            api_key_env: auth_env.unwrap_or_else(|| "OPENAI_API_KEY".into()),
            capabilities,
        },
        ProviderKind::Ollama => ProviderConfig::Ollama {
            base_url,
            capabilities,
        },
        ProviderKind::LmStudio | ProviderKind::OpenAiCompatible | ProviderKind::Unknown => {
            ProviderConfig::OpenaiCompatible {
                base_url,
                api_key_env: auth_env,
                local: candidate.is_local.as_ref().is_some_and(|value| value.value),
                headers: candidate
                    .custom_headers
                    .iter()
                    .map(|(key, value)| (key.clone(), value.value.clone()))
                    .collect(),
                capabilities,
            }
        }
    };
    Ok(NormalizedProviderProfile {
        name: candidate.suggested_name.clone(),
        provider,
        model_id: candidate.model_id.as_ref().map(|model| model.value.clone()),
        api_mode: candidate
            .api_mode
            .as_ref()
            .map_or(ApiMode::Unknown, |mode| mode.value),
        defaults: candidate
            .defaults
            .iter()
            .map(|(key, value)| (key.clone(), value.value.clone()))
            .collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalization_requires_secret_reference_and_reuses_gateway_types() {
        let source = include_str!("../tests/fixtures/provider.py");
        let candidate = import_provider(source, Some(InputFormat::Python)).unwrap();
        assert!(normalize_candidate(&candidate, None).is_err());
        let normalized = normalize_candidate(&candidate, Some("NVIDIA_API_KEY")).unwrap();
        assert_eq!(normalized.model_id.as_deref(), Some("z-ai/glm-5.2"));
        match normalized.provider {
            ProviderConfig::OpenaiCompatible {
                api_key_env, local, ..
            } => {
                assert_eq!(api_key_env.as_deref(), Some("NVIDIA_API_KEY"));
                assert!(!local);
            }
            _ => panic!("expected compatible provider"),
        }
    }
}
