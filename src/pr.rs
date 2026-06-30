//! GitHub PR *write* actions via `gh`: open-in-browser, mark-ready, merge, close.
//!
//! This complements `vcs`'s read-only PR status. Every action is built as a pure
//! argv vector (the program is always `gh`, supplied by the spawn site) so it can
//! be unit-tested exactly. SECURITY: the branch is ALWAYS a single, discrete argv
//! element — it is never interpolated into or split by a shell string. The actual
//! spawn ([`run_gh`]) is intentionally thin and untested; a missing `gh` or a
//! non-zero exit degrades to an `Err` carrying the reason rather than panicking.

use std::path::Path;
use std::process::Command;

use serde::{Deserialize, Serialize};

/// How a PR is merged. Maps to the corresponding `gh pr merge` flag. The default
/// (`Squash`) matches wtcc's one-commit-per-branch workflow.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MergeStrategy {
    #[default]
    Squash,
    Merge,
    Rebase,
}

impl MergeStrategy {
    /// The `gh pr merge` flag selecting this strategy.
    pub fn strategy_flag(self) -> &'static str {
        match self {
            MergeStrategy::Squash => "--squash",
            MergeStrategy::Merge => "--merge",
            MergeStrategy::Rebase => "--rebase",
        }
    }
}

/// `gh pr view <branch> --web` — opens the PR for `branch` in the browser.
pub fn open_in_browser_argv(branch: &str) -> Vec<String> {
    vec![
        "pr".to_string(),
        "view".to_string(),
        branch.to_string(),
        "--web".to_string(),
    ]
}

/// `gh pr ready <branch>` — marks the draft PR for `branch` ready for review.
pub fn mark_ready_argv(branch: &str) -> Vec<String> {
    vec!["pr".to_string(), "ready".to_string(), branch.to_string()]
}

/// `gh pr close <branch>` — closes the PR for `branch`.
pub fn close_argv(branch: &str) -> Vec<String> {
    vec!["pr".to_string(), "close".to_string(), branch.to_string()]
}

/// `gh pr merge <branch> --<strategy>` — merges the PR for `branch`. Never passes
/// `--delete-branch`: wtcc owns the worktree/branch lifecycle.
pub fn merge_argv(branch: &str, strategy: MergeStrategy) -> Vec<String> {
    vec![
        "pr".to_string(),
        "merge".to_string(),
        branch.to_string(),
        strategy.strategy_flag().to_string(),
    ]
}

/// Spawns `gh` with `argv` in `cwd` and reports the outcome. Thin and untested:
/// a spawn failure (e.g. `gh` not installed) or a non-zero exit yields an `Err`
/// carrying the first non-empty stderr line (or the spawn error), never a panic.
pub fn run_gh(argv: &[String], cwd: &Path) -> Result<(), String> {
    let out = Command::new("gh")
        .args(argv)
        .current_dir(cwd)
        .env("GH_NO_UPDATE_NOTIFIER", "1")
        .output()
        .map_err(|e| format!("could not run gh: {e}"))?;
    if out.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    let reason = stderr
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("gh exited with a non-zero status");
    Err(reason.to_string())
}
