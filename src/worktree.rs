use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone, PartialEq)]
pub struct Worktree {
    pub path: PathBuf,
    pub branch: String,
    pub head: String,
    pub is_bare: bool,
    pub is_detached: bool,
}

pub fn parse_worktree_list(output: &str) -> Vec<Worktree> {
    let mut worktrees = Vec::new();
    let mut current: Option<Worktree> = None;

    for line in output.lines() {
        if line.is_empty() {
            if let Some(wt) = current.take() {
                worktrees.push(wt);
            }
            continue;
        }

        if let Some(path) = line.strip_prefix("worktree ") {
            if let Some(wt) = current.take() {
                worktrees.push(wt);
            }
            current = Some(Worktree {
                path: PathBuf::from(path),
                branch: String::new(),
                head: String::new(),
                is_bare: false,
                is_detached: false,
            });
        } else if let Some(wt) = current.as_mut() {
            if let Some(head) = line.strip_prefix("HEAD ") {
                wt.head = head.to_string();
            } else if let Some(branch) = line.strip_prefix("branch ") {
                wt.branch = branch
                    .strip_prefix("refs/heads/")
                    .unwrap_or(branch)
                    .to_string();
            } else if line == "bare" {
                wt.is_bare = true;
            } else if line == "detached" {
                wt.is_detached = true;
            }
        }
    }

    if let Some(wt) = current.take() {
        worktrees.push(wt);
    }

    worktrees
}

pub fn slugify(name: &str) -> String {
    let mut slug = String::with_capacity(name.len());
    let mut prev_dash = false;

    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
            prev_dash = false;
        } else if !prev_dash {
            slug.push('-');
            prev_dash = true;
        }
    }

    slug.trim_matches('-').to_string()
}

pub fn list(repo_path: &Path) -> anyhow::Result<Vec<Worktree>> {
    let repo = repo_path.to_string_lossy();
    let output = Command::new("git")
        .args(["-C", &repo, "worktree", "list", "--porcelain"])
        .output()?;

    if !output.status.success() {
        anyhow::bail!(
            "git worktree list failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    Ok(parse_worktree_list(&String::from_utf8_lossy(
        &output.stdout,
    )))
}

/// Reports whether `name` resolves to an existing branch (local or remote) in
/// the repo. Uses `git rev-parse --verify --quiet` against a concrete ref so the
/// untrusted `name` is never interpreted as a flag or shell input; a non-zero
/// exit (unknown ref) maps to `false` rather than an error.
pub fn branch_exists(repo_path: &Path, name: &str) -> bool {
    let repo = repo_path.to_string_lossy();
    let local = format!("refs/heads/{name}");
    if ref_resolves(&repo, &local) {
        return true;
    }
    let remote = format!("refs/remotes/{name}");
    ref_resolves(&repo, &remote)
}

fn ref_resolves(repo: &str, reference: &str) -> bool {
    Command::new("git")
        .args(["-C", repo, "rev-parse", "--verify", "--quiet", reference])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Creates a worktree on a NEW branch: `git worktree add -b <branch> <path>`.
/// `branch` is passed as a discrete argv arg (the literal branch name, not
/// slugified); the caller slugifies only the derived filesystem `new_path`.
pub fn add_new_branch(repo_path: &Path, new_path: &Path, branch: &str) -> anyhow::Result<()> {
    let repo = repo_path.to_string_lossy();
    let new = new_path.to_string_lossy();
    let output = Command::new("git")
        .args(["-C", &repo, "worktree", "add", "-b", branch, &new])
        .output()?;

    if !output.status.success() {
        anyhow::bail!(
            "git worktree add failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    Ok(())
}

/// Creates a worktree checking out an EXISTING branch:
/// `git worktree add <path> <branch>`. Used to review a PR or resume work on a
/// branch that already exists locally or on a remote.
pub fn add_existing_branch(repo_path: &Path, new_path: &Path, branch: &str) -> anyhow::Result<()> {
    let repo = repo_path.to_string_lossy();
    let new = new_path.to_string_lossy();
    let output = Command::new("git")
        .args(["-C", &repo, "worktree", "add", &new, branch])
        .output()?;

    if !output.status.success() {
        anyhow::bail!(
            "git worktree add failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    Ok(())
}

/// argv for renaming a branch in place: `git branch -m <old> <new>`. Both names
/// are discrete argv elements — the new name is the user's text passed verbatim
/// (never slugified, never interpolated into a shell string).
pub fn rename_branch_argv(old: &str, new: &str) -> Vec<String> {
    vec![
        "branch".to_string(),
        "-m".to_string(),
        old.to_string(),
        new.to_string(),
    ]
}

/// Renames the branch `old` to `new` in place via `git -C <repo> branch -m`.
/// `git branch -m` only renames the ref, so the worktree directory does NOT move
/// and path-keyed state stays valid. A non-zero exit (e.g. a name collision)
/// surfaces as an error rather than panicking.
pub fn rename_branch(repo_path: &Path, old: &str, new: &str) -> anyhow::Result<()> {
    let repo = repo_path.to_string_lossy();
    let output = Command::new("git")
        .args(["-C", &repo])
        .args(rename_branch_argv(old, new))
        .output()?;

    if !output.status.success() {
        anyhow::bail!(
            "git branch -m failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    Ok(())
}

pub fn remove(repo_path: &Path, worktree_path: &Path) -> anyhow::Result<()> {
    let repo = repo_path.to_string_lossy();
    let wt = worktree_path.to_string_lossy();
    let output = Command::new("git")
        .args(["-C", &repo, "worktree", "remove", &wt])
        .output()?;

    if !output.status.success() {
        anyhow::bail!(
            "git worktree remove failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_normal_detached_and_bare() {
        let output = "\
worktree /repo/main
HEAD abc123
branch refs/heads/main

worktree /repo/detached-wt
HEAD def456
detached

worktree /repo/bare
bare
";

        let parsed = parse_worktree_list(output);

        assert_eq!(
            parsed,
            vec![
                Worktree {
                    path: PathBuf::from("/repo/main"),
                    branch: "main".to_string(),
                    head: "abc123".to_string(),
                    is_bare: false,
                    is_detached: false,
                },
                Worktree {
                    path: PathBuf::from("/repo/detached-wt"),
                    branch: String::new(),
                    head: "def456".to_string(),
                    is_bare: false,
                    is_detached: true,
                },
                Worktree {
                    path: PathBuf::from("/repo/bare"),
                    branch: String::new(),
                    head: String::new(),
                    is_bare: true,
                    is_detached: false,
                },
            ]
        );
    }

    #[test]
    fn parse_handles_final_block_without_trailing_newline() {
        let output = "worktree /repo/main\nHEAD abc123\nbranch refs/heads/main";
        let parsed = parse_worktree_list(output);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].branch, "main");
    }

    #[test]
    fn slugify_example() {
        assert_eq!(slugify("Feature/Foo Bar!!"), "feature-foo-bar");
    }

    #[test]
    fn slugify_collapses_trims_and_lowercases() {
        assert_eq!(slugify("___Hello___World___"), "hello-world");
        assert_eq!(slugify("Multiple   Spaces"), "multiple-spaces");
        assert_eq!(slugify("UPPER"), "upper");
        assert_eq!(slugify("!!!"), "");
    }
}
