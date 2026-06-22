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

pub fn add(repo_path: &Path, new_path: &Path, branch: &str) -> anyhow::Result<()> {
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
