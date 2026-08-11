//! The evidence dock: Tests, Problems, Terminal, Activity.
//!
//! PRD §12 and §15: all of this is secondary or below. It lives in a dock
//! that is closed by default and opened from the status bar when the user
//! wants it. Nothing in the dock competes with the conversation for
//! attention.

use egui::{Align, Layout, RichText, ScrollArea, Ui};

use super::{DockTab, PurrCodeIde};
use crate::theme;

impl PurrCodeIde {
    /// The docked evidence panel at the bottom.
    pub(crate) fn dock_panel(&mut self, ui: &mut Ui) {
        let Some(tab) = self.dock else {
            return;
        };

        ui.horizontal(|ui| {
            self.section_heading_label(ui, tab.label());
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.small_button("×").clicked() {
                    self.bottom_panel = None;
                }
            });
        });
        ui.add_space(4.0);

        match tab {
            DockTab::Tests => self.tests_panel(ui),
            DockTab::Problems => self.problems_panel(ui),
            DockTab::Terminal => self.terminal_panel(ui),
            DockTab::Activity => self.activity_panel(ui),
        }
    }

    // ── Tests ────────────────────────────────────────────────────────

    fn tests_panel(&self, ui: &mut Ui) {
        let validation = &self.session.validation;
        if validation.stages.is_empty() {
            ui.vertical_centered(|ui| {
                ui.add_space(24.0);
                ui.label(RichText::new("No validation has run").color(self.tokens.text_muted));
            });
            return;
        }

        if let Some(aggregate) = validation.aggregate() {
            let color = if validation.needs_attention() {
                self.tokens.status_warning
            } else {
                self.tokens.status_success
            };
            ui.horizontal(|ui| {
                ui.label(RichText::new(aggregate).strong().color(color));
                // Validation re-run is not wired to a daemon route yet, so no
                // "Retry failed" button is offered: a control that looks
                // tappable and does nothing is worse than none. The dock badge
                // already names the failure.
            });
            ui.add_space(6.0);
        }

        ScrollArea::vertical()
            .id_salt("tests_stages")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for stage in &validation.stages {
                    let passed = stage.passed();
                    let color = if passed {
                        self.tokens.status_success
                    } else if stage.outcome == "timed out" {
                        self.tokens.status_warning
                    } else {
                        self.tokens.status_error
                    };
                    let marker = if passed {
                        "done"
                    } else if stage.outcome == "timed out" {
                        "blocked"
                    } else {
                        "failed"
                    };
                    ui.horizontal(|ui| {
                        crate::icons::step_marker(ui, marker, 9.0, color);
                        ui.label(
                            RichText::new(&stage.stage)
                                .small()
                                .color(self.tokens.text_secondary),
                        );
                        if let Some(detail) = &stage.detail {
                            ui.label(RichText::new(detail).small().color(self.tokens.text_muted));
                        }
                    });
                }
            });
    }

    // ── Problems ────────────────────────────────────────────────────

    fn problems_panel(&mut self, ui: &mut Ui) {
        // Two independent sources answer different questions, and collapsing
        // them would let one stand in for the other: validation reports what
        // ran and failed, language servers report what the code says right
        // now. A project whose tests never ran is not a project without
        // problems, so the panel names each source and what it has actually
        // seen rather than printing one unqualified "no problems".
        let tokens = self.tokens;
        let repository = self.repository.clone();
        ui.label(
            RichText::new("FROM LANGUAGE SERVERS")
                .size(theme::TYPE_EYEBROW)
                .strong()
                .color(tokens.text_muted),
        );
        ui.add_space(2.0);
        let jump = crate::app::language::diagnostics_list(ui, &tokens, &self.language, &repository);
        if let Some(location) = jump {
            self.open_location(&location);
        }
        ui.add_space(10.0);
        ui.label(
            RichText::new("FROM VALIDATION")
                .size(theme::TYPE_EYEBROW)
                .strong()
                .color(tokens.text_muted),
        );
        ui.add_space(2.0);

        let problems = crate::model::problems_from(&self.session.validation);
        if problems.is_empty() {
            let explanation = if self.session.validation.stages.is_empty() {
                "No validation has run for this session yet."
            } else {
                "Every validation stage that ran passed."
            };
            ui.label(RichText::new(explanation).small().color(tokens.text_muted));
            return;
        }

        // Group by category
        let mut by_category: std::collections::BTreeMap<
            crate::model::ProblemCategory,
            Vec<&crate::model::Problem>,
        > = std::collections::BTreeMap::new();
        for p in &problems {
            by_category.entry(p.category).or_default().push(p);
        }

        for (category, items) in by_category {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(category.label())
                        .small()
                        .strong()
                        .color(self.tokens.text_muted),
                );
                ui.label(
                    RichText::new(format!("({})", items.len()))
                        .small()
                        .color(self.tokens.text_muted),
                );
            });
            ui.add_space(4.0);
            ScrollArea::vertical()
                .id_salt(format!("problems_{category:?}"))
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    for problem in items {
                        ui.horizontal(|ui| {
                            let color = match problem.category {
                                crate::model::ProblemCategory::Build
                                | crate::model::ProblemCategory::TypeChecking => {
                                    self.tokens.status_error
                                }
                                crate::model::ProblemCategory::Tests => self.tokens.status_warning,
                                crate::model::ProblemCategory::Environment
                                | crate::model::ProblemCategory::Dependencies => {
                                    self.tokens.status_warning
                                }
                                _ => self.tokens.text_secondary,
                            };
                            let marker =
                                if matches!(problem.category, crate::model::ProblemCategory::Tests)
                                {
                                    "blocked"
                                } else {
                                    "failed"
                                };
                            crate::icons::step_marker(ui, marker, 9.0, color);
                            ui.label(
                                RichText::new(&problem.summary)
                                    .small()
                                    .color(self.tokens.text_secondary),
                            );
                            if let Some((path, line)) = &problem.location {
                                ui.label(
                                    RichText::new(format!("{path}:{line}"))
                                        .small()
                                        .color(self.tokens.text_muted),
                                );
                            }
                        });
                    }
                });
            ui.add_space(6.0);
        }
    }

    // ── Terminal ────────────────────────────────────────────────────

    fn terminal_panel(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("Terminal")
                    .small()
                    .color(self.tokens.text_muted),
            );
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.small_button("New").clicked() {
                    self.start_new_terminal();
                }
            });
        });
        ui.add_space(4.0);

        if self.terminals.is_empty() {
            ui.vertical_centered(|ui| {
                ui.add_space(24.0);
                ui.label(RichText::new("No terminals open").color(self.tokens.text_muted));
                ui.add_space(8.0);
                if ui.small_button("Open terminal").clicked() {
                    self.start_new_terminal();
                }
            });
            return;
        }

        // Terminal tabs — same visual language as the editor strip.
        let mut terminal_to_close: Option<usize> = None;
        ui.horizontal(|ui| {
            for (i, term) in self.terminals.iter().enumerate() {
                let is_active = i == self.active_terminal;
                let (clicked, closed) = super::editor::tab(
                    ui,
                    &self.tokens,
                    Some(super::editor::TabIcon::Glyph(crate::icons::Glyph::Terminal)),
                    &term.title(),
                    is_active,
                    false,
                    true,
                    ("terminal_tab", i),
                );
                if clicked {
                    self.active_terminal = i;
                }
                if closed {
                    terminal_to_close = Some(i);
                }
            }
        });
        ui.add_space(4.0);

        if let Some(idx) = terminal_to_close {
            self.close_terminal(idx);
        }

        // Active terminal view
        let appearance = self.appearance;
        let tokens = self.tokens;
        let take_focus = std::mem::take(&mut self.focus_terminal);
        if let Some(term) = self.terminals.get_mut(self.active_terminal) {
            let commands = term.render(ui, &tokens, appearance, take_focus);
            self.apply_terminal_commands(ui, commands);
        }
    }

    // ── Activity ────────────────────────────────────────────────────

    fn activity_panel(&self, ui: &mut Ui) {
        // Show task graph lanes if available
        if !self.session.controls.lanes.is_empty() {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("Plan Lanes")
                        .small()
                        .strong()
                        .color(self.tokens.text_muted),
                );
            });
            ui.add_space(4.0);
            ScrollArea::vertical()
                .id_salt("activity_lanes")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    for lane in &self.session.controls.lanes {
                        let color = self.tokens.state(match lane.status.as_str() {
                            "done" => purrcode_runtime_core::StateColor::Success,
                            "running" => purrcode_runtime_core::StateColor::Running,
                            "failed" => purrcode_runtime_core::StateColor::Error,
                            "blocked" => purrcode_runtime_core::StateColor::Warning,
                            _ => purrcode_runtime_core::StateColor::Neutral,
                        });
                        ui.horizontal(|ui| {
                            crate::icons::step_marker(ui, lane.marker(), 9.0, color);
                            ui.label(
                                RichText::new(&lane.label)
                                    .small()
                                    .color(self.tokens.text_secondary),
                            );
                            if let Some(summary) = &lane.summary {
                                ui.label(
                                    RichText::new(summary).small().color(self.tokens.text_muted),
                                );
                            }
                        });
                    }
                });
            ui.add_space(12.0);
        }

        // Coverage / requirements evidence
        if !self.session.coverage.is_empty() {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("Evidence Coverage")
                        .small()
                        .strong()
                        .color(self.tokens.text_muted),
                );
            });
            ui.add_space(4.0);
            ScrollArea::vertical()
                .id_salt("activity_coverage")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    for row in &self.session.coverage {
                        let color = match row.status.as_str() {
                            "passed" => self.tokens.status_success,
                            "failed" => self.tokens.status_error,
                            _ => self.tokens.text_muted,
                        };
                        ui.horizontal(|ui| {
                            crate::icons::step_marker(
                                ui,
                                if color == self.tokens.status_success {
                                    "done"
                                } else if color == self.tokens.status_error {
                                    "failed"
                                } else {
                                    "blocked"
                                },
                                9.0,
                                color,
                            );
                            ui.label(
                                RichText::new(&row.statement)
                                    .small()
                                    .color(self.tokens.text_secondary),
                            );
                            if let Some(summary) = &row.summary {
                                ui.label(
                                    RichText::new(summary).small().color(self.tokens.text_muted),
                                );
                            }
                        });
                    }
                });
            ui.add_space(12.0);
        }

        // Raw activity stream (condensed)
        let condensed = crate::model::condense(&self.session.activity);
        if !condensed.is_empty() {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("Activity")
                        .small()
                        .strong()
                        .color(self.tokens.text_muted),
                );
            });
            ui.add_space(4.0);
            ScrollArea::vertical()
                .id_salt("activity_stream")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    for (line, count) in condensed {
                        let color = self.tokens.state(match line.status.as_str() {
                            "done" => purrcode_runtime_core::StateColor::Success,
                            "running" => purrcode_runtime_core::StateColor::Running,
                            "failed" => purrcode_runtime_core::StateColor::Error,
                            "blocked" => purrcode_runtime_core::StateColor::Warning,
                            _ => purrcode_runtime_core::StateColor::Neutral,
                        });
                        ui.horizontal(|ui| {
                            crate::icons::step_marker(ui, line.marker(), 8.0, color);
                            ui.label(
                                RichText::new(&line.label)
                                    .small()
                                    .color(self.tokens.text_secondary),
                            );
                            if count > 1 {
                                ui.label(
                                    RichText::new(format!("×{count}"))
                                        .small()
                                        .color(self.tokens.text_muted),
                                );
                            }
                            if let Some(summary) = &line.summary {
                                ui.label(
                                    RichText::new(format!("— {summary}"))
                                        .small()
                                        .color(self.tokens.text_muted),
                                );
                            }
                        });
                    }
                });
        }
    }

    fn start_new_terminal(&mut self) {
        // The daemon scopes a terminal to the repository root for now; a
        // session worktree override is not resolved here, so the working
        // directory is honest about being the checkout rather than pretending
        // to be session-scoped.
        let wd = self.repository_string();
        self.client.send(crate::daemon::Request::StartTerminal {
            working_directory: wd,
            rows: 24,
            cols: 80,
        });
    }

    fn close_terminal(&mut self, index: usize) {
        if index < self.terminals.len() {
            let term = &self.terminals[index];
            self.client.send(crate::daemon::Request::StopTerminal {
                terminal: term.terminal_id.clone(),
            });
            self.terminals.remove(index);
            if self.active_terminal >= self.terminals.len() && self.active_terminal > 0 {
                self.active_terminal = self.terminals.len() - 1;
            }
        }
    }

    /// Dispatch the [`TerminalCommand`]s returned by [`GuiTerminal::render`] to the daemon.
    fn apply_terminal_commands(
        &mut self,
        ui: &egui::Ui,
        commands: Vec<crate::terminal::TerminalCommand>,
    ) {
        use crate::terminal::TerminalCommand;
        if let Some(term) = self.terminals.get(self.active_terminal) {
            let terminal_id = term.terminal_id.clone();
            let generation = term.generation;
            for cmd in commands {
                match cmd {
                    TerminalCommand::Input(bytes) => {
                        self.client.send(crate::daemon::Request::SendTerminalInput {
                            terminal: terminal_id.clone(),
                            generation,
                            bytes,
                        });
                    }
                    TerminalCommand::Resize { rows, cols } => {
                        self.client.send(crate::daemon::Request::ResizeTerminal {
                            terminal: terminal_id.clone(),
                            rows,
                            cols,
                        });
                    }
                    TerminalCommand::Interrupt => {
                        self.client.send(crate::daemon::Request::SendTerminalInput {
                            terminal: terminal_id.clone(),
                            generation,
                            bytes: vec![0x03],
                        });
                    }
                    TerminalCommand::Terminate => {
                        self.client.send(crate::daemon::Request::StopTerminal {
                            terminal: terminal_id.clone(),
                        });
                    }
                    TerminalCommand::Close => {
                        self.close_terminal(self.active_terminal);
                    }
                    TerminalCommand::Copy(text) => {
                        ui.ctx().copy_text(text);
                    }
                    // Not yet wired through the daemon; accept them so they
                    // don't silently drop when the UI dispatches them.
                    TerminalCommand::Restart
                    | TerminalCommand::TakeControl
                    | TerminalCommand::ReturnToAgent
                    | TerminalCommand::OpenShell => {
                        // TODO: wire these through daemon control routes
                    }
                }
            }
        }
    }
}
