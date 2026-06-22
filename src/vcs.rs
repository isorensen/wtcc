//! Per-worktree VCS status: git working-tree dirtiness plus, when a remote PR
//! exists for the branch, its PR state and CI rollup.
//!
//! All process spawning is argv-only (`Command::new("git"/"gh").args([...])`):
//! branch names and paths are untrusted and are NEVER interpolated into a shell
//! string. Any failure (no PR, `gh` missing, not authenticated, malformed
//! output) degrades to `pr: None` rather than surfacing an error to the user.

use std::path::Path;
use std::process::Command;

use crate::worktree::Worktree;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrState {
    Open,
    Merged,
    Closed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChecksState {
    Passing,
    Failing,
    Pending,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PrStatus {
    pub number: u64,
    pub state: PrState,
    pub checks: ChecksState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct VcsStatus {
    pub dirty: bool,
    pub pr: Option<PrStatus>,
}

/// Computes per-worktree VCS status. Implemented by `GitGhProvider` in
/// production and by fakes in tests, so `App` can be exercised without spawning
/// real `git`/`gh`.
pub trait VcsProvider: Send + Sync {
    fn status(&self, repo_path: &Path, worktree: &Worktree) -> VcsStatus;
}

pub struct GitGhProvider;

impl VcsProvider for GitGhProvider {
    fn status(&self, _repo_path: &Path, worktree: &Worktree) -> VcsStatus {
        VcsStatus {
            dirty: dirty(&worktree.path),
            pr: pr_status(&worktree.path, &worktree.branch),
        }
    }
}

/// Runs `git -C <wt> status --porcelain`; non-empty output means dirty. A
/// failure to spawn or a non-zero exit reports clean (not dirty) — the worst
/// case is an out-of-date badge, never a crash.
fn dirty(worktree_path: &Path) -> bool {
    let output = Command::new("git")
        .arg("-C")
        .arg(worktree_path)
        .args(["status", "--porcelain"])
        .output();
    match output {
        Ok(out) if out.status.success() => is_dirty(&String::from_utf8_lossy(&out.stdout)),
        _ => false,
    }
}

/// Non-empty porcelain output (ignoring blank lines) means the working tree is
/// dirty.
pub fn is_dirty(porcelain: &str) -> bool {
    porcelain.lines().any(|l| !l.trim().is_empty())
}

/// Queries `gh` for a PR on `branch`. Any error path — `gh` missing, not
/// authenticated, no PR, malformed JSON — yields `None`.
fn pr_status(worktree_path: &Path, branch: &str) -> Option<PrStatus> {
    if branch.is_empty() {
        return None;
    }
    let output = Command::new("gh")
        .args([
            "pr",
            "view",
            branch,
            "--json",
            "number,state,statusCheckRollup",
        ])
        .current_dir(worktree_path)
        .env("GH_NO_UPDATE_NOTIFIER", "1")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    parse_pr_json(&String::from_utf8_lossy(&output.stdout))
}

/// Pure parser: `gh pr view --json number,state,statusCheckRollup` output into a
/// `PrStatus`. Returns `None` for empty/malformed input or a missing `number`.
/// Defensive throughout: unknown shapes degrade to `None`/`ChecksState::None`
/// rather than erroring.
pub fn parse_pr_json(json: &str) -> Option<PrStatus> {
    let value: serde_json::Value = serde_json::from_str(json.trim()).ok()?;
    let number = value.get("number")?.as_u64()?;
    let state = match value.get("state").and_then(|s| s.as_str()) {
        Some("OPEN") => PrState::Open,
        Some("MERGED") => PrState::Merged,
        Some("CLOSED") => PrState::Closed,
        _ => return None,
    };
    let checks = parse_checks(value.get("statusCheckRollup"));
    Some(PrStatus {
        number,
        state,
        checks,
    })
}

/// Folds `gh`'s `statusCheckRollup` array into a single `ChecksState`.
///
/// Each entry is either a CheckRun (has `status`/`conclusion`) or a legacy
/// StatusContext (has `state`). Aggregation: any failure wins; else any pending
/// keeps it pending; else if there were checks at all, passing; else none.
fn parse_checks(rollup: Option<&serde_json::Value>) -> ChecksState {
    let Some(entries) = rollup.and_then(|r| r.as_array()) else {
        return ChecksState::None;
    };
    if entries.is_empty() {
        return ChecksState::None;
    }

    let mut any_pending = false;
    for entry in entries {
        match check_outcome(entry) {
            ChecksState::Failing => return ChecksState::Failing,
            ChecksState::Pending => any_pending = true,
            _ => {}
        }
    }
    if any_pending {
        ChecksState::Pending
    } else {
        ChecksState::Passing
    }
}

/// Classifies a single rollup entry. CheckRun: not-COMPLETED `status` is
/// pending; on COMPLETED, a `conclusion` outside the success set is failing.
/// StatusContext: `state` of SUCCESS/FAILURE/PENDING.
fn check_outcome(entry: &serde_json::Value) -> ChecksState {
    if let Some(status) = entry.get("status").and_then(|s| s.as_str()) {
        if status != "COMPLETED" {
            return ChecksState::Pending;
        }
        return match entry.get("conclusion").and_then(|c| c.as_str()) {
            Some("SUCCESS") | Some("NEUTRAL") | Some("SKIPPED") => ChecksState::Passing,
            _ => ChecksState::Failing,
        };
    }
    match entry.get("state").and_then(|s| s.as_str()) {
        Some("SUCCESS") => ChecksState::Passing,
        Some("PENDING") | Some("EXPECTED") => ChecksState::Pending,
        Some(_) => ChecksState::Failing,
        None => ChecksState::Pending,
    }
}

/// Compact sidebar suffix for a worktree's status, e.g. `* #42 ✓`. Empty when
/// the tree is clean and there is no PR. Kept short to fit a 34-col sidebar.
pub fn status_badge(status: &VcsStatus) -> String {
    let mut parts: Vec<String> = Vec::new();
    if status.dirty {
        parts.push("*".to_string());
    }
    if let Some(pr) = &status.pr {
        parts.push(format!("#{}", pr.number));
        if let Some(glyph) = pr_glyph(pr) {
            parts.push(glyph.to_string());
        }
    }
    parts.join(" ")
}

/// PR/CI glyph: a closed/merged PR shows its lifecycle marker; an open PR shows
/// its CI rollup. `None` means "nothing extra to draw".
fn pr_glyph(pr: &PrStatus) -> Option<&'static str> {
    match pr.state {
        PrState::Merged => Some("⇡"),
        PrState::Closed => Some("✗"),
        PrState::Open => match pr.checks {
            ChecksState::Passing => Some("✓"),
            ChecksState::Failing => Some("✗"),
            ChecksState::Pending => Some("…"),
            ChecksState::None => None,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn badge_empty_when_clean_and_no_pr() {
        assert_eq!(status_badge(&VcsStatus::default()), "");
    }

    #[test]
    fn badge_dirty_only() {
        assert_eq!(
            status_badge(&VcsStatus {
                dirty: true,
                pr: None,
            }),
            "*"
        );
    }

    #[test]
    fn badge_dirty_open_pr_passing() {
        let s = VcsStatus {
            dirty: true,
            pr: Some(PrStatus {
                number: 42,
                state: PrState::Open,
                checks: ChecksState::Passing,
            }),
        };
        assert_eq!(status_badge(&s), "* #42 ✓");
    }

    #[test]
    fn badge_merged_pr() {
        let s = VcsStatus {
            dirty: false,
            pr: Some(PrStatus {
                number: 7,
                state: PrState::Merged,
                checks: ChecksState::None,
            }),
        };
        assert_eq!(status_badge(&s), "#7 ⇡");
    }

    #[test]
    fn badge_open_pr_no_checks_omits_glyph() {
        let s = VcsStatus {
            dirty: false,
            pr: Some(PrStatus {
                number: 3,
                state: PrState::Open,
                checks: ChecksState::None,
            }),
        };
        assert_eq!(status_badge(&s), "#3");
    }

    #[test]
    fn is_dirty_detects_nonempty_porcelain() {
        assert!(is_dirty(" M src/main.rs\n"));
        assert!(is_dirty("?? new.txt"));
    }

    #[test]
    fn is_dirty_false_for_empty_or_blank() {
        assert!(!is_dirty(""));
        assert!(!is_dirty("\n  \n"));
    }

    #[test]
    fn parse_open_pr_with_passing_checks() {
        let json = r#"{
            "number": 42,
            "state": "OPEN",
            "statusCheckRollup": [
                {"status": "COMPLETED", "conclusion": "SUCCESS"},
                {"status": "COMPLETED", "conclusion": "SKIPPED"}
            ]
        }"#;
        assert_eq!(
            parse_pr_json(json),
            Some(PrStatus {
                number: 42,
                state: PrState::Open,
                checks: ChecksState::Passing,
            })
        );
    }

    #[test]
    fn parse_pr_with_failing_check_wins() {
        let json = r#"{
            "number": 7,
            "state": "OPEN",
            "statusCheckRollup": [
                {"status": "COMPLETED", "conclusion": "SUCCESS"},
                {"status": "COMPLETED", "conclusion": "FAILURE"},
                {"status": "IN_PROGRESS"}
            ]
        }"#;
        assert_eq!(parse_pr_json(json).unwrap().checks, ChecksState::Failing);
    }

    #[test]
    fn parse_pr_with_pending_check() {
        let json = r#"{
            "number": 7,
            "state": "OPEN",
            "statusCheckRollup": [
                {"status": "COMPLETED", "conclusion": "SUCCESS"},
                {"status": "QUEUED"}
            ]
        }"#;
        assert_eq!(parse_pr_json(json).unwrap().checks, ChecksState::Pending);
    }

    #[test]
    fn parse_pr_no_checks_is_none_checks() {
        let json = r#"{"number": 1, "state": "MERGED", "statusCheckRollup": []}"#;
        let pr = parse_pr_json(json).unwrap();
        assert_eq!(pr.state, PrState::Merged);
        assert_eq!(pr.checks, ChecksState::None);
    }

    #[test]
    fn parse_pr_missing_rollup_is_none_checks() {
        let json = r#"{"number": 1, "state": "CLOSED"}"#;
        let pr = parse_pr_json(json).unwrap();
        assert_eq!(pr.state, PrState::Closed);
        assert_eq!(pr.checks, ChecksState::None);
    }

    #[test]
    fn parse_legacy_status_context() {
        let json = r#"{
            "number": 9,
            "state": "OPEN",
            "statusCheckRollup": [{"state": "SUCCESS"}, {"state": "PENDING"}]
        }"#;
        assert_eq!(parse_pr_json(json).unwrap().checks, ChecksState::Pending);
    }

    #[test]
    fn stateless_entry_does_not_yield_passing() {
        let json = r#"{
            "number": 5,
            "state": "OPEN",
            "statusCheckRollup": [{}]
        }"#;
        let pr = parse_pr_json(json).unwrap();
        assert_ne!(pr.checks, ChecksState::Passing);
    }

    #[test]
    fn parse_empty_output_is_none() {
        assert_eq!(parse_pr_json(""), None);
        assert_eq!(parse_pr_json("   "), None);
    }

    #[test]
    fn parse_malformed_json_is_none() {
        assert_eq!(parse_pr_json("not json"), None);
        assert_eq!(parse_pr_json("{ broken"), None);
    }

    #[test]
    fn parse_missing_number_is_none() {
        assert_eq!(parse_pr_json(r#"{"state": "OPEN"}"#), None);
    }

    #[test]
    fn parse_unknown_state_is_none() {
        assert_eq!(parse_pr_json(r#"{"number": 5, "state": "DRAFT"}"#), None);
    }

    #[test]
    fn dirty_on_real_temp_repo() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path();
        let run = |args: &[&str]| {
            Command::new("git")
                .args(args)
                .current_dir(path)
                .output()
                .unwrap()
        };
        run(&["init", "-q"]);
        run(&["config", "user.email", "t@t.com"]);
        run(&["config", "user.name", "t"]);
        std::fs::write(path.join("a.txt"), "hi").unwrap();
        run(&["add", "a.txt"]);
        run(&["commit", "-qm", "init"]);

        assert!(!dirty(path));
        std::fs::write(path.join("a.txt"), "changed").unwrap();
        assert!(dirty(path));
    }
}
