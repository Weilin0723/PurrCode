use purrcode_alignment_fixture::ui::settings::{SettingsPanel, SAVE_BUTTON_LABEL};

fn main() {
    assert_eq!(SAVE_BUTTON_LABEL, "Submit");
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
        assert!(panel.reachable(key), "{key} is not reachable");
    }
    assert_eq!(panel.sidebar(), vec!["General", "Editor", "Network", "Models"]);

    let mut panel = SettingsPanel::new();
    panel.add_mcp_server("files", "purrcode-mcp-files");
    assert_eq!(panel.mcp_servers.len(), 1);
    panel.remove_mcp_server("files");
    assert!(panel.mcp_servers.is_empty());
}
