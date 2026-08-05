//! The review screen: changed files, bounded diff, validation summary.

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Widget};

use crate::app::App;
use crate::components::hints::Hints;
use crate::components::validation_summary::ValidationSummaryView;
use crate::design::{Emphasis, Role, Symbols, Tokens};
use crate::layout::Breakpoint;
use crate::review::ReviewState;

pub fn draw(frame: &mut Frame<'_>, app: &App) {
    let theme = app.theme.clone();
    let tokens = Tokens::new(&theme);
    let Some(review) = &app.review else {
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Min(3),
                Constraint::Length(1),
            ])
            .split(frame.area());
        Paragraph::new(Line::from(Span::styled(
            "Review".to_owned(),
            tokens.styled(Role::Accent, Emphasis::Strong),
        )))
        .render(rows[0], frame.buffer_mut());
        draw_unavailable(frame, rows[1], &tokens);
        Hints {
            entries: vec!["Esc Close".to_owned()],
        }
        .render(frame, rows[2], &tokens);
        return;
    };

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(3),
            Constraint::Length(if app.review_validation_expanded {
                10
            } else {
                4
            }),
            Constraint::Length(1),
        ])
        .split(frame.area());

    Paragraph::new(Line::from(vec![
        Span::styled(
            "Review  ".to_owned(),
            tokens.styled(Role::Accent, Emphasis::Strong),
        ),
        Span::styled(
            review.summary_with(Symbols::new(tokens.unicode()).field_separator()),
            tokens.styled(Role::Muted, Emphasis::Dim),
        ),
    ]))
    .render(rows[0], frame.buffer_mut());

    // Narrow terminals show one surface at a time: the file list and the diff
    // never share a row below 80 columns.
    if Breakpoint::of(frame.area().width) == Breakpoint::Narrow {
        if app.review_show_files {
            draw_files(frame, rows[1], review, &tokens);
        } else {
            draw_diff(frame, rows[1], review, &tokens);
        }
    } else {
        let columns = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(34), Constraint::Min(30)])
            .split(rows[1]);
        draw_files(frame, columns[0], review, &tokens);
        draw_diff(frame, columns[1], review, &tokens);
    }

    ValidationSummaryView::new(&app.conversation.progress.validation).render(
        frame,
        crate::design::Spacing::gutter(rows[2]),
        &tokens,
    );

    let mut hints = vec![
        "J Next file".to_owned(),
        "K Previous file".to_owned(),
        "N Next hunk".to_owned(),
        "P Previous hunk".to_owned(),
        "V Validation".to_owned(),
        "T Trace".to_owned(),
        "Esc Close".to_owned(),
    ];
    if Breakpoint::of(frame.area().width) == Breakpoint::Narrow {
        hints.insert(0, "F Files/diff".to_owned());
    }
    Hints { entries: hints }.render(frame, rows[3], &tokens);
}

fn draw_files(frame: &mut Frame<'_>, area: Rect, review: &ReviewState, tokens: &Tokens<'_>) {
    let symbols = Symbols::new(tokens.unicode());
    let block = Block::default()
        .title(" Changed files ")
        .borders(Borders::RIGHT)
        .border_set(symbols.border_set())
        .border_style(tokens.style(Role::Border));
    let inner = crate::design::Spacing::gutter(block.inner(area));
    block.render(area, frame.buffer_mut());
    if inner.width == 0 || inner.height == 0 {
        return;
    }

    let mut lines = Vec::new();
    for (index, file) in review.files.iter().enumerate() {
        let selected = index == review.selected_file;
        lines.push(Line::from(vec![
            Span::styled(
                format!(
                    "{}{} ",
                    if selected {
                        symbols.selection()
                    } else {
                        symbols.no_selection()
                    },
                    file.status.letter()
                ),
                tokens.style(status_role(file.status)),
            ),
            Span::styled(
                file.path.clone(),
                tokens.styled(
                    if selected {
                        Role::Selected
                    } else {
                        Role::Primary
                    },
                    Emphasis::Normal,
                ),
            ),
        ]));
        lines.push(Line::from(vec![
            Span::raw("   "),
            Span::styled(
                file.summary_with(symbols.field_separator()),
                tokens.styled(Role::Muted, Emphasis::Dim),
            ),
        ]));
    }
    // Changed files the patch has no body for are still listed, so the count in
    // the header always matches what the user can see.
    for effect in &review.without_patch {
        lines.push(Line::from(vec![
            Span::styled(
                format!("  {} ", effect.status.letter()),
                tokens.style(status_role(effect.status)),
            ),
            Span::styled(effect.path.clone(), tokens.style(Role::Primary)),
        ]));
        lines.push(Line::from(vec![
            Span::raw("   "),
            Span::styled(
                format!(
                    "{}{}no diff body recorded",
                    effect.status.word(),
                    symbols.field_separator()
                ),
                tokens.styled(Role::Muted, Emphasis::Dim),
            ),
        ]));
    }
    if lines.is_empty() {
        lines.push(Line::from(Span::styled(
            "No changed files were recorded".to_owned(),
            tokens.styled(Role::Muted, Emphasis::Dim),
        )));
    }
    Paragraph::new(lines).render(inner, frame.buffer_mut());
}

const fn status_role(status: crate::activity::EffectStatus) -> Role {
    use crate::activity::EffectStatus;
    match status {
        EffectStatus::Added | EffectStatus::Untracked => Role::Success,
        EffectStatus::Deleted => Role::Danger,
        EffectStatus::Modified | EffectStatus::Renamed => Role::Warning,
        EffectStatus::Unknown => Role::Muted,
    }
}

fn draw_diff(frame: &mut Frame<'_>, area: Rect, review: &ReviewState, tokens: &Tokens<'_>) {
    let area = crate::design::Spacing::gutter(area);
    if area.width == 0 || area.height == 0 {
        return;
    }
    let Some(file) = review.file() else {
        Paragraph::new(Line::from(Span::styled(
            "Select a file to see its changes".to_owned(),
            tokens.styled(Role::Muted, Emphasis::Dim),
        )))
        .render(area, frame.buffer_mut());
        return;
    };

    let mut lines = vec![Line::from(vec![
        Span::styled(
            file.path.clone(),
            tokens.styled(Role::Primary, Emphasis::Strong),
        ),
        Span::styled(
            format!(
                "  {}",
                file.summary_with(Symbols::new(tokens.unicode()).field_separator())
            ),
            tokens.styled(Role::Muted, Emphasis::Dim),
        ),
    ])];

    if file.binary {
        lines.push(Line::from(Span::styled(
            "Binary file — there is no reviewable text difference.".to_owned(),
            tokens.style(Role::Warning),
        )));
    } else if file.hunks.is_empty() {
        lines.push(Line::from(Span::styled(
            "No hunks were recorded for this file.".to_owned(),
            tokens.styled(Role::Muted, Emphasis::Dim),
        )));
    } else {
        lines.push(Line::from(Span::styled(
            format!("hunk {} of {}", review.selected_hunk + 1, file.hunks.len()),
            tokens.styled(Role::Muted, Emphasis::Dim),
        )));
        let (body, truncated) = review.selected_hunk_lines();
        if truncated > 0 {
            // Stated before the body: a notice below a long hunk would fall off
            // the bottom of the region and never be read.
            lines.push(Line::from(Span::styled(
                format!("{truncated} more line(s) in this hunk are not shown"),
                tokens.styled(Role::Warning, Emphasis::Dim),
            )));
        }
        for line in body {
            let role = if line.starts_with('+') && !line.starts_with("+++") {
                Role::Success
            } else if line.starts_with('-') && !line.starts_with("---") {
                Role::Danger
            } else if line.starts_with("@@") {
                Role::Accent
            } else {
                Role::Muted
            };
            lines.push(Line::from(Span::styled(
                line.to_owned(),
                tokens.style(role),
            )));
        }
    }
    Paragraph::new(lines).render(area, frame.buffer_mut());
}

fn draw_unavailable(frame: &mut Frame<'_>, area: Rect, tokens: &Tokens<'_>) {
    use crate::components::inspector::{Inspector, InspectorSubject};
    Inspector::new(InspectorSubject::Unavailable("The session diff")).render(frame, area, tokens);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_fixtures::{offline_app, test_terminal};
    use serde_json::json;

    const PATCH: &str = "\
diff --git a/src/lib.rs b/src/lib.rs
@@ -1,2 +1,3 @@
 fn one() {}
+fn two() {}
diff --git a/src/gone.rs b/src/gone.rs
@@ -1,2 +0,0 @@
-fn gone() {}
";
    const PORCELAIN: &str = " M src/lib.rs\n D src/gone.rs\n?? notes.txt\n";

    fn app_with_review() -> App {
        let mut app = offline_app();
        app.workspace.daemon_health = "connected".into();
        app.review = Some(ReviewState::from_daemon(PATCH, PORCELAIN));
        app.conversation.reconcile(
            None,
            vec![
                json!({"event":"validation_recorded","data":{"status":"failed","evidence":"2 checks failed"}}),
            ],
        );
        app
    }

    fn screen(app: &App, width: u16, height: u16) -> String {
        let mut terminal = test_terminal(width, height);
        terminal.draw(|frame| draw(frame, app)).unwrap();
        terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }

    #[test]
    fn review_lists_files_with_their_status_and_shows_the_selected_diff() {
        let rendered = screen(&app_with_review(), 120, 30);
        assert!(rendered.contains("src/lib.rs"));
        assert!(rendered.contains("src/gone.rs"));
        assert!(rendered.contains("modified"));
        assert!(rendered.contains("deleted"));
        assert!(rendered.contains("+fn two() {}"));
        assert!(rendered.contains("hunk 1 of 1"));
    }

    #[test]
    fn a_changed_file_with_no_patch_body_is_still_listed() {
        let rendered = screen(&app_with_review(), 120, 30);
        assert!(rendered.contains("notes.txt"));
        assert!(rendered.contains("no diff body recorded"));
    }

    #[test]
    fn validation_is_summarized_beside_the_diff_and_failure_is_not_hidden() {
        let rendered = screen(&app_with_review(), 120, 30);
        assert!(rendered.contains("Validation failed"));
        assert!(rendered.contains("2 checks failed"));
        assert!(!rendered.contains("Validation passed"));
    }

    #[test]
    fn navigation_hints_are_always_visible() {
        let rendered = screen(&app_with_review(), 120, 30);
        for hint in ["J Next file", "K Previous file", "N Next hunk", "Esc Close"] {
            assert!(rendered.contains(hint), "missing {hint}");
        }
    }

    #[test]
    fn narrow_review_shows_one_surface_at_a_time() {
        let mut app = app_with_review();
        app.review_show_files = true;
        let files = screen(&app, 60, 24);
        assert!(files.contains("src/lib.rs"));
        assert!(
            !files.contains("+fn two() {}"),
            "the diff must not share the row at 60 columns: {files}"
        );
        assert!(files.contains("F Files/diff"));

        app.review_show_files = false;
        let diff = screen(&app, 60, 24);
        assert!(diff.contains("+fn two() {}"));
    }

    #[test]
    fn selecting_a_binary_file_says_there_is_nothing_to_review() {
        let mut app = offline_app();
        app.review = Some(ReviewState::from_daemon(
            "diff --git a/logo.png b/logo.png\nBinary files a/logo.png and b/logo.png differ\n",
            "M  logo.png\n",
        ));
        let rendered = screen(&app, 120, 24);
        assert!(rendered.contains("Binary file"));
        assert!(rendered.contains("no reviewable text difference"));
    }

    #[test]
    fn an_absent_diff_is_reported_as_unavailable_not_as_no_changes() {
        let app = offline_app();
        let rendered = screen(&app, 120, 24);
        assert!(rendered.contains("The session diff is unavailable"));
        assert!(rendered.contains("missing record, not an empty one"));
        assert!(
            rendered.contains("Review"),
            "an unavailable review must still say which screen it is"
        );
        assert!(rendered.contains("Esc Close"), "and how to leave it");
    }

    #[test]
    fn a_large_hunk_reports_what_it_withheld() {
        let mut patch = String::from("diff --git a/big.rs b/big.rs\n@@ -1,900 +1,900 @@\n");
        for index in 0..900 {
            patch.push_str(&format!("+line {index}\n"));
        }
        let mut app = offline_app();
        app.review = Some(ReviewState::from_daemon(&patch, "M  big.rs\n"));
        let rendered = screen(&app, 120, 40);
        assert!(rendered.contains("line(s) in this hunk are not shown"));
    }

    #[test]
    fn review_renders_at_every_supported_size() {
        let app = app_with_review();
        for (width, height) in [(60, 24), (80, 24), (120, 30), (160, 40)] {
            let rendered = screen(&app, width, height);
            assert!(
                rendered.contains("Review"),
                "review header vanished at {width}x{height}"
            );
        }
    }
}
