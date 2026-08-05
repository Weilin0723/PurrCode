//! The compact top bar.
//!
//! Shows only what orients the user: product, repository/branch, model, mode,
//! current phase, and the privacy state. Session identifiers, sandbox backend,
//! daemon version, worktree path, provider endpoint and resource details move to
//! the inspector's status drawer — they are still reachable, just not competing
//! with the task for attention.
//!
//! Privacy is the one piece of security state that must never be dropped when
//! space runs out, so it is placed first among the droppable fields and is the
//! last to go.

use crate::design::{Emphasis, Role, Symbols, Tokens};
use purrcode_runtime_core::adaptation::{SearchPolicy, WorkflowControl, WorkflowProfile};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};
use unicode_width::UnicodeWidthStr;

/// One header field, ordered from most to least important.
struct Field {
    text: String,
    role: Role,
    /// Lower drops later. Product name and privacy are never dropped.
    priority: u8,
}

#[derive(Clone, Debug)]
pub struct Header<'data> {
    pub repository: &'data str,
    pub branch: &'data str,
    /// Fully qualified model id, e.g. `ollama/qwen2.5-coder:7b`. The header
    /// renders only the model part; `/status` shows the whole id.
    pub model: &'data str,
    pub workflow_control: WorkflowControl,
    pub workflow_profile: WorkflowProfile,
    pub search_policy: SearchPolicy,
    pub mode: &'data str,
    /// Permission mode: Ask, Auto, or Full Access (PRD §12, §14).
    pub permission: &'data str,
    pub phase: &'data str,
    /// True when inference stays on this machine.
    pub local_only: bool,
}

impl<'data> Header<'data> {
    pub fn fields(&self, tokens: &Tokens<'_>) -> Vec<Line<'static>> {
        vec![Line::from(self.spans(tokens, u16::MAX))]
    }

    fn ordered_fields(&self, symbols: Symbols) -> Vec<Field> {
        vec![
            Field {
                text: "PurrCode".to_owned(),
                role: Role::Accent,
                priority: 0,
            },
            Field {
                // Privacy is security state: it outranks every other field but
                // the product name.
                text: if self.local_only {
                    format!("{} local-only", symbols.local())
                } else {
                    format!("{} network", symbols.remote())
                },
                role: if self.local_only {
                    Role::Success
                } else {
                    Role::Warning
                },
                priority: 1,
            },
            Field {
                text: format!("{}/{}", self.repository, self.branch),
                role: Role::Primary,
                priority: 2,
            },
            Field {
                // PRD §10.1: the model is the fact, the provider is secondary.
                // A configured id like `nvidia-nim-x/deepseek-ai/deepseek-v4-pro`
                // is 45 characters of header for one piece of information, and
                // it pushes the mode and phase off a narrow terminal.
                text: model_name(self.model).to_owned(),
                role: Role::Primary,
                priority: 3,
            },
            Field {
                text: match self.workflow_control {
                    WorkflowControl::Auto => {
                        format!("Auto{}{}", symbols.workflow_arrow(), self.workflow_profile)
                    }
                    control => control.label().to_owned(),
                },
                role: Role::Primary,
                priority: 4,
            },
            Field {
                text: self.search_policy.to_string(),
                role: Role::Primary,
                priority: 5,
            },
            Field {
                text: self.mode.to_owned(),
                role: Role::Muted,
                priority: 6,
            },
            Field {
                text: self.permission.to_owned(),
                role: Role::Muted,
                priority: 7,
            },
            Field {
                text: self.phase.to_owned(),
                role: Role::Muted,
                priority: 8,
            },
        ]
    }

    /// Spans that fit within `width`, dropping the least important fields first.
    pub fn spans(&self, tokens: &Tokens<'_>, width: u16) -> Vec<Span<'static>> {
        let symbols = Symbols::new(tokens.unicode());
        let separator = symbols.field_separator();
        let mut fields: Vec<Field> = self
            .ordered_fields(symbols)
            .into_iter()
            .filter(|field| !field.text.trim().is_empty())
            .collect();

        let width = width as usize;
        while fields.len() > 1 && rendered_width(&fields, separator) > width {
            let droppable = fields
                .iter()
                .enumerate()
                .filter(|(_, field)| field.priority > 1)
                .max_by_key(|(_, field)| field.priority)
                .map(|(index, _)| index);
            match droppable {
                Some(index) => {
                    fields.remove(index);
                }
                // Only the product name and privacy remain. Shrink the privacy
                // label to its compact form rather than dropping it; privacy is
                // the one field that must never disappear.
                None => {
                    let compact = if self.local_only {
                        format!("{} local", symbols.local())
                    } else {
                        format!("{} net", symbols.remote())
                    };
                    if let Some(field) = fields.iter_mut().find(|field| field.priority == 1) {
                        if field.text == compact {
                            break;
                        }
                        field.text = compact;
                    } else {
                        break;
                    }
                }
            }
        }

        // Last resort: clip the product name so the privacy state still fits.
        if rendered_width(&fields, separator) > width && fields.len() == 2 {
            let privacy =
                UnicodeWidthStr::width(fields[1].text.as_str()) + UnicodeWidthStr::width(separator);
            let room = width.saturating_sub(privacy);
            fields[0].text = fields[0].text.chars().take(room).collect();
        }

        let mut spans = Vec::new();
        for (index, field) in fields.iter().enumerate() {
            if index > 0 {
                spans.push(Span::styled(
                    separator.to_owned(),
                    tokens.styled(Role::Border, Emphasis::Dim),
                ));
            }
            spans.push(Span::styled(
                field.text.clone(),
                tokens.styled(
                    field.role,
                    if index == 0 {
                        Emphasis::Strong
                    } else {
                        Emphasis::Normal
                    },
                ),
            ));
        }
        spans
    }

    pub fn render(&self, frame: &mut Frame<'_>, area: Rect, tokens: &Tokens<'_>) {
        let spans = self.spans(tokens, area.width);
        Paragraph::new(Line::from(spans)).render(area, frame.buffer_mut());
    }
}

fn rendered_width(fields: &[Field], separator: &str) -> usize {
    let separators = fields.len().saturating_sub(1) * UnicodeWidthStr::width(separator);
    fields
        .iter()
        .map(|field| UnicodeWidthStr::width(field.text.as_str()))
        .sum::<usize>()
        + separators
}

/// The model portion of a provider-qualified id.
///
/// Provider names can themselves contain slashes in a model path
/// (`nvidia-nim/deepseek-ai/deepseek-v4-pro`), so only the leading provider
/// segment is dropped — taking the last segment would turn
/// `deepseek-ai/deepseek-v4-pro` into something the user never typed.
pub fn model_name(model: &str) -> &str {
    model.split_once('/').map_or(model, |(_, rest)| rest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_fixtures::{monochrome_theme, test_theme};

    fn header() -> Header<'static> {
        Header {
            repository: "PurrCode",
            branch: "main",
            model: "ollama/coder:7b",
            workflow_control: WorkflowControl::Auto,
            workflow_profile: WorkflowProfile::Standard,
            search_policy: SearchPolicy::Auto,
            mode: "Build",
            permission: "Auto",
            phase: "executing",
            local_only: true,
        }
    }

    fn rendered(width: u16, local_only: bool) -> String {
        let theme = test_theme();
        let tokens = Tokens::new(&theme);
        let mut header = header();
        header.local_only = local_only;
        header
            .spans(&tokens, width)
            .iter()
            .map(|span| span.content.as_ref())
            .collect()
    }

    #[test]
    fn wide_header_shows_the_full_orientation_line() {
        let line = rendered(120, true);
        for expected in [
            "PurrCode",
            "PurrCode/main",
            "coder:7b",
            "Standard",
            "Auto",
            "Build",
            "executing",
        ] {
            assert!(line.contains(expected), "missing {expected} in {line}");
        }
        // The provider is secondary information (PRD §10.1) and belongs to
        // `/status`, not to the line the user reads on every frame.
        assert!(
            !line.contains("ollama/"),
            "the provider prefix should not be in the header: {line}"
        );
    }

    #[test]
    fn header_omits_internal_metadata() {
        let line = rendered(160, true);
        for forbidden in ["seatbelt", "127.0.0.1", "worktree", "daemon"] {
            assert!(
                !line.contains(forbidden),
                "{forbidden} belongs in the inspector, not the header: {line}"
            );
        }
    }

    #[test]
    fn header_never_overflows_its_width() {
        for width in [20, 40, 60, 80, 120, 160] {
            let line = rendered(width, true);
            assert!(
                UnicodeWidthStr::width(line.as_str()) <= width as usize,
                "header at {width} columns rendered {} columns: {line}",
                UnicodeWidthStr::width(line.as_str())
            );
        }
    }

    /// Privacy may shrink from `local-only`/`network` to `local`/`net`, but it
    /// must never disappear and the two states must never be confusable.
    #[test]
    fn privacy_state_survives_every_width() {
        for width in [20, 30, 40, 60, 80, 120] {
            let local = rendered(width, true);
            assert!(
                local.contains("local"),
                "local privacy state disappeared at {width} columns: {local}"
            );
            let remote = rendered(width, false);
            assert!(
                remote.contains("net"),
                "network privacy state disappeared at {width} columns: {remote}"
            );
            assert!(
                !remote.contains("local"),
                "remote inference must never read as local at {width} columns: {remote}"
            );
        }
    }

    #[test]
    fn a_very_narrow_header_shrinks_the_privacy_label_rather_than_dropping_it() {
        let line = rendered(18, true);
        assert!(line.contains("local"), "privacy disappeared: {line}");
        assert!(UnicodeWidthStr::width(line.as_str()) <= 18, "{line}");
    }

    #[test]
    fn narrow_headers_drop_the_least_important_fields_first() {
        let narrow = rendered(46, true);
        assert!(narrow.contains("PurrCode"));
        assert!(narrow.contains("local-only"));
        assert!(
            !narrow.contains("executing"),
            "phase is the first field to go: {narrow}"
        );
    }

    #[test]
    fn local_and_remote_are_visually_different_without_colour() {
        let theme = monochrome_theme();
        let tokens = Tokens::new(&theme);
        let mut local = header();
        local.local_only = true;
        let mut remote = header();
        remote.local_only = false;
        let local_text: String = local
            .spans(&tokens, 120)
            .iter()
            .map(|span| span.content.as_ref())
            .collect();
        let remote_text: String = remote
            .spans(&tokens, 120)
            .iter()
            .map(|span| span.content.as_ref())
            .collect();
        assert_ne!(local_text, remote_text);
        assert!(local_text.contains("[local]"));
        assert!(remote_text.contains("[remote]"));
    }

    #[test]
    fn empty_fields_are_skipped_rather_than_leaving_stray_separators() {
        let theme = test_theme();
        let tokens = Tokens::new(&theme);
        let sparse = Header {
            repository: "repo",
            branch: "main",
            model: "",
            workflow_control: WorkflowControl::Direct,
            workflow_profile: WorkflowProfile::Direct,
            search_policy: SearchPolicy::Off,
            mode: "",
            permission: "",
            phase: "",
            local_only: true,
        };
        let line: String = sparse
            .spans(&tokens, 80)
            .iter()
            .map(|span| span.content.as_ref())
            .collect();
        assert!(!line.contains(" ·  · "), "stray separators in {line}");
        assert!(!line.ends_with('·'));
    }

    #[test]
    fn a_one_column_header_does_not_panic() {
        for width in 0..4 {
            let _ = rendered(width, true);
        }
    }
}
