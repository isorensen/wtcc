//! TDD RED contract for issue #49: per-repo `setup`/`archive` lifecycle scripts.
//!
//! These integration tests drive the real `App` against a real git repo and a
//! real `sh`, with NO tmux required for the archive path. They pin the issue's
//! acceptance criteria:
//!   - ARCHIVE runs synchronously in the worktree dir BEFORE git removal.
//!   - A non-zero archive does NOT block removal.
//!   - A hanging archive is killed at the timeout and removal still proceeds.
//!   - The no-script path is unchanged.
//!   - SETUP (tmux-gated) runs once in the new worktree dir on create.
//!
//! No production code lives here; the file is expected to FAIL TO COMPILE until
//! `Repository::{setup,archive}`, `app::run_archive`, `app::ArchiveOutcome`, and
//! `app::ARCHIVE_TIMEOUT` exist.

use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

use tempfile::TempDir;
use wtcc::app::{ARCHIVE_TIMEOUT, App};
use wtcc::config::Config;

fn git_available() -> bool {
    Command::new("git")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn tmux_available() -> bool {
    Command::new("tmux")
        .arg("-V")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn run_git(repo: &Path, args: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .expect("failed to spawn git");
    assert!(
        output.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&output.stderr)
    );
}

fn init_repo() -> TempDir {
    let dir = tempfile::tempdir().expect("create tempdir");
    let path = dir.path();

    let output = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(["-c", "init.defaultBranch=main", "init"])
        .output()
        .expect("failed to spawn git init");
    assert!(
        output.status.success(),
        "git init failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    run_git(path, &["config", "user.email", "test@example.com"]);
    run_git(path, &["config", "user.name", "Test"]);

    std::fs::write(path.join("README.md"), b"hello").expect("write file");

    run_git(path, &["add", "."]);
    run_git(path, &["commit", "-m", "init"]);

    dir
}

/// Registers `repo` into a fresh `App` with a redirected config path so the test
/// never writes to the real XDG config.
fn app_for(repo: &TempDir) -> (App, TempDir) {
    let config_dir = tempfile::tempdir().expect("config tempdir");
    let mut app = App::new(Config::default());
    app.config_path = Some(config_dir.path().join("config.toml"));
    app.register_repository(repo.path().to_str().unwrap());
    (app, config_dir)
}

fn worktree_path(app: &App, branch: &str) -> std::path::PathBuf {
    app.worktrees
        .iter()
        .find(|w| w.branch == branch)
        .unwrap_or_else(|| panic!("worktree {branch} should be present after add"))
        .path
        .clone()
}

/// ARCHIVE runs in the worktree dir BEFORE git removal: the script records its
/// own `$PWD` (the worktree) into a marker OUTSIDE the worktree, which could only
/// happen while the worktree still existed.
#[test]
fn archive_runs_in_worktree_dir_before_git_removal() {
    if !git_available() {
        eprintln!("skipping: git not available on PATH");
        return;
    }

    let repo = init_repo();
    let marker_dir = tempfile::tempdir().expect("marker tempdir");
    let marker = marker_dir.path().join("archive-cwd.txt");

    let (mut app, _cfg) = app_for(&repo);
    app.config.repos[0].archive = Some(format!("pwd -P > {}", marker.display()));

    app.add_worktree("feature-x");
    let wt = worktree_path(&app, "feature-x");
    let wt_canon = wt
        .canonicalize()
        .expect("worktree dir must exist after add");

    app.remove_worktree(&wt);

    assert!(
        !app.worktrees.iter().any(|w| w.branch == "feature-x"),
        "worktree must be gone after remove"
    );
    let recorded =
        std::fs::read_to_string(&marker).expect("archive must have run and written the marker");
    assert_eq!(
        recorded.trim(),
        wt_canon.to_string_lossy(),
        "archive must run with cwd = the worktree, before it is removed"
    );
}

/// A non-zero archive exit must NOT block removal; the worktree is still removed
/// and the archive is observed to have run first.
#[test]
fn failing_archive_does_not_block_removal() {
    if !git_available() {
        eprintln!("skipping: git not available on PATH");
        return;
    }

    let repo = init_repo();
    let marker_dir = tempfile::tempdir().expect("marker tempdir");
    let marker = marker_dir.path().join("ran.txt");

    let (mut app, _cfg) = app_for(&repo);
    app.config.repos[0].archive = Some(format!("echo ran > {} ; exit 7", marker.display()));

    app.add_worktree("boom");
    let wt = worktree_path(&app, "boom");

    app.remove_worktree(&wt);

    assert!(
        !app.worktrees.iter().any(|w| w.branch == "boom"),
        "a non-zero archive must not block worktree removal"
    );
    assert!(marker.exists(), "archive must have run before removal");
}

/// A hanging archive is killed at the timeout and removal still proceeds, with a
/// timeout surfaced in status — the TUI must never freeze on a slow script.
#[test]
fn hanging_archive_times_out_and_removal_proceeds() {
    if !git_available() {
        eprintln!("skipping: git not available on PATH");
        return;
    }

    let repo = init_repo();
    let (mut app, _cfg) = app_for(&repo);
    app.config.repos[0].archive = Some("sleep 30".to_string());

    app.add_worktree("slow");
    let wt = worktree_path(&app, "slow");

    let start = Instant::now();
    app.remove_worktree(&wt);
    let elapsed = start.elapsed();

    assert!(
        !app.worktrees.iter().any(|w| w.branch == "slow"),
        "removal must proceed even when the archive hangs"
    );
    assert!(
        elapsed < ARCHIVE_TIMEOUT + Duration::from_secs(5),
        "remove_worktree must not block on the hanging archive (took {elapsed:?})"
    );
    let status = app.status.clone().unwrap_or_default().to_lowercase();
    assert!(
        status.contains("timed out") || status.contains("timeout"),
        "status must report the archive timeout, got: {status:?}"
    );
}

/// With no setup/archive configured, create + remove behave exactly as before:
/// the worktree is created and removed and nothing extra is written.
#[test]
fn no_lifecycle_scripts_create_and_remove_unchanged() {
    if !git_available() {
        eprintln!("skipping: git not available on PATH");
        return;
    }

    let repo = init_repo();
    let (mut app, _cfg) = app_for(&repo);
    assert_eq!(app.config.repos[0].setup, None);
    assert_eq!(app.config.repos[0].archive, None);

    app.add_worktree("plain");
    let wt = worktree_path(&app, "plain");
    assert!(wt.exists(), "worktree dir must be created");

    app.remove_worktree(&wt);
    assert!(
        !app.worktrees.iter().any(|w| w.branch == "plain"),
        "worktree must be gone after remove on the no-script path"
    );
}

/// The status line reports that setup started when a `setup` script is
/// configured, and does NOT mention setup on the no-script path. Deterministic:
/// asserts on the status string only (no tmux/session liveness), git-gated.
#[test]
fn add_worktree_status_reports_setup_only_when_configured() {
    if !git_available() {
        eprintln!("skipping: git not available on PATH");
        return;
    }

    let repo = init_repo();
    let (mut app, _cfg) = app_for(&repo);
    app.config.repos[0].setup = Some("true".to_string());

    app.add_worktree("with-setup");
    let status = app.status.clone().expect("status set after add");
    assert!(
        status.contains("setup"),
        "status must mention setup when a setup script is configured, got: {status:?}"
    );

    app.config.repos[0].setup = None;
    app.add_worktree("plain-branch");
    let status = app.status.clone().expect("status set after add");
    assert!(
        !status.contains("setup"),
        "status must NOT mention setup on the no-script path, got: {status:?}"
    );
}

/// SETUP runs once in the new worktree dir on create (tmux-gated): the script
/// touches a marker INSIDE the worktree, proving cwd = the new worktree.
#[test]
fn setup_runs_in_new_worktree_on_create() {
    if !git_available() || !tmux_available() {
        eprintln!("skipping: git or tmux not available on PATH");
        return;
    }

    let repo = init_repo();
    let (mut app, _cfg) = app_for(&repo);
    app.config.repos[0].setup = Some("touch setup-done.txt".to_string());

    app.add_worktree("setup-x");
    let wt = worktree_path(&app, "setup-x");
    let marker = wt.join("setup-done.txt");

    let mut found = false;
    for _ in 0..100 {
        if marker.exists() {
            found = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    // Cleanup the worktree (and its setup session) before asserting.
    app.remove_worktree(&wt);

    assert!(
        found,
        "setup script must run once with cwd = the new worktree directory"
    );
}
