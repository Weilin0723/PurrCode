//! The application bar and the status bar.
//!
//! Between them they answer PRD §57's first question — *where am I?* — and
//! nothing else. There is no task mode here, no execution style, no permission
//! dropdown and no model picker: none of those are things a user must decide
//! before they are allowed to work (PRD §10), and every one of them has a home
//! in Settings for the people who want it (PRD §26).
//!
//! The bar is the window's own title bar. On macOS the system strip is made
//! transparent (see `lib.rs`) so the traffic lights float over this row instead
//! of sitting above a second one that repeats the same title; the bar reserves
//! room for them on the left and can be dragged like the strip it replaced.

use egui::{Align, Align2, FontId, Layout, Rect, RichText, Sense, Ui, Vec2, ViewportCommand};
use purrcode_runtime_core::ProductState;

use super::{DockTab, PurrCodeIde, Stage, Startup};
use crate::icons::Glyph;
use crate::theme;

impl PurrCodeIde {
    pub(crate) fn application_bar(&mut self, ui: &mut Ui) {
        // Registered first so every button added below wins the click; only
        // the bare background is left to drag the window by.
        self.title_bar_drag(ui);

        let bar = ui.max_rect();
        // One deliberate strip, not two independent halves: the left cluster
        // and the right cluster share a vertical optical centre, so a text
        // control and an icon-only control never sit on different baselines.
        // The row is pinned to the bar's centre; the mascot is a 30pt square,
        // so the text that starts on the same baseline as it also ends up
        // centred against it (the wordmark and labels occupy roughly 14–18pt
        // of a 30pt box, and `Align::Center` puts the mean of that against
        // the mean of the mark).
        //
        // The two cluster extents are captured so the centred command pill can
        // measure the span it actually has — the pill must never be painted
        // over either cluster (gap 3).
        let mut left_end = bar.left();
        let mut right_start = bar.right();
        ui.allocate_ui_with_layout(bar.size(), Layout::left_to_right(Align::Center), |ui| {
            if cfg!(target_os = "macos") {
                // Close / minimise / zoom live here now.
                ui.add_space(theme::TRAFFIC_LIGHT_INSET - 8.0);
            }

            if self.stage == Stage::Welcome {
                crate::icons::brand_badge(ui, 30.0);
                crate::icons::wordmark(
                    ui,
                    theme::TYPE_TITLE,
                    self.tokens.text_primary,
                    self.tokens.accent_primary,
                );
                ui.add_space(8.0);
                ui.label(
                    RichText::new("No folder opened")
                        .size(theme::TYPE_META)
                        .color(self.tokens.text_muted),
                );
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    self.connection_dot(ui);
                });
                return;
            }

            // Keep the product identity in the chrome. The mascot is the
            // supplied PurrCode mark; workspace and branch are context, not a
            // replacement logo.
            crate::icons::brand_badge(ui, 30.0);
            crate::icons::wordmark(
                ui,
                theme::TYPE_TITLE,
                self.tokens.text_primary,
                self.tokens.accent_primary,
            );
            ui.add_space(8.0);
            crate::icons::inline(ui, Glyph::Folder, 13.0, self.tokens.text_secondary);
            let folder = ui
                .label(
                    RichText::new(self.repository_name())
                        .strong()
                        .color(self.tokens.text_primary),
                )
                .interact(Sense::click())
                .on_hover_text(format!(
                    "{}\nClick to open a different folder",
                    self.repository.display()
                ));
            if folder.hovered() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            if folder.clicked() {
                self.close_workspace();
            }
            // The folder's own branch. A session's branch describes the
            // worktree it was given, which is a different thing and belongs to
            // a different folder entirely once the user switches.
            if let Some(branch) = self.workspace_state.branch.clone() {
                ui.add_space(4.0);
                self.branch_chip(ui, &branch);
            }
            left_end = ui.cursor().min.x;

            let right = ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.add_space(2.0);
                self.connection_dot(ui);
                ui.add_space(6.0);
                self.layout_toggles(ui);
                ui.add_space(8.0);
                self.state_chip(ui);
            });
            right_start = right.response.rect.min.x;
        });

        // Drawn last so it sits on top of the row it is centred in.
        if self.stage == Stage::Workspace {
            self.command_pill(ui, bar, left_end, right_start);
        }
    }

    /// Let the bar move the window, and let a double-click zoom it.
    fn title_bar_drag(&mut self, ui: &mut Ui) {
        let response = ui.interact(
            ui.max_rect(),
            egui::Id::new("purrcode_title_bar_drag"),
            Sense::click_and_drag(),
        );
        if response.drag_started() {
            ui.ctx().send_viewport_cmd(ViewportCommand::StartDrag);
        }
        if response.double_clicked() {
            let maximized = ui.input(|input| input.viewport().maximized.unwrap_or(false));
            ui.ctx()
                .send_viewport_cmd(ViewportCommand::Maximized(!maximized));
        }
    }

    /// The centred command bar: what workspace this window is on, and the way
    /// into everything by name.
    ///
    /// It is the same target as ⌘P, put where the eye already goes, so the
    /// keyboard route is discoverable without a tour.
    ///
    /// `left_end` / `right_start` are the free span's edges measured after the
    /// two clusters are laid out. The pill is skipped when that span is too
    /// narrow to hold it, because painting an opaque pill over the branch chip
    /// is worse than not offering the affordance at all — ⌘P still works.
    fn command_pill(&mut self, ui: &mut Ui, bar: Rect, left_end: f32, right_start: f32) {
        let desired = (bar.width() * 0.32).clamp(200.0, 440.0);
        // The free span between the clusters, minus a small gutter on each
        // side so the pill never kisses a neighbour even at its widest.
        let free_span = (right_start - left_end) - 24.0;
        if free_span < 160.0 {
            // Below a usable width there is no honest placement: skip the
            // pill entirely rather than let it hide the branch chip.
            return;
        }
        let width = desired.min(free_span);
        let center_x = (left_end + right_start) / 2.0;
        let rect =
            Rect::from_center_size(egui::pos2(center_x, bar.center().y), Vec2::new(width, 24.0));
        let response = ui.interact(rect, egui::Id::new("purrcode_command_pill"), Sense::click());
        let hovered = response.hovered();
        let (fill, stroke) = if hovered {
            (self.tokens.surface_hover, self.tokens.accent_primary)
        } else {
            (self.tokens.background_raised, self.tokens.border_subtle)
        };
        let painter = ui.painter();
        painter.rect_filled(rect, theme::RADIUS_PILL, fill);
        painter.rect_stroke(
            rect,
            theme::RADIUS_PILL,
            egui::Stroke::new(if hovered { 1.5_f32 } else { 1.0_f32 }, stroke),
            egui::StrokeKind::Inside,
        );
        crate::icons::draw(
            ui,
            Rect::from_center_size(
                egui::pos2(rect.left() + 16.0, rect.center().y),
                Vec2::splat(12.0),
            ),
            Glyph::Search,
            if hovered {
                self.tokens.accent_primary
            } else {
                self.tokens.text_muted
            },
        );
        ui.painter().text(
            egui::pos2(rect.left() + 28.0, rect.center().y),
            Align2::LEFT_CENTER,
            self.repository_name(),
            FontId::proportional(theme::TYPE_LABEL),
            if hovered {
                self.tokens.text_primary
            } else {
                self.tokens.text_secondary
            },
        );
        // The shortcut rides the far edge inside the pill, separated from the
        // label by a thin divider so the pill reads as a command surface
        // (search glyph, prompt, action hint) rather than as a bare rectangle.
        let key_x = rect.right() - 12.0;
        painter.vline(
            key_x - 14.0,
            egui::Rangef::new(rect.center().y - 5.5, rect.center().y + 5.5),
            self.tokens.hairline(),
        );
        ui.painter().text(
            egui::pos2(key_x, rect.center().y),
            Align2::RIGHT_CENTER,
            "⌘P",
            FontId::proportional(theme::TYPE_META),
            if hovered {
                self.tokens.text_secondary
            } else {
                self.tokens.text_muted
            },
        );
        if hovered {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        if response.has_focus() {
            self.tokens.focus_ring(ui.painter(), rect);
        }
        if response.clicked() {
            self.quick_open = Some(String::new());
        }
        response.on_hover_text("Go to a file by name (⌘P)");
    }

    /// The branch, as a chip rather than as another run of grey text.
    fn branch_chip(&mut self, ui: &mut Ui, branch: &str) {
        egui::Frame::new()
            .fill(self.tokens.background_raised)
            .corner_radius(theme::RADIUS_PILL)
            .inner_margin(egui::Margin::symmetric(8, 2))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    crate::icons::inline(ui, Glyph::Branch, 11.0, self.tokens.text_muted);
                    ui.label(
                        RichText::new(branch)
                            .size(theme::TYPE_META)
                            .color(self.tokens.text_secondary),
                    );
                });
            })
            .response
            .on_hover_text("The branch this folder is on");
    }

    /// Show and hide the three docks. These are the only layout controls in the
    /// chrome, and each one is a toggle with a visible on-state.
    fn layout_toggles(&mut self, ui: &mut Ui) {
        // Right-to-left: listed in reverse of how they read on screen.
        let bottom_open = self.bottom_panel.is_some();
        if self
            .panel_toggle(ui, Glyph::PanelBottom, bottom_open, "Toggle panel (⌘J)")
            .clicked()
        {
            self.bottom_panel = if bottom_open {
                None
            } else {
                Some(DockTab::Terminal)
            };
        }
        let aux_open = self.aux_panel.is_some();
        if self
            .panel_toggle(ui, Glyph::PanelRight, aux_open, "Toggle side panel")
            .clicked()
        {
            self.aux_panel = if aux_open {
                None
            } else {
                // The Agent transcript owns the centre column while the
                // editor tab is active. Opening an Agent panel here would
                // render a second universal composer with the same egui ID.
                // Use the useful read-only Changes/Source surface instead;
                // the explicit Split control is the only route that moves
                // the Agent session into the auxiliary column.
                Some(super::AuxView::Source)
            };
        }
        let sidebar_open = self.sidebar_visible;
        if self
            .panel_toggle(ui, Glyph::PanelLeft, sidebar_open, "Toggle sidebar (⌘B)")
            .clicked()
        {
            self.sidebar_visible = !sidebar_open;
        }
    }

    fn panel_toggle(&self, ui: &mut Ui, glyph: Glyph, open: bool, tooltip: &str) -> egui::Response {
        crate::icons::ghost_button(
            ui,
            glyph,
            26.0,
            15.0,
            if open {
                self.tokens.text_primary
            } else {
                self.tokens.text_muted
            },
            self.tokens.surface_hover,
            tooltip,
        )
    }

    /// The one place the current work announces its state in the chrome.
    ///
    /// Drawn whenever a workspace is open. With a session, it shows that
    /// session's live state. With no session, it answers the question that
    /// matters before any work begins: is an API model ready to go, or is this
    /// a local-only setup? (PRD §14 success metrics, behavioral spec §3.)
    fn state_chip(&mut self, ui: &mut Ui) {
        if self.stage != Stage::Workspace {
            return;
        }
        let view = if self.selected.is_some() {
            self.session.state_view()
        } else {
            // No session: report model readiness rather than a meaningless
            // "Ready". A remote (non-local) model means the API is usable; only
            // local models means the runtime is up but API calls are not.
            self.model_readiness_view()
        };
        let color = self.tokens.state(view.color);
        let detail = view.detail.clone().unwrap_or_else(|| view.label.clone());
        egui::Frame::new()
            .fill(self.tokens.tint(color))
            .corner_radius(theme::RADIUS_PILL)
            .inner_margin(egui::Margin::symmetric(9, 3))
            .show(ui, |ui| {
                ui.horizontal_centered(|ui| {
                    // The marker follows the real state, not just running-vs-idle:
                    // a completed or clean session gets a solid dot, a terminal
                    // failure keeps its cross, and nothing draws a hollow ring for
                    // "pending" — an empty circle reads as an unresolved step even
                    // when the work is actually done.
                    let marker = if view.execution_active {
                        "running"
                    } else {
                        match view.state {
                            ProductState::Completed
                            | ProductState::Ready
                            | ProductState::ReadyForReview => "done",
                            ProductState::Failed
                            | ProductState::Cancelled
                            | ProductState::NeedsRecovery => "failed",
                            _ => "pending",
                        }
                    };
                    crate::icons::step_marker(ui, marker, 9.0, color);
                    // One line, always: the label is 12pt against a chip that
                    // is 3px taller than the branch chip, so the two chips
                    // hold the same centreline instead of the state label
                    // sitting a point lower (which is what makes a row of
                    // chrome controls look misaligned).
                    ui.add_space(3.0);
                    ui.label(
                        RichText::new(view.label.clone())
                            .size(theme::TYPE_META)
                            .color(color),
                    );
                });
            })
            .response
            .on_hover_text(detail);
    }

    /// The no-session status: does the model setup have an API ready, or is
    /// this local-only? A remote (non-local) model means an API call is
    /// possible; only local models mean the runtime is up but API calls are not.
    ///
    /// The daemon's model list may not reflect the configured default when that
    /// default names a model the provider has not pinned in capabilities, so
    /// this reads the list as the provider surface it is and stays honest about
    /// "some API model is configured" rather than pretending to know the exact
    /// role assignment the daemon will resolve.
    fn model_readiness_view(&self) -> purrcode_runtime_core::ProductStateView {
        use purrcode_runtime_core::{ProductState, ProductStateView, StateColor};
        let has_api = self.models.iter().any(|model| !model.local);
        let has_local = self.models.iter().any(|model| model.local);
        let (state, label, color, detail) = if has_api {
            (
                ProductState::Ready,
                "API ready".to_owned(),
                StateColor::Success,
                "API models are configured and ready to use",
            )
        } else if has_local {
            (
                ProductState::Repairing,
                "Local only".to_owned(),
                StateColor::Warning,
                "Only local models are available; no API model configured",
            )
        } else {
            (
                ProductState::NeedsRecovery,
                "No model".to_owned(),
                StateColor::Neutral,
                "No models configured — add a provider in Settings",
            )
        };
        ProductStateView {
            state,
            label,
            glyph: state.glyph(true).to_owned(),
            color,
            primary_action: None,
            secondary_action: None,
            input: state.input(),
            execution_active: false,
            detail: Some(detail.to_owned()),
        }
    }

    /// The status bar: a single line of facts that are true right now.
    pub(crate) fn status_bar(&mut self, ui: &mut Ui) {
        // The status bar's top edge is where it meets the canvas, so the
        // separator line belongs at the top — a bottom hairline would sit
        // against the window edge and disappear. The fill is `background_raised`
        // so the bar reads as a raised shelf of chrome rather than as a strip
        // of the canvas below the work, staying distinct from the central panel
        // while using only tokens.
        ui.painter().rect_filled(
            ui.max_rect(),
            egui::CornerRadius::ZERO,
            self.tokens.background_raised,
        );
        ui.painter().hline(
            ui.max_rect().x_range(),
            ui.max_rect().top(),
            self.tokens.hairline(),
        );
        // A fixed row height so the left and right clusters always sit on the
        // same vertical centre — the status bar never grows from its content.
        ui.set_height(theme::STATUS_BAR_HEIGHT);
        ui.horizontal_centered(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            match self.startup {
                Startup::Connecting => {
                    ui.spinner();
                    ui.label(
                        RichText::new("Starting PurrCode…")
                            .size(theme::TYPE_META)
                            .color(self.tokens.text_secondary),
                    );
                }
                Startup::Restoring => {
                    ui.spinner();
                    ui.label(
                        RichText::new("Restoring your workspace…")
                            .size(theme::TYPE_META)
                            .color(self.tokens.text_secondary),
                    );
                }
                Startup::Unavailable => {
                    crate::icons::inline(ui, Glyph::Warning, 11.0, self.tokens.status_error);
                    ui.label(
                        RichText::new("PurrCode couldn't start its local service")
                            .size(theme::TYPE_META)
                            .color(self.tokens.status_error),
                    );
                    if ui.small_button("Retry").clicked() {
                        self.retry_connection();
                    }
                    if ui.small_button("View diagnostics").clicked() {
                        self.diagnostics_open = true;
                    }
                }
                Startup::Ready => {
                    if self.stage == Stage::Workspace {
                        // One number, one source (FR-C8): prefer the workspace
                        // change set once it has arrived, and append `+A −M`.
                        // The porcelain count is only the fallback while the
                        // per-file set has not loaded yet.
                        let workspace = &self.workspace_changes;
                        let (glyph, summary) = if workspace.available {
                            if workspace.files_changed == 0 {
                                (Glyph::Check, "Working tree clean".to_owned())
                            } else {
                                (
                                    Glyph::Pencil,
                                    format!(
                                        "{} uncommitted file{} · +{} −{}",
                                        workspace.files_changed,
                                        if workspace.files_changed == 1 {
                                            ""
                                        } else {
                                            "s"
                                        },
                                        workspace.additions,
                                        workspace.deletions,
                                    ),
                                )
                            }
                        } else if self.workspace_state.git_known {
                            if self.workspace_state.clean {
                                (Glyph::Check, "Working tree clean".to_owned())
                            } else {
                                (
                                    Glyph::Pencil,
                                    format!(
                                        "{} uncommitted file{}",
                                        self.workspace_state.changed_file_count,
                                        if self.workspace_state.changed_file_count == 1 {
                                            ""
                                        } else {
                                            "s"
                                        }
                                    ),
                                )
                            }
                        } else if self.workspace_state.is_git_repository {
                            (Glyph::Warning, "Git state not checked".to_owned())
                        } else {
                            (Glyph::Folder, "Not a Git repository".to_owned())
                        };
                        crate::icons::inline(ui, glyph, 11.0, self.tokens.text_muted);
                        ui.label(
                            RichText::new(summary)
                                .size(theme::TYPE_META)
                                .color(self.tokens.text_muted),
                        );
                        // Session usage: tokens, cache hit, elapsed time. One
                        // quiet line so the cost of a run is visible without
                        // opening the dock (PRD §12.11).
                        let statline = self.session.usage.statline();
                        if !statline.is_empty() {
                            ui.separator();
                            crate::icons::inline(ui, Glyph::Sparkle, 10.0, self.tokens.text_muted);
                            ui.label(
                                RichText::new(statline)
                                    .size(theme::TYPE_META)
                                    .color(self.tokens.text_muted),
                            );
                        }
                        // Which language servers are actually running. A
                        // machine with none says so here rather than leaving
                        // the user to wonder why nothing resolves.
                        ui.separator();
                        let summary = crate::app::language::status_summary(&self.language);
                        let colour =
                            if self.language.servers_checked && self.language.servers.is_empty() {
                                self.tokens.status_warning
                            } else {
                                self.tokens.text_muted
                            };
                        crate::icons::inline(ui, Glyph::Sparkle, 10.0, colour);
                        ui.label(
                            RichText::new(summary)
                                .size(theme::TYPE_META)
                                .color(colour),
                        )
                        .on_hover_text(match &self.language.last_error {
                            Some(error) => format!("Last language-server error: {error}"),
                            None => {
                                "Language intelligence comes from the language servers installed \
                                 on this machine."
                                    .to_owned()
                            }
                        });
                    }
                }
            }

            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.add_space(2.0);
                // The bottom panel (Terminal / Problems / Tests) is toggled
                // from here, matching the IDE status-bar convention.
                for tab in DockTab::ALL.iter().rev() {
                    self.dock_status_item(ui, *tab);
                }
            });
        });
    }

    /// One dock toggle in the status bar: icon, name, and a badge when the
    /// dock has something to say.
    fn dock_status_item(&mut self, ui: &mut Ui, tab: DockTab) {
        let open = self.bottom_panel == Some(tab);
        let badge = self.dock_badge(tab);
        let glyph = match tab {
            DockTab::Tests => Glyph::Beaker,
            DockTab::Problems => Glyph::Warning,
            DockTab::Terminal => Glyph::Terminal,
            DockTab::Activity => Glyph::Sparkle,
        };
        let color = if open {
            self.tokens.accent_primary
        } else {
            self.tokens.text_muted
        };
        // The group's right edge must not move when a badge appears, so the
        // badge reserves its space whether or not it is shown. The dock
        // toggles then read as one cohesive right-aligned group whose outer
        // edge stays put; the small amount of trailing space a badge-less
        // item keeps is the price of that stable edge.
        let response = egui::Frame::new()
            .fill(if open {
                self.tokens.accent_soft
            } else {
                egui::Color32::TRANSPARENT
            })
            .corner_radius(theme::RADIUS_CONTROL)
            .inner_margin(egui::Margin::symmetric(6, 1))
            .show(ui, |ui| {
                ui.horizontal_centered(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    crate::icons::inline(ui, glyph, 11.0, color);
                    ui.label(
                        RichText::new(tab.label())
                            .size(theme::TYPE_META)
                            .color(color),
                    );
                    // Reserve the badge slot. The slot is a fixed width whether
                    // the badge is "✓", "…" or "12", so the item does not widen
                    // when a badge appears and push its neighbours around — the
                    // group's right edge stays put.
                    let (slot, _) =
                        ui.allocate_exact_size(egui::Vec2::new(13.0, 14.0), Sense::hover());
                    if let Some(badge) = &badge {
                        ui.painter().text(
                            slot.center(),
                            egui::Align2::CENTER_CENTER,
                            badge,
                            FontId::proportional(theme::TYPE_META),
                            self.tokens.text_secondary,
                        );
                    }
                });
            })
            .response
            .interact(Sense::click())
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_text(format!(
                "{} — {} (⌘{})",
                tab.label(),
                match tab {
                    DockTab::Tests => "Validation results",
                    DockTab::Problems => "Build and type errors",
                    DockTab::Terminal => "Terminal sessions",
                    DockTab::Activity => "Agent activity lanes",
                },
                tab.shortcut()
            ));
        if response.has_focus() {
            self.tokens.focus_ring(ui.painter(), response.rect);
        }
        if response.clicked() {
            self.bottom_panel = if open { None } else { Some(tab) };
        }
    }
}
