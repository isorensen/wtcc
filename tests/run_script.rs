//! TDD RED contract for issue #56: per-repo `run` command launched on a keypress
//! into a dedicated Run TAB (reusing the #48 tabs surface, NOT a bespoke pane
//! toggle).
//!
//! A repo may store one `run` command (e.g. `pnpm dev`, `cargo test`). Pressing
//! `s` in SIDEBAR focus starts `wtcc-run-<slug>` in the worktree dir and shows
//! its output in a Run tab; with no `run` set it explains via status and spawns
//! nothing. In AGENT focus `s` forwards to the PTY like any printable key.
//!
//! This file pins the issue's acceptance criteria for the parts reachable
//! through the public crate API:
//!   - the data-driven keymap registration (`s` -> RunScript, palette membership,
//!     no chord collisions, NOT reserved in agent focus),
//!   - input routing through `handle_key` in SIDEBAR vs AGENT focus,
//!   - the Run tab (`TabKind::Run`) backed by the `wtcc-run-<slug>` session.
//!
//! No production code is written here. It is expected to FAIL TO COMPILE until
//! `Repository::run`, `Action::RunScript`, `TabKind::Run`, `App::start_run_script`
//! and `session::run_session_name` exist. No real tmux/git is touched: the repo
//! path is non-existent and tab-model mutation is in-memory.

use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use wtcc::app::{App, Focus};
use wtcc::config::Config;
use wtcc::event::handle_key;
use wtcc::keymap::{self, AGENT, Action, Chord, PRIMARY};
use wtcc::layout::TabKind;
use wtcc::repository::Repository;
use wtcc::session::run_session_name;
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
            path: PathBuf::from("/tmp/wtcc-issue56-does-not-exist"),
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

fn has_run_tab(app: &App, slug: &str) -> bool {
    app.layouts
        .get(slug)
        .map(|l| l.tabs.iter().any(|t| t.kind == TabKind::Run))
        .unwrap_or(false)
}

// --- keymap: data-driven registration of the run-script Action --------------

#[test]
fn primary_binds_s_to_run_script() {
    assert_eq!(keymap::dispatch(PRIMARY, key('s')), Some(Action::RunScript));
}

#[test]
fn s_is_not_reserved_in_agent_focus() {
    // In agent focus every printable key forwards to the PTY, so `s` must NOT be
    // reserved by the AGENT map.
    assert_eq!(keymap::dispatch(AGENT, key('s')), None);
}

#[test]
fn run_script_is_offered_in_the_palette_with_a_label() {
    assert!(Action::RunScript.in_palette());
    assert!(
        !Action::RunScript.label().is_empty(),
        "RunScript must carry a human label"
    );
    assert!(
        palette::filter("").contains(&Action::RunScript),
        "palette must contain RunScript"
    );
}

#[test]
fn registering_run_script_introduces_no_chord_collisions_in_primary() {
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

// --- event: `s` routes to start_run_script in SIDEBAR focus only -------------

#[test]
fn s_in_sidebar_focus_opens_a_run_tab_when_configured() {
    let mut app = app_with(&["main"]);
    app.config.repos[0].run = Some("pnpm dev".to_string());

    handle_key(&mut app, key('s'));

    assert!(
        has_run_tab(&app, "main"),
        "s in sidebar focus opens a Run tab for the current worktree"
    );
    let layout = app.layouts.get("main").unwrap();
    let run_tab = layout.tabs.iter().find(|t| t.kind == TabKind::Run).unwrap();
    assert_eq!(
        run_tab.session,
        run_session_name("main"),
        "the Run tab is backed by the wtcc-run-<slug> session"
    );
}

#[test]
fn s_in_sidebar_focus_with_no_run_sets_status_and_opens_no_tab() {
    let mut app = app_with(&["main"]);
    assert_eq!(app.config.repos[0].run, None);
    app.status = None;

    handle_key(&mut app, key('s'));

    assert!(
        !has_run_tab(&app, "main"),
        "no run configured -> no Run tab is opened"
    );
    assert!(
        app.status.is_some(),
        "no run configured must explain via status"
    );
}

#[test]
fn s_in_agent_focus_does_not_open_a_run_tab() {
    // Consistency guard: with the agent focused, `s` forwards to the PTY and must
    // NOT open a Run tab even when a run command is configured.
    let mut app = app_with(&["main"]);
    app.config.repos[0].run = Some("pnpm dev".to_string());
    app.focus = Focus::Agent;

    handle_key(&mut app, key('s'));

    assert!(
        !has_run_tab(&app, "main"),
        "s is forwarded to the PTY in agent focus and must not open a run tab"
    );
}
