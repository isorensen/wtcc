pub mod palette;
pub mod sidebar;
pub mod statusbar;

use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Widget, Wrap};
use tui_term::widget::PseudoTerminal;

use crate::app::{App, Confirm, Overlay, Prompt};

const AGENT_PLACEHOLDER: &str = "Agent pane — select a worktree (PTY coming next milestone)";
const SIDEBAR_WIDTH: u16 = 34;
const STATUS_HEIGHT: u16 = 1;

pub fn draw(frame: &mut Frame, app: &App) {
    let area = frame.area();
    render(app, area, frame.buffer_mut());
}

/// Renders the full UI into `buf`. Split out from [`draw`] so it can be
/// exercised against a `TestBackend` buffer without a real terminal.
pub fn render(app: &App, area: Rect, buf: &mut Buffer) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(STATUS_HEIGHT)])
        .split(area);

    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(SIDEBAR_WIDTH), Constraint::Min(10)])
        .split(chunks[0]);

    sidebar::render(app, body[0], buf);
    render_agent(app, body[1], buf);
    statusbar::render(app, chunks[1], buf);

    match &app.overlay {
        Overlay::None => {}
        Overlay::Palette { query, selected } => render_palette(query, *selected, area, buf),
        Overlay::Input { prompt, buffer } => render_input(prompt, buffer, area, buf),
        Overlay::Confirm(confirm) => render_confirm(confirm, area, buf),
    }
}

fn render_agent(app: &App, area: Rect, buf: &mut Buffer) {
    let title = match app.current_worktree() {
        Some(wt) if !wt.branch.is_empty() => format!(" agent · {} ", wt.branch),
        _ => " agent ".to_string(),
    };
    let block = Block::default().title(title).borders(Borders::ALL);
    let session = app
        .active_session
        .as_deref()
        .and_then(|n| app.session_manager.get(n));
    match session {
        Some(s) => {
            let parser = s.parser().lock().unwrap();
            PseudoTerminal::new(parser.screen())
                .block(block)
                .render(area, buf);
        }
        None => {
            Paragraph::new(AGENT_PLACEHOLDER)
                .block(block)
                .alignment(Alignment::Center)
                .wrap(Wrap { trim: true })
                .render(area, buf);
        }
    }
}

/// Inner (rows, cols) of the agent pane given the full frame area, matching `render`'s layout.
pub fn agent_pane_size(area: Rect) -> (u16, u16) {
    let body_height = area.height.saturating_sub(STATUS_HEIGHT);
    let pane_width = area.width.saturating_sub(SIDEBAR_WIDTH);
    // subtract the bordered Block (1 cell each side)
    let rows = body_height.saturating_sub(2);
    let cols = pane_width.saturating_sub(2);
    (rows.max(1), cols.max(1))
}

fn render_palette(query: &str, selected: usize, area: Rect, buf: &mut Buffer) {
    let rect = centered(60, 40, area);
    Clear.render(rect, buf);

    let block = Block::default()
        .title(" command palette ")
        .borders(Borders::ALL);
    let inner = block.inner(rect);
    block.render(rect, buf);

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(1)])
        .split(inner);

    Paragraph::new(Line::from(vec![
        Span::raw("> "),
        Span::raw(query),
        Span::styled("█", Style::default().add_modifier(Modifier::SLOW_BLINK)),
    ]))
    .render(rows[0], buf);

    let items: Vec<ListItem> = palette::filter(query)
        .into_iter()
        .enumerate()
        .map(|(i, cmd)| {
            let style = if i == selected {
                Style::default().add_modifier(Modifier::REVERSED)
            } else {
                Style::default()
            };
            ListItem::new(Line::styled(cmd.label(), style))
        })
        .collect();
    List::new(items).render(rows[1], buf);
}

fn render_input(prompt: &Prompt, buffer: &str, area: Rect, buf: &mut Buffer) {
    let rect = centered(60, 20, area);
    Clear.render(rect, buf);

    let title = match prompt {
        Prompt::AddWorktree => " new worktree branch ",
    };
    let block = Block::default().title(title).borders(Borders::ALL);
    let inner = block.inner(rect);
    block.render(rect, buf);

    Paragraph::new(Line::from(vec![
        Span::raw("branch: "),
        Span::raw(buffer),
        Span::styled("█", Style::default().add_modifier(Modifier::SLOW_BLINK)),
    ]))
    .render(inner, buf);
}

fn render_confirm(confirm: &Confirm, area: Rect, buf: &mut Buffer) {
    let rect = centered(60, 20, area);
    Clear.render(rect, buf);

    let block = Block::default().title(" confirm ").borders(Borders::ALL);
    let inner = block.inner(rect);
    block.render(rect, buf);

    let text = match confirm {
        Confirm::RemoveWorktree(path) => {
            format!("Remove worktree {}? (y/n)", path.display())
        }
    };
    Paragraph::new(text)
        .wrap(Wrap { trim: true })
        .render(inner, buf);
}

fn centered(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(area);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(vertical[1])[1]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::repository::Repository;
    use crate::worktree::Worktree;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use std::path::PathBuf;

    fn app_for_render() -> App {
        let mut app = App {
            config: Config {
                repos: vec![Repository {
                    name: "demo-repo".to_string(),
                    path: PathBuf::from("/tmp/demo-repo"),
                }],
                agent_cmd: "claude".to_string(),
            },
            selected_repo: Some(0),
            worktrees: vec![Worktree {
                path: PathBuf::from("/tmp/demo-repo/main"),
                branch: "main".to_string(),
                head: "abc123".to_string(),
                is_bare: false,
                is_detached: false,
            }],
            selected_worktree: Some(0),
            focus: crate::app::Focus::Sidebar,
            overlay: Overlay::None,
            status: None,
            should_quit: false,
            session_manager: crate::session::SessionManager::new(),
            active_session: None,
        };
        app.status = None;
        app
    }

    fn rendered_text(app: &App) -> String {
        let backend = TestBackend::new(100, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| draw(f, app)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        buffer
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>()
    }

    #[test]
    fn renders_repo_name_and_keybind_hints() {
        let app = app_for_render();
        let text = rendered_text(&app);
        assert!(text.contains("demo-repo"), "expected repo name in output");
        assert!(text.contains("main"), "expected worktree branch in output");
        assert!(text.contains("q quit"), "expected keybind hint in output");
        assert!(
            text.contains("Agent pane"),
            "expected agent placeholder in output"
        );
    }

    #[test]
    fn renders_status_line_when_set() {
        let mut app = app_for_render();
        app.status = Some("something happened".to_string());
        let text = rendered_text(&app);
        assert!(text.contains("something happened"));
    }

    #[test]
    fn renders_palette_overlay() {
        let mut app = app_for_render();
        app.overlay = Overlay::Palette {
            query: String::new(),
            selected: 0,
        };
        let text = rendered_text(&app);
        assert!(text.contains("command palette"));
        assert!(text.contains("Add worktree"));
    }
}
