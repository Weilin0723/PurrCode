//! The navigation column: the session list and text search.
//!
//! PRD §19 defines the "Recent Work" model — a persistent list of sessions
//! that survives application restarts. The navigation column keeps the user's
//! work in front of them; the project file tree lives in the auxiliary panel
//! (code.rs), so this column stays about sessions and search.

use std::path::{Path, PathBuf};

use egui::{RichText, Sense, Ui};
use purrcode_runtime_core::ProductState;

use super::primitives;
use super::{PurrCodeIde, Stage};
use crate::theme;

impl PurrCodeIde {
    /// The navigation column. Content depends on which nav tab is selected.
    pub(crate) fn navigation(&mut self, ui: &mut Ui) {
        if self.stage == Stage::Welcome {
            self.navigation_welcome(ui);
            return;
        }

        // The Agent sidebar: sessions grouped by current work. The Activity
        // Bar switches between this and Explorer/Search/Source Control.
        let mut start_new = false;
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("AGENT")
                    .size(theme::TYPE_EYEBROW)
                    .strong()
                    .color(self.tokens.text_muted),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                start_new |= self
                    .icon_action(ui, crate::icons::Glyph::Plus, "New session")
                    .clicked();
            });
        });
        ui.add_space(4.0);
        if start_new {
            // Deselecting is what starts a new session: the next thing typed
            // in the composer opens one. Nothing is created until then.
            self.selected = None;
            self.session = crate::model::Session::default();
            self.diff = None;
            self.focus_composer = true;
        }
        self.session_list(ui);
    }

    // ── Welcome (no folder) ─────────────────────────────────────────

    fn navigation_welcome(&mut self, ui: &mut Ui) {
        // Delegate to the full welcome implementation: a folder that was moved
        // or deleted stays visible with a "missing" chip and a Remove action
        // instead of vanishing from the list with no way to clean it up. The
        // choice is applied here the same way the centre pane applies its own.
        let recents = self.recents.clone();
        let tokens = self.tokens;
        let choice = crate::welcome::navigation(ui, &tokens, &recents);
        self.apply_welcome_choice(choice);
    }

    // ── Session list ────────────────────────────────────────────────

    fn session_list(&mut self, ui: &mut Ui) {
        if self.sessions.is_empty() {
            ui.add_space(12.0);
            ui.label(
                RichText::new("No sessions yet")
                    .small()
                    .color(self.tokens.text_muted),
            );
            ui.add_space(8.0);
            ui.label(
                RichText::new("Type something in the composer to start working.")
                    .small()
                    .color(self.tokens.text_muted),
            );
            return;
        }

        // Group sessions by time (clone all data to avoid borrow issues)
        let mut groups: std::collections::BTreeMap<String, Vec<(String, String, bool, String)>> =
            std::collections::BTreeMap::new();
        let sessions_data: Vec<(String, String, bool, String, String)> = self
            .sessions
            .iter()
            .map(|row| {
                let group_key = if is_closed(row.state) {
                    "Closed".to_owned()
                } else if row.group == "Today" || row.group == "Yesterday" {
                    row.group.clone()
                } else {
                    "Earlier".to_owned()
                };
                (
                    row.id.clone(),
                    row.title.clone(),
                    row.needs_attention,
                    row.relative_time.clone(),
                    group_key,
                )
            })
            .collect();

        for (id, title, needs_attention, relative_time, group_key) in sessions_data {
            groups
                .entry(group_key)
                .or_default()
                .push((id, title, needs_attention, relative_time));
        }

        // Order: Today, Yesterday, Earlier, Closed
        let mut session_to_select: Option<String> = None;
        let earlier_visibility_id = egui::Id::new("purrcode_show_earlier_sessions");
        let mut show_earlier = ui.data_mut(|data| {
            data.get_temp::<bool>(earlier_visibility_id)
                .unwrap_or(false)
        });
        let closed_visibility_id = egui::Id::new("purrcode_show_closed_sessions");
        let mut show_closed =
            ui.data_mut(|data| data.get_temp::<bool>(closed_visibility_id).unwrap_or(false));
        egui::ScrollArea::vertical()
            .id_salt("session_list")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for group_name in ["Today", "Yesterday", "Earlier", "Closed"] {
                    let Some(entries) = groups.get(group_name) else {
                        continue;
                    };
                    let hidden = if group_name == "Earlier" {
                        !show_earlier
                    } else if group_name == "Closed" {
                        !show_closed
                    } else {
                        false
                    };
                    if hidden {
                        ui.add_space(4.0);
                        self.section_heading(ui, group_name);
                        let response = ui
                            .button(format!("Show {} {group_name} sessions", entries.len()))
                            .on_hover_text(if group_name == "Earlier" {
                                "Older durable runs are hidden from the working list. Their audit records are preserved."
                            } else {
                                "Finished sessions that cannot be resumed are folded up. Their audit records are preserved."
                            });
                        if response.clicked() {
                            if group_name == "Earlier" {
                                show_earlier = true;
                                ui.data_mut(|data| {
                                    data.insert_temp(earlier_visibility_id, true);
                                });
                            } else {
                                show_closed = true;
                                ui.data_mut(|data| {
                                    data.insert_temp(closed_visibility_id, true);
                                });
                            }
                        }
                        continue;
                    }
                    ui.add_space(2.0);
                    ui.horizontal(|ui| {
                        self.section_heading_label(ui, group_name);
                        if (group_name == "Earlier" && ui.small_button("Hide").clicked())
                            || (group_name == "Closed" && ui.small_button("Hide").clicked())
                        {
                            if group_name == "Earlier" {
                                show_earlier = false;
                                ui.data_mut(|data| {
                                    data.insert_temp(earlier_visibility_id, false);
                                });
                            } else {
                                show_closed = false;
                                ui.data_mut(|data| {
                                    data.insert_temp(closed_visibility_id, false);
                                });
                            }
                        }
                    });
                    for (id, title, needs_attention, relative_time) in entries {
                        let selected = self.selected.as_deref() == Some(id);
                        if self.session_row(ui, title, relative_time, *needs_attention, selected) {
                            session_to_select = Some(id.clone());
                        }
                    }
                    ui.add_space(6.0);
                }
            });
        if let Some(id) = session_to_select {
            self.select_session(&id);
        }
    }

    /// One session in the sidebar.
    ///
    /// The whole row is the target — the previous version only accepted clicks
    /// on the title glyphs themselves, so half of a row that looked clickable
    /// was not. Returns `true` when it was chosen.
    fn session_row(
        &self,
        ui: &mut Ui,
        title: &str,
        relative_time: &str,
        needs_attention: bool,
        selected: bool,
    ) -> bool {
        let width = ui.available_width();
        let response =
            ui.allocate_response(egui::vec2(width, crate::theme::ROW_HEIGHT), Sense::click());
        let rect = response.rect;
        if selected {
            ui.painter()
                .rect_filled(rect, crate::theme::RADIUS_CONTROL, self.tokens.accent_soft);
        } else if response.hovered() {
            ui.painter().rect_filled(
                rect,
                crate::theme::RADIUS_CONTROL,
                self.tokens.surface_hover,
            );
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        if response.has_focus() {
            self.tokens.focus_ring(ui.painter(), rect);
        }

        let dot = if needs_attention {
            self.tokens.status_warning
        } else if selected {
            self.tokens.accent_primary
        } else {
            self.tokens.text_muted
        };
        ui.painter()
            .circle_filled(egui::pos2(rect.left() + 12.0, rect.center().y), 3.0, dot);

        // The timestamp is measured first so the title can be given exactly
        // the space that is left, and elided rather than overrun.
        let meta_font = egui::FontId::proportional(10.5);
        let meta = if needs_attention {
            format!("• {relative_time}")
        } else {
            relative_time.to_owned()
        };
        let meta_width = ui.fonts_mut(|fonts| {
            fonts
                .layout_no_wrap(meta.clone(), meta_font.clone(), self.tokens.text_muted)
                .size()
                .x
        });
        ui.painter().text(
            egui::pos2(rect.right() - 8.0, rect.center().y),
            egui::Align2::RIGHT_CENTER,
            &meta,
            meta_font,
            if needs_attention {
                self.tokens.status_warning
            } else {
                self.tokens.text_muted
            },
        );

        // One line, ending in an ellipsis when it does not fit. A session
        // title that wraps turns a list into a wall, and a title clipped
        // mid-stroke reads as a rendering fault rather than as "there is more".
        let color = if selected {
            self.tokens.text_primary
        } else {
            self.tokens.text_secondary
        };
        let mut job = egui::text::LayoutJob::single_section(
            title.to_owned(),
            egui::TextFormat::simple(egui::FontId::proportional(12.0), color),
        );
        job.wrap = egui::text::TextWrapping {
            max_width: (rect.width() - 22.0 - meta_width - 16.0).max(40.0),
            max_rows: 1,
            break_anywhere: true,
            overflow_character: Some('…'),
        };
        let galley = ui.fonts_mut(|fonts| fonts.layout_job(job));
        let elided = galley.elided;
        ui.painter().galley(
            egui::pos2(rect.left() + 22.0, rect.center().y - galley.size().y * 0.5),
            galley,
            color,
        );

        let response = if elided {
            response.on_hover_text(title)
        } else {
            response
        };
        response.clicked()
    }

    // ── File open ───────────────────────────────────────────────────

    pub(crate) fn open_file(&mut self, path: PathBuf) {
        let label = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());
        let body = match std::fs::read_to_string(&path) {
            Ok(content) => Ok(content),
            Err(e) => Err(e.to_string()),
        };
        // Check if already open
        if let Some(pos) = self.open_files.iter().position(|f| f.path == path) {
            self.active_file = pos;
        } else {
            self.open_files.push(super::OpenFile {
                path: path.clone(),
                label,
                body,
                scroll_to_line: None,
                modified: false,
                disk_stamp: super::disk_stamp(&path),
                external_change: false,
            });
            self.active_file = self.open_files.len() - 1;
        }
        self.code_panel = super::CodePanel::Source;
        self.activity = super::ActivityBar::Explorer;
        self.agent_location = super::AgentLocation::Aux;
        self.aux_panel = Some(super::AuxView::Agent);
        // Hand the document to its language server so analysis starts, and
        // ask for its outline. Both are no-ops without a server for the type.
        self.open_in_language_server(&path);
        self.request_symbols(&path);
    }

    // ── Search ──────────────────────────────────────────────────────

    /// The project-wide search surface: a polished field that matches the
    /// settings search control (glyph, hairline border, focus ring) and result
    /// rows that read as one object — path on the left, matching line number,
    /// matched text elided on the right.
    pub(crate) fn search_panel(&mut self, ui: &mut Ui) {
        let tokens = self.tokens;
        self.panel_header(ui, "Search", None);

        // The field, painted like the settings "Find a setting" control so the
        // two search affordances share one visual language.
        let (rect, _) =
            ui.allocate_exact_size(egui::vec2(ui.available_width(), 28.0), Sense::hover());
        let painter = ui.painter().clone();
        painter.rect_filled(rect, theme::RADIUS_CONTROL, tokens.background_raised);
        painter.rect_stroke(
            rect,
            theme::RADIUS_CONTROL,
            tokens.hairline(),
            egui::StrokeKind::Inside,
        );
        let glyph = egui::Rect::from_center_size(
            egui::pos2(rect.left() + 15.0, rect.center().y),
            egui::Vec2::splat(13.0),
        );
        crate::icons::draw(ui, glyph, crate::icons::Glyph::Search, tokens.text_muted);
        let field = egui::Rect::from_min_max(
            egui::pos2(glyph.right() + 7.0, rect.top() + 3.0),
            egui::pos2(
                (rect.right() - 8.0).max(glyph.right() + 7.0),
                rect.bottom() - 3.0,
            ),
        );
        let response = ui.put(
            field,
            egui::TextEdit::singleline(&mut self.search_query)
                .frame(false)
                .hint_text("Search across project files…"),
        );
        if response.has_focus() {
            painter.rect_stroke(
                rect.expand(1.0),
                theme::RADIUS_CONTROL,
                egui::Stroke::new(2.0_f32, tokens.accent_primary),
                egui::StrokeKind::Outside,
            );
        }

        ui.add_space(6.0);

        let query = self.search_query.trim().to_owned();
        let changed = self.search_ran_for.as_deref() != Some(query.as_str());
        if query.is_empty() {
            self.search_results.clear();
            self.search_ran = false;
            self.search_ran_for = None;
            ui.add_space(4.0);
            ui.label(
                RichText::new("Search for a word or symbol to find where it lives.")
                    .size(theme::TYPE_META)
                    .color(tokens.text_muted),
            );
            return;
        } else if changed {
            // Search is a live query, not a one-shot: every edit re-runs it.
            self.search_ran = true;
            self.search_ran_for = Some(query.clone());
            self.run_search(&query);
        }

        egui::ScrollArea::vertical()
            .id_salt("search_results_scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let results = self.search_results.to_vec();
                if results.is_empty() && self.search_ran {
                    ui.add_space(4.0);
                    ui.label(
                        RichText::new(format!("No matches for \"{query}\""))
                            .size(theme::TYPE_META)
                            .color(tokens.text_muted),
                    );
                    ui.label(
                        RichText::new("Try a different word, or a file name.")
                            .size(theme::TYPE_META)
                            .color(tokens.text_muted.gamma_multiply(0.8)),
                    );
                }
                for (path, line, text) in results {
                    let display = PathBuf::from(&path)
                        .file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    // The full row is the target — a highlight as wide as the
                    // matched text made a ragged stack of half-width bars.
                    let width = ui.available_width().max(0.0);
                    let (rect, response) = ui
                        .allocate_exact_size(egui::vec2(width, theme::ROW_HEIGHT), Sense::click());
                    if response.hovered() {
                        ui.painter()
                            .rect_filled(rect, theme::RADIUS_CONTROL, tokens.surface_hover);
                        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                    }
                    if response.has_focus() {
                        tokens.focus_ring(ui.painter(), rect);
                    }
                    // The location is measured first so the excerpt is given
                    // exactly the space that is left.
                    let loc_font = egui::FontId::monospace(theme::TYPE_META);
                    let loc_width = ui.fonts_mut(|fonts| {
                        fonts
                            .layout_no_wrap(
                                format!("{display}:{line}"),
                                loc_font.clone(),
                                tokens.accent_primary,
                            )
                            .size()
                            .x
                    });
                    ui.painter().text(
                        egui::pos2(rect.left() + 8.0, rect.center().y),
                        egui::Align2::LEFT_CENTER,
                        format!("{display}:{line}"),
                        loc_font,
                        tokens.accent_primary,
                    );
                    let excerpt_max = (rect.width() - loc_width - 24.0).max(24.0);
                    let excerpt_font = egui::FontId::proportional(theme::TYPE_META);
                    let excerpt_galley = primitives::fit_tail(
                        ui,
                        &text,
                        excerpt_font,
                        tokens.text_secondary,
                        excerpt_max,
                    );
                    ui.painter().galley(
                        egui::pos2(
                            rect.left() + loc_width + 16.0,
                            rect.center().y - excerpt_galley.size().y * 0.5,
                        ),
                        excerpt_galley,
                        tokens.text_secondary,
                    );
                    if response.clicked() {
                        let repo = self.repository.clone();
                        let absolute = if Path::new(&path).is_absolute() {
                            path
                        } else {
                            repo.join(path)
                        };
                        self.open_file(absolute);
                    }
                    ui.add_space(2.0);
                }
            });
    }

    fn run_search(&mut self, query: &str) {
        let repository = self.repository.clone();
        self.search_results.clear();
        if query.is_empty() || repository.as_os_str().is_empty() {
            return;
        }

        if let Ok(results) = walk_dir(&repository, query, 50) {
            self.search_results = results;
        }
    }
}

fn walk_dir(
    root: &std::path::Path,
    query: &str,
    max_results: usize,
) -> Result<Vec<(PathBuf, usize, String)>, String> {
    let lower = query.to_ascii_lowercase();
    let mut results = Vec::new();
    let mut entries = Vec::new();
    entries.push(root.to_path_buf());
    while let Some(dir) = entries.pop() {
        if results.len() >= max_results {
            break;
        }
        let Ok(read) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in read.flatten() {
            if results.len() >= max_results {
                break;
            }
            let path = entry.path();
            if path
                .file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.starts_with('.'))
                .unwrap_or(false)
            {
                continue;
            }
            if path.is_dir() {
                entries.push(path);
            } else if path.is_file() {
                // Only search text-like files
                let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
                match ext {
                    "rs" | "py" | "js" | "ts" | "tsx" | "jsx" | "md" | "toml" | "json" | "yaml"
                    | "yml" | "html" | "css" | "sh" | "txt" | "lock" | "env" => {}
                    _ => continue,
                }
                if let Ok(content) = std::fs::read_to_string(&path) {
                    for (i, line) in content.lines().enumerate() {
                        if line.to_ascii_lowercase().contains(&lower) {
                            results.push((path.clone(), i + 1, line.trim().to_owned()));
                            if results.len() >= max_results {
                                break;
                            }
                        }
                    }
                }
            }
        }
    }
    Ok(results)
}

/// True when a session is in a terminal, not-recyclable state and should be
/// folded into the "Closed" section instead of cluttering the working list.
///
/// Resumability is judged by whether the state still has something to act on
/// (`primary_action().is_some()`); `Failed` and `NeedsRecovery` keep their
/// primary action and `needs_attention` already flags them, so they stay
/// visible. `Cancelled` is the one truly closed terminal state today.
pub(crate) fn is_closed(state: ProductState) -> bool {
    matches!(state, ProductState::Cancelled)
}

#[cfg(test)]
mod tests {
    use super::*;
    use purrcode_runtime_core::ProductState;

    #[test]
    fn cancelled_is_closed_and_other_terminal_states_are_not() {
        for state in ProductState::ALL {
            let expected = *state == ProductState::Cancelled;
            assert_eq!(is_closed(*state), expected, "{state:?}");
        }
    }

    #[test]
    fn failed_and_needs_recovery_are_resumable_and_stay_visible() {
        // These states carry a retry/recover action and `needs_attention`, so
        // they keep a row in the working list rather than folding into Closed.
        assert!(!is_closed(ProductState::Failed));
        assert!(!is_closed(ProductState::NeedsRecovery));
    }
}
