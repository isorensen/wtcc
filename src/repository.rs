use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Whether a registered target is a real git repository or a plain (non-git)
/// directory. A plain dir is modeled as a repo with one synthetic worktree that
/// IS the directory, so git-only actions gate off the single `is_git` predicate.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum RepoKind {
    #[default]
    Git,
    Plain,
}

impl RepoKind {
    pub fn is_git(&self) -> bool {
        matches!(self, RepoKind::Git)
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Repository {
    pub name: String,
    pub path: PathBuf,
    /// Git vs plain directory. Legacy configs and git repos omit this field
    /// (serde default `Git`); only plain dirs serialize `kind = "plain"`.
    #[serde(default, skip_serializing_if = "RepoKind::is_git")]
    pub kind: RepoKind,
    /// User-authored command run once (best-effort) in the new worktree after it
    /// is created. Absent in legacy configs and omitted from output when unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub setup: Option<String>,
    /// User-authored command run in the worktree just before it is removed.
    /// Absent in legacy configs and omitted from output when unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archive: Option<String>,
    /// Worktree paths soft-hidden from the sidebar. A pure UI/config marker: the
    /// worktree and its branch stay on disk. Absent in legacy configs and omitted
    /// from output when empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub archived: Vec<PathBuf>,
    /// Start-point for NEW-branch worktrees (e.g. `origin/main`). When set, a new
    /// branch forks from this ref instead of HEAD. Absent in legacy configs and
    /// omitted from output when unset; edited by hand.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_ref: Option<String>,
    /// Relative paths (from the repo root) copied into a new worktree on creation,
    /// e.g. `.env` or `config/local.toml` — git-ignored files that do not carry
    /// over. Absent in legacy configs and omitted from output when empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub copy_on_create: Vec<String>,
    /// User-authored command launched on a keypress into a dedicated Run tab,
    /// e.g. `pnpm dev` or `cargo test`. Run via `sh -c <command>` in the worktree
    /// dir. Absent in legacy configs and omitted from output when unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<String>,
}

#[derive(thiserror::Error, Debug)]
pub enum RegisterError {
    #[error("not a directory: {0}")]
    NotADirectory(PathBuf),
}

pub fn register(path: impl Into<PathBuf>) -> Result<Repository, RegisterError> {
    let path = path.into();

    if !path.is_dir() {
        return Err(RegisterError::NotADirectory(path));
    }

    let kind = if path.join(".git").exists() {
        RepoKind::Git
    } else {
        RepoKind::Plain
    };

    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();

    Ok(Repository {
        name,
        path,
        kind,
        setup: None,
        archive: None,
        archived: Vec::new(),
        base_ref: None,
        copy_on_create: Vec::new(),
        run: None,
    })
}

impl Repository {
    pub fn is_git(&self) -> bool {
        self.kind.is_git()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn register_without_git_dir_is_a_plain_repo() {
        let dir = tempfile::tempdir().unwrap();
        let repo = register(dir.path()).unwrap();
        assert_eq!(repo.kind, RepoKind::Plain);
        assert!(!repo.is_git());
    }

    #[test]
    fn register_with_git_dir_succeeds_and_uses_dir_name() {
        let parent = tempfile::tempdir().unwrap();
        let repo_path = parent.path().join("my-repo");
        fs::create_dir(&repo_path).unwrap();
        fs::create_dir(repo_path.join(".git")).unwrap();

        let repo = register(&repo_path).unwrap();
        assert_eq!(repo.name, "my-repo");
        assert_eq!(repo.path, repo_path);
        assert_eq!(repo.kind, RepoKind::Git);
        assert!(repo.is_git());
    }

    #[test]
    fn register_on_regular_file_is_not_a_directory() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("a-file");
        fs::write(&file_path, b"content").unwrap();

        let err = register(&file_path).unwrap_err();
        assert!(matches!(err, RegisterError::NotADirectory(_)));
    }
}
