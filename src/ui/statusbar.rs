use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::widgets::{Paragraph, Widget};

use crate::app::{App, Focus};

const SIDEBAR_HINTS: &str =
    "j/k move  Tab agent  n/d worktree  a/D repo  R restart  r refresh  : palette  ? help  q quit";
const AGENT_HINTS: &str = "keys go to the agent  Ctrl-O back to sidebar  Ctrl-Q quit";

pub fn render(app: &App, area: Rect, buf: &mut Buffer) {
    let theme = app.theme;
    let attention = app.attention_count();
    let line = match &app.status {
        Some(status) => Line::styled(status.clone(), Style::default().fg(theme.status)),
        None if attention > 0 => Line::styled(
            format!("{attention} agent(s) need input — press g to jump"),
            Style::default().fg(theme.attention),
        ),
        None => {
            let hints = match app.focus {
                Focus::Sidebar => SIDEBAR_HINTS,
                Focus::Agent => AGENT_HINTS,
            };
            Line::styled(hints, Style::default().fg(theme.hint))
        }
    };
    Paragraph::new(line).render(area, buf);
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    use crate::keymap::{self, PRIMARY};

    /// Maps a status-line key token (`"j"`, `"Tab"`, `"Ctrl-P"`) back to the
    /// `KeyEvent` it advertises.
    fn token_to_key(token: &str) -> KeyEvent {
        if let Some(rest) = token.strip_prefix("Ctrl-") {
            let c = rest.chars().next().unwrap().to_ascii_lowercase();
            return KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL);
        }
        match token {
            "Tab" => KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE),
            _ => KeyEvent::new(
                KeyCode::Char(token.chars().next().unwrap()),
                KeyModifiers::NONE,
            ),
        }
    }

    /// Anti-drift: every key token teased in the sidebar status line must map to
    /// a real PRIMARY binding, so the hint can never advertise a dead key after
    /// a keymap change.
    #[test]
    fn every_sidebar_hint_token_maps_to_a_primary_binding() {
        for segment in super::SIDEBAR_HINTS.split("  ") {
            let Some(keys) = segment.split_whitespace().next() else {
                continue;
            };
            for token in keys.split('/') {
                let ev = token_to_key(token);
                assert!(
                    keymap::dispatch(PRIMARY, ev).is_some(),
                    "sidebar hint token {token:?} has no PRIMARY binding"
                );
            }
        }
    }

    // --- issue #47: aggregated attention count ------------------------------

    use crate::app::App;
    use crate::config::Config;
    use crate::repository::Repository;
    use crate::session::{ATTENTION_QUIET, SessionManager};
    use crate::worktree::Worktree;
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
    use std::path::PathBuf;

    fn app_with_flagged(branches: &[&str]) -> App {
        let mut app = App::new(Config {
            repos: vec![Repository {
                name: "r".to_string(),
                path: PathBuf::from("/tmp/wtcc-statusbar-attn-none"),
                setup: None,
                archive: None,
            }],
            agent_cmd: "claude".to_string(),
            notify: true,
            merge_strategy: crate::pr::MergeStrategy::default(),
        });
        app.status = None;
        app.selected_repo = Some(0);
        app.worktrees = branches
            .iter()
            .map(|b| Worktree {
                path: PathBuf::from(format!("/r/{b}")),
                branch: b.to_string(),
                head: "h".to_string(),
                is_bare: false,
                is_detached: false,
            })
            .collect();
        // No active selection so the flags persist.
        app.selected_worktree = None;

        let names: Vec<String> = branches
            .iter()
            .map(|b| SessionManager::session_name(b))
            .collect();
        let busy: Vec<(String, std::time::Duration)> = names
            .iter()
            .map(|n| (n.clone(), std::time::Duration::ZERO))
            .collect();
        let quiet: Vec<(String, std::time::Duration)> =
            names.iter().map(|n| (n.clone(), ATTENTION_QUIET)).collect();
        app.attention.poll(&busy, None);
        app.attention.poll(&quiet, None);
        app
    }

    fn rendered(app: &App) -> String {
        let area = Rect::new(0, 0, 80, 1);
        let mut buf = Buffer::empty(area);
        super::render(app, area, &mut buf);
        buf.content().iter().map(|c| c.symbol()).collect()
    }

    #[test]
    fn statusbar_shows_attention_count_when_positive() {
        let app = app_with_flagged(&["feat", "fix"]);
        assert_eq!(app.attention_count(), 2);
        let text = rendered(&app);
        assert!(
            text.contains('2'),
            "expected the attention count, got {text:?}"
        );
        assert!(
            text.contains("need input"),
            "expected the aggregated attention message, got {text:?}"
        );
    }

    #[test]
    fn statusbar_hides_attention_message_when_count_is_zero() {
        let app = app_with_flagged(&[]);
        assert_eq!(app.attention_count(), 0);
        let text = rendered(&app);
        assert!(
            !text.contains("need input"),
            "attention message must be hidden at zero, got {text:?}"
        );
    }
}
