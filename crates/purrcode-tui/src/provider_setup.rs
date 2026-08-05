//! Provider discovery, manual setup, and script-import review state.

use purrcode_provider_import::{
    DEFAULT_MAX_INPUT_BYTES, ImportedSecretState, ParsedProviderImport, ProviderImportCandidate,
    ProviderKind, SecretReference, import_provider_secure,
};
use zeroize::Zeroize;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProviderType {
    Ollama,
    LmStudio,
    Openai,
    OpenaiCompatible,
    NvidiaNim,
    EnterpriseGateway,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SetupScreen {
    Discovery,
    Form,
    ImportSource,
    ImportAuthChoice,
    ImportKeychainConfirm,
    ImportEnvironment,
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
    pub secure_import: Option<ParsedProviderImport>,
    pub import_auth_choice: usize,
    pub environment_reference: String,
    pub keychain_storage_confirmed: bool,
    pub editing_existing: bool,
    preserved_credential_reference: Option<SecretReference>,
}

impl Drop for ProviderSetup {
    fn drop(&mut self) {
        self.api_key.zeroize();
        self.import_source.zeroize();
    }
}

impl Default for ProviderSetup {
    fn default() -> Self {
        Self::new()
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
            secure_import: None,
            import_auth_choice: 0,
            environment_reference: String::new(),
            keychain_storage_confirmed: false,
            editing_existing: false,
            preserved_credential_reference: None,
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
        setup.preserved_credential_reference = configuration
            .get("api_key_env")
            .and_then(serde_json::Value::as_str)
            .map(|reference| {
                if reference.starts_with("keychain:") {
                    SecretReference::Keychain(reference.to_owned())
                } else {
                    SecretReference::Environment(reference.to_owned())
                }
            });
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
                self.configure_defaults("ollama", "http://127.0.0.1:11434", true, true)
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
            ProviderType::NvidiaNim => self.configure_defaults(
                "nvidia-nim",
                "https://integrate.api.nvidia.com/v1",
                false,
                false,
            ),
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

    pub fn insert_active_paste(&mut self, content: &str) {
        self.active_value_mut().push_str(content);
        self.error = None;
    }

    pub fn backspace(&mut self) {
        self.active_value_mut().pop();
        self.error = None;
    }

    pub fn clear_active_field(&mut self) {
        self.active_value_mut().zeroize();
        self.error = None;
    }

    pub fn cycle_discovered_model(&mut self, delta: isize) {
        if self.active_field != 3 || self.discovered_models.is_empty() {
            return;
        }
        let current = self
            .discovered_models
            .iter()
            .position(|model| model == &self.model_id)
            .unwrap_or(0);
        let last = self.discovered_models.len() - 1;
        let next = current.saturating_add_signed(delta).min(last);
        self.model_id = self.discovered_models[next].clone();
        self.error = None;
    }

    pub fn insert_import(&mut self, source: &str) {
        if self.import_source.len().saturating_add(source.len()) > DEFAULT_MAX_INPUT_BYTES {
            self.error = Some(format!(
                "Provider import is limited to {DEFAULT_MAX_INPUT_BYTES} bytes"
            ));
            return;
        }
        // Provider import preserves the exact pasted bytes, including original newline style.
        self.import_source.push_str(source);
        self.error = None;
    }

    pub fn review_import(&mut self) {
        let source = std::mem::take(&mut self.import_source);
        match import_provider_secure(source, None) {
            Ok(mut parsed) => {
                let candidate = parsed.candidate.clone();
                self.apply_candidate(candidate);
                let requires_choice = matches!(
                    parsed.secret_state,
                    ImportedSecretState::DetectedTransient(_)
                );
                if requires_choice {
                    if let Err(error) = parsed.secret_state.begin_storage_choice() {
                        self.error = Some(error.to_string());
                        return;
                    }
                } else {
                    self.environment_reference = parsed
                        .secret_state
                        .reference()
                        .and_then(|reference| match reference {
                            SecretReference::Environment(variable) => Some(variable),
                            SecretReference::Keychain(_) => None,
                        })
                        .unwrap_or_default();
                }
                self.secure_import = Some(parsed);
                self.screen = if requires_choice {
                    SetupScreen::ImportAuthChoice
                } else {
                    SetupScreen::ImportReview
                };
                self.active_field = 0;
                self.error = None;
            }
            Err(error) => {
                self.error = Some(error.to_string());
            }
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
        } else if !self.import_auth_is_resolved() {
            self.error = Some(
                "Authentication is unresolved. Confirm credential storage, choose an environment reference, or enter another API key.".into(),
            );
        } else {
            self.complete = true;
            self.error = None;
        }
    }

    pub fn move_import_auth_choice(&mut self, delta: isize) {
        self.import_auth_choice = self.import_auth_choice.saturating_add_signed(delta).min(2);
    }

    pub fn choose_import_auth(&mut self) {
        match self.import_auth_choice {
            0 => {
                self.screen = SetupScreen::ImportKeychainConfirm;
                self.error = None;
            }
            1 => {
                if self.environment_reference.is_empty() {
                    self.environment_reference =
                        suggested_environment_reference(&self.profile_name);
                }
                self.screen = SetupScreen::ImportEnvironment;
                self.error = None;
            }
            _ => {
                if let Some(parsed) = &mut self.secure_import {
                    parsed.secret_state.discard();
                }
                self.keychain_storage_confirmed = false;
                self.environment_reference.clear();
                self.screen = SetupScreen::ImportReview;
                self.active_field = 2;
                self.error =
                    Some("Detected secret discarded. Enter another API key before saving.".into());
            }
        }
    }

    pub fn confirm_keychain_choice(&mut self, confirmed: bool) {
        if confirmed {
            self.keychain_storage_confirmed = true;
            self.screen = SetupScreen::ImportReview;
            self.active_field = 0;
            self.error = None;
        } else {
            self.keychain_storage_confirmed = false;
            self.screen = SetupScreen::ImportAuthChoice;
        }
    }

    pub fn edit_environment_reference(&mut self, character: char) {
        if self.environment_reference.len() < 128 {
            self.environment_reference.push(character);
        }
        self.error = None;
    }

    pub fn insert_environment_reference(&mut self, content: &str) {
        if self
            .environment_reference
            .len()
            .saturating_add(content.len())
            <= 128
        {
            self.environment_reference.push_str(content);
            self.error = None;
        } else {
            self.error = Some("Environment reference is limited to 128 bytes".into());
        }
    }

    pub fn backspace_environment_reference(&mut self) {
        self.environment_reference.pop();
        self.error = None;
    }

    pub fn confirm_environment_reference(&mut self) {
        let Some(parsed) = &mut self.secure_import else {
            self.error = Some("No secure provider import is active".into());
            return;
        };
        match parsed
            .secret_state
            .confirm_environment_reference(self.environment_reference.trim(), true)
        {
            Ok(()) => {
                self.keychain_storage_confirmed = false;
                self.screen = SetupScreen::ImportReview;
                self.active_field = 0;
                self.error = None;
            }
            Err(error) => self.error = Some(error.to_string()),
        }
    }

    pub fn pending_keychain_secret(&self) -> Option<&str> {
        if !self.keychain_storage_confirmed {
            return None;
        }
        self.secure_import
            .as_ref()?
            .secret_state
            .transient_secrets()?
            .single()
            .ok()
            .map(|secret| secret.expose_secret())
    }

    pub fn confirm_keychain_stored(&mut self) -> Result<(), String> {
        let parsed = self
            .secure_import
            .as_mut()
            .ok_or_else(|| "No secure provider import is active".to_owned())?;
        parsed
            .secret_state
            .confirm_keychain_stored(self.profile_name.trim(), true)
            .map_err(|error| error.to_string())?;
        self.keychain_storage_confirmed = false;
        Ok(())
    }

    pub fn credential_reference(&self) -> Option<SecretReference> {
        self.secure_import
            .as_ref()
            .and_then(|parsed| parsed.secret_state.reference())
            .or_else(|| self.preserved_credential_reference.clone())
    }

    pub fn auth_status(&self) -> &'static str {
        if !self.api_key.is_empty() {
            return "new secret ready for confirmed credential storage";
        }
        if self.keychain_storage_confirmed {
            return "detected secret ready for confirmed credential storage";
        }
        match self
            .secure_import
            .as_ref()
            .map(|parsed| &parsed.secret_state)
        {
            Some(ImportedSecretState::Stored(_)) => "stored in credentials.toml",
            Some(ImportedSecretState::EnvironmentReference(_)) => "environment reference selected",
            Some(
                ImportedSecretState::DetectedTransient(_)
                | ImportedSecretState::AwaitingStorageChoice(_),
            ) => "detected secret awaiting storage choice",
            Some(ImportedSecretState::Discarded) => "detected secret discarded",
            Some(ImportedSecretState::None) => "not required",
            None if matches!(
                self.preserved_credential_reference,
                Some(SecretReference::Keychain(_))
            ) =>
            {
                "stored in credentials.toml"
            }
            None if matches!(
                self.preserved_credential_reference,
                Some(SecretReference::Environment(_))
            ) =>
            {
                "environment reference selected"
            }
            None if self.provider_type == Some(ProviderType::Openai) => "not set (required)",
            None => "not required or not set",
        }
    }

    fn import_auth_is_resolved(&self) -> bool {
        if !self.api_key.is_empty() || self.keychain_storage_confirmed {
            return true;
        }
        if self.secure_import.is_none() && self.provider_type == Some(ProviderType::Openai) {
            return false;
        }
        self.secure_import
            .as_ref()
            .is_none_or(|parsed| parsed.validate_auth_resolved().is_ok())
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
            ProviderKind::NvidiaNim => ProviderType::NvidiaNim,
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

fn suggested_environment_reference(profile_name: &str) -> String {
    let mut variable = profile_name
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect::<String>();
    while variable.contains("__") {
        variable = variable.replace("__", "_");
    }
    let variable = variable.trim_matches('_');
    if variable.is_empty() {
        "PROVIDER_API_KEY".into()
    } else {
        format!("{variable}_API_KEY")
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
    fn manual_openai_setup_never_treats_missing_auth_as_complete() {
        let mut setup = ProviderSetup::new();
        setup.select_provider(ProviderType::Openai);
        setup.model_id = "gpt-test".into();
        setup.request_test_and_save();
        assert!(!setup.complete);
        assert_eq!(setup.auth_status(), "not set (required)");
        setup.api_key = "replacement-secret".into();
        setup.request_test_and_save();
        assert!(setup.complete);
    }

    #[test]
    fn provider_form_can_clear_fields_and_choose_a_discovered_model() {
        let mut setup = ProviderSetup::new();
        setup.select_provider(ProviderType::OpenaiCompatible);
        setup.active_field = 1;
        setup.base_url = "wrong endpoint".into();
        setup.clear_active_field();
        assert!(setup.base_url.is_empty());

        setup.active_field = 3;
        setup.discovered_models = vec!["small".into(), "large".into()];
        setup.model_id = "small".into();
        setup.cycle_discovered_model(1);
        assert_eq!(setup.model_id, "large");
        setup.cycle_discovered_model(-1);
        assert_eq!(setup.model_id, "small");
    }

    #[test]
    fn pasted_script_becomes_an_editable_redacted_review() {
        let mut setup = ProviderSetup::import_mode();
        setup.insert_import(include_str!(
            "../../provider-import/tests/fixtures/provider.py"
        ));
        setup.review_import();
        assert_eq!(setup.screen, SetupScreen::ImportAuthChoice);
        assert_eq!(setup.profile_name, "nvidia-nim");
        assert_eq!(setup.model_id, "z-ai/glm-5.2");
        assert!(setup.import_source.is_empty());
        assert!(
            setup
                .import_candidate
                .as_ref()
                .is_some_and(|candidate| !candidate
                    .redacted_source
                    .contains("nvapi-fixture-secret"))
        );
        setup.request_test_and_save();
        assert!(!setup.complete);
        setup.choose_import_auth();
        assert_eq!(setup.screen, SetupScreen::ImportKeychainConfirm);
        setup.confirm_keychain_choice(true);
        setup.request_test_and_save();
        assert!(setup.complete);
        assert!(setup.pending_keychain_secret().is_some());
        setup.confirm_keychain_stored().unwrap();
        assert_eq!(
            setup.credential_reference(),
            Some(SecretReference::Keychain("keychain:nvidia-nim".into()))
        );
        assert!(setup.pending_keychain_secret().is_none());
    }

    #[test]
    fn environment_and_discard_choices_have_truthful_auth_state() {
        let source =
            "BASE_URL=https://example.com/v1\nMODEL=test\nAPI_KEY=static-secret-value".to_owned();
        let mut environment = ProviderSetup::import_mode();
        environment.insert_import(&source);
        environment.review_import();
        environment.import_auth_choice = 1;
        environment.choose_import_auth();
        assert_eq!(environment.screen, SetupScreen::ImportEnvironment);
        environment.environment_reference = "EXAMPLE_API_KEY".into();
        environment.confirm_environment_reference();
        environment.request_test_and_save();
        assert!(environment.complete);
        assert_eq!(
            environment.credential_reference(),
            Some(SecretReference::Environment("EXAMPLE_API_KEY".into()))
        );

        let mut discarded = ProviderSetup::import_mode();
        discarded.insert_import(&source);
        discarded.review_import();
        discarded.import_auth_choice = 2;
        discarded.choose_import_auth();
        discarded.request_test_and_save();
        assert!(!discarded.complete);
        discarded.api_key = "replacement-secret".into();
        discarded.request_test_and_save();
        assert!(discarded.complete);
    }

    #[test]
    fn import_surface_preserves_newlines_and_rejects_oversized_paste_atomically() {
        let mut setup = ProviderSetup::import_mode();
        setup.insert_import("first\r\n  second\rthird");
        assert_eq!(setup.import_source, "first\r\n  second\rthird");
        let before = setup.import_source.clone();
        setup.insert_import(&"x".repeat(DEFAULT_MAX_INPUT_BYTES));
        assert_eq!(setup.import_source, before);
        assert!(
            setup
                .error
                .as_deref()
                .is_some_and(|error| error.contains("limited"))
        );
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

    #[test]
    fn saved_keychain_profile_reuses_its_credential_reference() {
        let value = serde_json::json!({
            "name": "nvidia-nim",
            "configuration": {
                "type": "openai-compatible",
                "base_url": "https://integrate.api.nvidia.com/v1",
                "api_key_env": "keychain:nvidia-nim",
                "local": false,
                "headers": {},
                "capabilities": {"z-ai/glm-5.2": {}}
            },
            "models": ["z-ai/glm-5.2"]
        });
        let setup = ProviderSetup::from_saved(&value).unwrap();
        assert_eq!(setup.auth_status(), "stored in credentials.toml");
        assert_eq!(
            setup.credential_reference(),
            Some(SecretReference::Keychain("keychain:nvidia-nim".into()))
        );
    }
}
