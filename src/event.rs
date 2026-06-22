use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::{App, Confirm, Focus, Overlay, Prompt};
use crate::ui::palette::{self, Command};

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

    match &app.overlay {
        Overlay::None => {
            if app.focus == Focus::Agent {
                handle_agent(app, key, ctrl);
            } else {
                handle_primary(app, key, ctrl);
            }
        }
        Overlay::Palette { .. } => handle_palette(app, key),
        Overlay::Input { .. } => handle_input(app, key),
        Overlay::Confirm(_) => handle_confirm(app, key),
    }
}

fn handle_agent(app: &mut App, key: KeyEvent, ctrl: bool) {
    // Ctrl-O returns focus to the sidebar (not forwarded).
    if ctrl && matches!(key.code, KeyCode::Char('o')) {
        app.toggle_focus();
        return;
    }
    let Some(name) = app.active_session.clone() else {
        return;
    };
    let Some(session) = app.session_manager.get(&name) else {
        return;
    };

    let bytes: Vec<u8> = match key.code {
        KeyCode::Enter => vec![b'\r'],
        KeyCode::Backspace => vec![0x7f],
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
        _ => return,
    };
    let _ = session.write_input(&bytes);
}

fn handle_primary(app: &mut App, key: KeyEvent, ctrl: bool) {
    if ctrl && matches!(key.code, KeyCode::Char('p')) {
        open_palette(app);
        return;
    }

    match key.code {
        KeyCode::Char('q') => app.should_quit = true,
        KeyCode::Char(':') => open_palette(app),
        KeyCode::Char('j') | KeyCode::Down => app.next(),
        KeyCode::Char('k') | KeyCode::Up => app.prev(),
        KeyCode::Tab => app.toggle_focus(),
        KeyCode::Char('r') => app.refresh_worktrees(),
        KeyCode::Char('n') => open_add_prompt(app),
        KeyCode::Char('d') => request_remove(app),
        _ => {}
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
            if let Some(cmd) = chosen {
                run_command(app, cmd);
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

fn request_remove(app: &mut App) {
    match app.current_worktree() {
        Some(wt) => app.overlay = Overlay::Confirm(Confirm::RemoveWorktree(wt.path.clone())),
        None => app.status = Some("no worktree selected".to_string()),
    }
}

fn run_command(app: &mut App, cmd: Command) {
    match cmd {
        Command::AddWorktree => open_add_prompt(app),
        Command::RemoveWorktree => request_remove(app),
        Command::SwitchRepo => app.cycle_repo(),
        Command::Refresh => app.refresh_worktrees(),
        Command::Quit => app.should_quit = true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::repository::Repository;
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
            }],
            agent_cmd: "claude".to_string(),
        });
        app.status = None;
        app.worktrees = vec![Worktree {
            path: PathBuf::from("/repo/main"),
            branch: "main".to_string(),
            head: "abc".to_string(),
            is_bare: false,
            is_detached: false,
        }];
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
    fn d_requests_confirm_when_worktree_selected() {
        let mut a = app();
        handle_key(&mut a, key(KeyCode::Char('d')));
        assert!(matches!(a.overlay, Overlay::Confirm(_)));
        handle_key(&mut a, key(KeyCode::Char('n')));
        assert_eq!(a.overlay, Overlay::None);
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
    fn plain_char_in_agent_focus_is_noop_without_session() {
        let mut a = app();
        a.focus = crate::app::Focus::Agent;
        let before = a.focus;
        handle_key(&mut a, key(KeyCode::Char('x')));
        assert!(!a.should_quit);
        assert_eq!(a.focus, before);
    }
}
