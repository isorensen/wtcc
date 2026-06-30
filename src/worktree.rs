use std::path::{Component, Path, PathBuf};
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

/// argv (after `-C <repo>`) for creating a worktree on a NEW branch:
/// `["worktree", "add", "-b", <branch>, <path>]`. When `base` is set AND
/// non-empty it is appended as the FINAL discrete element so the new branch
/// forks from that ref instead of HEAD; `None` or `Some("")` reproduces today's
/// HEAD-based argv exactly. Every element stays a discrete argv arg (never a
/// shell string).
pub fn add_new_branch_argv(new_path: &Path, branch: &str, base: Option<&str>) -> Vec<String> {
    let mut argv = vec![
        "worktree".to_string(),
        "add".to_string(),
        "-b".to_string(),
        branch.to_string(),
        new_path.to_string_lossy().into_owned(),
    ];
    if let Some(base) = base.filter(|b| !b.is_empty()) {
        argv.push(base.to_string());
    }
    argv
}

/// Creates a worktree on a NEW branch via `git -C <repo>` + [`add_new_branch_argv`].
/// `branch` is the literal branch name (not slugified); the caller slugifies only
/// the derived filesystem `new_path`. `base`, when set, is the start-point ref.
pub fn add_new_branch(
    repo_path: &Path,
    new_path: &Path,
    branch: &str,
    base: Option<&str>,
) -> anyhow::Result<()> {
    let repo = repo_path.to_string_lossy();
    let output = Command::new("git")
        .args(["-C", &repo])
        .args(add_new_branch_argv(new_path, branch, base))
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

/// Outcome of a `copy_on_create` pass: how many sources were copied and how many
/// were skipped (invalid, missing, or already present at the destination).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CopyReport {
    pub copied: usize,
    pub skipped: usize,
}

/// Security seam for `copy_on_create`: validates that `rel` is a safe relative
/// path naming an existing FILE inside `repo_root`. Rejects (without panicking)
/// the empty string, absolute paths, and any `..`/parent-dir component up front;
/// then canonicalizes and requires the resolved source to stay inside the
/// canonical `repo_root` (so a symlink escaping the repo is rejected) and to be a
/// regular file. Returns the canonical source path on success.
pub(crate) fn validate_rel_copy_path(repo_root: &Path, rel: &str) -> Result<PathBuf, String> {
    if rel.is_empty() {
        return Err("empty path".to_string());
    }
    let rel_path = Path::new(rel);
    if rel_path.is_absolute() {
        return Err(format!("absolute path rejected: {rel}"));
    }
    for component in rel_path.components() {
        match component {
            Component::Normal(_) | Component::CurDir => {}
            _ => return Err(format!("invalid path component in: {rel}")),
        }
    }

    let canonical_root =
        std::fs::canonicalize(repo_root).map_err(|e| format!("cannot resolve repo root: {e}"))?;
    let canonical = std::fs::canonicalize(canonical_root.join(rel_path))
        .map_err(|e| format!("cannot resolve {rel}: {e}"))?;
    if !canonical.starts_with(&canonical_root) {
        return Err(format!("path escapes repo root: {rel}"));
    }
    if !canonical.is_file() {
        return Err(format!("not a file: {rel}"));
    }
    Ok(canonical)
}

/// Copies each validated `entry` from `repo_root` into the SAME relative location
/// under `new_path`, creating parent dirs as needed. Never overwrites an existing
/// destination (no-clobber) and never panics: invalid/missing sources and copy
/// failures are counted as skips. Pure `std::fs`, no subprocess.
pub fn copy_into_worktree(repo_root: &Path, new_path: &Path, entries: &[String]) -> CopyReport {
    let mut report = CopyReport::default();
    for rel in entries {
        let Ok(src) = validate_rel_copy_path(repo_root, rel) else {
            report.skipped += 1;
            continue;
        };
        let dest = new_path.join(rel);
        if let Some(parent) = dest.parent()
            && std::fs::create_dir_all(parent).is_err()
        {
            report.skipped += 1;
            continue;
        }
        // Atomic no-clobber: create_new fails with AlreadyExists if the
        // destination appears between validation and write, so a concurrent
        // writer can never be clobbered.
        let Ok(mut out) = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&dest)
        else {
            report.skipped += 1;
            continue;
        };
        let copied = std::fs::File::open(&src)
            .and_then(|mut input| std::io::copy(&mut input, &mut out))
            .is_ok();
        if copied {
            report.copied += 1;
        } else {
            report.skipped += 1;
        }
    }
    report
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

    // --- issue #54: per-repo base ref for NEW-branch worktrees --------------
    //
    // TDD RED: a new-branch worktree may fork from a configured base ref instead
    // of HEAD. `add_new_branch_argv(new_path, branch, base)` is the PURE argv seam
    // (mirrors `rename_branch_argv`): it returns the args AFTER `-C <repo>`, i.e.
    // `["worktree", "add", "-b", <branch>, <path>]`, with `<base>` appended as the
    // FINAL DISCRETE element ONLY when it is set AND non-empty. This pins
    // acceptance criterion #3 ("the base ref is a discrete argv element", never a
    // shell string) without spawning git.

    fn argv(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn add_new_branch_argv_with_base_appends_it_as_final_discrete_arg() {
        let got = add_new_branch_argv(
            Path::new("/repo/.worktrees/feat"),
            "feat",
            Some("origin/main"),
        );
        assert_eq!(
            got,
            argv(&[
                "worktree",
                "add",
                "-b",
                "feat",
                "/repo/.worktrees/feat",
                "origin/main",
            ])
        );
        assert_eq!(
            got.last().map(String::as_str),
            Some("origin/main"),
            "the base ref must be the final, discrete argv element"
        );
    }

    #[test]
    fn add_new_branch_argv_without_base_omits_it() {
        let got = add_new_branch_argv(Path::new("/repo/.worktrees/feat"), "feat", None);
        assert_eq!(
            got,
            argv(&["worktree", "add", "-b", "feat", "/repo/.worktrees/feat"]),
            "None must reproduce today's argv exactly (branches from HEAD)"
        );
    }

    #[test]
    fn add_new_branch_argv_empty_base_is_treated_as_unset() {
        // The contract is "set AND non-empty": a Some("") must not append an arg.
        let got = add_new_branch_argv(Path::new("/repo/.worktrees/feat"), "feat", Some(""));
        assert_eq!(
            got,
            argv(&["worktree", "add", "-b", "feat", "/repo/.worktrees/feat"]),
            "an empty base ref must be omitted, same as None"
        );
    }

    // --- issue #55: copy-on-create files into new worktrees -----------------
    //
    // TDD RED: copy a configured allowlist of ignored/untracked files (e.g.
    // `.env`) from the repo root into a freshly created worktree. The SECURITY
    // SEAM is `validate_rel_copy_path(repo_root, rel) -> Result<PathBuf, String>`:
    // it rejects absolute paths, any `..`/ParentDir component, and the empty
    // string OUTRIGHT, then canonicalizes and asserts the source is a FILE that
    // still resolves INSIDE `repo_root` (so a symlink escaping the repo is
    // rejected). `copy_into_worktree(repo_root, new_path, &entries) -> CopyReport`
    // copies only validated, existing files, NEVER clobbers an existing
    // destination, skips missing/invalid sources without panicking, creates parent
    // dirs for nested paths, and reports `{copied, skipped}` counts. std::fs only,
    // no subprocess.

    #[test]
    fn validate_rel_copy_path_accepts_simple_and_nested_relative_files() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join(".env"), b"SECRET=1").unwrap();
        std::fs::create_dir_all(root.path().join("config")).unwrap();
        std::fs::write(root.path().join("config/local.toml"), b"k = 1").unwrap();

        assert!(
            validate_rel_copy_path(root.path(), ".env").is_ok(),
            "a simple relative file must be accepted"
        );
        assert!(
            validate_rel_copy_path(root.path(), "config/local.toml").is_ok(),
            "a nested relative file must be accepted"
        );
    }

    #[test]
    fn validate_rel_copy_path_rejects_absolute_parent_and_empty() {
        let root = tempfile::tempdir().unwrap();

        assert!(
            validate_rel_copy_path(root.path(), "/etc/passwd").is_err(),
            "an absolute path must be rejected"
        );
        assert!(
            validate_rel_copy_path(root.path(), "../secret").is_err(),
            "a leading parent-dir escape must be rejected"
        );
        assert!(
            validate_rel_copy_path(root.path(), "a/../../b").is_err(),
            "any embedded parent-dir component must be rejected"
        );
        assert!(
            validate_rel_copy_path(root.path(), "").is_err(),
            "the empty string must be rejected"
        );
    }

    #[test]
    fn validate_rel_copy_path_rejects_a_directory_and_a_missing_source() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("adir")).unwrap();

        assert!(
            validate_rel_copy_path(root.path(), "adir").is_err(),
            "a directory must be rejected (files only)"
        );
        assert!(
            validate_rel_copy_path(root.path(), "missing.env").is_err(),
            "a missing source must be rejected, not panic"
        );
    }

    #[test]
    fn validate_rel_copy_path_rejects_a_symlink_resolving_outside_repo_root() {
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let secret = outside.path().join("secret");
        std::fs::write(&secret, b"TOP SECRET").unwrap();
        symlink(&secret, root.path().join("link.env")).unwrap();

        assert!(
            validate_rel_copy_path(root.path(), "link.env").is_err(),
            "a symlink that canonicalizes outside repo_root must be rejected"
        );
    }

    #[test]
    fn copy_into_worktree_copies_a_file_with_its_contents() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join(".env"), b"SECRET=1").unwrap();
        let wt = tempfile::tempdir().unwrap();

        let report = copy_into_worktree(root.path(), wt.path(), &[".env".to_string()]);

        let dest = wt.path().join(".env");
        assert_eq!(std::fs::read(&dest).unwrap(), b"SECRET=1");
        assert_eq!(report.copied, 1);
        assert_eq!(report.skipped, 0);
    }

    #[test]
    fn copy_into_worktree_creates_parent_dirs_for_a_nested_path() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("config")).unwrap();
        std::fs::write(root.path().join("config/local.toml"), b"k = 1").unwrap();
        let wt = tempfile::tempdir().unwrap();

        let report = copy_into_worktree(root.path(), wt.path(), &["config/local.toml".to_string()]);

        let dest = wt.path().join("config/local.toml");
        assert_eq!(std::fs::read(&dest).unwrap(), b"k = 1");
        assert_eq!(report.copied, 1);
    }

    #[test]
    fn copy_into_worktree_never_clobbers_an_existing_destination() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("app.conf"), b"NEW").unwrap();
        let wt = tempfile::tempdir().unwrap();
        std::fs::write(wt.path().join("app.conf"), b"OLD").unwrap();

        let report = copy_into_worktree(root.path(), wt.path(), &["app.conf".to_string()]);

        assert_eq!(
            std::fs::read(wt.path().join("app.conf")).unwrap(),
            b"OLD",
            "an existing destination must never be overwritten"
        );
        assert_eq!(report.copied, 0);
        assert_eq!(report.skipped, 1);
    }

    #[test]
    fn copy_into_worktree_skips_missing_and_invalid_sources_without_panicking() {
        let root = tempfile::tempdir().unwrap();
        let wt = tempfile::tempdir().unwrap();

        let report = copy_into_worktree(
            root.path(),
            wt.path(),
            &[
                "gone.env".to_string(),
                "/etc/passwd".to_string(),
                "../escape".to_string(),
            ],
        );

        assert_eq!(report.copied, 0);
        assert_eq!(
            report.skipped, 3,
            "every invalid/missing source must be skipped"
        );
        assert!(
            !wt.path().join("gone.env").exists(),
            "a missing source must not create a destination"
        );
        assert!(
            !wt.path().join("escape").exists(),
            "a parent-escape entry must never write anything"
        );
    }
}
