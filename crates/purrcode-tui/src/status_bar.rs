//! Status bar showing model, privacy mode, and mode indicator.

pub struct StatusBar {
    pub model: String,
    pub privacy: String,
    pub local: bool,
    pub mode_name: String,
    pub context_info: String,
}

impl Default for StatusBar {
    fn default() -> Self {
        Self::new()
    }
}

impl StatusBar {
    pub fn new() -> Self {
        Self {
            model: "none".into(),
            privacy: "local-only".into(),
            local: true,
            mode_name: "build".into(),
            context_info: String::new(),
        }
    }

    pub fn set_model(&mut self, model: &str) {
        self.model = model.to_string();
    }

    pub fn set_privacy(&mut self, privacy: &str) {
        self.privacy = privacy.to_string();
    }

    pub fn set_mode(&mut self, mode: &str) {
        self.mode_name = mode.to_string();
    }
}
