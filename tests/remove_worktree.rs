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

    std::fs::write(path.join("README.md"), b"hello").expect("write file");

    run_git(path, &["add", "."]);
    run_git(path, &["commit", "-m", "init"]);

    dir
}

/// End-to-end through the App on a real repo, with NO tmux session ever spawned:
/// adding then removing a worktree must succeed and never panic. The kill on
/// remove is best-effort, so the absence of a session is not an error.
#[test]
fn remove_worktree_succeeds_with_no_session_and_no_panic() {
    if !git_available() {
        eprintln!("skipping: git not available on PATH");
        return;
    }

    let repo = init_repo();
    let config_dir = tempfile::tempdir().expect("config tempdir");
    let config_path = config_dir.path().join("config.toml");

    let mut app = App::new(Config::default());
    app.config_path = Some(config_path);
    app.register_repository(repo.path().to_str().unwrap());
    app.add_worktree("feature-x");

    let wt_path = app
        .worktrees
        .iter()
        .find(|w| w.branch == "feature-x")
        .expect("feature-x worktree should be present after add")
        .path
        .clone();

    app.remove_worktree(&wt_path);

    // A successful remove refreshes the list (which clears `status`); the
    // worktree disappearing is the success signal. No agent session existed, so
    // the best-effort kill is a no-op and must not turn removal into an error.
    assert!(
        !app.worktrees.iter().any(|w| w.branch == "feature-x"),
        "feature-x worktree should be gone after a successful remove"
    );
}
