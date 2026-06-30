//! TDD RED contract for issue #43: a single, data-driven keymap table that is
//! the one source of truth for key dispatch, the help overlay, the statusbar
//! hints, and the command palette — so those four places can no longer drift.
//!
//! These tests pin the target public API of `wtcc::keymap` and the derivation
//! of the palette from it. They are expected to FAIL TO COMPILE until
//! `src/keymap.rs` exists and `palette::filter` returns `keymap::Action`.
//! No production code is written here.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use wtcc::keymap::{self, AGENT, Action, Binding, PRIMARY};
use wtcc::ui::palette;

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn ctrl(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
}

// --- dispatch: every existing binding present with its current action --------

#[test]
fn dispatch_primary_preserves_every_existing_binding() {
    assert_eq!(
        keymap::dispatch(PRIMARY, key(KeyCode::Char(':'))),
        Some(Action::OpenPalette)
    );
    assert_eq!(
        keymap::dispatch(PRIMARY, ctrl('p')),
        Some(Action::OpenPalette)
    );
    assert_eq!(
        keymap::dispatch(PRIMARY, key(KeyCode::Char('q'))),
        Some(Action::Quit)
    );
    assert_eq!(keymap::dispatch(PRIMARY, ctrl('q')), Some(Action::Quit));
    assert_eq!(
        keymap::dispatch(PRIMARY, key(KeyCode::Char('j'))),
        Some(Action::Next)
    );
    assert_eq!(
        keymap::dispatch(PRIMARY, key(KeyCode::Down)),
        Some(Action::Next)
    );
    assert_eq!(
        keymap::dispatch(PRIMARY, key(KeyCode::Char('k'))),
        Some(Action::Prev)
    );
    assert_eq!(
        keymap::dispatch(PRIMARY, key(KeyCode::Up)),
        Some(Action::Prev)
    );
    assert_eq!(
        keymap::dispatch(PRIMARY, key(KeyCode::Tab)),
        Some(Action::ToggleFocus)
    );
    assert_eq!(
        keymap::dispatch(PRIMARY, key(KeyCode::Char('a'))),
        Some(Action::AddRepo)
    );
    assert_eq!(
        keymap::dispatch(PRIMARY, key(KeyCode::Char('D'))),
        Some(Action::RemoveRepo)
    );
    assert_eq!(
        keymap::dispatch(PRIMARY, key(KeyCode::Char('n'))),
        Some(Action::AddWorktree)
    );
    assert_eq!(
        keymap::dispatch(PRIMARY, key(KeyCode::Char('d'))),
        Some(Action::RemoveWorktree)
    );
    assert_eq!(
        keymap::dispatch(PRIMARY, key(KeyCode::Char('R'))),
        Some(Action::RestartAgent)
    );
    assert_eq!(
        keymap::dispatch(PRIMARY, key(KeyCode::Char('r'))),
        Some(Action::Refresh)
    );
    assert_eq!(
        keymap::dispatch(PRIMARY, key(KeyCode::Char('?'))),
        Some(Action::Help)
    );
}

#[test]
fn dispatch_primary_unbound_key_is_none() {
    assert_eq!(keymap::dispatch(PRIMARY, key(KeyCode::Char('z'))), None);
}

#[test]
fn dispatch_agent_reserves_only_ctrl_o_and_ctrl_q() {
    assert_eq!(
        keymap::dispatch(AGENT, ctrl('o')),
        Some(Action::FocusSidebar)
    );
    assert_eq!(keymap::dispatch(AGENT, ctrl('q')), Some(Action::Quit));
    // Plain printable keys are NOT reserved in agent focus — they forward to the
    // PTY, so dispatch must report no binding for them.
    assert_eq!(keymap::dispatch(AGENT, key(KeyCode::Char('x'))), None);
}

// --- no two bindings in a context may share a chord -------------------------

fn assert_no_chord_collisions(bindings: &[Binding]) {
    let chords: Vec<_> = bindings
        .iter()
        .flat_map(|b| b.chords.iter().copied())
        .collect();
    for i in 0..chords.len() {
        for j in (i + 1)..chords.len() {
            assert_ne!(
                chords[i], chords[j],
                "duplicate chord within a single focus map"
            );
        }
    }
}

#[test]
fn no_duplicate_chords_in_primary() {
    assert_no_chord_collisions(PRIMARY);
}

#[test]
fn no_duplicate_chords_in_agent() {
    assert_no_chord_collisions(AGENT);
}

// --- every binding is reachable by at least one chord -----------------------

#[test]
fn every_binding_has_at_least_one_chord() {
    for b in PRIMARY.iter().chain(AGENT.iter()) {
        assert!(
            !b.chords.is_empty(),
            "binding for {:?} has no chord and is unreachable",
            b.action
        );
    }
}

// --- help_rows: one row per binding, multi-chord aliases grouped as "a/b" ----

#[test]
fn help_rows_emit_one_row_per_binding() {
    assert_eq!(keymap::help_rows(PRIMARY).len(), PRIMARY.len());
    assert_eq!(keymap::help_rows(AGENT).len(), AGENT.len());
}

#[test]
fn help_rows_group_multi_chord_aliases_with_slash() {
    let rows = keymap::help_rows(PRIMARY);
    assert!(
        rows.contains(&(":/Ctrl-P".to_string(), Action::OpenPalette.label())),
        "expected grouped palette row, got {rows:?}"
    );
    assert!(
        rows.contains(&("q/Ctrl-Q".to_string(), Action::Quit.label())),
        "expected grouped quit row, got {rows:?}"
    );
}

#[test]
fn help_rows_render_single_chord_bindings_plainly() {
    let rows = keymap::help_rows(PRIMARY);
    assert!(
        rows.contains(&("?".to_string(), Action::Help.label())),
        "expected single-chord help row, got {rows:?}"
    );
}

// --- palette is DERIVED from the keymap: in_palette() filter -----------------

#[test]
fn in_palette_excludes_pure_nav_and_modal_actions() {
    assert!(!Action::Next.in_palette());
    assert!(!Action::Prev.in_palette());
    assert!(!Action::ToggleFocus.in_palette());
    assert!(!Action::OpenPalette.in_palette());
    assert!(!Action::Help.in_palette());
    assert!(!Action::FocusSidebar.in_palette());
}

#[test]
fn in_palette_includes_every_command_action() {
    for a in [
        Action::AddRepo,
        Action::RemoveRepo,
        Action::AddWorktree,
        Action::RemoveWorktree,
        Action::RestartAgent,
        Action::SwitchRepo,
        Action::Refresh,
        Action::Quit,
    ] {
        assert!(a.in_palette(), "{a:?} should be offered in the palette");
    }
}

#[test]
fn palette_filter_empty_yields_exactly_the_palette_actions() {
    let all = palette::filter("");
    for a in [
        Action::AddRepo,
        Action::RemoveRepo,
        Action::AddWorktree,
        Action::RemoveWorktree,
        Action::RestartAgent,
        Action::SwitchRepo,
        Action::Refresh,
        Action::Quit,
    ] {
        assert!(all.contains(&a), "palette must contain {a:?}");
    }
    for a in [
        Action::Next,
        Action::Prev,
        Action::ToggleFocus,
        Action::OpenPalette,
        Action::Help,
        Action::FocusSidebar,
    ] {
        assert!(!all.contains(&a), "palette must not contain {a:?}");
    }
}

#[test]
fn palette_filter_ranks_worktree_query_first() {
    assert_eq!(
        palette::filter("worktree").first(),
        Some(&Action::AddWorktree)
    );
}

#[test]
fn palette_filter_nonsense_query_is_empty() {
    assert!(palette::filter("zzzzqqqq").is_empty());
}

// --- the palette label and the help label are the SAME string ---------------

#[test]
fn palette_and_help_share_one_label_per_action() {
    // Any action shown in the palette must carry the identical label() that the
    // help overlay derives from `help_rows`, so the two cannot drift.
    let help: Vec<&'static str> = keymap::help_rows(PRIMARY)
        .into_iter()
        .map(|(_, label)| label)
        .collect();
    for a in palette::filter("") {
        if matches!(a, Action::SwitchRepo) {
            // SwitchRepo has no PRIMARY chord (palette-only), so it is absent
            // from the PRIMARY help rows by design — skip the help cross-check.
            continue;
        }
        assert!(
            help.contains(&a.label()),
            "palette action {a:?} label {:?} not found among help labels",
            a.label()
        );
    }
}

// --- issue #47: jump-to-attention key + palette command ----------------------

#[test]
fn dispatch_primary_binds_g_to_jump_attention() {
    assert_eq!(
        keymap::dispatch(PRIMARY, key(KeyCode::Char('g'))),
        Some(Action::JumpAttention)
    );
}

#[test]
fn jump_attention_is_offered_in_the_palette() {
    assert!(Action::JumpAttention.in_palette());
}

#[test]
fn palette_filter_includes_jump_attention() {
    assert!(
        palette::filter("").contains(&Action::JumpAttention),
        "JumpAttention must appear in the palette command list"
    );
}
