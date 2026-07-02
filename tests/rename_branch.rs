//! TDD RED contract for issue #51: rename the selected worktree's branch and
//! RE-KEY its agent's tmux session WITHOUT killing the live agent.
//!
//! These tests pin the issue's acceptance criteria and the contract in its
//! Technical notes. They are expected to FAIL (compile errors) until the
//! production API exists:
//!   - `wtcc::worktree::rename_branch_argv` (pure `git branch -m` arg vector) and
//!     `wtcc::worktree::rename_branch` (argv-only `git -C <repo> branch -m`),
//!   - `wtcc::session::rename_session_argv` (pure `tmux rename-session` arg
//!     vector) and `SessionManager::rename` (in-memory map re-key + best-effort
//!     `tmux rename-session`, tested in-module),
//!   - `App::rename_branch` plus `Prompt::RenameBranch`,
//!   - keymap `Action::RenameBranch` (a primary key + `in_palette()`).
//!
//! No production code is written here. The real `git`/`tmux` spawns stay thin
//! behind the pure argv-builders and the unit-tested `SessionManager` re-key;
//! the only real-process work below is a git tempfile repo for the behavioural
//! rename, and the argv shape is verified purely.

use std::path::Path;
use std::process::Command;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use wtcc::app::{App, Overlay, Prompt};
use wtcc::config::Config;
use wtcc::event::handle_key;
use wtcc::keymap::{self, Action, PRIMARY};
use wtcc::repository::Repository;
use wtcc::session::{SessionManager, rename_session_argv};
use wtcc::ui::palette;
use wtcc::worktree::{self, Worktree, rename_branch_argv};

// --- helpers ----------------------------------------------------------------

fn key(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
}

fn run_git(repo: &Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .expect("git must be installed");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Initialises a git repo with one empty commit so HEAD is a real (renamable)
/// branch. The default branch name is left to the host git config and read back
/// from the worktree list, so the tests never assume `main` vs `master`.
fn init_repo(repo: &Path) {
    let out = Command::new("git")
        .arg("init")
        .arg(repo)
        .output()
        .expect("git must be installed");
    assert!(out.status.success(), "git init failed");
    run_git(repo, &["config", "user.email", "t@example.com"]);
    run_git(repo, &["config", "user.name", "wtcc test"]);
    run_git(repo, &["commit", "--allow-empty", "-m", "init"]);
}

/// An App with one selected repo at a NON-existent path (so no real git runs)
/// and one selected normal worktree `feature` at `/repo/feature`.
fn base_app() -> App {
    let cfg = Config {
        repos: vec![Repository {
            name: "demo".to_string(),
            path: std::path::PathBuf::from("/tmp/wtcc-issue51-does-not-exist"),
            setup: None,
            archive: None,
            archived: Vec::new(),
            base_ref: None,
            copy_on_create: Vec::new(),
            run: None,
            kind: wtcc::repository::RepoKind::Git,
        }],
        ..Default::default()
    };
    let mut app = App::new(cfg);
    app.worktrees = vec![Worktree {
        path: std::path::PathBuf::from("/repo/feature"),
        branch: "feature".to_string(),
        head: "abc".to_string(),
        is_bare: false,
        is_detached: false,
    }];
    app.worktree_repo = vec![0];
    app.selected_worktree = Some(0);
    app.status = None;
    app.overlay = Overlay::None;
    app
}

fn run_palette(app: &mut App, query: &str) {
    handle_key(app, key(':'));
    for c in query.chars() {
        handle_key(app, key(c));
    }
    handle_key(app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
}

// --- pure argv-builders: branch names are discrete, never shell strings ------

#[test]
fn rename_branch_argv_is_branch_dash_m_old_new() {
    assert_eq!(
        rename_branch_argv("old", "new"),
        vec!["branch", "-m", "old", "new"]
    );
}

#[test]
fn rename_branch_argv_keeps_the_new_name_as_one_discrete_element() {
    // SECURITY: a branch with shell metacharacters must remain exactly one argv
    // element — proof it is a discrete argument to `git`, never interpolated into
    // or split by a shell string.
    let evil = "feat/x; rm -rf / #";
    let argv = rename_branch_argv("old", evil);
    assert_eq!(argv, vec!["branch", "-m", "old", evil]);
    assert_eq!(
        argv.iter().filter(|a| a.as_str() == evil).count(),
        1,
        "the new branch must appear exactly once as its own element, got {argv:?}"
    );
}

#[test]
fn rename_session_argv_is_rename_session_t_old_new() {
    assert_eq!(
        rename_session_argv("wtcc-old", "wtcc-new"),
        vec!["rename-session", "-t", "wtcc-old", "wtcc-new"]
    );
}

#[test]
fn session_name_slugifies_the_new_branch() {
    // The branch goes to git verbatim, but every DERIVED identifier (the tmux
    // session key) is slugified first.
    assert_eq!(
        SessionManager::session_name("Feature/My Branch!"),
        "wtcc-feature-my-branch"
    );
}

// --- worktree::rename_branch over a REAL git repo ---------------------------

#[test]
fn rename_branch_renames_in_place_leaving_the_worktree_dir_unmoved() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path();
    init_repo(repo);

    let before = worktree::list(repo).unwrap();
    assert_eq!(before.len(), 1);
    let old = before[0].branch.clone();
    let old_path = before[0].path.clone();

    worktree::rename_branch(repo, &old, "renamed-here").unwrap();

    let after = worktree::list(repo).unwrap();
    assert_eq!(after.len(), 1);
    assert_eq!(after[0].branch, "renamed-here", "branch must be renamed");
    assert_ne!(after[0].branch, old);
    assert_eq!(
        after[0].path, old_path,
        "git branch -m must NOT move the worktree directory"
    );
    assert!(after[0].path.exists(), "the worktree dir must still exist");
}

#[test]
fn rename_branch_errors_on_a_name_collision() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path();
    init_repo(repo);
    let old = worktree::list(repo).unwrap()[0].branch.clone();
    run_git(repo, &["branch", "taken"]);

    let result = worktree::rename_branch(repo, &old, "taken");
    assert!(
        result.is_err(),
        "renaming onto an existing branch name must error"
    );
}

// --- App::rename_branch guards (no git, no spawn) ----------------------------

#[test]
fn rename_branch_empty_name_is_rejected() {
    let mut app = base_app();
    app.rename_branch("   ");
    let status = app.status.clone().unwrap_or_default().to_lowercase();
    assert!(
        status.contains("empty"),
        "an empty/whitespace name must be rejected, got {status:?}"
    );
}

#[test]
fn rename_branch_with_no_worktree_selected_is_rejected() {
    let mut app = base_app();
    app.worktrees.clear();
    app.selected_worktree = None;
    app.rename_branch("anything");
    assert_eq!(app.status.as_deref(), Some("no worktree selected"));
}

#[test]
fn rename_branch_on_a_detached_worktree_is_rejected() {
    let mut app = base_app();
    app.worktrees[0].is_detached = true;
    app.rename_branch("anything");
    let status = app.status.clone().unwrap_or_default().to_lowercase();
    assert!(
        status.contains("detached"),
        "a detached worktree must be refused, got {status:?}"
    );
}

#[test]
fn rename_branch_on_a_bare_worktree_is_rejected() {
    let mut app = base_app();
    app.worktrees[0].is_bare = true;
    app.rename_branch("anything");
    let status = app.status.clone().unwrap_or_default().to_lowercase();
    assert!(
        status.contains("bare"),
        "a bare worktree must be refused, got {status:?}"
    );
}

// --- keymap: a primary key + palette membership, no chord collisions ---------

#[test]
fn primary_binds_a_key_to_rename_branch() {
    assert_eq!(
        keymap::dispatch(PRIMARY, key('b')),
        Some(Action::RenameBranch)
    );
}

#[test]
fn rename_branch_is_offered_in_the_palette() {
    assert!(Action::RenameBranch.in_palette());
    assert!(palette::filter("").contains(&Action::RenameBranch));
}

#[test]
fn rename_branch_action_has_a_label() {
    assert_eq!(Action::RenameBranch.label(), "Rename branch");
}

#[test]
fn registering_the_rename_key_introduces_no_chord_collisions_in_primary() {
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

// --- event: the key + palette open the Input overlay; submit runs the rename -

#[test]
fn the_rename_key_opens_the_rename_input_prompt() {
    let mut app = base_app();
    handle_key(&mut app, key('b'));
    assert!(
        matches!(
            app.overlay,
            Overlay::Input {
                prompt: Prompt::RenameBranch,
                ..
            }
        ),
        "the rename key must open an Input overlay prompting for the new branch, got {:?}",
        app.overlay
    );
}

#[test]
fn palette_rename_opens_the_rename_input_prompt() {
    let mut app = base_app();
    run_palette(&mut app, "rename");
    assert!(
        matches!(
            app.overlay,
            Overlay::Input {
                prompt: Prompt::RenameBranch,
                ..
            }
        ),
        "the palette Rename command must open the rename Input overlay, got {:?}",
        app.overlay
    );
}

#[test]
fn submitting_the_rename_input_performs_the_rename_and_closes_the_overlay() {
    // base_app points at a non-existent repo, so the underlying git rename fails
    // fast; the test only pins the wiring: typing a name + Enter dispatches into
    // `rename_branch`, which closes the overlay and reports an outcome in status
    // without panicking.
    let mut app = base_app();
    handle_key(&mut app, key('b'));
    for c in "renamed".chars() {
        handle_key(&mut app, key(c));
    }
    handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    assert_eq!(app.overlay, Overlay::None, "submit must close the overlay");
    assert!(
        app.status.is_some(),
        "submit must report the rename outcome in status"
    );
}
