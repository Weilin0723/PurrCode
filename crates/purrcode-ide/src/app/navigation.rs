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
        self.session_search_field(ui);
        if self.session_search_active() {
            self.session_search_results(ui);
            return;
        }

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

        // Group sessions by time (clone all data to avoid borrow issues).
        //
        // Pinned and Archived are lifecycle groups, so they outrank the time
        // buckets: something the user pinned should not sink out of view
        // because it was last touched on Tuesday, and something they archived
        // should not keep appearing under Today.
        let mut groups: std::collections::BTreeMap<String, Vec<SessionEntry>> =
            std::collections::BTreeMap::new();
        let sessions_data: Vec<(SessionEntry, String)> = self
            .sessions
            .iter()
            .map(|row| {
                let group_key = if row.archived {
                    "Archived".to_owned()
                } else if row.pinned {
                    "Pinned".to_owned()
                } else if is_closed(row.state) {
                    "Closed".to_owned()
                } else if row.group == "Today" || row.group == "Yesterday" {
                    row.group.clone()
                } else {
                    "Earlier".to_owned()
                };
                (
                    SessionEntry {
                        id: row.id.clone(),
                        title: row.title.clone(),
                        needs_attention: row.needs_attention,
                        relative_time: row.relative_time.clone(),
                        pinned: row.pinned,
                        archived: row.archived,
                        forked: row.parent_id.is_some(),
                        running: row.state.execution_active(),
                    },
                    group_key,
                )
            })
            .collect();

        for (entry, group_key) in sessions_data {
            groups.entry(group_key).or_default().push(entry);
        }

        let mut session_to_select: Option<String> = None;
        let mut action: Option<(SessionAction, SessionEntry)> = None;
        // Collapsed groups keep their own flag, keyed by name: folding
        // "Closed" must not also fold "Archived", which is a different
        // decision about a different set of sessions.
        let collapsed_id = |group: &str| egui::Id::new(("purrcode_session_group", group));
        egui::ScrollArea::vertical()
            .id_salt("session_list")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for group_name in [
                    "Pinned",
                    "Today",
                    "Yesterday",
                    "Earlier",
                    "Closed",
                    "Archived",
                ] {
                    let Some(entries) = groups.get(group_name) else {
                        continue;
                    };
                    // Groups that are noise by default start folded; the ones
                    // describing current work never do.
                    let folds = matches!(group_name, "Earlier" | "Closed" | "Archived");
                    let shown = if folds {
                        ui.data_mut(|data| {
                            data.get_temp::<bool>(collapsed_id(group_name)).unwrap_or(false)
                        })
                    } else {
                        true
                    };
                    if !shown {
                        ui.add_space(4.0);
                        self.section_heading(ui, group_name);
                        let response = ui
                            .button(format!("Show {} {group_name} sessions", entries.len()))
                            .on_hover_text(match group_name {
                                "Earlier" => "Older durable runs are hidden from the working list. Their audit records are preserved.",
                                "Archived" => "Archived sessions are out of the way, not deleted. Their audit records are preserved.",
                                _ => "Finished sessions that cannot be resumed are folded up. Their audit records are preserved.",
                            });
                        if response.clicked() {
                            ui.data_mut(|data| data.insert_temp(collapsed_id(group_name), true));
                        }
                        continue;
                    }
                    ui.add_space(2.0);
                    ui.horizontal(|ui| {
                        self.section_heading_label(ui, group_name);
                        if folds && ui.small_button("Hide").clicked() {
                            ui.data_mut(|data| data.insert_temp(collapsed_id(group_name), false));
                        }
                    });
                    for entry in entries {
                        let selected = self.selected.as_deref() == Some(entry.id.as_str());
                        let outcome = self.session_row(ui, entry, selected);
                        if outcome.selected {
                            session_to_select = Some(entry.id.clone());
                        }
                        if let Some(chosen) = outcome.action {
                            action = Some((chosen, entry.clone()));
                        }
                    }
                    ui.add_space(6.0);
                }
            });
        if let Some((chosen, entry)) = action {
            self.apply_session_action(chosen, &entry);
        }
        if let Some(id) = session_to_select {
            self.select_session(&id);
        }
    }

    /// One session in the sidebar.
    ///
    /// The whole row is the target — the previous version only accepted clicks
    /// on the title glyphs themselves, so half of a row that looked clickable
    /// was not. Returns `true` when it was chosen.
    fn session_row(&self, ui: &mut Ui, entry: &SessionEntry, selected: bool) -> RowOutcome {
        let title = entry.title.as_str();
        let relative_time = entry.relative_time.as_str();
        let needs_attention = entry.needs_attention;
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

        // A session that is still working shows it whether or not it is the
        // one on screen: a background run the user switched away from is
        // exactly the thing they need to be able to see from here.
        let dot = if needs_attention {
            self.tokens.status_warning
        } else if entry.running {
            self.tokens.status_running
        } else if selected {
            self.tokens.accent_primary
        } else {
            self.tokens.text_muted
        };
        ui.painter()
            .circle_filled(egui::pos2(rect.left() + 12.0, rect.center().y), 3.0, dot);
        if entry.running && !selected {
            // A ring around the dot, so "working elsewhere" is legible at a
            // glance without adding a second column of chrome.
            ui.painter().circle_stroke(
                egui::pos2(rect.left() + 12.0, rect.center().y),
                5.5,
                egui::Stroke::new(1.0_f32, self.tokens.status_running.gamma_multiply(0.6)),
            );
        }

        // The timestamp is measured first so the title can be given exactly
        // the space that is left, and elided rather than overrun.
        let meta_font = egui::FontId::proportional(10.5);
        // Markers ride with the timestamp rather than beside the title, so a
        // pinned or forked session is legible without stealing width from the
        // one thing the user actually reads.
        let mut marks = String::new();
        if entry.pinned {
            marks.push('📌');
        }
        if entry.forked {
            marks.push('⑂');
        }
        let meta = match (needs_attention, marks.is_empty()) {
            (true, true) => format!("• {relative_time}"),
            (true, false) => format!("{marks} • {relative_time}"),
            (false, true) => relative_time.to_owned(),
            (false, false) => format!("{marks} {relative_time}"),
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

        // Lifecycle actions live in the row's own context menu rather than in
        // a row of icons: a sidebar where every session carries five buttons
        // stops being a list of work and becomes a control panel.
        let mut action = None;
        response.context_menu(|ui| {
            action = session_context_menu(ui, entry);
        });
        RowOutcome {
            selected: response.clicked(),
            action,
        }
    }

    // ── Session workspace ───────────────────────────────────────────

    /// Whether the sidebar is showing search results rather than the list.
    fn session_search_active(&self) -> bool {
        !self.session_query.trim().is_empty()
    }

    /// The search field above the session list.
    fn session_search_field(&mut self, ui: &mut Ui) {
        let mut query = self.session_query.clone();
        let response = ui.add(
            egui::TextEdit::singleline(&mut query)
                .hint_text("Search sessions…")
                .desired_width(f32::INFINITY),
        );
        if query != self.session_query {
            self.session_query = query;
            // Results for the previous query would be shown against this one
            // for a frame; clear them rather than mislabel them.
            self.session_hits.clear();
            self.session_hits_for = None;
        }
        if response.changed() || (response.lost_focus() && self.session_search_active()) {
            self.run_session_search();
        }
        ui.add_space(4.0);
    }

    fn run_session_search(&mut self) {
        let query = self.session_query.trim().to_owned();
        if query.is_empty() || self.session_hits_for.as_deref() == Some(query.as_str()) {
            return;
        }
        self.session_hits_for = Some(query.clone());
        self.client
            .send(crate::daemon::Request::SearchSessions { query });
    }

    /// Search results: a snippet per matching event, grouped by session.
    ///
    /// The search runs over the event log, so a hit is evidence — the actual
    /// text that matched, with the event that carried it — rather than a
    /// title guess.
    fn session_search_results(&mut self, ui: &mut Ui) {
        let tokens = self.tokens;
        if self.session_hits_for.as_deref() != Some(self.session_query.trim()) {
            ui.label(RichText::new("Searching…").small().color(tokens.text_muted));
            return;
        }
        if self.session_hits.is_empty() {
            ui.label(
                RichText::new("Nothing matched.")
                    .small()
                    .color(tokens.text_muted),
            );
            return;
        }
        let hits = self.session_hits.clone();
        let titles: std::collections::BTreeMap<String, String> = self
            .sessions
            .iter()
            .map(|row| (row.id.clone(), row.title.clone()))
            .collect();
        let mut select = None;
        egui::ScrollArea::vertical()
            .id_salt("session_search_results")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for hit in &hits {
                    let title = titles
                        .get(&hit.session_id)
                        .cloned()
                        // A hit in a session this folder's list does not carry
                        // is named by its event rather than silently dropped.
                        .unwrap_or_else(|| {
                            format!("Session {}", &hit.session_id[..8.min(hit.session_id.len())])
                        });
                    let response = ui
                        .vertical(|ui| {
                            ui.label(
                                RichText::new(title)
                                    .size(crate::theme::TYPE_LABEL)
                                    .color(tokens.text_primary),
                            );
                            ui.label(
                                RichText::new(&hit.snippet)
                                    .size(crate::theme::TYPE_META)
                                    .color(tokens.text_secondary),
                            );
                            ui.label(
                                RichText::new(format!(
                                    "{} · {}",
                                    hit.event_type,
                                    crate::model::relative_time(&hit.occurred_at)
                                ))
                                .size(crate::theme::TYPE_EYEBROW)
                                .color(tokens.text_muted),
                            );
                        })
                        .response;
                    if response.interact(Sense::click()).clicked() {
                        select = Some(hit.session_id.clone());
                    }
                    ui.add_space(6.0);
                }
            });
        if let Some(id) = select {
            self.session_query.clear();
            self.session_hits.clear();
            self.session_hits_for = None;
            self.select_session(&id);
        }
    }

    /// Runs a lifecycle action against one session.
    fn apply_session_action(&mut self, action: SessionAction, entry: &SessionEntry) {
        use crate::daemon::Request;
        let session = entry.id.clone();
        match action {
            SessionAction::Rename => {
                self.renaming_session = Some((session, entry.title.clone()));
            }
            SessionAction::TogglePin => self.client.send(Request::UpdateSessionMeta {
                session,
                title: None,
                archived: None,
                pinned: Some(!entry.pinned),
            }),
            SessionAction::ToggleArchive => self.client.send(Request::UpdateSessionMeta {
                session,
                title: None,
                archived: Some(!entry.archived),
                pinned: None,
            }),
            SessionAction::Delete => self.deleting_session = Some((session, entry.title.clone())),
        }
    }

    /// Rename and delete both need a word from the user before they commit.
    pub(crate) fn session_dialogs(&mut self, ctx: &egui::Context) {
        self.rename_session_dialog(ctx);
        self.delete_session_dialog(ctx);
    }

    fn rename_session_dialog(&mut self, ctx: &egui::Context) {
        let Some((session, title)) = self.renaming_session.clone() else {
            return;
        };
        let mut next = title;
        let mut close = false;
        let mut commit = false;
        egui::Modal::new(egui::Id::new("purrcode_rename_session")).show(ctx, |ui| {
            ui.set_width(340.0);
            ui.label(
                RichText::new("Rename session")
                    .size(crate::theme::TYPE_TITLE)
                    .color(self.tokens.text_primary),
            );
            ui.add_space(8.0);
            let response = ui.add(
                egui::TextEdit::singleline(&mut next)
                    .desired_width(f32::INFINITY)
                    .hint_text("Session name"),
            );
            response.request_focus();
            if response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter)) {
                commit = true;
            }
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if ui.button("Cancel").clicked() {
                    close = true;
                }
                ui.add_enabled_ui(!next.trim().is_empty(), |ui| {
                    if ui.button("Rename").clicked() {
                        commit = true;
                    }
                });
            });
        });
        self.renaming_session = Some((session.clone(), next.clone()));
        if commit && !next.trim().is_empty() {
            self.client.send(crate::daemon::Request::UpdateSessionMeta {
                session,
                title: Some(next.trim().to_owned()),
                archived: None,
                pinned: None,
            });
            close = true;
        }
        if close {
            self.renaming_session = None;
        }
    }

    fn delete_session_dialog(&mut self, ctx: &egui::Context) {
        let Some((session, title)) = self.deleting_session.clone() else {
            return;
        };
        let mut close = false;
        let mut confirm = false;
        egui::Modal::new(egui::Id::new("purrcode_delete_session")).show(ctx, |ui| {
            ui.set_width(380.0);
            ui.label(
                RichText::new("Delete session")
                    .size(crate::theme::TYPE_TITLE)
                    .color(self.tokens.text_primary),
            );
            ui.add_space(6.0);
            ui.label(
                RichText::new(format!("“{title}” is removed from this list."))
                    .color(self.tokens.text_primary),
            );
            ui.add_space(4.0);
            // Stated because it is true and because it matters: this is a
            // soft delete, and a user who believes they have erased an audit
            // trail has been misled about what PurrCode keeps.
            ui.label(
                RichText::new(
                    "Its audit record is preserved. Archive it instead if you only want it out \
                     of the way.",
                )
                .size(crate::theme::TYPE_META)
                .color(self.tokens.text_secondary),
            );
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if ui.button("Cancel").clicked() {
                    close = true;
                }
                if ui.button("Delete").clicked() {
                    confirm = true;
                }
            });
        });
        if confirm {
            self.client
                .send(crate::daemon::Request::DeleteSession { session });
            close = true;
        }
        if close {
            self.deleting_session = None;
        }
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
/// One session as the sidebar needs it, flattened out of `SessionRow` so the
/// list can be drawn while `self` is borrowed mutably elsewhere.
#[derive(Clone, Debug)]
pub(crate) struct SessionEntry {
    pub id: String,
    pub title: String,
    pub relative_time: String,
    pub needs_attention: bool,
    pub pinned: bool,
    pub archived: bool,
    /// Forked from another session, so the row can say so.
    pub forked: bool,
    /// Still executing. True for a background run the user has switched away
    /// from, which is the whole point of showing it.
    pub running: bool,
}

/// What a session row's context menu asked for.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SessionAction {
    Rename,
    TogglePin,
    ToggleArchive,
    Delete,
}

/// What one row reported this frame.
pub(crate) struct RowOutcome {
    pub selected: bool,
    pub action: Option<SessionAction>,
}

fn session_context_menu(ui: &mut Ui, entry: &SessionEntry) -> Option<SessionAction> {
    let mut chosen = None;
    if ui.button("Rename…").clicked() {
        chosen = Some(SessionAction::Rename);
        ui.close();
    }
    if ui
        .button(if entry.pinned { "Unpin" } else { "Pin" })
        .clicked()
    {
        chosen = Some(SessionAction::TogglePin);
        ui.close();
    }
    if ui
        .button(if entry.archived {
            "Unarchive"
        } else {
            "Archive"
        })
        .clicked()
    {
        chosen = Some(SessionAction::ToggleArchive);
        ui.close();
    }
    ui.separator();
    if ui
        .button("Delete…")
        .on_hover_text("Removes it from this list. The audit record is kept.")
        .clicked()
    {
        chosen = Some(SessionAction::Delete);
        ui.close();
    }
    chosen
}

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
