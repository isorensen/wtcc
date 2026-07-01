//! TDD RED contract for issue #48: per-worktree TABS — multiple terminal
//! surfaces in one worktree (NO split panes; explicitly out of scope).
//!
//! Each worktree owns an ordered list of tabs with an active index. Tab 0 is the
//! AGENT (`wtcc-<slug>`, unchanged reattach behavior); additional tabs are SHELL
//! surfaces (`wtcc-<slug>-t<n>`, monotonic `n`). One tab is visible/routed at a
//! time; the list is restored on worktree switch.
//!
//! This file pins the issue's acceptance criteria and the contract in its
//! Technical notes for the parts reachable through the public crate API:
//!   - the PURE `wtcc::layout` tab model,
//!   - the data-driven keymap registration (`t`/`w`/`]`/`[` + Next/Prev/New/Close
//!     tab Actions, palette membership, no chord collisions),
//!   - input routing through `handle_key` in SIDEBAR focus (and the decision that
//!     tab keys are NOT reserved in AGENT focus, so they forward to the PTY).
//!
//! No production code is written here. It is expected to FAIL TO COMPILE until
//! `src/layout.rs` exists, `App` gains the tab API, and the keymap registers the
//! new Actions. No real tmux is touched: tab-model mutation is in-memory and the
//! repo path is non-existent so no real git runs.

use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use wtcc::app::{App, Focus};
use wtcc::config::Config;
use wtcc::event::handle_key;
use wtcc::keymap::{self, AGENT, Action, Chord, PRIMARY};
use wtcc::layout::{TabKind, WorktreeLayout};
use wtcc::repository::Repository;
use wtcc::session::SessionManager;
use wtcc::ui::palette;
use wtcc::worktree::Worktree;

// --- helpers ----------------------------------------------------------------

fn key(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
}

fn worktrees(branches: &[&str]) -> Vec<Worktree> {
    branches
        .iter()
        .enumerate()
        .map(|(i, b)| Worktree {
            path: PathBuf::from(format!("/repo/{b}")),
            branch: (*b).to_string(),
            head: format!("h{i}"),
            is_bare: false,
            is_detached: false,
        })
        .collect()
}

/// An App with one repo (non-existent path so no real git runs) and the given
/// branches injected as worktrees, selection on the first, status cleared.
fn app_with(branches: &[&str]) -> App {
    let cfg = Config {
        repos: vec![Repository {
            name: "demo".to_string(),
            path: PathBuf::from("/tmp/wtcc-issue48-does-not-exist"),
            ..Default::default()
        }],
        ..Default::default()
    };
    let mut app = App::new(cfg);
    let wts = worktrees(branches);
    app.worktree_repo = vec![0; wts.len()];
    app.worktrees = wts;
    app.selected_worktree = Some(0);
    app.status = None;
    app
}

// --- layout: new() seeds exactly one Agent tab ------------------------------

#[test]
fn new_seeds_a_single_agent_tab() {
    let layout = WorktreeLayout::new("feat-x");
    assert_eq!(
        layout.tabs.len(),
        1,
        "a worktree starts with exactly one tab"
    );
    assert_eq!(layout.active, 0);
    assert_eq!(layout.next_id, 1, "the next shell id starts at 1");
    let agent = &layout.tabs[0];
    assert_eq!(agent.kind, TabKind::Agent);
    assert_eq!(
        agent.session, "wtcc-feat-x",
        "the agent tab reattaches to the existing wtcc-<slug> session"
    );
    assert_eq!(agent.title, "agent");
}

// --- layout: add_shell_tab appends `-t<n>` (monotonic) and focuses ----------

#[test]
fn add_shell_tab_appends_monotonic_session_names_and_focuses() {
    let mut layout = WorktreeLayout::new("feat-x");

    layout.add_shell_tab("feat-x");
    assert_eq!(layout.tabs.len(), 2);
    assert_eq!(layout.active, 1, "the new shell tab is focused");
    assert_eq!(layout.tabs[1].kind, TabKind::Shell);
    assert_eq!(layout.tabs[1].session, "wtcc-feat-x-t1");
    assert_eq!(layout.tabs[1].title, "shell 1");
    assert_eq!(layout.next_id, 2);

    layout.add_shell_tab("feat-x");
    assert_eq!(layout.tabs.len(), 3);
    assert_eq!(layout.active, 2);
    assert_eq!(layout.tabs[2].session, "wtcc-feat-x-t2");
    assert_eq!(layout.tabs[2].title, "shell 2");
    assert_eq!(layout.next_id, 3);
}

// --- layout: close_active is guarded for the agent tab / the last tab -------

#[test]
fn close_active_refuses_the_agent_tab() {
    let mut layout = WorktreeLayout::new("feat-x");
    layout.add_shell_tab("feat-x");
    layout.prev_tab(); // back to the agent (index 0)
    assert_eq!(layout.active, 0);

    assert_eq!(
        layout.close_active(),
        None,
        "the agent tab (index 0) is never closable"
    );
    assert_eq!(layout.tabs.len(), 2, "nothing is removed when refused");
}

#[test]
fn close_active_refuses_the_only_tab() {
    let mut layout = WorktreeLayout::new("feat-x");
    assert_eq!(
        layout.close_active(),
        None,
        "the last remaining tab cannot be closed"
    );
    assert_eq!(layout.tabs.len(), 1);
}

// --- layout: close_active removes the shell, RETURNS its name, refocuses -----

#[test]
fn close_active_removes_shell_returns_session_and_adjusts_active() {
    let mut layout = WorktreeLayout::new("feat-x");
    layout.add_shell_tab("feat-x"); // active 1 -> wtcc-feat-x-t1

    let killed = layout.close_active();
    assert_eq!(
        killed,
        Some("wtcc-feat-x-t1".to_string()),
        "close_active returns the removed session name for the caller to kill"
    );
    assert_eq!(layout.tabs.len(), 1);
    assert_eq!(
        layout.active, 0,
        "active falls back into range after removal"
    );
    assert_eq!(layout.tabs[0].kind, TabKind::Agent);
}

#[test]
fn close_active_in_the_middle_keeps_a_following_tab_active() {
    let mut layout = WorktreeLayout::new("feat-x");
    layout.add_shell_tab("feat-x"); // t1 (active 1)
    layout.add_shell_tab("feat-x"); // t2 (active 2)
    layout.prev_tab(); // active 1 -> t1

    let killed = layout.close_active();
    assert_eq!(killed, Some("wtcc-feat-x-t1".to_string()));
    assert_eq!(layout.tabs.len(), 2);
    assert_eq!(layout.active, 1);
    assert_eq!(
        layout.active_tab().session,
        "wtcc-feat-x-t2",
        "removing the active middle tab keeps the following tab focused"
    );
}

#[test]
fn next_id_stays_monotonic_after_a_close() {
    let mut layout = WorktreeLayout::new("feat-x");
    layout.add_shell_tab("feat-x"); // t1
    layout.add_shell_tab("feat-x"); // t2
    layout.close_active(); // removes t2; next_id must NOT be reused

    layout.add_shell_tab("feat-x");
    assert_eq!(
        layout.active_tab().session,
        "wtcc-feat-x-t3",
        "ids are monotonic so a reopened tab never collides with a live session"
    );
}

// --- layout: next/prev wrap -------------------------------------------------

#[test]
fn next_and_prev_are_noops_with_a_single_tab() {
    let mut layout = WorktreeLayout::new("feat-x");
    layout.next_tab();
    assert_eq!(layout.active, 0);
    layout.prev_tab();
    assert_eq!(layout.active, 0);
}

#[test]
fn next_and_prev_wrap_across_three_tabs() {
    let mut layout = WorktreeLayout::new("feat-x");
    layout.add_shell_tab("feat-x"); // active 1
    layout.add_shell_tab("feat-x"); // active 2

    layout.next_tab();
    assert_eq!(
        layout.active, 0,
        "next wraps from the last tab to the first"
    );
    layout.next_tab();
    assert_eq!(layout.active, 1);

    layout.prev_tab();
    assert_eq!(layout.active, 0);
    layout.prev_tab();
    assert_eq!(
        layout.active, 2,
        "prev wraps from the first tab to the last"
    );
}

#[test]
fn active_tab_reflects_the_active_index() {
    let mut layout = WorktreeLayout::new("feat-x");
    layout.add_shell_tab("feat-x");
    assert_eq!(layout.active_tab().session, "wtcc-feat-x-t1");
    layout.prev_tab();
    assert_eq!(layout.active_tab().kind, TabKind::Agent);
}

// --- keymap: data-driven registration of the tab Actions --------------------

#[test]
fn primary_binds_the_tab_management_chords() {
    assert_eq!(keymap::dispatch(PRIMARY, key('t')), Some(Action::NewTab));
    assert_eq!(keymap::dispatch(PRIMARY, key('w')), Some(Action::CloseTab));
    assert_eq!(keymap::dispatch(PRIMARY, key(']')), Some(Action::NextTab));
    assert_eq!(keymap::dispatch(PRIMARY, key('[')), Some(Action::PrevTab));
}

#[test]
fn tab_keys_are_not_reserved_in_agent_focus() {
    // In agent focus every printable key forwards to the PTY, so tab controls
    // must live in SIDEBAR focus only — the AGENT map reserves none of them.
    assert_eq!(keymap::dispatch(AGENT, key('t')), None);
    assert_eq!(keymap::dispatch(AGENT, key('w')), None);
    assert_eq!(keymap::dispatch(AGENT, key(']')), None);
    assert_eq!(keymap::dispatch(AGENT, key('[')), None);
}

#[test]
fn tab_actions_are_offered_in_the_palette_with_labels() {
    for a in [
        Action::NewTab,
        Action::CloseTab,
        Action::NextTab,
        Action::PrevTab,
    ] {
        assert!(a.in_palette(), "{a:?} should be offered in the palette");
        assert!(!a.label().is_empty(), "{a:?} must carry a human label");
        assert!(
            palette::filter("").contains(&a),
            "palette must contain {a:?}"
        );
    }
}

#[test]
fn registering_tab_keys_introduces_no_chord_collisions_in_primary() {
    let chords: Vec<Chord> = PRIMARY
        .iter()
        .flat_map(|b| b.chords.iter().copied())
        .collect();
    for i in 0..chords.len() {
        for j in (i + 1)..chords.len() {
            assert_ne!(
                chords[i], chords[j],
                "duplicate chord within the PRIMARY keymap"
            );
        }
    }
}

// --- event: tab keys mutate the current layout in SIDEBAR focus -------------

#[test]
fn t_in_sidebar_focus_adds_a_shell_tab_to_the_current_worktree() {
    let mut app = app_with(&["main"]);
    assert!(!app.layouts.contains_key(&app.worktree_key(0, "main")));

    handle_key(&mut app, key('t'));

    let layout = app
        .layouts
        .get(&app.worktree_key(0, "main"))
        .expect("t must create the layout and add a shell tab");
    assert_eq!(layout.tabs.len(), 2);
    assert_eq!(layout.active, 1, "the new shell tab is focused");
}

#[test]
fn bracket_keys_cycle_the_active_tab_in_sidebar_focus() {
    let mut app = app_with(&["main"]);
    app.new_shell_tab(); // [agent, shell] active 1

    handle_key(&mut app, key(']')); // next, wrap to 0
    assert_eq!(
        app.layouts
            .get(&app.worktree_key(0, "main"))
            .unwrap()
            .active,
        0
    );

    handle_key(&mut app, key('[')); // prev, wrap to 1
    assert_eq!(
        app.layouts
            .get(&app.worktree_key(0, "main"))
            .unwrap()
            .active,
        1
    );
}

#[test]
fn w_in_sidebar_focus_closes_the_active_shell_tab() {
    let mut app = app_with(&["main"]);
    app.new_shell_tab(); // [agent, shell] active 1

    handle_key(&mut app, key('w'));

    let layout = app.layouts.get(&app.worktree_key(0, "main")).unwrap();
    assert_eq!(layout.tabs.len(), 1, "w closes the active shell tab");
    assert_eq!(layout.active, 0);
}

#[test]
fn tab_keys_in_agent_focus_do_not_manage_tabs() {
    // Consistency guard: with the agent focused, `t` forwards to the PTY and must
    // NOT add a tab (no layout is created for the worktree).
    let mut app = app_with(&["main"]);
    app.focus = Focus::Agent;

    handle_key(&mut app, key('t'));

    assert!(
        !app.layouts.contains_key(&app.worktree_key(0, "main")),
        "tab management keys are inert in agent focus (forwarded to the PTY)"
    );
}

// --- naming: shell session names are slugified via SessionManager -----------

#[test]
fn shell_tab_names_share_the_slug_with_the_agent_session() {
    // The agent session and the shell sessions of a worktree share one slug, so
    // kill-on-remove can match every `wtcc-<slug>*` surface.
    let slug = "feature-big-thing";
    let layout = {
        let mut l = WorktreeLayout::new(slug);
        l.add_shell_tab(slug);
        l
    };
    assert_eq!(
        layout.tabs[0].session,
        SessionManager::session_name("feature/big thing")
    );
    assert!(
        layout.tabs[1]
            .session
            .starts_with("wtcc-feature-big-thing-t")
    );
}
