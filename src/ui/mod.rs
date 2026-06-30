pub mod palette;
pub mod sidebar;
pub mod statusbar;

use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Widget, Wrap};
use tui_term::widget::{Cursor, PseudoTerminal};

use crate::app::{App, Confirm, Focus, Overlay, Prompt};

const AGENT_PLACEHOLDER: &str = "No worktree selected — press a to register a repository";
pub(crate) const SIDEBAR_WIDTH: u16 = 34;
pub(crate) const STATUS_HEIGHT: u16 = 1;

/// Border style for a pane given whether it currently has focus. The focused
/// pane gets a distinct, bold border (`theme.border_focus`) as the focus cue;
/// unfocused panes use the dim `theme.border`.
pub(crate) fn pane_border_style(theme: &crate::theme::Theme, focused: bool) -> Style {
    if focused {
        Style::default()
            .fg(theme.border_focus)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.border)
    }
}

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
        Overlay::Input { prompt, buffer } => render_input(app, prompt, buffer, area, buf),
        Overlay::Confirm(confirm) => render_confirm(app, confirm, area, buf),
        Overlay::Help => render_help(app, area, buf),
    }
}

fn render_agent(app: &App, area: Rect, buf: &mut Buffer) {
    let title = match app.current_worktree() {
        Some(wt) if !wt.branch.is_empty() => format!(" agent · {} ", wt.branch),
        _ => " agent ".to_string(),
    };
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(pane_border_style(&app.theme, app.focus == Focus::Agent));
    let session = app
        .active_session
        .as_deref()
        .and_then(|n| app.session_manager.get(n));
    match session {
        Some(s) => {
            let parser = s.parser().lock().unwrap();
            let screen = parser.screen();
            // The widget only draws the cursor when the screen has not hidden it
            // (DECTCEM) AND `Cursor::show` is set, so gating on focus alone keeps
            // the screen's own hidden state intact.
            let cursor = Cursor::default().visibility(agent_cursor_shown(app.focus));
            PseudoTerminal::new(screen)
                .block(block)
                .cursor(cursor)
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

/// Whether the agent pane should request a visible terminal cursor. Only the
/// focused agent pane shows the caret; the screen's own DECTCEM hidden state is
/// still honored downstream by `tui-term`.
fn agent_cursor_shown(focus: Focus) -> bool {
    focus == Focus::Agent
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

fn render_input(app: &App, prompt: &Prompt, buffer: &str, area: Rect, buf: &mut Buffer) {
    let rect = centered(60, 20, area);
    Clear.render(rect, buf);

    let (title, label) = match prompt {
        Prompt::AddWorktree => (" add worktree ", "branch (new or existing): ".to_string()),
        Prompt::AddRepo => (" register repository ", "path: ".to_string()),
        Prompt::RenameBranch => (" rename branch ", "new branch name: ".to_string()),
        Prompt::SwitchAgent => {
            let names = app
                .config
                .presets()
                .iter()
                .map(|p| p.name.clone())
                .collect::<Vec<_>>()
                .join(", ");
            (" switch agent ", format!("switch agent ({names}): "))
        }
    };
    let block = Block::default().title(title).borders(Borders::ALL);
    let inner = block.inner(rect);
    block.render(rect, buf);

    Paragraph::new(Line::from(vec![
        Span::raw(label),
        Span::raw(buffer),
        Span::styled("█", Style::default().add_modifier(Modifier::SLOW_BLINK)),
    ]))
    .render(inner, buf);
}

fn render_confirm(app: &App, confirm: &Confirm, area: Rect, buf: &mut Buffer) {
    let rect = centered(60, 20, area);
    Clear.render(rect, buf);

    let block = Block::default().title(" confirm ").borders(Borders::ALL);
    let inner = block.inner(rect);
    block.render(rect, buf);

    let text = match confirm {
        Confirm::RemoveWorktree(path) => {
            format!("Remove worktree {}? (y/n)", path.display())
        }
        Confirm::RemoveRepo(index) => {
            let name = app
                .config
                .repos
                .get(*index)
                .map_or("<unknown>", |r| r.name.as_str());
            format!("Unregister repository {name}? (y/n)")
        }
        Confirm::RestartAgent(branch) => {
            format!("Restart agent for {branch}? (y/n)")
        }
        Confirm::MergePr(branch) => {
            format!("Merge PR for {branch}? (y/n)")
        }
        Confirm::ClosePr(branch) => {
            format!("Close PR for {branch}? (y/n)")
        }
    };
    Paragraph::new(text)
        .wrap(Wrap { trim: true })
        .render(inner, buf);
}

fn render_help(app: &App, area: Rect, buf: &mut Buffer) {
    use crate::keymap::{self, AGENT, PRIMARY};

    let bold = Style::default().add_modifier(Modifier::BOLD);
    let key_style = Style::default().fg(app.theme.accent);

    let row = |keys: &str, label: &str| {
        Line::from(vec![
            Span::styled(format!("  {keys:<12}"), key_style),
            Span::raw(label.to_string()),
        ])
    };

    // Derived entirely from the keymap table so help can never drift from the
    // live bindings; the agent forwarding notes are context, not bindings.
    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::styled("Sidebar".to_string(), bold));
    for (keys, label) in keymap::help_rows(PRIMARY) {
        lines.push(row(&keys, label));
    }
    lines.push(Line::raw(""));

    lines.push(Line::styled("Agent".to_string(), bold));
    lines.push(row("(keys)", "forwarded to the agent"));
    for (keys, label) in keymap::help_rows(AGENT) {
        lines.push(row(&keys, label));
    }
    lines.push(row("Ctrl-C", "forwarded to the agent"));

    // Size the box to its content (+2 for the border) so no section is clipped
    // on short terminals, clamped to the available area.
    let height = (lines.len() as u16 + 2).min(area.height);
    let width = (area.width * 6 / 10).clamp(40, area.width);
    let rect = centered_sized(width, height, area);
    Clear.render(rect, buf);

    let block = Block::default()
        .title(" help — keybindings ")
        .borders(Borders::ALL);
    let inner = block.inner(rect);
    block.render(rect, buf);

    Paragraph::new(lines).render(inner, buf);
}

/// Centers a fixed-size rect within `area`, clamping to its bounds.
fn centered_sized(width: u16, height: u16, area: Rect) -> Rect {
    let w = width.min(area.width);
    let h = height.min(area.height);
    let x = area.x + area.width.saturating_sub(w) / 2;
    let y = area.y + area.height.saturating_sub(h) / 2;
    Rect::new(x, y, w, h)
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
        let mut app = App::new(Config {
            repos: vec![Repository {
                name: "demo-repo".to_string(),
                path: PathBuf::from("/tmp/demo-repo"),
                setup: None,
                archive: None,
                archived: Vec::new(),
            }],
            agent_cmd: "claude".to_string(),
            notify: true,
            merge_strategy: crate::pr::MergeStrategy::default(),
            ..Default::default()
        });
        app.worktrees = vec![Worktree {
            path: PathBuf::from("/tmp/demo-repo/main"),
            branch: "main".to_string(),
            head: "abc123".to_string(),
            is_bare: false,
            is_detached: false,
        }];
        app.selected_worktree = Some(0);
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
        assert!(text.contains("quit"), "expected keybind hint in output");
        assert!(
            text.contains("register a repository"),
            "expected onboarding hint in agent placeholder"
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
    fn agent_cursor_shown_only_when_agent_focused() {
        assert!(agent_cursor_shown(Focus::Agent));
        assert!(!agent_cursor_shown(Focus::Sidebar));
    }

    #[test]
    fn renders_agent_pane_with_active_session_and_focus_without_panic() {
        use portable_pty::CommandBuilder;

        let mut app = app_for_render();
        let name = "wtcc-test-cursor";
        let mut cmd = CommandBuilder::new("printf");
        cmd.args(["hello-agent"]);
        app.session_manager
            .insert_spawned(name, cmd, &std::env::temp_dir(), 24, 80)
            .expect("spawn test session");
        app.active_session = Some(name.to_string());
        app.focus = Focus::Agent;

        // Must render the bordered pane (title) without panicking. The cursor
        // itself can't be asserted reliably in a buffer snapshot, but a clean
        // render with cursor visibility enabled is the regression guard.
        let text = rendered_text(&app);
        assert!(
            text.contains("agent"),
            "expected agent pane title in output"
        );
        assert!(
            !text.contains("register a repository"),
            "placeholder must not show once a session is active"
        );
    }

    #[test]
    fn renders_help_overlay() {
        let mut app = app_for_render();
        app.overlay = Overlay::Help;
        let text = rendered_text(&app);
        assert!(text.contains("keybindings"), "expected help title");
        assert!(text.contains("Sidebar"), "expected Sidebar heading");
        assert!(text.contains("command palette"), "expected a help binding");
        assert!(text.contains("quit"), "expected a quit binding");
    }

    #[test]
    fn help_overlay_is_derived_from_keymap_table() {
        use crate::keymap::{self, AGENT, PRIMARY};

        let mut app = app_for_render();
        app.overlay = Overlay::Help;

        // Render into a buffer large enough that the help box never clips.
        let backend = TestBackend::new(120, 40);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| draw(f, &app)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        let text: String = buffer.content().iter().map(|c| c.symbol()).collect();

        let mut rows = keymap::help_rows(PRIMARY);
        rows.extend(keymap::help_rows(AGENT));
        assert!(!rows.is_empty(), "expected a non-empty keymap table");
        for (_keys, label) in rows {
            assert!(
                text.contains(label),
                "help overlay is missing the binding label {label:?}"
            );
        }
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
