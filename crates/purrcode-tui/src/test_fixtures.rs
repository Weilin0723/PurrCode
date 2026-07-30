use ratatui::backend::TestBackend;
use ratatui::layout::Rect;
use ratatui::Terminal;

use crate::theme::{Palette, Theme};

/// An application state wired to a daemon URL that nothing is listening on.
///
/// Component and dispatcher tests use this so they exercise real code paths
/// without a live daemon: requests fail fast with a connection error, which is
/// itself a state the interface must present understandably.
#[cfg(test)]
pub(crate) fn offline_app() -> crate::app::App {
    crate::app::App {
        config: crate::app::TuiConfig {
            daemon_url: "http://127.0.0.1:1".into(),
            token_file: std::path::PathBuf::from("/nonexistent/token"),
            repository: std::path::PathBuf::from("/tmp"),
        },
        client: reqwest::Client::new(),
        token: String::new(),
        mode: crate::app::AppMode::Conversation,
        conversation: crate::conversation::Conversation::new(),
        composer: crate::composer::Composer::new(),
        secret_review: None,
        status_bar: crate::status_bar::StatusBar::new(),
        workspace: crate::workspace::WorkspaceContext::inspect(std::path::Path::new("/tmp")),
        provider_setup: None,
        skill_browser: None,
        diff_view: None,
        stream: crate::streaming::StreamController::new(),
        reconciliation: None,
        last_refresh: std::time::Instant::now(),
        message_bar: String::new(),
        session_id: None,
        session_choice: None,
        session_read_only: false,
        has_provider: false,
        trace_inspector_visible: false,
        trace_event_index: 0,
        trace_event_type: String::new(),
        trace_event_detail: Vec::new(),
        trace_total_events: 0,
        pending_command: None,
        pending_user_message: false,
        running_command: false,
        quit_requested: false,
        downloaded_skill: None,
        pending_skill_install_action: None,
        pending_research: None,
        pending_skill_download: None,
        pending_model_pull: None,
        active_pull_action: None,
        active_pull_session: None,
        theme: test_theme(),
        focus: crate::layout::Focus::Composer,
        inspector_open: false,
        activity_selected: None,
        approval: None,
        approval_dismissed: None,
        review: None,
        review_show_files: true,
        review_validation_expanded: false,
        review_needs_refresh: false,
        palette_query: String::new(),
        palette_selected: 0,
        model_choices: Vec::new(),
        model_selected: 0,
    }
}

pub fn test_terminal(width: u16, height: u16) -> Terminal<TestBackend> {
    let backend = TestBackend::new(width, height);
    Terminal::new(backend).expect("test terminal creation must succeed")
}

pub mod dimensions {
    pub const W60: (u16, u16) = (60, 24);
    pub const W80: (u16, u16) = (80, 24);
    pub const W120: (u16, u16) = (120, 24);
    pub const W160: (u16, u16) = (160, 24);
}

pub fn test_theme() -> Theme {
    Theme {
        palette: Palette::dark(),
        colors_enabled: true,
        unicode_enabled: true,
    }
}

pub fn monochrome_theme() -> Theme {
    Theme {
        palette: Palette::monochrome(),
        colors_enabled: false,
        unicode_enabled: false,
    }
}

/// Extract the rendered text from a TestBackend buffer.
pub fn render_snapshot(width: u16, height: u16) -> String {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).expect("test terminal creation must succeed");
    terminal
        .draw(|frame| {
            let area = Rect::new(0, 0, width, height);
            let theme = test_theme();
            crate::status_header::render_status_header(
                frame,
                area,
                &theme,
                "0.7.0",
                "owner/repo",
                "coder:7b",
                "seatbelt",
                "abc12345",
                "phase",
                "Plan",
                "🔒",
                "local",
            );
        })
        .expect("status header rendering must succeed");
    terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_terminal_at_60_columns() {
        let terminal = test_terminal(dimensions::W60.0, dimensions::W60.1);
        assert_eq!(terminal.size().unwrap().width, 60);
    }

    #[test]
    fn creates_terminal_at_80_columns() {
        let terminal = test_terminal(dimensions::W80.0, dimensions::W80.1);
        assert_eq!(terminal.size().unwrap().width, 80);
    }

    #[test]
    fn creates_terminal_at_120_columns() {
        let terminal = test_terminal(dimensions::W120.0, dimensions::W120.1);
        assert_eq!(terminal.size().unwrap().width, 120);
    }

    #[test]
    fn creates_terminal_at_160_columns() {
        let terminal = test_terminal(dimensions::W160.0, dimensions::W160.1);
        assert_eq!(terminal.size().unwrap().width, 160);
    }

    #[test]
    fn test_theme_is_dark() {
        let theme = test_theme();
        assert!(theme.colors_enabled);
        assert!(theme.unicode_enabled);
        assert_eq!(theme.palette.accent, Palette::dark().accent);
    }

    #[test]
    fn monochrome_theme_disables_color_and_unicode() {
        let theme = monochrome_theme();
        assert!(!theme.colors_enabled);
        assert!(!theme.unicode_enabled);
    }

    // ── visual snapshot tests at the four supported widths ──────────────

    #[test]
    fn status_header_snapshot_at_60_columns_is_stable() {
        let snapshot = render_snapshot(dimensions::W60.0, dimensions::W60.1);
        assert!(
            snapshot.contains("PurrCode"),
            "narrow header must include brand"
        );
        assert!(
            snapshot.contains("owner/repo"),
            "narrow header must include repo"
        );
        assert!(
            snapshot.contains("coder:7b"),
            "narrow header must include model"
        );
    }

    #[test]
    fn status_header_snapshot_at_80_columns_is_stable() {
        let snapshot = render_snapshot(dimensions::W80.0, dimensions::W80.1);
        assert!(
            snapshot.contains("PurrCode"),
            "compact header must include brand"
        );
        assert!(
            snapshot.contains("owner/repo"),
            "compact header must include repo"
        );
        assert!(
            snapshot.contains("coder:7b"),
            "compact header must include model"
        );
    }

    #[test]
    fn status_header_snapshot_at_120_columns_is_stable() {
        let snapshot = render_snapshot(dimensions::W120.0, dimensions::W120.1);
        assert!(snapshot.contains("PurrCode"));
        assert!(snapshot.contains("owner/repo"));
        assert!(snapshot.contains("coder:7b"));
        assert!(
            snapshot.contains("seatbelt"),
            "wide header must include sandbox"
        );
        assert!(snapshot.contains("Plan"), "wide header must include mode");
    }

    #[test]
    fn status_header_snapshot_at_160_columns_is_stable() {
        let snapshot = render_snapshot(dimensions::W160.0, dimensions::W160.1);
        assert!(snapshot.contains("PurrCode"));
        assert!(snapshot.contains("owner/repo"));
        assert!(snapshot.contains("coder:7b"));
        assert!(
            snapshot.contains("seatbelt"),
            "wide header must include sandbox"
        );
        assert!(snapshot.contains("Plan"), "wide header must include mode");
        assert!(snapshot.contains("phase"), "wide header must include phase");
    }

    #[test]
    fn status_header_widths_produce_deterministic_layouts() {
        // Confirm that all four widths render successfully and produce
        // non-empty snapshots. The exact text is theme-dependent, but the
        // rendered frames must contain the brand mark in every width.
        for (w, h) in [
            dimensions::W60,
            dimensions::W80,
            dimensions::W120,
            dimensions::W160,
        ] {
            let snapshot = render_snapshot(w, h);
            assert!(
                !snapshot.is_empty(),
                "snapshot at {w}×{h} must not be empty"
            );
            assert!(
                snapshot.contains("PurrCode"),
                "snapshot at {w}×{h} must contain brand"
            );
        }
    }
}
