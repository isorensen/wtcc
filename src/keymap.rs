//! The single source of truth for wtcc's keybindings.
//!
//! A keymap is a flat table of [`Binding`]s, one per focus context ([`PRIMARY`]
//! for the sidebar, [`AGENT`] for the agent pane). Key dispatch, the `?` help
//! overlay, and the command palette all DERIVE from this table, so they can no
//! longer drift apart. This is intentionally a static data table, not a
//! user-configurable keybinding engine — per-user remapping is out of scope.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// A semantic command the UI can perform. Some actions are pure navigation or
/// modal toggles ([`Action::in_palette`] is `false`); the rest are offered in
/// the command palette.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Next,
    Prev,
    ToggleFocus,
    OpenPalette,
    Help,
    FocusSidebar,
    AddRepo,
    RemoveRepo,
    AddWorktree,
    RemoveWorktree,
    RenameBranch,
    RestartAgent,
    JumpAttention,
    SwitchRepo,
    SwitchAgent,
    Refresh,
    OpenPrWeb,
    MarkReady,
    MergePr,
    ClosePr,
    ToggleArchive,
    ShowArchived,
    NewTab,
    CloseTab,
    NextTab,
    PrevTab,
    Quit,
}

impl Action {
    /// Every action, in palette declaration order. Palette ordering for the
    /// commands matches this list (filtered by [`Action::in_palette`]).
    pub const ALL: [Action; 27] = [
        Action::Next,
        Action::Prev,
        Action::ToggleFocus,
        Action::OpenPalette,
        Action::Help,
        Action::FocusSidebar,
        Action::AddRepo,
        Action::RemoveRepo,
        Action::AddWorktree,
        Action::RemoveWorktree,
        Action::RenameBranch,
        Action::RestartAgent,
        Action::JumpAttention,
        Action::SwitchRepo,
        Action::SwitchAgent,
        Action::Refresh,
        Action::OpenPrWeb,
        Action::MarkReady,
        Action::MergePr,
        Action::ClosePr,
        Action::ToggleArchive,
        Action::ShowArchived,
        Action::NewTab,
        Action::CloseTab,
        Action::NextTab,
        Action::PrevTab,
        Action::Quit,
    ];

    /// The one human-readable label for this action, shared by the help overlay
    /// and the command palette so the two cannot describe the same action
    /// differently.
    pub fn label(self) -> &'static str {
        match self {
            Action::Next => "move down",
            Action::Prev => "move up",
            Action::ToggleFocus => "toggle focus",
            Action::OpenPalette => "command palette",
            Action::Help => "help",
            Action::FocusSidebar => "back to sidebar",
            Action::AddRepo => "Add repository",
            Action::RemoveRepo => "Remove repository",
            Action::AddWorktree => "Add worktree",
            Action::RemoveWorktree => "Remove worktree",
            Action::RenameBranch => "Rename branch",
            Action::RestartAgent => "Restart agent",
            Action::JumpAttention => "Jump to attention",
            Action::SwitchRepo => "Switch repo",
            Action::SwitchAgent => "Switch agent",
            Action::Refresh => "Refresh",
            Action::OpenPrWeb => "Open PR in browser",
            Action::MarkReady => "Mark PR ready",
            Action::MergePr => "Merge PR",
            Action::ClosePr => "Close PR",
            Action::ToggleArchive => "Archive/unarchive worktree",
            Action::ShowArchived => "Show/hide archived worktrees",
            Action::NewTab => "New tab",
            Action::CloseTab => "Close tab",
            Action::NextTab => "Next tab",
            Action::PrevTab => "Previous tab",
            Action::Quit => "Quit",
        }
    }

    /// Whether this action is offered in the command palette. Pure navigation
    /// and modal actions are reachable only by their keys, never the palette.
    pub fn in_palette(self) -> bool {
        matches!(
            self,
            Action::AddRepo
                | Action::RemoveRepo
                | Action::AddWorktree
                | Action::RemoveWorktree
                | Action::RenameBranch
                | Action::RestartAgent
                | Action::JumpAttention
                | Action::SwitchRepo
                | Action::SwitchAgent
                | Action::Refresh
                | Action::OpenPrWeb
                | Action::MarkReady
                | Action::MergePr
                | Action::ClosePr
                | Action::ToggleArchive
                | Action::ShowArchived
                | Action::NewTab
                | Action::CloseTab
                | Action::NextTab
                | Action::PrevTab
                | Action::Quit
        )
    }
}

/// A single key chord (a key plus its modifiers).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Chord {
    pub code: KeyCode,
    pub mods: KeyModifiers,
}

impl Chord {
    const fn key(code: KeyCode) -> Self {
        Self {
            code,
            mods: KeyModifiers::NONE,
        }
    }

    const fn ctrl(c: char) -> Self {
        Self {
            code: KeyCode::Char(c),
            mods: KeyModifiers::CONTROL,
        }
    }

    fn matches(self, key: KeyEvent) -> bool {
        self.code == key.code && self.mods == key.modifiers
    }

    /// Renders the chord for the help overlay, e.g. `"j"`, `"Tab"`, `"Ctrl-P"`.
    fn render(self) -> String {
        let base = match self.code {
            KeyCode::Char(c) => c.to_string(),
            KeyCode::Tab => "Tab".to_string(),
            KeyCode::Enter => "Enter".to_string(),
            KeyCode::Esc => "Esc".to_string(),
            KeyCode::Up => "Up".to_string(),
            KeyCode::Down => "Down".to_string(),
            KeyCode::Left => "Left".to_string(),
            KeyCode::Right => "Right".to_string(),
            other => format!("{other:?}"),
        };
        if self.mods.contains(KeyModifiers::CONTROL) {
            format!("Ctrl-{}", base.to_ascii_uppercase())
        } else {
            base
        }
    }
}

/// A binding maps one or more interchangeable chords to a single [`Action`].
pub struct Binding {
    pub chords: &'static [Chord],
    pub action: Action,
}

/// Sidebar-focus keymap.
pub static PRIMARY: &[Binding] = &[
    Binding {
        chords: &[Chord::key(KeyCode::Char('j')), Chord::key(KeyCode::Down)],
        action: Action::Next,
    },
    Binding {
        chords: &[Chord::key(KeyCode::Char('k')), Chord::key(KeyCode::Up)],
        action: Action::Prev,
    },
    Binding {
        chords: &[Chord::key(KeyCode::Tab)],
        action: Action::ToggleFocus,
    },
    Binding {
        chords: &[Chord::key(KeyCode::Char('a'))],
        action: Action::AddRepo,
    },
    Binding {
        chords: &[Chord::key(KeyCode::Char('D'))],
        action: Action::RemoveRepo,
    },
    Binding {
        chords: &[Chord::key(KeyCode::Char('n'))],
        action: Action::AddWorktree,
    },
    Binding {
        chords: &[Chord::key(KeyCode::Char('d'))],
        action: Action::RemoveWorktree,
    },
    Binding {
        chords: &[Chord::key(KeyCode::Char('b'))],
        action: Action::RenameBranch,
    },
    Binding {
        chords: &[Chord::key(KeyCode::Char('R'))],
        action: Action::RestartAgent,
    },
    Binding {
        chords: &[Chord::key(KeyCode::Char('A'))],
        action: Action::SwitchAgent,
    },
    Binding {
        chords: &[Chord::key(KeyCode::Char('g'))],
        action: Action::JumpAttention,
    },
    Binding {
        chords: &[Chord::key(KeyCode::Char('r'))],
        action: Action::Refresh,
    },
    Binding {
        chords: &[Chord::key(KeyCode::Char('o'))],
        action: Action::OpenPrWeb,
    },
    Binding {
        chords: &[Chord::key(KeyCode::Char('m'))],
        action: Action::MergePr,
    },
    Binding {
        chords: &[Chord::key(KeyCode::Char('x'))],
        action: Action::ToggleArchive,
    },
    Binding {
        chords: &[Chord::key(KeyCode::Char('X'))],
        action: Action::ShowArchived,
    },
    Binding {
        chords: &[Chord::key(KeyCode::Char(':')), Chord::ctrl('p')],
        action: Action::OpenPalette,
    },
    Binding {
        chords: &[Chord::key(KeyCode::Char('?'))],
        action: Action::Help,
    },
    Binding {
        chords: &[Chord::key(KeyCode::Char('q')), Chord::ctrl('q')],
        action: Action::Quit,
    },
    // Tab management (issue #48). Sidebar-focus only: in agent focus every
    // printable key forwards to the PTY, so these live here and in the palette.
    Binding {
        chords: &[Chord::key(KeyCode::Char('t'))],
        action: Action::NewTab,
    },
    Binding {
        chords: &[Chord::key(KeyCode::Char('w'))],
        action: Action::CloseTab,
    },
    Binding {
        chords: &[Chord::key(KeyCode::Char(']'))],
        action: Action::NextTab,
    },
    Binding {
        chords: &[Chord::key(KeyCode::Char('['))],
        action: Action::PrevTab,
    },
];

/// Agent-focus keymap. Only these chords are reserved; every other key is
/// forwarded to the agent's PTY.
pub static AGENT: &[Binding] = &[
    Binding {
        chords: &[Chord::ctrl('o')],
        action: Action::FocusSidebar,
    },
    Binding {
        chords: &[Chord::ctrl('q')],
        action: Action::Quit,
    },
];

/// Resolves a key event against a keymap, returning the bound action if any.
pub fn dispatch(map: &[Binding], key: KeyEvent) -> Option<Action> {
    map.iter()
        .find(|b| b.chords.iter().any(|c| c.matches(key)))
        .map(|b| b.action)
}

/// One `(keys, label)` row per binding for the help overlay. Multiple chords for
/// the same action are joined with `/` (e.g. `":/Ctrl-P"`).
pub fn help_rows(map: &[Binding]) -> Vec<(String, &'static str)> {
    map.iter()
        .map(|b| {
            let keys = b
                .chords
                .iter()
                .map(|c| c.render())
                .collect::<Vec<_>>()
                .join("/");
            (keys, b.action.label())
        })
        .collect()
}
