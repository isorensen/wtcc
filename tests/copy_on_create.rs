//! TDD RED contract for issue #55: per-repo `copy_on_create` files.
//!
//! These integration tests drive the real `App` against a real git repo. Git
//! worktrees do not carry ignored/untracked files (e.g. `.env`), so on creation
//! `wtcc` copies a configured allowlist of relative paths from the repo root
//! (the primary checkout) into the SAME relative location under the new worktree.
//! They pin the issue's acceptance criteria:
//!   - every valid `copy_on_create` file is copied into the new worktree dir;
//!   - an existing destination is NEVER overwritten (no-clobber);
//!   - a missing source is skipped without an error/panic;
//!   - nested paths are copied with their parent dirs created;
//!   - the copy runs on BOTH the new-branch and existing-branch creation paths;
//!   - copies/skips are surfaced in `status`.
//!
//! No production code lives here; the file is expected to FAIL TO COMPILE until
//! `Repository::copy_on_create` exists and `App::add_worktree` performs the copy.

use std::path::Path;
use std::process::Command;

use tempfile::TempDir;
use wtcc::app::App;
use wtcc::config::Config;

fn git_available() -> bool {
    Command::new("git")
        .arg("--version")
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

    std::fs::write(path.join(".gitignore"), b".env\n").expect("write .gitignore");
    std::fs::write(path.join("README.md"), b"hello").expect("write file");

    run_git(path, &["add", "."]);
    run_git(path, &["commit", "-m", "init"]);

    dir
}

/// Registers `repo` into a fresh `App` with a redirected config path so the test
/// never writes to the real XDG config. Returns the canonical repo root so files
/// are written where `copy_into_worktree` reads them from.
fn app_for(repo: &TempDir) -> (App, TempDir, std::path::PathBuf) {
    let config_dir = tempfile::tempdir().expect("config tempdir");
    let mut app = App::new(Config::default());
    app.config_path = Some(config_dir.path().join("config.toml"));
    app.register_repository(repo.path().to_str().unwrap());
    let root = app.config.repos[0].path.clone();
    (app, config_dir, root)
}

fn worktree_path(app: &App, branch: &str) -> std::path::PathBuf {
    app.worktrees
        .iter()
        .find(|w| w.branch == branch)
        .unwrap_or_else(|| panic!("worktree {branch} should be present after add"))
        .path
        .clone()
}

/// A configured `.env` (ignored, so absent from the fresh worktree) is copied
/// from the repo root into the new worktree with its contents, on the NEW-branch
/// path, and the copy is surfaced in status.
#[test]
fn add_worktree_copies_env_file_into_new_worktree() {
    if !git_available() {
        eprintln!("skipping: git not available on PATH");
        return;
    }

    let repo = init_repo();
    let (mut app, _cfg, root) = app_for(&repo);
    std::fs::write(root.join(".env"), b"TOKEN=abc123").expect("write .env in repo root");
    app.config.repos[0].copy_on_create = vec![".env".to_string()];

    app.add_worktree("feature-x");

    let wt = worktree_path(&app, "feature-x");
    assert_eq!(
        std::fs::read(wt.join(".env")).expect("the .env must be copied into the new worktree"),
        b"TOKEN=abc123",
        "the copied .env must preserve its contents"
    );
    let status = app.status.clone().unwrap_or_default().to_lowercase();
    assert!(
        status.contains("copied"),
        "status must report the copy, got: {status:?}"
    );
}

/// A nested relative path is copied with its parent directories created.
#[test]
fn add_worktree_copies_nested_path_creating_parent_dirs() {
    if !git_available() {
        eprintln!("skipping: git not available on PATH");
        return;
    }

    let repo = init_repo();
    let (mut app, _cfg, root) = app_for(&repo);
    std::fs::create_dir_all(root.join("config")).expect("mk config dir");
    std::fs::write(root.join("config/local.env"), b"K=1").expect("write nested file");
    app.config.repos[0].copy_on_create = vec!["config/local.env".to_string()];

    app.add_worktree("feature-nested");

    let wt = worktree_path(&app, "feature-nested");
    assert_eq!(
        std::fs::read(wt.join("config/local.env"))
            .expect("the nested file must be copied with parent dirs created"),
        b"K=1"
    );
}

/// No-clobber: a destination already present in the fresh worktree (a tracked
/// file) is NEVER overwritten by the copy of a locally-modified source.
#[test]
fn add_worktree_does_not_clobber_an_existing_destination() {
    if !git_available() {
        eprintln!("skipping: git not available on PATH");
        return;
    }

    let repo = init_repo();
    let (mut app, _cfg, root) = app_for(&repo);
    // A tracked file is committed, so the fresh worktree already has it.
    std::fs::write(root.join("tracked.conf"), b"COMMITTED").expect("write tracked file");
    run_git(&root, &["add", "tracked.conf"]);
    run_git(&root, &["commit", "-m", "add tracked.conf"]);
    // The primary checkout's copy then diverges (uncommitted local edit).
    std::fs::write(root.join("tracked.conf"), b"LOCALMOD").expect("locally modify source");
    app.config.repos[0].copy_on_create = vec!["tracked.conf".to_string()];

    app.add_worktree("feature-noclobber");

    let wt = worktree_path(&app, "feature-noclobber");
    assert_eq!(
        std::fs::read(wt.join("tracked.conf")).unwrap(),
        b"COMMITTED",
        "an existing destination must survive (no-clobber), not be replaced by the local source"
    );
    let status = app.status.clone().unwrap_or_default().to_lowercase();
    assert!(
        status.contains("skip"),
        "a no-clobber skip must be reported in status, got: {status:?}"
    );
}

/// A missing source is skipped gracefully: the worktree is still created and no
/// destination is written, with no error/panic.
#[test]
fn add_worktree_skips_missing_source_without_error() {
    if !git_available() {
        eprintln!("skipping: git not available on PATH");
        return;
    }

    let repo = init_repo();
    let (mut app, _cfg, root) = app_for(&repo);
    let _ = root; // no source file is created for the configured entry
    app.config.repos[0].copy_on_create = vec!["does-not-exist.env".to_string()];

    app.add_worktree("feature-missing");

    let wt = worktree_path(&app, "feature-missing");
    assert!(wt.exists(), "worktree must still be created");
    assert!(
        !wt.join("does-not-exist.env").exists(),
        "a missing source must not produce a destination"
    );
}

/// The copy also runs when checking out an EXISTING branch (review-a-PR path),
/// not only on the new-branch path.
#[test]
fn add_worktree_copies_on_the_existing_branch_path() {
    if !git_available() {
        eprintln!("skipping: git not available on PATH");
        return;
    }

    let repo = init_repo();
    let (mut app, _cfg, root) = app_for(&repo);
    run_git(&root, &["branch", "review-me"]);
    std::fs::write(root.join(".env"), b"TOKEN=existing").expect("write .env in repo root");
    app.config.repos[0].copy_on_create = vec![".env".to_string()];

    app.add_worktree("review-me");

    let wt = worktree_path(&app, "review-me");
    assert_eq!(
        std::fs::read(wt.join(".env"))
            .expect("the .env must be copied on the existing-branch path too"),
        b"TOKEN=existing"
    );
}
