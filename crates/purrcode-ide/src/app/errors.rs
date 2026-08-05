//! Turning failures into something a person can act on.
//!
//! PRD §18 is a contract, not a style guide. Every notice this module produces
//! answers five questions:
//!
//! 1. What happened?
//! 2. Was my work affected?
//! 3. Can PurrCode recover automatically?
//! 4. What should I do next?
//! 5. Where can I inspect technical details?
//!
//! The raw message is never thrown away — it is carried in `detail` and shown
//! under "Technical details" in Diagnostics. It is simply never the headline,
//! because `error sending request for url (http://127.0.0.1:7377/v1/sessions)`
//! tells a user nothing they can use and everything they should not have to
//! know (PRD §38, §59).

use egui::{RichText, Ui};

use super::PurrCodeIde;
use crate::daemon::PanelKind;
use crate::theme;

/// Why something failed, in the vocabulary of PRD §18.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NoticeKind {
    /// The user can directly correct the condition.
    UserRecoverable,
    /// PurrCode can retry or continue safely.
    AgentRecoverable,
    /// A local runtime, dependency, filesystem or OS condition.
    Environment,
    /// A required configuration — an API key, a provider — is missing.
    Configuration,
    /// An unexpected PurrCode failure.
    Internal,
}

impl NoticeKind {
    /// The colour role. Configuration and environment problems are warnings:
    /// nothing is broken, something is missing.
    pub(crate) const fn is_error(self) -> bool {
        matches!(self, Self::Internal | Self::AgentRecoverable)
    }
}

/// A button a notice offers.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NoticeAction {
    Retry,
    Reconnect,
    OpenSettings,
    ViewDiagnostics,
    Dismiss,
}

impl NoticeAction {
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Retry => "Retry",
            Self::Reconnect => "Reconnect",
            Self::OpenSettings => "Open settings",
            Self::ViewDiagnostics => "View details",
            Self::Dismiss => "Dismiss",
        }
    }
}

/// One thing that went wrong, said in sentences.
#[derive(Clone, Debug)]
pub struct Notice {
    pub kind: NoticeKind,
    /// What happened. A sentence, in the user's terms.
    pub headline: String,
    /// Whether their work was affected. Always stated — silence here is the
    /// thing that makes a failure frightening.
    pub impact: String,
    /// What to do next.
    pub next_step: Option<String>,
    pub actions: Vec<NoticeAction>,
    /// The raw message. Never the headline.
    pub detail: Option<String>,
    /// Clear this automatically once the local service answers again.
    pub clears_on_reconnect: bool,
}

impl Notice {
    fn new(kind: NoticeKind, headline: impl Into<String>, impact: impl Into<String>) -> Self {
        Self {
            kind,
            headline: headline.into(),
            impact: impact.into(),
            next_step: None,
            actions: vec![NoticeAction::Dismiss],
            detail: None,
            clears_on_reconnect: false,
        }
    }

    fn next(mut self, step: impl Into<String>) -> Self {
        self.next_step = Some(step.into());
        self
    }

    fn acting(mut self, actions: &[NoticeAction]) -> Self {
        self.actions = actions.to_vec();
        self
    }

    fn detailed(mut self, detail: impl Into<String>) -> Self {
        let detail = detail.into();
        self.detail = (!detail.trim().is_empty()).then_some(detail);
        self
    }

    fn transient(mut self) -> Self {
        self.clears_on_reconnect = true;
        self
    }
}

/// The local service stopped answering.
pub(crate) fn disconnected_notice() -> Notice {
    Notice::new(
        NoticeKind::AgentRecoverable,
        "PurrCode can't reach its local service",
        "Your project files were not affected, and nothing you typed was sent.",
    )
    .next("PurrCode keeps trying automatically. Use Reconnect to try again now.")
    .acting(&[NoticeAction::Reconnect, NoticeAction::ViewDiagnostics])
    .transient()
}

/// Classify a raw transport or API failure.
///
/// Returns `None` for failures that are already represented elsewhere on
/// screen, so a single unreachable daemon does not stack five banners.
pub(crate) fn translate(raw: &str) -> Option<Notice> {
    let lower = raw.to_ascii_lowercase();

    // Transport. The single most common raw string users saw, and the one
    // PRD §18 names explicitly.
    if lower.contains("error sending request")
        || lower.contains("connection refused")
        || lower.contains("tcp connect")
        || lower.contains("os error 61")
        || lower.contains("timed out")
        || lower.contains("dns error")
    {
        return Some(disconnected_notice().detailed(raw));
    }

    if lower.contains("401") || lower.contains("unauthorized") || lower.contains("forbidden") {
        return Some(
            Notice::new(
                NoticeKind::Configuration,
                "PurrCode isn't authorised to talk to its local service",
                "Your project files were not affected.",
            )
            .next("Close and reopen PurrCode. If it happens again, the local service is running with a different security token.")
            .acting(&[NoticeAction::Reconnect, NoticeAction::ViewDiagnostics])
            .detailed(raw),
        );
    }

    if lower.contains("no provider")
        || lower.contains("api key")
        || lower.contains("credential")
        || lower.contains("no model")
        || lower.contains("not qualified")
        || lower.contains("model role")
    {
        return Some(
            Notice::new(
                NoticeKind::Configuration,
                "PurrCode needs a model before it can work on code",
                "Nothing was changed in your project.",
            )
            .next("Add a provider and choose a model in Settings.")
            .acting(&[NoticeAction::OpenSettings, NoticeAction::ViewDiagnostics])
            .detailed(raw),
        );
    }

    if lower.contains("not installed")
        || lower.contains("command not found")
        || lower.contains("no such file or directory")
        || lower.contains("permission denied")
    {
        return Some(
            Notice::new(
                NoticeKind::Environment,
                "Something PurrCode needed isn't available on this machine",
                "Your code changes are safe.",
            )
            .next(
                "The details name the missing tool. Installing it and retrying is usually enough.",
            )
            .acting(&[NoticeAction::Retry, NoticeAction::ViewDiagnostics])
            .detailed(raw),
        );
    }

    if lower.contains("queue is full") || lower.contains("overflow") {
        return Some(
            Notice::new(
                NoticeKind::AgentRecoverable,
                "PurrCode is catching up on a burst of updates",
                "Nothing was lost — the screen is a moment behind.",
            )
            .next("Retry in a second.")
            .acting(&[NoticeAction::Retry, NoticeAction::Dismiss])
            .detailed(raw)
            .transient(),
        );
    }

    if lower.contains("400") || lower.contains("refused") || lower.contains("rejected") {
        return Some(
            Notice::new(
                NoticeKind::UserRecoverable,
                "PurrCode couldn't accept that request",
                "Nothing was changed in your project.",
            )
            .next("The details explain what it objected to.")
            .acting(&[NoticeAction::ViewDiagnostics, NoticeAction::Dismiss])
            .detailed(raw),
        );
    }

    // A refused follow-up is never silent (PRD §2.3 FR-B3): the composer must
    // stay populated with the draft, and this notice names the daemon's reason
    // *and* the action that clears it. The composer restore happens in the
    // `SubmissionFailed` handler; this is the mandatory, human-readable reason.
    if lower.contains("cannot accept a follow-up") {
        return Some(
            Notice::new(
                NoticeKind::UserRecoverable,
                "This session can't take a message right now",
                "Your draft is still in the composer; nothing was sent.",
            )
            .next("Start a new session, then send the message there.")
            .acting(&[NoticeAction::Retry, NoticeAction::Dismiss])
            .detailed(raw),
        );
    }
    if lower.contains("still settling") {
        return Some(
            Notice::new(
                NoticeKind::AgentRecoverable,
                "The previous turn is still settling",
                "Your draft is still in the composer; nothing was sent.",
            )
            .next("Wait a moment for the cancel or pause to finish, then send it again.")
            .acting(&[NoticeAction::Retry, NoticeAction::Dismiss])
            .detailed(raw),
        );
    }
    if lower.contains("active daemon lease") {
        return Some(
            Notice::new(
                NoticeKind::AgentRecoverable,
                "The session is already working on a turn",
                "Your draft is still in the composer; nothing was sent.",
            )
            .next("Wait for the current turn to finish, then send it again.")
            .acting(&[NoticeAction::Retry, NoticeAction::Dismiss])
            .detailed(raw),
        );
    }

    Some(
        Notice::new(
            NoticeKind::Internal,
            "Something went wrong inside PurrCode",
            "Your project files were not affected.",
        )
        .next("Retrying is safe. The details below are what to report if it keeps happening.")
        .acting(&[NoticeAction::Retry, NoticeAction::ViewDiagnostics])
        .detailed(raw),
    )
}

/// One panel of the session view could not be fetched.
pub(crate) fn panel_notice(kind: PanelKind, raw: &str) -> Notice {
    Notice::new(
        NoticeKind::AgentRecoverable,
        format!("PurrCode couldn't load {}", kind.label().to_lowercase()),
        "The rest of this session is still accurate; only that panel is missing.",
    )
    .next("Retry loads just that panel.")
    .acting(&[NoticeAction::Retry, NoticeAction::ViewDiagnostics])
    .detailed(raw)
}

impl PurrCodeIde {
    /// Try the local service again, from the beginning of the startup model.
    pub(crate) fn retry_connection(&mut self) {
        self.startup = super::Startup::Connecting;
        self.startup_began = std::time::Instant::now();
        self.notices.retain(|notice| !notice.clears_on_reconnect);
        self.client.send(crate::daemon::Request::Bootstrap);
        self.refresh_workspace();
        if self.selected.is_some() {
            self.reload_session();
        }
    }

    /// Draw the notice stack. Above the conversation, because a failure the
    /// user cannot see is a failure they cannot recover from — and below the
    /// task, because PRD §15 will not let infrastructure outrank the work.
    pub(crate) fn notice_stack(&mut self, ui: &mut Ui) {
        if self.notices.is_empty() {
            return;
        }
        let mut dismissed = None;
        let mut action = None;
        for (index, notice) in self.notices.iter().enumerate() {
            let accent = if notice.kind.is_error() {
                self.tokens.status_error
            } else {
                self.tokens.status_warning
            };
            egui::Frame::new()
                .fill(accent.linear_multiply(0.10))
                .stroke(egui::Stroke::new(1.0_f32, accent.linear_multiply(0.55)))
                .corner_radius(theme::RADIUS_CONTROL)
                .inner_margin(egui::Margin::symmetric(12, 10))
                .show(ui, |ui| {
                    ui.label(
                        RichText::new(&notice.headline)
                            .strong()
                            .color(self.tokens.text_primary),
                    );
                    ui.label(
                        RichText::new(&notice.impact)
                            .small()
                            .color(self.tokens.text_secondary),
                    );
                    if let Some(step) = &notice.next_step {
                        ui.add_space(2.0);
                        ui.label(RichText::new(step).small().color(self.tokens.text_muted));
                    }
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        for offered in &notice.actions {
                            if ui.small_button(offered.label()).clicked() {
                                if *offered == NoticeAction::Dismiss {
                                    dismissed = Some(index);
                                } else {
                                    action = Some((index, *offered));
                                }
                            }
                        }
                    });
                });
            ui.add_space(6.0);
        }
        if let Some((index, offered)) = action {
            match offered {
                NoticeAction::Retry | NoticeAction::Reconnect => {
                    self.retry_connection();
                    dismissed = Some(index);
                }
                NoticeAction::OpenSettings => self.settings_open = true,
                NoticeAction::ViewDiagnostics => self.diagnostics_open = true,
                NoticeAction::Dismiss => dismissed = Some(index),
            }
        }
        if let Some(index) = dismissed
            && index < self.notices.len()
        {
            self.notices.remove(index);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_loopback_transport_error_never_becomes_a_headline() {
        let notice = translate("error sending request for url (http://127.0.0.1:7377/v1/sessions)")
            .expect("a transport failure must produce a notice");
        assert!(!notice.headline.contains("127.0.0.1"));
        assert!(!notice.headline.contains("http"));
        assert!(notice.headline.contains("local service"));
        // The raw text survives, for Diagnostics.
        assert!(notice.detail.is_some_and(|detail| detail.contains("7377")));
    }

    #[test]
    fn every_notice_states_whether_work_was_affected() {
        for raw in [
            "error sending request for url",
            "401 Unauthorized",
            "no provider configured",
            "clippy: command not found",
            "queue is full",
            "400 Bad Request",
            "something unexpected",
        ] {
            let notice = translate(raw).expect("every failure is classified");
            assert!(
                !notice.impact.trim().is_empty(),
                "{raw} produced a notice with no impact statement"
            );
            assert!(
                notice.next_step.is_some(),
                "{raw} produced a notice with no next step"
            );
        }
    }

    #[test]
    fn a_missing_model_is_configuration_not_an_internal_failure() {
        let notice = translate("no model is qualified for the coding role").unwrap();
        assert_eq!(notice.kind, NoticeKind::Configuration);
        assert!(notice.actions.contains(&NoticeAction::OpenSettings));
    }

    #[test]
    fn a_missing_tool_is_an_environment_problem() {
        let notice = translate("ruff: command not found").unwrap();
        assert_eq!(notice.kind, NoticeKind::Environment);
    }

    #[test]
    fn a_failed_panel_says_the_rest_of_the_session_is_still_true() {
        let notice = panel_notice(PanelKind::Evidence, "500 internal error");
        assert!(notice.headline.contains("evidence"));
        assert!(notice.impact.contains("still accurate"));
    }
}
