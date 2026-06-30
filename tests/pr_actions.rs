//! TDD RED contract for issue #50: GitHub PR *write* actions via `gh`
//! (open-in-browser, mark-ready, merge, close), from the command palette plus
//! two keys, with confirm-gating on the destructive ones.
//!
//! These tests pin the issue's acceptance criteria and the contract in its
//! Technical notes. They are expected to FAIL TO COMPILE until the production
//! API exists:
//!   - a write-only `wtcc::pr` module (`MergeStrategy`, `strategy_flag`, and the
//!     pure argv-builders `open_in_browser_argv` / `mark_ready_argv` /
//!     `merge_argv` / `close_argv`),
//!   - `Config::merge_strategy` (`#[serde(default)]`, default `Squash`),
//!   - the immediate actions `App::{pr_open_in_browser, pr_mark_ready}` and the
//!     confirm executors `App::{pr_merge_branch, pr_close_branch}`, all guarded
//!     via the `pr_target` helper,
//!   - `Confirm::{MergePr, ClosePr}` and their `render_confirm` arms,
//!   - keymap `Action::{OpenPrWeb, MarkReady, MergePr, ClosePr}` (with `o`/`m`
//!     bound and all four `in_palette()`).
//!
//! No production code is written here. The actual `gh` spawn is intentionally
//! left thin and UNTESTED behind the pure argv-builders; every test below is
//! pure or drives `App`/`event` only down paths that spawn nothing.

#![allow(clippy::field_reassign_with_default, clippy::type_complexity)]

use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use wtcc::app::{App, Confirm, Overlay};
use wtcc::config::Config;
use wtcc::event::handle_key;
use wtcc::keymap::{self, Action, PRIMARY};
use wtcc::pr::{
    self, MergeStrategy, close_argv, mark_ready_argv, merge_argv, open_in_browser_argv,
};
use wtcc::repository::Repository;
use wtcc::ui::palette;
use wtcc::vcs::{ChecksState, PrState, PrStatus, VcsStatus};
use wtcc::worktree::Worktree;

// --- helpers ----------------------------------------------------------------

fn key(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
}

/// An App with one selected repo (a non-existent path, so no real git runs) and
/// one selected worktree `main` at `/repo/main`. No cached PR by default.
fn base_app() -> App {
    let mut cfg = Config::default();
    cfg.repos = vec![Repository {
        name: "demo".to_string(),
        path: PathBuf::from("/tmp/wtcc-issue50-does-not-exist"),
        setup: None,
        archive: None,
        archived: Vec::new(),
        base_ref: None,
        copy_on_create: Vec::new(),
        run: None,
    }];
    let mut app = App::new(cfg);
    app.worktrees = vec![Worktree {
        path: PathBuf::from("/repo/main"),
        branch: "main".to_string(),
        head: "abc".to_string(),
        is_bare: false,
        is_detached: false,
    }];
    app.selected_worktree = Some(0);
    app.status = None;
    app.overlay = Overlay::None;
    app
}

/// `base_app` plus an injected cached OPEN PR on the selected worktree, so the
/// `selected_pr` guard sees a mergeable/closable PR.
fn app_with_open_pr() -> App {
    let mut app = base_app();
    app.vcs_status.insert(
        PathBuf::from("/repo/main"),
        VcsStatus {
            dirty: false,
            pr: Some(PrStatus {
                number: 7,
                state: PrState::Open,
                checks: ChecksState::Passing,
            }),
        },
    );
    app
}

fn app_no_worktree() -> App {
    let mut app = base_app();
    app.worktrees.clear();
    app.selected_worktree = None;
    app
}

/// Drives the command palette: open it, type `query`, press Enter.
fn run_palette(app: &mut App, query: &str) {
    handle_key(app, key(':'));
    for c in query.chars() {
        handle_key(app, key(c));
    }
    handle_key(app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
}

/// The four PR actions, each driven through its REAL entry point — a key
/// dispatch or the command palette, exactly as the production keymap/palette
/// invoke them — never an `App` method called directly. `merge`/`close` go
/// through `request_*` (which opens a confirm only when a PR exists), while
/// `open_in_browser`/`mark_ready` run immediately.
const PR_ACTIONS: [&str; 4] = ["merge", "close", "open_in_browser", "mark_ready"];

fn drive(app: &mut App, action: &str) {
    match action {
        "merge" => handle_key(app, key('m')),
        "open_in_browser" => handle_key(app, key('o')),
        "close" => run_palette(app, "close"),
        "mark_ready" => run_palette(app, "ready"),
        other => unreachable!("unknown PR action {other:?}"),
    }
}

// --- pr::strategy_flag (pure) -----------------------------------------------

#[test]
fn strategy_flag_maps_each_variant_to_its_gh_flag() {
    assert_eq!(MergeStrategy::Squash.strategy_flag(), "--squash");
    assert_eq!(MergeStrategy::Merge.strategy_flag(), "--merge");
    assert_eq!(MergeStrategy::Rebase.strategy_flag(), "--rebase");
}

#[test]
fn merge_strategy_default_is_squash() {
    assert_eq!(MergeStrategy::default(), MergeStrategy::Squash);
}

// --- pr::* argv-builders (pure) ---------------------------------------------
//
// Each builder returns the argument vector passed to `gh` (the program name is
// "gh", supplied by the spawn site). The branch is ALWAYS a single, discrete
// element — never interpolated into a shell string and never split.

#[test]
fn open_in_browser_argv_is_pr_view_branch_web() {
    assert_eq!(
        open_in_browser_argv("feat/x"),
        vec!["pr", "view", "feat/x", "--web"]
    );
}

#[test]
fn mark_ready_argv_is_pr_ready_branch() {
    assert_eq!(mark_ready_argv("feat/x"), vec!["pr", "ready", "feat/x"]);
}

#[test]
fn close_argv_is_pr_close_branch() {
    assert_eq!(close_argv("feat/x"), vec!["pr", "close", "feat/x"]);
}

#[test]
fn merge_argv_uses_the_selected_strategy_flag() {
    assert_eq!(
        merge_argv("feat/x", MergeStrategy::Squash),
        vec!["pr", "merge", "feat/x", "--squash"]
    );
    assert_eq!(
        merge_argv("feat/x", MergeStrategy::Merge),
        vec!["pr", "merge", "feat/x", "--merge"]
    );
    assert_eq!(
        merge_argv("feat/x", MergeStrategy::Rebase),
        vec!["pr", "merge", "feat/x", "--rebase"]
    );
}

#[test]
fn merge_argv_never_passes_delete_branch() {
    // wtcc owns the worktree/branch lifecycle, so the merge must not ask gh to
    // delete the branch.
    let argv = merge_argv("feat/x", MergeStrategy::Squash);
    assert!(
        !argv.iter().any(|a| a == "--delete-branch"),
        "merge must omit --delete-branch, got {argv:?}"
    );
}

#[test]
fn branch_is_a_single_discrete_argv_element_even_with_shell_metacharacters() {
    // SECURITY: a branch containing shell metacharacters must remain exactly one
    // argv element — proof that it is passed as a discrete argument, never
    // interpolated into or split by a shell string.
    let evil = "feat/x; rm -rf / #";
    for argv in [
        open_in_browser_argv(evil),
        mark_ready_argv(evil),
        close_argv(evil),
        merge_argv(evil, MergeStrategy::Squash),
    ] {
        assert!(
            argv.iter().filter(|a| a.as_str() == evil).count() == 1,
            "the branch must appear exactly once as its own element, got {argv:?}"
        );
    }
}

// --- config: merge_strategy is additive and back-compatible -----------------

#[test]
fn config_default_merge_strategy_is_squash() {
    assert_eq!(Config::default().merge_strategy, MergeStrategy::Squash);
}

#[test]
fn config_round_trips_a_non_default_merge_strategy() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");

    let mut original = Config::default();
    original.merge_strategy = MergeStrategy::Rebase;

    original.save_to(&path).unwrap();
    let loaded = Config::load_from(&path).unwrap();

    assert_eq!(loaded.merge_strategy, MergeStrategy::Rebase);
    assert_eq!(loaded, original);
}

#[test]
fn legacy_config_without_merge_strategy_defaults_to_squash() {
    // A config.toml written before #50 has no `merge_strategy` key.
    let cfg: Config = toml::from_str("agent_cmd = \"claude\"\n").unwrap();
    assert_eq!(cfg.merge_strategy, MergeStrategy::Squash);
}

// --- app: guards set a clear status and spawn nothing ------------------------

#[test]
fn pr_actions_with_no_worktree_set_status_and_open_no_overlay() {
    for name in PR_ACTIONS {
        let mut app = app_no_worktree();
        drive(&mut app, name);
        assert_eq!(
            app.overlay,
            Overlay::None,
            "{name}: must not open an overlay"
        );
        assert_eq!(
            app.status.as_deref(),
            Some("no worktree selected"),
            "{name}: must report no worktree selected"
        );
    }
}

#[test]
fn pr_actions_with_no_pr_set_a_no_pr_status_and_spawn_nothing() {
    for name in PR_ACTIONS {
        // base_app has a selected worktree but no cached PR.
        let mut app = base_app();
        drive(&mut app, name);
        // No confirm overlay means the destructive actions never reach a spawn;
        // the immediate ones guard out before any `gh` call.
        assert_eq!(
            app.overlay,
            Overlay::None,
            "{name}: a guarded-out action must open no overlay"
        );
        let status = app.status.clone().unwrap_or_default().to_lowercase();
        assert!(
            status.contains("no pr"),
            "{name}: expected a 'no PR' status, got {status:?}"
        );
    }
}

// --- keymap: registration, palette membership, labels, no collisions --------

#[test]
fn primary_binds_o_to_open_pr_web() {
    assert_eq!(keymap::dispatch(PRIMARY, key('o')), Some(Action::OpenPrWeb));
}

#[test]
fn primary_binds_m_to_merge_pr() {
    assert_eq!(keymap::dispatch(PRIMARY, key('m')), Some(Action::MergePr));
}

#[test]
fn all_four_pr_actions_are_offered_in_the_palette() {
    for a in [
        Action::OpenPrWeb,
        Action::MarkReady,
        Action::MergePr,
        Action::ClosePr,
    ] {
        assert!(a.in_palette(), "{a:?} must be offered in the palette");
    }
}

#[test]
fn pr_action_labels_match_the_acceptance_criteria() {
    assert_eq!(Action::OpenPrWeb.label(), "Open PR in browser");
    assert_eq!(Action::MarkReady.label(), "Mark PR ready");
    assert_eq!(Action::MergePr.label(), "Merge PR");
    assert_eq!(Action::ClosePr.label(), "Close PR");
}

#[test]
fn palette_lists_all_four_pr_actions() {
    let all = palette::filter("");
    for a in [
        Action::OpenPrWeb,
        Action::MarkReady,
        Action::MergePr,
        Action::ClosePr,
    ] {
        assert!(all.contains(&a), "palette must contain {a:?}");
    }
}

#[test]
fn registering_pr_keys_introduces_no_chord_collisions_in_primary() {
    let chords: Vec<_> = PRIMARY
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

// --- event: confirm-gating for the destructive actions ----------------------

#[test]
fn m_opens_a_merge_confirm_naming_the_branch() {
    let mut app = app_with_open_pr();
    handle_key(&mut app, key('m'));
    assert!(
        matches!(app.overlay, Overlay::Confirm(Confirm::MergePr(ref b)) if b == "main"),
        "m must open a MergePr confirm naming the branch, got {:?}",
        app.overlay
    );
}

#[test]
fn m_with_a_worktree_but_no_pr_sets_a_no_pr_status_and_opens_no_confirm() {
    // Drives the real key path (not App methods directly): with a worktree
    // selected but no cached PR, `m` must guard out at the request gate — set a
    // 'no PR' status and open NO confirm overlay (so nothing is ever spawned).
    let mut app = base_app();
    handle_key(&mut app, key('m'));
    assert_eq!(
        app.overlay,
        Overlay::None,
        "m must not open a confirm when there is no cached PR"
    );
    let status = app.status.clone().unwrap_or_default().to_lowercase();
    assert!(
        status.contains("no pr"),
        "expected a 'no PR' status, got {status:?}"
    );
}

#[test]
fn palette_merge_pr_opens_a_merge_confirm() {
    let mut app = app_with_open_pr();
    run_palette(&mut app, "merge");
    assert!(
        matches!(app.overlay, Overlay::Confirm(Confirm::MergePr(ref b)) if b == "main"),
        "palette Merge PR must open a MergePr confirm, got {:?}",
        app.overlay
    );
}

#[test]
fn n_aborts_the_merge_confirm() {
    let mut app = app_with_open_pr();
    handle_key(&mut app, key('m'));
    handle_key(&mut app, key('n'));
    assert_eq!(app.overlay, Overlay::None);
}

#[test]
fn confirming_merge_dispatches_to_pr_merge_branch_without_spawning_gh() {
    let mut app = app_with_open_pr();
    handle_key(&mut app, key('m'));
    assert!(matches!(app.overlay, Overlay::Confirm(Confirm::MergePr(_))));

    // Drop the cached PR so the post-confirm `pr_merge_branch` takes its no-PR
    // guard: this proves the y-arm dispatches into `pr_merge_branch` (which
    // resets the overlay and sets the no-PR status) WITHOUT spawning gh.
    app.vcs_status.clear();
    handle_key(&mut app, key('y'));

    assert_eq!(app.overlay, Overlay::None);
    let status = app.status.clone().unwrap_or_default().to_lowercase();
    assert!(
        status.contains("no pr"),
        "confirming must dispatch into pr_merge_branch (no-PR guard), got {status:?}"
    );
}

#[test]
fn palette_close_pr_opens_a_close_confirm() {
    let mut app = app_with_open_pr();
    run_palette(&mut app, "close");
    assert!(
        matches!(app.overlay, Overlay::Confirm(Confirm::ClosePr(ref b)) if b == "main"),
        "palette Close PR must open a ClosePr confirm, got {:?}",
        app.overlay
    );
}

#[test]
fn confirming_close_dispatches_to_pr_close_branch_without_spawning_gh() {
    let mut app = app_with_open_pr();
    run_palette(&mut app, "close");
    assert!(matches!(app.overlay, Overlay::Confirm(Confirm::ClosePr(_))));

    app.vcs_status.clear();
    handle_key(&mut app, key('y'));

    assert_eq!(app.overlay, Overlay::None);
    assert!(
        app.status.is_some(),
        "confirming close must dispatch into pr_close_branch and set a status"
    );
}

#[test]
fn open_in_browser_runs_immediately_with_no_confirm() {
    // No cached PR -> pr_open_in_browser guards out (no gh, no browser opened),
    // proving `o` runs the action directly with NO confirm overlay.
    let mut app = base_app();
    handle_key(&mut app, key('o'));
    assert_eq!(
        app.overlay,
        Overlay::None,
        "open-in-browser must not open a confirm"
    );
    assert!(
        app.status.is_some(),
        "open-in-browser must report the no-PR outcome"
    );
}

#[test]
fn mark_ready_runs_immediately_with_no_confirm() {
    // No cached PR -> pr_mark_ready guards out (no gh), proving it is immediate.
    let mut app = base_app();
    run_palette(&mut app, "ready");
    assert_eq!(
        app.overlay,
        Overlay::None,
        "mark-ready must not open a confirm"
    );
    assert!(app.status.is_some());
}

// --- ui: render_confirm names the branch for the new variants ---------------

#[test]
fn render_confirm_shows_the_branch_for_merge_and_close_without_panicking() {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    for confirm in [
        Confirm::MergePr("feature-x".to_string()),
        Confirm::ClosePr("feature-x".to_string()),
    ] {
        let mut app = base_app();
        app.overlay = Overlay::Confirm(confirm);

        let backend = TestBackend::new(100, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| wtcc::ui::draw(f, &app)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        let text: String = buffer.content().iter().map(|c| c.symbol()).collect();

        assert!(
            text.contains("feature-x"),
            "the confirm overlay must name the branch"
        );
    }
}

// Silence "unused import" if a future refactor drops a direct `pr::` reference;
// the module itself must exist for this contract to compile.
#[allow(unused_imports)]
use pr as _pr_module_must_exist;
