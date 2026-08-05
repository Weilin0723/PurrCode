//! The start screen: choose a folder before anything else happens.
//!
//! PurrCode is scoped to a repository. Every session, every worktree, every
//! terminal and every diff is relative to one, so "which folder?" is the first
//! question the application has to answer — not something inherited silently
//! from whatever directory a shell happened to be in.
//!
//! This is the same shape as a familiar editor's welcome page: what you can
//! start, and what you had open last. Choosing here is explicit, so the window
//! title, the file tree and the terminal's working directory all agree from the
//! first frame.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use egui::{RichText, Ui};
use serde::{Deserialize, Serialize};

use crate::theme;
use crate::theme::Tokens;

/// How many folders the start screen offers. Long enough to cover what someone
/// actually switches between, short enough to stay scannable.
const MAX_RECENTS: usize = 12;

/// One folder the user opened before.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RecentWorkspace {
    pub path: PathBuf,
    /// Seconds since the Unix epoch. Stored as a number rather than a formatted
    /// date so the file stays locale-independent.
    #[serde(default)]
    pub opened_at: u64,
}

impl RecentWorkspace {
    /// The folder's own name, which is what a person calls the project.
    pub fn name(&self) -> String {
        self.path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.path.display().to_string())
    }

    /// The location, with `$HOME` shortened to `~`.
    ///
    /// A full home path is both long and a small privacy leak in a screenshot;
    /// `~` is what the user would have typed anyway.
    pub fn location(&self) -> String {
        let parent = self.path.parent().unwrap_or(&self.path);
        let display = parent.display().to_string().replace('\\', "/");
        match home_directory() {
            Some(home) if parent.starts_with(&home) => match parent.strip_prefix(&home) {
                Ok(rest) if rest.as_os_str().is_empty() => "~".to_owned(),
                Ok(rest) => format!("~/{}", rest.display().to_string().replace('\\', "/")),
                Err(_) => display,
            },
            _ => display,
        }
    }

    /// Whether the folder is still there. A moved or deleted project must not
    /// look openable.
    pub fn exists(&self) -> bool {
        self.path.is_dir()
    }
}

/// The remembered folders, newest first.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct RecentWorkspaces {
    #[serde(default)]
    pub entries: Vec<RecentWorkspace>,
}

impl RecentWorkspaces {
    /// Read the list, or an empty one. A corrupt or missing file is not an
    /// error worth showing anybody: it just means there is no history yet.
    pub fn load() -> Self {
        recents_path()
            .and_then(|path| std::fs::read_to_string(path).ok())
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    /// Record a folder as most recently opened.
    pub fn record(&mut self, path: &Path) {
        let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        self.entries.retain(|entry| entry.path != path);
        self.entries.insert(
            0,
            RecentWorkspace {
                path,
                opened_at: now(),
            },
        );
        self.entries.truncate(MAX_RECENTS);
    }

    pub fn forget(&mut self, path: &Path) {
        self.entries.retain(|entry| entry.path != path);
    }

    /// Persist. Failing to write history must never stop the window opening,
    /// so the error is returned for logging rather than propagated.
    pub fn save(&self) -> Result<(), String> {
        let path = recents_path().ok_or_else(|| "no data directory".to_owned())?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        let text = serde_json::to_string_pretty(self).map_err(|error| error.to_string())?;
        std::fs::write(&path, text).map_err(|error| error.to_string())
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

fn home_directory() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

/// Where the recent list lives.
///
/// Beside the daemon's own state, never inside a user repository — a file that
/// lists every project someone has opened does not belong in one of them.
fn recents_path() -> Option<PathBuf> {
    crate::default_token_path()
        .ok()?
        .parent()
        .map(|parent| parent.join("recent-workspaces.json"))
}

/// What the user chose on the start screen.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WelcomeChoice {
    /// Open this folder.
    Open(PathBuf),
    /// Show a native folder picker.
    Browse,
    /// Drop this entry from the list.
    Forget(PathBuf),
}

/// The folder chooser, as the navigation column shows it when nothing is open.
///
/// This is a panel inside the normal window, not a screen that replaces it:
/// the brand bar, the icon rail and the status bar stay where they are, so
/// opening a folder changes what the window contains rather than what the
/// application appears to be.
pub fn navigation(
    ui: &mut Ui,
    tokens: &Tokens,
    recents: &RecentWorkspaces,
) -> Option<WelcomeChoice> {
    let mut choice = None;
    ui.label(
        RichText::new("NO FOLDER OPENED")
            .small()
            .strong()
            .color(tokens.text_muted),
    );
    ui.add_space(8.0);
    ui.label(
        RichText::new("PurrCode works inside one folder at a time.")
            .small()
            .color(tokens.text_secondary),
    );
    ui.add_space(10.0);
    if crate::app::primitives::button(
        ui,
        tokens,
        crate::app::primitives::Tone::Primary,
        "Open folder…",
    )
    .clicked()
    {
        choice = Some(WelcomeChoice::Browse);
    }

    if recents.entries.is_empty() {
        return choice;
    }
    ui.add_space(18.0);
    ui.label(
        RichText::new("RECENT")
            .small()
            .strong()
            .color(tokens.text_muted),
    );
    ui.add_space(6.0);
    egui::ScrollArea::vertical()
        .id_salt("welcome_recents")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for entry in &recents.entries {
                if let Some(picked) = recent_row(ui, tokens, entry) {
                    choice = Some(picked);
                }
            }
        });
    choice
}

/// The centre of the window while no folder is open.
///
/// Deliberately a card rather than a full-bleed splash: the surrounding chrome
/// is still the application, and covering it would make choosing a folder feel
/// like a separate program.
pub fn pane(
    ui: &mut Ui,
    tokens: &Tokens,
    recents: &RecentWorkspaces,
    error: Option<&str>,
) -> Option<WelcomeChoice> {
    let mut choice = None;
    // Anchor the card's top edge at a proportion of the pane, never at an
    // assumed card height: the "or reopen" block and the error line change the
    // card's real height, and computing the top from a guessed 340pt moved the
    // whole card when one of them appeared. With a proportional offset only the
    // bottom edge moves as content grows; the top edge stays put.
    let available = ui.available_height();
    ui.add_space((available * 0.25).clamp(16.0, 220.0));
    ui.vertical_centered(|ui| {
        egui::Frame::new()
            .fill(tokens.background_raised)
            .stroke(egui::Stroke::new(1.0_f32, tokens.border_subtle))
            .corner_radius(theme::RADIUS_CARD)
            .inner_margin(egui::Margin::symmetric(36, 30))
            .show(ui, |ui| {
                // Cap, not a fixed width: a 420pt column inside a frame with
                // 36pt margins is a 494pt card, and the 572pt centre pane at
                // the 900pt minimum window shrinks below that as soon as the
                // sidebar is dragged. Content reflows instead of clipping.
                ui.set_max_width(ui.available_width().min(420.0));
                ui.vertical_centered(|ui| {
                    crate::icons::mark(ui, 56.0);
                    ui.add_space(12.0);
                    // The wordmark is one two-coloured label, so this block's
                    // centring lands it directly under the mark — no wrapper
                    // that would claim the card's whole height to do it.
                    crate::icons::wordmark(
                        ui,
                        theme::TYPE_DISPLAY,
                        tokens.text_primary,
                        tokens.accent_primary,
                    );
                    ui.add_space(4.0);
                    ui.label(
                        RichText::new("Open a folder to start working in it.")
                            .color(tokens.text_secondary),
                    );
                    ui.add_space(20.0);
                    if crate::app::primitives::button(
                        ui,
                        tokens,
                        crate::app::primitives::Tone::Primary,
                        "Open folder…",
                    )
                    .clicked()
                    {
                        choice = Some(WelcomeChoice::Browse);
                    }
                    if let Some(entry) = recents.entries.iter().find(|entry| entry.exists()) {
                        ui.add_space(12.0);
                        ui.label(RichText::new("or reopen").small().color(tokens.text_muted));
                        ui.add_space(6.0);
                        if crate::app::primitives::button(
                            ui,
                            tokens,
                            crate::app::primitives::Tone::Secondary,
                            &entry.name(),
                        )
                        .clicked()
                        {
                            choice = Some(WelcomeChoice::Open(entry.path.clone()));
                        }
                    }
                    if let Some(error) = error {
                        ui.add_space(16.0);
                        ui.label(RichText::new(error).small().color(tokens.status_error));
                    }
                });
            });
    });
    choice
}

/// One remembered folder, as a clickable row.
fn recent_row(ui: &mut Ui, tokens: &Tokens, entry: &RecentWorkspace) -> Option<WelcomeChoice> {
    let exists = entry.exists();
    let mut choice = None;
    ui.horizontal(|ui| {
        let button = egui::Button::new(RichText::new(entry.name()).color(if exists {
            tokens.accent_primary
        } else {
            tokens.text_muted
        }))
        .fill(egui::Color32::TRANSPARENT)
        .stroke(egui::Stroke::NONE)
        .corner_radius(theme::RADIUS_CONTROL);
        let response = ui
            .add_enabled(exists, button)
            .on_hover_text(entry.path.display().to_string());
        if response.clicked() {
            choice = Some(WelcomeChoice::Open(entry.path.clone()));
        }
        ui.label(
            RichText::new(entry.location())
                .small()
                .color(tokens.text_muted),
        );
        if !exists {
            // Say why it cannot be opened rather than letting a click do
            // nothing at all.
            ui.label(
                RichText::new("missing")
                    .small()
                    .color(tokens.status_warning),
            )
            .on_hover_text("This folder has moved or been deleted.");
            if ui.small_button("Remove").clicked() {
                choice = Some(WelcomeChoice::Forget(entry.path.clone()));
            }
        }
    });
    choice
}

/// Ask the operating system for a folder.
///
/// Blocking, and called from the update loop, which on macOS and Windows is the
/// main thread — the only thread allowed to open a native panel.
pub fn pick_folder(start_in: Option<&Path>) -> Option<PathBuf> {
    let mut dialog = rfd::FileDialog::new().set_title("Open a folder in PurrCode");
    if let Some(start) = start_in.filter(|path| path.is_dir()) {
        dialog = dialog.set_directory(start);
    }
    dialog.pick_folder()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The card as the window actually paints it: its own rect, and the
    /// wordmark's, at a given window height.
    fn painted_card(window_height: f32, recents: &RecentWorkspaces) -> (egui::Rect, egui::Rect) {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx, crate::theme::Appearance::Dark);
        let tokens = Tokens::for_appearance(crate::theme::Appearance::Dark);
        let output = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1200.0, window_height),
                )),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let _ = pane(ui, &tokens, recents, None);
                });
            },
        );

        // The wordmark is measured as whatever it was painted as — one run or
        // several — so this stays a statement about where the product name
        // lands, not about how it is built.
        let (mut card, mut wordmark) = (None, None);
        fn walk(
            shape: &egui::epaint::Shape,
            fill: egui::Color32,
            card: &mut Option<egui::Rect>,
            wordmark: &mut Option<egui::Rect>,
        ) {
            match shape {
                egui::epaint::Shape::Vec(shapes) => {
                    for shape in shapes {
                        walk(shape, fill, card, wordmark);
                    }
                }
                egui::epaint::Shape::Rect(rect) if rect.fill == fill => {
                    *card = Some(rect.rect);
                }
                egui::epaint::Shape::Text(text)
                    if matches!(text.galley.text(), "PurrCode" | "Purr" | "Code") =>
                {
                    let rect = text.visual_bounding_rect();
                    *wordmark = Some(match *wordmark {
                        Some(seen) => seen.union(rect),
                        None => rect,
                    });
                }
                _ => {}
            }
        }
        for clipped in &output.shapes {
            walk(
                &clipped.shape,
                tokens.background_raised,
                &mut card,
                &mut wordmark,
            );
        }
        (
            card.expect("the card was painted"),
            wordmark.expect("the wordmark was painted"),
        )
    }

    #[test]
    fn the_card_stays_compact_however_tall_the_window_is() {
        // The defect this pins: the wordmark used to be centred by a wrapper
        // that claimed every remaining point of the panel, so the card grew
        // with the window and left a several-hundred-point hole above and
        // below the product name.
        let recents = RecentWorkspaces::default();
        let short = painted_card(800.0, &recents).0;
        let tall = painted_card(1400.0, &recents).0;
        assert_eq!(
            short.height(),
            tall.height(),
            "a 600pt taller window must not make the card 600pt taller"
        );
        assert!(
            short.height() < 400.0,
            "the card is a compact panel, not a splash screen ({}pt)",
            short.height()
        );
    }

    #[test]
    fn the_wordmark_sits_under_the_mascot_not_against_the_card_edge() {
        let recents = RecentWorkspaces::default();
        let (card, wordmark) = painted_card(800.0, &recents);
        assert!(
            (wordmark.center().x - card.center().x).abs() < 2.0,
            "the wordmark must be centred in the card (wordmark {}, card {})",
            wordmark.center().x,
            card.center().x
        );
        assert!(
            wordmark.top() - card.top() < 120.0,
            "the wordmark must follow the mascot, not float in the card's \
             middle ({}pt below the card's top edge)",
            wordmark.top() - card.top()
        );
    }

    #[test]
    fn the_most_recent_folder_is_first_and_never_duplicated() {
        let mut recents = RecentWorkspaces::default();
        recents.record(Path::new("/tmp/alpha"));
        recents.record(Path::new("/tmp/beta"));
        recents.record(Path::new("/tmp/alpha"));
        assert_eq!(recents.entries.len(), 2, "re-opening is not a new entry");
        assert_eq!(recents.entries[0].path, Path::new("/tmp/alpha"));
        assert_eq!(recents.entries[1].path, Path::new("/tmp/beta"));
    }

    #[test]
    fn the_list_is_bounded() {
        let mut recents = RecentWorkspaces::default();
        for index in 0..40 {
            recents.record(Path::new(&format!("/tmp/project-{index}")));
        }
        assert_eq!(recents.entries.len(), MAX_RECENTS);
        assert_eq!(
            recents.entries[0].path,
            Path::new("/tmp/project-39"),
            "the newest must survive the truncation"
        );
    }

    #[test]
    fn forgetting_removes_exactly_one_entry() {
        let mut recents = RecentWorkspaces::default();
        recents.record(Path::new("/tmp/alpha"));
        recents.record(Path::new("/tmp/beta"));
        recents.forget(Path::new("/tmp/alpha"));
        assert_eq!(recents.entries.len(), 1);
        assert_eq!(recents.entries[0].path, Path::new("/tmp/beta"));
    }

    #[test]
    fn a_home_relative_location_is_shortened() {
        // Long absolute paths are unreadable in a list and leak the account
        // name into every screenshot.
        let home = home_directory().expect("a home directory");
        let entry = RecentWorkspace {
            path: home.join("Documents").join("GitHub").join("PurrCode"),
            opened_at: 0,
        };
        assert_eq!(entry.name(), "PurrCode");
        assert_eq!(entry.location(), "~/Documents/GitHub");
    }

    #[test]
    fn a_location_outside_home_is_shown_in_full() {
        let entry = RecentWorkspace {
            path: PathBuf::from("/opt/work/service"),
            opened_at: 0,
        };
        assert_eq!(entry.location(), "/opt/work");
    }

    #[test]
    fn a_missing_folder_is_reported_as_missing() {
        let entry = RecentWorkspace {
            path: PathBuf::from("/definitely/not/here/at/all"),
            opened_at: 0,
        };
        assert!(!entry.exists());
    }

    #[test]
    fn the_recent_list_is_never_written_inside_a_repository() {
        let Some(path) = recents_path() else {
            return;
        };
        assert!(
            !path.starts_with(std::env::current_dir().unwrap_or_default()),
            "the list of every project a user opens must not live in one of them"
        );
        assert!(path.ends_with("recent-workspaces.json"));
    }

    #[test]
    fn a_stored_list_round_trips() {
        let mut recents = RecentWorkspaces::default();
        recents.record(Path::new("/tmp/alpha"));
        let text = serde_json::to_string(&recents).expect("serialise");
        let parsed: RecentWorkspaces = serde_json::from_str(&text).expect("parse");
        assert_eq!(parsed.entries[0].path, recents.entries[0].path);
    }
}
