use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::Line;
use ratatui::widgets::{Paragraph, Widget};

use crate::app::{App, Focus};

const SIDEBAR_HINTS: &str =
    "j/k move  Tab agent  a/D repo +/-  n/d worktree +/-  r refresh  : palette  q/Ctrl-Q quit";
const AGENT_HINTS: &str = "keys go to the agent  Ctrl-O back to sidebar  Ctrl-Q quit";

pub fn render(app: &App, area: Rect, buf: &mut Buffer) {
    let line = match &app.status {
        Some(status) => Line::styled(status.clone(), Style::default().fg(Color::Yellow)),
        None => {
            let hints = match app.focus {
                Focus::Sidebar => SIDEBAR_HINTS,
                Focus::Agent => AGENT_HINTS,
            };
            Line::styled(hints, Style::default().fg(Color::DarkGray))
        }
    };
    Paragraph::new(line).render(area, buf);
}
