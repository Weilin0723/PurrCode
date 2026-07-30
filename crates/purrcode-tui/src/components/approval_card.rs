//! The focused approval decision surface.
//!
//! A decision that grants execution authority is never a few lines mixed into a
//! generic status panel. This is a bordered, full-attention surface that states
//! the operation, why approval is required, the risk, every affected path, the
//! read/write/network scope, the resource limits, the exact action identity and
//! its digest, and any stale-state warning.
//!
//! The rendered decision keys reflect [`ApprovalRequest::can_decide`]: a stale
//! surface offers refresh instead of approve, so the user cannot authorize an
//! action the client can no longer vouch for.

use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Widget, Wrap};
use ratatui::Frame;

use crate::approval::ApprovalRequest;
use crate::design::{Emphasis, Role, Symbols, Tokens};

#[derive(Clone, Debug)]
pub struct ApprovalCard<'request> {
    pub request: &'request ApprovalRequest,
}

impl<'request> ApprovalCard<'request> {
    pub fn new(request: &'request ApprovalRequest) -> Self {
        Self { request }
    }

    /// The decision keys offered right now.
    pub fn decision_keys(&self) -> Vec<&'static str> {
        if self.request.can_decide() {
            vec![
                "A Approve exact action",
                "R Reject",
                "D Inspect details or diff",
                "I Add instruction",
                "Esc Leave pending",
            ]
        } else {
            vec!["F5 Refresh state", "D Inspect details", "Esc Leave pending"]
        }
    }

    pub fn lines(&self, tokens: &Tokens<'_>) -> Vec<Line<'static>> {
        let symbols = Symbols::new(tokens.unicode());
        let request = self.request;
        let mut lines = Vec::new();

        if let Some(warning) = request.staleness.warning() {
            lines.push(Line::from(Span::styled(
                format!("{} {warning}", symbols.attention()),
                tokens.styled(Role::Danger, Emphasis::Strong),
            )));
            lines.push(Line::from(""));
        }

        lines.push(Line::from(vec![
            Span::styled(
                format!("{} ", request.kind.label()),
                tokens.styled(Role::Warning, Emphasis::Strong),
            ),
            Span::styled(
                "requires your approval".to_owned(),
                tokens.style(Role::Primary),
            ),
        ]));
        lines.push(Line::from(""));

        lines.extend(field(
            tokens,
            "Operation",
            &request.operation,
            Role::Primary,
        ));
        lines.extend(field(tokens, "Reason", &request.reason, Role::Primary));
        let separator = symbols.field_separator();
        lines.extend(field(
            tokens,
            "Risk",
            &request.risk_summary_with(separator),
            Role::Warning,
        ));
        lines.extend(field(
            tokens,
            "Scope",
            &request.scope_summary_with(separator),
            Role::Primary,
        ));
        lines.extend(field(
            tokens,
            "Limits",
            &request.limits_summary_with(separator),
            Role::Primary,
        ));

        lines.push(Line::from(Span::styled(
            "Affected paths".to_owned(),
            tokens.styled(Role::Muted, Emphasis::Dim),
        )));
        for path in &request.affected_paths {
            lines.push(Line::from(vec![
                Span::raw("  "),
                Span::styled(path.clone(), tokens.style(Role::Primary)),
            ]));
        }
        lines.push(Line::from(""));

        // Exact action identity. Shown in full: a truncated digest cannot be
        // compared against the daemon's record.
        lines.extend(field(tokens, "Action", &request.action_id, Role::Muted));
        lines.extend(field(tokens, "Digest", &request.digest, Role::Muted));

        lines
    }

    pub fn render(&self, frame: &mut Frame<'_>, area: Rect, tokens: &Tokens<'_>) {
        let block = Block::default()
            .title(if self.request.can_decide() {
                " Approval required "
            } else {
                " Approval blocked "
            })
            .borders(Borders::ALL)
            .border_set(Symbols::new(tokens.unicode()).border_set())
            .border_style(tokens.style(if self.request.can_decide() {
                Role::Warning
            } else {
                Role::Danger
            }));
        let inner = block.inner(area);
        block.render(area, frame.buffer_mut());
        Paragraph::new(self.lines(tokens))
            .wrap(Wrap { trim: false })
            .render(inner, frame.buffer_mut());
    }
}

fn field(tokens: &Tokens<'_>, label: &str, value: &str, role: Role) -> Vec<Line<'static>> {
    vec![
        Line::from(Span::styled(
            label.to_owned(),
            tokens.styled(Role::Muted, Emphasis::Dim),
        )),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(value.to_owned(), tokens.style(role)),
        ]),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::approval::Staleness;
    use crate::test_fixtures::{monochrome_theme, test_terminal, test_theme};
    use purrcode_runtime_core::{ActionConstraints, ProposedAction, WriteFileAction};
    use serde_json::json;
    use std::path::PathBuf;

    fn request() -> ApprovalRequest {
        let constraints = ActionConstraints {
            working_directory: PathBuf::from("/repo"),
            network: false,
            timeout_seconds: 90,
            maximum_output_bytes: 65_536,
            allowed_write_globs: vec!["src/**".into()],
            maximum_changed_files: 2,
        };
        let action = ProposedAction::WriteFile(WriteFileAction {
            path: PathBuf::from("src/runtime.rs"),
            content: "fn main() {}".into(),
            expected_digest: None,
        });
        ApprovalRequest::from_events(&[
            json!({"event":"action_proposed","data":{"action_id":"action-abc","action":action}}),
            json!({"event":"judgment_recorded","data":{
                "action_id":"action-abc",
                "decision":{"decision":"require_approval","details":{
                    "reason":"writes a file the plan did not declare",
                    "constraints":constraints,
                }}
            }}),
        ])
        .expect("fixture must produce a pending approval")
    }

    fn screen(width: u16, height: u16, request: &ApprovalRequest, mono: bool) -> String {
        let theme = if mono {
            monochrome_theme()
        } else {
            test_theme()
        };
        let mut terminal = test_terminal(width, height);
        terminal
            .draw(|frame| {
                let tokens = Tokens::new(&theme);
                ApprovalCard::new(request).render(frame, frame.area(), &tokens);
            })
            .unwrap();
        terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }

    #[test]
    fn the_card_states_every_required_field() {
        let request = request();
        let rendered = screen(120, 30, &request, false);
        for expected in [
            "Write a file",
            "requires your approval",
            "src/runtime.rs",
            "writes a file the plan did not declare",
            "modifies files",
            "src/**",
            "no network",
            "90 s timeout",
            "at most 2 changed file(s)",
            "action-abc",
        ] {
            assert!(rendered.contains(expected), "missing {expected:?}");
        }
        assert!(
            rendered.contains(&request.digest[..16]),
            "the exact digest must be visible"
        );
    }

    #[test]
    fn the_digest_is_shown_in_full_so_it_can_be_compared() {
        let request = request();
        let lines = ApprovalCard::new(&request).lines(&Tokens::new(&test_theme()));
        let text: String = lines
            .iter()
            .flat_map(|line| line.spans.iter())
            .map(|span| span.content.as_ref())
            .collect();
        assert!(
            text.contains(&request.digest),
            "a truncated digest cannot be verified against the daemon record"
        );
    }

    #[test]
    fn a_current_surface_offers_approve_and_reject() {
        let keys = ApprovalCard::new(&request()).decision_keys().join(" ");
        assert!(keys.contains("A Approve exact action"));
        assert!(keys.contains("R Reject"));
        assert!(keys.contains("D Inspect"));
        assert!(keys.contains("I Add instruction"));
        assert!(keys.contains("Esc Leave pending"));
    }

    #[test]
    fn a_stale_surface_offers_refresh_instead_of_approve() {
        let stale = request().checked_against(Some("some-other-action"));
        let keys = ApprovalCard::new(&stale).decision_keys().join(" ");
        assert!(
            !keys.contains("Approve"),
            "a stale surface must not offer approval: {keys}"
        );
        assert!(keys.contains("Refresh"));
    }

    #[test]
    fn a_digest_mismatch_is_the_first_thing_on_screen() {
        let mismatched = request().verified_digest("deadbeef");
        assert_eq!(mismatched.staleness, Staleness::DigestMismatch);
        let rendered = screen(120, 30, &mismatched, false);
        assert!(rendered.contains("Approval blocked"));
        assert!(rendered.contains("does not match its recorded digest"));
    }

    #[test]
    fn the_card_is_usable_at_sixty_columns() {
        let request = request();
        let rendered = screen(60, 24, &request, false);
        for expected in [
            "requires your approval",
            "src/runtime.rs",
            "Digest",
            "Action",
        ] {
            assert!(
                rendered.contains(expected),
                "missing {expected:?} at 60 columns"
            );
        }
    }

    #[test]
    fn the_card_is_readable_without_colour() {
        let request = request();
        let rendered = screen(80, 24, &request, true);
        assert!(rendered.contains("Write a file"));
        assert!(rendered.contains("modifies files"));
    }

    #[test]
    fn no_file_content_is_rendered_on_the_decision_surface() {
        let request = request();
        let rendered = screen(120, 40, &request, false);
        assert!(
            !rendered.contains("fn main() {}"),
            "the card describes the action; the diff belongs behind Inspect"
        );
    }
}
