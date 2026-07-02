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

use crate::app::{App, Confirm, Focus, Overlay, Prompt, Selection};

const AGENT_PLACEHOLDER: &str = "No worktree selected — press a to register a repository";
pub(crate) const SIDEBAR_WIDTH: u16 = 34;
pub(crate) const STATUS_HEIGHT: u16 = 1;
/// Rows reserved for the per-worktree tab strip at the top of the agent pane.
pub(crate) const TAB_BAR_HEIGHT: u16 = 1;

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
    let inner = block.inner(area);
    block.render(area, buf);

    // Reserve the top row of the pane for the tab strip; the active tab's surface
    // fills the rest.
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(TAB_BAR_HEIGHT), Constraint::Min(1)])
        .split(inner);
    let strip_area = rows[0];
    let surface_area = rows[1];

    let layout = app
        .current_slug()
        .and_then(|s| app.layouts.get(&s).cloned());
    render_tab_strip(app, layout.as_ref(), strip_area, buf);

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
                .cursor(cursor)
                .render(surface_area, buf);
        }
        None => {
            Paragraph::new(AGENT_PLACEHOLDER)
                .alignment(Alignment::Center)
                .wrap(Wrap { trim: true })
                .render(surface_area, buf);
        }
    }

    // Overlay the drag-selection highlight on top of whatever the surface drew
    // (issue #103). We deliberately do NOT clear the selection on new PTY output:
    // the agent is a redraw-heavy TUI, so the highlight is cleared on the next
    // keypress instead (same rationale as the scrollback view in #106).
    if let Some(sel) = app.selection {
        overlay_selection(sel, area, buf);
    }
}

/// Reverses the styling of the cells covered by `sel` on the agent surface.
/// `area` is the agent pane rect (bordered block), so the surface's top-left is
/// `(area.x + 1, area.y + 1 + TAB_BAR_HEIGHT)`. Bounds-guarded via `cell_mut`:
/// cells outside the buffer are skipped, so it never panics.
fn overlay_selection(sel: Selection, area: Rect, buf: &mut Buffer) {
    let cols = area.width.saturating_sub(2);
    let x0 = area.x + 1;
    let y0 = area.y + 1 + TAB_BAR_HEIGHT;
    let (start, end) = sel.normalized();
    let reversed = Style::default().add_modifier(Modifier::REVERSED);
    for r in start.0..=end.0 {
        let c0 = if r == start.0 { start.1 } else { 0 };
        let c1 = if r == end.0 {
            end.1
        } else {
            cols.saturating_sub(1)
        };
        for c in c0..=c1 {
            if let Some(cell) = buf.cell_mut((x0 + c, y0 + r)) {
                cell.set_style(reversed);
            }
        }
    }
}

/// Draws the compact tab strip: each tab's title, the active one highlighted via
/// the theme accent (reversed/bold), inactive ones dimmed. Hidden (renders
/// nothing) when the worktree has no layout yet.
fn render_tab_strip(
    app: &App,
    layout: Option<&crate::layout::WorktreeLayout>,
    area: Rect,
    buf: &mut Buffer,
) {
    let Some(layout) = layout else {
        return;
    };
    let active_style = Style::default()
        .fg(app.theme.accent)
        .add_modifier(Modifier::REVERSED | Modifier::BOLD);
    let inactive_style = Style::default().fg(app.theme.hint);
    let spans: Vec<Span> = layout
        .tabs
        .iter()
        .enumerate()
        .map(|(i, tab)| {
            let style = if i == layout.active {
                active_style
            } else {
                inactive_style
            };
            Span::styled(format!(" {} ", tab.title), style)
        })
        .collect();
    Paragraph::new(Line::from(spans)).render(area, buf);
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
    let rows = body_height.saturating_sub(2).saturating_sub(TAB_BAR_HEIGHT);
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
        Confirm::MergePr { branch, .. } => {
            format!("Merge PR for {branch}? (y/n)")
        }
        Confirm::ClosePr { branch, .. } => {
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
    use crate::repository::{RepoKind, Repository};
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
                base_ref: None,
                copy_on_create: Vec::new(),
                run: None,
                kind: RepoKind::Git,
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
        app.worktree_repo = vec![0];
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

    // --- issue #48: per-worktree tab strip + agent_pane_size accounting -------
    //
    // TDD RED: `render_agent` draws a one-row tab strip (every tab title, the
    // active one highlighted via the theme) above the active tab's
    // `PseudoTerminal`, and `agent_pane_size` subtracts `TAB_BAR_HEIGHT` so the
    // PTY size equals the drawn pane. The strip is driven by the current
    // worktree's `WorktreeLayout` (titles "agent", "shell 1", ...).

    /// First column where `needle` starts in row `y`, comparing cell symbols so
    /// the index is a true terminal column (UTF-8 box-drawing dividers would
    /// otherwise skew a byte-based `str::find`).
    fn col_of(buf: &Buffer, y: u16, needle: &str) -> Option<u16> {
        let width = buf.area.width;
        let cells: Vec<String> = (0..width)
            .map(|x| buf[(x, y)].symbol().to_string())
            .collect();
        let want: Vec<String> = needle.chars().map(|c| c.to_string()).collect();
        if want.len() > cells.len() {
            return None;
        }
        (0..=cells.len() - want.len()).find_map(|start| {
            if cells[start..start + want.len()] == want[..] {
                Some(start as u16)
            } else {
                None
            }
        })
    }

    #[test]
    fn agent_pane_size_reserves_a_row_for_the_tab_strip() {
        let area = Rect::new(0, 0, 100, 24);
        let (rows, cols) = agent_pane_size(area);
        let body = 24u16 - STATUS_HEIGHT;
        assert_eq!(
            rows,
            body - 2 - TAB_BAR_HEIGHT,
            "the PTY height must exclude the borders AND the one-row tab strip"
        );
        assert_eq!(cols, 100 - SIDEBAR_WIDTH - 2);
    }

    #[test]
    fn tab_strip_renders_every_tab_title() {
        use portable_pty::CommandBuilder;

        let mut app = app_for_render(); // worktree branch "main"
        app.new_shell_tab(); // main layout: [agent, shell 1], active shell
        let shell = "wtcc-main-t1";
        let mut cmd = CommandBuilder::new("printf");
        cmd.args(["hi"]);
        app.session_manager
            .insert_spawned(shell, cmd, &std::env::temp_dir(), 24, 80)
            .unwrap();
        app.active_session = Some(shell.to_string());

        let text = rendered_text(&app);
        assert!(
            text.contains("agent"),
            "the agent tab title shows in the strip"
        );
        assert!(
            text.contains("shell 1"),
            "the shell tab title shows in the strip"
        );
    }

    #[test]
    fn tab_strip_highlights_the_active_tab() {
        use portable_pty::CommandBuilder;

        let mut app = app_for_render();
        app.new_shell_tab(); // active = shell 1 (index 1)
        let shell = "wtcc-main-t1";
        let mut cmd = CommandBuilder::new("printf");
        cmd.args(["hi"]);
        app.session_manager
            .insert_spawned(shell, cmd, &std::env::temp_dir(), 24, 80)
            .unwrap();
        app.active_session = Some(shell.to_string());

        let backend = TestBackend::new(100, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| draw(f, &app)).unwrap();
        let buffer = terminal.backend().buffer().clone();

        // The strip row is the one carrying the shell title.
        let strip_y = (0..buffer.area.height)
            .find(|&y| col_of(&buffer, y, "shell 1").is_some())
            .expect("a tab strip row with the shell title must render");
        let active_x = col_of(&buffer, strip_y, "shell 1").unwrap();
        let inactive_x = col_of(&buffer, strip_y, "agent").unwrap();

        assert_ne!(
            buffer[(active_x, strip_y)].style(),
            buffer[(inactive_x, strip_y)].style(),
            "the active tab title must be highlighted differently from inactive titles"
        );
    }

    // --- issue #103: drag-selection overlay ---------------------------------

    #[test]
    fn selection_reverses_the_selected_surface_cells() {
        let mut app = app_for_render();
        app.selection = Some(Selection {
            anchor: (0, 0),
            cursor: (0, 2),
        });

        let backend = TestBackend::new(100, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| draw(f, &app)).unwrap();
        let buffer = terminal.backend().buffer().clone();

        // Surface top-left in the full frame: sidebar + left border, then the
        // top border + tab strip.
        let x0 = SIDEBAR_WIDTH + 1;
        let y0 = 1 + TAB_BAR_HEIGHT;
        for c in 0..=2u16 {
            assert!(
                buffer[(x0 + c, y0)].modifier.contains(Modifier::REVERSED),
                "selected cell at col {c} must be reversed"
            );
        }
        assert!(
            !buffer[(x0 + 3, y0)].modifier.contains(Modifier::REVERSED),
            "the cell just past the selection must not be reversed"
        );
    }
}
