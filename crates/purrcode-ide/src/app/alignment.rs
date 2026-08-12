//! What PurrCode understood, and why it believes it is done (v1.5 §19–§22).
//!
//! The runtime half of v1.5 is invisible. A user watching PurrCode work sees
//! the same conversation they saw in v1.4; the contract, the reviewers and the
//! delivery gate all happen off-screen. That is the difference between a
//! release and an implementation detail — if the panel does not exist, the user
//! cannot answer any of the four questions the whole release is built around:
//!
//! > What did it understand? Which requirements passed? What is it still
//! > fixing? Why does it think this is done?
//!
//! So this is one panel in the work log, collapsed by default, with the tally
//! on its header line. It follows the worker tree's rule (§16): a surface that
//! is always open and usually empty trains the user to ignore it, and by the
//! time it has something urgent to say they have stopped looking.
//!
//! Two things are deliberately *not* rendered. There is no percentage anywhere,
//! because "94% aligned" cannot be wrong and therefore cannot be informative.
//! And a `verified` row whose trace is empty is marked as unsupported rather
//! than given a checkmark — the false `Done` in presentation form is a tick
//! next to an empty evidence list, and a user should not have to expand every
//! row to discover it.

use egui::{RichText, Ui};

use super::PurrCodeIde;
use crate::daemon::Request;
use crate::icons::Glyph;
use crate::theme;

/// How often the surface refreshes while a run is live.
const POLL: std::time::Duration = std::time::Duration::from_millis(1500);

impl PurrCodeIde {
    /// Keeps the alignment surface current for the selected session.
    pub(crate) fn poll_alignment(&mut self) {
        let Some(session) = self.selected.clone() else {
            self.alignment = crate::model::Alignment::default();
            self.alignment_for = None;
            return;
        };
        if self.alignment_for.as_deref() != Some(session.as_str()) {
            self.alignment = crate::model::Alignment::default();
            self.alignment_for = Some(session.clone());
            self.alignment_expanded.clear();
            self.last_alignment_poll = std::time::Instant::now();
            self.client.send(Request::Alignment { session });
            return;
        }
        // A finished session's contract cannot change, so it is read once. A
        // live one is re-read: the requirement tally is the most useful thing
        // on the screen while the agent is working, and a stale one is worse
        // than none.
        let live = self
            .sessions
            .iter()
            .any(|record| record.id == session && record.state.execution_active());
        if !live && self.alignment.present {
            return;
        }
        if self.last_alignment_poll.elapsed() >= POLL {
            self.last_alignment_poll = std::time::Instant::now();
            self.client.send(Request::Alignment { session });
        }
    }

    /// The alignment panel, or nothing when this session has no contract.
    ///
    /// Returns the requirement the user clicked, if any. The transcript is
    /// drawn while the message list is borrowed, so the click is recorded here
    /// and applied by the caller — the same shape `worker_tree` uses.
    #[must_use]
    pub(crate) fn alignment_panel(&self, ui: &mut Ui) -> Option<String> {
        if !self.alignment.worth_showing() {
            return None;
        }
        let tokens = self.tokens;
        let alignment = self.alignment.clone();
        let state_id = egui::Id::new(("purrcode_alignment", self.session.id.clone()));
        let mut open = ui.data_mut(|data| data.get_temp::<bool>(state_id).unwrap_or(false));
        let mut toggled: Option<String> = None;

        let response = egui::Frame::new()
            .fill(tokens.background_secondary)
            .stroke(egui::Stroke::new(1.0_f32, tokens.border_subtle))
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
                        11.0,
                        tokens.text_muted,
                    );
                    ui.label(
                        RichText::new(phase_label(&alignment.phase))
                            .size(theme::TYPE_META)
                            .color(tokens.text_primary),
                    );
                    ui.label(
                        RichText::new("·")
                            .size(theme::TYPE_META)
                            .color(tokens.text_muted),
                    );
                    // Two integers. There is no percentage here and there is
                    // deliberately nowhere to put one.
                    ui.label(
                        RichText::new(alignment.tally())
                            .size(theme::TYPE_META)
                            .color(tokens.text_secondary),
                    );
                    let blocking = alignment.blocking_findings();
                    if blocking > 0 {
                        ui.label(
                            RichText::new(format!("· {blocking} blocking"))
                                .size(theme::TYPE_META)
                                .color(tokens.status_error),
                        );
                    }
                });

                if !open {
                    // Collapsed, one thing still earns a line: the sentence
                    // saying why the task has not ended. Everything else can
                    // wait for the user to ask.
                    if let Some(still) = alignment.still_working_on.as_ref() {
                        ui.label(
                            RichText::new(first_line(still))
                                .size(theme::TYPE_EYEBROW)
                                .color(tokens.text_muted),
                        );
                    }
                    return;
                }

                ui.add_space(6.0);
                section(ui, &tokens, "What PurrCode understood");
                ui.label(
                    RichText::new(&alignment.objective)
                        .size(theme::TYPE_META)
                        .color(tokens.text_primary),
                );
                if alignment.revision > 1 {
                    // The contract changed direction. Silently showing the new
                    // text would hide that a correction landed.
                    ui.label(
                        RichText::new(format!(
                            "revised {} time(s) after your corrections",
                            alignment.revision - 1
                        ))
                        .size(theme::TYPE_EYEBROW)
                        .color(tokens.text_muted),
                    );
                }

                ui.add_space(4.0);
                for clause in &alignment.must_satisfy {
                    let expanded = self.alignment_expanded.contains(&clause.id);
                    let trace = alignment.traces.get(&clause.id);
                    ui.horizontal(|ui| {
                        ui.add_space(8.0);
                        let colour = match clause.status.as_str() {
                            "verified" => tokens.status_success,
                            "not satisfied" => tokens.status_error,
                            "undetermined" => tokens.status_warning,
                            _ => tokens.text_muted,
                        };
                        if ui
                            .label(
                                RichText::new(clause.marker)
                                    .monospace()
                                    .size(theme::TYPE_META)
                                    .color(colour),
                            )
                            .clicked()
                        {
                            toggled = Some(clause.id.clone());
                        }
                        if ui
                            .label(
                                RichText::new(&clause.statement)
                                    .size(theme::TYPE_META)
                                    .color(tokens.text_primary),
                            )
                            .on_hover_text("Why does PurrCode believe this?")
                            .clicked()
                        {
                            toggled = Some(clause.id.clone());
                        }
                        // A tick with nothing behind it is the thing this
                        // release exists to stop, so it is called out on the
                        // row rather than hidden one expand away.
                        if trace.is_some_and(|trace| !trace.supported) {
                            ui.label(
                                RichText::new("unsupported")
                                    .size(theme::TYPE_EYEBROW)
                                    .color(tokens.status_error),
                            )
                            .on_hover_text(
                                "This is shown as verified and nothing validated or reviewed it",
                            );
                        }
                    });
                    if let Some(detail) = clause.detail.as_ref() {
                        ui.horizontal(|ui| {
                            ui.add_space(24.0);
                            ui.label(
                                RichText::new(detail)
                                    .size(theme::TYPE_EYEBROW)
                                    .color(tokens.text_muted),
                            );
                        });
                    }
                    if expanded && let Some(trace) = trace {
                        trace_rows(ui, &tokens, "changed", &trace.implemented_by);
                        trace_rows(ui, &tokens, "checked by", &trace.validated_by);
                        trace_rows(ui, &tokens, "reviewed by", &trace.reviewed_by);
                        if trace.implemented_by.is_empty()
                            && trace.validated_by.is_empty()
                            && trace.reviewed_by.is_empty()
                        {
                            ui.horizontal(|ui| {
                                ui.add_space(24.0);
                                ui.label(
                                    RichText::new("nothing has established this yet")
                                        .size(theme::TYPE_EYEBROW)
                                        .color(tokens.text_muted),
                                );
                            });
                        }
                    }
                }

                if !alignment.not_requested.is_empty() {
                    ui.add_space(6.0);
                    section(ui, &tokens, "You said not to");
                    for statement in &alignment.not_requested {
                        bullet(ui, &tokens, statement, tokens.text_secondary);
                    }
                }

                if !alignment.open_questions.is_empty() {
                    ui.add_space(6.0);
                    section(ui, &tokens, "Still unresolved");
                    for question in &alignment.open_questions {
                        bullet(ui, &tokens, question, tokens.status_warning);
                    }
                }

                if !alignment.findings.is_empty() {
                    ui.add_space(6.0);
                    section(ui, &tokens, "What review found");
                    for finding in &alignment.findings {
                        ui.horizontal_wrapped(|ui| {
                            ui.add_space(8.0);
                            ui.label(
                                RichText::new(if finding.blocking { "●" } else { "○" })
                                    .monospace()
                                    .size(theme::TYPE_META)
                                    .color(if finding.blocking {
                                        tokens.status_error
                                    } else {
                                        tokens.text_muted
                                    }),
                            );
                            ui.label(
                                RichText::new(&finding.summary)
                                    .size(theme::TYPE_META)
                                    .color(tokens.text_primary),
                            );
                            // The difference between a list of complaints and a
                            // task that is still moving.
                            if finding.being_corrected {
                                ui.label(
                                    RichText::new("being fixed")
                                        .size(theme::TYPE_EYEBROW)
                                        .color(tokens.accent_primary),
                                );
                            }
                        });
                    }
                }

                if !alignment.validations.is_empty() {
                    ui.add_space(6.0);
                    section(ui, &tokens, "Checks");
                    for (name, passed, detail) in &alignment.validations {
                        ui.horizontal(|ui| {
                            ui.add_space(8.0);
                            ui.label(
                                RichText::new(if *passed { "✓" } else { "✗" })
                                    .monospace()
                                    .size(theme::TYPE_META)
                                    .color(if *passed {
                                        tokens.status_success
                                    } else {
                                        tokens.status_warning
                                    }),
                            );
                            ui.label(
                                RichText::new(name)
                                    .size(theme::TYPE_META)
                                    .color(tokens.text_primary),
                            );
                            // "Could not run" and "failed" are different facts,
                            // and a client that renders them as one red mark
                            // has thrown the difference away.
                            if let Some(detail) = detail {
                                ui.label(
                                    RichText::new(detail)
                                        .size(theme::TYPE_EYEBROW)
                                        .color(tokens.text_muted),
                                );
                            }
                        });
                    }
                }

                let unattributed = alignment.unattributed();
                if !alignment.change_groups.is_empty() {
                    ui.add_space(6.0);
                    section(ui, &tokens, "Changes, by what you asked for");
                    for group in &alignment.change_groups {
                        let colour = if group.is_unattributed() {
                            tokens.status_warning
                        } else {
                            tokens.text_secondary
                        };
                        ui.horizontal(|ui| {
                            ui.add_space(8.0);
                            ui.label(
                                RichText::new(&group.title)
                                    .size(theme::TYPE_META)
                                    .color(colour),
                            );
                            ui.label(
                                RichText::new(format!("{} file(s)", group.files.len()))
                                    .size(theme::TYPE_EYEBROW)
                                    .color(tokens.text_muted),
                            );
                        });
                        for (path, added, removed) in &group.files {
                            ui.horizontal(|ui| {
                                ui.add_space(24.0);
                                ui.label(
                                    RichText::new(path)
                                        .monospace()
                                        .size(theme::TYPE_EYEBROW)
                                        .color(tokens.text_secondary),
                                );
                                ui.label(
                                    RichText::new(format!("+{added} −{removed}"))
                                        .size(theme::TYPE_EYEBROW)
                                        .color(tokens.text_muted),
                                );
                            });
                        }
                    }
                    if !unattributed.is_empty() {
                        ui.horizontal_wrapped(|ui| {
                            ui.add_space(8.0);
                            ui.label(
                                RichText::new(
                                    "Some changes serve nothing you asked for — worth a look.",
                                )
                                .size(theme::TYPE_EYEBROW)
                                .color(tokens.status_warning),
                            );
                        });
                    }
                }

                ui.add_space(6.0);
                section(ui, &tokens, "Delivery");
                ui.horizontal_wrapped(|ui| {
                    ui.add_space(8.0);
                    ui.label(
                        RichText::new(verdict_label(&alignment.verdict))
                            .size(theme::TYPE_META)
                            .color(match alignment.verdict.as_str() {
                                "ready" => tokens.status_success,
                                "blocked" => tokens.status_error,
                                "needs decision" => tokens.status_warning,
                                _ => tokens.text_secondary,
                            }),
                    );
                });
                if let Some(still) = alignment.still_working_on.as_ref() {
                    ui.horizontal_wrapped(|ui| {
                        ui.add_space(8.0);
                        ui.label(
                            RichText::new(still)
                                .size(theme::TYPE_EYEBROW)
                                .color(tokens.text_muted),
                        );
                    });
                }
            });

        if response.response.interact(egui::Sense::click()).clicked() {
            open = !open;
            ui.data_mut(|data| data.insert_temp(state_id, open));
        }
        if toggled.is_some() {
            ui.data_mut(|data| data.insert_temp(state_id, true));
        }
        toggled
    }

    /// Expand or collapse the trace behind one requirement.
    pub(crate) fn toggle_requirement_trace(&mut self, id: String) {
        if !self.alignment_expanded.remove(&id) {
            self.alignment_expanded.insert(id);
        }
    }
}

/// The runtime's phase in the user's words. `Correcting` is honest about the
/// machine and alarming about the work.
fn phase_label(phase: &str) -> &'static str {
    match phase {
        "understanding" => "Understanding",
        "working" => "Working",
        "checking" => "Checking",
        "reviewing" => "Reviewing",
        "improving" => "Improving",
        "ready" => "Ready",
        "needs_you" => "Needs you",
        _ => "Working",
    }
}

fn verdict_label(verdict: &str) -> String {
    match verdict {
        "ready" => "PurrCode believes this is done, and the gate agrees".to_owned(),
        "partially complete" => "Not finished — work remains".to_owned(),
        "needs decision" => "This needs a decision from you".to_owned(),
        "blocked" => "Something is wrong or contradicts what you asked for".to_owned(),
        other => other.to_owned(),
    }
}

fn first_line(text: &str) -> String {
    text.lines().next().unwrap_or_default().to_owned()
}

fn section(ui: &mut Ui, tokens: &theme::Tokens, title: &str) {
    ui.label(
        RichText::new(title.to_uppercase())
            .size(theme::TYPE_EYEBROW)
            .color(tokens.text_muted),
    );
}

fn bullet(ui: &mut Ui, tokens: &theme::Tokens, text: &str, colour: egui::Color32) {
    ui.horizontal_wrapped(|ui| {
        ui.add_space(8.0);
        ui.label(
            RichText::new("·")
                .monospace()
                .size(theme::TYPE_META)
                .color(tokens.text_muted),
        );
        ui.label(RichText::new(text).size(theme::TYPE_META).color(colour));
    });
}

fn trace_rows(ui: &mut Ui, tokens: &theme::Tokens, label: &str, items: &[String]) {
    for item in items {
        ui.horizontal(|ui| {
            ui.add_space(24.0);
            ui.label(
                RichText::new(label)
                    .size(theme::TYPE_EYEBROW)
                    .color(tokens.text_muted),
            );
            ui.label(
                RichText::new(item)
                    .monospace()
                    .size(theme::TYPE_EYEBROW)
                    .color(tokens.text_secondary),
            );
        });
    }
}

#[cfg(test)]
mod tests {
    use crate::model::Alignment;
    use serde_json::json;

    fn panel(status: &str, verified: bool, validated: Vec<&str>) -> Alignment {
        Alignment::parse(&json!({
            "state": "ready",
            "data": {
                "progress": {
                    "phase": "reviewing",
                    "headline": "checking the settings panel",
                    "requirements_settled": if verified { 1 } else { 0 },
                    "requirements_total": 1,
                    "detail": {"workers": 0, "tool_calls": 4, "validations": 1, "corrections": 0, "reviews": 3}
                },
                "contract": {
                    "objective": "Simplify Settings without losing capability",
                    "revision": 1,
                    "must_satisfy": [{
                        "id": "r1",
                        "statement": "Every setting that existed before is still reachable",
                        "status": status
                    }],
                    "preferences": [],
                    "not_requested": ["redesign the editor"],
                    "open_questions": []
                },
                "review": {
                    "requirements": [],
                    "findings": [],
                    "validations": [],
                    "verdict": "partially complete"
                },
                "changes": {"groups": []},
                "traces": [{
                    "requirement_id": "r1",
                    "statement": "Every setting that existed before is still reachable",
                    "status": status,
                    "implemented_by": ["settings.rs"],
                    "validated_by": validated.iter().map(|item| item.to_string()).collect::<Vec<_>>(),
                    "reviewed_by": []
                }]
            }
        }))
    }

    #[test]
    fn a_verified_row_with_nothing_behind_it_is_marked_unsupported() {
        // The false Done in presentation form: a checkmark next to an empty
        // evidence list. The user must not have to expand the row to find out.
        let unsupported = panel("verified", true, vec![]);
        assert!(!unsupported.traces["r1"].supported);

        let supported = panel("verified", true, vec!["mcp_config_roundtrip"]);
        assert!(supported.traces["r1"].supported);
    }

    #[test]
    fn a_pending_row_claims_nothing_so_nothing_supports_it() {
        let pending = panel("pending", false, vec![]);
        assert!(pending.traces["r1"].supported);
        assert_eq!(pending.must_satisfy[0].marker, "·");
    }

    #[test]
    fn the_header_is_two_integers_and_never_a_percentage() {
        let view = panel("pending", false, vec![]);
        assert_eq!(view.tally(), "0 / 1 requirements verified");
        assert!(!view.tally().contains('%'));
    }

    #[test]
    fn undetermined_is_a_question_and_not_another_red_cross() {
        // A user can act on "we could not tell". They cannot act on a red cross
        // that means something different from the other red crosses.
        let view = panel("undetermined", false, vec![]);
        assert_eq!(view.must_satisfy[0].marker, "?");
        assert_eq!(view.must_satisfy[0].status, "undetermined");
    }

    #[test]
    fn a_session_with_no_contract_says_so_rather_than_showing_an_empty_panel() {
        // "No contract" means nothing is holding this work to what was asked
        // for. Rendering it as an empty panel would claim the opposite.
        let absent = Alignment::parse(&json!({
            "state": "empty",
            "message": "This session is running without a task contract"
        }));
        assert!(!absent.present);
        assert!(!absent.worth_showing());
        assert!(
            absent
                .absent_because
                .as_deref()
                .is_some_and(|reason| reason.contains("without a task contract"))
        );
    }
}
