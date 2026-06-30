//! The single source of truth for wtcc's UI colors.
//!
//! Deliberately NOT a theming engine: there is no TOML `[theme]` section, no
//! color parser, and no user-facing config of any kind (all cut as
//! over-engineering). It is a plain `Copy` struct of named `ratatui` colors,
//! resolved once at startup via `Theme::default()` and passed by value into
//! rendering. Every `Color::` literal that used to be scattered across `ui`
//! lives here under a named role.

use ratatui::style::Color;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Theme {
    /// Accent for repo headers, selection, and help keys.
    pub accent: Color,
    /// Reversed-selection highlight color for the active worktree row.
    pub selection: Color,
    /// Border of an unfocused pane.
    pub border: Color,
    /// Border of the focused pane — the focus-emphasis cue.
    pub border_focus: Color,
    /// Activity glyph for an agent that is actively working.
    pub activity_working: Color,
    /// Activity glyph for an idle agent.
    pub activity_idle: Color,
    /// The "needs input" attention marker (and aggregated statusbar message).
    pub attention: Color,
    /// Status-line message color.
    pub status: Color,
    /// Dim keybinding hints in the status bar.
    pub hint: Color,
    /// Dirty working-tree marker.
    pub dirty: Color,
    /// PR badge: open PR with passing checks (or merged).
    pub pr_ok: Color,
    /// PR badge: failing checks or a closed PR.
    pub pr_bad: Color,
    /// PR badge: pending checks.
    pub pr_pending: Color,
}

impl Default for Theme {
    fn default() -> Self {
        Theme {
            accent: Color::Cyan,
            selection: Color::Cyan,
            border: Color::DarkGray,
            border_focus: Color::Cyan,
            activity_working: Color::Green,
            activity_idle: Color::DarkGray,
            attention: Color::Magenta,
            status: Color::Yellow,
            hint: Color::DarkGray,
            dirty: Color::Yellow,
            pr_ok: Color::Green,
            pr_bad: Color::Red,
            pr_pending: Color::Yellow,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn focus_border_is_distinct_from_unfocused() {
        let t = Theme::default();
        assert_ne!(t.border_focus, t.border);
    }

    #[test]
    fn attention_stands_out_from_idle_and_hint() {
        let t = Theme::default();
        assert_ne!(t.attention, t.activity_idle);
        assert_ne!(t.attention, t.hint);
    }
}
