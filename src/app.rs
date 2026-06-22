use std::path::PathBuf;

use crate::config::Config;
use crate::session::SessionManager;
use crate::worktree::{self, Worktree};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Sidebar,
    Agent,
}

/// What the user is being prompted for while an inline input overlay is open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Prompt {
    AddWorktree,
}

/// What the user is being asked to confirm while a confirm overlay is open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Confirm {
    RemoveWorktree(PathBuf),
}

/// The active modal overlay, if any. Only one overlay is open at a time.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Overlay {
    #[default]
    None,
    Palette {
        query: String,
        selected: usize,
    },
    Input {
        prompt: Prompt,
        buffer: String,
    },
    Confirm(Confirm),
}

pub struct App {
    pub config: Config,
    pub selected_repo: Option<usize>,
    pub worktrees: Vec<Worktree>,
    pub selected_worktree: Option<usize>,
    pub focus: Focus,
    pub overlay: Overlay,
    pub status: Option<String>,
    pub should_quit: bool,
    pub session_manager: SessionManager,
    pub active_session: Option<String>,
}

impl App {
    pub fn new(config: Config) -> App {
        let selected_repo = (!config.repos.is_empty()).then_some(0);
        let mut app = App {
            config,
            selected_repo,
            worktrees: Vec::new(),
            selected_worktree: None,
            focus: Focus::Sidebar,
            overlay: Overlay::None,
            status: None,
            should_quit: false,
            session_manager: SessionManager::new(),
            active_session: None,
        };
        if selected_repo.is_some() {
            app.refresh_worktrees();
        }
        app
    }

    pub fn selected_repo_path(&self) -> Option<&std::path::Path> {
        self.selected_repo
            .and_then(|i| self.config.repos.get(i))
            .map(|r| r.path.as_path())
    }

    pub fn current_worktree(&self) -> Option<&Worktree> {
        self.selected_worktree.and_then(|i| self.worktrees.get(i))
    }

    pub fn select_repo(&mut self, index: usize) {
        if index >= self.config.repos.len() {
            return;
        }
        self.selected_repo = Some(index);
        self.refresh_worktrees();
    }

    /// Reloads the worktree list for the selected repo. Domain errors are
    /// captured into `status` rather than panicking.
    pub fn refresh_worktrees(&mut self) {
        let Some(path) = self.selected_repo_path().map(|p| p.to_path_buf()) else {
            self.worktrees.clear();
            self.selected_worktree = None;
            return;
        };
        match worktree::list(&path) {
            Ok(list) => {
                self.worktrees = list;
                self.selected_worktree = (!self.worktrees.is_empty()).then_some(0);
                self.status = None;
            }
            Err(e) => {
                self.worktrees.clear();
                self.selected_worktree = None;
                self.status = Some(format!("worktree list failed: {e}"));
            }
        }
    }

    pub fn next(&mut self) {
        match self.focus {
            Focus::Sidebar => self.next_worktree(),
            Focus::Agent => {}
        }
    }

    pub fn prev(&mut self) {
        match self.focus {
            Focus::Sidebar => self.prev_worktree(),
            Focus::Agent => {}
        }
    }

    fn next_worktree(&mut self) {
        if self.worktrees.is_empty() {
            return;
        }
        let i = self.selected_worktree.map_or(0, |i| {
            if i + 1 >= self.worktrees.len() {
                0
            } else {
                i + 1
            }
        });
        self.selected_worktree = Some(i);
    }

    fn prev_worktree(&mut self) {
        if self.worktrees.is_empty() {
            return;
        }
        let i = self.selected_worktree.map_or(0, |i| {
            if i == 0 {
                self.worktrees.len() - 1
            } else {
                i - 1
            }
        });
        self.selected_worktree = Some(i);
    }

    /// Lazily spawns (or reuses) the agent session for the current worktree and
    /// records its name in `active_session`. Spawn errors land in `status`.
    pub fn ensure_active_session(&mut self, rows: u16, cols: u16) {
        let Some(wt) = self.current_worktree() else {
            self.active_session = None;
            return;
        };
        let branch = wt.branch.clone();
        let path = wt.path.clone();
        let name = SessionManager::session_name(&branch);
        match self
            .session_manager
            .ensure(&branch, &path, &self.config.agent_cmd, rows, cols)
        {
            Ok(_) => self.active_session = Some(name),
            Err(e) => self.status = Some(format!("agent spawn failed: {e}")),
        }
    }

    pub fn toggle_focus(&mut self) {
        self.focus = match self.focus {
            Focus::Sidebar => Focus::Agent,
            Focus::Agent => Focus::Sidebar,
        };
    }

    /// Cycle to the next registered repo (used by the palette "Switch repo").
    pub fn cycle_repo(&mut self) {
        if self.config.repos.is_empty() {
            return;
        }
        let next = self
            .selected_repo
            .map_or(0, |i| (i + 1) % self.config.repos.len());
        self.select_repo(next);
    }

    pub fn add_worktree(&mut self, branch: &str) {
        let branch = branch.trim();
        if branch.is_empty() {
            self.status = Some("branch name cannot be empty".to_string());
            return;
        }
        let Some(repo) = self.selected_repo_path().map(|p| p.to_path_buf()) else {
            self.status = Some("no repo selected".to_string());
            return;
        };
        let slug = worktree::slugify(branch);
        let new_path = repo.join(".worktrees").join(&slug);
        match worktree::add(&repo, &new_path, branch) {
            Ok(()) => {
                self.status = Some(format!("added worktree {branch}"));
                self.refresh_worktrees();
            }
            Err(e) => self.status = Some(format!("add failed: {e}")),
        }
    }

    pub fn remove_worktree(&mut self, path: &std::path::Path) {
        let Some(repo) = self.selected_repo_path().map(|p| p.to_path_buf()) else {
            self.status = Some("no repo selected".to_string());
            return;
        };
        match worktree::remove(&repo, path) {
            Ok(()) => {
                self.status = Some("removed worktree".to_string());
                self.refresh_worktrees();
            }
            Err(e) => self.status = Some(format!("remove failed: {e}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repository::Repository;

    fn config_with_repo() -> Config {
        Config {
            repos: vec![Repository {
                name: "demo".to_string(),
                path: PathBuf::from("/tmp/does-not-exist-demo"),
            }],
            agent_cmd: "claude".to_string(),
        }
    }

    fn app_with_fake_worktrees() -> App {
        // Build without touching git, then inject worktrees directly.
        let mut app = App {
            config: config_with_repo(),
            selected_repo: Some(0),
            worktrees: vec![
                Worktree {
                    path: PathBuf::from("/repo/main"),
                    branch: "main".to_string(),
                    head: "abc123".to_string(),
                    is_bare: false,
                    is_detached: false,
                },
                Worktree {
                    path: PathBuf::from("/repo/feat"),
                    branch: "feat".to_string(),
                    head: "def456".to_string(),
                    is_bare: false,
                    is_detached: false,
                },
            ],
            selected_worktree: Some(0),
            focus: Focus::Sidebar,
            overlay: Overlay::None,
            status: None,
            should_quit: false,
            session_manager: SessionManager::new(),
            active_session: None,
        };
        app.selected_worktree = Some(0);
        app
    }

    #[test]
    fn next_prev_wrap_around() {
        let mut app = app_with_fake_worktrees();
        assert_eq!(app.selected_worktree, Some(0));
        app.next();
        assert_eq!(app.selected_worktree, Some(1));
        app.next();
        assert_eq!(app.selected_worktree, Some(0));
        app.prev();
        assert_eq!(app.selected_worktree, Some(1));
    }

    #[test]
    fn toggle_focus_round_trips() {
        let mut app = app_with_fake_worktrees();
        assert_eq!(app.focus, Focus::Sidebar);
        app.toggle_focus();
        assert_eq!(app.focus, Focus::Agent);
        app.toggle_focus();
        assert_eq!(app.focus, Focus::Sidebar);
    }

    #[test]
    fn navigation_noop_when_empty() {
        let mut app = app_with_fake_worktrees();
        app.worktrees.clear();
        app.selected_worktree = None;
        app.next();
        app.prev();
        assert_eq!(app.selected_worktree, None);
    }

    #[test]
    fn add_with_empty_branch_sets_status() {
        let mut app = app_with_fake_worktrees();
        app.add_worktree("   ");
        assert_eq!(app.status.as_deref(), Some("branch name cannot be empty"));
    }
}
