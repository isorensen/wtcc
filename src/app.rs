use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver};

use crate::config::Config;
use crate::session::{ActivityState, SessionManager};
use crate::vcs::{GitGhProvider, VcsProvider, VcsStatus};
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
    AddRepo,
}

/// What the user is being asked to confirm while a confirm overlay is open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Confirm {
    RemoveWorktree(PathBuf),
    RemoveRepo(usize),
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
    Help,
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
    /// When set, config is persisted here instead of the default XDG path.
    /// Used by tests to redirect writes into a temp directory.
    pub config_path: Option<PathBuf>,
    /// Cached VCS status per worktree path, filled asynchronously by a worker
    /// thread (see `spawn_vcs_refresh`). Absent entries render as "not loaded".
    pub vcs_status: HashMap<PathBuf, VcsStatus>,
    /// Computes per-worktree status off the UI thread. Boxed behind a trait so
    /// tests can inject a fake provider.
    pub vcs_provider: Arc<dyn VcsProvider>,
    /// Receiver for the in-flight VCS refresh worker, if any. Replaced on each
    /// refresh; results from a superseded worker are simply never drained.
    /// Replacing this `Receiver` drops it, so the orphaned worker's `Sender::send` returns `Err` and the thread exits.
    vcs_rx: Option<Receiver<(PathBuf, VcsStatus)>>,
}

/// Expands a leading `~` to the home directory and resolves relative paths
/// against the current working directory. Pure path manipulation: it does not
/// touch the filesystem beyond reading `$HOME`/cwd, so a non-existent path
/// still round-trips (the dir check happens in `repository::register`).
///
/// Returns `Err` for the `~user` form, which is not supported.
fn expand_path(input: &str) -> Result<PathBuf, String> {
    if input.starts_with('~') && input != "~" && !input.starts_with("~/") {
        return Err("unsupported ~user path; use an absolute path or ~/".to_string());
    }
    let tilde = (input == "~")
        .then_some("")
        .or_else(|| input.strip_prefix("~/"));
    if let Some(rest) = tilde
        && let Some(home) = dirs::home_dir()
    {
        return Ok(home.join(rest));
    }
    let path = PathBuf::from(input);
    if path.is_absolute() {
        return Ok(path);
    }
    match std::env::current_dir() {
        Ok(cwd) => Ok(cwd.join(path)),
        Err(_) => Ok(path),
    }
}

impl App {
    pub fn new(config: Config) -> App {
        Self::with_provider(config, Arc::new(GitGhProvider))
    }

    /// Constructs an `App` with an injected `VcsProvider`. Production uses
    /// `GitGhProvider` via `new`; tests pass a fake to exercise caching and
    /// error handling without spawning `git`/`gh`.
    pub(crate) fn with_provider(config: Config, vcs_provider: Arc<dyn VcsProvider>) -> App {
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
            config_path: None,
            vcs_status: HashMap::new(),
            vcs_provider,
            vcs_rx: None,
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

    /// Activity state of the agent for `branch`, mapped through the
    /// `wtcc-<slug>` session name. `None` when no session has been spawned for
    /// that worktree. Cheap enough to call per worktree each frame.
    pub fn worktree_activity(&self, branch: &str) -> ActivityState {
        self.session_manager
            .activity(&SessionManager::session_name(branch))
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
        self.spawn_vcs_refresh();
    }

    /// Spawns a worker thread that computes `VcsStatus` for every current
    /// worktree and streams results back over a channel. Kept off the UI thread
    /// because `gh` can take seconds. A previously in-flight worker is dropped:
    /// its sender's results are simply never drained. Stale cache entries (for
    /// removed worktrees) are pruned up front.
    pub fn spawn_vcs_refresh(&mut self) {
        let Some(repo) = self.selected_repo_path().map(|p| p.to_path_buf()) else {
            self.vcs_rx = None;
            return;
        };
        let live: std::collections::HashSet<PathBuf> =
            self.worktrees.iter().map(|w| w.path.clone()).collect();
        self.vcs_status.retain(|k, _| live.contains(k));

        let worktrees = self.worktrees.clone();
        let provider = Arc::clone(&self.vcs_provider);
        let (tx, rx) = mpsc::channel();
        self.vcs_rx = Some(rx);

        std::thread::spawn(move || {
            for wt in &worktrees {
                let status = provider.status(&repo, wt);
                if tx.send((wt.path.clone(), status)).is_err() {
                    break;
                }
            }
        });
    }

    /// Drains any VCS results produced since the last call into the cache.
    /// Non-blocking; called once per frame by the main loop.
    pub fn drain_vcs(&mut self) {
        let Some(rx) = &self.vcs_rx else {
            return;
        };
        let updates: Vec<(PathBuf, VcsStatus)> = rx.try_iter().collect();
        for (path, status) in updates {
            self.vcs_status.insert(path, status);
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

    /// Registers a repository from a user-entered path: expands `~` and resolves
    /// relative paths against the current directory, validates it is a git repo,
    /// then persists, selects it, and loads its worktrees. All failure modes
    /// (bad path, not a git repo, save error) land in `status` — never panics.
    pub fn register_repository(&mut self, input: &str) {
        let input = input.trim();
        if input.is_empty() {
            self.status = Some("path cannot be empty".to_string());
            return;
        }
        let expanded = match expand_path(input) {
            Ok(p) => p,
            Err(e) => {
                self.status = Some(e);
                return;
            }
        };
        let resolved = std::fs::canonicalize(&expanded).unwrap_or(expanded);
        let repo = match crate::repository::register(resolved) {
            Ok(repo) => repo,
            Err(e) => {
                self.status = Some(format!("register failed: {e}"));
                return;
            }
        };
        if self.config.repos.iter().any(|r| r.path == repo.path) {
            self.status = Some(format!("repo already registered: {}", repo.name));
            return;
        }
        let name = repo.name.clone();
        self.config.repos.push(repo);
        let save = match &self.config_path {
            Some(path) => self.config.save_to(path),
            None => self.config.save(),
        };
        if let Err(e) = save {
            self.config.repos.pop();
            self.status = Some(format!("save failed: {e}"));
            return;
        }
        self.select_repo(self.config.repos.len() - 1);
        self.status = Some(format!("registered repo {name}"));
    }

    /// Unregisters the repository at `index` from the config. This only edits
    /// wtcc's config — it never deletes anything on disk. The removed entry is
    /// restored if the persist step fails. On success, selection moves to the
    /// previous neighbor (clamped), or `None` when the list is now empty, and
    /// its worktrees are reloaded.
    pub fn remove_repository(&mut self, index: usize) {
        if index >= self.config.repos.len() {
            return;
        }
        let removed = self.config.repos.remove(index);
        let save = match &self.config_path {
            Some(path) => self.config.save_to(path),
            None => self.config.save(),
        };
        if let Err(e) = save {
            self.config.repos.insert(index, removed);
            self.status = Some(format!("save failed: {e}"));
            return;
        }
        let name = removed.name;
        if self.config.repos.is_empty() {
            self.selected_repo = None;
            self.refresh_worktrees();
        } else {
            self.select_repo(index.saturating_sub(1));
        }
        self.status = Some(format!("unregistered repo {name}"));
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
    use crate::vcs::{ChecksState, PrState, PrStatus};
    use std::sync::atomic::{AtomicBool, Ordering};

    /// Returns a fixed dirty/PR status for every worktree.
    struct FakeProvider {
        status: VcsStatus,
    }
    impl VcsProvider for FakeProvider {
        fn status(&self, _repo: &std::path::Path, _wt: &Worktree) -> VcsStatus {
            self.status
        }
    }

    /// Yields a clean/no-PR status but flips a flag, proving the worker ran even
    /// when it reports "nothing interesting" (the App-error analogue: a provider
    /// that returns default leaves `vcs_status` populated, never unset/panicking).
    struct FlagProvider {
        called: Arc<AtomicBool>,
    }
    impl VcsProvider for FlagProvider {
        fn status(&self, _repo: &std::path::Path, _wt: &Worktree) -> VcsStatus {
            self.called.store(true, Ordering::SeqCst);
            VcsStatus::default()
        }
    }

    fn drain_until<F: Fn(&App) -> bool>(app: &mut App, done: F) -> bool {
        for _ in 0..200 {
            app.drain_vcs();
            if done(app) {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        false
    }

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
            config_path: None,
            vcs_status: HashMap::new(),
            vcs_provider: Arc::new(GitGhProvider),
            vcs_rx: None,
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

    /// Builds an App with two repos and a redirected `config_path`, bypassing
    /// git by constructing fields directly (mirrors `app_with_fake_worktrees`).
    fn app_with_two_repos(repo_a: PathBuf, repo_b: PathBuf, config_path: PathBuf) -> App {
        App {
            config: Config {
                repos: vec![
                    Repository {
                        name: "repo-a".to_string(),
                        path: repo_a,
                    },
                    Repository {
                        name: "repo-b".to_string(),
                        path: repo_b,
                    },
                ],
                agent_cmd: "claude".to_string(),
            },
            selected_repo: Some(1),
            worktrees: Vec::new(),
            selected_worktree: None,
            focus: Focus::Sidebar,
            overlay: Overlay::None,
            status: None,
            should_quit: false,
            session_manager: SessionManager::new(),
            active_session: None,
            config_path: Some(config_path),
            vcs_status: HashMap::new(),
            vcs_provider: Arc::new(GitGhProvider),
            vcs_rx: None,
        }
    }

    #[test]
    fn remove_repository_unregisters_persists_and_reselects_neighbor() {
        let dir = tempfile::tempdir().unwrap();
        let repo_a = dir.path().join("repo-a");
        std::fs::create_dir(&repo_a).unwrap();
        let config_path = dir.path().join("config.toml");
        let mut app = app_with_two_repos(
            repo_a.clone(),
            PathBuf::from("/tmp/does-not-exist-repo-b"),
            config_path.clone(),
        );

        app.remove_repository(1);

        assert_eq!(app.config.repos.len(), 1);
        assert_eq!(app.config.repos[0].name, "repo-a");
        assert_eq!(app.selected_repo, Some(0));

        let persisted = Config::load_from(&config_path).unwrap();
        assert_eq!(persisted.repos.len(), 1);
        assert_eq!(persisted.repos[0].name, "repo-a");

        // Unregister must NOT delete the repo on disk.
        assert!(repo_a.exists(), "on-disk repo dir must survive unregister");
    }

    #[test]
    fn remove_repository_to_empty_clears_selection() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.toml");
        let mut app = app_with_two_repos(
            PathBuf::from("/tmp/does-not-exist-a"),
            PathBuf::from("/tmp/does-not-exist-b"),
            config_path,
        );

        app.remove_repository(1);
        app.remove_repository(0);

        assert!(app.config.repos.is_empty());
        assert_eq!(app.selected_repo, None);
        assert!(app.worktrees.is_empty());
    }

    #[test]
    fn remove_repository_out_of_bounds_is_noop() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.toml");
        let mut app = app_with_two_repos(
            PathBuf::from("/tmp/a"),
            PathBuf::from("/tmp/b"),
            config_path,
        );
        app.remove_repository(9);
        assert_eq!(app.config.repos.len(), 2);
    }

    #[test]
    fn add_with_empty_branch_sets_status() {
        let mut app = app_with_fake_worktrees();
        app.add_worktree("   ");
        assert_eq!(app.status.as_deref(), Some("branch name cannot be empty"));
    }

    #[test]
    fn expand_path_tilde_slash_joins_home_dir() {
        if let Some(home) = dirs::home_dir() {
            let result = expand_path("~/myrepo").expect("tilde-slash should expand without error");
            assert_eq!(result, home.join("myrepo"));
        }
    }

    #[test]
    fn expand_path_tilde_user_returns_err() {
        assert!(expand_path("~otheruser/foo").is_err());
    }

    #[test]
    fn vcs_refresh_caches_provider_results() {
        let pr = PrStatus {
            number: 42,
            state: PrState::Open,
            checks: ChecksState::Passing,
        };
        let mut app = app_with_fake_worktrees();
        app.vcs_provider = Arc::new(FakeProvider {
            status: VcsStatus {
                dirty: true,
                pr: Some(pr),
            },
        });
        app.spawn_vcs_refresh();
        assert!(
            drain_until(&mut app, |a| a.vcs_status.len() == a.worktrees.len()),
            "vcs worker did not deliver in time"
        );

        assert_eq!(app.vcs_status.len(), 2);
        let main = app.vcs_status.get(&PathBuf::from("/repo/main")).unwrap();
        assert!(main.dirty);
        assert_eq!(main.pr, Some(pr));
    }

    #[test]
    fn vcs_refresh_with_default_status_leaves_cache_populated_not_unset() {
        let called = Arc::new(AtomicBool::new(false));
        let mut app = app_with_fake_worktrees();
        app.vcs_provider = Arc::new(FlagProvider {
            called: Arc::clone(&called),
        });
        app.spawn_vcs_refresh();
        assert!(
            drain_until(&mut app, |a| a.vcs_status.len() == a.worktrees.len()),
            "vcs worker did not deliver in time"
        );

        assert!(called.load(Ordering::SeqCst));
        let main = app.vcs_status.get(&PathBuf::from("/repo/main")).unwrap();
        assert!(!main.dirty);
        assert_eq!(main.pr, None);
    }

    #[test]
    fn vcs_refresh_prunes_stale_entries_for_removed_worktrees() {
        let mut app = app_with_fake_worktrees();
        app.vcs_status
            .insert(PathBuf::from("/repo/gone"), VcsStatus::default());
        app.vcs_provider = Arc::new(FakeProvider {
            status: VcsStatus::default(),
        });
        app.spawn_vcs_refresh();
        assert!(!app.vcs_status.contains_key(&PathBuf::from("/repo/gone")));
    }

    #[test]
    fn drain_vcs_is_noop_without_refresh() {
        let mut app = app_with_fake_worktrees();
        app.drain_vcs();
        assert!(app.vcs_status.is_empty());
    }

    #[test]
    fn superseded_worker_results_never_appear_in_cache() {
        // First provider returns dirty=true; second returns dirty=false.
        // Spawning a second refresh supersedes the first — only the second
        // provider's results should land in the cache.
        struct ProviderA;
        impl VcsProvider for ProviderA {
            fn status(&self, _repo: &std::path::Path, _wt: &Worktree) -> VcsStatus {
                VcsStatus {
                    dirty: true,
                    pr: None,
                }
            }
        }

        struct ProviderB;
        impl VcsProvider for ProviderB {
            fn status(&self, _repo: &std::path::Path, _wt: &Worktree) -> VcsStatus {
                VcsStatus {
                    dirty: false,
                    pr: None,
                }
            }
        }

        let mut app = app_with_fake_worktrees();
        app.vcs_provider = Arc::new(ProviderA);
        app.spawn_vcs_refresh();
        // Immediately supersede with provider B — drops the first receiver.
        app.vcs_provider = Arc::new(ProviderB);
        app.spawn_vcs_refresh();

        assert!(
            drain_until(&mut app, |a| a.vcs_status.len() == a.worktrees.len()),
            "vcs worker did not deliver in time"
        );

        // All entries must reflect provider B (dirty=false).
        for status in app.vcs_status.values() {
            assert!(!status.dirty, "expected provider B (dirty=false) in cache");
        }
    }
}
