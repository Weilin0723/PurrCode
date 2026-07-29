use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use crate::theme::Theme;

pub struct ActionArea {
    pub visible: bool,
    pub message: Option<String>,
    pub pending_approval: bool,
    pub streaming: bool,
}

impl ActionArea {
    pub const fn height(&self) -> u16 {
        if self.visible && (self.message.is_some() || self.pending_approval || self.streaming) {
            4
        } else {
            1
        }
    }

    pub fn render(&self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let palette = &theme.palette;
        let style = Style::default().fg(palette.text_muted).bg(palette.surface);
        let border_style = Style::default().fg(palette.border);

        let mut lines = Vec::new();
        if let Some(msg) = &self.message {
            lines.push(ratatui::text::Line::from(ratatui::text::Span::styled(
                msg,
                Style::default().fg(palette.text_primary),
            )));
        }
        if self.pending_approval {
            lines.push(ratatui::text::Line::from(ratatui::text::Span::styled(
                "Pending approval: [A] approve · [R] reject",
                Style::default().fg(palette.warning),
            )));
        }
        if self.streaming {
            lines.push(ratatui::text::Line::from(ratatui::text::Span::styled(
                "Processing...",
                Style::default().fg(palette.accent),
            )));
        }

        let paragraph = Paragraph::new(lines)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(border_style)
                    .title(" Actions ")
                    .title_style(Style::default().fg(palette.text_muted)),
            )
            .style(style);

        frame.render_widget(paragraph, area);
    }
}
