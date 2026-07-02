use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;

use crate::app::{App, Confirm, Focus, Overlay, Prompt, Selection};
use crate::keymap::{self, AGENT, Action, PRIMARY};
use crate::repository::Repository;
use crate::ui::palette;
use crate::ui::sidebar::{self, SidebarRow};
use crate::ui::{SIDEBAR_WIDTH, STATUS_HEIGHT, TAB_BAR_HEIGHT};
use crate::worktree::Worktree;

/// Applies a key event to the app, dispatching to the active overlay first and
/// falling back to the primary keymap. Pure state transition: no terminal I/O.
///
/// Quit semantics: Ctrl-Q always quits. Ctrl-C quits only when focus is Sidebar
/// or an overlay is open; when focus is Agent, Ctrl-C is forwarded to the agent.
/// `q` quits only from Sidebar focus (printable char the agent needs). In Agent
/// focus, Ctrl-O returns focus to Sidebar; all other keys forward to the agent.
pub fn handle_key(app: &mut App, key: KeyEvent) {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

    // Ctrl-Q always quits.
    if ctrl && matches!(key.code, KeyCode::Char('q')) {
        app.should_quit = true;
        return;
    }

    let overlay_open = !matches!(app.overlay, Overlay::None);

    // Ctrl-C quits when an overlay is open or focus is Sidebar; forwarded otherwise.
    if ctrl && matches!(key.code, KeyCode::Char('c')) && (overlay_open || app.focus != Focus::Agent)
    {
        app.should_quit = true;
        return;
    }

    // Any keystroke clears an active drag-selection highlight (issue #103). This
    // also covers focus changes made via keys (e.g. Ctrl-O).
    app.selection = None;

    match &app.overlay {
        Overlay::None => {
            if app.focus == Focus::Agent {
                handle_agent(app, key, ctrl);
            } else {
                handle_primary(app, key);
            }
        }
        Overlay::Palette { .. } => handle_palette(app, key),
        Overlay::Input { .. } => handle_input(app, key),
        Overlay::Confirm(_) => handle_confirm(app, key),
        // Any key dismisses the help overlay; it never leaks to the panes.
        Overlay::Help => app.overlay = Overlay::None,
    }
}

/// The UI element under a click, resolved by [`hit_test`]. `Repo`/`Worktree`
/// carry the index the click selects; `Agent` focuses the agent pane; `None` is
/// a blank/inert area (no-op).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hit {
    None,
    Agent,
    Repo(usize),
    Worktree(usize),
}

/// Pure hit-test: maps a click at `(col, row)` within `area` to a [`Hit`],
/// mirroring `ui::render`'s layout exactly. The body is everything above the
/// status line; the sidebar is a bordered block on the left and the agent pane
/// fills the rest. Sidebar rows are resolved through [`sidebar::sidebar_rows`]
/// so this can never drift from what the sidebar draws. No I/O, no panics.
#[allow(clippy::too_many_arguments)]
pub fn hit_test(
    col: u16,
    row: u16,
    area: Rect,
    repos: &[Repository],
    worktrees: &[Worktree],
    worktree_repo: &[usize],
    expanded_repos: &std::collections::HashSet<std::path::PathBuf>,
    show_archived: bool,
) -> Hit {
    let body_height = area.height.saturating_sub(STATUS_HEIGHT);
    // Below the body (status line, or off-screen) is inert.
    if row >= body_height {
        return Hit::None;
    }

    if col >= SIDEBAR_WIDTH {
        return Hit::Agent;
    }

    // Inside the sidebar's bordered block: the list starts one cell in from the
    // top/left border. A click on the border itself selects nothing.
    if row == 0 || col == 0 || col >= SIDEBAR_WIDTH - 1 {
        return Hit::None;
    }
    let list_index = (row - 1) as usize;
    match sidebar::sidebar_rows(
        repos,
        worktrees,
        worktree_repo,
        expanded_repos,
        show_archived,
    )
    .get(list_index)
    {
        Some(SidebarRow::RepoHeader(ri)) => Hit::Repo(*ri),
        Some(SidebarRow::Worktree(wi)) => Hit::Worktree(*wi),
        _ => Hit::None,
    }
}

/// Applies a left-click at `(col, row)` to the app, given the current terminal
/// `area`. Clicks in the sidebar focus it; clicks in the agent pane focus it.
/// Overlays absorb clicks (no-op) to keep modal handling simple. Pure state
/// transition: no terminal I/O, never panics on out-of-range clicks.
pub fn handle_mouse(app: &mut App, col: u16, row: u16, area: Rect) {
    if !matches!(app.overlay, Overlay::None) {
        return;
    }
    // A click on the agent surface focuses it and begins a text drag-selection
    // (issue #103); the anchor and cursor start on the same cell (a bare click
    // until a drag extends it).
    if let Some(cell) = agent_grid_cell(col, row, area) {
        app.focus = Focus::Agent;
        app.selection = Some(Selection {
            anchor: cell,
            cursor: cell,
        });
        return;
    }
    match hit_test(
        col,
        row,
        area,
        &app.config.repos,
        &app.worktrees,
        &app.worktree_repo,
        &app.expanded_repos,
        app.show_archived,
    ) {
        Hit::None => {}
        // The agent pane's border/tab strip (outside the selectable surface):
        // focus it, leaving no selection behind.
        Hit::Agent => {
            app.focus = Focus::Agent;
            app.selection = None;
        }
        // Clicking a repo header expands/collapses it (issue #82).
        Hit::Repo(i) => {
            app.focus = Focus::Sidebar;
            app.selection = None;
            app.toggle_repo(i);
        }
        // Clicking a worktree selects it AND activates its repo, so the
        // invariant (selected_worktree's repo == selected_repo) holds even when
        // the click lands in a different expanded repo.
        Hit::Worktree(i) => {
            app.focus = Focus::Sidebar;
            app.selection = None;
            if i < app.worktrees.len() {
                app.selected_worktree = Some(i);
                if let Some(&ri) = app.worktree_repo.get(i) {
                    app.selected_repo = Some(ri);
                }
            }
        }
    }
}

/// Maps a screen `(col, row)` to a `(grid_row, grid_col)` cell in the agent
/// surface, or `None` when the point falls on the sidebar, any border, the tab
/// strip, the status bar, or outside the surface. Mirrors `render_agent`'s
/// geometry exactly. Pure and total: never panics on out-of-range coordinates.
pub fn agent_grid_cell(col: u16, row: u16, area: Rect) -> Option<(u16, u16)> {
    let (rows, cols) = crate::ui::agent_pane_size(area);
    let x0 = SIDEBAR_WIDTH + 1;
    let y0 = 1 + TAB_BAR_HEIGHT;
    if col < x0 || row < y0 {
        return None;
    }
    let (gc, gr) = (col - x0, row - y0);
    if gc >= cols || gr >= rows {
        return None;
    }
    Some((gr, gc))
}

/// Extends the active drag-selection to the surface cell under `(col, row)`.
/// A no-op when no selection is active; a drag that leaves the surface (maps to
/// `None`) is ignored, clamping the cursor at its last in-bounds cell.
pub fn handle_mouse_drag(app: &mut App, col: u16, row: u16, area: Rect) {
    if let Some(sel) = app.selection.as_mut()
        && let Some(cell) = agent_grid_cell(col, row, area)
    {
        sel.cursor = cell;
    }
}

/// Completes a drag-selection on mouse-up. A real drag (`anchor != cursor`)
/// extracts the selected text from the active session's screen and stages an
/// OSC 52 copy in `app.pending_clipboard`, keeping the highlight until the next
/// keypress. A bare click (`anchor == cursor`) was only a focus click and clears
/// the selection without copying. Pure: no terminal I/O (the run loop writes).
pub fn handle_mouse_up(app: &mut App, _col: u16, _row: u16, _area: Rect) {
    let Some(sel) = app.selection else {
        return;
    };
    if sel.anchor == sel.cursor {
        app.selection = None;
        return;
    }
    let (start, end) = sel.normalized();
    let Some(name) = app.active_session.clone() else {
        app.selection = None;
        return;
    };
    let Some(session) = app.session_manager.get(&name) else {
        app.selection = None;
        return;
    };
    // Clamp both endpoints to the CURRENT screen size before extracting: the
    // anchor was captured at mouse-down and may be stale if the terminal (and its
    // vt100 screen) shrank mid-drag. `contents_between` does an unchecked
    // `cols - start_col`, so a start column past the width would panic (debug) or
    // yield garbage (release).
    let text = {
        let parser = session.parser().lock().unwrap();
        let screen = parser.screen();
        let (srows, scols) = screen.size();
        if srows == 0 || scols == 0 {
            String::new()
        } else {
            let cr = |(r, c): (u16, u16)| (r.min(srows - 1), c.min(scols - 1));
            let (s, e) = (cr(start), cr(end));
            let end_col = (e.1 + 1).min(scols); // end col exclusive; never exceeds width
            screen.contents_between(s.0, s.1, e.0, end_col)
        }
    };
    app.pending_clipboard = Some(osc52_copy(&text));
    // `app.selection` stays set: the reversed highlight persists until a keypress.
}

/// Wraps `text` in an OSC 52 "set clipboard" sequence (`ESC ] 52 ; c ; <b64> BEL`),
/// which sets the host terminal's clipboard even over SSH and in the alternate
/// screen. The payload is standard base64 of the raw UTF-8 bytes.
pub fn osc52_copy(text: &str) -> String {
    format!("\x1b]52;c;{}\x07", base64_encode(text.as_bytes()))
}

/// Standard RFC 4648 base64 (`+/` alphabet, `=` padding). Hand-rolled to keep the
/// dependency surface minimal for this single call site.
fn base64_encode(input: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0];
        let b1 = chunk.get(1).copied().unwrap_or(0);
        let b2 = chunk.get(2).copied().unwrap_or(0);
        let n = (u32::from(b0) << 16) | (u32::from(b1) << 8) | u32::from(b2);
        out.push(ALPHABET[((n >> 18) & 0x3f) as usize] as char);
        out.push(ALPHABET[((n >> 12) & 0x3f) as usize] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[((n >> 6) & 0x3f) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[(n & 0x3f) as usize] as char
        } else {
            '='
        });
    }
    out
}

/// Applies a wheel-scroll at column `col` to the app. Scrolling within the
/// sidebar columns moves the selection like `j`/`k`, reusing the same
/// [`Action::Next`]/[`Action::Prev`] navigation; scrolling over the agent pane
/// (or any column outside the sidebar) is inert. Overlays absorb scroll. Pure
/// state transition: no terminal I/O, never panics on out-of-range columns.
///
/// Pointing the wheel at the sidebar acts on the sidebar regardless of current
/// focus, mirroring how clicking a sidebar row focuses it: focus moves to the
/// sidebar first so navigation runs even when the agent pane holds focus.
pub fn handle_scroll(app: &mut App, up: bool, col: u16) {
    if !matches!(app.overlay, Overlay::None) {
        return;
    }
    if col >= SIDEBAR_WIDTH {
        // The scrollback view moves independently of the captured grid cells, so a
        // persisted highlight would point at the wrong content; clear it whether or
        // not a session is active.
        app.selection = None;
        // Wheel over the agent pane scrolls its terminal scrollback (#106);
        // focus is unchanged.
        if let Some(name) = app.active_session.clone()
            && let Some(session) = app.session_manager.get(&name)
        {
            const STEP: usize = 3;
            if up {
                session.scroll_up(STEP)
            } else {
                session.scroll_down(STEP)
            }
        }
        return;
    }
    app.focus = Focus::Sidebar;
    run_action(app, if up { Action::Prev } else { Action::Next });
}

fn handle_agent(app: &mut App, key: KeyEvent, ctrl: bool) {
    // Ctrl-O returns focus to the sidebar (not forwarded); every other key
    // falls through to the agent's PTY.
    if matches!(keymap::dispatch(AGENT, key), Some(Action::FocusSidebar)) {
        app.toggle_focus();
        return;
    }
    let Some(name) = app.active_session.clone() else {
        return;
    };
    let Some(session) = app.session_manager.get(&name) else {
        return;
    };

    // Typing follows the live agent: snap the scrollback view back to the
    // bottom on any forwarded keypress (#106).
    session.scroll_to_bottom();

    let shift = key.modifiers.contains(KeyModifiers::SHIFT);
    let Some(bytes) = agent_key_bytes(key.code, ctrl, shift) else {
        return;
    };
    let _ = session.write_input(&bytes);
}

/// Translates a key into the byte sequence forwarded to the agent's PTY.
/// Returns `None` for keys that are not forwarded. Pure: no I/O, no state.
///
/// Shift+Tab is emitted as CSI Z (`\x1b[Z`) whether crossterm reports it as
/// `BackTab` (legacy protocol) or `Tab` + SHIFT (Kitty keyboard protocol).
fn agent_key_bytes(code: KeyCode, ctrl: bool, shift: bool) -> Option<Vec<u8>> {
    let bytes = match code {
        KeyCode::Enter => vec![b'\r'],
        KeyCode::Backspace => vec![0x7f],
        KeyCode::BackTab => b"\x1b[Z".to_vec(),
        KeyCode::Tab if shift => b"\x1b[Z".to_vec(),
        KeyCode::Tab => vec![b'\t'],
        KeyCode::Esc => vec![0x1b],
        KeyCode::Left => b"\x1b[D".to_vec(),
        KeyCode::Right => b"\x1b[C".to_vec(),
        KeyCode::Up => b"\x1b[A".to_vec(),
        KeyCode::Down => b"\x1b[B".to_vec(),
        KeyCode::Home => b"\x1b[H".to_vec(),
        KeyCode::End => b"\x1b[F".to_vec(),
        KeyCode::PageUp => b"\x1b[5~".to_vec(),
        KeyCode::PageDown => b"\x1b[6~".to_vec(),
        KeyCode::Delete => b"\x1b[3~".to_vec(),
        KeyCode::Char(c) if ctrl && c.is_ascii_alphabetic() => {
            vec![(c.to_ascii_lowercase() as u8) & 0x1f]
        }
        KeyCode::Char(c) => c.to_string().into_bytes(),
        _ => return None,
    };
    Some(bytes)
}

fn handle_primary(app: &mut App, key: KeyEvent) {
    if let Some(action) = keymap::dispatch(PRIMARY, key) {
        run_action(app, action);
    }
}

fn handle_palette(app: &mut App, key: KeyEvent) {
    let Overlay::Palette { query, selected } = &mut app.overlay else {
        return;
    };
    match key.code {
        KeyCode::Esc => app.overlay = Overlay::None,
        KeyCode::Up => *selected = selected.saturating_sub(1),
        KeyCode::Down => {
            let count = palette::filter(query).len();
            if count > 0 {
                *selected = (*selected + 1).min(count - 1);
            }
        }
        KeyCode::Backspace => {
            query.pop();
            *selected = 0;
        }
        KeyCode::Char(c) => {
            query.push(c);
            *selected = 0;
        }
        KeyCode::Enter => {
            let matches = palette::filter(query);
            let chosen = matches.get(*selected).copied();
            app.overlay = Overlay::None;
            if let Some(action) = chosen {
                run_action(app, action);
            }
        }
        _ => {}
    }
}

fn handle_input(app: &mut App, key: KeyEvent) {
    let Overlay::Input { prompt, buffer } = &mut app.overlay else {
        return;
    };
    match key.code {
        KeyCode::Esc => app.overlay = Overlay::None,
        KeyCode::Backspace => {
            buffer.pop();
        }
        KeyCode::Char(c) => buffer.push(c),
        KeyCode::Enter => {
            let value = std::mem::take(buffer);
            let prompt = prompt.clone();
            app.overlay = Overlay::None;
            match prompt {
                Prompt::AddWorktree => app.add_worktree(&value),
                Prompt::AddRepo => app.register_repository(&value),
                Prompt::RenameBranch => app.rename_branch(&value),
                Prompt::SwitchAgent => match app.current_worktree().map(|w| w.branch.clone()) {
                    Some(branch) => app.set_worktree_agent(&branch, value.trim()),
                    None => app.status = Some("no worktree selected".to_string()),
                },
            }
        }
        _ => {}
    }
}

fn handle_confirm(app: &mut App, key: KeyEvent) {
    let Overlay::Confirm(confirm) = &app.overlay else {
        return;
    };
    match key.code {
        KeyCode::Char('y') | KeyCode::Char('Y') => {
            let confirm = confirm.clone();
            app.overlay = Overlay::None;
            match confirm {
                Confirm::RemoveWorktree(path) => app.remove_worktree(&path),
                Confirm::RemoveRepo(index) => app.remove_repository(index),
                Confirm::RestartAgent(branch) => app.restart_agent(&branch),
                Confirm::MergePr { branch, path } => app.pr_merge_branch(&branch, &path),
                Confirm::ClosePr { branch, path } => app.pr_close_branch(&branch, &path),
            }
        }
        KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => app.overlay = Overlay::None,
        _ => {}
    }
}

fn open_palette(app: &mut App) {
    app.overlay = Overlay::Palette {
        query: String::new(),
        selected: 0,
    };
}

fn open_add_prompt(app: &mut App) {
    if app.selected_repo_path().is_none() {
        app.status = Some("no repo selected".to_string());
        return;
    }
    app.overlay = Overlay::Input {
        prompt: Prompt::AddWorktree,
        buffer: String::new(),
    };
}

fn open_register_prompt(app: &mut App) {
    app.overlay = Overlay::Input {
        prompt: Prompt::AddRepo,
        buffer: String::new(),
    };
}

fn open_rename_prompt(app: &mut App) {
    match app.current_worktree() {
        Some(_) => {
            app.overlay = Overlay::Input {
                prompt: Prompt::RenameBranch,
                buffer: String::new(),
            }
        }
        None => app.status = Some("no worktree selected".to_string()),
    }
}

fn open_switch_agent_prompt(app: &mut App) {
    match app.current_worktree() {
        Some(_) => {
            app.overlay = Overlay::Input {
                prompt: Prompt::SwitchAgent,
                buffer: String::new(),
            }
        }
        None => app.status = Some("no worktree selected".to_string()),
    }
}

fn request_remove(app: &mut App) {
    match app.current_worktree() {
        Some(wt) => app.overlay = Overlay::Confirm(Confirm::RemoveWorktree(wt.path.clone())),
        None => app.status = Some("no worktree selected".to_string()),
    }
}

fn request_remove_repo(app: &mut App) {
    match app.selected_repo {
        Some(index) => app.overlay = Overlay::Confirm(Confirm::RemoveRepo(index)),
        None => app.status = Some("no repo selected".to_string()),
    }
}

fn request_restart_agent(app: &mut App) {
    match app.current_worktree() {
        Some(wt) => {
            let branch = wt.branch.clone();
            app.overlay = Overlay::Confirm(Confirm::RestartAgent(branch));
        }
        None => app.status = Some("no worktree selected".to_string()),
    }
}

fn request_merge_pr(app: &mut App) {
    match app.pr_target() {
        Ok((branch, path)) => app.overlay = Overlay::Confirm(Confirm::MergePr { branch, path }),
        Err(msg) => app.status = Some(msg),
    }
}

fn request_close_pr(app: &mut App) {
    match app.pr_target() {
        Ok((branch, path)) => app.overlay = Overlay::Confirm(Confirm::ClosePr { branch, path }),
        Err(msg) => app.status = Some(msg),
    }
}

/// Toggles the selected worktree's archived (soft-hidden) state: archives it if
/// currently visible, unarchives it if already archived. No-op with a status note
/// when no worktree is selected.
fn toggle_archive(app: &mut App) {
    let Some(path) = app.current_worktree().map(|w| w.path.clone()) else {
        app.status = Some("no worktree selected".to_string());
        return;
    };
    if sidebar::selected_archived(app).iter().any(|p| p == &path) {
        app.unarchive_worktree(&path);
    } else {
        app.archive_worktree(&path);
    }
    // Archiving the selected worktree while archived rows are hidden would
    // strand selection on an invisible row; move it to a visible neighbor.
    app.select_nearest_visible();
}

/// Executes a resolved [`Action`], whether it came from a key dispatch or the
/// command palette. The single place that maps semantic actions to app effects.
fn run_action(app: &mut App, action: Action) {
    match action {
        Action::Next => app.next(),
        Action::Prev => app.prev(),
        Action::ToggleFocus => app.toggle_focus(),
        Action::OpenPalette => open_palette(app),
        Action::Help => app.overlay = Overlay::Help,
        Action::FocusSidebar => app.toggle_focus(),
        Action::AddRepo => open_register_prompt(app),
        Action::RemoveRepo => request_remove_repo(app),
        Action::AddWorktree => open_add_prompt(app),
        Action::RemoveWorktree => request_remove(app),
        Action::RenameBranch => open_rename_prompt(app),
        Action::RestartAgent => request_restart_agent(app),
        Action::RunScript => app.start_run_script(),
        Action::JumpAttention => app.jump_to_attention(),
        Action::SwitchRepo => app.cycle_repo(),
        Action::ToggleRepo => app.toggle_selected_repo(),
        Action::SwitchAgent => open_switch_agent_prompt(app),
        Action::Refresh => app.refresh_worktrees(),
        Action::OpenPrWeb => app.pr_open_in_browser(),
        Action::MarkReady => app.pr_mark_ready(),
        Action::MergePr => request_merge_pr(app),
        Action::ClosePr => request_close_pr(app),
        Action::ToggleArchive => toggle_archive(app),
        Action::ShowArchived => app.show_archived = !app.show_archived,
        Action::NewTab => app.new_shell_tab(),
        Action::CloseTab => app.close_tab(),
        Action::NextTab => app.next_tab(),
        Action::PrevTab => app.prev_tab(),
        Action::Quit => app.should_quit = true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::repository::Repository;
    use crate::session::SessionManager;
    use crate::worktree::Worktree;
    use std::path::PathBuf;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn app() -> App {
        let mut app = App::new(Config {
            repos: vec![Repository {
                name: "demo".to_string(),
                path: PathBuf::from("/tmp/nope"),
                setup: None,
                archive: None,
                archived: Vec::new(),
                base_ref: None,
                copy_on_create: Vec::new(),
                run: None,
            }],
            agent_cmd: "claude".to_string(),
            notify: true,
            merge_strategy: crate::pr::MergeStrategy::default(),
            ..Default::default()
        });
        app.status = None;
        app.worktrees = vec![Worktree {
            path: PathBuf::from("/repo/main"),
            branch: "main".to_string(),
            head: "abc".to_string(),
            is_bare: false,
            is_detached: false,
        }];
        app.worktree_repo = vec![0];
        app.selected_worktree = Some(0);
        app
    }

    #[test]
    fn q_quits() {
        let mut a = app();
        handle_key(&mut a, key(KeyCode::Char('q')));
        assert!(a.should_quit);
    }

    #[test]
    fn ctrl_c_quits_even_in_overlay() {
        let mut a = app();
        a.overlay = Overlay::Palette {
            query: String::new(),
            selected: 0,
        };
        handle_key(
            &mut a,
            KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
        );
        assert!(a.should_quit);
    }

    #[test]
    fn colon_opens_palette_and_esc_closes() {
        let mut a = app();
        handle_key(&mut a, key(KeyCode::Char(':')));
        assert!(matches!(a.overlay, Overlay::Palette { .. }));
        handle_key(&mut a, key(KeyCode::Esc));
        assert_eq!(a.overlay, Overlay::None);
    }

    #[test]
    fn palette_quit_command_quits() {
        let mut a = app();
        handle_key(&mut a, key(KeyCode::Char(':')));
        for c in "quit".chars() {
            handle_key(&mut a, key(KeyCode::Char(c)));
        }
        handle_key(&mut a, key(KeyCode::Enter));
        assert!(a.should_quit);
    }

    #[test]
    fn n_opens_input_prompt() {
        let mut a = app();
        handle_key(&mut a, key(KeyCode::Char('n')));
        assert!(matches!(a.overlay, Overlay::Input { .. }));
    }

    #[test]
    fn a_opens_register_repo_prompt() {
        let mut a = app();
        handle_key(&mut a, key(KeyCode::Char('a')));
        assert!(matches!(
            a.overlay,
            Overlay::Input {
                prompt: Prompt::AddRepo,
                ..
            }
        ));
    }

    #[test]
    fn d_requests_confirm_when_worktree_selected() {
        let mut a = app();
        handle_key(&mut a, key(KeyCode::Char('d')));
        assert!(matches!(a.overlay, Overlay::Confirm(_)));
        handle_key(&mut a, key(KeyCode::Char('n')));
        assert_eq!(a.overlay, Overlay::None);
    }

    #[test]
    fn shift_d_requests_remove_repo_confirm() {
        let mut a = app();
        handle_key(&mut a, key(KeyCode::Char('D')));
        assert!(matches!(
            a.overlay,
            Overlay::Confirm(Confirm::RemoveRepo(0))
        ));
        handle_key(&mut a, key(KeyCode::Char('n')));
        assert_eq!(a.overlay, Overlay::None);
    }

    #[test]
    fn shift_r_requests_restart_agent_confirm() {
        let mut a = app();
        handle_key(&mut a, key(KeyCode::Char('R')));
        assert!(matches!(
            a.overlay,
            Overlay::Confirm(Confirm::RestartAgent(ref b)) if b == "main"
        ));
        handle_key(&mut a, key(KeyCode::Char('n')));
        assert_eq!(a.overlay, Overlay::None);
    }

    #[test]
    fn restart_agent_with_no_worktree_sets_status_and_no_confirm() {
        let mut a = app();
        a.worktrees.clear();
        a.selected_worktree = None;
        handle_key(&mut a, key(KeyCode::Char('R')));
        assert_eq!(a.overlay, Overlay::None);
        assert_eq!(a.status.as_deref(), Some("no worktree selected"));
    }

    #[test]
    fn confirming_restart_clears_active_session_and_sets_status() {
        let mut a = app();
        a.active_session = Some(SessionManager::session_name(&a.worktree_key(0, "main")));
        handle_key(&mut a, key(KeyCode::Char('R')));
        handle_key(&mut a, key(KeyCode::Char('y')));
        assert_eq!(a.overlay, Overlay::None);
        assert_eq!(a.active_session, None);
        assert_eq!(a.status.as_deref(), Some("restarting agent for main"));
    }

    #[test]
    fn tab_toggles_focus() {
        let mut a = app();
        let before = a.focus;
        handle_key(&mut a, key(KeyCode::Tab));
        assert_ne!(a.focus, before);
    }

    #[test]
    fn ctrl_q_quits_from_agent_focus() {
        let mut a = app();
        a.focus = crate::app::Focus::Agent;
        handle_key(
            &mut a,
            KeyEvent::new(KeyCode::Char('q'), KeyModifiers::CONTROL),
        );
        assert!(a.should_quit);
    }

    #[test]
    fn ctrl_c_in_agent_focus_does_not_quit() {
        let mut a = app();
        a.focus = crate::app::Focus::Agent;
        handle_key(
            &mut a,
            KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
        );
        assert!(!a.should_quit);
    }

    #[test]
    fn ctrl_o_returns_focus_to_sidebar() {
        let mut a = app();
        a.focus = crate::app::Focus::Agent;
        handle_key(
            &mut a,
            KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL),
        );
        assert_eq!(a.focus, crate::app::Focus::Sidebar);
    }

    #[test]
    fn question_mark_opens_help_from_sidebar() {
        let mut a = app();
        handle_key(&mut a, key(KeyCode::Char('?')));
        assert_eq!(a.overlay, Overlay::Help);
    }

    #[test]
    fn any_key_closes_help_without_moving_selection() {
        let mut a = app();
        a.worktrees.push(Worktree {
            path: PathBuf::from("/repo/feat"),
            branch: "feat".to_string(),
            head: "def".to_string(),
            is_bare: false,
            is_detached: false,
        });
        a.selected_worktree = Some(0);
        a.overlay = Overlay::Help;

        // `j` would normally move the selection, but Help swallows it.
        handle_key(&mut a, key(KeyCode::Char('j')));
        assert_eq!(a.overlay, Overlay::None);
        assert_eq!(a.selected_worktree, Some(0));
    }

    #[test]
    fn esc_closes_help() {
        let mut a = app();
        a.overlay = Overlay::Help;
        handle_key(&mut a, key(KeyCode::Esc));
        assert_eq!(a.overlay, Overlay::None);
    }

    #[test]
    fn g_jumps_to_the_next_flagged_worktree() {
        let mut a = app();
        a.worktrees.push(Worktree {
            path: PathBuf::from("/repo/feat"),
            branch: "feat".to_string(),
            head: "def".to_string(),
            is_bare: false,
            is_detached: false,
        });
        a.worktree_repo.push(0);
        a.selected_worktree = Some(0);
        let feat = SessionManager::session_name(&a.worktree_key(0, "feat"));
        a.attention
            .poll(&[(feat.clone(), std::time::Duration::ZERO)], None);
        a.attention
            .poll(&[(feat, crate::session::ATTENTION_QUIET)], None);

        handle_key(&mut a, key(KeyCode::Char('g')));
        assert_eq!(a.selected_worktree, Some(1));
    }

    #[test]
    fn g_is_noop_when_nothing_flagged() {
        let mut a = app();
        handle_key(&mut a, key(KeyCode::Char('g')));
        assert_eq!(a.selected_worktree, Some(0));
    }

    #[test]
    fn plain_char_in_agent_focus_is_noop_without_session() {
        let mut a = app();
        a.focus = crate::app::Focus::Agent;
        let before = a.focus;
        handle_key(&mut a, key(KeyCode::Char('x')));
        assert!(!a.should_quit);
        assert_eq!(a.focus, before);
    }

    // --- mouse / hit-test ---------------------------------------------------

    fn repos(n: usize) -> Vec<Repository> {
        (0..n)
            .map(|i| Repository {
                name: format!("repo{i}"),
                path: PathBuf::from(format!("/tmp/repo{i}")),
                setup: None,
                archive: None,
                archived: Vec::new(),
                base_ref: None,
                copy_on_create: Vec::new(),
                run: None,
            })
            .collect()
    }

    fn worktrees(n: usize) -> Vec<Worktree> {
        (0..n)
            .map(|i| Worktree {
                path: PathBuf::from(format!("/repo/wt{i}")),
                branch: format!("wt{i}"),
                head: "abc".to_string(),
                is_bare: false,
                is_detached: false,
            })
            .collect()
    }

    /// The expanded set holding only `repos(n)`'s first repo (`/tmp/repo0`), so
    /// the hit-test tests exercise the single-repo layout they were written for.
    fn expanded0() -> std::collections::HashSet<PathBuf> {
        [PathBuf::from("/tmp/repo0")].into_iter().collect()
    }

    /// Layout for one selected repo with two worktrees. Sidebar list rows
    /// (offset from the inner top, i.e. screen row 1+):
    ///   row 1 -> RepoHeader(0)
    ///   row 2 -> Worktree(0)
    ///   row 3 -> Worktree(1)
    fn area() -> Rect {
        Rect::new(0, 0, 80, 24)
    }

    #[test]
    fn hit_repo_header_row() {
        let hit = hit_test(
            2,
            1,
            area(),
            &repos(2),
            &worktrees(2),
            &[0, 0, 0],
            &expanded0(),
            false,
        );
        assert_eq!(hit, Hit::Repo(0));
    }

    #[test]
    fn hit_worktree_rows_under_selected_repo() {
        let r = repos(2);
        let w = worktrees(2);
        assert_eq!(
            hit_test(3, 2, area(), &r, &w, &[0, 0, 0], &expanded0(), false),
            Hit::Worktree(0)
        );
        assert_eq!(
            hit_test(3, 3, area(), &r, &w, &[0, 0, 0], &expanded0(), false),
            Hit::Worktree(1)
        );
    }

    #[test]
    fn second_repo_header_falls_after_first_repos_worktrees() {
        // rows: 1=Repo(0), 2=Worktree(0), 3=Worktree(1), 4=Repo(1)
        let hit = hit_test(
            2,
            4,
            area(),
            &repos(2),
            &worktrees(2),
            &[0, 0, 0],
            &expanded0(),
            false,
        );
        assert_eq!(hit, Hit::Repo(1));
    }

    #[test]
    fn hit_agent_region() {
        let hit = hit_test(
            SIDEBAR_WIDTH,
            5,
            area(),
            &repos(1),
            &worktrees(1),
            &[0, 0, 0],
            &expanded0(),
            false,
        );
        assert_eq!(hit, Hit::Agent);
    }

    #[test]
    fn hit_status_line_is_none() {
        // Last body row is height-1-STATUS_HEIGHT; the status line itself is row 23.
        let hit = hit_test(
            2,
            23,
            area(),
            &repos(1),
            &worktrees(1),
            &[0, 0, 0],
            &expanded0(),
            false,
        );
        assert_eq!(hit, Hit::None);
    }

    #[test]
    fn hit_blank_sidebar_space_is_none() {
        // Below the last list row (only Repo(0)+2 worktrees occupy rows 1..=3).
        let hit = hit_test(
            2,
            10,
            area(),
            &repos(1),
            &worktrees(2),
            &[0, 0, 0],
            &expanded0(),
            false,
        );
        assert_eq!(hit, Hit::None);
    }

    #[test]
    fn hit_top_and_left_border_is_none() {
        let r = repos(1);
        let w = worktrees(1);
        assert_eq!(
            hit_test(2, 0, area(), &r, &w, &[0, 0, 0], &expanded0(), false),
            Hit::None
        ); // top border
        assert_eq!(
            hit_test(0, 1, area(), &r, &w, &[0, 0, 0], &expanded0(), false),
            Hit::None
        ); // left border
    }

    #[test]
    fn hit_does_not_panic_on_out_of_range() {
        let hit = hit_test(
            1000,
            1000,
            area(),
            &repos(1),
            &worktrees(1),
            &[0, 0, 0],
            &expanded0(),
            false,
        );
        assert_eq!(hit, Hit::None);
    }

    #[test]
    fn handle_mouse_worktree_selects_and_focuses_sidebar() {
        let mut a = app();
        a.worktrees = worktrees(2);
        a.worktree_repo = vec![0, 0];
        a.selected_worktree = Some(0);
        a.focus = Focus::Agent;
        handle_mouse(&mut a, 3, 3, area()); // Worktree(1)
        assert_eq!(a.selected_worktree, Some(1));
        assert_eq!(a.focus, Focus::Sidebar);
    }

    /// issue #82: a second repo can be expanded alongside the first. Clicking a
    /// worktree that lives under a DIFFERENT expanded repo selects it AND makes
    /// that repo active, keeping the selected_worktree/selected_repo invariant.
    fn two_expanded_repos_app() -> App {
        let mut a = app(); // repo 0 = /tmp/nope, expanded
        a.config.repos.push(Repository {
            name: "second".to_string(),
            path: PathBuf::from("/tmp/nope2"),
            setup: None,
            archive: None,
            archived: Vec::new(),
            base_ref: None,
            copy_on_create: Vec::new(),
            run: None,
        });
        a.expanded_repos.insert(PathBuf::from("/tmp/nope2"));
        // Flat list spanning both repos: repo 0's worktree, then repo 1's.
        a.worktrees = vec![
            Worktree {
                path: PathBuf::from("/a/main"),
                branch: "am".to_string(),
                head: "h".to_string(),
                is_bare: false,
                is_detached: false,
            },
            Worktree {
                path: PathBuf::from("/b/main"),
                branch: "bm".to_string(),
                head: "h".to_string(),
                is_bare: false,
                is_detached: false,
            },
        ];
        a.worktree_repo = vec![0, 1];
        a.selected_repo = Some(0);
        a.selected_worktree = Some(0);
        a
    }

    #[test]
    fn click_worktree_in_another_expanded_repo_activates_its_repo() {
        let mut a = two_expanded_repos_app();
        a.focus = Focus::Agent;
        // Rows: 1=Header(0), 2=Worktree(0), 3=Header(1), 4=Worktree(1).
        handle_mouse(&mut a, 3, 4, area());
        assert_eq!(a.selected_worktree, Some(1));
        assert_eq!(
            a.selected_repo,
            Some(1),
            "clicking a worktree activates its own repo (keeps the invariant)"
        );
        assert_eq!(a.focus, Focus::Sidebar);
    }

    #[test]
    fn click_collapsed_repo_header_expands_it() {
        let mut a = app(); // repo 0 expanded, one worktree
        a.config.repos.push(Repository {
            name: "second".to_string(),
            path: PathBuf::from("/tmp/nope2"),
            setup: None,
            archive: None,
            archived: Vec::new(),
            base_ref: None,
            copy_on_create: Vec::new(),
            run: None,
        });
        a.worktree_repo = vec![0];
        assert!(!a.expanded_repos.contains(&PathBuf::from("/tmp/nope2")));

        // Rows: 1=Header(0), 2=Worktree(0), 3=Header(1, collapsed).
        handle_mouse(&mut a, 2, 3, area());

        assert!(
            a.expanded_repos.contains(&PathBuf::from("/tmp/nope2")),
            "clicking a collapsed repo header expands it"
        );
    }

    #[test]
    fn handle_mouse_agent_region_focuses_agent() {
        let mut a = app();
        a.focus = Focus::Sidebar;
        handle_mouse(&mut a, SIDEBAR_WIDTH + 2, 4, area());
        assert_eq!(a.focus, Focus::Agent);
    }

    #[test]
    fn handle_mouse_ignored_while_overlay_open() {
        let mut a = app();
        a.focus = Focus::Sidebar;
        a.overlay = Overlay::Help;
        handle_mouse(&mut a, SIDEBAR_WIDTH + 2, 4, area());
        assert_eq!(a.focus, Focus::Sidebar);
        assert_eq!(a.overlay, Overlay::Help);
    }

    // --- issue #45: wheel scroll moves sidebar selection -------------------

    #[test]
    fn scroll_down_in_sidebar_advances_selection() {
        let mut a = app();
        a.worktrees = worktrees(2);
        a.worktree_repo = vec![0, 0];
        a.selected_worktree = Some(0);
        handle_scroll(&mut a, false, 2);
        assert_eq!(a.selected_worktree, Some(1));
    }

    #[test]
    fn scroll_up_in_sidebar_moves_selection_back() {
        let mut a = app();
        a.worktrees = worktrees(2);
        a.worktree_repo = vec![0, 0];
        a.selected_worktree = Some(1);
        handle_scroll(&mut a, true, 2);
        assert_eq!(a.selected_worktree, Some(0));
    }

    // Covers only the NO-active-session case: scrolling over the agent pane must
    // not disturb the sidebar (selection/focus). The active-session wiring (the
    // scrollback move) is pinned by `scroll_over_agent_with_active_session_moves_scrollback`.
    #[test]
    fn scroll_in_agent_region_without_session_leaves_sidebar_untouched() {
        let mut a = app();
        a.worktrees = worktrees(2);
        a.worktree_repo = vec![0, 0];
        a.selected_worktree = Some(0);
        let before_focus = a.focus;
        handle_scroll(&mut a, false, SIDEBAR_WIDTH + 2);
        assert_eq!(a.selected_worktree, Some(0));
        assert_eq!(a.focus, before_focus);
    }

    #[test]
    fn scroll_over_sidebar_in_agent_focus_moves_selection() {
        // Design decision (issue #45): pointing the wheel at the sidebar moves
        // the selection even when the agent pane holds focus, like clicking a row.
        let mut a = app();
        a.worktrees = worktrees(2);
        a.worktree_repo = vec![0, 0];
        a.selected_worktree = Some(0);
        a.focus = Focus::Agent;
        handle_scroll(&mut a, false, 2);
        assert_eq!(a.selected_worktree, Some(1));
        assert_eq!(a.focus, Focus::Sidebar);
    }

    #[test]
    fn scroll_ignored_while_overlay_open() {
        let mut a = app();
        a.worktrees = worktrees(2);
        a.worktree_repo = vec![0, 0];
        a.selected_worktree = Some(0);
        a.overlay = Overlay::Help;
        handle_scroll(&mut a, false, 2);
        assert_eq!(a.selected_worktree, Some(0));
        assert_eq!(a.overlay, Overlay::Help);
    }

    #[test]
    fn scroll_does_not_panic_on_out_of_range_col() {
        let mut a = app();
        a.worktrees = worktrees(2);
        a.worktree_repo = vec![0, 0];
        a.selected_worktree = Some(0);
        handle_scroll(&mut a, false, 1000);
        handle_scroll(&mut a, true, 1000);
    }

    #[test]
    fn agent_key_bytes_backtab_is_csi_z() {
        assert_eq!(
            agent_key_bytes(KeyCode::BackTab, false, false),
            Some(b"\x1b[Z".to_vec())
        );
    }

    #[test]
    fn agent_key_bytes_shift_tab_is_csi_z() {
        assert_eq!(
            agent_key_bytes(KeyCode::Tab, false, true),
            Some(b"\x1b[Z".to_vec())
        );
    }

    #[test]
    fn agent_key_bytes_plain_tab_is_horizontal_tab() {
        assert_eq!(
            agent_key_bytes(KeyCode::Tab, false, false),
            Some(vec![b'\t'])
        );
    }

    #[test]
    fn agent_key_bytes_enter_is_carriage_return() {
        assert_eq!(
            agent_key_bytes(KeyCode::Enter, false, false),
            Some(vec![b'\r'])
        );
    }

    // --- issue #103: drag-select + OSC 52 copy ------------------------------

    #[test]
    fn base64_encode_rfc4648_vectors() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foob"), "Zm9vYg==");
        assert_eq!(base64_encode(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn osc52_copy_wraps_base64_in_the_clipboard_sequence() {
        assert_eq!(osc52_copy("foobar"), "\x1b]52;c;Zm9vYmFy\x07");
    }

    #[test]
    fn agent_grid_cell_maps_surface_points() {
        // area() is 80x24: x0 = 35, y0 = 2, surface = 20 rows x 44 cols.
        assert_eq!(agent_grid_cell(35, 2, area()), Some((0, 0)));
        assert_eq!(agent_grid_cell(40, 5, area()), Some((3, 5)));
    }

    #[test]
    fn agent_grid_cell_rejects_non_surface_points() {
        assert_eq!(agent_grid_cell(2, 5, area()), None); // sidebar
        assert_eq!(agent_grid_cell(34, 5, area()), None); // agent left border
        assert_eq!(agent_grid_cell(40, 1, area()), None); // tab strip row
        assert_eq!(agent_grid_cell(40, 23, area()), None); // status bar row
        assert_eq!(agent_grid_cell(1000, 1000, area()), None); // out of range
    }

    #[test]
    fn contents_between_end_col_is_exclusive_so_plus_one_includes_cursor_cell() {
        let mut parser = vt100::Parser::new(24, 80, 0);
        parser.process(b"hello\r\nworld");
        let screen = parser.screen();
        // Selecting grid (0,0)..=(0,4) is "hello": end_col 4 inclusive -> +1 = 5.
        assert_eq!(screen.contents_between(0, 0, 0, 5), "hello");
        // A multi-row selection ending at (1,4) inclusive -> +1 = 5 = "world".
        assert_eq!(screen.contents_between(0, 0, 1, 5), "hello\nworld");
    }

    #[test]
    fn click_without_drag_copies_nothing_and_clears_selection() {
        let mut a = app();
        handle_mouse(&mut a, 35, 2, area()); // Down on surface cell (0,0)
        assert!(a.selection.is_some());
        handle_mouse_up(&mut a, 35, 2, area());
        assert_eq!(a.pending_clipboard, None);
        assert_eq!(a.selection, None);
    }

    #[test]
    fn drag_then_up_stages_osc52_copy_and_keeps_selection() {
        use portable_pty::CommandBuilder;

        let mut a = app();
        let name = "wtcc-copy-test";
        // `true` produces no PTY output, so the parser holds exactly the content
        // we feed it below (no race with the reader thread).
        let cmd = CommandBuilder::new("true");
        a.session_manager
            .insert_spawned(name, cmd, &std::env::temp_dir(), 24, 80)
            .unwrap();
        a.active_session = Some(name.to_string());
        a.session_manager
            .get(name)
            .unwrap()
            .parser()
            .lock()
            .unwrap()
            .process(b"hello\r\nworld");

        // Down (0,0)=screen(35,2), drag to (0,4)=screen(39,2), release.
        handle_mouse(&mut a, 35, 2, area());
        handle_mouse_drag(&mut a, 39, 2, area());
        handle_mouse_up(&mut a, 39, 2, area());

        assert_eq!(a.pending_clipboard, Some(osc52_copy("hello")));
        assert!(
            a.selection.is_some(),
            "highlight persists until next keypress"
        );
    }

    #[test]
    fn drag_outside_surface_clamps_cursor_to_last_in_bounds_cell() {
        let mut a = app();
        handle_mouse(&mut a, 35, 2, area()); // anchor (0,0)
        handle_mouse_drag(&mut a, 39, 2, area()); // cursor (0,4)
        handle_mouse_drag(&mut a, 2, 2, area()); // into the sidebar -> ignored
        let sel = a.selection.unwrap();
        assert_eq!(sel.cursor, (0, 4));
    }

    /// BLOCKER regression: the anchor is captured at mouse-down and goes stale if
    /// the vt100 screen shrinks mid-drag (terminal resize). `handle_mouse_up` must
    /// clamp both endpoints to the current screen before `contents_between`, which
    /// otherwise does an unchecked `cols - start_col` (panic in debug).
    #[test]
    fn mouse_up_clamps_stale_selection_after_screen_shrinks() {
        use portable_pty::CommandBuilder;

        let mut a = app();
        let name = "wtcc-clamp-test";
        let mut cmd = CommandBuilder::new("sh");
        cmd.args(["-c", "seq 1 60"]);
        a.session_manager
            .insert_spawned(name, cmd, &std::env::temp_dir(), 24, 80)
            .unwrap();
        a.active_session = Some(name.to_string());

        // Anchor near the wide right edge, cursor a row below: both fall outside a
        // 10-column screen once it shrinks.
        a.selection = Some(Selection {
            anchor: (0, 70),
            cursor: (1, 5),
        });
        a.session_manager.get(name).unwrap().resize(5, 10).unwrap();

        handle_mouse_up(&mut a, 0, 0, area());
        assert!(
            a.pending_clipboard.is_some(),
            "a clamped selection still stages a copy instead of panicking"
        );
    }

    /// ISSUE #103 fix: a real drag that ends with no active session must clear the
    /// lingering highlight (and copy nothing), not leave it over the placeholder.
    #[test]
    fn mouse_up_with_no_active_session_clears_selection_without_copy() {
        let mut a = app();
        a.active_session = None;
        a.selection = Some(Selection {
            anchor: (0, 0),
            cursor: (1, 3),
        });
        handle_mouse_up(&mut a, 0, 0, area());
        assert_eq!(a.selection, None);
        assert_eq!(a.pending_clipboard, None);
    }

    /// Wheel-scroll over the agent pane clears any highlight (the scrollback view
    /// moves independently), even with no active session.
    #[test]
    fn scroll_over_agent_clears_selection() {
        let mut a = app();
        a.selection = Some(Selection {
            anchor: (0, 0),
            cursor: (1, 2),
        });
        handle_scroll(&mut a, true, SIDEBAR_WIDTH + 2);
        assert_eq!(a.selection, None);
    }

    /// Pins the agent-pane scroll wiring with a live session: wheel up scrolls into
    /// history (scrollback grows), wheel down moves back toward the live bottom.
    #[test]
    fn scroll_over_agent_with_active_session_moves_scrollback() {
        use portable_pty::CommandBuilder;

        let mut a = app();
        let name = "wtcc-scroll-wire-test";
        let mut cmd = CommandBuilder::new("sh");
        cmd.args(["-c", "seq 1 60"]);
        a.session_manager
            .insert_spawned(name, cmd, &std::env::temp_dir(), 5, 20)
            .unwrap();
        a.active_session = Some(name.to_string());

        let scrollback = |a: &App| {
            a.session_manager
                .get(name)
                .unwrap()
                .parser()
                .lock()
                .unwrap()
                .screen()
                .scrollback()
        };
        // Wait until output exceeds the 5-row screen so history exists to scroll into.
        for _ in 0..40 {
            let has_history = a
                .session_manager
                .get(name)
                .unwrap()
                .parser()
                .lock()
                .unwrap()
                .screen()
                .contents()
                .contains("60");
            if has_history {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }

        assert_eq!(scrollback(&a), 0, "starts at the live bottom");
        handle_scroll(&mut a, true, SIDEBAR_WIDTH + 2); // wheel up
        let up = scrollback(&a);
        assert!(up > 0, "wheel up scrolls into history");
        handle_scroll(&mut a, false, SIDEBAR_WIDTH + 2); // wheel down
        assert!(
            scrollback(&a) < up,
            "wheel down scrolls back toward the live bottom"
        );
    }

    #[test]
    fn keypress_clears_active_selection() {
        let mut a = app();
        a.selection = Some(Selection {
            anchor: (0, 0),
            cursor: (1, 2),
        });
        handle_key(&mut a, key(KeyCode::Char('j')));
        assert_eq!(a.selection, None);
    }
}
