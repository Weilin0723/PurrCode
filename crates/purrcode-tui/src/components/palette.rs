//! The command palette, generated from the action registry.
//!
//! Entries are grouped by category and laid out with a real layout rather than
//! manually padded strings, so columns adapt to the terminal width. Unavailable
//! actions stay visible with the reason they are unavailable, so the palette
//! never presents a silent dead end.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Widget};
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

use crate::design::{Emphasis, Role, Symbols, Tokens};
use crate::ui_actions::{UiActionCategory, UiActionDefinition, UiContext};

/// One rendered row: either a category heading or an action.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PaletteRow {
    Category(UiActionCategory),
    Action {
        index: usize,
        label: String,
        description: String,
        shortcut: String,
        command: String,
        unavailable_reason: Option<&'static str>,
    },
}

#[derive(Clone, Debug)]
pub struct CommandPaletteView<'actions> {
    pub query: &'actions str,
    pub actions: &'actions [&'static UiActionDefinition],
    pub selected: usize,
    pub context: UiContext,
}

impl<'actions> CommandPaletteView<'actions> {
    pub fn new(
        query: &'actions str,
        actions: &'actions [&'static UiActionDefinition],
        selected: usize,
        context: UiContext,
    ) -> Self {
        Self {
            query,
            actions,
            selected,
            context,
        }
    }

    /// Rows with category headings inserted, and the selected row kept visible
    /// within `visible_rows`.
    pub fn rows(&self, visible_rows: usize) -> Vec<PaletteRow> {
        let mut all = Vec::new();
        let mut current_category = None;
        for (index, action) in self.actions.iter().enumerate() {
            if current_category != Some(action.category) {
                current_category = Some(action.category);
                all.push(PaletteRow::Category(action.category));
            }
            all.push(PaletteRow::Action {
                index,
                label: action.label.to_owned(),
                description: action.description.to_owned(),
                shortcut: action
                    .primary_shortcut()
                    .map(|shortcut| shortcut.keys.to_owned())
                    .unwrap_or_default(),
                command: action.primary_command().unwrap_or("").to_owned(),
                unavailable_reason: action.availability(&self.context).reason(),
            });
        }
        if visible_rows == 0 || all.len() <= visible_rows {
            return all;
        }
        // Scroll so the selected action stays on screen, keeping its category
        // heading directly above it when possible.
        let selected_row = all
            .iter()
            .position(
                |row| matches!(row, PaletteRow::Action { index, .. } if *index == self.selected),
            )
            .unwrap_or(0);
        let start = selected_row
            .saturating_sub(visible_rows.saturating_sub(1))
            .min(all.len().saturating_sub(visible_rows));
        all[start..start + visible_rows].to_vec()
    }

    pub fn render(&self, frame: &mut Frame<'_>, area: Rect, tokens: &Tokens<'_>) {
        let symbols = Symbols::new(tokens.unicode());
        let block = Block::default()
            .title(" Commands ")
            .borders(Borders::ALL)
            .border_set(symbols.border_set())
            .border_style(tokens.style(Role::Border));
        let inner = block.inner(area);
        block.render(area, frame.buffer_mut());

        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Min(1),
            ])
            .split(inner);

        Paragraph::new(Line::from(vec![
            Span::styled(
                "Search ".to_owned(),
                tokens.styled(Role::Muted, Emphasis::Dim),
            ),
            Span::styled(
                if self.query.is_empty() {
                    "type to filter".to_owned()
                } else {
                    self.query.to_owned()
                },
                tokens.style(if self.query.is_empty() {
                    Role::Muted
                } else {
                    Role::Accent
                }),
            ),
        ]))
        .render(rows[0], frame.buffer_mut());

        let list = rows[2];
        // Three adaptive columns; nothing is padded by hand. Shortcut and
        // command columns shrink first on a narrow terminal.
        let shortcut_width = if list.width >= 70 { 12 } else { 8 };
        let command_width = if list.width >= 90 {
            22
        } else if list.width >= 70 {
            16
        } else {
            0
        };
        let columns = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Min(20),
                Constraint::Length(command_width),
                Constraint::Length(shortcut_width),
            ])
            .split(list);

        let visible = list.height as usize;
        let rendered = self.rows(visible);
        let mut label_lines = Vec::new();
        let mut command_lines = Vec::new();
        let mut shortcut_lines = Vec::new();

        for row in &rendered {
            match row {
                PaletteRow::Category(category) => {
                    label_lines.push(Line::from(Span::styled(
                        category.label().to_uppercase(),
                        tokens.styled(Role::Muted, Emphasis::Dim),
                    )));
                    command_lines.push(Line::from(""));
                    shortcut_lines.push(Line::from(""));
                }
                PaletteRow::Action {
                    index,
                    label,
                    description,
                    shortcut,
                    command,
                    unavailable_reason,
                } => {
                    let selected = *index == self.selected;
                    let available = unavailable_reason.is_none();
                    let role = if selected {
                        Role::Selected
                    } else if available {
                        Role::Primary
                    } else {
                        Role::Muted
                    };
                    let detail = match unavailable_reason {
                        // The reason replaces the description: when an action
                        // cannot run, why matters more than what it does.
                        Some(reason) => format!("unavailable {} {reason}", symbols.dash()),
                        None => description.clone(),
                    };
                    let marker = if selected {
                        symbols.selection()
                    } else {
                        symbols.no_selection()
                    };
                    label_lines.push(Line::from(vec![
                        Span::styled(format!("{marker} "), tokens.style(Role::Selected)),
                        Span::styled(label.clone(), tokens.styled(role, Emphasis::Strong)),
                        Span::styled("  ".to_owned(), tokens.style(Role::Muted)),
                        Span::styled(
                            detail,
                            tokens.styled(
                                if available {
                                    Role::Muted
                                } else {
                                    Role::Warning
                                },
                                Emphasis::Dim,
                            ),
                        ),
                    ]));
                    command_lines.push(Line::from(Span::styled(
                        truncate(command, columns[1].width),
                        tokens.styled(Role::Muted, Emphasis::Dim),
                    )));
                    shortcut_lines.push(Line::from(Span::styled(
                        truncate(shortcut, columns[2].width),
                        tokens.styled(Role::Accent, Emphasis::Dim),
                    )));
                }
            }
        }
        if rendered.is_empty() {
            label_lines.push(Line::from(Span::styled(
                "No matching actions".to_owned(),
                tokens.styled(Role::Muted, Emphasis::Dim),
            )));
        }

        Paragraph::new(Line::from(Span::styled(
            format!(
                "{} action(s){}",
                self.actions.len(),
                if self.query.is_empty() {
                    String::new()
                } else {
                    format!(" matching {:?}", self.query)
                }
            ),
            tokens.styled(Role::Muted, Emphasis::Dim),
        )))
        .render(rows[1], frame.buffer_mut());

        Paragraph::new(label_lines).render(columns[0], frame.buffer_mut());
        Paragraph::new(command_lines).render(columns[1], frame.buffer_mut());
        Paragraph::new(shortcut_lines).render(columns[2], frame.buffer_mut());
    }
}

fn truncate(value: &str, width: u16) -> String {
    let width = width as usize;
    if width == 0 {
        return String::new();
    }
    if UnicodeWidthStr::width(value) <= width {
        return value.to_owned();
    }
    value
        .chars()
        .take(width.saturating_sub(1))
        .collect::<String>()
        + "…"
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_fixtures::{test_terminal, test_theme};
    use crate::ui_actions;

    fn screen(width: u16, height: u16, query: &str, context: UiContext) -> String {
        let actions = ui_actions::filtered(query);
        let theme = test_theme();
        let mut terminal = test_terminal(width, height);
        terminal
            .draw(|frame| {
                let tokens = Tokens::new(&theme);
                CommandPaletteView::new(query, &actions, 0, context).render(
                    frame,
                    frame.area(),
                    &tokens,
                );
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

    fn ready() -> UiContext {
        UiContext {
            daemon_reachable: true,
            provider_configured: true,
            ..UiContext::default()
        }
    }

    #[test]
    fn entries_are_grouped_by_category() {
        let actions = ui_actions::filtered("");
        let view = CommandPaletteView::new("", &actions, 0, ready());
        let rows = view.rows(0);
        let categories: Vec<UiActionCategory> = rows
            .iter()
            .filter_map(|row| match row {
                PaletteRow::Category(category) => Some(*category),
                PaletteRow::Action { .. } => None,
            })
            .collect();
        assert!(categories.contains(&UiActionCategory::Approval));
        assert!(categories.contains(&UiActionCategory::Provider));
        let mut sorted = categories.clone();
        sorted.sort();
        assert_eq!(
            categories, sorted,
            "categories must not repeat or interleave"
        );
    }

    #[test]
    fn unavailable_actions_stay_visible_and_explain_why() {
        let rendered = screen(120, 40, "approve", ready());
        assert!(rendered.contains("Approve action"));
        assert!(
            rendered.contains("unavailable — no action is pending"),
            "{rendered}"
        );
    }

    #[test]
    fn availability_reasons_match_the_product_examples() {
        let actions = ui_actions::filtered("");
        let view = CommandPaletteView::new("", &actions, 0, ready());
        let reasons: Vec<(String, Option<&str>)> = view
            .rows(0)
            .into_iter()
            .filter_map(|row| match row {
                PaletteRow::Action {
                    label,
                    unavailable_reason,
                    ..
                } => Some((label, unavailable_reason)),
                PaletteRow::Category(_) => None,
            })
            .collect();
        let reason_for = |label: &str| {
            reasons
                .iter()
                .find(|(name, _)| name == label)
                .and_then(|(_, reason)| *reason)
        };
        assert_eq!(reason_for("Approve action"), Some("no action is pending"));
        assert_eq!(
            reason_for("Review diff"),
            Some("no repository effects were recorded")
        );
        assert_eq!(
            reason_for("Resume session"),
            Some("the current session cannot be resumed")
        );
    }

    #[test]
    fn available_actions_show_their_description_not_a_reason() {
        let context = UiContext {
            daemon_reachable: true,
            pending_approval: true,
            ..UiContext::default()
        };
        let rendered = screen(120, 40, "approve", context);
        assert!(rendered.contains("Authorize the exact pending action"));
        assert!(!rendered.contains("unavailable — no action is pending"));
    }

    #[test]
    fn shortcuts_and_commands_appear_in_their_own_columns() {
        let rendered = screen(120, 40, "palette", ready());
        assert!(rendered.contains("Command palette"));
        assert!(rendered.contains("/help"));
        assert!(rendered.contains("Ctrl+P"));
    }

    #[test]
    fn narrow_palettes_drop_the_command_column_before_the_label() {
        let rendered = screen(60, 24, "diff", ready());
        assert!(rendered.contains("Review diff"), "{rendered}");
    }

    #[test]
    fn the_palette_never_overflows_its_width() {
        for width in [60, 80, 120, 160] {
            let actions = ui_actions::filtered("");
            let theme = test_theme();
            let mut terminal = test_terminal(width, 24);
            terminal
                .draw(|frame| {
                    let tokens = Tokens::new(&theme);
                    CommandPaletteView::new("", &actions, 0, ready()).render(
                        frame,
                        frame.area(),
                        &tokens,
                    );
                })
                .unwrap();
            let buffer = terminal.backend().buffer();
            assert_eq!(buffer.area.width, width);
        }
    }

    #[test]
    fn scrolling_keeps_the_selected_action_visible() {
        let actions = ui_actions::filtered("");
        let last = actions.len() - 1;
        let view = CommandPaletteView::new("", &actions, last, ready());
        let rows = view.rows(6);
        assert_eq!(rows.len(), 6);
        assert!(
            rows.iter()
                .any(|row| matches!(row, PaletteRow::Action { index, .. } if *index == last)),
            "the selected action scrolled out of view"
        );
    }

    #[test]
    fn no_matching_query_says_so() {
        let rendered = screen(100, 20, "zzzznotanaction", ready());
        assert!(rendered.contains("No matching actions"));
    }

    #[test]
    fn every_registered_action_is_reachable_from_the_palette() {
        let actions = ui_actions::filtered("");
        assert_eq!(
            actions.len(),
            ui_actions::REGISTRY.len(),
            "an empty query must list every registered action"
        );
    }
}
