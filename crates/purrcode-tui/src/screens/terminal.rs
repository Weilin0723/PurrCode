//! The terminal screen (PRD §19).
//!
//! Tabs on top, the emulated screen in the middle, and one hint line that
//! always states which terminal is focused and whether its process is still
//! running — the process keeps running whoever is typing, so "where does my
//! typing go" is the one question this surface must never leave ambiguous.

use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Widget};
use ratatui::Frame;

use crate::app::App;
use crate::components::hints::Hints;
use crate::design::{Emphasis, Role, Tokens};

pub fn draw(frame: &mut Frame<'_>, app: &App) {
    let theme = app.theme.clone();
    let tokens = Tokens::new(&theme);
    // A reported outcome gets its own row rather than competing with the key
    // hints for one line: an action that appears to do nothing is worse than
    // one that fails loudly, and hints the user cannot read are not hints.
    let notice = (!app.message_bar.is_empty()).then(|| app.message_bar.clone());
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(3),
            Constraint::Length(u16::from(notice.is_some())),
            Constraint::Length(1),
        ])
        .split(frame.area());

    Paragraph::new(Line::from(vec![
        Span::styled(
            "Terminal  ".to_owned(),
            tokens.styled(Role::Accent, Emphasis::Strong),
        ),
        Span::styled(
            app.terminal.tab_strip(),
            tokens.styled(Role::Primary, Emphasis::Normal),
        ),
    ]))
    .render(rows[0], frame.buffer_mut());

    let frame_block = Block::default()
        .borders(Borders::ALL)
        .border_style(tokens.styled(Role::Border, Emphasis::Normal));
    let inner = frame_block.inner(rows[1]);
    frame_block.render(rows[1], frame.buffer_mut());

    match app.terminal.active() {
        Some(tab) => {
            // Show the active region: trailing blank rows carry no information,
            // and the emulated screen is usually taller than the drawn area, so
            // slicing the raw bottom would hide the output entirely.
            let mut lines = tab.screen.lines();
            while lines
                .last()
                .is_some_and(|line| line.spans.iter().all(|span| span.content.trim().is_empty()))
            {
                lines.pop();
            }
            let first = lines.len().saturating_sub(inner.height as usize);
            Paragraph::new(lines[first..].to_vec()).render(inner, frame.buffer_mut());
        }
        None => Paragraph::new(Line::from(Span::styled(
            "No terminal is open.".to_owned(),
            tokens.styled(Role::Muted, Emphasis::Normal),
        )))
        .render(inner, frame.buffer_mut()),
    }

    if let Some(notice) = notice {
        Paragraph::new(Line::from(Span::styled(
            notice,
            tokens.styled(Role::Muted, Emphasis::Normal),
        )))
        .render(rows[2], frame.buffer_mut());
    }
    Hints {
        entries: hints(app),
    }
    .render(frame, rows[3], &tokens);
}

/// Contextual hints. Ownership is stated in words, never by colour alone, and
/// an action is only offered when it is actually available.
pub fn hints(app: &App) -> Vec<String> {
    let mut entries = Vec::new();
    let Some(tab) = app.terminal.active() else {
        entries.push("Esc Close".to_owned());
        return entries;
    };
    entries.push(format!(
        "{} {}",
        tab.title(),
        if tab.alive { "running" } else { "exited" }
    ));
    if tab.alive {
        entries.push("Typing goes to the process".to_owned());
        entries.push("Ctrl+W Return control to agent".to_owned());
    }
    if app.terminal.tabs.len() > 1 {
        entries.push("Tab Next terminal".to_owned());
    }
    entries.push("Esc Close".to_owned());
    entries
}

#[cfg(test)]
mod tests {
    use crate::terminal::{TabKind, TerminalPane, TerminalTab};

    fn pane_with(alive: bool, count: usize) -> TerminalPane {
        let mut pane = TerminalPane::default();
        for index in 0..count {
            let mut tab = TerminalTab::new(format!("t{index}"), TabKind::Tests, 4, 10);
            tab.alive = alive;
            pane.tabs.push(tab);
        }
        pane
    }

    #[test]
    fn an_exited_terminal_stops_offering_input_actions() {
        let running = pane_with(true, 1);
        let exited = pane_with(false, 1);
        assert!(running.active().unwrap().alive);
        assert_eq!(running.active().unwrap().title(), "Tests");
        assert_eq!(exited.active().unwrap().title(), "Tests (exited)");
    }

    #[test]
    fn the_tab_strip_only_appears_once_there_is_more_than_one_terminal() {
        assert_eq!(pane_with(true, 1).tabs.len(), 1);
        let two = pane_with(true, 2);
        assert!(two.tab_strip().contains("[Tests]"));
    }
}
