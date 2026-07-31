//! Presentation contracts shared by every PurrCode client (PRD §31).
//!
//! Clients must not each invent their own reading of the durable event log. The
//! TUI derived activity labels in Rust while the Studio derived them again in
//! JavaScript, so the same run could be described two different ways depending
//! on which window you were looking at. These types fix one vocabulary, the
//! daemon produces it, and the clients render it.
//!
//! Everything here is *presentation*: a label a person reads, a status they act
//! on, and a flag saying whether more detail exists. Internal event names,
//! payloads and identifiers stay on the runtime side of the boundary.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// PurrCode public UI/runtime protocol version.
///
/// The daemon advertises this on `/v1/health` and the Studio shell refuses to
/// drive a daemon whose version does not match. Bump on any breaking change to
/// the types in this crate.
///
/// v2 is the v0.9 reset: the v0.8 dashboard screen/action/event registry is
/// gone, replaced by the presentation contracts below.
pub const STUDIO_API_VERSION: u32 = 2;

/// Compatibility check: a daemon's version must equal the client's.
pub fn is_compatible(client: u32, daemon: u32) -> bool {
    client == daemon
}

/// The surfaces a v0.9 client presents (PRD §24.3, §24.5).
///
/// There is one primary surface — the conversation — and a set of contextual
/// ones that open when they have something to say. This is deliberately not the
/// v0.8 list: Workspaces, Agent Runs, Agent Factory, Deployments and a global
/// Evidence page were navigation entries with no complete user journey behind
/// them, and PRD §29 forbids shipping those in the primary UI.
#[derive(
    Clone, Copy, Debug, Eq, Hash, JsonSchema, Ord, PartialEq, PartialOrd, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum Surface {
    /// The session conversation and its composer. Always present.
    Conversation,
    Changes,
    Terminal,
    Tests,
    Evidence,
    Settings,
}

impl Surface {
    pub const ALL: &'static [Self] = &[
        Self::Conversation,
        Self::Changes,
        Self::Terminal,
        Self::Tests,
        Self::Evidence,
        Self::Settings,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Conversation => "Conversation",
            Self::Changes => "Changes",
            Self::Terminal => "Terminal",
            Self::Tests => "Tests",
            Self::Evidence => "Evidence",
            Self::Settings => "Settings",
        }
    }

    /// True when the surface only appears once it has content (PRD §3.4
    /// progressive disclosure). Only the conversation is permanent.
    pub const fn contextual(self) -> bool {
        !matches!(self, Self::Conversation)
    }
}

/// What kind of work an activity entry describes (PRD §31.4).
#[derive(
    Clone, Copy, Debug, Eq, Hash, JsonSchema, Ord, PartialEq, PartialOrd, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum ActivityKind {
    Inspection,
    Planning,
    Edit,
    Command,
    Validation,
    Approval,
    Recovery,
    Completion,
}

/// How an activity entry is going.
///
/// `Blocked` is distinct from `Failed`: one needs a person, the other needs a
/// repair. Collapsing them is how a UI ends up showing "failed" for something
/// that is simply waiting for an approval.
#[derive(
    Clone, Copy, Debug, Eq, Hash, JsonSchema, Ord, PartialEq, PartialOrd, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum ActivityStatus {
    Pending,
    Running,
    Done,
    Blocked,
    Failed,
}

impl ActivityStatus {
    /// A glyph plus a word. Status must never depend on colour or on a symbol
    /// alone — monochrome terminals and screen readers get the word.
    pub const fn glyph(self, unicode: bool) -> &'static str {
        match (self, unicode) {
            (Self::Done, true) => "✓",
            (Self::Done, false) => "[x]",
            (Self::Running, true) => "●",
            (Self::Running, false) => "[>]",
            (Self::Pending, true) => "○",
            (Self::Pending, false) => "[ ]",
            (Self::Blocked, _) => "[!]",
            (Self::Failed, true) => "✗",
            (Self::Failed, false) => "[f]",
        }
    }

    pub const fn word(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::Done => "done",
            Self::Blocked => "needs you",
            Self::Failed => "failed",
        }
    }
}

/// One line of user-facing progress (PRD §31.4).
#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct ActivityItem {
    pub id: String,
    pub kind: ActivityKind,
    /// What a person reads, e.g. "Running 18 focused tests".
    pub label: String,
    pub status: ActivityStatus,
    #[serde(default)]
    pub summary: Option<String>,
    /// True when expanding this entry would show something more — the command,
    /// the affected files, the test output, the authorization decision.
    pub detail_available: bool,
}

/// Truthful validation outcome (PRD §21.3).
///
/// `Unavailable` and `Skipped` are their own states precisely so no client can
/// render them as success. A stage that did not run has not passed.
#[derive(
    Clone, Copy, Debug, Eq, Hash, JsonSchema, Ord, PartialEq, PartialOrd, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum ValidationOutcome {
    Passed,
    Failed,
    TimedOut,
    Cancelled,
    InfrastructureError,
    Unavailable,
    Skipped,
}

impl ValidationOutcome {
    /// True only for an outcome that actually demonstrates the stage worked.
    pub const fn is_success(self) -> bool {
        matches!(self, Self::Passed)
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Passed => "passed",
            Self::Failed => "failed",
            Self::TimedOut => "timed out",
            Self::Cancelled => "cancelled",
            Self::InfrastructureError => "infrastructure error",
            Self::Unavailable => "unavailable",
            Self::Skipped => "skipped",
        }
    }
}

/// One validation stage as a client shows it (PRD §31.5).
#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct ValidationStageView {
    pub stage: String,
    pub outcome: ValidationOutcome,
    #[serde(default)]
    pub detail: Option<String>,
}

/// The validation summary for a session (PRD §31.5).
#[derive(Clone, Debug, Default, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct ValidationSummary {
    pub stages: Vec<ValidationStageView>,
    /// True only when every required stage actually passed.
    pub complete: bool,
}

impl ValidationSummary {
    pub fn new(stages: Vec<ValidationStageView>) -> Self {
        let complete = !stages.is_empty() && stages.iter().all(|stage| stage.outcome.is_success());
        Self { stages, complete }
    }

    /// A one-line summary that never overstates the result.
    pub fn headline(&self) -> String {
        if self.stages.is_empty() {
            return "No validation has run".to_owned();
        }
        if self.complete {
            return format!("{} validation stage(s) passed", self.stages.len());
        }
        let unresolved: Vec<&ValidationStageView> = self
            .stages
            .iter()
            .filter(|stage| !stage.outcome.is_success())
            .collect();
        format!(
            "{} of {} stage(s) did not pass: {}",
            unresolved.len(),
            self.stages.len(),
            unresolved
                .iter()
                .map(|stage| format!("{} {}", stage.stage, stage.outcome.label()))
                .collect::<Vec<_>>()
                .join(", ")
        )
    }
}

/// What every client puts in its header (PRD §31.1, §14).
///
/// Deliberately free of the things §14 forbids showing by default: no
/// filesystem paths, no worktree cache, no commit SHA, no session UUID, no
/// database path, no provider endpoint, no raw event count.
#[derive(Clone, Debug, Default, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct UiStatus {
    pub repository: String,
    pub branch: String,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub provider: Option<String>,
    pub task_mode: String,
    pub permission_mode: String,
    pub phase: String,
    pub local_only: bool,
    /// Surfaces that currently have something to show.
    #[serde(default)]
    pub available_surfaces: Vec<Surface>,
}

/// A session as it appears in a history list (PRD §31.3).
#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct SessionSummary {
    pub id: String,
    pub objective: String,
    pub status: String,
    pub repository: String,
    #[serde(default)]
    pub branch: Option<String>,
    pub changed_file_count: usize,
    /// The plan, when one was produced. In Plan mode this *is* the deliverable
    /// (PRD §11), so a client that cannot show it has nothing to review.
    #[serde(default)]
    pub plan: Vec<String>,
    #[serde(default)]
    pub validation: Option<ValidationSummary>,
    /// True when this session needs a person before it can continue.
    pub needs_attention: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_conversation_is_permanent() {
        assert!(!Surface::Conversation.contextual());
        for surface in Surface::ALL.iter().filter(|s| **s != Surface::Conversation) {
            assert!(surface.contextual(), "{surface:?} must open contextually");
        }
    }

    #[test]
    fn the_surface_list_has_no_v08_dashboard_pages() {
        let labels: Vec<&str> = Surface::ALL.iter().map(|s| s.label()).collect();
        for forbidden in ["Workspaces", "Agent Runs", "Agent Factory", "Deployments"] {
            assert!(
                !labels.contains(&forbidden),
                "{forbidden} must not be a v0.9 surface"
            );
        }
    }

    #[test]
    fn only_passed_counts_as_success() {
        assert!(ValidationOutcome::Passed.is_success());
        for outcome in [
            ValidationOutcome::Failed,
            ValidationOutcome::TimedOut,
            ValidationOutcome::Cancelled,
            ValidationOutcome::InfrastructureError,
            ValidationOutcome::Unavailable,
            ValidationOutcome::Skipped,
        ] {
            assert!(
                !outcome.is_success(),
                "{outcome:?} must never read as success"
            );
        }
    }

    #[test]
    fn an_unavailable_stage_is_not_reported_as_complete() {
        let summary = ValidationSummary::new(vec![
            ValidationStageView {
                stage: "unit tests".into(),
                outcome: ValidationOutcome::Passed,
                detail: None,
            },
            ValidationStageView {
                stage: "integration tests".into(),
                outcome: ValidationOutcome::Unavailable,
                detail: Some("no docker daemon".into()),
            },
        ]);
        assert!(!summary.complete);
        let headline = summary.headline();
        assert!(
            headline.contains("integration tests unavailable"),
            "{headline}"
        );
        assert!(!headline.contains("passed"), "{headline}");
    }

    #[test]
    fn no_validation_is_not_the_same_as_passing_validation() {
        let empty = ValidationSummary::default();
        assert!(!empty.complete);
        assert_eq!(empty.headline(), "No validation has run");
    }

    #[test]
    fn all_stages_passing_is_complete() {
        let summary = ValidationSummary::new(vec![ValidationStageView {
            stage: "unit tests".into(),
            outcome: ValidationOutcome::Passed,
            detail: None,
        }]);
        assert!(summary.complete);
        assert_eq!(summary.headline(), "1 validation stage(s) passed");
    }

    #[test]
    fn blocked_is_distinct_from_failed() {
        assert_eq!(ActivityStatus::Blocked.word(), "needs you");
        assert_eq!(ActivityStatus::Failed.word(), "failed");
        assert_ne!(
            ActivityStatus::Blocked.glyph(true),
            ActivityStatus::Failed.glyph(true)
        );
    }

    #[test]
    fn every_status_carries_a_word_for_monochrome_and_screen_readers() {
        for status in [
            ActivityStatus::Pending,
            ActivityStatus::Running,
            ActivityStatus::Done,
            ActivityStatus::Blocked,
            ActivityStatus::Failed,
        ] {
            assert!(!status.word().is_empty());
            assert!(!status.glyph(false).is_empty());
            assert!(status.glyph(false).is_ascii());
        }
    }

    #[test]
    fn activity_items_round_trip() {
        let item = ActivityItem {
            id: "a1".into(),
            kind: ActivityKind::Validation,
            label: "Running 18 focused tests".into(),
            status: ActivityStatus::Running,
            summary: None,
            detail_available: true,
        };
        let encoded = serde_json::to_string(&item).unwrap();
        assert!(encoded.contains("\"kind\":\"validation\""));
        assert!(encoded.contains("\"status\":\"running\""));
        assert_eq!(
            serde_json::from_str::<ActivityItem>(&encoded).unwrap(),
            item
        );
    }

    #[test]
    fn ui_status_omits_everything_prd_14_forbids() {
        let status = UiStatus {
            repository: "payment-api".into(),
            branch: "main".into(),
            model: Some("qwen3-coder:30b".into()),
            provider: Some("Ollama".into()),
            task_mode: "Build".into(),
            permission_mode: "Auto".into(),
            phase: "testing".into(),
            local_only: true,
            available_surfaces: vec![Surface::Conversation, Surface::Changes],
        };
        let encoded = serde_json::to_string(&status).unwrap();
        for forbidden in ["session_id", "head", "database", "endpoint", "event_count"] {
            assert!(
                !encoded.contains(forbidden),
                "{forbidden} leaked: {encoded}"
            );
        }
    }

    #[test]
    fn version_compatibility_requires_an_exact_match() {
        assert!(is_compatible(STUDIO_API_VERSION, STUDIO_API_VERSION));
        assert!(!is_compatible(STUDIO_API_VERSION, STUDIO_API_VERSION - 1));
    }
}
