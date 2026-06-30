use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::Line;
use ratatui::widgets::{Paragraph, Widget};

use crate::app::{App, Focus};

const SIDEBAR_HINTS: &str =
    "j/k move  Tab agent  n/d worktree  a/D repo  R restart  r refresh  : palette  ? help  q quit";
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
}
