//! The agent workspace: what the subagents are doing, and how to stop one.
//!
//! v1.2 Pillar 4, with a deliberate ceiling. The supervisor can run several
//! workers at once, and the user needs to see that and be able to interrupt
//! one of them. What they do *not* need is an orchestration dashboard: a
//! panel of agent windows turns an IDE into a monitoring console for its own
//! machinery, and the conversation stops being the centre of the screen.
//!
//! So this is one collapsed line in the work log. It expands when the user
//! asks, and it disappears entirely for the overwhelmingly common case of a
//! session that never ran a worker at all.

use egui::{RichText, Ui};

use super::PurrCodeIde;
use crate::daemon::Request;
use crate::icons::Glyph;
use crate::model::Worker;
use crate::theme;

/// How often the worker tree refreshes while a run is live.
const POLL: std::time::Duration = std::time::Duration::from_millis(1200);

impl PurrCodeIde {
    /// Keeps the worker tree current for the selected session.
    ///
    /// Polled only while there is something to watch: a session with no
    /// workers is asked once when it is selected, and then left alone.
    pub(crate) fn poll_supervisor(&mut self) {
        let Some(session) = self.selected.clone() else {
            self.supervisor = crate::model::Supervisor::default();
            self.supervisor_for = None;
            return;
        };
        let switched = self.supervisor_for.as_deref() != Some(session.as_str());
        if switched {
            self.supervisor = crate::model::Supervisor::default();
            self.supervisor_for = Some(session.clone());
            self.last_supervisor_poll = std::time::Instant::now();
            self.client.send(Request::SupervisorStatus { session });
            return;
        }
        // Once the tree is known, only a live run is worth re-reading.
        if self.supervisor.running() == 0 || self.last_supervisor_poll.elapsed() < POLL {
            return;
        }
        self.last_supervisor_poll = std::time::Instant::now();
        self.client.send(Request::SupervisorStatus { session });
    }

    /// The worker tree, as one collapsed row in the work log.
    ///
    /// Draws nothing when no workers exist, which is most sessions. Returns
    /// the worker the user asked to stop, applied by the caller: the tree is
    /// drawn while the transcript holds a borrow of the message list.
    pub(crate) fn worker_tree(&self, ui: &mut Ui) -> Option<String> {
        if self.supervisor.is_empty() {
            return None;
        }
        let tokens = self.tokens;
        let supervisor = self.supervisor.clone();
        let running = supervisor.running();
        let state_id = egui::Id::new(("purrcode_worker_tree", self.session.id.clone()));
        let mut open = ui.data_mut(|data| data.get_temp::<bool>(state_id).unwrap_or(false));
        let mut stop: Option<String> = None;

        let response =
            egui::Frame::new()
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
                            RichText::new(summary_line(&supervisor))
                                .size(theme::TYPE_META)
                                .color(if running > 0 {
                                    tokens.text_primary
                                } else {
                                    tokens.text_secondary
                                }),
                        );
                    });

                    if !open {
                        return;
                    }
                    ui.add_space(4.0);
                    for worker in &supervisor.workers {
                        ui.horizontal(|ui| {
                            ui.add_space(8.0);
                            let colour = worker_colour(&tokens, worker);
                            crate::icons::step_marker(ui, marker_for(worker), 9.0, colour);
                            ui.label(
                                RichText::new(&worker.id)
                                    .size(theme::TYPE_META)
                                    .color(tokens.text_primary),
                            );
                            ui.label(
                                RichText::new(worker.outcome())
                                    .size(theme::TYPE_EYEBROW)
                                    .color(tokens.text_muted),
                            );
                            // Stop is offered only for a worker that is actually
                            // running: the daemon rejects the rest, and a button
                            // that always fails is worse than no button.
                            if worker.is_running() {
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        if super::primitives::button(
                                            ui,
                                            &tokens,
                                            super::primitives::Tone::Danger,
                                            "Stop",
                                        )
                                        .on_hover_text(
                                            "Stop this worker. The rest of the run continues.",
                                        )
                                        .clicked()
                                        {
                                            stop = Some(worker.id.clone());
                                        }
                                    },
                                );
                            }
                        });
                        // The files it touched, so a finished worker's result is
                        // inspectable rather than just a status word.
                        if !worker.changed_paths.is_empty() {
                            ui.horizontal_wrapped(|ui| {
                                ui.add_space(24.0);
                                for path in &worker.changed_paths {
                                    ui.label(
                                        RichText::new(path)
                                            .monospace()
                                            .size(theme::TYPE_EYEBROW)
                                            .color(tokens.text_muted),
                                    );
                                }
                            });
                        }
                    }

                    // Two workers that changed the same file is the one outcome a
                    // human has to settle, so it is stated rather than folded in
                    // with the per-worker results.
                    if !supervisor.conflicts.is_empty() {
                        ui.add_space(6.0);
                        ui.label(
                        RichText::new(format!(
                            "{} file{} changed by more than one worker — review before merging:",
                            supervisor.conflicts.len(),
                            if supervisor.conflicts.len() == 1 { "" } else { "s" }
                        ))
                        .size(theme::TYPE_META)
                        .color(tokens.status_warning),
                    );
                        for path in &supervisor.conflicts {
                            ui.horizontal(|ui| {
                                ui.add_space(8.0);
                                ui.label(
                                    RichText::new(path)
                                        .monospace()
                                        .size(theme::TYPE_EYEBROW)
                                        .color(tokens.status_warning),
                                );
                            });
                        }
                    }
                })
                .response;

        if response
            .interact(egui::Sense::click())
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .clicked()
        {
            open = !open;
            ui.data_mut(|data| data.insert_temp(state_id, open));
        }
        stop
    }

    /// Stops one worker without cancelling the rest of the run.
    pub(crate) fn stop_worker(&mut self, worker: String) {
        if let Some(session) = self.selected.clone() {
            self.client.send(Request::StopWorker { session, worker });
        }
    }
}

/// The collapsed line: enough to decide whether to open it.
fn summary_line(supervisor: &crate::model::Supervisor) -> String {
    let total = supervisor.workers.len();
    let running = supervisor.running();
    if running > 0 {
        return format!(
            "{running} of {total} worker{} still running",
            if total == 1 { "" } else { "s" }
        );
    }
    let changed: usize = supervisor
        .workers
        .iter()
        .map(|worker| worker.changed_paths.len())
        .sum();
    format!(
        "{total} worker{} finished · {changed} file{} changed",
        if total == 1 { "" } else { "s" },
        if changed == 1 { "" } else { "s" }
    )
}

fn marker_for(worker: &Worker) -> &'static str {
    match worker.status.as_str() {
        "running" => "running",
        "completed" | "succeeded" => "done",
        "failed" => "failed",
        _ => "blocked",
    }
}

fn worker_colour(tokens: &theme::Tokens, worker: &Worker) -> egui::Color32 {
    match worker.status.as_str() {
        "running" => tokens.status_running,
        "completed" | "succeeded" => tokens.status_success,
        "failed" => tokens.status_error,
        _ => tokens.status_warning,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Supervisor;
    use serde_json::json;

    #[test]
    fn a_session_that_never_ran_workers_has_an_empty_tree() {
        // The surface must disappear rather than announce "0 workers": most
        // sessions never run one, and the conversation owns the centre.
        let supervisor = Supervisor::parse(&json!({"workers": [], "conflicts": []}));
        assert!(supervisor.is_empty());
        assert_eq!(supervisor.running(), 0);
    }

    #[test]
    fn the_collapsed_line_leads_with_running_work() {
        let supervisor = Supervisor::parse(&json!({
            "workers": [
                {"id": "scout", "status": "completed", "changed_paths": ["a.rs"]},
                {"id": "impl", "status": "running", "changed_paths": []},
            ],
        }));
        assert_eq!(supervisor.running(), 1);
        assert_eq!(summary_line(&supervisor), "1 of 2 workers still running");
    }

    #[test]
    fn a_finished_run_is_summarised_by_what_it_changed() {
        let supervisor = Supervisor::parse(&json!({
            "workers": [
                {"id": "a", "status": "completed", "changed_paths": ["a.rs", "b.rs"]},
                {"id": "b", "status": "failed", "changed_paths": []},
            ],
        }));
        assert_eq!(supervisor.running(), 0);
        assert_eq!(
            summary_line(&supervisor),
            "2 workers finished · 2 files changed"
        );
    }

    #[test]
    fn a_running_worker_reports_work_rather_than_a_file_count() {
        let supervisor = Supervisor::parse(&json!({
            "workers": [{"id": "scout", "status": "running", "changed_paths": []}],
        }));
        assert_eq!(supervisor.workers[0].outcome(), "working…");
        // A finished worker that changed nothing says so, rather than
        // borrowing the running phrasing.
        let done = Supervisor::parse(&json!({
            "workers": [{"id": "scout", "status": "completed", "changed_paths": []}],
        }));
        assert_eq!(done.workers[0].outcome(), "completed");
    }

    #[test]
    fn a_worker_with_no_id_is_dropped() {
        // Stop addresses a worker by id; an entry without one would render a
        // button that cannot work.
        let supervisor = Supervisor::parse(&json!({
            "workers": [{"status": "running"}, {"id": "ok", "status": "running"}],
        }));
        assert_eq!(supervisor.workers.len(), 1);
        assert_eq!(supervisor.workers[0].id, "ok");
    }

    #[test]
    fn conflicts_survive_parsing() {
        let supervisor = Supervisor::parse(&json!({
            "workers": [{"id": "a", "status": "completed"}],
            "conflicts": ["src/lib.rs"],
            "review_required": true,
        }));
        assert_eq!(supervisor.conflicts, vec!["src/lib.rs".to_owned()]);
        assert!(supervisor.review_required);
    }
}
