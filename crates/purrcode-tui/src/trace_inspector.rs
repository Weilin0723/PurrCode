use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};

use crate::theme::Theme;

pub struct TraceInspector {
    pub visible: bool,
    pub event_index: usize,
    pub event_type: String,
    pub event_detail: Vec<String>,
    pub total_events: usize,
}

impl TraceInspector {
    pub fn render(&self, frame: &mut Frame, area: Rect, theme: &Theme) {
        if !self.visible {
            return;
        }
        let palette = &theme.palette;

        let overlay = Rect {
            x: area.width.saturating_sub(60).min(area.width) / 2,
            y: area.height.saturating_sub(16).min(area.height) / 2,
            width: 60.min(area.width),
            height: 16.min(area.height),
        };

        frame.render_widget(Clear, overlay);

        let mut lines = vec![
            Line::from(ratatui::text::Span::styled(
                format!(" Event {} / {} ", self.event_index + 1, self.total_events),
                Style::default().fg(palette.accent),
            )),
            Line::from(ratatui::text::Span::styled(
                format!(" Type: {}", self.event_type),
                Style::default().fg(palette.text_primary),
            )),
            Line::from(""),
        ];

        for detail in &self.event_detail {
            for line in textwrap::wrap(detail, 56) {
                lines.push(Line::from(ratatui::text::Span::styled(
                    line.to_string(),
                    Style::default().fg(palette.text_muted),
                )));
            }
        }

        lines.push(Line::from(""));
        lines.push(
            Line::from(ratatui::text::Span::styled(
                " \u{2190} \u{2192} navigate \u{00B7} Esc close ",
                Style::default().fg(palette.text_muted),
            ))
            .alignment(Alignment::Center),
        );

        let paragraph = Paragraph::new(lines)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(palette.border))
                    .title(" Trace Inspector ")
                    .title_style(Style::default().fg(palette.accent)),
            )
            .style(Style::default().bg(palette.elevated))
            .wrap(Wrap { trim: false });

        frame.render_widget(paragraph, overlay);
    }
}
