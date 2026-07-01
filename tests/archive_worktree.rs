//! TDD RED contract for issue #53: soft-archive (hide) worktrees.
//!
//! Archiving is purely a UI/config concern: the worktree and its branch stay on
//! disk and in `git worktree list`. This file pins the issue's acceptance
//! criteria and the contract in its Technical notes. It is expected to FAIL
//! (compile errors, then assertions) until the production API exists:
//!   - `Repository` derives `Default` and gains
//!     `#[serde(default, skip_serializing_if = "Vec::is_empty")] archived: Vec<PathBuf>`,
//!   - `App` gains a `show_archived: bool` (default false) plus
//!     `archive_worktree(&Path)` / `unarchive_worktree(&Path)` (mutate the
//!     selected repo's `archived`, persist via `config_path`/`save`, roll back on
//!     save error),
//!   - `ui::sidebar::sidebar_rows` becomes the SINGLE filter point, taking
//!     `archived: &[PathBuf]` + `show_archived: bool` and keeping
//!     `SidebarRow::Worktree(wi)` carrying the REAL index into the full vec,
//!   - `event::hit_test` threads the same two args,
//!   - `keymap::Action::{ToggleArchive, ShowArchived}` are registered in the
//!     data-driven keymap (a PRIMARY chord each + `in_palette()`), with no chord
//!     collisions, and dispatch through the palette and `handle_key`.
//!
//! No production code is written here. No real git/tmux is touched: worktrees are
//! injected directly and persistence is redirected to a temp `config_path`.

use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use wtcc::app::App;
use wtcc::config::Config;
use wtcc::event::{self, Hit, handle_key};
use wtcc::keymap::{self, Action, Chord, PRIMARY};
use wtcc::repository::Repository;
use wtcc::ui::palette;
use wtcc::ui::sidebar::{SidebarRow, sidebar_rows};
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

/// An App with one repo (at a non-existent path so no real git runs) and the
/// given branches injected as worktrees, selection on the first.
fn app_with(branches: &[&str]) -> App {
    let cfg = Config {
        repos: vec![Repository {
            name: "demo".to_string(),
            path: PathBuf::from("/tmp/wtcc-issue53-does-not-exist"),
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

// --- config: Repository.archived is additive, path-keyed, back-compatible ----

#[test]
fn legacy_repo_entry_without_archived_loads_with_empty_default() {
    // A config.toml written before #53 has no `archived` key on its repos.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        "agent_cmd = \"claude\"\n[[repos]]\nname = \"demo\"\npath = \"/tmp/demo\"\n",
    )
    .unwrap();

    let cfg = Config::load_from(&path).unwrap();
    assert!(
        cfg.repos[0].archived.is_empty(),
        "a legacy repo entry must load with an empty `archived` (serde default)"
    );
}

#[test]
fn archived_round_trips_and_empty_is_omitted_on_save() {
    let dir = tempfile::tempdir().unwrap();

    // Empty `archived` must be omitted from the serialized output.
    let empty_path = dir.path().join("empty.toml");
    Config {
        repos: vec![Repository {
            name: "demo".to_string(),
            path: PathBuf::from("/home/user/demo"),
            ..Default::default()
        }],
        ..Default::default()
    }
    .save_to(&empty_path)
    .unwrap();
    let empty_serialized = std::fs::read_to_string(&empty_path).unwrap();
    assert!(
        !empty_serialized.contains("archived"),
        "an empty `archived` must be omitted via skip_serializing_if, got:\n{empty_serialized}"
    );

    // A populated `archived` must serialize (as PATHS) and round-trip exactly.
    let archived_path = PathBuf::from("/home/user/demo/.worktrees/feat");
    let original = Config {
        repos: vec![Repository {
            name: "demo".to_string(),
            path: PathBuf::from("/home/user/demo"),
            archived: vec![archived_path.clone()],
            ..Default::default()
        }],
        ..Default::default()
    };
    let path = dir.path().join("populated.toml");
    original.save_to(&path).unwrap();
    let serialized = std::fs::read_to_string(&path).unwrap();
    assert!(
        serialized.contains("archived"),
        "a populated `archived` must serialize, got:\n{serialized}"
    );
    assert!(
        serialized.contains("feat"),
        "archived worktrees must be stored as PATHS, got:\n{serialized}"
    );

    let loaded = Config::load_from(&path).unwrap();
    assert_eq!(loaded, original);
    assert_eq!(loaded.repos[0].archived, vec![archived_path]);
}

// --- app: archive/unarchive mutate the selected repo, persist, roll back -----

#[test]
fn show_archived_defaults_to_false() {
    let app = app_with(&["main"]);
    assert!(
        !app.show_archived,
        "the show-archived UI toggle must start hidden (false)"
    );
}

#[test]
fn archive_then_unarchive_round_trips_and_persists_to_config() {
    let dir = tempfile::tempdir().unwrap();
    let cfg_path = dir.path().join("config.toml");
    let mut app = app_with(&["main", "feat"]);
    app.config_path = Some(cfg_path.clone());
    let feat = app.worktrees[1].path.clone();

    app.archive_worktree(&feat);
    assert!(
        app.config.repos[0].archived.contains(&feat),
        "archiving must add the path to the selected repo's `archived`"
    );
    let persisted = Config::load_from(&cfg_path).unwrap();
    assert!(
        persisted.repos[0].archived.contains(&feat),
        "archive state must persist so it survives a wtcc restart"
    );

    app.unarchive_worktree(&feat);
    assert!(
        !app.config.repos[0].archived.contains(&feat),
        "unarchiving must remove the path from `archived`"
    );
    let persisted = Config::load_from(&cfg_path).unwrap();
    assert!(
        !persisted.repos[0].archived.contains(&feat),
        "the unarchive must persist too"
    );
}

#[test]
fn archive_keeps_the_worktree_on_disk_list_untouched() {
    // Archiving is a soft hide: the worktrees vec (mirrors `git worktree list`)
    // must be unchanged — only config's `archived` marker changes.
    let dir = tempfile::tempdir().unwrap();
    let mut app = app_with(&["main", "feat"]);
    app.config_path = Some(dir.path().join("config.toml"));
    let before = app.worktrees.clone();
    let feat = app.worktrees[1].path.clone();

    app.archive_worktree(&feat);

    assert_eq!(
        app.worktrees, before,
        "archive must NOT remove the worktree (no git/disk op)"
    );
}

#[test]
fn archive_with_a_failing_save_rolls_the_marker_back_and_reports_it() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app_with(&["main", "feat"]);
    // Point config_path at an EXISTING DIRECTORY so the write fails.
    app.config_path = Some(dir.path().to_path_buf());
    let feat = app.worktrees[1].path.clone();

    app.archive_worktree(&feat);

    assert!(
        !app.config.repos[0].archived.contains(&feat),
        "a failed save must roll the `archived` marker back out of the config"
    );
    let status = app.status.clone().unwrap_or_default().to_lowercase();
    assert!(
        status.contains("save"),
        "a save failure must be reported in status, got {status:?}"
    );
}

#[test]
fn archiving_the_selected_worktree_keeps_selection_within_bounds() {
    // The worktrees vec is unchanged by archiving, so the selection index must
    // stay valid (never out of bounds, never panics).
    let dir = tempfile::tempdir().unwrap();
    let mut app = app_with(&["main", "feat", "bug"]);
    app.config_path = Some(dir.path().join("config.toml"));
    app.selected_worktree = Some(1);
    let feat = app.worktrees[1].path.clone();

    app.archive_worktree(&feat);

    match app.selected_worktree {
        Some(i) => assert!(
            i < app.worktrees.len(),
            "selection must stay within the worktrees vec after archiving"
        ),
        None => assert!(
            app.worktrees.is_empty(),
            "selection may only be None when there are no worktrees"
        ),
    }
}

// --- sidebar_rows: the SINGLE filter point, real-index invariant -------------

#[test]
fn sidebar_rows_omits_archived_when_hidden_with_real_indices_preserved() {
    let wts = worktrees(&["main", "feat", "bug"]); // real indices 0, 1, 2
    let repos = vec![Repository {
        name: "demo".to_string(),
        path: PathBuf::from("/tmp/demo"),
        archived: vec![wts[1].path.clone()], // archive the middle one
        ..Default::default()
    }];
    let tags = vec![0usize; wts.len()];
    let expanded: std::collections::HashSet<PathBuf> =
        [PathBuf::from("/tmp/demo")].into_iter().collect();

    let rows = sidebar_rows(&repos, &wts, &tags, &expanded, false);

    let indices: Vec<usize> = rows
        .iter()
        .filter_map(|r| match r {
            SidebarRow::Worktree(i) => Some(*i),
            _ => None,
        })
        .collect();
    assert_eq!(
        indices,
        vec![0, 2],
        "hidden archived row must be skipped, and the surviving rows must carry \
         their REAL index into the full worktrees vec (2, not a reindexed 1)"
    );
}

#[test]
fn sidebar_rows_includes_archived_when_shown_with_full_indices() {
    let wts = worktrees(&["main", "feat", "bug"]);
    let repos = vec![Repository {
        name: "demo".to_string(),
        path: PathBuf::from("/tmp/demo"),
        archived: vec![wts[1].path.clone()],
        ..Default::default()
    }];
    let tags = vec![0usize; wts.len()];
    let expanded: std::collections::HashSet<PathBuf> =
        [PathBuf::from("/tmp/demo")].into_iter().collect();

    let rows = sidebar_rows(&repos, &wts, &tags, &expanded, true);

    let indices: Vec<usize> = rows
        .iter()
        .filter_map(|r| match r {
            SidebarRow::Worktree(i) => Some(*i),
            _ => None,
        })
        .collect();
    assert_eq!(
        indices,
        vec![0, 1, 2],
        "showing archived must reveal every worktree with contiguous real indices"
    );
}

// --- event: hit_test stays consistent with archived filtering ----------------

#[test]
fn hit_test_maps_filtered_rows_to_real_worktree_indices() {
    // Layout: row 0 = top border, row 1 = RepoHeader(0), then the worktree rows.
    let area = ratatui::layout::Rect::new(0, 0, 80, 24);
    let wts = worktrees(&["main", "feat", "bug"]);
    let repos = vec![Repository {
        name: "demo".to_string(),
        path: PathBuf::from("/tmp/demo"),
        archived: vec![wts[1].path.clone()], // hide feat (index 1)
        ..Default::default()
    }];
    let tags = vec![0usize; wts.len()];
    let expanded: std::collections::HashSet<PathBuf> =
        [PathBuf::from("/tmp/demo")].into_iter().collect();

    // Hidden: rows are RepoHeader(0), Worktree(0), Worktree(2). Screen row 3 is
    // the second worktree row, which must resolve to the REAL index 2.
    let hit = event::hit_test(3, 3, area, &repos, &wts, &tags, &expanded, false);
    assert_eq!(
        hit,
        Hit::Worktree(2),
        "with feat hidden, the second worktree row must hit-test to real index 2"
    );

    // Shown: rows are RepoHeader(0), Worktree(0), Worktree(1), Worktree(2).
    // Screen row 3 is now Worktree(1).
    let hit = event::hit_test(3, 3, area, &repos, &wts, &tags, &expanded, true);
    assert_eq!(
        hit,
        Hit::Worktree(1),
        "with archived shown, the same screen row must hit-test to real index 1"
    );
}

// --- keymap: ToggleArchive + ShowArchived registered, no collisions ----------

#[test]
fn primary_binds_x_to_toggle_archive_and_shift_x_to_show_archived() {
    assert_eq!(
        keymap::dispatch(PRIMARY, key('x')),
        Some(Action::ToggleArchive)
    );
    assert_eq!(
        keymap::dispatch(PRIMARY, key('X')),
        Some(Action::ShowArchived)
    );
}

#[test]
fn archive_actions_are_offered_in_the_palette_with_labels() {
    assert!(Action::ToggleArchive.in_palette());
    assert!(Action::ShowArchived.in_palette());
    assert!(palette::filter("").contains(&Action::ToggleArchive));
    assert!(palette::filter("").contains(&Action::ShowArchived));
    assert!(
        !Action::ToggleArchive.label().is_empty(),
        "ToggleArchive must carry a human-readable label"
    );
    assert!(
        !Action::ShowArchived.label().is_empty(),
        "ShowArchived must carry a human-readable label"
    );
}

#[test]
fn registering_archive_keys_introduces_no_chord_collisions_in_primary() {
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

// --- event: the keys dispatch through handle_key -----------------------------

#[test]
fn the_toggle_archive_key_archives_then_unarchives_the_selected_worktree() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app_with(&["main"]);
    app.config_path = Some(dir.path().join("config.toml"));
    let main = app.worktrees[0].path.clone();

    handle_key(&mut app, key('x'));
    assert!(
        app.config.repos[0].archived.contains(&main),
        "the toggle-archive key must archive the selected worktree"
    );

    handle_key(&mut app, key('x'));
    assert!(
        !app.config.repos[0].archived.contains(&main),
        "pressing the toggle-archive key again must unarchive it"
    );
}

#[test]
fn archiving_the_selected_worktree_while_hidden_moves_selection_to_a_visible_neighbor() {
    // show_archived defaults to false: archiving the selected row hides it, so
    // selection must move to a still-visible neighbor (never strand on a hidden
    // row, which current_worktree would otherwise still resolve to).
    let dir = tempfile::tempdir().unwrap();
    let mut app = app_with(&["main", "feat", "bug"]);
    app.config_path = Some(dir.path().join("config.toml"));
    app.selected_worktree = Some(0);
    let main = app.worktrees[0].path.clone();

    handle_key(&mut app, key('x'));

    assert_eq!(
        app.selected_worktree,
        Some(1),
        "selection must move off the just-hidden row to the next visible one"
    );
    assert_ne!(
        app.current_worktree().map(|w| w.path.clone()),
        Some(main),
        "the resolved current worktree must not be the hidden, archived one"
    );
}

#[test]
fn the_show_archived_key_toggles_the_show_archived_flag() {
    let mut app = app_with(&["main"]);
    assert!(!app.show_archived);

    handle_key(&mut app, key('X'));
    assert!(
        app.show_archived,
        "the show-archived key must reveal archived rows"
    );

    handle_key(&mut app, key('X'));
    assert!(
        !app.show_archived,
        "pressing the show-archived key again must hide them"
    );
}
