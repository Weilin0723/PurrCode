//! Provider discovery, manual setup, and script-import review state.

use purrcode_provider_import::{import_provider, ProviderImportCandidate, ProviderKind};
use zeroize::Zeroize;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProviderType {
    Ollama,
    LmStudio,
    Openai,
    OpenaiCompatible,
    EnterpriseGateway,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SetupScreen {
    Discovery,
    Form,
    ImportSource,
    ImportReview,
}

#[derive(Debug)]
pub struct ProviderSetup {
    pub screen: SetupScreen,
    pub selected: usize,
    pub active_field: usize,
    pub provider_type: Option<ProviderType>,
    pub profile_name: String,
    pub base_url: String,
    pub api_key: String,
    pub model_id: String,
    pub role: String,
    pub local: bool,
    pub complete: bool,
    pub test_result: Option<String>,
    pub error: Option<String>,
    pub discovered_models: Vec<String>,
    pub discovery_requested: bool,
    pub import_source: String,
    pub import_candidate: Option<ProviderImportCandidate>,
    pub editing_existing: bool,
}

impl Drop for ProviderSetup {
    fn drop(&mut self) {
        self.api_key.zeroize();
        self.import_source.zeroize();
    }
}

impl ProviderSetup {
    pub fn new() -> Self {
        Self {
            screen: SetupScreen::Discovery,
            selected: 0,
            active_field: 0,
            provider_type: None,
            profile_name: String::new(),
            base_url: String::new(),
            api_key: String::new(),
            model_id: String::new(),
            role: "coding_worker".into(),
            local: false,
            complete: false,
            test_result: None,
            error: None,
            discovered_models: Vec::new(),
            discovery_requested: false,
            import_source: String::new(),
            import_candidate: None,
            editing_existing: false,
        }
    }

    pub fn import_mode() -> Self {
        let mut setup = Self::new();
        setup.screen = SetupScreen::ImportSource;
        setup
    }

    pub fn from_saved(value: &serde_json::Value) -> Result<Self, String> {
        let configuration = value
            .get("configuration")
            .ok_or_else(|| "provider response omitted configuration".to_owned())?;
        let kind = configuration
            .get("type")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| "provider response omitted type".to_owned())?;
        let provider = match kind {
            "ollama" => ProviderType::Ollama,
            "openai" => ProviderType::Openai,
            "openai-compatible" => {
                let base = configuration
                    .get("base_url")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default();
                if base.contains(":1234") {
                    ProviderType::LmStudio
                } else {
                    ProviderType::OpenaiCompatible
                }
            }
            "enterprise-gateway" => ProviderType::EnterpriseGateway,
            other => return Err(format!("unsupported saved provider type `{other}`")),
        };
        let mut setup = Self::new();
        setup.select_provider(provider);
        setup.profile_name = value
            .get("name")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_owned();
        setup.base_url = configuration
            .get("base_url")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_owned();
        setup.discovered_models = value
            .get("models")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|model| model.as_str().map(str::to_owned))
            .collect();
        setup.model_id = setup.discovered_models.first().cloned().unwrap_or_default();
        setup.discovery_requested = false;
        setup.editing_existing = true;
        Ok(setup)
    }

    pub fn move_selection(&mut self, delta: isize) {
        self.selected = self.selected.saturating_add_signed(delta).min(5);
    }

    pub fn choose_selected(&mut self) {
        if self.selected == 5 {
            self.screen = SetupScreen::ImportSource;
            return;
        }
        let provider = match self.selected {
            0 => ProviderType::Ollama,
            1 => ProviderType::LmStudio,
            2 => ProviderType::Openai,
            3 => ProviderType::OpenaiCompatible,
            _ => ProviderType::EnterpriseGateway,
        };
        self.select_provider(provider);
    }

    pub fn select_provider(&mut self, provider: ProviderType) {
        self.provider_type = Some(provider);
        self.screen = SetupScreen::Form;
        self.active_field = 0;
        self.error = None;
        match provider {
            ProviderType::Ollama => {
                self.configure_defaults("ollama", "http://127.0.0.1:11434/v1", true, true)
            }
            ProviderType::LmStudio => {
                self.configure_defaults("lm-studio", "http://127.0.0.1:1234/v1", true, true)
            }
            ProviderType::Openai => {
                self.configure_defaults("openai", "https://api.openai.com/v1", false, false)
            }
            ProviderType::OpenaiCompatible => {
                self.configure_defaults("openai-compatible", "", false, false)
            }
            ProviderType::EnterpriseGateway => {
                self.configure_defaults("enterprise-gateway", "", false, false)
            }
        }
    }

    pub fn next_field(&mut self, backwards: bool) {
        self.active_field = if backwards {
            self.active_field.checked_sub(1).unwrap_or(4)
        } else {
            (self.active_field + 1) % 5
        };
    }

    pub fn edit_char(&mut self, character: char) {
        self.active_value_mut().push(character);
        self.error = None;
    }

    pub fn backspace(&mut self) {
        self.active_value_mut().pop();
        self.error = None;
    }

    pub fn insert_import(&mut self, source: &str) {
        self.import_source
            .push_str(&source.replace("\r\n", "\n").replace('\r', "\n"));
    }

    pub fn review_import(&mut self) {
        match import_provider(&self.import_source, None) {
            Ok(candidate) => {
                self.apply_candidate(candidate);
                self.import_source.zeroize();
                self.import_source.clear();
                self.screen = SetupScreen::ImportReview;
                self.active_field = 0;
                self.error = None;
            }
            Err(error) => self.error = Some(error.to_string()),
        }
    }

    pub fn request_test_and_save(&mut self) {
        if self.provider_type.is_none() {
            self.error = Some("Select or import a provider first".into());
        } else if self.profile_name.trim().is_empty() {
            self.error = Some("Profile name is required".into());
        } else if self.base_url.trim().is_empty() {
            self.error = Some("Base URL is required".into());
        } else if self.model_id.trim().is_empty() {
            self.error = Some("Select or enter a real model ID".into());
        } else {
            self.complete = true;
            self.error = None;
        }
    }

    fn configure_defaults(&mut self, name: &str, base_url: &str, local: bool, discover: bool) {
        self.profile_name = name.into();
        self.base_url = base_url.into();
        self.local = local;
        self.discovery_requested = discover;
    }

    fn apply_candidate(&mut self, candidate: ProviderImportCandidate) {
        self.provider_type = Some(match candidate.provider_kind {
            ProviderKind::OpenAi => ProviderType::Openai,
            ProviderKind::Ollama => ProviderType::Ollama,
            ProviderKind::LmStudio => ProviderType::LmStudio,
            ProviderKind::OpenAiCompatible | ProviderKind::Unknown => {
                ProviderType::OpenaiCompatible
            }
        });
        self.profile_name = slug(&candidate.suggested_name);
        self.base_url = candidate
            .base_url
            .as_ref()
            .map_or_else(String::new, |value| value.value.clone());
        self.model_id = candidate
            .model_id
            .as_ref()
            .map_or_else(String::new, |value| value.value.clone());
        self.local = candidate.is_local.as_ref().is_some_and(|value| value.value);
        self.import_candidate = Some(candidate);
    }

    fn active_value_mut(&mut self) -> &mut String {
        match self.active_field {
            0 => &mut self.profile_name,
            1 => &mut self.base_url,
            2 => &mut self.api_key,
            3 => &mut self.model_id,
            _ => &mut self.role,
        }
    }
}

fn slug(value: &str) -> String {
    let slug = value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>();
    slug.split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovery_selects_local_provider_and_requires_real_model() {
        let mut setup = ProviderSetup::new();
        setup.choose_selected();
        assert_eq!(setup.provider_type, Some(ProviderType::Ollama));
        assert!(setup.discovery_requested);
        setup.request_test_and_save();
        assert!(!setup.complete);
        setup.model_id = "qwen3-coder".into();
        setup.request_test_and_save();
        assert!(setup.complete);
    }

    #[test]
    fn pasted_script_becomes_an_editable_redacted_review() {
        let mut setup = ProviderSetup::import_mode();
        setup.insert_import(include_str!(
            "../../provider-import/tests/fixtures/provider.py"
        ));
        setup.review_import();
        assert_eq!(setup.screen, SetupScreen::ImportReview);
        assert_eq!(setup.profile_name, "nvidia-nim");
        assert_eq!(setup.model_id, "z-ai/glm-5.2");
        assert!(setup.import_source.is_empty());
        assert!(setup
            .import_candidate
            .as_ref()
            .is_some_and(|candidate| !candidate.redacted_source.contains("nvapi-fixture-secret")));
    }

    #[test]
    fn saved_profile_can_be_reopened_for_editing_without_a_secret_value() {
        let value = serde_json::json!({
            "name": "local",
            "configuration": {
                "type": "openai-compatible",
                "base_url": "http://127.0.0.1:1234/v1",
                "api_key_env": null,
                "local": true,
                "headers": {},
                "capabilities": {"model-a": {}}
            },
            "models": ["model-a"]
        });
        let setup = ProviderSetup::from_saved(&value).unwrap();
        assert_eq!(setup.provider_type, Some(ProviderType::LmStudio));
        assert_eq!(setup.model_id, "model-a");
        assert!(setup.api_key.is_empty());
    }
}
