use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use crate::theme::Theme;

#[allow(clippy::too_many_arguments)]
pub fn render_status_header(
    frame: &mut Frame,
    area: Rect,
    theme: &Theme,
    version: &str,
    repository: &str,
    model: &str,
    sandbox: &str,
    session: &str,
    phase: &str,
    mode: &str,
    privacy: &str,
    locality: &str,
) {
    let palette = &theme.palette;
    let wide = area.width >= 100;

    let mut parts: Vec<(String, Style)> = Vec::new();

    let accent_style = Style::default().fg(palette.accent);
    let muted_style = Style::default().fg(palette.text_muted);

    parts.push(("PurrCode ".to_string(), accent_style));
    parts.push((version.to_string(), muted_style));

    if wide {
        parts.push((" · ".to_string(), muted_style));
        parts.push((repository.to_string(), Style::default().fg(palette.accent)));
        parts.push((" · ".to_string(), muted_style));
        parts.push((model.to_string(), Style::default().fg(palette.warning)));
        parts.push((" · ".to_string(), muted_style));
        parts.push((mode.to_string(), Style::default().fg(palette.secondary)));
        parts.push((" · ".to_string(), muted_style));
        parts.push((locality.to_string(), muted_style));
        parts.push((" ".to_string(), muted_style));
        parts.push((privacy.to_string(), muted_style));
        parts.push((" · sandbox:".to_string(), muted_style));
        parts.push((sandbox.to_string(), muted_style));
        parts.push((" · session:".to_string(), muted_style));
        parts.push((session.to_string(), muted_style));
        parts.push((" ".to_string(), muted_style));
        parts.push((phase.to_string(), Style::default().fg(palette.success)));
    } else {
        parts.push((" ".to_string(), muted_style));
        parts.push((repository.to_string(), Style::default().fg(palette.accent)));
        parts.push((" ".to_string(), muted_style));
        parts.push((model.to_string(), Style::default().fg(palette.warning)));
    }

    let spans: Vec<_> = parts
        .into_iter()
        .map(|(text, style)| ratatui::text::Span::styled(text, style))
        .collect();

    let paragraph = Paragraph::new(Line::from(spans))
        .block(
            Block::default()
                .borders(Borders::BOTTOM)
                .border_style(Style::default().fg(palette.border)),
        )
        .style(Style::default().bg(palette.surface));

    frame.render_widget(paragraph, area);
}
