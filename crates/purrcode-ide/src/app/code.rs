//! The auxiliary panel: file tree, source viewer, diff panel, and change review.
//!
//! PRD §21 defines Change Review as a first-class completion surface. This is
//! the panel that opens on demand to the right of the conversation: the file
//! tree when nothing is open, a source file when the user opens one, a diff
//! during edits, and the change review surface on completion.
//!
//! PRD §15: code and changes sit beside the conversation, not below it, and
//! they are hidden until the user asks — the conversation owns the centre.

use egui::{Align, Color32, Layout, RichText, ScrollArea, Sense, Ui};
use std::collections::BTreeMap;
use std::path::PathBuf;

use super::primitives;
use super::{CodePanel, PurrCodeIde};
use crate::daemon::{PanelKind, Request};
use crate::model::{ChangeScope, ChangedFile};
use crate::theme;

/// egui temp-data key for the changed file whose diff is open in the panel.
/// Lives in temp data (not the app struct) so this file owns its own UI state.
pub(crate) const SELECTED_DIFF_ID: &str = "purrcode_changes_selected_diff";

impl PurrCodeIde {
    /// The right-hand code/changes column.
    ///
    /// When no file is open, the changes panel is the default (FR-C3): a
    /// repository with uncommitted work must show its per-file list on first
    /// open, without the user having to have selected a changed-file row. The
    /// file tree follows below an unavailable statement, never instead of it.
    pub(crate) fn code_column(&mut self, ui: &mut Ui) {
        match self.code_column_kind() {
            CodePanel::Changes => self.changes_panel(ui),
            CodePanel::Source => self.source_panel(ui),
        }
    }

    /// Which surface the code column shows this frame.
    ///
    /// `CodePanel::Source` falls back to the changes panel when no file is
    /// open (FR-C3): the panel titled "Changes" must show changes, not the
    /// file tree, and the file tree is only ever an explicit Explorer surface.
    pub(crate) fn code_column_kind(&self) -> CodePanel {
        column_kind(self.code_panel, !self.open_files.is_empty())
    }

    // ── File tree (default artifact when nothing is open) ─────────────

    fn code_file_tree(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            self.section_heading(ui, "Files");
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.label(
                    RichText::new("⌘2")
                        .monospace()
                        .small()
                        .color(self.tokens.text_muted),
                );
            });
        });

        let repository = self.repository.clone();
        if repository.as_os_str().is_empty() {
            ui.label(
                RichText::new("No folder opened")
                    .small()
                    .color(self.tokens.text_muted),
            );
            return;
        }
        let changed = self.changed_path_index();

        egui::ScrollArea::vertical()
            .id_salt("code_file_tree")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                self.render_tree_level(ui, &repository, &repository, 0, &changed);
            });
    }

    /// A repo-relative path index of changed files, built once per frame from
    /// both change sets (FR-C7).
    ///
    /// Keyed by the repository-relative path, not the basename: two files named
    /// `mod.rs` in different directories must not both light up when only one
    /// changed. `(None, None)` marks a file with no numstat (binary).
    pub(crate) fn changed_path_index(&self) -> BTreeMap<PathBuf, (Option<usize>, Option<usize>)> {
        changed_path_index_from(&self.workspace_changes, &self.session.changes)
    }

    pub(crate) fn render_tree_level(
        &mut self,
        ui: &mut Ui,
        dir: &std::path::Path,
        repository: &std::path::Path,
        depth: usize,
        changed: &BTreeMap<PathBuf, (Option<usize>, Option<usize>)>,
    ) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        let mut dirs = Vec::new();
        let mut files = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            // Skip hidden, target/, node_modules/, .git/
            if name.starts_with('.') || name == "target" || name == "node_modules" {
                continue;
            }
            if path.is_dir() {
                dirs.push((name, path));
            } else {
                files.push((name, path));
            }
        }
        dirs.sort_by(|a, b| a.0.cmp(&b.0));
        files.sort_by(|a, b| a.0.cmp(&b.0));

        for (name, path) in &dirs {
            let expanded = self.expanded.contains(path);
            let indent = (depth * 14) as f32;
            let width = ui.available_width().max(0.0);
            let (rect, response) =
                ui.allocate_exact_size(egui::vec2(width, theme::ROW_HEIGHT), Sense::click());
            if response.hovered() {
                ui.painter()
                    .rect_filled(rect, theme::RADIUS_CONTROL, self.tokens.surface_hover);
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            crate::icons::draw(
                ui,
                egui::Rect::from_center_size(
                    egui::pos2(rect.left() + indent + 6.0, rect.center().y),
                    egui::Vec2::splat(12.0),
                ),
                if expanded {
                    crate::icons::Glyph::FolderOpen
                } else {
                    crate::icons::Glyph::Folder
                },
                self.tokens.text_secondary,
            );
            ui.painter().text(
                egui::pos2(rect.left() + indent + 18.0, rect.center().y),
                egui::Align2::LEFT_CENTER,
                name,
                egui::FontId::proportional(theme::TYPE_META),
                self.tokens.text_secondary,
            );
            if response.clicked() {
                if expanded {
                    self.expanded.remove(path);
                } else {
                    self.expanded.insert(path.clone());
                }
            }
            if expanded {
                self.render_tree_level(ui, path, repository, depth + 1, changed);
            }
        }

        for (name, path) in &files {
            let indent = (depth * 14) as f32;
            // Mark a file changed by its repository-relative path (FR-C7). A
            // bare basename comparison would dot every `mod.rs` in the tree
            // when one of them changed.
            let relative = path.strip_prefix(repository).unwrap_or(path);
            let counts = changed.get(relative);
            let width = ui.available_width().max(0.0);
            let (rect, response) =
                ui.allocate_exact_size(egui::vec2(width, theme::ROW_HEIGHT), Sense::click());
            if response.hovered() {
                ui.painter()
                    .rect_filled(rect, theme::RADIUS_CONTROL, self.tokens.surface_hover);
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            if response.has_focus() {
                self.tokens.focus_ring(ui.painter(), rect);
            }
            crate::icons::draw(
                ui,
                egui::Rect::from_center_size(
                    egui::pos2(rect.left() + indent + 5.0, rect.center().y),
                    egui::Vec2::splat(11.0),
                ),
                crate::icons::Glyph::File,
                self.tokens.text_muted,
            );
            // A number, not a bullet: a changed file carries its real count so
            // the tree and the list agree (FR-C7, FR-C5). The additions read
            // in the added colour and the deletions in the removed colour,
            // never both in one. A file with only one numstat side still shows
            // that side. Right-aligned so every row's numbers share one edge.
            let counts_galley =
                |ui: &Ui, text: &str, color: egui::Color32| -> std::sync::Arc<egui::Galley> {
                    ui.painter().layout_no_wrap(
                        text.to_owned(),
                        egui::FontId::monospace(theme::TYPE_CODE),
                        color,
                    )
                };
            let name_font = egui::FontId::proportional(theme::TYPE_META);
            let name_color = if counts.is_some() {
                self.tokens.accent_primary
            } else {
                self.tokens.text_primary
            };
            // Reserve room for the counts, laid out right-to-left.
            let mut right = rect.right();
            if let Some((add, del)) = counts {
                if let Some(del) = del {
                    let galley =
                        counts_galley(ui, &format!("−{del}"), self.tokens.diff_removed_text);
                    right -= galley.size().x;
                    ui.painter().galley(
                        egui::pos2(right, rect.center().y - galley.size().y * 0.5),
                        galley,
                        self.tokens.diff_removed_text,
                    );
                    right -= 4.0;
                }
                if let Some(add) = add {
                    let galley = counts_galley(ui, &format!("+{add}"), self.tokens.diff_added_text);
                    right -= galley.size().x;
                    ui.painter().galley(
                        egui::pos2(right, rect.center().y - galley.size().y * 0.5),
                        galley,
                        self.tokens.diff_added_text,
                    );
                    right -= 6.0;
                }
            }
            let name_max = (right - (rect.left() + indent + 18.0)).max(24.0);
            let name_galley =
                primitives::fit_tail(ui, name, name_font.clone(), name_color, name_max);
            ui.painter().galley(
                egui::pos2(
                    rect.left() + indent + 18.0,
                    rect.center().y - name_galley.size().y * 0.5,
                ),
                name_galley,
                name_color,
            );
            if response.clicked() {
                // A changed file opens its diff (the same per-file view the
                // Changes list opens), so the user can see where the changes
                // are; an unchanged file opens the source.
                if counts.is_some() {
                    ui.ctx().data_mut(|data| {
                        data.insert_temp(
                            egui::Id::new(SELECTED_DIFF_ID),
                            relative.display().to_string(),
                        );
                    });
                    self.code_panel = CodePanel::Changes;
                    self.refresh_diff();
                } else {
                    self.open_file(path.clone());
                }
            }
        }
    }

    // ── Changes panel ───────────────────────────────────────────────

    fn changes_panel(&mut self, ui: &mut Ui) {
        // Clone both change sets up front so the rows below can borrow them
        // while the render methods also need `&mut self`.
        let workspace = self.workspace_changes.clone();
        let agent = self.session.changes.clone();

        // Header, VS Code source-control style: "Changes", a count, and the
        // aggregate `+A −D` right-aligned in the same fixed column the rows use.
        let (count_text, plus, minus) = if workspace.available {
            (
                Some(workspace.files_changed.to_string()),
                Some(format!("+{}", workspace.additions)),
                Some(format!("−{}", workspace.deletions)),
            )
        } else {
            // Not available yet. The header must not claim "still loading"
            // forever: only a check that is actually in flight shows the
            // spinner, and a genuinely unavailable set shows no counts at all
            // (its group below already states "Not checked yet").
            (None, None, None)
        };
        self.horizontal_header(
            ui,
            "Changes",
            count_text,
            plus,
            minus,
            self.workspace_changes_loading,
        );
        ui.add_space(4.0);

        // Two labelled groups (FR-C4): the user's own uncommitted changes and,
        // when a session is selected, the agent's worktree changes. A user
        // must never guess which tree a row describes.
        self.render_change_group(ui, "Your uncommitted changes", &workspace, true);
        if self.selected.is_some() {
            self.render_change_group(ui, "Agent changes", &agent, false);
        }

        // FR-C6: when the workspace was genuinely never checked (not a git
        // repo, or the check failed), the file tree follows *below* the
        // statement — never instead of it. While the first check is still in
        // flight, show a spinner instead of the tree so the panel never reads
        // as "no changes" before it has asked.
        if !workspace.available && !self.workspace_changes_loading {
            if self.workspace_changes_checked {
                ui.add_space(8.0);
                self.code_file_tree(ui);
            } else {
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(
                        RichText::new("Checking working tree…")
                            .small()
                            .color(self.tokens.text_muted),
                    );
                });
            }
        }

        ui.add_space(8.0);

        // Diff view. A selected row opens that file's diff in the same panel
        // (VS Code source-control style); the back control returns to the list.
        let diff = self.diff.clone();
        if self.diff_loading {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(
                    RichText::new("Loading diff…")
                        .small()
                        .color(self.tokens.text_muted),
                );
            });
        } else if let Some(diff) = diff {
            if diff.trim().is_empty() {
                ui.label(
                    RichText::new("No diff available for this scope")
                        .small()
                        .color(self.tokens.text_muted),
                );
            } else {
                let open_path = ui
                    .ctx()
                    .data(|data| data.get_temp::<String>(egui::Id::new(SELECTED_DIFF_ID)));
                if let Some(path) = open_path {
                    let file_diff = split_diff_for(&diff, &path);
                    self.render_diff_header(ui, &path, file_diff.as_deref());
                    self.render_diff(ui, file_diff.as_deref().unwrap_or(&diff), "aux_diff_scroll");
                } else {
                    self.render_diff(ui, &diff, "aux_diff_scroll");
                }
            }
        }
    }

    /// The header line of an open file's diff: a back control, the path, and
    /// the file's own right-aligned `+N −M`.
    fn render_diff_header(&self, ui: &mut Ui, path: &str, file_diff: Option<&str>) {
        let (plus, minus) = match file_diff {
            Some(_) => self.counts_for_path(path),
            None => (None, None),
        };
        let width = self.count_column_width(ui);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            let back = ui
                .small_button("← Back")
                .on_hover_cursor(egui::CursorIcon::PointingHand);
            if back.clicked() {
                ui.ctx().data_mut(|data| {
                    data.remove::<String>(egui::Id::new(SELECTED_DIFF_ID));
                });
            }
            ui.label(
                RichText::new(path)
                    .monospace()
                    .small()
                    .color(self.tokens.text_primary),
            );
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                self.right_counts(ui, width, plus.as_deref(), minus.as_deref());
            });
        });
        ui.add_space(4.0);
    }

    /// The `+N` / `−M` strings for one path, from the two change sets.
    fn counts_for_path(&self, path: &str) -> (Option<String>, Option<String>) {
        for changes in [&self.workspace_changes, &self.session.changes] {
            if let Some(entry) = changes.entries.iter().find(|entry| entry.path == path) {
                return (
                    entry
                        .additions
                        .filter(|add| *add > 0)
                        .map(|count| format!("+{count}")),
                    entry
                        .deletions
                        .filter(|del| *del > 0)
                        .map(|count| format!("−{count}")),
                );
            }
        }
        (None, None)
    }

    /// The width, in points, of the fixed right-aligned count column.
    ///
    /// The counts of every changed file must line up down the list, so the
    /// column is sized once per frame from the widest number in the two change
    /// sets rather than per row. The width is the widest `+N −M` cell measured
    /// in the live monospace face, so the header and every row share one edge.
    fn count_column_width(&self, ui: &Ui) -> f32 {
        let cells: [String; 4] = [
            format!("+{}", self.workspace_changes.additions),
            format!("−{}", self.workspace_changes.deletions),
            format!("+{}", self.session.changes.additions),
            format!("−{}", self.session.changes.deletions),
        ];
        let font = egui::FontId::monospace(theme::TYPE_CODE);
        let widest = cells
            .iter()
            .map(|cell| {
                ui.fonts_mut(|fonts| {
                    fonts
                        .layout_no_wrap(cell.clone(), font.clone(), Color32::PLACEHOLDER)
                        .size()
                        .x
                })
            })
            .fold(0.0_f32, f32::max);
        // The `+N` and `−M` labels sit 4pt apart inside the cell.
        widest.max(theme::TYPE_CODE * 1.2 * 5.0) + 4.0
    }

    /// Draw the right-aligned `+N −M` counters and pad the rest of the fixed
    /// count column, so every row's counters share one right edge.
    ///
    /// In a right-to-left layout the first widget lands on the right edge, so
    /// the `−M` is added before the `+N` to read `+N −M` with the minus flush
    /// against the column.
    fn right_counts(&self, ui: &mut Ui, width: f32, plus: Option<&str>, minus: Option<&str>) {
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            if let Some(minus) = minus {
                ui.label(
                    RichText::new(minus)
                        .monospace()
                        .small()
                        .color(self.tokens.diff_removed_text),
                );
            }
            if let Some(plus) = plus {
                ui.label(
                    RichText::new(plus)
                        .monospace()
                        .small()
                        .color(self.tokens.diff_added_text),
                );
            }
        });
        let used = ui.min_rect().width();
        ui.allocate_space(egui::vec2((width - used).max(0.0), 0.0));
    }

    /// The Changes panel header, VS Code source-control style: the title, a
    /// count beside it, and the aggregate `+A −D` right-aligned in the same
    /// fixed column the rows use.
    fn horizontal_header(
        &mut self,
        ui: &mut Ui,
        title: &str,
        count: Option<String>,
        plus: Option<String>,
        minus: Option<String>,
        loading: bool,
    ) {
        let width = self.count_column_width(ui);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            ui.label(
                RichText::new(title)
                    .size(theme::TYPE_BODY)
                    .strong()
                    .color(self.tokens.text_primary),
            );
            if let Some(count) = count {
                ui.label(
                    RichText::new(count)
                        .size(theme::TYPE_BODY)
                        .color(self.tokens.text_secondary),
                );
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if plus.is_some() || minus.is_some() {
                    self.right_counts(ui, width, plus.as_deref(), minus.as_deref());
                } else if loading {
                    ui.spinner();
                }
            });
        });
    }

    /// One labelled change-set group: its aggregate summary, then a row per
    /// file. An unavailable set is stated and retryable, never silently
    /// replaced by something else.
    fn render_change_group(
        &mut self,
        ui: &mut Ui,
        label: &str,
        changes: &crate::model::Changes,
        is_workspace: bool,
    ) {
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(label.to_ascii_uppercase())
                    .size(theme::TYPE_EYEBROW)
                    .strong()
                    .color(self.tokens.text_muted),
            );
            if changes.available && changes.files_changed > 0 {
                ui.label(
                    RichText::new(changes.summary())
                        .size(theme::TYPE_EYEBROW)
                        .color(self.tokens.text_muted.gamma_multiply(0.85)),
                );
            }
        });
        ui.add_space(4.0);

        if !changes.available {
            // "Unavailable" and "zero" are different facts (FR-C6): say the
            // check never ran and offer a retry.
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("Not checked yet")
                        .small()
                        .color(self.tokens.text_muted),
                );
                if ui.small_button("Retry").clicked() {
                    self.retry_change_set(is_workspace);
                }
            });
            return;
        }
        if changes.files_changed == 0 {
            ui.label(
                RichText::new("No changes")
                    .small()
                    .color(self.tokens.text_muted),
            );
            return;
        }

        let entries = changes.entries.clone();
        let scope = changes.scope;
        for entry in &entries {
            self.changed_file_row(ui, entry, scope);
        }
    }

    /// Re-ask for one change set. The workspace set re-runs the workspace
    /// route; a session's agent set retries the `Changes` panel, which is the
    /// transport's dedicated retry path.
    fn retry_change_set(&mut self, is_workspace: bool) {
        if is_workspace {
            self.request_workspace_changes();
        } else if let Some(session) = self.selected.clone() {
            self.client.send(Request::RetryPanel {
                session,
                panel: PanelKind::Changes,
                scope: self.diff_scope.slug(),
            });
        }
    }

    /// One changed-file row, VS Code source-control style: a status letter
    /// column, the repo-relative path in monospace, and the `+N −M` counts
    /// right-aligned in the shared count column. Clicking the row opens that
    /// file's diff in the same panel.
    pub(crate) fn changed_file_row(
        &mut self,
        ui: &mut Ui,
        entry: &ChangedFile,
        scope: ChangeScope,
    ) {
        let tokens = self.tokens;
        let status_color = match entry.status {
            'M' => tokens.status_info,
            'A' => tokens.status_success,
            'D' => tokens.status_error,
            'R' => tokens.status_warning,
            _ => tokens.text_secondary,
        };

        // A file is "binary" only when numstat gave no counts at all. A 0/0
        // row (rename, mode change) is a real, countable change set, not a
        // binary blob.
        let binary = entry.additions.is_none() && entry.deletions.is_none();
        let (plus, minus) = match (entry.additions, entry.deletions) {
            (None, None) => (None, None),
            (add, del) => (
                add.filter(|add| *add > 0).map(|add| format!("+{add}")),
                del.filter(|del| *del > 0).map(|del| format!("−{del}")),
            ),
        };
        let count_width = self.count_column_width(ui);
        let mono = egui::FontId::monospace(theme::TYPE_CODE);

        // Hand-painted row (the layout primitives own their text, which would
        // double-draw the path): allocate the hit area, hover fill, then paint
        // the status letter, the path and the right-aligned counts.
        let width = ui.available_width().max(0.0);
        let (rect, response) =
            ui.allocate_exact_size(egui::vec2(width, theme::ROW_HEIGHT), Sense::click());
        let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
        let hovered = response.hovered();
        if hovered {
            ui.painter()
                .rect_filled(rect, theme::RADIUS_CONTROL, tokens.surface_hover);
        }
        if response.has_focus() {
            tokens.focus_ring(ui.painter(), rect);
        }
        let center_y = rect.center().y;

        // Status letter, fixed at the column's left edge.
        let status =
            ui.painter()
                .layout_no_wrap(entry.status.to_string(), mono.clone(), status_color);
        ui.painter().galley(
            egui::pos2(rect.left() + 8.0, center_y - status.size().y * 0.5),
            status,
            status_color,
        );

        // Repo-relative path, elided to the space left before the count column.
        let path = &entry.path;
        let path_color = if hovered {
            tokens.accent_primary
        } else {
            tokens.text_primary
        };
        let path_max = (rect.width() - 16.0 - count_width - 22.0).max(24.0);
        let path_galley = primitives::fit_tail(ui, path, mono.clone(), path_color, path_max);
        ui.painter().galley(
            egui::pos2(rect.left() + 24.0, center_y - path_galley.size().y * 0.5),
            path_galley,
            path_color,
        );

        // Counts, right-aligned in the fixed count column. The `+N −M` pair
        // is laid out so the `−M` sits flush against the column's right edge
        // and `+N` sits 4pt to its left, whatever the digit widths happen to
        // be — every row shares one right edge.
        if binary {
            let label =
                ui.painter()
                    .layout_no_wrap("binary".to_owned(), mono.clone(), tokens.text_muted);
            ui.painter().galley(
                egui::pos2(
                    rect.right() - label.size().x,
                    center_y - label.size().y * 0.5,
                ),
                label,
                tokens.text_muted,
            );
        } else {
            let right_edge = rect.right();
            let minus_galley = minus.as_ref().map(|text| {
                ui.painter()
                    .layout_no_wrap(text.clone(), mono.clone(), tokens.diff_removed_text)
            });
            let plus_galley = plus.as_ref().map(|text| {
                ui.painter()
                    .layout_no_wrap(text.clone(), mono.clone(), tokens.diff_added_text)
            });
            let minus_w = minus_galley.as_ref().map_or(0.0, |g| g.size().x);
            if let Some(minus_galley) = &minus_galley {
                ui.painter().galley(
                    egui::pos2(
                        right_edge - minus_galley.size().x,
                        center_y - minus_galley.size().y * 0.5,
                    ),
                    minus_galley.clone(),
                    tokens.diff_removed_text,
                );
            }
            if let Some(plus_galley) = &plus_galley {
                let left = right_edge - minus_w - 4.0 - plus_galley.size().x;
                ui.painter().galley(
                    egui::pos2(left, center_y - plus_galley.size().y * 0.5),
                    plus_galley.clone(),
                    tokens.diff_added_text,
                );
            }
        }

        if response.clicked() {
            ui.ctx().data_mut(|data| {
                data.insert_temp(egui::Id::new(SELECTED_DIFF_ID), entry.path.clone());
            });
            self.code_panel = CodePanel::Changes;
            self.refresh_diff_for_scope(scope);
        }
    }

    /// The colored diff viewer: a left gutter of real file line numbers parsed
    /// from the `@@` hunk headers, removed lines tinted red, added lines green.
    /// `diff --git`, `---`, `+++` and `@@` headers are not rendered as rows —
    /// they drive the line-number computation and stay hidden. Between hunks a
    /// faint separator marks the jump instead of showing the `@@` text.
    pub(crate) fn render_diff(&self, ui: &mut Ui, diff: &str, id_salt: &str) {
        let tokens = self.tokens;
        let hunks = parse_diff_hunks(diff);
        egui::Frame::new()
            .fill(tokens.background_secondary)
            .corner_radius(theme::RADIUS_CONTROL)
            .inner_margin(egui::Margin::symmetric(0, 0))
            .show(ui, |ui| {
                ScrollArea::both()
                    .id_salt(id_salt)
                    .auto_shrink([false, false])
                    .max_height(ui.available_height())
                    .show(ui, |ui| {
                        ui.set_min_width(ui.available_width());
                        let mono = egui::FontId::monospace(theme::TYPE_CODE);
                        let digit_w = ui.fonts_mut(|fonts| fonts.glyph_width(&mono, 'M'));
                        let content_pad = 8.0;
                        // A fixed gutter sized to the widest line number in the
                        // diff, so every row's numbers share one right edge.
                        let max_no = hunks
                            .iter()
                            .flat_map(|hunk| hunk.lines.iter())
                            .filter_map(|line| line.new_no.or(line.old_no))
                            .max()
                            .unwrap_or(1);
                        let gutter_w = (digit_w * max_no.to_string().len() as f32 + 14.0)
                            .max(digit_w * 4.0 + 12.0);

                        for (hunk_index, hunk) in hunks.iter().enumerate() {
                            if hunk_index > 0 {
                                // A small gap and a faint separator take the
                                // place of the `@@` header line.
                                ui.add_space(6.0);
                                let row_w = ui.available_width().max(0.0);
                                let (rect, _) =
                                    ui.allocate_exact_size(egui::vec2(row_w, 1.0), Sense::hover());
                                ui.painter().line_segment(
                                    [
                                        egui::pos2(rect.left(), rect.center().y),
                                        egui::pos2(rect.right(), rect.center().y),
                                    ],
                                    egui::Stroke::new(
                                        1.0_f32,
                                        tokens.text_muted.gamma_multiply(0.4),
                                    ),
                                );
                                ui.add_space(6.0);
                            }

                            for line in &hunk.lines {
                                let sign = match line.kind {
                                    LineKind::Added => '+',
                                    LineKind::Removed => '-',
                                    LineKind::Context => ' ',
                                };
                                let text_color = match line.kind {
                                    LineKind::Added => tokens.diff_added_text,
                                    LineKind::Removed => tokens.diff_removed_text,
                                    LineKind::Context => tokens.text_primary,
                                };
                                // Removed lines show the OLD number, added lines
                                // the NEW number; context shows the new-file
                                // number.
                                let number = line
                                    .new_no
                                    .or(line.old_no)
                                    .map(|n| n.to_string())
                                    .unwrap_or_default();
                                let has_number = !number.is_empty();
                                // The diff body already carries the sign glyph;
                                // the parsed text is the body after it, so the
                                // two are rejoined verbatim.
                                let mut text = sign.to_string();
                                text.push_str(&line.text);
                                let (galley, num_galley) = ui.fonts_mut(|fonts| {
                                    (
                                        fonts.layout_no_wrap(text, mono.clone(), text_color),
                                        fonts.layout_no_wrap(
                                            number,
                                            mono.clone(),
                                            tokens.text_muted,
                                        ),
                                    )
                                });
                                let row_h = galley.size().y.max(18.0);
                                let row_w = ui.available_width().max(0.0);
                                let (rect, _) = ui
                                    .allocate_exact_size(egui::vec2(row_w, row_h), Sense::hover());
                                // Row background tint for changed lines.
                                match line.kind {
                                    LineKind::Added => {
                                        ui.painter().rect_filled(rect, 0.0, tokens.diff_added);
                                    }
                                    LineKind::Removed => {
                                        ui.painter().rect_filled(rect, 0.0, tokens.diff_removed);
                                    }
                                    LineKind::Context => {}
                                }
                                // Line number, right-aligned in the fixed gutter.
                                if has_number {
                                    ui.painter().galley(
                                        egui::pos2(
                                            rect.left() + gutter_w - 6.0 - num_galley.size().x,
                                            rect.center().y - num_galley.size().y * 0.5,
                                        ),
                                        num_galley,
                                        tokens.text_muted,
                                    );
                                }
                                // The sign column and content, in the line colour.
                                ui.painter().galley(
                                    egui::pos2(
                                        rect.left() + gutter_w + content_pad,
                                        rect.center().y - galley.size().y * 0.5,
                                    ),
                                    galley,
                                    text_color,
                                );
                            }
                        }
                    });
            });
    }

    // ── Source panel ────────────────────────────────────────────────

    pub(crate) fn source_panel(&mut self, ui: &mut Ui) {
        // Clone needed info to avoid borrow checker issues
        let active_idx = self.active_file;
        let open_files: Vec<_> = self
            .open_files
            .iter()
            .map(|f| (f.label.clone(), f.body.clone()))
            .collect();
        let Some((_label, body)) = open_files.get(active_idx) else {
            self.code_file_tree(ui);
            return;
        };

        // Tab bar — same visual language as the editor strip.
        ui.horizontal(|ui| {
            for (i, (label, _)) in open_files.iter().enumerate() {
                let is_active = i == active_idx;
                let (clicked, closed) = super::editor::tab(
                    ui,
                    &self.tokens,
                    Some(super::editor::TabIcon::Glyph(crate::icons::Glyph::File)),
                    label,
                    is_active,
                    self.dirty.contains(&self.open_files[i].path),
                    true,
                    ("source_tab", i),
                );
                if clicked {
                    self.active_file = i;
                }
                if closed {
                    self.close_file(i);
                }
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.small_button("× Close all").clicked() {
                    // Dirty files go through the same confirm-as-you-go path
                    // as the per-tab "×"; clean files close immediately.
                    let first_dirty = self
                        .open_files
                        .iter()
                        .position(|file| self.dirty.contains(&file.path));
                    match first_dirty {
                        Some(index) => self.pending_close = Some(index),
                        None => {
                            self.open_files.clear();
                            self.dirty.clear();
                            self.code_panel = CodePanel::Source;
                        }
                    }
                }
            });
        });
        ui.add_space(4.0);

        // Content
        match body {
            Ok(content) => {
                egui::Frame::new()
                    .fill(self.tokens.background_secondary)
                    .corner_radius(theme::RADIUS_CONTROL)
                    .inner_margin(egui::Margin::same(0))
                    .show(ui, |ui| {
                        ScrollArea::both()
                            .id_salt(format!("source_{active_idx}"))
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                ui.set_min_width(ui.available_width());
                                ui.monospace(content);
                            });
                    });
            }
            Err(err) => {
                ui.vertical_centered(|ui| {
                    ui.add_space(24.0);
                    ui.label(
                        RichText::new(format!("Could not read file: {err}"))
                            .color(self.tokens.status_error),
                    );
                });
            }
        }
    }

    pub(crate) fn close_file(&mut self, index: usize) {
        if index >= self.open_files.len() {
            return;
        }
        let path = self.open_files[index].path.clone();
        // A file with unsaved edits is not silently discarded (impeccable
        // triage rank 1: data loss). The tab's "×" routes through the same
        // confirm as every other close so there is exactly one discard path.
        if self.dirty.contains(&path) {
            self.pending_close = Some(index);
            return;
        }
        self.open_files.remove(index);
        self.dirty.remove(&path);
        if self.active_file >= self.open_files.len() && self.active_file > 0 {
            self.active_file = self.open_files.len() - 1;
        }
    }

    /// A file close is waiting on the "discard changes?" modal; render it and
    /// apply the choice.
    pub(crate) fn close_confirm_modal(&mut self, ctx: &egui::Context) {
        let Some(index) = self.pending_close else {
            return;
        };
        let Some(file) = self.open_files.get(index) else {
            self.pending_close = None;
            return;
        };
        let label = file.label.clone();
        let mut decision: Option<bool> = None;
        egui::Modal::new(egui::Id::new("purrcode_discard_modal")).show(ctx, |ui| {
            ui.set_min_width(360.0);
            ui.label(
                RichText::new("Discard changes?")
                    .strong()
                    .color(self.tokens.text_primary),
            );
            ui.add_space(6.0);
            ui.label(
                RichText::new(format!("{label} has unsaved edits. Close without saving?"))
                    .color(self.tokens.text_secondary),
            );
            ui.add_space(14.0);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                if ui.button("Save").clicked() {
                    decision = Some(false);
                }
                if ui
                    .button(RichText::new("Discard").color(self.tokens.status_error))
                    .clicked()
                {
                    decision = Some(true);
                }
            });
        });
        match decision {
            Some(true) => {
                // The modal consumed the click; force the close without
                // re-asking, then clear the pending index.
                self.pending_close = None;
                let path = self.open_files[index].path.clone();
                self.open_files.remove(index);
                self.dirty.remove(&path);
                if self.active_file >= self.open_files.len() && self.active_file > 0 {
                    self.active_file = self.open_files.len() - 1;
                }
            }
            Some(false) => {
                // Save, then close.
                let path = self.open_files[index].path.clone();
                self.pending_close = None;
                self.save_file(index);
                self.dirty.remove(&path);
                self.open_files.remove(index);
                if self.active_file >= self.open_files.len() && self.active_file > 0 {
                    self.active_file = self.open_files.len() - 1;
                }
            }
            None => {}
        }
    }
}

/// A unified-diff body line and its file line numbers, as computed from the
/// `@@` hunk header.
#[derive(Clone, Debug, Eq, PartialEq)]
struct HunkLine {
    kind: LineKind,
    /// The old-file line number, `Some` for context and removed lines.
    old_no: Option<usize>,
    /// The new-file line number, `Some` for context and added lines.
    new_no: Option<usize>,
    /// The line body without the leading `+`/`-`/space sign glyph.
    text: String,
}

/// A contiguous run of unified-diff body lines starting at a `@@` hunk header.
#[derive(Clone, Debug, Eq, PartialEq)]
struct Hunk {
    old_start: usize,
    new_start: usize,
    lines: Vec<HunkLine>,
}

/// How a unified-diff line should be tinted and numbered.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LineKind {
    /// A `+` line: green tint, new-file line number.
    Added,
    /// A `-` line: red tint, old-file line number.
    Removed,
    /// A context line: neutral, carries both old and new line numbers.
    Context,
}

/// Parse a unified diff into hunks with real old/new line numbers.
///
/// Each `@@ -A,B +C,D @@` header seeds the old/new counters; the body is then
/// walked line by line: context (leading space) sets both numbers, `-` sets
/// the old number, `+` sets the new number. The `---`/`+++` file labels that
/// sit between the header and the body are skipped, as are `diff --git`,
/// `index` and rename headers, which drive nothing here. A diff with no `@@`
/// header degrades to a single pseudo-hunk whose lines carry no numbers.
fn parse_diff_hunks(diff: &str) -> Vec<Hunk> {
    let mut hunks: Vec<Hunk> = Vec::new();
    // The current hunk's header start, set from the most recent `@@` line.
    let mut old_start: usize = 0;
    let mut new_start: usize = 0;
    // The running counters, `None` before any `@@` header is seen.
    let mut old_no: Option<usize> = None;
    let mut new_no: Option<usize> = None;
    let mut lines: Vec<HunkLine> = Vec::new();

    // A file label (`---`/`+++`), a `diff --git`/`index`/rename header or a
    // trailing blank line is not body content.
    fn is_structural(line: &str) -> bool {
        line.starts_with("+++ ")
            || line.starts_with("--- ")
            || line.starts_with("diff --git ")
            || line.starts_with("index ")
            || line.starts_with("new file mode ")
            || line.starts_with("deleted file mode ")
            || line.starts_with("old mode ")
            || line.starts_with("new mode ")
            || line.starts_with("similarity index ")
            || line.starts_with("rename from ")
            || line.starts_with("rename to ")
            || line.starts_with("copy from ")
            || line.starts_with("copy to ")
            || line.is_empty()
    }

    for line in diff.lines() {
        if let Some(header) = line.strip_prefix("@@") {
            // A new hunk starts: flush the previous one with its own header
            // start, then seed the counters from this `@@` header.
            if !lines.is_empty() {
                hunks.push(Hunk {
                    old_start,
                    new_start,
                    lines: std::mem::take(&mut lines),
                });
            }
            old_no = None;
            new_no = None;
            if let Some((old, new)) = parse_hunk_header(header) {
                old_start = old;
                new_start = new;
                old_no = Some(old);
                new_no = Some(new);
            }
            continue;
        }
        if is_structural(line) {
            continue;
        }

        let (kind, text) = match line.chars().next() {
            Some(' ') => (LineKind::Context, &line[1..]),
            Some('-') => (LineKind::Removed, &line[1..]),
            Some('+') => (LineKind::Added, &line[1..]),
            // Outside any hunk (no `@@` header seen) a bare text line is
            // treated as context so the diff still renders.
            _ if old_no.is_none() && new_no.is_none() => (LineKind::Context, line),
            _ => continue,
        };
        // Walk the counters per kind: context advances both files, `-` only
        // the old file, `+` only the new file. A body line with no active hunk
        // (no `@@` header seen) still renders, just without line numbers.
        let old = match kind {
            LineKind::Removed | LineKind::Context => old_no.map(|n| {
                let current = n;
                old_no = Some(n + 1);
                current
            }),
            LineKind::Added => None,
        };
        let new = match kind {
            LineKind::Added | LineKind::Context => new_no.map(|n| {
                let current = n;
                new_no = Some(n + 1);
                current
            }),
            LineKind::Removed => None,
        };
        lines.push(HunkLine {
            kind,
            old_no: old,
            new_no: new,
            text: text.to_owned(),
        });
    }
    if !lines.is_empty() {
        hunks.push(Hunk {
            old_start,
            new_start,
            lines,
        });
    }
    hunks
}

/// Parse `@@ -A,B +C,D @@ ...` into `(old_start, new_start)`.
fn parse_hunk_header(rest: &str) -> Option<(usize, usize)> {
    let (old, new) = rest.split_once(" +")?;
    let old = old.strip_prefix(" -")?;
    let old_start = old.split(',').next()?;
    let new_start = new.split([',', ' ']).next()?;
    Some((old_start.parse().ok()?, new_start.parse().ok()?))
}

/// Extract one file's section from a multi-file unified patch.
///
/// The patch is split on `diff --git a/<path> b/<path>` headers. When the
/// patch has exactly one file, the whole patch is returned. Otherwise the
/// section whose post-image path matches `path` is returned (handling
/// `a/x b/new-name` renames), with the header line kept so the diff still
/// reads as a patch. `None` when no section matches.
fn split_diff_for(patch: &str, path: &str) -> Option<String> {
    let mut sections: Vec<(String, Vec<&str>)> = Vec::new();
    let mut current: Option<(String, Vec<&str>)> = None;
    for line in patch.lines() {
        if let Some(header) = diff_header_path(line) {
            if let Some(section) = current.take() {
                sections.push(section);
            }
            current = Some((header, vec![line]));
        } else if let Some(section) = current.as_mut() {
            section.1.push(line);
        }
    }
    if let Some(section) = current.take() {
        sections.push(section);
    }

    if sections.len() == 1 {
        // A one-file patch is that file's diff — but only if it is the file
        // the caller asked for. Returning the whole patch for a different
        // requested path would show another file's content under the clicked
        // file's header.
        let (header, _) = &sections[0];
        return (header == path).then(|| patch.to_owned());
    }
    sections
        .into_iter()
        .find(|(header, _)| header == path)
        .map(|(_, lines)| lines.join("\n"))
}

/// Extract the post-image path from a `diff --git a/x b/y` header.
fn diff_header_path(line: &str) -> Option<String> {
    let remainder = line.strip_prefix("diff --git ")?;
    let (_, post) = remainder.rsplit_once(" b/")?;
    Some(post.to_owned())
}

/// Build the repo-relative changed-path index from both change sets (FR-C7).
///
/// Keyed by path, not basename, so two files named `mod.rs` in different
/// directories do not both light up when only one changed. Each side is kept
/// independently (`None` when that side has no numstat count), so a file with
/// only additions still shows its `+N` in the tree — the Changes list and the
/// tree must agree (FR-C7/FR-C5).
fn changed_path_index_from(
    workspace: &crate::model::Changes,
    session: &crate::model::Changes,
) -> BTreeMap<PathBuf, (Option<usize>, Option<usize>)> {
    let mut index = BTreeMap::new();
    for changes in [workspace, session] {
        for entry in &changes.entries {
            index.insert(
                PathBuf::from(&entry.path),
                (entry.additions, entry.deletions),
            );
        }
    }
    index
}

/// Which surface the code column shows, given the panel mode and whether any
/// file is open. Extracted so FR-C3's dispatch is testable without a window.
fn column_kind(panel: CodePanel, any_file_open: bool) -> CodePanel {
    match panel {
        CodePanel::Changes => CodePanel::Changes,
        CodePanel::Source => {
            if any_file_open {
                CodePanel::Source
            } else {
                CodePanel::Changes
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ChangedFile, Changes};

    fn changed_file(
        path: &str,
        status: char,
        additions: Option<usize>,
        deletions: Option<usize>,
    ) -> ChangedFile {
        ChangedFile {
            path: path.to_owned(),
            status,
            additions,
            deletions,
        }
    }

    fn changes_with(entries: Vec<ChangedFile>) -> Changes {
        Changes {
            available: true,
            files_changed: entries.len(),
            entries,
            ..Changes::default()
        }
    }

    #[test]
    fn code_column_defaults_to_changes_when_no_file_is_open() {
        // FR-C3: with no file open and the panel on `Source`, the changes
        // panel is what renders — the panel titled "Changes" must not silently
        // show the file tree.
        assert_eq!(column_kind(CodePanel::Source, false), CodePanel::Changes);
        assert_eq!(column_kind(CodePanel::Source, true), CodePanel::Source);
        assert_eq!(column_kind(CodePanel::Changes, false), CodePanel::Changes);
        assert_eq!(column_kind(CodePanel::Changes, true), CodePanel::Changes);
    }

    #[test]
    fn two_files_named_mod_rs_mark_only_the_changed_one() {
        // FR-C7: one changed `a/mod.rs` must not light up `b/mod.rs` too.
        let workspace = changes_with(vec![changed_file("a/mod.rs", 'M', Some(3), Some(1))]);
        let session = Changes::default();
        let index = changed_path_index_from(&workspace, &session);
        assert_eq!(
            index.get(PathBuf::from("a/mod.rs").as_path()),
            Some(&(Some(3), Some(1)))
        );
        assert!(
            !index.contains_key(PathBuf::from("b/mod.rs").as_path()),
            "a same-named file in another directory must not be marked"
        );
        assert_eq!(index.len(), 1);
    }

    #[test]
    fn a_file_with_no_numstat_is_marked_binary_not_missing() {
        // FR-C5: an entry with neither count still appears in the index, so
        // the tree shows `binary` rather than silently nothing.
        let workspace = changes_with(vec![changed_file("c.bin", 'M', None, None)]);
        let index = changed_path_index_from(&workspace, &Changes::default());
        assert_eq!(
            index.get(PathBuf::from("c.bin").as_path()),
            Some(&(None, None))
        );
    }

    #[test]
    fn a_single_sided_numstat_survives_in_the_index() {
        // FR-C7/FR-C5: a file with only additions (no deletions reported) must
        // still show its `+N` in the tree, matching the Changes-list row.
        let workspace = changes_with(vec![changed_file("d.rs", 'A', Some(3), None)]);
        let index = changed_path_index_from(&workspace, &Changes::default());
        assert_eq!(
            index.get(PathBuf::from("d.rs").as_path()),
            Some(&(Some(3), None))
        );
    }

    #[test]
    fn split_diff_for_returns_the_matching_section_of_a_multifile_patch() {
        let patch = "\
diff --git a/src/lib.rs b/src/lib.rs
index 111..222 100644
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -1,3 +1,4 @@
 fn one() {}
+fn two() {}
diff --git a/src/other.rs b/src/other.rs
--- a/src/other.rs
+++ b/src/other.rs
@@ -1,1 +1,1 @@
-old
+new
";
        let got = split_diff_for(patch, "src/lib.rs").expect("the section must be found");
        assert!(got.starts_with("diff --git a/src/lib.rs b/src/lib.rs"));
        assert!(got.contains("+fn two() {}"));
        assert!(!got.contains("src/other.rs"));
        assert!(!got.contains("+new"));
    }

    #[test]
    fn split_diff_for_returns_the_whole_single_file_patch_only_for_its_own_path() {
        let patch = "\
diff --git a/src/lib.rs b/src/lib.rs
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -1 +1 @@
-old
+new
";
        // A one-file patch is returned whole when it is the requested file…
        assert_eq!(split_diff_for(patch, "src/lib.rs").as_deref(), Some(patch));
        // …and `None` for any other path, so a workspace file that is absent
        // from the agent diff never shows another file's content under its
        // header.
        assert_eq!(split_diff_for(patch, "src/other.rs"), None);
    }

    #[test]
    fn split_diff_for_matches_a_rename_via_the_post_image_path() {
        // `a/old.rs b/new.rs` should be found under its new name. (A one-file
        // patch matches only its own path; this multi-section patch exercises
        // the rename lookup.)
        let patch = "\
diff --git a/old.rs b/new.rs
similarity index 100%
rename from old.rs
rename to new.rs
@@ -1,1 +1,1 @@
-old
+new
diff --git a/src/other.rs b/src/other.rs
--- a/src/other.rs
+++ b/src/other.rs
@@ -1,1 +1,1 @@
-old
+new
";
        let got = split_diff_for(patch, "new.rs").expect("the renamed section must be found");
        assert!(got.starts_with("diff --git a/old.rs b/new.rs"));
        assert!(split_diff_for(patch, "old.rs").is_none());
    }

    #[test]
    fn split_diff_for_is_none_when_no_section_matches() {
        let patch = "\
diff --git a/a.rs b/a.rs
--- a/a.rs
+++ b/a.rs
@@ -1 +1 @@
-old
+new
diff --git a/b.rs b/b.rs
--- a/b.rs
+++ b/b.rs
@@ -1 +1 @@
-old
+new
";
        assert!(split_diff_for(patch, "missing.rs").is_none());
    }

    #[test]
    fn parse_diff_hunks_simple_hunk_numbers_old_and_new() {
        // `@@ -1,3 +1,4 @@` seeds old at 1, new at 1: the context line is old 1
        // / new 1, the removed line old 2, the added lines new 2 and 3.
        let diff = "\
@@ -1,3 +1,4 @@
 fn one() {}
-let old = 1;
+let new = 2;
+let another = 3;
";
        let hunks = parse_diff_hunks(diff);
        assert_eq!(hunks.len(), 1);
        let hunk = &hunks[0];
        assert_eq!((hunk.old_start, hunk.new_start), (1, 1));
        assert_eq!(hunk.lines.len(), 4);
        assert_eq!(hunk.lines[0].kind, LineKind::Context);
        assert_eq!(
            (hunk.lines[0].old_no, hunk.lines[0].new_no),
            (Some(1), Some(1))
        );
        assert_eq!(hunk.lines[0].text, "fn one() {}");
        assert_eq!(hunk.lines[1].kind, LineKind::Removed);
        assert_eq!(
            (hunk.lines[1].old_no, hunk.lines[1].new_no),
            (Some(2), None)
        );
        assert_eq!(hunk.lines[1].text, "let old = 1;");
        assert_eq!(hunk.lines[2].kind, LineKind::Added);
        assert_eq!(
            (hunk.lines[2].old_no, hunk.lines[2].new_no),
            (None, Some(2))
        );
        assert_eq!(hunk.lines[2].text, "let new = 2;");
        assert_eq!(hunk.lines[3].kind, LineKind::Added);
        assert_eq!(
            (hunk.lines[3].old_no, hunk.lines[3].new_no),
            (None, Some(3))
        );
        assert_eq!(hunk.lines[3].text, "let another = 3;");
    }

    #[test]
    fn parse_diff_hunks_pure_additions_advance_only_new() {
        // `@@ -4,0 +4,2 @@` with no context lines: only the new counter moves.
        let diff = "\
@@ -4,0 +4,2 @@
+fn added_a() {}
+fn added_b() {}
";
        let hunks = parse_diff_hunks(diff);
        assert_eq!(hunks.len(), 1);
        let hunk = &hunks[0];
        assert_eq!((hunk.old_start, hunk.new_start), (4, 4));
        assert_eq!(hunk.lines.len(), 2);
        assert_eq!(
            (hunk.lines[0].old_no, hunk.lines[0].new_no),
            (None, Some(4))
        );
        assert_eq!(
            (hunk.lines[1].old_no, hunk.lines[1].new_no),
            (None, Some(5))
        );
    }

    #[test]
    fn parse_diff_hunks_without_header_does_not_panic() {
        // A bare patch with no `@@` line renders as unnumbered rows rather
        // than crashing.
        let hunks = parse_diff_hunks("fn one() {}\nfn two() {}\n");
        assert_eq!(hunks.len(), 1);
        assert_eq!(hunks[0].lines.len(), 2);
        assert_eq!(hunks[0].lines[0].kind, LineKind::Context);
        assert_eq!(
            (hunks[0].lines[0].old_no, hunks[0].lines[0].new_no),
            (None, None)
        );
        assert_eq!(parse_diff_hunks("").len(), 0);
    }

    #[test]
    fn parse_diff_hunks_multi_hunk_restarts_counters() {
        // Each hunk numbers itself from its own `@@` header.
        let diff = "\
@@ -10,1 +12,1 @@
 ctx
@@ -20,1 +22,1 @@
-removed
+added
";
        let hunks = parse_diff_hunks(diff);
        assert_eq!(hunks.len(), 2);
        assert_eq!((hunks[0].old_start, hunks[0].new_start), (10, 12));
        assert_eq!(
            (hunks[0].lines[0].old_no, hunks[0].lines[0].new_no),
            (Some(10), Some(12))
        );
        assert_eq!((hunks[1].old_start, hunks[1].new_start), (20, 22));
        assert_eq!(
            (hunks[1].lines[0].old_no, hunks[1].lines[0].new_no),
            (Some(20), None)
        );
        assert_eq!(
            (hunks[1].lines[1].old_no, hunks[1].lines[1].new_no),
            (None, Some(22))
        );
    }

    #[test]
    fn parse_diff_hunks_skips_file_labels_and_patch_headers() {
        // `diff --git`, `---`, `+++` and `index` lines never become rows and
        // do not disturb the counters; `\ No newline at end of file` markers
        // are skipped too.
        let diff = "\
diff --git a/src/lib.rs b/src/lib.rs
index 111..222 100644
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -1,2 +1,2 @@
-old
+new
\\ No newline at end of file
";
        let hunks = parse_diff_hunks(diff);
        assert_eq!(hunks.len(), 1);
        let hunk = &hunks[0];
        assert_eq!(hunk.lines.len(), 2);
        assert_eq!(hunk.lines[0].kind, LineKind::Removed);
        assert_eq!(
            (hunk.lines[0].old_no, hunk.lines[0].new_no),
            (Some(1), None)
        );
        assert_eq!(hunk.lines[1].kind, LineKind::Added);
        assert_eq!(
            (hunk.lines[1].old_no, hunk.lines[1].new_no),
            (None, Some(1))
        );
    }

    #[test]
    fn diff_header_path_takes_the_post_image_path() {
        assert_eq!(diff_header_path("diff --git a/x b/y"), Some("y".to_owned()));
        assert_eq!(
            diff_header_path("diff --git a/src/lib.rs b/src/lib.rs"),
            Some("src/lib.rs".to_owned())
        );
        assert_eq!(diff_header_path("index 111..222 100644"), None);
    }
}
