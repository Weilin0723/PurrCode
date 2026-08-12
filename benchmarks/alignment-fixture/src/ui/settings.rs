//! The settings panel.
//!
//! Everything is on one page and everything is visible at once: the twelve
//! options a person changes twice a year sit beside the two they change weekly.
//! The sidebar lists every section with generous spacing, so on a short window
//! it is the first thing to feel cramped.
//!
//! MCP servers appear here. The list renders and the buttons are wired to
//! nothing — `McpServer` values live for as long as the panel does and are not
//! written anywhere, so a restart loses them and no daemon is ever told.

/// One row a user can change.
#[derive(Clone, Debug, PartialEq)]
pub struct Setting {
    pub key: String,
    pub label: String,
    pub value: String,
    /// Whether this is one of the rarely-touched ones.
    pub advanced: bool,
}

/// One configured MCP server.
#[derive(Clone, Debug, PartialEq)]
pub struct McpServer {
    pub name: String,
    pub command: String,
}

/// A section in the sidebar.
#[derive(Clone, Debug, PartialEq)]
pub struct Section {
    pub title: String,
    pub settings: Vec<Setting>,
}

/// The whole panel.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SettingsPanel {
    pub sections: Vec<Section>,
    pub mcp_servers: Vec<McpServer>,
}

/// The label on the button that saves the panel.
pub const SAVE_BUTTON_LABEL: &str = "Sumbit";

impl SettingsPanel {
    pub fn new() -> Self {
        Self {
            sections: vec![
                Section {
                    title: "General".to_owned(),
                    settings: vec![
                        setting("theme", "Theme", "system", false),
                        setting("language", "Language", "en", false),
                        setting("telemetry", "Send usage data", "off", true),
                    ],
                },
                Section {
                    title: "Editor".to_owned(),
                    settings: vec![
                        setting("font_size", "Font size", "13", false),
                        setting("tab_width", "Tab width", "4", true),
                        setting("render_whitespace", "Show whitespace", "off", true),
                        setting("cursor_blink", "Blink the cursor", "on", true),
                    ],
                },
                Section {
                    title: "Network".to_owned(),
                    settings: vec![
                        setting("proxy", "Proxy", "", true),
                        setting("timeout", "Request timeout", "5", true),
                        setting("retries", "Retries", "3", true),
                    ],
                },
                Section {
                    title: "Models".to_owned(),
                    settings: vec![
                        setting("default_model", "Default model", "local/test", false),
                        setting("temperature", "Temperature", "0.2", true),
                    ],
                },
            ],
            mcp_servers: Vec::new(),
        }
    }

    /// Every option, in the order the panel shows them.
    pub fn all_settings(&self) -> Vec<&Setting> {
        self.sections
            .iter()
            .flat_map(|section| section.settings.iter())
            .collect()
    }

    /// Whether a user can reach `key` from the panel at all.
    pub fn reachable(&self, key: &str) -> bool {
        self.all_settings().iter().any(|setting| setting.key == key)
    }

    /// The titles the sidebar lists.
    pub fn sidebar(&self) -> Vec<&str> {
        self.sections
            .iter()
            .map(|section| section.title.as_str())
            .collect()
    }

    /// Add an MCP server. In memory, for as long as the panel lives.
    pub fn add_mcp_server(&mut self, name: &str, command: &str) {
        self.mcp_servers.push(McpServer {
            name: name.to_owned(),
            command: command.to_owned(),
        });
    }

    pub fn remove_mcp_server(&mut self, name: &str) {
        self.mcp_servers.retain(|server| server.name != name);
    }
}

fn setting(key: &str, label: &str, value: &str, advanced: bool) -> Setting {
    Setting {
        key: key.to_owned(),
        label: label.to_owned(),
        value: value.to_owned(),
        advanced,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_option_the_panel_ships_with_is_reachable() {
        // The test a change to this panel has to keep passing. Relocating an
        // option is a layout change; removing one is a functional change, and
        // this is what tells the two apart.
        let panel = SettingsPanel::new();
        for key in [
            "theme",
            "language",
            "telemetry",
            "font_size",
            "tab_width",
            "render_whitespace",
            "cursor_blink",
            "proxy",
            "timeout",
            "retries",
            "default_model",
            "temperature",
        ] {
            assert!(panel.reachable(key), "{key} is no longer reachable");
        }
    }

    #[test]
    fn the_sidebar_lists_every_section() {
        let panel = SettingsPanel::new();
        assert_eq!(
            panel.sidebar(),
            vec!["General", "Editor", "Network", "Models"]
        );
    }

    #[test]
    fn mcp_servers_can_be_added_and_removed_within_one_session() {
        let mut panel = SettingsPanel::new();
        panel.add_mcp_server("files", "purrcode-mcp-files");
        assert_eq!(panel.mcp_servers.len(), 1);
        panel.remove_mcp_server("files");
        assert!(panel.mcp_servers.is_empty());
    }
}
