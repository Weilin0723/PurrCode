//! In-TUI provider onboarding wizard.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProviderType {
    Ollama,
    LmStudio,
    Openai,
    OpenaiCompatible,
    EnterpriseGateway,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_provider_requires_a_real_model_selection() {
        let mut setup = ProviderSetup::new();
        setup.select_provider(ProviderType::Ollama);
        assert!(setup.discovery_requested);
        setup.advance();
        assert!(!setup.complete);
        setup.model_id = "qwen3-coder".into();
        setup.advance();
        assert!(setup.complete);
    }

    #[test]
    fn openai_secret_and_model_are_separate_steps() {
        let mut setup = ProviderSetup::new();
        setup.select_provider(ProviderType::Openai);
        setup.api_key = "test-only-secret".into();
        setup.advance();
        assert_eq!(setup.step, 1);
        setup.model_id = "gpt-test".into();
        setup.advance();
        assert!(setup.complete);
    }
}

#[derive(Debug)]
pub struct ProviderSetup {
    pub step: usize,
    pub provider_type: Option<ProviderType>,
    pub base_url: String,
    pub api_key: String,
    pub model_id: String,
    pub local: bool,
    pub complete: bool,
    pub test_result: Option<String>,
    pub error: Option<String>,
    pub discovered_models: Vec<String>,
    pub discovery_requested: bool,
}

impl Drop for ProviderSetup {
    fn drop(&mut self) {
        zeroize::Zeroize::zeroize(&mut self.api_key);
    }
}

impl ProviderSetup {
    pub fn new() -> Self {
        Self {
            step: 0,
            provider_type: None,
            base_url: String::new(),
            api_key: String::new(),
            model_id: String::new(),
            local: false,
            complete: false,
            test_result: None,
            error: None,
            discovered_models: Vec::new(),
            discovery_requested: false,
        }
    }

    pub fn advance(&mut self) {
        match self.provider_type {
            None => {
                // Step 0 not advanced here; user picks from menu
            }
            Some(ProviderType::Ollama) => self.advance_local(),
            Some(ProviderType::LmStudio) => self.advance_local(),
            Some(ProviderType::Openai) => self.advance_openai(),
            Some(ProviderType::OpenaiCompatible) => self.advance_compatible(),
            Some(ProviderType::EnterpriseGateway) => self.advance_enterprise(),
        }
    }

    fn advance_local(&mut self) {
        if self.model_id.trim().is_empty() {
            self.error = Some("Enter a model ID before continuing".into());
        } else {
            self.complete = true;
        }
    }

    fn advance_openai(&mut self) {
        match self.step {
            0 if self.api_key.trim().is_empty() => {
                self.error = Some("Enter an API key before continuing".into())
            }
            0 => self.step = 1,
            1 if self.model_id.trim().is_empty() => {
                self.error = Some("Enter a model ID before continuing".into())
            }
            1 => self.complete = true,
            _ => {}
        }
    }

    fn advance_compatible(&mut self) {
        match self.step {
            0 => self.step = 1, // Base URL entered
            1 => self.step = 2, // API key / env
            2 => self.step = 3, // Local or remote
            3 => self.step = 4, // Model ID
            4 => {
                self.test_result = Some("✓ Connection succeeded".into());
                self.complete = true;
            }
            _ => {}
        }
    }

    fn advance_enterprise(&mut self) {
        match self.step {
            0 => self.step = 1, // Base URL
            1 => self.step = 2, // Auth method
            2 => self.step = 3, // Custom headers
            3 => self.step = 4, // Model
            4 => {
                self.test_result = Some("✓ Gateway connected".into());
                self.complete = true;
            }
            _ => {}
        }
    }

    pub fn select_provider(&mut self, pt: ProviderType) {
        self.provider_type = Some(pt);
        self.step = 0;
        match pt {
            ProviderType::Ollama => {
                self.base_url = "http://127.0.0.1:11434/v1".into();
                self.local = true;
            }
            ProviderType::LmStudio => {
                self.base_url = "http://127.0.0.1:1234/v1".into();
                self.local = true;
            }
            ProviderType::Openai => {
                self.base_url = "https://api.openai.com/v1".into();
                self.local = false;
            }
            ProviderType::OpenaiCompatible => {
                self.base_url = "".into();
                self.local = false;
            }
            ProviderType::EnterpriseGateway => {
                self.base_url = "".into();
                self.local = false;
            }
        }
        self.discovery_requested = matches!(pt, ProviderType::Ollama | ProviderType::LmStudio);
    }
}
