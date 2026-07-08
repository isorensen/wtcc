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
    RunScript,
    JumpAttention,
    SwitchRepo,
    ToggleRepo,
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
    // Dump the active tab's scrollback to `$PAGER`/`$EDITOR` in a new tab (#124).
    DumpScrollback,
    DumpScrollbackEditor,
    // Scrollback navigation (#122). All pure modal/navigation actions —
    // `in_palette` is false for every one, so they are reachable only by key.
    ScrollMode,
    ScrollUp,
    ScrollDown,
    ScrollPageUp,
    ScrollPageDown,
    ScrollTop,
    ScrollBottom,
    ScrollExit,
    Quit,
}

impl Action {
    /// Every action, in palette declaration order. Palette ordering for the
    /// commands matches this list (filtered by [`Action::in_palette`]).
    pub const ALL: [Action; 39] = [
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
        Action::RunScript,
        Action::JumpAttention,
        Action::SwitchRepo,
        Action::ToggleRepo,
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
        Action::DumpScrollback,
        Action::DumpScrollbackEditor,
        Action::ScrollMode,
        Action::ScrollUp,
        Action::ScrollDown,
        Action::ScrollPageUp,
        Action::ScrollPageDown,
        Action::ScrollTop,
        Action::ScrollBottom,
        Action::ScrollExit,
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
            Action::RunScript => "Run script",
            Action::JumpAttention => "Jump to attention",
            Action::SwitchRepo => "Switch repo",
            Action::ToggleRepo => "Expand/collapse repo",
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
            Action::DumpScrollback => "Dump scrollback to pager",
            Action::DumpScrollbackEditor => "Dump scrollback to editor",
            Action::ScrollMode => "scroll mode",
            Action::ScrollUp => "scroll up",
            Action::ScrollDown => "scroll down",
            Action::ScrollPageUp => "page up",
            Action::ScrollPageDown => "page down",
            Action::ScrollTop => "scroll to top",
            Action::ScrollBottom => "scroll to bottom",
            Action::ScrollExit => "exit scroll",
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
                | Action::RunScript
                | Action::JumpAttention
                | Action::SwitchRepo
                | Action::ToggleRepo
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
                | Action::DumpScrollback
                | Action::DumpScrollbackEditor
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
        // A character key already encodes Shift in its value (Shift+D => 'D'),
        // and terminals speaking the Kitty keyboard protocol (e.g. ghostty)
        // additionally report a redundant SHIFT modifier. Ignore SHIFT for Char
        // codes so the uppercase bindings (D, R, A, X) fire regardless of which
        // keyboard protocol the terminal negotiates. Non-Char keys (Tab, arrows)
        // keep exact modifier matching.
        let key_mods = match key.code {
            KeyCode::Char(_) => key.modifiers.difference(KeyModifiers::SHIFT),
            _ => key.modifiers,
        };
        self.code == key.code && self.mods == key_mods
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
    // Run script (issue #56). Sidebar-focus only: in agent focus every printable
    // key forwards to the PTY, so `s` lives here and in the palette.
    Binding {
        chords: &[Chord::key(KeyCode::Char('s'))],
        action: Action::RunScript,
    },
    Binding {
        chords: &[Chord::key(KeyCode::Char('A'))],
        action: Action::SwitchAgent,
    },
    // Switch to the next repo (#108). `s` is RunScript; uppercase `S` is free and
    // `Chord::matches` keeps the two distinct.
    Binding {
        chords: &[Chord::key(KeyCode::Char('S'))],
        action: Action::SwitchRepo,
    },
    Binding {
        chords: &[Chord::key(KeyCode::Char('g'))],
        action: Action::JumpAttention,
    },
    // Expand/collapse the selected repo's worktrees (issue #82). Space and Enter
    // are both free in sidebar focus.
    Binding {
        chords: &[Chord::key(KeyCode::Char(' ')), Chord::key(KeyCode::Enter)],
        action: Action::ToggleRepo,
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
    // Dump the active tab's scrollback to `$PAGER` in a new tab (#124). `P` is
    // free; `Chord::matches` ignores SHIFT for Char, so uppercase `P` fires. The
    // editor variant is palette-only.
    Binding {
        chords: &[Chord::key(KeyCode::Char('P'))],
        action: Action::DumpScrollback,
    },
];

/// Agent-focus keymap. Only these chords are reserved; every other key is
/// forwarded to the agent's PTY.
pub static AGENT: &[Binding] = &[
    Binding {
        chords: &[Chord::ctrl('o')],
        action: Action::FocusSidebar,
    },
    // Enter modal scrollback navigation (#122). Two ergonomic entry chords that a
    // full-screen agent never needs: Ctrl-↑ and Shift-PageUp. `Chord::matches`
    // exact-matches modifiers for these non-Char keys.
    Binding {
        chords: &[
            Chord {
                code: KeyCode::Up,
                mods: KeyModifiers::CONTROL,
            },
            Chord {
                code: KeyCode::PageUp,
                mods: KeyModifiers::SHIFT,
            },
        ],
        action: Action::ScrollMode,
    },
    Binding {
        chords: &[Chord::ctrl('q')],
        action: Action::Quit,
    },
];

/// Scroll-mode keymap (#122). Active only while the agent pane is in
/// [`crate::app::TermMode::Scroll`]; every key here drives the vt100 scrollback
/// view instead of the agent, and any unbound key is a modal no-op.
pub static SCROLL: &[Binding] = &[
    Binding {
        chords: &[Chord::key(KeyCode::Char('k')), Chord::key(KeyCode::Up)],
        action: Action::ScrollUp,
    },
    Binding {
        chords: &[Chord::key(KeyCode::Char('j')), Chord::key(KeyCode::Down)],
        action: Action::ScrollDown,
    },
    Binding {
        chords: &[Chord::ctrl('u'), Chord::key(KeyCode::PageUp)],
        action: Action::ScrollPageUp,
    },
    Binding {
        chords: &[Chord::ctrl('d'), Chord::key(KeyCode::PageDown)],
        action: Action::ScrollPageDown,
    },
    Binding {
        chords: &[Chord::key(KeyCode::Char('g')), Chord::key(KeyCode::Home)],
        action: Action::ScrollTop,
    },
    Binding {
        chords: &[Chord::key(KeyCode::Char('G')), Chord::key(KeyCode::End)],
        action: Action::ScrollBottom,
    },
    Binding {
        chords: &[
            Chord::key(KeyCode::Esc),
            Chord::key(KeyCode::Char('q')),
            Chord::ctrl('c'),
        ],
        action: Action::ScrollExit,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn ev(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, mods)
    }

    // --- issue #90: uppercase bindings must fire under the Kitty keyboard -----
    // protocol, where Shift+<letter> arrives as the uppercase char WITH a
    // redundant SHIFT modifier. `Chord::matches` ignores SHIFT for Char codes.

    #[test]
    fn uppercase_bindings_fire_with_a_redundant_shift_modifier() {
        // ghostty/Kitty: Shift+D => Char('D') + SHIFT. Each uppercase binding
        // must still resolve to its action.
        for (c, action) in [
            ('D', Action::RemoveRepo),
            ('R', Action::RestartAgent),
            ('A', Action::SwitchAgent),
            ('X', Action::ShowArchived),
        ] {
            assert_eq!(
                dispatch(PRIMARY, ev(KeyCode::Char(c), KeyModifiers::SHIFT)),
                Some(action),
                "Shift+{c} must fire {action:?} even with a redundant SHIFT modifier"
            );
        }
    }

    #[test]
    fn uppercase_bindings_still_fire_without_a_shift_modifier() {
        // Legacy terminals report the bare uppercase char (no SHIFT); unchanged.
        assert_eq!(
            dispatch(PRIMARY, ev(KeyCode::Char('D'), KeyModifiers::NONE)),
            Some(Action::RemoveRepo)
        );
    }

    #[test]
    fn lowercase_and_uppercase_stay_distinct() {
        // Ignoring SHIFT must not collapse case: 'd' and 'D' are different keys.
        assert_eq!(
            dispatch(PRIMARY, ev(KeyCode::Char('d'), KeyModifiers::NONE)),
            Some(Action::RemoveWorktree)
        );
        assert_eq!(
            dispatch(PRIMARY, ev(KeyCode::Char('D'), KeyModifiers::SHIFT)),
            Some(Action::RemoveRepo)
        );
    }

    #[test]
    fn switch_repo_binds_to_uppercase_s_distinct_from_run_script() {
        // #108: `S` switches repo; `s` stays RunScript (case must not collapse).
        assert_eq!(
            dispatch(PRIMARY, ev(KeyCode::Char('S'), KeyModifiers::NONE)),
            Some(Action::SwitchRepo)
        );
        assert_eq!(
            dispatch(PRIMARY, ev(KeyCode::Char('S'), KeyModifiers::SHIFT)),
            Some(Action::SwitchRepo),
            "Shift+S must still fire under the Kitty protocol"
        );
        assert_eq!(
            dispatch(PRIMARY, ev(KeyCode::Char('s'), KeyModifiers::NONE)),
            Some(Action::RunScript)
        );
    }

    // --- issue #122: scrollback keyboard navigation -------------------------

    #[test]
    fn agent_entry_chords_open_scroll_mode() {
        assert_eq!(
            dispatch(AGENT, ev(KeyCode::Up, KeyModifiers::CONTROL)),
            Some(Action::ScrollMode),
            "Ctrl-↑ enters scroll mode"
        );
        assert_eq!(
            dispatch(AGENT, ev(KeyCode::PageUp, KeyModifiers::SHIFT)),
            Some(Action::ScrollMode),
            "Shift-PageUp enters scroll mode"
        );
    }

    #[test]
    fn scroll_bindings_each_resolve() {
        for (code, mods, action) in [
            (KeyCode::Char('k'), KeyModifiers::NONE, Action::ScrollUp),
            (KeyCode::Up, KeyModifiers::NONE, Action::ScrollUp),
            (KeyCode::Char('j'), KeyModifiers::NONE, Action::ScrollDown),
            (KeyCode::Down, KeyModifiers::NONE, Action::ScrollDown),
            (
                KeyCode::Char('u'),
                KeyModifiers::CONTROL,
                Action::ScrollPageUp,
            ),
            (KeyCode::PageUp, KeyModifiers::NONE, Action::ScrollPageUp),
            (
                KeyCode::Char('d'),
                KeyModifiers::CONTROL,
                Action::ScrollPageDown,
            ),
            (
                KeyCode::PageDown,
                KeyModifiers::NONE,
                Action::ScrollPageDown,
            ),
            (KeyCode::Char('g'), KeyModifiers::NONE, Action::ScrollTop),
            (KeyCode::Home, KeyModifiers::NONE, Action::ScrollTop),
            (KeyCode::Char('G'), KeyModifiers::NONE, Action::ScrollBottom),
            (KeyCode::End, KeyModifiers::NONE, Action::ScrollBottom),
            (KeyCode::Esc, KeyModifiers::NONE, Action::ScrollExit),
            (KeyCode::Char('q'), KeyModifiers::NONE, Action::ScrollExit),
            (
                KeyCode::Char('c'),
                KeyModifiers::CONTROL,
                Action::ScrollExit,
            ),
        ] {
            assert_eq!(
                dispatch(SCROLL, ev(code, mods)),
                Some(action),
                "{code:?}+{mods:?} must resolve to {action:?}"
            );
        }
    }

    #[test]
    fn scroll_actions_are_never_offered_in_the_palette() {
        for action in [
            Action::ScrollMode,
            Action::ScrollUp,
            Action::ScrollDown,
            Action::ScrollPageUp,
            Action::ScrollPageDown,
            Action::ScrollTop,
            Action::ScrollBottom,
            Action::ScrollExit,
        ] {
            assert!(
                !action.in_palette(),
                "{action:?} must stay out of the palette"
            );
        }
    }

    #[test]
    fn ctrl_bindings_still_require_ctrl() {
        // Ctrl+P opens the palette; a bare 'p' (with a stray SHIFT) must not.
        assert_eq!(
            dispatch(PRIMARY, ev(KeyCode::Char('p'), KeyModifiers::CONTROL)),
            Some(Action::OpenPalette)
        );
        assert_ne!(
            dispatch(PRIMARY, ev(KeyCode::Char('p'), KeyModifiers::SHIFT)),
            Some(Action::OpenPalette)
        );
    }
}
