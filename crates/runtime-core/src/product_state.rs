//! The canonical product states every PurrCode client shows (PRD §13).
//!
//! v0.9 let each client translate raw runtime events into its own words, so the
//! same session could read `paused blocked` in one window and `Plan ready` in
//! another. This module is the one vocabulary: the daemon resolves a state and
//! the clients render it. Nothing here is a runtime enum — `SessionPaused`,
//! `ModelRequestStarted` and `WorktreeCreated` stay on the runtime side of the
//! boundary, which is exactly what PRD §13 forbids showing to a normal user.
//!
//! Every state carries everything a surface needs to draw it without inventing
//! anything: a label, a monochrome-safe glyph, a semantic colour *role* (never a
//! literal colour — the IDE maps these onto VS Code theme tokens and the TUI
//! onto its palette), the primary and secondary action, whether the composer
//! accepts input, and whether execution is running.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// A semantic colour role. Clients map these onto their own palette so a state
/// looks native in a terminal, in a VS Code light theme and in high contrast.
#[derive(
    Clone, Copy, Debug, Eq, Hash, JsonSchema, Ord, PartialEq, PartialOrd, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum StateColor {
    Neutral,
    Running,
    Success,
    Warning,
    Error,
    Info,
}

/// What the composer does with typed text in this state.
///
/// `PlanFeedback` is the reason this is not a boolean: while a plan is waiting,
/// a message revises the plan instead of starting new work (PRD §13, §11).
#[derive(
    Clone, Copy, Debug, Eq, Hash, JsonSchema, Ord, PartialEq, PartialOrd, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum InputDisposition {
    /// Accepted as a new instruction or a follow-up.
    Accepted,
    /// Accepted, and folded into the pending plan rather than run.
    PlanFeedback,
    /// Refused — the surface must say why rather than silently dropping input.
    Refused,
}

impl InputDisposition {
    pub const fn accepts_input(self) -> bool {
        !matches!(self, Self::Refused)
    }
}

/// The thirteen labels PRD §13 declares authoritative. No client may invent a
/// fourteenth, and no client may render a runtime event name instead of one of
/// these.
#[derive(
    Clone, Copy, Debug, Eq, Hash, JsonSchema, Ord, PartialEq, PartialOrd, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum ProductState {
    Ready,
    Thinking,
    PlanReady,
    Working,
    RunningCommand,
    Testing,
    Repairing,
    PermissionRequired,
    ReadyForReview,
    Completed,
    Failed,
    Cancelled,
    NeedsRecovery,
}

impl ProductState {
    pub const ALL: &'static [Self] = &[
        Self::Ready,
        Self::Thinking,
        Self::PlanReady,
        Self::Working,
        Self::RunningCommand,
        Self::Testing,
        Self::Repairing,
        Self::PermissionRequired,
        Self::ReadyForReview,
        Self::Completed,
        Self::Failed,
        Self::Cancelled,
        Self::NeedsRecovery,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Ready => "Ready",
            Self::Thinking => "Thinking",
            Self::PlanReady => "Plan ready",
            Self::Working => "Working",
            Self::RunningCommand => "Running command",
            Self::Testing => "Testing",
            Self::Repairing => "Repairing",
            Self::PermissionRequired => "Permission required",
            Self::ReadyForReview => "Ready for review",
            Self::Completed => "Completed",
            Self::Failed => "Failed",
            Self::Cancelled => "Cancelled",
            Self::NeedsRecovery => "Needs recovery",
        }
    }

    /// A glyph that survives a monochrome terminal. The label always travels
    /// with it, so nothing here carries meaning by colour or symbol alone
    /// (PRD §27).
    pub const fn glyph(self, unicode: bool) -> &'static str {
        match (self, unicode) {
            (Self::Ready, true) => "○",
            (Self::Ready, false) => "[ ]",
            (Self::Thinking, true) => "◐",
            (Self::Thinking, false) => "[~]",
            (Self::PlanReady, true) => "▣",
            (Self::PlanReady, false) => "[p]",
            (Self::Working | Self::RunningCommand | Self::Testing | Self::Repairing, true) => "●",
            (Self::Working | Self::RunningCommand | Self::Testing | Self::Repairing, false) => {
                "[>]"
            }
            (Self::PermissionRequired, _) => "[!]",
            (Self::ReadyForReview, true) => "◆",
            (Self::ReadyForReview, false) => "[r]",
            (Self::Completed, true) => "✓",
            (Self::Completed, false) => "[x]",
            (Self::Failed, true) => "✗",
            (Self::Failed, false) => "[f]",
            (Self::Cancelled, true) => "⊘",
            (Self::Cancelled, false) => "[c]",
            (Self::NeedsRecovery, true) => "⟳",
            (Self::NeedsRecovery, false) => "[!]",
        }
    }

    pub const fn color(self) -> StateColor {
        match self {
            Self::Ready => StateColor::Neutral,
            Self::Thinking | Self::Working | Self::RunningCommand | Self::Testing => {
                StateColor::Running
            }
            Self::PlanReady | Self::ReadyForReview => StateColor::Info,
            Self::Repairing | Self::PermissionRequired | Self::NeedsRecovery => StateColor::Warning,
            Self::Completed => StateColor::Success,
            Self::Failed => StateColor::Error,
            Self::Cancelled => StateColor::Neutral,
        }
    }

    /// The action a surface offers first. `None` means the state has nothing to
    /// act on and the surface should show the composer alone.
    pub const fn primary_action(self) -> Option<&'static str> {
        match self {
            Self::Ready => None,
            Self::Thinking
            | Self::Working
            | Self::RunningCommand
            | Self::Testing
            | Self::Repairing => Some("Stop"),
            Self::PlanReady => Some("Build this plan"),
            Self::PermissionRequired => Some("Approve"),
            Self::ReadyForReview => Some("Review changes"),
            Self::Completed => Some("Create pull request"),
            Self::Failed => Some("Retry"),
            Self::Cancelled => Some("Start again"),
            Self::NeedsRecovery => Some("Recover session"),
        }
    }

    pub const fn secondary_action(self) -> Option<&'static str> {
        match self {
            Self::PlanReady => Some("Revise plan"),
            Self::PermissionRequired => Some("Reject"),
            Self::ReadyForReview => Some("Commit"),
            Self::Completed => Some("Review changes"),
            Self::Failed | Self::NeedsRecovery => Some("Show evidence"),
            _ => None,
        }
    }

    pub const fn input(self) -> InputDisposition {
        match self {
            // Plan review is a conversation, not a verdict: typed text edits the
            // plan instead of queueing a second instruction behind it.
            Self::PlanReady => InputDisposition::PlanFeedback,
            // An approval decision must be made on the approval card, where the
            // exact action is shown. Free text cannot grant authority.
            Self::PermissionRequired => InputDisposition::Refused,
            Self::NeedsRecovery => InputDisposition::Refused,
            _ => InputDisposition::Accepted,
        }
    }

    /// True while the runtime is actually doing work. Surfaces use this to
    /// decide between a Send and a Stop button, and to keep a spinner honest.
    pub const fn execution_active(self) -> bool {
        matches!(
            self,
            Self::Thinking | Self::Working | Self::RunningCommand | Self::Testing | Self::Repairing
        )
    }

    /// True once the session cannot accept another conversational turn.
    /// Completed, failed and cancelled describe the latest turn; the
    /// conversation itself can still accept a follow-up.
    pub const fn terminal(self) -> bool {
        matches!(self, Self::NeedsRecovery)
    }
}

/// A resolved state, ready to render. The daemon builds this; no client
/// re-derives it from the event log (PRD §31).
#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct ProductStateView {
    pub state: ProductState,
    pub label: String,
    pub glyph: String,
    pub color: StateColor,
    #[serde(default)]
    pub primary_action: Option<String>,
    #[serde(default)]
    pub secondary_action: Option<String>,
    pub input: InputDisposition,
    pub execution_active: bool,
    /// One sentence of context, when the state alone is not enough — the
    /// command awaiting approval, the stage that failed, the reason for
    /// recovery. Never a raw event name.
    #[serde(default)]
    pub detail: Option<String>,
}

impl ProductStateView {
    pub fn new(state: ProductState) -> Self {
        Self {
            state,
            label: state.label().to_owned(),
            glyph: state.glyph(true).to_owned(),
            color: state.color(),
            primary_action: state.primary_action().map(str::to_owned),
            secondary_action: state.secondary_action().map(str::to_owned),
            input: state.input(),
            execution_active: state.execution_active(),
            detail: None,
        }
    }

    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        let detail = detail.into();
        self.detail = (!detail.trim().is_empty()).then_some(detail);
        self
    }

    /// ASCII glyphs for terminals that cannot render the Unicode set.
    pub fn ascii(mut self) -> Self {
        self.glyph = self.state.glyph(false).to_owned();
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_thirteen_prd_labels_are_exactly_the_ones_we_ship() {
        let labels: Vec<&str> = ProductState::ALL.iter().map(|s| s.label()).collect();
        assert_eq!(
            labels,
            vec![
                "Ready",
                "Thinking",
                "Plan ready",
                "Working",
                "Running command",
                "Testing",
                "Repairing",
                "Permission required",
                "Ready for review",
                "Completed",
                "Failed",
                "Cancelled",
                "Needs recovery",
            ]
        );
    }

    #[test]
    fn no_runtime_vocabulary_reaches_a_user_facing_label() {
        // PRD §13 names these explicitly as things a normal user must never see.
        let forbidden = [
            "paused blocked",
            "plan-only session",
            "SessionPaused",
            "ModelRequestStarted",
            "WorktreeCreated",
            "CheckpointCreated",
            "JudgmentRecorded",
        ];
        for state in ProductState::ALL {
            let view = ProductStateView::new(*state);
            let rendered = format!(
                "{} {} {}",
                view.label,
                view.primary_action.clone().unwrap_or_default(),
                view.secondary_action.clone().unwrap_or_default()
            );
            for term in forbidden {
                assert!(
                    !rendered.contains(term),
                    "{state:?} leaked runtime vocabulary: {rendered}"
                );
            }
        }
    }

    #[test]
    fn every_state_defines_the_six_things_prd_13_requires() {
        for state in ProductState::ALL {
            assert!(!state.label().is_empty(), "{state:?} has no label");
            assert!(!state.glyph(true).is_empty(), "{state:?} has no glyph");
            assert!(
                state.glyph(false).is_ascii(),
                "{state:?} has no monochrome-safe glyph"
            );
            // `color`, `input` and `execution_active` are total functions, so
            // reaching here means all three are defined for this state.
            let _ = state.color();
            let _ = state.input();
            let _ = state.execution_active();
        }
    }

    #[test]
    fn plan_ready_reads_typed_text_as_feedback_not_as_a_new_task() {
        let view = ProductStateView::new(ProductState::PlanReady);
        assert_eq!(view.input, InputDisposition::PlanFeedback);
        assert!(view.input.accepts_input());
        assert_eq!(view.primary_action.as_deref(), Some("Build this plan"));
        assert_eq!(view.secondary_action.as_deref(), Some("Revise plan"));
        assert!(!view.execution_active, "a waiting plan is not running");
    }

    #[test]
    fn authority_is_never_granted_by_typing() {
        // A free-text message must not be able to approve an action: the user
        // has to see the exact action on the approval card first.
        assert_eq!(
            ProductState::PermissionRequired.input(),
            InputDisposition::Refused
        );
    }

    #[test]
    fn only_the_working_states_report_execution() {
        for state in ProductState::ALL {
            let active = state.execution_active();
            let expected = matches!(
                state,
                ProductState::Thinking
                    | ProductState::Working
                    | ProductState::RunningCommand
                    | ProductState::Testing
                    | ProductState::Repairing
            );
            assert_eq!(active, expected, "{state:?}");
        }
    }

    #[test]
    fn a_terminal_state_is_not_still_running() {
        for state in ProductState::ALL.iter().filter(|s| s.terminal()) {
            assert!(!state.execution_active(), "{state:?}");
        }
    }

    #[test]
    fn views_round_trip_and_carry_the_ascii_fallback() {
        let view = ProductStateView::new(ProductState::Testing)
            .with_detail("cargo test -p purrcode-tui")
            .ascii();
        assert_eq!(view.glyph, "[>]");
        let encoded = serde_json::to_string(&view).unwrap();
        assert!(encoded.contains("\"state\":\"testing\""), "{encoded}");
        assert_eq!(
            serde_json::from_str::<ProductStateView>(&encoded).unwrap(),
            view
        );
    }

    #[test]
    fn blank_detail_is_dropped_rather_than_shown_as_an_empty_line() {
        assert!(
            ProductStateView::new(ProductState::Ready)
                .with_detail("   ")
                .detail
                .is_none()
        );
    }
}
