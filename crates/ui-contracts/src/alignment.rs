//! Presentation contracts for alignment-first development (v1.5 §18–§22).
//!
//! The v1.5 UI is organised around what the user asked for, not around how the
//! runtime is built. That distinction decides almost every type in this module.
//!
//! A person watching their task does not care that a supervisor dispatched two
//! workers to a judge and an integrator. They care whether the thing they asked
//! for is happening, and whether anything is wrong. So the runtime's phases
//! arrive here already translated (§18), findings arrive grouped by the
//! requirement they bear on (§21), and every requirement can answer the one
//! question that matters about an autonomous agent:
//!
//! > **Why does PurrCode believe this is done?**
//!
//! One rule is enforced by the shape of the data rather than by convention:
//! there is no percentage anywhere in this module, and no field a client could
//! render as one. "6 of 7 requirements verified" names the gap and can be
//! wrong. "94% aligned" cannot be wrong, so it cannot be informative — and it
//! is exactly the number an agent would reach for to make an unfinished task
//! look nearly done.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// What the user sees about where the task is (§18).
///
/// The internal lifecycle has `Implementing`, `Correcting` and `NeedsAttention`;
/// this is the vocabulary those become. `Correcting` in particular reads as an
/// alarm to a person watching, when what is happening is the agent doing its
/// job.
#[derive(
    Clone, Copy, Debug, Eq, Hash, JsonSchema, Ord, PartialEq, PartialOrd, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum ProgressPhase {
    Understanding,
    Working,
    Checking,
    Reviewing,
    Improving,
    Ready,
    NeedsYou,
}

impl ProgressPhase {
    pub fn label(self) -> &'static str {
        match self {
            Self::Understanding => "Understanding",
            Self::Working => "Working",
            Self::Checking => "Checking",
            Self::Reviewing => "Reviewing",
            Self::Improving => "Improving",
            Self::Ready => "Ready",
            Self::NeedsYou => "Needs you",
        }
    }

    /// Whether the user has to do something before this moves again.
    pub fn awaits_user(self) -> bool {
        self == Self::NeedsYou
    }
}

/// The numbers behind the phase, shown when the user opens it (§18).
///
/// Counts only. A user who wants to know what the agent is doing gets facts
/// they can check, not an internal state machine they would have to learn.
#[derive(Clone, Copy, Debug, Default, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct ProgressDetail {
    pub workers: u32,
    pub tool_calls: u32,
    pub validations: u32,
    pub corrections: u32,
    pub reviews: u32,
}

/// The headline the user reads while a task runs.
#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct ProgressView {
    pub phase: ProgressPhase,
    /// One line about what is happening, in product terms.
    pub headline: String,
    /// Requirements settled, and how many there are. Two integers, on purpose.
    pub requirements_settled: u32,
    pub requirements_total: u32,
    pub detail: ProgressDetail,
}

impl ProgressView {
    /// The one-line summary, e.g. `Reviewing · 3 / 6 requirements verified`.
    pub fn summary(&self) -> String {
        format!(
            "{} · {} / {} requirements verified",
            self.phase.label(),
            self.requirements_settled,
            self.requirements_total
        )
    }
}

/// Where a requirement stands, for display (§20).
#[derive(
    Clone, Copy, Debug, Eq, Hash, JsonSchema, Ord, PartialEq, PartialOrd, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum RequirementStatusView {
    Verified,
    /// Checked, and the implementation contradicts it.
    NotSatisfied,
    /// Checked, and the check could not settle it. Shown distinctly from
    /// `Pending` because a user can act on "we could not tell" and cannot act
    /// on "not yet".
    Undetermined,
    Pending,
    Waived,
}

impl RequirementStatusView {
    /// The glyph the panel shows. `!` rather than `✗` for an undetermined
    /// requirement: it is not a failure, it is a question.
    pub fn marker(self) -> &'static str {
        match self {
            Self::Verified => "✓",
            Self::NotSatisfied => "✗",
            Self::Undetermined => "?",
            Self::Pending => "·",
            Self::Waived => "—",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Verified => "verified",
            Self::NotSatisfied => "not satisfied",
            Self::Undetermined => "could not be determined",
            Self::Pending => "pending",
            Self::Waived => "waived",
        }
    }

    pub fn settled(self) -> bool {
        matches!(self, Self::Verified | Self::Waived)
    }
}

/// One line of "What PurrCode understood" (§19).
#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct ContractClauseView {
    pub id: String,
    pub statement: String,
    pub status: RequirementStatusView,
    /// Why it is in this state, when that needs saying — the reviewer's reason
    /// for a `NotSatisfied`, the human's reason for a `Waived`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// The panel the user opens to check the agent understood them (§19).
///
/// Not shown as an up-front plan to approve. The v1.5 promise is that the user
/// describes an outcome and the agent decides the workflow; interrupting them
/// with a plan puts the workflow back in their lap. It is always *available*,
/// which is a different thing.
#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct TaskContractView {
    pub objective: String,
    /// Bumped whenever a correction lands, so the panel can show that the
    /// direction changed rather than silently displaying new text.
    pub revision: u64,
    pub must_satisfy: Vec<ContractClauseView>,
    pub preferences: Vec<ContractClauseView>,
    /// Things the user ruled out. Shown because "not requested" is the half of
    /// a contract that scope drift violates, and a user cannot check that the
    /// agent stayed in bounds if the bounds are invisible.
    pub not_requested: Vec<String>,
    #[serde(default)]
    pub open_questions: Vec<String>,
}

impl TaskContractView {
    /// `(settled, total)` over hard requirements — never a percentage.
    pub fn tally(&self) -> (u32, u32) {
        let settled = self
            .must_satisfy
            .iter()
            .filter(|clause| clause.status.settled())
            .count() as u32;
        (settled, self.must_satisfy.len() as u32)
    }

    pub fn tally_label(&self) -> String {
        let (settled, total) = self.tally();
        format!("{settled} / {total} verified")
    }
}

/// A finding as the user sees it (§20).
///
/// The reviewer's reasoning is deliberately not carried here. A review panel
/// that renders a wall of model deliberation is unreadable, and the user's
/// question is what is wrong and whether it is being handled — not how the
/// reviewer arrived at it.
#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct FindingView {
    pub id: String,
    pub summary: String,
    /// Whether this is holding delivery.
    pub blocking: bool,
    /// Whether the agent is already fixing it — the difference between a list
    /// of complaints and a task that is still moving.
    pub being_corrected: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requirement_id: Option<String>,
    #[serde(default)]
    pub evidence: Vec<String>,
}

/// The review panel (§20).
#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct ReviewPanelView {
    pub requirements: Vec<ContractClauseView>,
    pub findings: Vec<FindingView>,
    /// Deterministic checks, named and individually pass/fail.
    pub validations: Vec<ValidationLineView>,
    /// What the delivery gate concluded, in the user's words.
    pub verdict: String,
    /// The sentence under the verdict explaining why the task has not ended.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub still_working_on: Option<String>,
}

impl ReviewPanelView {
    /// Whether this panel is worth showing at all (§16).
    ///
    /// The right-hand panel appears when there is something to say. A panel
    /// that is always open and usually empty trains the user to ignore it,
    /// which is worse than not having it.
    pub fn worth_showing(&self) -> bool {
        !self.requirements.is_empty() || !self.findings.is_empty() || !self.validations.is_empty()
    }

    pub fn blocking_findings(&self) -> usize {
        self.findings
            .iter()
            .filter(|finding| finding.blocking)
            .count()
    }
}

/// One deterministic check.
#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct ValidationLineView {
    pub name: String,
    pub passed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// One changed file.
#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct ChangedFileView {
    pub path: String,
    pub added: u32,
    pub removed: u32,
}

/// Changes grouped by the requirement they serve (§21).
///
/// A flat file list makes the user reconstruct why each file changed. Grouping
/// by requirement matches how they think about the task, and it makes one
/// specific failure visible that a flat list hides: a group with no requirement
/// is work nobody asked for.
#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct ChangeGroupView {
    /// `None` for changes that serve no requirement — which is the interesting
    /// case, and why this is an option rather than a string.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requirement_id: Option<String>,
    pub title: String,
    pub files: Vec<ChangedFileView>,
}

impl ChangeGroupView {
    /// Work that serves no recorded requirement (§29).
    pub fn is_unattributed(&self) -> bool {
        self.requirement_id.is_none()
    }

    pub fn added(&self) -> u32 {
        self.files.iter().map(|file| file.added).sum()
    }

    pub fn removed(&self) -> u32 {
        self.files.iter().map(|file| file.removed).sum()
    }
}

/// The changes panel.
#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct ChangesView {
    pub groups: Vec<ChangeGroupView>,
}

impl ChangesView {
    pub fn files_changed(&self) -> usize {
        let mut paths: std::collections::BTreeSet<&str> = Default::default();
        for group in &self.groups {
            for file in &group.files {
                paths.insert(file.path.as_str());
            }
        }
        paths.len()
    }

    pub fn added(&self) -> u32 {
        self.groups.iter().map(ChangeGroupView::added).sum()
    }

    pub fn removed(&self) -> u32 {
        self.groups.iter().map(ChangeGroupView::removed).sum()
    }

    /// Groups serving no requirement. Surfaced rather than merged into the
    /// rest, because unrequested work is what the user most needs to see.
    pub fn unattributed(&self) -> Vec<&ChangeGroupView> {
        self.groups
            .iter()
            .filter(|group| group.is_unattributed())
            .collect()
    }

    pub fn headline(&self) -> String {
        format!(
            "{} files changed  +{} −{}",
            self.files_changed(),
            self.added(),
            self.removed()
        )
    }
}

/// The answer to "why does PurrCode believe this requirement is done?" (§22).
#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct RequirementTraceView {
    pub requirement_id: String,
    pub statement: String,
    pub status: RequirementStatusView,
    /// Where the work landed.
    pub implemented_by: Vec<String>,
    /// What checked it.
    pub validated_by: Vec<String>,
    /// Which review signed it off.
    pub reviewed_by: Vec<String>,
}

impl RequirementTraceView {
    /// Whether the trace actually supports the status it claims.
    ///
    /// A `Verified` requirement with nothing validating and nothing reviewing
    /// it is the false `Done` in presentation form. The client shows the
    /// requirement as unsupported rather than rendering a checkmark backed by
    /// an empty list — a user reading the panel should not have to expand every
    /// row to discover the evidence is missing.
    pub fn supports_its_status(&self) -> bool {
        if self.status != RequirementStatusView::Verified {
            return true;
        }
        !self.validated_by.is_empty() || !self.reviewed_by.is_empty()
    }
}

/// Everything the alignment surface shows, read in one go.
///
/// One type and one request rather than five, because the panels are one thing
/// to the user and five independent fetches can disagree with each other: a
/// contract from before a correction beside findings from after it describes a
/// session that never existed. Read together, they are at least a consistent
/// account of one moment.
#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize, Deserialize)]
pub struct AlignmentView {
    pub progress: ProgressView,
    /// "What PurrCode understood" (§19).
    pub contract: TaskContractView,
    /// Requirements, findings, checks and the gate's verdict (§20).
    pub review: ReviewPanelView,
    /// Changes grouped by the requirement they serve (§21).
    pub changes: ChangesView,
    /// "Why does PurrCode believe this is done?", one per requirement (§22).
    #[serde(default)]
    pub traces: Vec<RequirementTraceView>,
}

impl AlignmentView {
    /// Requirements whose displayed status is not supported by the trace behind
    /// it (§22).
    ///
    /// Surfaced rather than left for the user to find by expanding every row: a
    /// checkmark with an empty evidence list is the false `Done` in
    /// presentation form, and it is invisible until somebody looks.
    pub fn unsupported(&self) -> Vec<&RequirementTraceView> {
        self.traces
            .iter()
            .filter(|trace| !trace.supports_its_status())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clause(statement: &str, status: RequirementStatusView) -> ContractClauseView {
        ContractClauseView {
            id: statement.into(),
            statement: statement.into(),
            status,
            detail: None,
        }
    }

    fn contract() -> TaskContractView {
        TaskContractView {
            objective: "Improve the Settings experience".into(),
            revision: 1,
            must_satisfy: vec![
                clause("MCP configuration works", RequirementStatusView::Verified),
                clause("model configuration works", RequirementStatusView::Verified),
                clause("settings persist", RequirementStatusView::Pending),
            ],
            preferences: vec![clause(
                "reduce visual clutter",
                RequirementStatusView::Pending,
            )],
            not_requested: vec!["redesign the editor".into()],
            open_questions: vec![],
        }
    }

    #[test]
    fn the_tally_counts_hard_requirements_and_never_becomes_a_percentage() {
        let view = contract();
        assert_eq!(view.tally(), (2, 3));
        assert_eq!(view.tally_label(), "2 / 3 verified");
        // The preference is real and displayed, and it is not in the number
        // that says whether the task is done.
        assert_eq!(view.preferences.len(), 1);
    }

    #[test]
    fn a_waived_requirement_counts_as_settled_without_pretending_it_passed() {
        let mut view = contract();
        view.must_satisfy[2].status = RequirementStatusView::Waived;
        assert_eq!(view.tally(), (3, 3));
        assert_eq!(RequirementStatusView::Waived.marker(), "—");
        assert_ne!(
            RequirementStatusView::Waived.marker(),
            RequirementStatusView::Verified.marker(),
            "a waiver must not be shown as a pass"
        );
    }

    #[test]
    fn undetermined_is_shown_as_a_question_not_a_failure() {
        // A user can act on "we could not tell". They cannot act on a red cross
        // that means something different from the other red crosses.
        assert_eq!(RequirementStatusView::Undetermined.marker(), "?");
        assert_eq!(RequirementStatusView::NotSatisfied.marker(), "✗");
        assert!(!RequirementStatusView::Undetermined.settled());
    }

    #[test]
    fn the_user_never_sees_the_runtime_vocabulary() {
        assert_eq!(ProgressPhase::Improving.label(), "Improving");
        assert_eq!(ProgressPhase::Working.label(), "Working");
        assert!(ProgressPhase::NeedsYou.awaits_user());
        assert!(!ProgressPhase::Reviewing.awaits_user());
    }

    #[test]
    fn the_progress_line_reads_as_counts() {
        let view = ProgressView {
            phase: ProgressPhase::Reviewing,
            headline: "checking the settings panel".into(),
            requirements_settled: 3,
            requirements_total: 6,
            detail: ProgressDetail {
                workers: 2,
                tool_calls: 8,
                validations: 3,
                corrections: 1,
                reviews: 2,
            },
        };
        assert_eq!(view.summary(), "Reviewing · 3 / 6 requirements verified");
    }

    #[test]
    fn the_review_panel_stays_shut_until_it_has_something_to_say() {
        // A panel that is always open and usually empty trains the user to
        // ignore it.
        let empty = ReviewPanelView {
            requirements: vec![],
            findings: vec![],
            validations: vec![],
            verdict: "working".into(),
            still_working_on: None,
        };
        assert!(!empty.worth_showing());

        let populated = ReviewPanelView {
            requirements: contract().must_satisfy,
            ..empty
        };
        assert!(populated.worth_showing());
    }

    #[test]
    fn a_finding_being_corrected_reads_differently_from_one_just_reported() {
        // The difference between a list of complaints and a task still moving.
        let panel = ReviewPanelView {
            requirements: vec![],
            findings: vec![
                FindingView {
                    id: "f1".into(),
                    summary: "advanced controls still visible by default".into(),
                    blocking: true,
                    being_corrected: true,
                    requirement_id: None,
                    evidence: vec!["settings.rs:221".into()],
                },
                FindingView {
                    id: "f2".into(),
                    summary: "consider extracting the form".into(),
                    blocking: false,
                    being_corrected: false,
                    requirement_id: None,
                    evidence: vec!["settings.rs:88".into()],
                },
            ],
            validations: vec![],
            verdict: "blocked".into(),
            still_working_on: Some("PurrCode is correcting 1 remaining issue".into()),
        };
        assert_eq!(panel.blocking_findings(), 1);
        assert!(panel.worth_showing());
        assert!(panel.still_working_on.is_some());
    }

    #[test]
    fn changes_are_grouped_so_unrequested_work_cannot_hide_in_the_list() {
        // The failure a flat file list conceals: a change serving no
        // requirement looks exactly like every other row.
        let view = ChangesView {
            groups: vec![
                ChangeGroupView {
                    requirement_id: Some("r1".into()),
                    title: "MCP Configuration".into(),
                    files: vec![
                        ChangedFileView {
                            path: "settings.rs".into(),
                            added: 82,
                            removed: 13,
                        },
                        ChangedFileView {
                            path: "mcp.rs".into(),
                            added: 44,
                            removed: 10,
                        },
                    ],
                },
                ChangeGroupView {
                    requirement_id: None,
                    title: "Other changes".into(),
                    files: vec![ChangedFileView {
                        path: "editor/layout.rs".into(),
                        added: 120,
                        removed: 60,
                    }],
                },
            ],
        };
        assert_eq!(view.files_changed(), 3);
        assert_eq!(view.headline(), "3 files changed  +246 −83");
        let unattributed = view.unattributed();
        assert_eq!(unattributed.len(), 1);
        assert_eq!(unattributed[0].title, "Other changes");
    }

    #[test]
    fn a_verified_requirement_with_no_evidence_does_not_get_a_checkmark() {
        // The false Done in presentation form. A user should not have to expand
        // the row to find out the evidence list is empty.
        let unsupported = RequirementTraceView {
            requirement_id: "r1".into(),
            statement: "MCP configuration works".into(),
            status: RequirementStatusView::Verified,
            implemented_by: vec!["settings.rs".into()],
            validated_by: vec![],
            reviewed_by: vec![],
        };
        assert!(!unsupported.supports_its_status());

        let supported = RequirementTraceView {
            validated_by: vec!["mcp_config_roundtrip".into()],
            ..unsupported.clone()
        };
        assert!(supported.supports_its_status());

        // An unverified requirement makes no claim, so there is nothing to
        // support.
        let pending = RequirementTraceView {
            status: RequirementStatusView::Pending,
            ..unsupported
        };
        assert!(pending.supports_its_status());
    }

    #[test]
    fn the_views_round_trip_through_json() {
        let encoded = serde_json::to_string(&contract()).unwrap();
        let decoded: TaskContractView = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, contract());
        assert!(
            !encoded.contains("percent"),
            "no percentage may reach a client: {encoded}"
        );
    }
}
