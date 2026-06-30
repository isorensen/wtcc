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

    worktree::add_new_branch(repo_path, &new_wt_path, "feature-x", None).expect("add worktree");

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

#[test]
fn add_existing_branch_checks_it_out_without_duplicate_error() {
    if !git_available() {
        eprintln!("skipping: git not available on PATH");
        return;
    }

    let repo = init_repo();
    let repo_path = repo.path();

    // Pre-create a branch (without a worktree) to simulate reviewing a PR.
    run_git(repo_path, &["branch", "review-me"]);

    let wt_parent = tempfile::tempdir().expect("create sibling tempdir");
    let new_wt_path = wt_parent.path().join("review-me-wt");

    worktree::add_existing_branch(repo_path, &new_wt_path, "review-me")
        .expect("add worktree for existing branch");

    let worktrees = worktree::list(repo_path).expect("list worktrees");
    assert!(
        worktrees.iter().any(|w| w.branch == "review-me"),
        "review-me worktree should check out the existing branch"
    );
}

#[test]
fn branch_exists_true_for_existing_false_otherwise() {
    if !git_available() {
        eprintln!("skipping: git not available on PATH");
        return;
    }

    let repo = init_repo();
    let repo_path = repo.path();

    run_git(repo_path, &["branch", "already-here"]);

    assert!(worktree::branch_exists(repo_path, "already-here"));
    assert!(!worktree::branch_exists(repo_path, "does-not-exist"));
}

// --- issue #54: per-repo base ref for NEW-branch worktrees ------------------
//
// TDD RED (real git): a new-branch worktree forks from the given base ref when
// `Some(base)` is passed, and from HEAD when `None` (current behavior). The base
// ref is a discrete trailing arg to `git worktree add -b <branch> <path> <base>`.

fn rev_parse(repo: &Path, rev: &str) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["rev-parse", rev])
        .output()
        .expect("failed to spawn git rev-parse");
    assert!(
        out.status.success(),
        "git rev-parse {rev} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

#[test]
fn add_new_branch_from_base_ref_lands_on_that_ref() {
    if !git_available() {
        eprintln!("skipping: git not available on PATH");
        return;
    }

    let repo = init_repo();
    let repo_path = repo.path();

    // Capture commit A (current HEAD), pin a base branch at A, then advance HEAD
    // to commit B so the base ref is genuinely distinct from HEAD.
    let base_commit = rev_parse(repo_path, "HEAD");
    run_git(repo_path, &["branch", "the-base"]);
    run_git(repo_path, &["commit", "--allow-empty", "-m", "second"]);
    let head_commit = rev_parse(repo_path, "HEAD");
    assert_ne!(
        base_commit, head_commit,
        "HEAD must have advanced past the base"
    );

    let wt_parent = tempfile::tempdir().expect("create sibling tempdir");
    let new_wt_path = wt_parent.path().join("feat-wt");

    worktree::add_new_branch(repo_path, &new_wt_path, "feat", Some("the-base"))
        .expect("add new branch from base ref");

    let wt_head = rev_parse(&new_wt_path, "HEAD");
    assert_eq!(
        wt_head, base_commit,
        "a new branch with a base ref must start at the base, not HEAD"
    );
    assert_ne!(wt_head, head_commit);
}

#[test]
fn add_new_branch_without_base_branches_from_head() {
    if !git_available() {
        eprintln!("skipping: git not available on PATH");
        return;
    }

    let repo = init_repo();
    let repo_path = repo.path();
    run_git(repo_path, &["commit", "--allow-empty", "-m", "second"]);
    let head_commit = rev_parse(repo_path, "HEAD");

    let wt_parent = tempfile::tempdir().expect("create sibling tempdir");
    let new_wt_path = wt_parent.path().join("feat-wt");

    worktree::add_new_branch(repo_path, &new_wt_path, "feat", None)
        .expect("add new branch from HEAD");

    let wt_head = rev_parse(&new_wt_path, "HEAD");
    assert_eq!(
        wt_head, head_commit,
        "with no base ref, a new branch forks from HEAD (current behavior preserved)"
    );
}
