//! The workbench: the centre of the window where the agent's work appears.
//!
//! This is the primary surface of PRD §24 — the conversation and its
//! meaningful progress, never a raw event log. The composer sits at the
//! bottom, the transcript scrolls above it, and notices live in between.
//!
//! PRD §15: the conversation owns the centre. Nothing below it competes for
//! attention until the user opens the dock.
//!
//! Everything the agent *did* is a card in the transcript rather than a fixed
//! band above it: a card can be read in passing, it scrolls away when it stops
//! mattering, and it keeps the composer where the user last saw it instead of
//! pushing it down every time a step lands.

use egui::{Align, Layout, RichText, Sense, Ui};
use purrcode_runtime_core::ProductState;
use serde_json::Value;

use super::{DockTab, PurrCodeIde};
use crate::daemon::Request;
use crate::icons::Glyph;
use crate::markdown;
use crate::model;
use crate::theme;

/// The room the composer takes at the foot of the workbench.
const COMPOSER_ROOM: f32 = 104.0;
/// The band between the transcript and the composer, which the working strip
/// fills while a run is active and which stays empty when one is not.
///
/// A fixed height rather than "whatever the strip measured": the whole point is
/// that the number cannot change, and a self-measuring band would change it.
const WORKING_STRIP_ROOM: f32 = 28.0;
/// The gap under the working strip's band.
const WORKING_STRIP_GAP: f32 = 6.0;
/// The widest a user's message bubble may get, as a share of the transcript.
///
/// A bubble that runs the full measure is indistinguishable from the agent's
/// reply; one capped here still reads as a quotation of the reader's own words.
const USER_BUBBLE_SHARE: f32 = 0.72;
/// Below this a bubble is narrower than the words inside it.
const USER_BUBBLE_FLOOR: f32 = 140.0;
/// The bubble's own padding, needed twice: once by the frame that draws it and
/// once by the measurement that decides where its left edge goes.
const USER_BUBBLE_PAD_X: f32 = 11.0;

impl PurrCodeIde {
    /// The agent conversation surface. This is the ONE renderer of a session,
    /// embedded identically as an editor tab, the right auxiliary panel, or
    /// the Agent-Focus main view (PRD §28-29).
    pub(crate) fn agent_surface(&mut self, ui: &mut Ui) {
        if self.selected.is_none() {
            self.empty_workbench(ui);
        } else {
            self.active_workbench(ui);
        }
    }

    /// The empty-state workbench: teaches the spatial model.
    ///
    /// NOT a giant centered card. The transcript area shows a lightweight
    /// prompt that explains its own purpose, and the composer sits at the
    /// bottom in its natural position.
    fn empty_workbench(&mut self, ui: &mut Ui) {
        let composer_room = COMPOSER_ROOM;
        let mut starter = None;
        ui.vertical(|ui| {
            ui.allocate_ui(
                egui::vec2(
                    ui.available_width(),
                    (ui.available_height() - composer_room).max(60.0),
                ),
                |ui| {
                    ui.add_space((ui.available_height() * 0.14).max(14.0));
                    ui.horizontal(|ui| {
                        // The starter content spans the same column as the
                        // composer below it, so the empty state and the input
                        // share one left edge. The small inset keeps the lockup
                        // from hugging the window frame at narrow widths.
                        ui.add_space((ui.available_width() * 0.06).min(32.0));
                        ui.vertical(|ui| {
                            // A cap, not a floor: `.max(520.0)` used to force a
                            // 520pt column into the 320pt auxiliary panel and
                            // clip the headline. The starter lockup reflows
                            // instead of shrinking below its minimum.
                            ui.set_max_width(ui.available_width().min(520.0));
                            ui.horizontal(|ui| {
                                crate::icons::brand_badge(ui, 28.0);
                                ui.label(
                                    RichText::new("AGENT WORKSPACE")
                                        .size(theme::TYPE_EYEBROW)
                                        .strong()
                                        .color(self.tokens.accent_primary),
                                );
                            });
                            ui.add_space(10.0);
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 0.0;
                                for (text, color) in [
                                    ("Give Purr", self.tokens.text_primary),
                                    ("Code", self.tokens.accent_primary),
                                    (" a job.", self.tokens.text_primary),
                                ] {
                                    ui.label(
                                        RichText::new(text)
                                            .family(egui::FontFamily::Name(
                                                "purrcode_display".into(),
                                            ))
                                            .size(theme::TYPE_DISPLAY)
                                            .strong()
                                            .color(color),
                                    );
                                }
                            });
                            ui.add_space(5.0);
                            ui.label(
                                RichText::new(
                                    "Ask a question or describe the outcome you want. PurrCode will choose the workflow and keep the evidence beside the work.",
                                )
                                .size(12.5)
                                .color(self.tokens.text_secondary),
                            );
                            ui.add_space(18.0);
                            ui.label(
                                RichText::new("START WITH")
                                    .size(theme::TYPE_EYEBROW)
                                    .strong()
                                    .color(self.tokens.text_muted),
                            );
                            ui.add_space(4.0);
                            for (label, prompt) in [
                                (
                                    "Understand this workspace",
                                    "Explain this codebase and identify the most important entry points.",
                                ),
                                (
                                    "Find the next risky change",
                                    "Review the current workspace and identify the highest-risk unfinished change.",
                                ),
                                (
                                    "Build a reviewable improvement",
                                    "Choose one small, high-impact improvement, implement it, and validate it.",
                                ),
                            ] {
                                let response = egui::Frame::new()
                                    .fill(self.tokens.background_raised)
                                    .stroke(egui::Stroke::new(
                                        1.0_f32,
                                        self.tokens.border_subtle,
                                    ))
                                    .corner_radius(theme::RADIUS_CONTROL)
                                    .inner_margin(egui::Margin::symmetric(10, 7))
                                    .show(ui, |ui| {
                                        ui.horizontal(|ui| {
                                            ui.label(
                                                RichText::new(label)
                                                    .size(11.5)
                                                    .color(self.tokens.text_primary),
                                            );
                                            ui.with_layout(
                                                Layout::right_to_left(Align::Center),
                                                |ui| {
                                                    crate::icons::inline(
                                                        ui,
                                                        Glyph::ChevronRight,
                                                        11.0,
                                                        self.tokens.text_muted,
                                                    );
                                                },
                                            );
                                        });
                                    })
                                    .response
                                    .interact(Sense::click())
                                    .on_hover_cursor(egui::CursorIcon::PointingHand);
                                if response.clicked() {
                                    starter = Some(prompt.to_owned());
                                }
                                ui.add_space(4.0);
                            }
                        });
                    });
                },
            );
            if let Some(prompt) = starter.take() {
                self.composer = prompt;
                self.focus_composer = true;
            }
            self.composer_widget(ui);
        });
    }

    fn active_workbench(&mut self, ui: &mut Ui) {
        let state_view = self.session.state_view();
        let running = state_view.execution_active;

        let has_task_card = !self.session.tasks.is_empty();
        if has_task_card {
            self.current_task_card(ui);
            ui.add_space(8.0);
        }

        // ── Transcript ───────────────────────────────────────────────
        //
        // The composer and the working strip are reserved out of the height
        // first so the scroll area takes exactly what is left; measuring the
        // other way round is what makes an input drift off the bottom edge.
        //
        // The reservation is the same whether or not a run is active. It used
        // to grow by the strip's height the moment work started, which moved
        // the transcript's bottom edge — and with it every line the reader was
        // looking at — twice per turn. Holding the number still pins that edge,
        // and because the scroll area sticks to the bottom the conversation
        // stays exactly where it was. The task card above has the same property
        // for the same reason: it is subtracted from the height before this
        // line runs, so when a plan arrives the transcript loses room off the
        // top rather than pushing its contents down.
        let transcript = transcript_height(ui.available_height(), running);
        // The width the transcript's content actually gets, taken out here
        // rather than inside the scroll area: geometry derived from
        // `available_width()` in there is measured against whatever the
        // scrollbar left behind, and a message would slide sideways the frame
        // the conversation grew past one screen.
        let measure = ui.available_width();
        let mut open_aux = false;
        let mut toggle_inline = false;
        // A restore/fork clicked in the transcript, applied after the scroll
        // area releases its borrow of the message list.
        let mut message_action: Option<(super::checkpoints::MessageAction, model::Message)> = None;
        egui::ScrollArea::vertical()
            .id_salt("conversation_scroll")
            .stick_to_bottom(true)
            .max_height(transcript)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if state_view.state == ProductState::NeedsRecovery
                    || self.session.recovery_reconciled
                {
                    self.recovery_card(ui);
                    ui.add_space(8.0);
                }
                if self.session.awaiting_plan_review {
                    self.plan_preview(ui);
                    ui.add_space(8.0);
                }

                let condensed = model::condense(&self.session.activity);
                if self.session.messages.is_empty() && !self.session_loading {
                    ui.vertical_centered(|ui| {
                        ui.add_space(24.0);
                        ui.label(
                            RichText::new("Send a message to start working")
                                .size(12.0)
                                .color(self.tokens.text_muted),
                        );
                    });
                } else {
                    // Runtime activity belongs between the user's request and
                    // the final answer — anchored to the request by exact
                    // `turn_id`, not to the answer; see `work_log_anchor` for
                    // why.
                    let anchor = work_log_anchor(&self.session.messages, &self.session.activity);
                    let mut work_log_rendered = false;
                    for (index, message) in self.session.messages.iter().enumerate() {
                        // Every user message is a bubble, including the first:
                        // even when it restates the objective, the user typed
                        // those exact words and the transcript should show them
                        // as their own. The objective itself lives in the
                        // breadcrumb; the bubble below is the user's turn.
                        //
                        // The transcript is drawn while `session.messages` is
                        // borrowed, so a restore/fork click is recorded here
                        // and run after the loop.
                        if let Some(action) = self.message(ui, message, measure) {
                            message_action = Some((action, message.clone()));
                        }
                        if Some(index) == anchor && !condensed.is_empty() {
                            ui.add_space(4.0);
                            self.work_log(ui, &condensed);
                            work_log_rendered = true;
                            ui.add_space(4.0);
                        }
                    }
                    if !work_log_rendered && !condensed.is_empty() {
                        ui.add_space(4.0);
                        self.work_log(ui, &condensed);
                    }
                }

                if !self.session.tasks.is_empty() {
                    ui.add_space(4.0);
                    self.task_progress(ui);
                }

                if self.session_loading {
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(
                            RichText::new("Loading…")
                                .size(11.0)
                                .color(self.tokens.text_muted),
                        );
                    });
                }

                // The agent's changes appear here, in the conversation, the
                // way Kiro shows diffs — not only off to the side.
                if let Some(diff) = self.diff.as_ref()
                    && !diff.trim().is_empty()
                {
                    ui.add_space(6.0);
                    toggle_inline |= self.inline_diff(ui, diff, &mut open_aux);
                }
                ui.add_space(4.0);
            });
        if open_aux {
            self.aux_panel = Some(super::AuxView::Source);
        }
        if toggle_inline {
            self.inline_diff_open = !self.inline_diff_open;
        }
        if let Some((action, message)) = message_action {
            self.apply_message_action(action, &message);
        }

        ui.add_space(6.0);

        // ── Notices ─────────────────────────────────────────────────
        self.notice_stack(ui);

        // ── Working strip ───────────────────────────────────────────
        //
        // The band is allocated whether or not a run is active. It is the only
        // thing between the transcript and the composer, so letting it appear
        // and vanish would shove the whole conversation by its height at the
        // start of every turn and again at the end. An empty band costs a strip
        // of background and holds the layout still.
        self.working_strip(ui, &state_view, running);
        ui.add_space(WORKING_STRIP_GAP);

        // ── Composer (always at the bottom) ─────────────────────────
        self.composer_widget(ui);
    }

    /// What is happening right now, immediately above the composer.
    ///
    /// The transcript answers "what has happened"; this answers "what is
    /// happening", which is the question a user asks while they wait. When
    /// nothing is running it still fills its room with the session state so
    /// the status ("Ready", "Needs recovery") is never lost — and when that
    /// state has a resumable action, the bare word becomes a button ("Retry",
    /// "Recover session") so the session reads as something to carry forward,
    /// not a dead-end.
    fn working_strip(
        &mut self,
        ui: &mut Ui,
        state: &purrcode_runtime_core::ProductStateView,
        running: bool,
    ) {
        if !running {
            // The band is pinned to the working-strip height even when idle,
            // and everything in it rides that fixed band so the composer does
            // not move. `vertical_centered` would expand the child to the
            // available height and shove the composer off the bottom, so the
            // band is allocated exact and the elements are painted into it.
            let color = self.tokens.state(state.color);
            let (rect, _) = ui.allocate_exact_size(
                egui::vec2(ui.available_width(), WORKING_STRIP_ROOM),
                Sense::hover(),
            );

            // A resumable state gets an affordance instead of a bare word:
            // the state word in muted text, with a tinted action button on the
            // right in the state's own colour — the running strip's visual
            // language, minus the icon.
            if let Some(action_label) = state.primary_action.as_deref() {
                let action = resume_action(action_label);
                let button_galley = ui.fonts_mut(|fonts| {
                    fonts.layout_no_wrap(
                        action_label.to_owned(),
                        egui::FontId::proportional(11.0),
                        color,
                    )
                });
                let button_h = 22.0;
                let button_w = button_galley.size().x + 18.0;
                let button_rect = egui::Rect::from_min_size(
                    egui::pos2(
                        rect.right() - 10.0 - button_w,
                        rect.center().y - button_h * 0.5,
                    ),
                    egui::vec2(button_w, button_h),
                );
                let button = ui
                    .interact(button_rect, ui.id().with("strip_action"), Sense::click())
                    .on_hover_cursor(egui::CursorIcon::PointingHand);
                let fill = if button.hovered() {
                    theme::mix(self.tokens.tint(color), color, 0.25)
                } else {
                    self.tokens.tint(color)
                };
                ui.painter()
                    .rect_filled(button_rect, theme::RADIUS_CONTROL, fill);
                ui.painter().galley(
                    egui::pos2(
                        button_rect.center().x - button_galley.size().x * 0.5,
                        button_rect.center().y - button_galley.size().y * 0.5,
                    ),
                    button_galley,
                    color,
                );
                if button.clicked() && !self.submitting {
                    self.dispatch_strip_action(action);
                }

                // The state word rides to the left of the button, muted, so
                // the pair reads "Failed  [Retry]" rather than dropping the
                // status in favour of the action.
                let word = ui.fonts_mut(|fonts| {
                    fonts.layout_no_wrap(
                        state.label.clone(),
                        egui::FontId::proportional(11.0),
                        self.tokens.text_muted,
                    )
                });
                ui.painter().galley(
                    egui::pos2(
                        button_rect.left() - 12.0 - word.size().x,
                        rect.center().y - word.size().y * 0.5,
                    ),
                    word,
                    self.tokens.text_muted,
                );
                return;
            }

            // No action (Ready, Thinking, Working…): keep it minimal — just
            // the state label right-aligned as today, pinned to the band.
            let galley = ui.fonts_mut(|fonts| {
                fonts.layout_no_wrap(state.label.clone(), egui::FontId::proportional(11.0), color)
            });
            ui.painter().galley(
                egui::pos2(
                    rect.right() - 10.0 - galley.size().x,
                    rect.center().y - galley.size().y * 0.5,
                ),
                galley,
                color,
            );
            return;
        }
        let color = self.tokens.state(state.color);
        let detail = self
            .session
            .tasks
            .iter()
            .find_map(|task| task.current_activity.clone())
            .or_else(|| state.detail.clone());
        // The live workflow bar: how many distinct steps have closed against
        // how many are visible in the activity stream, plus the tool-call count
        // — the "little progress bar" the workflow feels like while it runs.
        let condensed = model::condense(&self.session.activity);
        let closed = condensed
            .iter()
            .filter(|(line, _)| matches!(line.status.as_str(), "done" | "complete"))
            .count();
        let total = condensed.len();
        let fraction = if total > 0 {
            (closed as f32 / total as f32).clamp(0.04, 1.0)
        } else {
            0.06
        };
        let tool_calls: usize = condensed.iter().map(|(_, count)| *count).sum();
        egui::Frame::new()
            .fill(self.tokens.tint(color))
            .corner_radius(theme::RADIUS_CONTROL)
            .inner_margin(egui::Margin::symmetric(10, 5))
            .show(ui, |ui| {
                // Pinned to the height the idle band reserves, so the strip
                // fills exactly the room that was already held for it and the
                // composer does not step down when a run begins.
                ui.set_min_height(WORKING_STRIP_ROOM - 10.0);
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    crate::icons::step_marker(ui, "running", 10.0, color);
                    ui.label(RichText::new(&state.label).size(11.5).color(color));
                    if let Some(detail) = detail {
                        ui.label(
                            RichText::new(detail)
                                .size(11.0)
                                .color(self.tokens.text_muted),
                        );
                    }
                    if tool_calls > 0 {
                        ui.label(
                            RichText::new(format!(
                                "{tool_calls} tool call{}",
                                if tool_calls == 1 { "" } else { "s" }
                            ))
                            .size(theme::TYPE_EYEBROW)
                            .color(self.tokens.text_muted),
                        );
                    }
                    // The progress line, Claude-bar style: a faint track with an
                    // accent fill advancing as steps close.
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let track_w = 90.0;
                        let (track, _) =
                            ui.allocate_exact_size(egui::vec2(track_w, 4.0), Sense::hover());
                        ui.painter()
                            .rect_filled(track, 2.0, self.tokens.border_subtle);
                        let fill_w = (track_w * fraction).max(3.0);
                        ui.painter().rect_filled(
                            egui::Rect::from_min_size(track.min, egui::vec2(fill_w, 4.0)),
                            2.0,
                            color,
                        );
                        ui.add_space(6.0);
                        ui.label(
                            RichText::new("Esc to stop waiting")
                                .size(theme::TYPE_EYEBROW)
                                .color(self.tokens.text_muted),
                        );
                    });
                });
            });
    }

    /// Send the daemon action behind a strip affordance, flipping the submitting
    /// flag first so the button reads as busy and cannot double-fire. The action
    /// is always the safe resume/recover kind, so a stray click can only start
    /// work again from the recorded boundary.
    fn dispatch_strip_action(&mut self, action: &'static str) {
        if let Some(session) = self.selected.clone() {
            self.submitting = true;
            self.client.send(Request::SessionAction {
                session,
                action,
                body: Value::Null,
            });
        }
    }

    /// Persistent orientation for durable work. The transcript explains what
    /// happened; this small card answers what is happening now and what comes
    /// next without requiring the user to scroll to the latest task event.
    fn current_task_card(&self, ui: &mut Ui) {
        let current_index = self
            .session
            .tasks
            .iter()
            .position(|task| task.status == "running")
            .or_else(|| {
                self.session
                    .current_task
                    .as_ref()
                    .and_then(|id| self.session.tasks.iter().position(|task| &task.id == id))
            })
            .unwrap_or(0);
        let current = &self.session.tasks[current_index];
        let next = self
            .session
            .tasks
            .iter()
            .skip(current_index + 1)
            .find(|task| task.status != "done");
        let card = egui::Frame::new()
            .fill(self.tokens.background_raised)
            .stroke(egui::Stroke::new(1.0_f32, self.tokens.border_subtle))
            .corner_radius(theme::RADIUS_CARD)
            .inner_margin(egui::Margin::symmetric(10, 8))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(format!(
                            "TASK {} / {}",
                            current_index + 1,
                            self.session.tasks.len()
                        ))
                        .size(10.0)
                        .strong()
                        .color(self.tokens.accent_primary),
                    );
                    ui.label(
                        RichText::new(&current.objective)
                            .size(11.5)
                            .strong()
                            .color(self.tokens.text_primary),
                    );
                });
                ui.horizontal(|ui| {
                    let now = current
                        .current_activity
                        .as_deref()
                        .unwrap_or(current.status.as_str());
                    ui.label(
                        RichText::new(format!("Now · {now}"))
                            .size(theme::TYPE_EYEBROW)
                            .color(self.tokens.text_secondary),
                    );
                    if let Some(next) = next {
                        ui.label(
                            RichText::new(format!("Next · {}", next.objective))
                                .size(theme::TYPE_EYEBROW)
                                .color(self.tokens.text_muted),
                        );
                    }
                });
            });
        ui.painter().line_segment(
            [
                egui::pos2(
                    card.response.rect.left() + 1.0,
                    card.response.rect.top() + 8.0,
                ),
                egui::pos2(
                    card.response.rect.left() + 1.0,
                    card.response.rect.bottom() - 8.0,
                ),
            ],
            egui::Stroke::new(2.0_f32, self.tokens.accent_primary),
        );
    }

    fn composer_widget(&mut self, ui: &mut Ui) {
        let submitting = self.submitting || self.session_loading;
        let can_submit = !self.composer.trim().is_empty() && !submitting;
        let hint = if submitting {
            "Working… Esc to cancel"
        } else {
            super::composer::COMPOSER_HINT
        };
        let focused = ui
            .ctx()
            .memory(|memory| memory.has_focus(egui::Id::new("universal_composer")));
        let completion_open = self.active_completion.is_some();
        // Set by the completion popup when it consumes the frame's Enter.
        let mut completion_took_enter = false;

        egui::Frame::new()
            .fill(self.tokens.background_raised)
            .stroke(egui::Stroke::new(
                1.0_f32,
                if focused {
                    self.tokens.accent_primary
                } else {
                    self.tokens.border_subtle
                },
            ))
            .corner_radius(theme::RADIUS_CARD)
            .inner_margin(egui::Margin::symmetric(10, 8))
            .show(ui, |ui| {
                // Measured *inside* the frame, so the field stops at the
                // padding instead of running off the right edge of the window.
                let width = ui.available_width();
                let output = egui::TextEdit::multiline(&mut self.composer)
                    .id(egui::Id::new("universal_composer"))
                    .hint_text(RichText::new(hint).color(self.tokens.text_muted))
                    .frame(false)
                    .desired_rows(2)
                    .desired_width(width)
                    .lock_focus(self.focus_composer)
                    .show(ui);
                self.focus_composer = false;

                // Track the caret so `/ @ #` completion knows which token is
                // being typed. egui counts characters; the token grammar is
                // byte-indexed, so convert once here rather than at each use.
                if let Some(range) = output.state.cursor.char_range() {
                    self.composer_caret = self
                        .composer
                        .char_indices()
                        .nth(range.primary.index)
                        .map_or(self.composer.len(), |(index, _)| index);
                }
                self.active_completion = if output.response.has_focus() {
                    super::composer::active_token(&self.composer, self.composer_caret)
                } else {
                    None
                };

                // Completions for the token being typed, and the references
                // the daemon resolved out of the draft. The popup owns Enter
                // while it is open, so accepting a suggestion cannot also
                // send the request.
                if completion_open {
                    ui.add_space(4.0);
                    completion_took_enter = self.completion_popup(ui);
                }
                self.reference_chips(ui);

                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    self.bypass_chip(ui);
                    // Intent legend: / commands, @ files, # symbols. These are
                    // now live — typing a prefix opens the completion list
                    // above — so the pills say what to type rather than
                    // pretending to be buttons.
                    if self.composer.trim().is_empty() && !submitting {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 3.0;
                            for (shortcut, name) in
                                [("/", "commands"), ("@", "files"), ("#", "symbols")]
                            {
                                egui::Frame::new()
                                    .fill(self.tokens.background_secondary)
                                    .corner_radius(theme::RADIUS_PILL)
                                    .inner_margin(egui::Margin::symmetric(6, 1))
                                    .show(ui, |ui| {
                                        ui.spacing_mut().item_spacing.x = 1.0;
                                        ui.label(
                                            RichText::new(shortcut)
                                                .size(theme::TYPE_EYEBROW)
                                                .color(self.tokens.text_muted),
                                        );
                                        ui.label(
                                            RichText::new(name)
                                                .size(theme::TYPE_EYEBROW)
                                                .color(self.tokens.text_muted),
                                        );
                                    });
                            }
                        });
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if submitting {
                            ui.spinner();
                        } else if self.send_button(ui, can_submit).clicked() && can_submit {
                            self.submit();
                        }
                    });
                });
                // Token usage meter (PRD v1.1 Phase 5).
                self.token_usage_meter(ui);
            });

        // Track input-method composition. While the IME is composing, Enter
        // confirms a candidate and Esc cancels it — neither may send or clear.
        let mut composing = false;
        for event in ui.input(|input| input.events.clone()) {
            if let egui::Event::Ime(ime) = event {
                composing = matches!(ime, egui::ImeEvent::Preedit(_));
                if matches!(ime, egui::ImeEvent::Commit(_) | egui::ImeEvent::Disabled) {
                    composing = false;
                }
            }
        }
        self.ime_composing = composing;

        // Keep the resolved-reference row in step with the draft. Debounced
        // inside `refresh_references`, which ignores an unchanged draft.
        self.refresh_references();

        // Submit on Enter (not Shift+Enter, never mid-composition, and never
        // when the completion popup already used that Enter to accept a
        // suggestion — the user was picking a file, not sending).
        if ui.input(|input| input.key_pressed(egui::Key::Enter))
            && !ui.input(|input| input.modifiers.shift)
            && can_submit
            && !self.ime_composing
            && !completion_took_enter
        {
            self.submit();
        }

        // Escape cancels a running task: the working strip advertises
        // "Esc to stop waiting", so Esc must actually stop the run on the
        // daemon, not just flip the local submitting flag (which re-opened a
        // double-send window while the first run kept executing). When idle,
        // Esc clears the draft. Never fires while the IME is composing.
        if ui.input(|input| input.key_pressed(egui::Key::Escape)) && !self.ime_composing {
            if submitting {
                if let Some(session) = self.selected.clone() {
                    self.client.send(Request::SessionAction {
                        session,
                        action: "cancel",
                        body: Value::Null,
                    });
                }
                self.submitting = false;
                self.focus_composer = true;
            } else if !self.composer.is_empty() {
                self.composer.clear();
            }
        }
    }

    /// Send: a filled accent target when there is something to send, and a
    /// quiet outline when there is not, so the control never lies about
    /// whether pressing it will do anything.
    fn send_button(&self, ui: &mut Ui, enabled: bool) -> egui::Response {
        let (rect, response) = ui.allocate_exact_size(egui::Vec2::splat(26.0), Sense::click());
        let hovered = response.hovered() && enabled;
        let fill = if !enabled {
            self.tokens.surface_hover
        } else if hovered {
            self.tokens.accent_hover
        } else {
            self.tokens.accent_primary
        };
        ui.painter().rect_filled(rect, theme::RADIUS_CONTROL, fill);
        if response.has_focus() {
            self.tokens.focus_ring(ui.painter(), rect);
        }
        crate::icons::draw(
            ui,
            egui::Rect::from_center_size(rect.center(), egui::Vec2::splat(13.0)),
            Glyph::Send,
            if enabled {
                self.tokens.accent_on
            } else {
                self.tokens.text_muted
            },
        );
        if hovered {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        response.on_hover_text(if enabled {
            "Send (Enter)"
        } else {
            "Type something to send"
        })
    }

    /// Session-scoped prompt bypass. It changes approval prompting, never the
    /// durable PawGate judgment or the execution adapter's exact recheck.
    fn bypass_chip(&mut self, ui: &mut Ui) {
        let bypass = self.session.permission_mode == "full_access";
        let color = if bypass {
            self.tokens.status_warning
        } else {
            self.tokens.text_muted
        };
        let response = egui::Frame::new()
            .fill(if bypass {
                self.tokens.tint(self.tokens.status_warning)
            } else {
                self.tokens.background_secondary
            })
            .corner_radius(theme::RADIUS_PILL)
            .inner_margin(egui::Margin::symmetric(8, 2))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    crate::icons::inline(ui, Glyph::Shield, 11.0, color);
                    ui.label(
                        RichText::new(if bypass { "Bypass on" } else { "Bypass" })
                            .size(theme::TYPE_EYEBROW)
                            .color(color),
                    );
                });
            })
            .response
            .interact(Sense::click())
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_text(
                "Run without permission prompts for this session. Durable policy judgment and exact execution authorization still apply.",
            );
        if response.has_focus() {
            self.tokens.focus_ring(ui.painter(), response.rect);
        }
        if response.clicked()
            && let Some(session) = self.selected.clone()
        {
            let permission_mode = if bypass { "auto" } else { "full_access" };
            self.client.send(Request::SetControls {
                session,
                controls: serde_json::json!({ "permission_mode": permission_mode }),
            });
        }
    }

    /// One turn of the conversation.
    ///
    /// The user's own words are a tinted card, because finding "what did I
    /// ask?" in a long transcript is a real task. The agent's reply is markdown
    /// at full width: a bubble around a paragraph of prose narrows the measure
    /// and buys nothing, and the model writes headings and lists whether or not
    /// anyone asked it to.
    ///
    /// `measure` is the transcript's content width, handed down from outside
    /// the scroll area so this method never asks how much room the scrollbar
    /// left it.
    fn message(
        &self,
        ui: &mut Ui,
        message: &model::Message,
        measure: f32,
    ) -> Option<super::checkpoints::MessageAction> {
        let response = ui
            .scope(|ui| {
                if message.is_user() {
                    self.user_bubble(ui, &message.content, measure);
                } else {
                    // The user typed plain text; rendering *their* words as
                    // markdown would put emphasis in their mouth that they
                    // did not write.
                    markdown::render(ui, &self.tokens, &message.content, measure);
                }
            })
            .response;
        // Restore/fork controls, revealed on hover so the transcript stays
        // readable (v1.2 Pillar 3).
        let hovered = ui.rect_contains_pointer(response.rect);
        let action = super::checkpoints::message_actions(ui, &self.tokens, message, hovered);
        ui.add_space(10.0);
        action
    }

    /// The user's own words, right-aligned against the transcript's edge.
    ///
    /// The bubble used to be indented by a tenth of `available_width()` inside
    /// a frame that shrink-wrapped its contents. Two things went wrong with
    /// that. The indent was measured against whatever the scrollbar left
    /// behind, so it changed the frame the transcript grew past one screen and
    /// the bubble jumped sideways. And a shrink-wrapping frame whose left edge
    /// sits at 10% is neither left- nor right-aligned: a short message became a
    /// small card floating in the middle of the column.
    ///
    /// So the text is measured first, and the bubble is placed by subtraction
    /// from a width that has nothing to do with the scrollbar. Every message in
    /// the conversation now shares one right edge, whatever its length.
    fn user_bubble(&self, ui: &mut Ui, content: &str, measure: f32) {
        let (row, cap) = user_bubble_geometry(measure);
        let galley = ui.fonts_mut(|fonts| {
            fonts.layout_job(egui::text::LayoutJob::simple(
                content.to_owned(),
                egui::FontId::new(theme::TYPE_BODY, egui::FontFamily::Proportional),
                self.tokens.text_primary,
                cap - 2.0 * USER_BUBBLE_PAD_X,
            ))
        });
        let bubble = galley.size().x + 2.0 * USER_BUBBLE_PAD_X;

        ui.horizontal(|ui| {
            ui.add_space(user_bubble_indent(row, bubble));
            egui::Frame::new()
                .fill(self.tokens.accent_soft)
                .stroke(egui::Stroke::new(
                    1.0_f32,
                    self.tokens.accent_primary.gamma_multiply(0.35),
                ))
                .corner_radius(theme::RADIUS_CARD)
                .inner_margin(egui::Margin::symmetric(USER_BUBBLE_PAD_X as i8, 8))
                .show(ui, |ui| {
                    // The galley is already laid out at the capped measure, so
                    // the frame wraps it exactly and the arithmetic above and
                    // the pixels below cannot disagree.
                    ui.add(egui::Label::new(galley));
                });
        });
    }

    /// One thing the agent did, as a card.
    ///
    /// Icon, sentence, and — when there is one — the artifact it touched, set
    /// as a code chip so a file name never gets lost in a run of prose.
    fn activity_card(&self, ui: &mut Ui, line: &model::ActivityLine, count: usize) {
        let color = self.tokens.state(match line.status.as_str() {
            "done" => purrcode_runtime_core::StateColor::Success,
            "failed" => purrcode_runtime_core::StateColor::Error,
            "running" => purrcode_runtime_core::StateColor::Running,
            "blocked" => purrcode_runtime_core::StateColor::Warning,
            _ => purrcode_runtime_core::StateColor::Neutral,
        });
        egui::Frame::new()
            .fill(self.tokens.background_raised)
            .stroke(egui::Stroke::new(1.0_f32, self.tokens.border_subtle))
            .corner_radius(theme::RADIUS_CONTROL)
            .inner_margin(egui::Margin::symmetric(10, 6))
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    crate::icons::inline(ui, activity_glyph(&line.label), 13.0, color);
                    ui.label(
                        RichText::new(&line.label)
                            .size(12.0)
                            .color(self.tokens.text_primary),
                    );
                    if let Some(summary) = &line.summary {
                        self.code_chip(ui, summary);
                    }
                    if count > 1 {
                        ui.label(
                            RichText::new(format!("×{count}"))
                                .size(theme::TYPE_EYEBROW)
                                .color(self.tokens.text_muted),
                        );
                    }
                });
            });
        ui.add_space(4.0);
    }

    /// A bounded, user-controlled record of tool activity.
    ///
    /// The final answer is the primary artifact. Routine reads, searches and
    /// validations therefore start collapsed, while failed or blocked work is
    /// rendered outside the disclosure so a problem can never be hidden by a
    /// presentation preference.
    fn work_log(&self, ui: &mut Ui, lines: &[(model::ActivityLine, usize)]) {
        let (attention, routine): (Vec<_>, Vec<_>) = lines
            .iter()
            .partition(|(line, _)| activity_needs_attention(line));

        for (line, count) in attention {
            self.activity_card(ui, line, *count);
        }
        if routine.is_empty() {
            return;
        }

        let update_count = routine.iter().map(|(_, count)| *count).sum::<usize>();
        let session_id = self.session.id.clone();
        let state_id = egui::Id::new(("purrcode_work_log_open", session_id));
        let mut open = ui.data_mut(|data| data.get_temp::<bool>(state_id).unwrap_or(false));

        let response = egui::Frame::new()
            .fill(self.tokens.background_secondary)
            .stroke(egui::Stroke::new(1.0_f32, self.tokens.border_subtle))
            .corner_radius(theme::RADIUS_CONTROL)
            .inner_margin(egui::Margin::symmetric(10, 6))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    crate::icons::inline(
                        ui,
                        if open {
                            Glyph::ChevronDown
                        } else {
                            Glyph::ChevronRight
                        },
                        12.0,
                        self.tokens.text_muted,
                    );
                    ui.label(
                        RichText::new("Work log")
                            .size(theme::TYPE_LABEL)
                            .strong()
                            .color(self.tokens.text_primary),
                    );
                    ui.label(
                        RichText::new(format!(
                            "{update_count} {}",
                            if update_count == 1 {
                                "update"
                            } else {
                                "updates"
                            }
                        ))
                        .size(theme::TYPE_META)
                        .color(self.tokens.text_muted),
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.label(
                            RichText::new(if open { "Hide details" } else { "Show details" })
                                .size(theme::TYPE_META)
                                .color(self.tokens.text_muted),
                        );
                    });
                });
            })
            .response
            .interact(Sense::click())
            .on_hover_cursor(egui::CursorIcon::PointingHand);
        if response.clicked() {
            open = !open;
            ui.data_mut(|data| data.insert_temp(state_id, open));
        }

        if open {
            ui.add_space(4.0);
            egui::ScrollArea::vertical()
                .id_salt(("purrcode_work_log", &self.session.id))
                .max_height(240.0)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    for (line, count) in routine {
                        self.activity_card(ui, line, *count);
                    }
                });
        }
    }

    /// A file name, a command, a symbol — anything that is code rather than
    /// prose and should not be read as a sentence.
    ///
    /// The appearance is `markdown::code_chip`'s, not this file's: the same
    /// path can appear here in the work log and three lines away inside an
    /// answer, and two descriptions of one chip is how those two ended up in
    /// different sizes and different colours. The frame is still drawn here
    /// because this chip is a widget in a wrapped row, where the one in a reply
    /// is a run inside a galley.
    fn code_chip(&self, ui: &mut Ui, text: &str) {
        let chip = crate::markdown::code_chip(&self.tokens);
        egui::Frame::new()
            .fill(chip.fill)
            .corner_radius(theme::RADIUS_CONTROL)
            .inner_margin(egui::Margin::symmetric(6, 1))
            .show(ui, |ui| {
                ui.label(RichText::new(text).font(chip.font.clone()).color(chip.text));
            });
    }

    /// The agent's changes, rendered inline in the conversation.
    ///
    /// Collapsed by default so a long diff does not own the stream; the header
    /// toggles it, and "Open full view" moves it into the auxiliary panel.
    /// Returns `true` when the collapse/expand toggle was pressed, so the
    /// caller can mutate `inline_diff_open` after the `&self` closures end.
    fn inline_diff(&self, ui: &mut Ui, diff: &str, open_aux: &mut bool) -> bool {
        let expanded = self.inline_diff_open;
        let mut toggle = false;
        self.tokens.card().show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                crate::icons::inline(ui, Glyph::Diff, 13.0, self.tokens.status_info);
                ui.label(
                    RichText::new(format!("Changes — {}", self.diff_scope.label()))
                        .size(12.0)
                        .strong()
                        .color(self.tokens.text_primary),
                );
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.small_button("Open full view").clicked() {
                        *open_aux = true;
                    }
                    if ui
                        .small_button(if expanded { "Collapse" } else { "Expand" })
                        .clicked()
                    {
                        toggle = true;
                    }
                });
            });
            if expanded {
                ui.add_space(6.0);
                self.render_diff(ui, diff, "inline_diff_scroll");
            }
        });
        toggle
    }

    fn plan_preview(&mut self, ui: &mut Ui) {
        let mut resume = false;
        let mut view_plan = false;
        egui::Frame::new()
            .fill(self.tokens.tint(self.tokens.status_info))
            .stroke(egui::Stroke::new(
                1.0_f32,
                self.tokens.status_info.gamma_multiply(0.45),
            ))
            .corner_radius(theme::RADIUS_CARD)
            .inner_margin(egui::Margin::symmetric(11, 8))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    crate::icons::inline(ui, Glyph::Checklist, 13.0, self.tokens.status_info);
                    ui.label(
                        RichText::new("Plan ready for review")
                            .size(12.0)
                            .strong()
                            .color(self.tokens.status_info),
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        // The way out of plan review: approve the plan and
                        // start building. Typing in the composer stays plan
                        // feedback; this is the explicit "go" that resumes
                        // into execution.
                        if ui
                            .small_button("Build this plan")
                            .on_hover_text("Approve the plan and start executing it")
                            .clicked()
                        {
                            resume = true;
                        }
                        if ui.small_button("View plan").clicked() {
                            view_plan = true;
                        }
                    });
                });
            });
        if resume && let Some(session) = self.selected.clone() {
            self.client.send(Request::SessionAction {
                session,
                action: "resume",
                body: Value::Null,
            });
        }
        if view_plan {
            // The dock is what actually renders; `self.dock` is only the flag
            // the dock body reads while it draws.
            self.bottom_panel = Some(DockTab::Activity);
        }
    }

    fn recovery_card(&mut self, ui: &mut Ui) {
        let reconciled = self.session.recovery_reconciled;
        let mut action = None;
        let mut inspect = false;
        egui::Frame::new()
            .fill(self.tokens.tint(self.tokens.status_warning))
            .stroke(egui::Stroke::new(
                1.0_f32,
                self.tokens.status_warning.gamma_multiply(0.55),
            ))
            .corner_radius(theme::RADIUS_CARD)
            .inner_margin(egui::Margin::symmetric(11, 9))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    crate::icons::inline(ui, Glyph::Sparkle, 13.0, self.tokens.status_warning);
                    ui.vertical(|ui| {
                        ui.label(
                            RichText::new(if reconciled {
                                "Recovery check complete"
                            } else {
                                "Interrupted turn needs recovery"
                            })
                            .size(12.0)
                            .strong()
                            .color(self.tokens.text_primary),
                        );
                        ui.label(
                            RichText::new(if reconciled {
                                "The durable log and isolated worktree agree. Resume will replan from the recorded boundary; no uncertain effect was replayed."
                            } else {
                                "First inspect the isolated worktree against the durable log. PurrCode will not replay or discard an uncertain effect."
                            })
                            .size(11.0)
                            .color(self.tokens.text_secondary),
                        );
                    });
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui
                            .add_enabled(
                                !self.submitting,
                                egui::Button::new(if reconciled {
                                    "Resume safely"
                                } else {
                                    "Check recovery"
                                }),
                            )
                            .clicked()
                        {
                            action = Some(if reconciled { "resume" } else { "recover" });
                        }
                        if ui.small_button("Inspect changes").clicked() {
                            inspect = true;
                        }
                    });
                });
            });

        if inspect {
            self.aux_panel = Some(super::AuxView::Source);
            self.refresh_diff();
        }
        if let Some(action) = action
            && let Some(session) = self.selected.clone()
        {
            self.submitting = true;
            self.client.send(Request::SessionAction {
                session,
                action,
                body: Value::Null,
            });
        }
    }

    fn task_progress(&self, ui: &mut Ui) {
        self.tokens.card().show(ui, |ui| {
            ui.label(
                RichText::new("TASKS")
                    .size(theme::TYPE_EYEBROW)
                    .strong()
                    .color(self.tokens.text_muted),
            );
            ui.add_space(6.0);
            for task in &self.session.tasks {
                let (color, marker) = match task.status.as_str() {
                    "done" | "complete" => (self.tokens.status_success, "done"),
                    "running" | "in_progress" => (self.tokens.status_running, "running"),
                    "failed" => (self.tokens.status_error, "failed"),
                    "blocked" => (self.tokens.status_warning, "blocked"),
                    _ => (self.tokens.text_muted, "pending"),
                };
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    crate::icons::step_marker(ui, marker, 10.0, color);
                    ui.label(
                        RichText::new(&task.objective)
                            .size(12.0)
                            .color(self.tokens.text_secondary),
                    );
                    if let Some(activity) = &task.current_activity {
                        ui.label(
                            RichText::new(activity)
                                .size(11.0)
                                .color(self.tokens.text_muted),
                        );
                    }
                });
                ui.add_space(3.0);
            }
        });
    }

    /// Token usage meter shown below the composer (PRD v1.1 Phase 5).
    ///
    /// Renders a compact, proportional bar chart with one segment per
    /// ContextClass, sourced from the Phase 1 context-ledger endpoint.
    /// Design intent (impeccable "product" surface rules):
    /// - Typographic: all type at TYPE_EYEBROW, no bold — quiet monitoring, not
    ///   a dashboard alert.
    /// - Color: each segment uses a tinted neutral drawn from the status palette
    ///   so the bar reads as context rather than traffic.
    /// - Size: the bar is 4px tall and fits on one line; the meter must never
    ///   compete with the composer for attention.
    /// - Contrast: labels use text_muted on the canvas background, and the bar
    ///   segments are solid against the background_secondary track.
    fn token_usage_meter(&self, ui: &mut Ui) {
        let usage = &self.session.usage;
        if usage.total_tokens == 0 {
            return;
        }
        ui.add_space(4.0);

        // Context row: proportional bar comparing the *current turn's*
        // actual prompt size against the model's effective capacity (its
        // context window minus the daemon's reserved-output budget). This
        // answers "how close is the next request to overflowing" — a
        // different question from the cumulative Session row below it, which
        // answers "how much has this session cost so far". Only the daemon
        // knows either number (they depend on the configured provider's
        // resolved capabilities and the live context ledger), so — mirroring
        // the capacity-gated bar this replaced — the row is simply omitted
        // whenever either is unresolved rather than drawn against a
        // fabricated ceiling.
        if let (Some(current), Some(capacity)) = (
            usage.current_context_tokens,
            usage
                .effective_capacity_tokens
                .filter(|capacity| *capacity > 0),
        ) {
            let track_height = 4.0;
            let bar_width = ui.available_width().min(240.0);

            let accent = self.tokens.accent_primary;
            let track = self.tokens.background_secondary;

            let (rect, response) =
                ui.allocate_exact_size(egui::vec2(bar_width, track_height), egui::Sense::hover());
            let painter = ui.painter();
            painter.rect_filled(rect, theme::RADIUS_PILL, track);

            let fill_frac = (current as f64 / capacity as f64).clamp(0.0, 1.0) as f32;
            if fill_frac > 0.0 {
                let mut fill_rect = rect;
                fill_rect.set_width(rect.width() * fill_frac);
                painter.rect_filled(fill_rect, theme::RADIUS_PILL, accent);
            }
            response.on_hover_text(format!(
                "{current} of {capacity} tokens in the current turn's prompt, against what the model can fill before compaction"
            ));

            ui.add_space(3.0);
            ui.label(
                RichText::new(format!(
                    "{} / {} context",
                    format_token_count(current),
                    format_token_count(capacity)
                ))
                .size(theme::TYPE_EYEBROW)
                .color(self.tokens.text_muted),
            );
        }

        // Session row: cumulative totals across the whole session. Prefixed
        // with a "Session" label so it reads as clearly distinct from the
        // per-turn Context row above rather than as a continuation of it.
        let total_str = format_token_count(usage.total_tokens);
        ui.add_space(3.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            ui.label(
                RichText::new("Session")
                    .size(theme::TYPE_EYEBROW)
                    .color(self.tokens.text_muted),
            );
            ui.label(
                RichText::new(format!("{total_str} tokens"))
                    .size(theme::TYPE_EYEBROW)
                    .color(self.tokens.text_muted),
            )
            .on_hover_text(
                "Total tokens used across this session, not the size of the next request",
            );
            if usage.model_calls > 0 {
                ui.label(
                    RichText::new(format!("· {} calls", usage.model_calls))
                        .size(theme::TYPE_EYEBROW)
                        .color(self.tokens.text_muted),
                );
            }
            if let Some(ref cost) = usage.estimated_cost
                && !cost.is_empty()
            {
                ui.label(
                    RichText::new(format!("· {cost}"))
                        .size(theme::TYPE_EYEBROW)
                        .color(self.tokens.text_muted),
                );
            }
        });
    }
}

/// Compact K/M form for a token count, shared by the token usage meter's
/// Context and Session rows so the two never drift into slightly different
/// rounding or suffix rules.
fn format_token_count(tokens: u64) -> String {
    if tokens >= 1_000_000 {
        format!("{:.1}M", tokens as f64 / 1_000_000.0)
    } else if tokens >= 1000 {
        format!("{:.0}K", tokens as f64 / 1000.0)
    } else {
        format!("{tokens}")
    }
}

/// The daemon route a strip affordance maps to.
///
/// Nearly every primary action resumes the session, so the fallback is
/// "resume" and only the labels with a genuinely different route opt out.
fn resume_action(action_label: &str) -> &'static str {
    match action_label {
        "Recover session" => "recover",
        "Approve" => "approve",
        _ => "resume",
    }
}

/// Pick the icon that matches what a step actually did.
///
/// The activity stream is prose written by the runtime, so the mapping is on
/// words rather than on a type tag: a step whose wording is not recognised gets
/// the neutral agent mark rather than a wrong one.
fn activity_glyph(label: &str) -> Glyph {
    let text = label.to_ascii_lowercase();
    let any = |needles: &[&str]| needles.iter().any(|needle| text.contains(needle));
    if any(&["read", "inspect", "open", "review"]) {
        Glyph::Eye
    } else if any(&["search", "grep", "find", "locate"]) {
        Glyph::Search
    } else if any(&["command", "ran ", "run ", "shell", "terminal", "build"]) {
        Glyph::Terminal
    } else if any(&["diagnos", "error", "failed", "problem"]) {
        Glyph::Bug
    } else if any(&["test", "validat", "check"]) {
        Glyph::Beaker
    } else if any(&["edit", "wrote", "write", "chang", "appl", "patch"]) {
        Glyph::Pencil
    } else if any(&["plan", "task", "step"]) {
        Glyph::Checklist
    } else if any(&["commit", "branch", "merge", "push"]) {
        Glyph::Branch
    } else {
        Glyph::Sparkle
    }
}

fn activity_needs_attention(line: &model::ActivityLine) -> bool {
    matches!(line.status.as_str(), "failed" | "blocked")
}

/// The row a user's bubble is placed in, and the widest that bubble may be.
///
/// Derived from nothing but the width handed down from outside the scroll area,
/// which is the whole of the fix: there is no term in here that a scrollbar can
/// change, so the bubble cannot move sideways as the conversation grows.
fn user_bubble_geometry(measure: f32) -> (f32, f32) {
    let row = measure.max(USER_BUBBLE_FLOOR);
    let cap = (row * USER_BUBBLE_SHARE).clamp(USER_BUBBLE_FLOOR.min(row), row);
    (row, cap)
}

/// Where a bubble of a given width starts, so that it ends at the row's edge.
///
/// Placing by subtraction from the right rather than by a fixed indent from the
/// left is what makes a one-word question and a six-line one line up: they are
/// aligned to the thing they share, which is the edge of the column.
fn user_bubble_indent(row: f32, bubble: f32) -> f32 {
    (row - bubble).max(0.0)
}

/// How tall the transcript's scroll area is, given the room the workbench got.
///
/// `_running` is accepted and then deliberately ignored. It used to add the
/// working strip's height to the reservation, which moved the transcript's
/// bottom edge — and, because the area sticks to the bottom, every line the
/// reader was looking at — the instant a turn started and again when it ended.
/// The strip's band is now held open whether or not it draws, so this number is
/// a constant subtraction and the conversation holds still. Keeping the
/// parameter in the signature is what lets a test say so out loud.
fn transcript_height(available: f32, _running: bool) -> f32 {
    (available - COMPOSER_ROOM - WORKING_STRIP_ROOM - WORKING_STRIP_GAP).max(90.0)
}

/// The message the current turn's work log is drawn under.
///
/// Anchored by exact `turn_id`, matched against the *user* message that
/// started the turn: the log belongs under the request that carries the same
/// `turn_id` as the activity being shown, the identity `run_until_pause`
/// stamps on every `ProposedAction`/`ActionOutputRecorded`/`JudgmentRecorded`
/// it emits for one turn and that the IDE now threads onto both `Message` and
/// `ActivityLine` (PRD v1.1 §6.3). This replaced a positional guess — "the
/// last user message" — that broke the moment a turn was not exactly "one
/// user message, then the agent's activity": correlating by identity instead
/// of position is what stopped the work log jumping between the request and
/// the final answer as a turn completed.
///
/// The daemon stamps the *same* `turn_id` on both the user message that opens
/// a turn and the assistant's final answer to it, so matching on `turn_id`
/// alone is not enough: once the answer lands, both messages carry the turn,
/// and an unqualified `rposition` — searching from the end — finds the
/// assistant's reply instead of the request, which is exactly the jump this
/// function exists to prevent. Requiring `message.is_user()` alongside the
/// `turn_id` match is what keeps the anchor pinned to the request even after
/// the answer arrives.
///
/// The turn identity itself is read by scanning `activity` from the end and
/// taking the first `Some`, not by reading `activity.last()` directly:
/// the most recent activity line can be an aggregated summary with no single
/// owning turn (`turn_id: None`) even while an earlier line in the same batch
/// does carry one, so `.last()?.turn_id` would go missing exactly when a
/// running turn's own activity is still on screen.
///
/// `None` when no activity item carries a turn identity yet (nothing has run)
/// or the turn names no on-screen user message — a session restored from
/// durable history can begin with an assistant line and no matching turn at
/// all. Either way the caller falls back to the end of the transcript, the
/// only honest place for activity that cannot be pinned to a request.
fn work_log_anchor(messages: &[model::Message], activity: &[model::ActivityLine]) -> Option<usize> {
    let turn = activity.iter().rev().find_map(|item| item.turn_id)?;
    messages
        .iter()
        .rposition(|message| message.is_user() && message.turn_id == Some(turn))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The minimal `PurrCodeIde` a test can render a session with: every field
    /// is the default except the session this surface draws. `message` only
    /// touches `self.tokens` and `self.session`, so this is enough to exercise
    /// a real bubble.
    fn with_session(session: &model::Session) -> PurrCodeIde {
        let mut ide = PurrCodeIde::new(
            &egui::Context::default(),
            crate::app::Config {
                repository: None,
                base_url: "http://127.0.0.1:0".into(),
                token: String::new(),
                initial_session: None,
            },
        );
        ide.session = session.clone();
        ide
    }

    #[test]
    fn a_primary_action_maps_to_the_daemon_route_that_serves_it() {
        // "Recover session" goes to the daemon's recovery endpoint; approval
        // travels the approval route. Every other primary action — including
        // labels the IDE does not yet special-case — safely resumes the session
        // from its recorded boundary, so the fallback must not change.
        assert_eq!(resume_action("Recover session"), "recover");
        assert_eq!(resume_action("Approve"), "approve");
        for label in [
            "Retry",
            "Start again",
            "Build this plan",
            "Review changes",
            "Create pull request",
            "A primary action this IDE has not seen",
        ] {
            assert_eq!(resume_action(label), "resume", "{label}");
        }
    }

    #[test]
    fn an_activity_line_gets_the_icon_its_wording_implies() {
        assert_eq!(activity_glyph("Read file(s)"), Glyph::Eye);
        assert_eq!(activity_glyph("Searched the repository"), Glyph::Search);
        assert_eq!(activity_glyph("Ran a command"), Glyph::Terminal);
        assert_eq!(activity_glyph("Checked diagnostics"), Glyph::Bug);
        assert_eq!(activity_glyph("Validation passed"), Glyph::Beaker);
        assert_eq!(activity_glyph("Applied an edit"), Glyph::Pencil);
        assert_eq!(activity_glyph("Created a plan"), Glyph::Checklist);
    }

    #[test]
    fn unrecognised_wording_falls_back_to_the_agent_mark() {
        // A wrong icon is worse than a neutral one: it tells the user the
        // agent did something it did not do.
        assert_eq!(activity_glyph("Thinking about it"), Glyph::Sparkle);
        assert_eq!(activity_glyph(""), Glyph::Sparkle);
    }

    #[test]
    fn the_icon_mapping_is_case_insensitive() {
        assert_eq!(activity_glyph("READ FILE"), Glyph::Eye);
        assert_eq!(activity_glyph("read file"), Glyph::Eye);
    }

    #[test]
    fn failed_and_blocked_work_never_disappears_inside_the_collapsed_log() {
        let line = |status: &str| model::ActivityLine {
            label: "step".into(),
            status: status.into(),
            summary: None,
            ..Default::default()
        };
        assert!(activity_needs_attention(&line("failed")));
        assert!(activity_needs_attention(&line("blocked")));
        assert!(!activity_needs_attention(&line("done")));
        assert!(!activity_needs_attention(&line("running")));
    }

    fn message(role: &str, content: &str) -> model::Message {
        model::Message {
            role: role.into(),
            content: content.into(),
            timestamp: String::new(),
            ..Default::default()
        }
    }

    /// Like [`message`], but stamped with a specific `turn_id` — for
    /// exercising `work_log_anchor`'s exact-identity correlation, where the
    /// default (each call gets its own fresh `TurnId`) would never match
    /// anything by construction.
    fn message_in_turn(
        role: &str,
        content: &str,
        turn_id: purrcode_runtime_core::TurnId,
    ) -> model::Message {
        model::Message {
            turn_id: Some(turn_id),
            ..message(role, content)
        }
    }

    /// One activity line stamped with a specific `turn_id`, standing in for
    /// whatever `run_until_pause` produced during that turn — the anchor
    /// scans `activity` from the end for the first line that carries a turn,
    /// so the rest of the fields are unconstrained.
    fn activity_in_turn(turn_id: purrcode_runtime_core::TurnId) -> model::ActivityLine {
        model::ActivityLine {
            label: "step".into(),
            status: "running".into(),
            turn_id: Some(turn_id),
            ..Default::default()
        }
    }

    #[test]
    fn the_work_log_stays_put_when_the_reply_lands() {
        // One turn. The log is drawn under the request while the agent works,
        // and it is still drawn under the same request afterwards — the card
        // does not move when the answer arrives.
        let turn = purrcode_runtime_core::TurnId::new();
        let activity = vec![activity_in_turn(turn)];
        let working = vec![message_in_turn("user", "improve it", turn)];
        let answered = vec![
            message_in_turn("user", "improve it", turn),
            // The daemon stamps the assistant's final answer with the same
            // `turn_id` as the user message that opened the turn — using
            // `message_in_turn` here (not a plain `message`) reproduces that
            // real shared-identity data, so this assertion actually exercises
            // the `is_user()` guard in `work_log_anchor` instead of passing
            // vacuously because the assistant message never matched anyway.
            message_in_turn("assistant", "Here.", turn),
        ];
        assert_eq!(work_log_anchor(&working, &activity), Some(0));
        assert_eq!(work_log_anchor(&answered, &activity), Some(0));
    }

    #[test]
    fn the_work_log_stays_put_across_three_turns() {
        let first = purrcode_runtime_core::TurnId::new();
        let second = purrcode_runtime_core::TurnId::new();
        let third = purrcode_runtime_core::TurnId::new();
        let mut transcript = vec![
            message_in_turn("user", "first", first),
            message("assistant", "one"),
            message_in_turn("user", "second", second),
            message("assistant", "two"),
            message_in_turn("user", "introduce a plan to improve it", third),
        ];
        let activity = vec![activity_in_turn(third)];
        // The third request is the anchor while it is still being worked on…
        assert_eq!(work_log_anchor(&transcript, &activity), Some(4));
        transcript.push(message("assistant", "three"));
        // …and the same one after the answer lands. Anchoring to the last
        // assistant message would have moved this from 5 to 4 and back to 5
        // again on the next follow-up.
        assert_eq!(work_log_anchor(&transcript, &activity), Some(4));
    }

    #[test]
    fn anchoring_uses_turn_id_not_the_old_last_user_message_heuristic() {
        // PRD v1.1 §14.1: the deleted `work_log_anchor` used to be
        // `messages.iter().rposition(model::Message::is_user)` — anchor to
        // whichever user message is *last in the transcript*, regardless of
        // which turn produced the activity being shown. A later, unrelated
        // user message (e.g. a follow-up queued while the first turn is still
        // running) sits after the first turn's message in the transcript, so
        // the old heuristic would have anchored the first turn's still-running
        // work log under the *second* message — a stale-looking scroll jump.
        // Exact `turn_id` correlation must anchor it under the first message
        // instead.
        let first_turn = purrcode_runtime_core::TurnId::new();
        let second_turn = purrcode_runtime_core::TurnId::new();
        let transcript = vec![
            message_in_turn("user", "start the long task", first_turn),
            message_in_turn("user", "also do this while you're at it", second_turn),
        ];
        let activity = vec![activity_in_turn(first_turn)];

        // The old `rposition(is_user)` heuristic, kept here only to prove the
        // real function no longer matches it.
        let old_heuristic_anchor = transcript.iter().rposition(model::Message::is_user);
        assert_eq!(
            old_heuristic_anchor,
            Some(1),
            "sanity check: the old heuristic would anchor to the last user message"
        );

        assert_eq!(
            work_log_anchor(&transcript, &activity),
            Some(0),
            "exact turn_id correlation must anchor to the message that started this turn, \
             not to whichever user message happens to be last in the transcript"
        );
        assert_ne!(
            work_log_anchor(&transcript, &activity),
            old_heuristic_anchor,
            "no positional fallback may remain: turn_id correlation must diverge from the \
             deleted rposition(is_user) heuristic exactly when they disagree"
        );
    }

    #[test]
    fn a_transcript_with_no_request_in_it_anchors_nowhere() {
        // No activity at all: nothing to anchor.
        assert_eq!(work_log_anchor(&[], &[]), None);
        // Activity names a turn no on-screen message belongs to — a session
        // restored from durable history can begin with an assistant line and
        // no matching turn. Both fall through to the caller's "render at the
        // end" path, which is the only honest place for activity that cannot
        // be pinned to a request.
        let activity = vec![activity_in_turn(purrcode_runtime_core::TurnId::new())];
        assert_eq!(
            work_log_anchor(&[message("assistant", "restored from history")], &activity),
            None
        );
    }

    #[test]
    fn the_transcript_is_the_same_height_running_or_idle() {
        // The regression this guards: the reservation used to be
        // `104.0 + if running { 34.0 } else { 0.0 }`, so the transcript's
        // bottom edge — and every line of conversation above it — moved 34px
        // when a turn started and 34px back when it finished.
        for available in [300.0_f32, 700.0, 1200.0] {
            assert_eq!(
                transcript_height(available, true),
                transcript_height(available, false),
                "the transcript may not resize when a run starts"
            );
        }
    }

    #[test]
    fn a_cramped_workbench_still_leaves_a_transcript_to_read() {
        // Less room than the composer alone needs: the height floors instead of
        // going negative and inverting the scroll area.
        assert_eq!(transcript_height(10.0, true), 90.0);
        assert_eq!(transcript_height(0.0, false), 90.0);
    }

    #[test]
    fn a_user_bubble_is_capped_well_short_of_the_full_measure() {
        // A bubble that fills the column is indistinguishable from the reply
        // below it, and one wider than the column would push the transcript
        // sideways.
        for measure in [0.0_f32, 180.0, 420.0, 900.0, 1600.0] {
            let (row, cap) = user_bubble_geometry(measure);
            assert!(cap <= row, "a bubble may never be wider than its row");
            assert!(
                cap - 2.0 * USER_BUBBLE_PAD_X > 0.0,
                "the padding may never eat the whole measure"
            );
        }
        // At any width a person actually reads at, the cap leaves the agent's
        // reply visibly wider than the request above it.
        let (row, cap) = user_bubble_geometry(900.0);
        assert!(cap < row - 100.0);
    }

    #[test]
    fn every_user_bubble_shares_one_right_edge_whatever_its_length() {
        // The defect: a short message ("introduce a plan to improve it") became
        // a small card whose *left* edge sat at 10% of the width, so it read as
        // floating in the middle of the column and shifted whenever that width
        // moved. Alignment is now to the edge every message has in common.
        let (row, _) = user_bubble_geometry(900.0);
        for bubble in [64.0_f32, 210.0, 648.0] {
            assert_eq!(
                user_bubble_indent(row, bubble) + bubble,
                row,
                "a {bubble}pt bubble must still end at the row's edge"
            );
        }
        // A bubble somehow wider than its row starts at the left rather than
        // at a negative offset.
        assert_eq!(user_bubble_indent(row, row + 50.0), 0.0);
    }

    #[test]
    fn a_narrow_panel_still_gets_a_bubble_wide_enough_to_hold_words() {
        // The auxiliary panel can be dragged narrower than the floor. The
        // bubble stops shrinking there rather than collapsing to a sliver, and
        // the row grows with it so the placement arithmetic stays positive.
        let (row, cap) = user_bubble_geometry(40.0);
        assert_eq!(row, USER_BUBBLE_FLOOR);
        assert_eq!(cap, USER_BUBBLE_FLOOR);
        assert_eq!(user_bubble_indent(row, cap), 0.0);
    }

    #[test]
    fn the_first_user_message_renders_as_a_bubble_not_a_hidden_duplicate() {
        // The objective prompt is not swallowed: even when it restates the
        // session objective exactly, it must render through the same user
        // bubble path as every follow-up, so a single-turn session shows one
        // consistent user message.
        use crate::theme::Appearance;
        let ctx = egui::Context::default();
        crate::theme::install(&ctx, Appearance::Dark);
        let tokens = crate::theme::Tokens::for_appearance(Appearance::Dark);

        let session = model::Session {
            objective: "Fix the login flow".into(),
            messages: vec![message("user", "Fix the login flow")],
            ..Default::default()
        };

        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::Vec2::new(900.0, 700.0),
            )),
            ..Default::default()
        };
        let output = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                with_session(&session).active_workbench(ui);
            });
        });

        // The user's own words must be visible as the accent-soft bubble, not
        // hidden behind the objective heading. Collect every painted fill.
        let mut fills = Vec::new();
        for clipped in &output.shapes {
            collect_fills(&clipped.shape, &mut fills);
        }
        assert!(
            fills.contains(&tokens.accent_soft),
            "the first user message must be drawn as the accent-soft bubble"
        );
    }

    /// Recursively gather the fill colours of every painted rectangle.
    fn collect_fills(shape: &egui::epaint::Shape, fills: &mut Vec<egui::Color32>) {
        match shape {
            egui::epaint::Shape::Vec(shapes) => {
                for shape in shapes {
                    collect_fills(shape, fills);
                }
            }
            egui::epaint::Shape::Rect(rect) => fills.push(rect.fill),
            _ => {}
        }
    }

    /// Recursively gather every string painted as text.
    fn collect_text(shape: &egui::epaint::Shape, text: &mut Vec<String>) {
        match shape {
            egui::epaint::Shape::Vec(shapes) => {
                for shape in shapes {
                    collect_text(shape, text);
                }
            }
            egui::epaint::Shape::Text(shape) => text.push(shape.galley.text().to_owned()),
            _ => {}
        }
    }

    #[test]
    fn the_session_state_is_still_visible_when_idle() {
        // The workbench heading used to carry the right-aligned state label
        // ("Ready", "Needs recovery"). With the heading gone, that label must
        // survive in the idle working strip instead of being dropped.
        use crate::theme::Appearance;
        let ctx = egui::Context::default();
        crate::theme::install(&ctx, Appearance::Dark);

        let session = model::Session {
            objective: "Fix the login flow".into(),
            messages: vec![message("user", "Fix the login flow")],
            ..Default::default()
        };

        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::Vec2::new(900.0, 700.0),
            )),
            ..Default::default()
        };
        let output = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                with_session(&session).active_workbench(ui);
            });
        });

        let mut strings = Vec::new();
        for clipped in &output.shapes {
            collect_text(&clipped.shape, &mut strings);
        }
        assert!(
            strings.iter().any(|s| s.trim() == "Ready"),
            "the idle session state must still be painted: {strings:?}"
        );
    }
}
