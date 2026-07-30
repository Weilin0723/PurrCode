//! The contextual inspector.
//!
//! Hidden until requested or required. When open it shows detail for one
//! subject at a time: the selected activity entry, the pending decision, the
//! validation outcome, recovery state, or the status drawer holding the
//! identifiers the header deliberately dropped.
//!
//! The inspector renders nothing when hidden, so a closed inspector costs no
//! layout work and no formatting work.

use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Widget, Wrap};
use ratatui::Frame;

use crate::activity::{ActivityEntry, ValidationSummary};
use crate::approval::ApprovalRequest;
use crate::design::{Emphasis, Role, Tokens};

/// What the inspector is currently describing.
#[derive(Clone, Debug)]
pub enum InspectorSubject<'data> {
    /// Session identifiers and environment: everything the compact header moved
    /// out of the way but must remain reachable.
    StatusDrawer(StatusDrawer<'data>),
    Activity {
        entry: &'data ActivityEntry,
        /// Human-readable detail lines for the backing durable event.
        detail: Vec<String>,
    },
    Decision(&'data ApprovalRequest),
    /// A local model download awaiting explicit approval. Digest-bound exactly
    /// like a write/command decision: downloading and running a new local
    /// model is a resource and security decision, and must be shown with the
    /// same rigor rather than as a bare status line.
    ModelPull {
        model: &'data str,
        action_id: &'data str,
        action_digest: &'data str,
        approved: bool,
    },
    Validation(&'data ValidationSummary),
    Recovery {
        reason: &'data str,
        options: Vec<String>,
    },
    Unavailable(&'data str),
}

#[derive(Clone, Debug, Default)]
pub struct StatusDrawer<'data> {
    pub session_id: &'data str,
    pub sandbox_backend: &'data str,
    pub daemon_version: &'data str,
    pub daemon_health: &'data str,
    pub worktree: &'data str,
    pub provider_endpoint: &'data str,
    pub privacy: &'data str,
    pub resources: &'data str,
}

#[derive(Clone, Debug)]
pub struct Inspector<'data> {
    pub subject: InspectorSubject<'data>,
    pub focused: bool,
}

impl<'data> Inspector<'data> {
    pub fn new(subject: InspectorSubject<'data>) -> Self {
        Self {
            subject,
            focused: false,
        }
    }

    pub fn focused(mut self, focused: bool) -> Self {
        self.focused = focused;
        self
    }

    pub fn title(&self) -> &'static str {
        match self.subject {
            InspectorSubject::StatusDrawer(_) => "Session status",
            InspectorSubject::Activity { .. } => "Activity detail",
            InspectorSubject::Decision(_) => "Decision detail",
            InspectorSubject::ModelPull { .. } => "Model pull",
            InspectorSubject::Validation(_) => "Validation",
            InspectorSubject::Recovery { .. } => "Recovery",
            InspectorSubject::Unavailable(_) => "Unavailable",
        }
    }

    pub fn lines(&self, tokens: &Tokens<'_>) -> Vec<Line<'static>> {
        match &self.subject {
            InspectorSubject::StatusDrawer(drawer) => vec![
                labelled(tokens, "Session", drawer.session_id),
                labelled(tokens, "Privacy", drawer.privacy),
                labelled(tokens, "Provider", drawer.provider_endpoint),
                labelled(tokens, "Sandbox", drawer.sandbox_backend),
                labelled(tokens, "Daemon", drawer.daemon_health),
                labelled(tokens, "Version", drawer.daemon_version),
                labelled(tokens, "Worktree", drawer.worktree),
                labelled(tokens, "Resources", drawer.resources),
            ],
            InspectorSubject::Activity { entry, detail } => {
                let mut lines = vec![
                    Line::from(Span::styled(
                        entry.label.clone(),
                        tokens.styled(Role::Primary, Emphasis::Strong),
                    )),
                    labelled(tokens, "State", entry.state.word()),
                ];
                if let Some(entry_detail) = &entry.detail {
                    lines.push(labelled(tokens, "Detail", entry_detail));
                }
                if detail.is_empty() {
                    lines.push(Line::from(Span::styled(
                        "No further detail was recorded for this step.".to_owned(),
                        tokens.styled(Role::Muted, Emphasis::Dim),
                    )));
                } else {
                    lines.push(Line::from(""));
                    for line in detail {
                        lines.push(Line::from(Span::styled(
                            line.clone(),
                            tokens.style(Role::Muted),
                        )));
                    }
                }
                lines
            }
            InspectorSubject::Decision(request) => {
                let separator = crate::design::Symbols::new(tokens.unicode()).field_separator();
                let mut lines = vec![
                    Line::from(Span::styled(
                        request.kind.label().to_owned(),
                        tokens.styled(Role::Primary, Emphasis::Strong),
                    )),
                    labelled(tokens, "Operation", &request.operation),
                    labelled(tokens, "Reason", &request.reason),
                    labelled(tokens, "Risk", &request.risk_summary_with(separator)),
                    labelled(tokens, "Scope", &request.scope_summary_with(separator)),
                    labelled(tokens, "Limits", &request.limits_summary_with(separator)),
                    labelled(tokens, "Action", &request.action_id),
                    labelled(tokens, "Digest", &request.digest),
                ];
                for path in &request.affected_paths {
                    lines.push(labelled(tokens, "Path", path));
                }
                if let Some(warning) = request.staleness.warning() {
                    lines.push(Line::from(Span::styled(
                        warning.to_owned(),
                        tokens.styled(Role::Danger, Emphasis::Strong),
                    )));
                }
                lines
            }
            InspectorSubject::ModelPull {
                model,
                action_id,
                action_digest,
                approved,
            } => vec![
                Line::from(Span::styled(
                    "Local model download requires your approval".to_owned(),
                    tokens.styled(Role::Warning, Emphasis::Strong),
                )),
                labelled(tokens, "Model", model),
                labelled(
                    tokens,
                    "State",
                    if *approved {
                        "approved; start can be retried"
                    } else {
                        "awaiting explicit approval"
                    },
                ),
                labelled(tokens, "Action", action_id),
                labelled(tokens, "Digest", action_digest),
                Line::from(Span::styled(
                    "P approve/start this exact action".to_owned(),
                    tokens.styled(Role::Muted, Emphasis::Dim),
                )),
            ],
            InspectorSubject::Validation(summary) => {
                super::validation_summary::ValidationSummaryView::new(summary).lines(tokens)
            }
            InspectorSubject::Recovery { reason, options } => {
                let mut lines = vec![
                    Line::from(Span::styled(
                        "This session needs a decision before it can continue".to_owned(),
                        tokens.styled(Role::Warning, Emphasis::Strong),
                    )),
                    Line::from(""),
                    labelled(tokens, "Reason", reason),
                    Line::from(""),
                ];
                for option in options {
                    lines.push(Line::from(Span::styled(
                        option.clone(),
                        tokens.style(Role::Primary),
                    )));
                }
                lines
            }
            InspectorSubject::Unavailable(what) => vec![
                Line::from(Span::styled(
                    format!("{what} is unavailable"),
                    tokens.styled(Role::Warning, Emphasis::Strong),
                )),
                Line::from(""),
                Line::from(Span::styled(
                    "This is a missing record, not an empty one. Nothing is being hidden."
                        .to_owned(),
                    tokens.styled(Role::Muted, Emphasis::Dim),
                )),
            ],
        }
    }

    pub fn render(&self, frame: &mut Frame<'_>, area: Rect, tokens: &Tokens<'_>) {
        let block = Block::default()
            .title(format!(" {} ", self.title()))
            .borders(Borders::LEFT)
            .border_set(crate::design::Symbols::new(tokens.unicode()).border_set())
            .border_style(tokens.style(if self.focused {
                Role::Accent
            } else {
                Role::Border
            }));
        let inner = crate::design::Spacing::gutter(block.inner(area));
        block.render(area, frame.buffer_mut());
        if inner.width == 0 || inner.height == 0 {
            return;
        }
        Paragraph::new(self.lines(tokens))
            .wrap(Wrap { trim: false })
            .render(inner, frame.buffer_mut());
    }
}

fn labelled(tokens: &Tokens<'_>, label: &str, value: &str) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            format!("{label}  "),
            tokens.styled(Role::Muted, Emphasis::Dim),
        ),
        Span::styled(value.to_owned(), tokens.style(Role::Primary)),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::activity::{ActivityState, ValidationState};
    use crate::test_fixtures::{test_terminal, test_theme};

    fn text(inspector: &Inspector<'_>) -> String {
        let theme = test_theme();
        let tokens = Tokens::new(&theme);
        inspector
            .lines(&tokens)
            .iter()
            .flat_map(|line| line.spans.clone())
            .map(|span| span.content.to_string())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn the_status_drawer_holds_what_the_header_dropped() {
        let drawer = StatusDrawer {
            session_id: "0e3f9c22-1111-2222-3333-444455556666",
            sandbox_backend: "seatbelt",
            daemon_version: "0.8.0",
            daemon_health: "connected",
            worktree: "/tmp/purrcode/worktree",
            provider_endpoint: "http://127.0.0.1:11434",
            privacy: "local-only",
            resources: "16 GiB · low pressure",
        };
        let rendered = text(&Inspector::new(InspectorSubject::StatusDrawer(drawer)));
        for expected in [
            "0e3f9c22-1111-2222-3333-444455556666",
            "seatbelt",
            "0.8.0",
            "/tmp/purrcode/worktree",
            "http://127.0.0.1:11434",
            "local-only",
        ] {
            assert!(rendered.contains(expected), "missing {expected}");
        }
    }

    #[test]
    fn an_activity_entry_with_no_detail_says_so_rather_than_rendering_blank() {
        let entry = ActivityEntry {
            state: ActivityState::Done,
            label: "Context prepared".into(),
            detail: None,
            event_index: Some(1),
        };
        let rendered = text(&Inspector::new(InspectorSubject::Activity {
            entry: &entry,
            detail: Vec::new(),
        }));
        assert!(rendered.contains("Context prepared"));
        assert!(rendered.contains("No further detail was recorded"));
    }

    #[test]
    fn activity_state_is_spelled_out() {
        let entry = ActivityEntry {
            state: ActivityState::Failed,
            label: "Tool failed".into(),
            detail: Some("exit 1".into()),
            event_index: Some(3),
        };
        let rendered = text(&Inspector::new(InspectorSubject::Activity {
            entry: &entry,
            detail: vec!["stderr: boom".into()],
        }));
        assert!(rendered.contains("failed"));
        assert!(rendered.contains("exit 1"));
        assert!(rendered.contains("stderr: boom"));
    }

    #[test]
    fn unavailable_evidence_is_reported_as_missing_not_as_empty() {
        let rendered = text(&Inspector::new(InspectorSubject::Unavailable(
            "The evidence bundle",
        )));
        assert!(rendered.contains("The evidence bundle is unavailable"));
        assert!(rendered.contains("missing record, not an empty one"));
    }

    #[test]
    fn validation_detail_reuses_the_validation_component() {
        let summary = ValidationSummary {
            state: ValidationState::TimedOut,
            evidence: "cargo test did not finish".into(),
            records: 1,
        };
        let rendered = text(&Inspector::new(InspectorSubject::Validation(&summary)));
        assert!(rendered.contains("timed out"));
        assert!(rendered.contains("cargo test did not finish"));
    }

    #[test]
    fn recovery_detail_lists_the_available_options() {
        let rendered = text(&Inspector::new(InspectorSubject::Recovery {
            reason: "another client owns this session's lease",
            options: vec!["R Reconnect".into(), "O Open read-only".into()],
        }));
        assert!(rendered.contains("another client owns"));
        assert!(rendered.contains("R Reconnect"));
        assert!(rendered.contains("O Open read-only"));
    }

    #[test]
    fn a_hidden_inspector_costs_nothing_because_it_is_never_constructed() {
        // The layout returns `None` for a hidden inspector, so this test pins the
        // contract the workbench relies on: no subject, no render call.
        let areas = crate::layout::WorkbenchAreas::compute(
            Rect::new(0, 0, 160, 40),
            crate::layout::adaptive::LayoutRequest {
                composer_lines: 1,
                activity_lines: 3,
                inspector: crate::layout::adaptive::InspectorReason::Hidden,
                focused_surface: false,
            },
        );
        assert!(areas.inspector.is_none());
    }

    #[test]
    fn the_inspector_renders_in_a_narrow_column_without_panicking() {
        let theme = test_theme();
        let drawer = StatusDrawer {
            session_id: "abc",
            ..StatusDrawer::default()
        };
        for width in [1, 2, 3, 12, 40] {
            let mut terminal = test_terminal(width, 10);
            terminal
                .draw(|frame| {
                    let tokens = Tokens::new(&theme);
                    Inspector::new(InspectorSubject::StatusDrawer(drawer.clone())).render(
                        frame,
                        frame.area(),
                        &tokens,
                    );
                })
                .unwrap();
        }
    }
}
