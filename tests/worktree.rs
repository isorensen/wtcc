use std::path::Path;
use std::process::Command;

use tempfile::TempDir;
use wtcc::worktree;

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

#[test]
fn add_list_remove_worktree_flow() {
    if !git_available() {
        eprintln!("skipping: git not available on PATH");
        return;
    }

    let repo = init_repo();
    let repo_path = repo.path();

    let wt_parent = tempfile::tempdir().expect("create sibling tempdir");
    let new_wt_path = wt_parent.path().join("feature-x-wt");

    worktree::add(repo_path, &new_wt_path, "feature-x").expect("add worktree");

    let worktrees = worktree::list(repo_path).expect("list worktrees");
    let added = worktrees
        .iter()
        .find(|w| w.branch == "feature-x")
        .expect("feature-x worktree present");

    let expected = new_wt_path.canonicalize().unwrap_or(new_wt_path.clone());
    let actual = added.path.canonicalize().unwrap_or(added.path.clone());
    assert_eq!(
        actual.file_name(),
        expected.file_name(),
        "worktree path final component mismatch: {actual:?} vs {expected:?}"
    );

    worktree::remove(repo_path, &new_wt_path).expect("remove worktree");

    let after = worktree::list(repo_path).expect("list after remove");
    assert!(
        !after.iter().any(|w| w.branch == "feature-x"),
        "feature-x worktree should be gone after remove"
    );
}
