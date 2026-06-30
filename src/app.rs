use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use crate::config::Config;
use crate::layout::{TabKind, WorktreeLayout};
use crate::session::{ActivityState, AttentionTracker, SessionManager};
use crate::theme::Theme;
use crate::vcs::{GitGhProvider, VcsProvider, VcsStatus};
use crate::worktree::{self, Worktree};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Sidebar,
    Agent,
}

/// How long an ARCHIVE script may run before it is killed so worktree removal can
/// proceed. A crude bound: the run is synchronous, so this caps how long a
/// hanging user script can freeze the UI.
pub const ARCHIVE_TIMEOUT: Duration = Duration::from_secs(5);

/// Outcome of a bounded ARCHIVE run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveOutcome {
    Success,
    Failed,
    TimedOut,
}

/// Runs a USER-AUTHORED archive command via `sh -c <command>` in `cwd`, bounded
/// by `timeout`. SECURITY: the command is passed as a single, un-interpolated
/// argv element and `cwd` is set with `current_dir` — the worktree path is never
/// string-built into the command. On timeout the child is killed and `TimedOut`
/// is returned without waiting for it to finish.
pub fn run_archive(command: &str, cwd: &Path, timeout: Duration) -> ArchiveOutcome {
    let mut child = match std::process::Command::new("sh")
        .arg("-c")
        .arg(command)
        .current_dir(cwd)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        Ok(child) => child,
        Err(_) => return ArchiveOutcome::Failed,
    };
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                return if status.success() {
                    ArchiveOutcome::Success
                } else {
                    ArchiveOutcome::Failed
                };
            }
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    // Reap the SIGKILL'd `sh` immediately so it is not left a
                    // zombie until wtcc exits; SIGKILL makes this near-instant.
                    let _ = child.wait();
                    return ArchiveOutcome::TimedOut;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(_) => return ArchiveOutcome::Failed,
        }
    }
}

/// Runs a USER-AUTHORED setup command once via `sh -c <command>` in `cwd`,
/// detached on a background thread so worktree creation never blocks on it.
/// SECURITY: the command is passed as a single, un-interpolated argv element and
/// `cwd` is set with `current_dir` — the worktree path is never string-built into
/// the command. Best-effort: a spawn failure is swallowed and the child is reaped
/// inside the thread by `.status()` (which waits off the UI thread).
pub fn spawn_setup(command: &str, cwd: &Path) {
    let command = command.to_string();
    let cwd = cwd.to_path_buf();
    std::thread::spawn(move || {
        let _ = std::process::Command::new("sh")
            .arg("-c")
            .arg(&command)
            .current_dir(&cwd)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    });
}

/// What the user is being prompted for while an inline input overlay is open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Prompt {
    AddWorktree,
    AddRepo,
    RenameBranch,
    SwitchAgent,
}

/// What the user is being asked to confirm while a confirm overlay is open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Confirm {
    RemoveWorktree(PathBuf),
    RemoveRepo(usize),
    /// Restart the agent for the named branch (kill its tmux session; a fresh
    /// agent respawns on the next frame). The branch is shown in the prompt.
    RestartAgent(String),
    /// Merge the PR for the named branch. Destructive, so confirm-gated.
    MergePr(String),
    /// Close the PR for the named branch. Destructive, so confirm-gated.
    ClosePr(String),
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
    /// In-memory UI toggle: when `true`, archived worktrees are shown in the
    /// sidebar. Defaults to `false`; not persisted (the `archived` markers are).
    pub show_archived: bool,
    /// UI colors, resolved once at startup. Default-only; no user config.
    pub theme: Theme,
    pub overlay: Overlay,
    pub status: Option<String>,
    pub should_quit: bool,
    pub session_manager: SessionManager,
    pub active_session: Option<String>,
    /// Per-worktree tab layouts (issue #48), keyed by branch slug. In-memory only
    /// (KNOWN GAP: not persisted, so shell tabs are lost on restart; the agent tab
    /// persists via its named tmux session). Lazily created on first use.
    pub layouts: HashMap<String, WorktreeLayout>,
    /// Edge-triggered tracker that flags agents which have gone quiet and need
    /// the user's input. Polled once per frame from the run loop.
    pub attention: AttentionTracker,
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
            show_archived: false,
            theme: Theme::default(),
            overlay: Overlay::None,
            status: None,
            should_quit: false,
            session_manager: SessionManager::new(),
            active_session: None,
            layouts: HashMap::new(),
            attention: AttentionTracker::default(),
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

    /// Worktree indices that keyboard navigation may land on, in display order.
    /// When `show_archived` is true every worktree is navigable; otherwise the
    /// selected repo's archived (soft-hidden) worktrees are skipped, using the
    /// same `archived` set the sidebar filters on. Hidden rows are treated as
    /// non-existent for selection.
    fn navigable_worktrees(&self) -> Vec<usize> {
        if self.show_archived {
            return (0..self.worktrees.len()).collect();
        }
        let archived = crate::ui::sidebar::selected_archived(self);
        (0..self.worktrees.len())
            .filter(|&i| !archived.iter().any(|p| p == &self.worktrees[i].path))
            .collect()
    }

    fn next_worktree(&mut self) {
        let nav = self.navigable_worktrees();
        let Some(&first) = nav.first() else {
            return;
        };
        let next = self
            .selected_worktree
            .and_then(|cur| nav.iter().position(|&i| i == cur))
            .map_or(first, |pos| nav[(pos + 1) % nav.len()]);
        self.selected_worktree = Some(next);
    }

    fn prev_worktree(&mut self) {
        let nav = self.navigable_worktrees();
        let Some(&first) = nav.first() else {
            return;
        };
        let prev = self
            .selected_worktree
            .and_then(|cur| nav.iter().position(|&i| i == cur))
            .map_or(first, |pos| nav[(pos + nav.len() - 1) % nav.len()]);
        self.selected_worktree = Some(prev);
    }

    /// If the current selection points at a worktree that is no longer
    /// navigable (e.g. it was just archived while archived rows are hidden),
    /// moves selection to the nearest still-visible row — the first visible row
    /// after it, else the last visible one before it. A no-op when the selection
    /// is already visible, and when nothing is visible the selection is kept so
    /// the row stays reversible (e.g. unarchiving the only worktree via toggle).
    pub fn select_nearest_visible(&mut self) {
        let nav = self.navigable_worktrees();
        let Some(cur) = self.selected_worktree else {
            return;
        };
        if nav.contains(&cur) {
            return;
        }
        if let Some(target) = nav
            .iter()
            .copied()
            .find(|&i| i > cur)
            .or_else(|| nav.iter().copied().rev().find(|&i| i < cur))
        {
            self.selected_worktree = Some(target);
        }
    }

    /// Branch slug of the selected worktree, slugified so untrusted branch names
    /// are safe to use as layout keys and session-name stems.
    pub fn current_slug(&self) -> Option<String> {
        self.current_worktree()
            .map(|w| crate::worktree::slugify(&w.branch))
    }

    /// Returns the current worktree's layout, creating an agent-only one if none
    /// exists yet. Internal helper for the tab commands.
    fn layout_for_current(&mut self) -> Option<&mut WorktreeLayout> {
        let slug = self.current_slug()?;
        Some(
            self.layouts
                .entry(slug.clone())
                .or_insert_with(|| WorktreeLayout::new(&slug)),
        )
    }

    /// Adds a focused shell tab to the current worktree's layout (in-memory only;
    /// the real PTY spawns next frame in `ensure_active_session`).
    pub fn new_shell_tab(&mut self) {
        let Some(slug) = self.current_slug() else {
            return;
        };
        self.layouts
            .entry(slug.clone())
            .or_insert_with(|| WorktreeLayout::new(&slug))
            .add_shell_tab(&slug);
    }

    /// The selected repo's `run` command, if any. Re-resolved at spawn time so the
    /// Run tab need not store the command.
    fn selected_run_command(&self) -> Option<String> {
        self.selected_repo
            .and_then(|i| self.config.repos.get(i))
            .and_then(|r| r.run.clone())
    }

    /// Opens (or re-focuses) the current worktree's Run tab, backed by the
    /// `wtcc-run-<slug>` session. In-memory only — the real PTY (`sh -c <run>`)
    /// spawns next frame in `ensure_active_session`. With no `run` configured for
    /// the selected repo, nothing is opened and the reason is surfaced in `status`.
    pub fn start_run_script(&mut self) {
        if self.selected_run_command().is_none() {
            self.status = Some("no run command configured for this repo".to_string());
            return;
        }
        let Some(slug) = self.current_slug() else {
            self.status = Some("no worktree selected".to_string());
            return;
        };
        self.layouts
            .entry(slug.clone())
            .or_insert_with(|| WorktreeLayout::new(&slug))
            .add_run_tab(&slug);
    }

    /// Focuses the next tab of the current worktree (wrapping). No-op if no layout.
    pub fn next_tab(&mut self) {
        if let Some(layout) = self.layout_for_current() {
            layout.next_tab();
        }
    }

    /// Focuses the previous tab of the current worktree (wrapping). No-op if no
    /// layout.
    pub fn prev_tab(&mut self) {
        if let Some(layout) = self.layout_for_current() {
            layout.prev_tab();
        }
    }

    /// Closes the active tab. A shell tab's named session is killed and
    /// `active_session` is cleared if it pointed there (so it respawns next
    /// frame). The agent tab (0) and the last remaining tab are guarded with a
    /// status message.
    pub fn close_tab(&mut self) {
        let Some(slug) = self.current_slug() else {
            return;
        };
        let closed = match self.layouts.get_mut(&slug) {
            Some(layout) => layout.close_active(),
            None => return,
        };
        match closed {
            Some(session) => {
                self.session_manager.kill(&session);
                if self.active_session.as_deref() == Some(session.as_str()) {
                    self.active_session = None;
                }
            }
            None => self.status = Some("cannot close the agent tab".to_string()),
        }
    }

    /// Lazily spawns (or reuses) the ACTIVE tab's session for the current worktree
    /// and records its name in `active_session`. Seeds the worktree's tab layout
    /// (agent-only) on first use. The agent tab runs the worktree's agent command;
    /// a shell tab runs the default shell (`None`). Spawn errors land in `status`.
    pub fn ensure_active_session(&mut self, rows: u16, cols: u16) {
        let Some(wt) = self.current_worktree() else {
            self.active_session = None;
            return;
        };
        let branch = wt.branch.clone();
        let path = wt.path.clone();
        let slug = crate::worktree::slugify(&branch);
        let tab = self
            .layouts
            .entry(slug.clone())
            .or_insert_with(|| WorktreeLayout::new(&slug))
            .active_tab()
            .clone();
        let agent_cmd = self.config.agent_cmd_for(&branch);
        let run_cmd = self.selected_run_command();
        let result = match tab.kind {
            TabKind::Agent => self.session_manager.ensure_named(
                &tab.session,
                &path,
                Some(agent_cmd.as_str()),
                rows,
                cols,
            ),
            TabKind::Shell => {
                self.session_manager
                    .ensure_named(&tab.session, &path, None, rows, cols)
            }
            // SECURITY: the run command reaches tmux (run via `$SHELL -c`) as a single
            // un-interpolated trailing argv element via `ensure_run`. A run tab with no configured command
            // (config edited away after opening) degrades to a plain shell.
            TabKind::Run => match run_cmd.as_deref() {
                Some(cmd) => self
                    .session_manager
                    .ensure_run(&tab.session, cmd, &path, rows, cols),
                None => self
                    .session_manager
                    .ensure_named(&tab.session, &path, None, rows, cols),
            },
        };
        match result {
            Ok(_) => self.active_session = Some(tab.session),
            Err(e) => self.status = Some(format!("agent spawn failed: {e}")),
        }
    }

    /// Polls the attention tracker with a fresh idle snapshot, suppressing the
    /// active session. Returns the branch labels that newly need attention this
    /// frame, for the run loop to surface as desktop notifications.
    pub fn poll_attention(&mut self) -> Vec<String> {
        let snapshot = self.session_manager.idle_durations();
        let active = self.active_session.clone();
        let fired = self.attention.poll(&snapshot, active.as_deref());
        fired
            .iter()
            .filter_map(|name| {
                self.worktrees
                    .iter()
                    .find(|w| &SessionManager::session_name(&w.branch) == name)
                    .map(|w| w.branch.clone())
            })
            .collect()
    }

    /// Whether the agent for `branch` is currently flagged for attention.
    pub fn attention_for(&self, branch: &str) -> bool {
        self.attention.needs(&SessionManager::session_name(branch))
    }

    /// How many agents are currently flagged for attention.
    pub fn attention_count(&self) -> usize {
        self.attention.count()
    }

    /// Moves the selection to the next worktree (cyclically, after the current
    /// one) whose agent is flagged for attention. No-op when none are flagged.
    pub fn jump_to_attention(&mut self) {
        let n = self.worktrees.len();
        if n == 0 {
            return;
        }
        let start = self.selected_worktree.unwrap_or(0);
        for offset in 1..=n {
            let i = (start + offset) % n;
            if self.attention_for(&self.worktrees[i].branch) {
                self.selected_worktree = Some(i);
                return;
            }
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
        // Auto-detect: an existing branch is checked out (review a PR / resume
        // work); an unknown name becomes a new branch. No mode toggle needed.
        let result = if worktree::branch_exists(&repo, branch) {
            worktree::add_existing_branch(&repo, &new_path, branch)
        } else {
            let base_ref = self
                .selected_repo
                .and_then(|i| self.config.repos.get(i))
                .and_then(|r| r.base_ref.clone());
            worktree::add_new_branch(&repo, &new_path, branch, base_ref.as_deref())
        };
        match result {
            Ok(()) => {
                self.refresh_worktrees();
                let repo_entry = self.selected_repo.and_then(|i| self.config.repos.get(i));
                let copy_on_create = repo_entry
                    .map(|r| r.copy_on_create.clone())
                    .unwrap_or_default();
                let setup = repo_entry.and_then(|r| r.setup.clone());

                let mut msg = format!("added worktree {branch}");
                if !copy_on_create.is_empty() {
                    let report = worktree::copy_into_worktree(&repo, &new_path, &copy_on_create);
                    msg.push_str(&format!(
                        "; copied {} skipped {}",
                        report.copied, report.skipped
                    ));
                }
                // SETUP runs once in the new worktree, best-effort and detached.
                if let Some(setup) = setup {
                    msg.push_str("; running setup…");
                    spawn_setup(&setup, &new_path);
                }
                self.status = Some(msg);
            }
            Err(e) => self.status = Some(format!("add failed: {e}")),
        }
    }

    /// Renames the selected worktree's branch to `new` and RE-KEYS its agent's
    /// tmux session in place so the live agent stays attached under the new
    /// `wtcc-<slug>` key (never killed). `git branch -m` does not move the
    /// worktree directory, so path-keyed state (vcs/attention) stays valid and is
    /// left untouched. `new` is passed to git verbatim; only the DERIVED tmux
    /// session key is slugified. Guards (empty name, no/detached/bare worktree,
    /// name collision) and any git failure land in `status` — never panics.
    pub fn rename_branch(&mut self, new: &str) {
        let new = new.trim();
        if new.is_empty() {
            self.status = Some("branch name cannot be empty".to_string());
            return;
        }
        let Some(wt) = self.current_worktree() else {
            self.status = Some("no worktree selected".to_string());
            return;
        };
        if wt.is_detached {
            self.status = Some("cannot rename a detached worktree".to_string());
            return;
        }
        if wt.is_bare {
            self.status = Some("cannot rename a bare worktree".to_string());
            return;
        }
        let old_branch = wt.branch.clone();
        let Some(repo) = self.selected_repo_path().map(|p| p.to_path_buf()) else {
            self.status = Some("no repo selected".to_string());
            return;
        };
        if worktree::branch_exists(&repo, new) {
            self.status = Some(format!("branch already exists: {new}"));
            return;
        }
        match worktree::rename_branch(&repo, &old_branch, new) {
            Ok(()) => {
                let old_name = SessionManager::session_name(&old_branch);
                let new_name = SessionManager::session_name(new);
                self.session_manager.rename(&old_name, &new_name);
                if self.active_session.as_deref() == Some(old_name.as_str()) {
                    self.active_session = Some(new_name);
                }
                self.refresh_worktrees();
                self.status = Some(format!("renamed branch to {new}"));
            }
            Err(e) => self.status = Some(format!("rename failed: {e}")),
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
        // Spec ordering: kill agent session -> archive -> git remove. The agent
        // is reaped first so it is quiescent before the archive runs (it can't be
        // writing files mid-archive).
        let branch = self
            .worktrees
            .iter()
            .find(|w| w.path == path)
            .map(|w| w.branch.clone());
        // `Session::Drop` detaches without killing tmux (for reattach), so the
        // explicit remove path is the only place that reaps the worktree's
        // sessions. Kill EVERY tab surface (`wtcc-<slug>` agent + `wtcc-<slug>-t*`
        // shells) and drop the layout so no shell session is orphaned (#48), then
        // the branch-keyed agent kill covers the no-layout case. Best-effort.
        if let Some(branch) = branch.as_deref() {
            let slug = crate::worktree::slugify(branch);
            if let Some(layout) = self.layouts.remove(&slug) {
                for tab in &layout.tabs {
                    self.session_manager.kill(&tab.session);
                    if self.active_session.as_deref() == Some(tab.session.as_str()) {
                        self.active_session = None;
                    }
                }
            }
            let name = SessionManager::session_name(branch);
            self.session_manager.kill(&name);
            if self.active_session.as_deref() == Some(name.as_str()) {
                self.active_session = None;
            }
        }
        // ARCHIVE runs in the worktree dir, after the agent is killed and before
        // git removes the worktree. Bounded so a hanging script can't freeze the
        // UI; on failure or timeout removal proceeds anyway, with the outcome
        // folded into the final status.
        let archive_note = self
            .selected_repo
            .and_then(|i| self.config.repos.get(i))
            .and_then(|r| r.archive.clone())
            .and_then(|cmd| match run_archive(&cmd, path, ARCHIVE_TIMEOUT) {
                ArchiveOutcome::Success => None,
                ArchiveOutcome::Failed => Some("archive failed"),
                ArchiveOutcome::TimedOut => Some("archive timed out"),
            });
        match worktree::remove(&repo, path) {
            Ok(()) => {
                // `refresh_worktrees` clears `status` on success, so set the
                // outcome (incl. any archive note) afterwards to keep it visible.
                self.refresh_worktrees();
                self.status = Some(match archive_note {
                    Some(note) => format!("removed worktree ({note})"),
                    None => "removed worktree".to_string(),
                });
            }
            Err(e) => self.status = Some(format!("remove failed: {e}")),
        }
    }

    /// Soft-hides `path` from the sidebar by adding it to the selected repo's
    /// `archived` markers and persisting the config. Pure UI/config: the worktree
    /// and its branch stay on disk (no git op). A no-op if already archived; a
    /// persist failure rolls the marker back out and reports it. Never panics.
    pub fn archive_worktree(&mut self, path: &Path) {
        let Some(repo_idx) = self.selected_repo else {
            self.status = Some("no repo selected".to_string());
            return;
        };
        {
            let Some(repo) = self.config.repos.get_mut(repo_idx) else {
                return;
            };
            if repo.archived.iter().any(|p| p == path) {
                return;
            }
            repo.archived.push(path.to_path_buf());
        }
        if let Err(e) = self.persist_config() {
            if let Some(repo) = self.config.repos.get_mut(repo_idx) {
                repo.archived.retain(|p| p != path);
            }
            self.status = Some(format!("save failed: {e}"));
        }
    }

    /// Un-hides `path` by removing it from the selected repo's `archived` markers
    /// and persisting the config. A no-op if not archived; a persist failure
    /// re-adds the marker and reports it. Never panics.
    pub fn unarchive_worktree(&mut self, path: &Path) {
        let Some(repo_idx) = self.selected_repo else {
            self.status = Some("no repo selected".to_string());
            return;
        };
        {
            let Some(repo) = self.config.repos.get_mut(repo_idx) else {
                return;
            };
            let before = repo.archived.len();
            repo.archived.retain(|p| p != path);
            if repo.archived.len() == before {
                return;
            }
        }
        if let Err(e) = self.persist_config() {
            if let Some(repo) = self.config.repos.get_mut(repo_idx) {
                repo.archived.push(path.to_path_buf());
            }
            self.status = Some(format!("save failed: {e}"));
        }
    }

    /// Persists the config to the redirected `config_path` (tests) or the default
    /// XDG path (production).
    fn persist_config(&self) -> anyhow::Result<()> {
        match &self.config_path {
            Some(path) => self.config.save_to(path),
            None => self.config.save(),
        }
    }

    /// Restarts the agent for `branch`: kills its `wtcc-<slug>` tmux session and
    /// drops the local `Session`, then clears `active_session` if it pointed at
    /// that session so the run loop's `ensure_active_session` respawns a fresh
    /// agent next frame. Touches only the named session, never other worktrees'.
    /// Works whether or not a live local session exists.
    pub fn restart_agent(&mut self, branch: &str) {
        let name = SessionManager::session_name(branch);
        self.session_manager.kill(&name);
        if self.active_session.as_deref() == Some(name.as_str()) {
            self.active_session = None;
        }
        self.status = Some(format!("restarting agent for {branch}"));
    }

    /// Records `branch`'s agent choice (preset `name`), persists the config, then
    /// restarts that worktree's agent so the new cmd takes effect (reuses
    /// `restart_agent`: kills only that `wtcc-<slug>` session). A persist failure
    /// rolls the in-memory choice back and reports it — never panics, never
    /// touches another worktree's session. An unknown preset name is rejected
    /// (status reports the valid names) and nothing is persisted or restarted.
    pub fn set_worktree_agent(&mut self, branch: &str, name: &str) {
        let presets = self.config.presets();
        if !presets.iter().any(|p| p.name == name) {
            let available = presets
                .iter()
                .map(|p| p.name.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            self.status = Some(format!("unknown agent '{name}'; available: {available}"));
            return;
        }
        self.config.set_worktree_agent(branch, name);
        let save = match &self.config_path {
            Some(path) => self.config.save_to(path),
            None => self.config.save(),
        };
        if let Err(e) = save {
            self.config.worktree_agents.remove(branch);
            self.status = Some(format!("save failed: {e}"));
            return;
        }
        self.restart_agent(branch);
        self.status = Some(format!("switched agent for {branch} to {name}"));
    }

    /// Resolves the `(branch, path)` of the selected worktree's PR, or an error
    /// message describing why no PR action can run: no worktree selected, or the
    /// selected worktree has no cached PR. The branch is the discrete argument
    /// every `gh` PR action takes.
    pub(crate) fn pr_target(&self) -> Result<(String, PathBuf), String> {
        let branch = match self.current_worktree() {
            Some(wt) => wt.branch.clone(),
            None => return Err("no worktree selected".to_string()),
        };
        let path = self.pr_target_for(&branch)?;
        Ok((branch, path))
    }

    /// Resolves the worktree path to run `gh` in for `branch`'s PR, re-checking
    /// the cached PR. Used by the confirm executors, which carry a pre-captured
    /// branch: this re-validates against current state so a stale confirm (the PR
    /// vanished between opening the dialog and confirming) guards out cleanly.
    fn pr_target_for(&self, branch: &str) -> Result<PathBuf, String> {
        let Some(wt) = self.worktrees.iter().find(|w| w.branch == branch) else {
            return Err(format!("no worktree for {branch}"));
        };
        match self.vcs_status.get(&wt.path).and_then(|s| s.pr) {
            Some(_) => Ok(wt.path.clone()),
            None => Err(format!("no PR for {branch}")),
        }
    }

    /// Opens the selected worktree's PR in a browser (`gh pr view --web`). Runs
    /// immediately (no confirm). Guards and any `gh` failure land in `status`.
    /// Opens nothing on the `gh` side, so it never refreshes the cached status.
    pub fn pr_open_in_browser(&mut self) {
        self.status = Some(match self.pr_target() {
            Ok((branch, path)) => {
                match crate::pr::run_gh(&crate::pr::open_in_browser_argv(&branch), &path) {
                    Ok(()) => format!("opening PR for {branch} in browser"),
                    Err(e) => format!("could not open PR for {branch}: {e}"),
                }
            }
            Err(msg) => msg,
        });
    }

    /// Marks the selected worktree's draft PR ready (`gh pr ready`). Immediate,
    /// no confirm. On success the cached VCS status is refreshed so the sidebar
    /// PR badge reflects the new state. Guards and any `gh` failure land in
    /// `status`.
    pub fn pr_mark_ready(&mut self) {
        let status = match self.pr_target() {
            Ok((branch, path)) => {
                match crate::pr::run_gh(&crate::pr::mark_ready_argv(&branch), &path) {
                    Ok(()) => {
                        self.spawn_vcs_refresh();
                        format!("marked PR ready for {branch}")
                    }
                    Err(e) => format!("could not mark PR ready for {branch}: {e}"),
                }
            }
            Err(msg) => msg,
        };
        self.status = Some(status);
    }

    /// Merges `branch`'s PR (`gh pr merge --<strategy>`). The executor the merge
    /// confirm dispatches into with its pre-captured branch. On success the
    /// cached VCS status is refreshed so the sidebar PR badge updates. Guards and
    /// any `gh` failure land in `status`.
    pub fn pr_merge_branch(&mut self, branch: &str) {
        let path = match self.pr_target_for(branch) {
            Ok(path) => path,
            Err(msg) => {
                self.status = Some(msg);
                return;
            }
        };
        let strategy = self.config.merge_strategy;
        let status = match crate::pr::run_gh(&crate::pr::merge_argv(branch, strategy), &path) {
            Ok(()) => {
                self.spawn_vcs_refresh();
                format!("merged PR for {branch}")
            }
            Err(e) => format!("merge failed for {branch}: {e}"),
        };
        self.status = Some(status);
    }

    /// Closes `branch`'s PR (`gh pr close`). The executor the close confirm
    /// dispatches into with its pre-captured branch. On success the cached VCS
    /// status is refreshed so the sidebar PR badge updates. Guards and any `gh`
    /// failure land in `status`.
    pub fn pr_close_branch(&mut self, branch: &str) {
        let path = match self.pr_target_for(branch) {
            Ok(path) => path,
            Err(msg) => {
                self.status = Some(msg);
                return;
            }
        };
        let status = match crate::pr::run_gh(&crate::pr::close_argv(branch), &path) {
            Ok(()) => {
                self.spawn_vcs_refresh();
                format!("closed PR for {branch}")
            }
            Err(e) => format!("close failed for {branch}: {e}"),
        };
        self.status = Some(status);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repository::Repository;
    use crate::vcs::{ChecksState, PrState, PrStatus};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::{Duration, Instant};

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
                setup: None,
                archive: None,
                archived: Vec::new(),
                base_ref: None,
                copy_on_create: Vec::new(),
                run: None,
            }],
            agent_cmd: "claude".to_string(),
            notify: true,
            merge_strategy: crate::pr::MergeStrategy::default(),
            ..Default::default()
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
            show_archived: false,
            theme: Theme::default(),
            overlay: Overlay::None,
            status: None,
            should_quit: false,
            session_manager: SessionManager::new(),
            active_session: None,
            layouts: HashMap::new(),
            attention: AttentionTracker::default(),
            config_path: None,
            vcs_status: HashMap::new(),
            vcs_provider: Arc::new(GitGhProvider),
            vcs_rx: None,
        };
        app.selected_worktree = Some(0);
        app
    }

    #[test]
    fn restart_agent_drops_named_session_clears_active_and_keeps_others() {
        use portable_pty::CommandBuilder;

        let mut app = app_with_fake_worktrees();
        let main = SessionManager::session_name("main");
        let feat = SessionManager::session_name("feat");
        let mut a = CommandBuilder::new("printf");
        a.args(["a"]);
        let mut b = CommandBuilder::new("printf");
        b.args(["b"]);
        app.session_manager
            .insert_spawned(&main, a, &std::env::temp_dir(), 24, 80)
            .unwrap();
        app.session_manager
            .insert_spawned(&feat, b, &std::env::temp_dir(), 24, 80)
            .unwrap();
        app.active_session = Some(main.clone());

        app.restart_agent("main");

        assert!(app.session_manager.get(&main).is_none());
        assert!(
            app.session_manager.get(&feat).is_some(),
            "other worktree's session must survive a restart"
        );
        assert_eq!(app.active_session, None);
        assert_eq!(app.status.as_deref(), Some("restarting agent for main"));
    }

    #[test]
    fn restart_agent_without_live_session_is_safe() {
        let mut app = app_with_fake_worktrees();
        // No local session, active_session unset: must not panic, sets status.
        app.restart_agent("main");
        assert_eq!(app.status.as_deref(), Some("restarting agent for main"));
        assert_eq!(app.active_session, None);
    }

    // --- issue #46: removing a worktree must kill its agent session ----------
    //
    // `Session::Drop` intentionally detaches without killing tmux (for
    // reattach/persistence), so the ONLY place the explicit remove path can
    // reap the `wtcc-<slug>` session is `remove_worktree`. The git removal runs
    // against a fake repo path here and fails, but the kill is keyed off the
    // removed worktree's branch and happens around/before that removal, so the
    // session side-effect is observable without tmux or a real repo.

    #[test]
    fn remove_worktree_kills_removed_worktrees_session_clears_active_and_keeps_others() {
        use portable_pty::CommandBuilder;

        let mut app = app_with_fake_worktrees(); // main(/repo/main), feat(/repo/feat)
        let main = SessionManager::session_name("main");
        let feat = SessionManager::session_name("feat");
        let mut a = CommandBuilder::new("printf");
        a.args(["a"]);
        let mut b = CommandBuilder::new("printf");
        b.args(["b"]);
        app.session_manager
            .insert_spawned(&main, a, &std::env::temp_dir(), 24, 80)
            .unwrap();
        app.session_manager
            .insert_spawned(&feat, b, &std::env::temp_dir(), 24, 80)
            .unwrap();
        app.active_session = Some(main.clone());

        app.remove_worktree(&PathBuf::from("/repo/main"));

        assert!(
            app.session_manager.get(&main).is_none(),
            "removing a worktree must kill its wtcc-<slug> agent session"
        );
        assert!(
            app.session_manager.get(&feat).is_some(),
            "removing one worktree must leave every other worktree's session intact"
        );
        assert_eq!(
            app.active_session, None,
            "active_session must clear when the removed worktree was the active one"
        );
    }

    #[test]
    fn remove_worktree_without_live_session_is_safe() {
        let mut app = app_with_fake_worktrees();
        let feat = SessionManager::session_name("feat");
        // No session was ever spawned for feat: the best-effort kill must not
        // panic and must leave no session behind.
        app.remove_worktree(&PathBuf::from("/repo/feat"));
        assert!(
            app.session_manager.get(&feat).is_none(),
            "absent session stays absent; kill is best-effort"
        );
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

    /// Builds the 2-worktree fake app, appends a third (`bug`) and archives the
    /// middle one (`feat`), selection on `main`. Mirrors the sidebar's hidden
    /// state: navigable rows are 0 and 2.
    fn app_with_archived_middle() -> App {
        let mut app = app_with_fake_worktrees();
        app.worktrees.push(Worktree {
            path: PathBuf::from("/repo/bug"),
            branch: "bug".to_string(),
            head: "ghi789".to_string(),
            is_bare: false,
            is_detached: false,
        });
        app.config.repos[0].archived = vec![PathBuf::from("/repo/feat")];
        app.selected_worktree = Some(0);
        app
    }

    #[test]
    fn nav_skips_hidden_archived_worktree() {
        let mut app = app_with_archived_middle();
        assert!(!app.show_archived);
        assert_eq!(app.selected_worktree, Some(0));

        // Forward never lands on the hidden index 1: 0 -> 2 -> wrap to 0.
        app.next();
        assert_eq!(app.selected_worktree, Some(2));
        app.next();
        assert_eq!(app.selected_worktree, Some(0));

        // Backward likewise skips 1: 0 -> 2 -> 0.
        app.prev();
        assert_eq!(app.selected_worktree, Some(2));
        app.prev();
        assert_eq!(app.selected_worktree, Some(0));
    }

    #[test]
    fn nav_visits_every_worktree_when_archived_shown() {
        let mut app = app_with_archived_middle();
        app.show_archived = true;

        app.next();
        assert_eq!(app.selected_worktree, Some(1));
        app.next();
        assert_eq!(app.selected_worktree, Some(2));
        app.next();
        assert_eq!(app.selected_worktree, Some(0));
    }

    #[test]
    fn select_nearest_visible_moves_off_a_hidden_selection() {
        let mut app = app_with_archived_middle();
        // Pretend selection is stranded on the hidden middle row.
        app.selected_worktree = Some(1);

        app.select_nearest_visible();

        assert_eq!(
            app.selected_worktree,
            Some(2),
            "a hidden selection must move to the nearest visible neighbor"
        );
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
                        setup: None,
                        archive: None,
                        archived: Vec::new(),
                        base_ref: None,
                        copy_on_create: Vec::new(),
                        run: None,
                    },
                    Repository {
                        name: "repo-b".to_string(),
                        path: repo_b,
                        setup: None,
                        archive: None,
                        archived: Vec::new(),
                        base_ref: None,
                        copy_on_create: Vec::new(),
                        run: None,
                    },
                ],
                agent_cmd: "claude".to_string(),
                notify: true,
                merge_strategy: crate::pr::MergeStrategy::default(),
                ..Default::default()
            },
            selected_repo: Some(1),
            worktrees: Vec::new(),
            selected_worktree: None,
            focus: Focus::Sidebar,
            show_archived: false,
            theme: Theme::default(),
            overlay: Overlay::None,
            status: None,
            should_quit: false,
            session_manager: SessionManager::new(),
            active_session: None,
            layouts: HashMap::new(),
            attention: AttentionTracker::default(),
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

    // --- issue #47: attention routing ---------------------------------------

    /// Drives the tracker through a Busy->Quiet edge for `branch`'s session so
    /// it becomes flagged, without needing a real PTY.
    fn flag_branch(app: &mut App, branch: &str) {
        let name = SessionManager::session_name(branch);
        let busy = [(name.clone(), std::time::Duration::ZERO)];
        let quiet = [(name, crate::session::ATTENTION_QUIET)];
        app.attention.poll(&busy, None);
        app.attention.poll(&quiet, None);
    }

    #[test]
    fn jump_to_attention_advances_to_flagged_worktree() {
        let mut app = app_with_fake_worktrees(); // main(0), feat(1), selected 0
        flag_branch(&mut app, "feat");
        app.jump_to_attention();
        assert_eq!(app.selected_worktree, Some(1));
    }

    #[test]
    fn jump_to_attention_is_noop_when_none_flagged() {
        let mut app = app_with_fake_worktrees();
        app.jump_to_attention();
        assert_eq!(app.selected_worktree, Some(0));
    }

    #[test]
    fn attention_count_and_attention_for_reflect_the_flagged_set() {
        let mut app = app_with_fake_worktrees();
        assert_eq!(app.attention_count(), 0);
        assert!(!app.attention_for("feat"));

        flag_branch(&mut app, "feat");

        assert_eq!(app.attention_count(), 1);
        assert!(app.attention_for("feat"));
        assert!(!app.attention_for("main"));
    }

    #[test]
    fn poll_attention_is_empty_without_sessions() {
        let mut app = app_with_fake_worktrees();
        assert!(app.poll_attention().is_empty());
    }

    // --- issue #51: rename branch + re-key the live agent session -----------
    //
    // TDD RED: `App::rename_branch(new)` takes `old` from the selected worktree,
    // renames the branch via git (argv-only `git branch -m`), then RE-KEYS the
    // agent's `wtcc-<slug>` tmux session in place — the live agent stays attached
    // under the new key (never killed) — updates `active_session` if it matched,
    // and refreshes the worktree list. The worktree DIRECTORY does not move, so
    // path-keyed state stays valid and is left untouched.

    fn init_git_repo(repo: &Path) {
        let init = std::process::Command::new("git")
            .arg("init")
            .arg(repo)
            .output()
            .expect("git must be installed");
        assert!(init.status.success(), "git init failed");
        let run = |args: &[&str]| {
            let out = std::process::Command::new("git")
                .arg("-C")
                .arg(repo)
                .args(args)
                .output()
                .expect("git must be installed");
            assert!(
                out.status.success(),
                "git {args:?} failed: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        };
        run(&["config", "user.email", "t@example.com"]);
        run(&["config", "user.name", "wtcc test"]);
        run(&["commit", "--allow-empty", "-m", "init"]);
    }

    fn app_for_repo(repo: PathBuf) -> App {
        let mut app = App {
            config: Config {
                repos: vec![Repository {
                    name: "demo".to_string(),
                    path: repo,
                    setup: None,
                    archive: None,
                    archived: Vec::new(),
                    base_ref: None,
                    copy_on_create: Vec::new(),
                    run: None,
                }],
                agent_cmd: "claude".to_string(),
                notify: true,
                merge_strategy: crate::pr::MergeStrategy::default(),
                ..Default::default()
            },
            selected_repo: Some(0),
            worktrees: Vec::new(),
            selected_worktree: None,
            focus: Focus::Sidebar,
            show_archived: false,
            theme: Theme::default(),
            overlay: Overlay::None,
            status: None,
            should_quit: false,
            session_manager: SessionManager::new(),
            active_session: None,
            layouts: HashMap::new(),
            attention: AttentionTracker::default(),
            config_path: None,
            vcs_status: HashMap::new(),
            vcs_provider: Arc::new(GitGhProvider),
            vcs_rx: None,
        };
        app.refresh_worktrees();
        app
    }

    #[test]
    fn rename_branch_rekeys_live_session_updates_active_and_branch_field() {
        use portable_pty::CommandBuilder;

        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().to_path_buf();
        init_git_repo(&repo);
        let mut app = app_for_repo(repo);

        let old_branch = app.current_worktree().unwrap().branch.clone();
        let old_name = SessionManager::session_name(&old_branch);
        let other_name = SessionManager::session_name("other-wt");
        let mut s = CommandBuilder::new("printf");
        s.args(["x"]);
        let mut o = CommandBuilder::new("printf");
        o.args(["y"]);
        app.session_manager
            .insert_spawned(&old_name, s, &std::env::temp_dir(), 24, 80)
            .unwrap();
        app.session_manager
            .insert_spawned(&other_name, o, &std::env::temp_dir(), 24, 80)
            .unwrap();
        app.active_session = Some(old_name.clone());

        app.rename_branch("renamed-branch");

        let new_name = SessionManager::session_name("renamed-branch");
        assert!(
            app.session_manager.get(&old_name).is_none(),
            "the old session key must be gone"
        );
        assert!(
            app.session_manager.get(&new_name).is_some(),
            "the live agent must be RE-KEYED under the new slug, not killed"
        );
        assert!(
            app.session_manager.get(&other_name).is_some(),
            "renaming one worktree must leave every other session intact"
        );
        assert_eq!(
            app.active_session.as_deref(),
            Some(new_name.as_str()),
            "active_session must follow the rename so the pane stays attached"
        );
        assert!(
            app.worktrees.iter().any(|w| w.branch == "renamed-branch"),
            "the sidebar must show the new branch after refresh"
        );
        assert!(
            !app.worktrees.iter().any(|w| w.branch == old_branch),
            "the old branch must be gone after the rename"
        );
    }

    #[test]
    fn rename_branch_to_existing_name_is_refused_and_keeps_the_session() {
        use portable_pty::CommandBuilder;

        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().to_path_buf();
        init_git_repo(&repo);
        let collide = std::process::Command::new("git")
            .arg("-C")
            .arg(&repo)
            .args(["branch", "taken"])
            .output()
            .unwrap();
        assert!(collide.status.success());
        let mut app = app_for_repo(repo);

        let old_branch = app.current_worktree().unwrap().branch.clone();
        let old_name = SessionManager::session_name(&old_branch);
        let mut s = CommandBuilder::new("printf");
        s.args(["x"]);
        app.session_manager
            .insert_spawned(&old_name, s, &std::env::temp_dir(), 24, 80)
            .unwrap();

        app.rename_branch("taken");

        assert!(
            app.session_manager.get(&old_name).is_some(),
            "a refused rename must not touch the agent session"
        );
        let status = app.status.clone().unwrap_or_default().to_lowercase();
        assert!(
            status.contains("exists"),
            "expected an 'already exists' status, got {status:?}"
        );
        assert!(
            app.worktrees.iter().any(|w| w.branch == old_branch),
            "the branch must be unchanged after a refused rename"
        );
    }

    // --- issue #49: bounded archive runner ----------------------------------
    //
    // TDD RED: `run_archive` is the synchronous, timed cleanup seam. It runs a
    // USER-AUTHORED command via `sh -c <command>` with `current_dir(cwd)` and a
    // hard timeout. A zero exit -> Success, non-zero -> Failed, and a command
    // that outlives the timeout is killed and reported as TimedOut WITHOUT
    // waiting for it to finish. The path/branch is NEVER interpolated into the
    // command string — cwd is set via `current_dir`, proven here by a command
    // that uses a shell redirect and reads `$PWD`.

    #[test]
    fn run_archive_reports_success_on_zero_exit() {
        let out = run_archive("true", &std::env::temp_dir(), Duration::from_secs(5));
        assert_eq!(out, ArchiveOutcome::Success);
    }

    #[test]
    fn run_archive_reports_failed_on_nonzero_exit() {
        let out = run_archive("exit 7", &std::env::temp_dir(), Duration::from_secs(5));
        assert_eq!(out, ArchiveOutcome::Failed);
    }

    #[test]
    fn run_archive_kills_and_reports_timeout_without_waiting_for_a_slow_script() {
        let start = Instant::now();
        let out = run_archive(
            "sleep 30",
            &std::env::temp_dir(),
            Duration::from_millis(200),
        );
        assert_eq!(out, ArchiveOutcome::TimedOut);
        let elapsed = start.elapsed();
        assert!(
            elapsed >= Duration::from_millis(150),
            "must wait for the timeout before killing, not report TimedOut early (took {elapsed:?})"
        );
        assert!(
            elapsed < Duration::from_secs(5),
            "a hanging archive must be killed at the timeout, not waited on (took {elapsed:?})"
        );
    }

    #[test]
    fn run_archive_executes_in_the_given_cwd_via_a_shell() {
        // A redirect requires a real shell (`sh -c`), and `pwd -P` proves the
        // command ran with cwd set via `current_dir` — never string-interpolated.
        let out_dir = tempfile::tempdir().unwrap();
        let marker = out_dir.path().join("cwd.txt");
        let cwd = tempfile::tempdir().unwrap();
        let cmd = format!("pwd -P > {}", marker.display());

        let out = run_archive(&cmd, cwd.path(), Duration::from_secs(5));

        assert_eq!(out, ArchiveOutcome::Success);
        let recorded = std::fs::read_to_string(&marker).unwrap();
        assert_eq!(
            recorded.trim(),
            cwd.path().canonicalize().unwrap().to_string_lossy()
        );
    }

    // --- issue #52: per-worktree agent presets (session isolation) ----------
    //
    // TDD RED: `App::set_worktree_agent(branch, name)` records the choice in
    // `config.worktree_agents`, persists it to the redirected `config_path`, then
    // RESTARTS that worktree's agent (kills its `wtcc-<slug>` tmux session + drops
    // the local `Session` so a fresh agent respawns with the new cmd) and sets
    // status. It must touch ONLY the chosen worktree's session, never another's.
    // `insert_spawned` is the test-only seam to seed live sessions without tmux,
    // so this isolation check lives in-module rather than in the integration file.

    #[test]
    fn set_worktree_agent_persists_restarts_that_session_and_leaves_others() {
        use portable_pty::CommandBuilder;

        let dir = tempfile::tempdir().unwrap();
        let cfg_path = dir.path().join("config.toml");
        let mut app = app_with_fake_worktrees(); // main(/repo/main), feat(/repo/feat)
        app.config.agents = vec![crate::config::AgentPreset {
            name: "codex".to_string(),
            cmd: "codex --model x".to_string(),
        }];
        app.config_path = Some(cfg_path.clone());

        let main = SessionManager::session_name("main");
        let feat = SessionManager::session_name("feat");
        let mut a = CommandBuilder::new("printf");
        a.args(["a"]);
        let mut b = CommandBuilder::new("printf");
        b.args(["b"]);
        app.session_manager
            .insert_spawned(&main, a, &std::env::temp_dir(), 24, 80)
            .unwrap();
        app.session_manager
            .insert_spawned(&feat, b, &std::env::temp_dir(), 24, 80)
            .unwrap();
        app.active_session = Some(main.clone());

        app.set_worktree_agent("main", "codex");

        assert_eq!(
            app.config.worktree_agents.get("main"),
            Some(&"codex".to_string()),
            "the choice must be recorded in config"
        );
        assert!(
            app.session_manager.get(&main).is_none(),
            "switching agents must restart (kill) the chosen worktree's session"
        );
        assert!(
            app.session_manager.get(&feat).is_some(),
            "switching one worktree's agent must leave every other session intact"
        );
        assert_eq!(
            app.active_session, None,
            "active_session must clear so the new cmd respawns next frame"
        );
        assert!(app.status.is_some(), "the switch must report status");

        let persisted = Config::load_from(&cfg_path).unwrap();
        assert_eq!(
            persisted.worktree_agents.get("main"),
            Some(&"codex".to_string()),
            "the choice must survive a restart (persisted to config_path)"
        );
    }

    #[test]
    fn set_worktree_agent_rejects_unknown_preset_without_restart_or_persist() {
        use portable_pty::CommandBuilder;

        let dir = tempfile::tempdir().unwrap();
        let cfg_path = dir.path().join("config.toml");
        let mut app = app_with_fake_worktrees();
        app.config.agents = vec![crate::config::AgentPreset {
            name: "codex".to_string(),
            cmd: "codex --model x".to_string(),
        }];
        app.config_path = Some(cfg_path.clone());

        let main = SessionManager::session_name("main");
        let mut a = CommandBuilder::new("printf");
        a.args(["a"]);
        app.session_manager
            .insert_spawned(&main, a, &std::env::temp_dir(), 24, 80)
            .unwrap();
        app.active_session = Some(main.clone());

        app.set_worktree_agent("main", "bogus");

        assert_eq!(
            app.config.worktree_agents.get("main"),
            None,
            "an unknown preset name must not be recorded"
        );
        assert!(
            app.session_manager.get(&main).is_some(),
            "rejecting an unknown preset must not restart the session"
        );
        assert_eq!(
            app.active_session,
            Some(main),
            "rejecting an unknown preset must not clear the active session"
        );
        let status = app.status.as_deref().unwrap_or_default();
        assert!(
            status.contains("unknown agent 'bogus'") && status.contains("codex"),
            "status must name the rejected input and list valid presets, got: {status}"
        );
        assert!(
            !cfg_path.exists(),
            "a rejected switch must not persist the config"
        );
    }

    // --- issue #54: per-repo base ref for NEW-branch worktrees --------------
    //
    // TDD RED (acceptance criterion #1): `App::add_worktree` on an UNKNOWN branch
    // forks the new branch from the selected repo's `base_ref` when set, and from
    // HEAD when unset (behavior identical to today). Exercised against a real git
    // repo via the existing `init_git_repo`/`app_for_repo` seam.

    fn rev_parse(repo: &Path, rev: &str) -> String {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(["rev-parse", rev])
            .output()
            .expect("git rev-parse");
        assert!(
            out.status.success(),
            "git rev-parse {rev} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    fn git_in(repo: &Path, args: &[&str]) {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .output()
            .expect("git must be installed");
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    #[test]
    fn add_worktree_uses_repo_base_ref_for_new_branch() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().to_path_buf();
        init_git_repo(&repo);
        // base ref pinned at commit A; HEAD then advances to B.
        let base_commit = rev_parse(&repo, "HEAD");
        git_in(&repo, &["branch", "the-base"]);
        git_in(&repo, &["commit", "--allow-empty", "-m", "B"]);
        let head_commit = rev_parse(&repo, "HEAD");
        assert_ne!(base_commit, head_commit);

        let mut app = app_for_repo(repo.clone());
        app.config.repos[0].base_ref = Some("the-base".to_string());

        app.add_worktree("brand-new");

        let wt = repo.join(".worktrees").join("brand-new");
        let wt_head = rev_parse(&wt, "HEAD");
        assert_eq!(
            wt_head, base_commit,
            "add_worktree must start the new branch at the repo's base_ref, not HEAD"
        );
        assert_ne!(wt_head, head_commit);
    }

    #[test]
    fn add_worktree_without_base_ref_branches_from_head() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().to_path_buf();
        init_git_repo(&repo);
        git_in(&repo, &["commit", "--allow-empty", "-m", "B"]);
        let head_commit = rev_parse(&repo, "HEAD");

        let mut app = app_for_repo(repo.clone());
        app.config.repos[0].base_ref = None; // explicit: unset -> current behavior

        app.add_worktree("brand-new");

        let wt = repo.join(".worktrees").join("brand-new");
        let wt_head = rev_parse(&wt, "HEAD");
        assert_eq!(
            wt_head, head_commit,
            "with base_ref unset, a new branch forks from HEAD (unchanged)"
        );
    }

    // --- issue #48: per-worktree TABS (multiple surfaces, no split panes) ----
    //
    // TDD RED: each worktree owns a `WorktreeLayout` (keyed by slug in
    // `app.layouts`). Tab 0 is the AGENT (`wtcc-<slug>`); `new_shell_tab` appends
    // a SHELL surface (`wtcc-<slug>-t<n>`), focuses it, and only mutates the
    // in-memory model — the real spawn happens next frame in
    // `ensure_active_session` (which drives the active tab and sets
    // `active_session`). `close_tab` kills the removed shell's session (and only
    // that one), refusing on the agent/last tab with a status. `next_tab`/
    // `prev_tab` cycle with wrap. Switching worktrees restores each layout.
    // `insert_spawned` seeds live sessions so the kill side-effect is observable
    // without tmux.

    #[test]
    fn new_shell_tab_creates_the_layout_and_appends_a_focused_shell() {
        let mut app = app_with_fake_worktrees(); // main selected
        assert!(!app.layouts.contains_key("main"));

        app.new_shell_tab();

        let layout = app
            .layouts
            .get("main")
            .expect("new_shell_tab must create the worktree's layout");
        assert_eq!(layout.tabs.len(), 2, "agent tab + one shell tab");
        assert_eq!(layout.active, 1, "the new shell tab is focused");
        assert_eq!(layout.tabs[0].session, SessionManager::session_name("main"));
        assert_eq!(layout.tabs[0].kind, crate::layout::TabKind::Agent);
        assert_eq!(layout.tabs[1].session, "wtcc-main-t1");
        assert_eq!(layout.tabs[1].kind, crate::layout::TabKind::Shell);
    }

    #[test]
    fn next_tab_and_prev_tab_cycle_the_current_layout() {
        let mut app = app_with_fake_worktrees();
        app.new_shell_tab(); // [agent, shell] active 1

        app.next_tab(); // wrap to 0
        assert_eq!(app.layouts.get("main").unwrap().active, 0);
        app.prev_tab(); // wrap to 1
        assert_eq!(app.layouts.get("main").unwrap().active, 1);
    }

    #[test]
    fn close_tab_kills_only_the_removed_shell_session_and_keeps_the_agent() {
        use portable_pty::CommandBuilder;

        let mut app = app_with_fake_worktrees();
        app.new_shell_tab(); // active shell, session wtcc-main-t1
        let agent = SessionManager::session_name("main"); // wtcc-main
        let shell = "wtcc-main-t1".to_string();
        let mut a = CommandBuilder::new("printf");
        a.args(["a"]);
        let mut b = CommandBuilder::new("printf");
        b.args(["b"]);
        app.session_manager
            .insert_spawned(&agent, a, &std::env::temp_dir(), 24, 80)
            .unwrap();
        app.session_manager
            .insert_spawned(&shell, b, &std::env::temp_dir(), 24, 80)
            .unwrap();
        app.active_session = Some(shell.clone());

        app.close_tab();

        assert!(
            app.session_manager.get(&shell).is_none(),
            "closing a shell tab kills its named session"
        );
        assert!(
            app.session_manager.get(&agent).is_some(),
            "the agent (tab 0) session must survive a shell-tab close"
        );
        let layout = app.layouts.get("main").unwrap();
        assert_eq!(layout.tabs.len(), 1);
        assert_eq!(layout.active, 0, "focus falls back to the agent tab");
        assert_eq!(
            app.active_session, None,
            "active_session clears when the closed tab was active (respawns next frame)"
        );
    }

    #[test]
    fn close_tab_refuses_the_agent_tab_with_a_status() {
        let mut app = app_with_fake_worktrees();
        app.new_shell_tab();
        app.close_tab(); // remove the shell -> agent-only
        app.status = None;

        app.close_tab(); // only the agent tab remains: guarded no-op

        let layout = app.layouts.get("main").unwrap();
        assert_eq!(layout.tabs.len(), 1, "the agent tab is not closable");
        assert_eq!(layout.active, 0);
        assert!(
            app.status.is_some(),
            "a refused close reports a status message"
        );
    }

    #[test]
    fn ensure_active_session_creates_the_layout_and_activates_the_agent_tab() {
        use portable_pty::CommandBuilder;

        let mut app = app_with_fake_worktrees(); // main selected
        let agent = SessionManager::session_name("main");
        let mut a = CommandBuilder::new("printf");
        a.args(["x"]);
        // Pre-seed the agent session so the active tab's ensure reuses it
        // (idempotent) — no tmux needed in the unit test.
        app.session_manager
            .insert_spawned(&agent, a, &std::env::temp_dir(), 24, 80)
            .unwrap();

        app.ensure_active_session(24, 80);

        let layout = app
            .layouts
            .get("main")
            .expect("ensure_active_session must seed the worktree layout");
        assert_eq!(
            layout.tabs.len(),
            1,
            "a fresh worktree has only the agent tab"
        );
        assert_eq!(layout.tabs[0].kind, crate::layout::TabKind::Agent);
        assert_eq!(
            app.active_session.as_deref(),
            Some(agent.as_str()),
            "the active tab's session becomes active_session"
        );
    }

    #[test]
    fn each_worktree_keeps_its_own_tab_layout_across_switches() {
        let mut app = app_with_fake_worktrees(); // main(0), feat(1), selected 0
        app.new_shell_tab(); // main: [agent, t1] active 1

        app.selected_worktree = Some(1); // switch to feat
        app.new_shell_tab();
        app.new_shell_tab(); // feat: [agent, t1, t2] active 2

        app.selected_worktree = Some(0); // back to main

        let main = app.layouts.get("main").unwrap();
        assert_eq!(main.tabs.len(), 2);
        assert_eq!(main.active, 1);
        assert_eq!(main.tabs[1].session, "wtcc-main-t1");

        let feat = app.layouts.get("feat").unwrap();
        assert_eq!(feat.tabs.len(), 3);
        assert_eq!(feat.active, 2);
        assert_eq!(feat.tabs[2].session, "wtcc-feat-t2");
    }

    #[test]
    fn current_slug_slugifies_the_selected_branch() {
        let mut app = app_with_fake_worktrees();
        app.worktrees.push(Worktree {
            path: PathBuf::from("/repo/feature"),
            branch: "Feature/Big Thing".to_string(),
            head: "z".to_string(),
            is_bare: false,
            is_detached: false,
        });
        app.selected_worktree = Some(app.worktrees.len() - 1);
        assert_eq!(
            app.current_slug().as_deref(),
            Some("feature-big-thing"),
            "current_slug must slugify untrusted branch names before they key state"
        );
    }

    // --- issue #56: per-repo `run` command into a Run tab -------------------
    //
    // TDD RED: `App::start_run_script` opens a dedicated Run tab for the selected
    // worktree (reusing the #48 tabs surface, NOT a bespoke pane toggle) backed
    // by the `wtcc-run-<slug>` session. With no `run` configured it spawns nothing
    // and explains via status. Removing the worktree kills the run session along
    // with every other tab surface (extends the existing remove cleanup).
    // `insert_spawned` seeds live sessions so the kill side-effect is observable
    // without tmux.

    #[test]
    fn start_run_script_with_no_run_configured_opens_no_tab_and_sets_status() {
        let mut app = app_with_fake_worktrees(); // main selected, run defaults None
        assert_eq!(app.config.repos[0].run, None);
        app.status = None;

        app.start_run_script();

        let no_run_tab = app
            .layouts
            .get("main")
            .is_none_or(|l| l.tabs.iter().all(|t| t.kind != crate::layout::TabKind::Run));
        assert!(no_run_tab, "no run script -> no Run tab is opened");
        assert!(
            app.status.is_some(),
            "no run script must explain via status"
        );
        assert!(
            app.session_manager
                .get(&crate::session::run_session_name("main"))
                .is_none(),
            "no run script must spawn nothing"
        );
    }

    #[test]
    fn start_run_script_with_run_configured_appends_a_focused_run_tab() {
        let mut app = app_with_fake_worktrees(); // main selected
        app.config.repos[0].run = Some("pnpm dev".to_string());

        app.start_run_script();

        let layout = app
            .layouts
            .get("main")
            .expect("start_run_script must create/keep the worktree layout");
        assert!(
            layout.tabs.len() >= 2,
            "a Run tab is appended alongside the agent tab"
        );
        let run_tab = layout
            .tabs
            .iter()
            .find(|t| t.kind == crate::layout::TabKind::Run)
            .expect("a Run tab must be appended");
        assert_eq!(
            run_tab.session,
            crate::session::run_session_name("main"),
            "the Run tab is backed by the wtcc-run-<slug> session"
        );
        assert_eq!(run_tab.session, "wtcc-run-main");
        assert!(
            run_tab.title == "run" || run_tab.title == "pnpm dev",
            "a run tab's title is 'run' or the command, got {:?}",
            run_tab.title
        );
        assert_eq!(
            layout.active_tab().kind,
            crate::layout::TabKind::Run,
            "the appended Run tab is focused"
        );
    }

    #[test]
    fn remove_worktree_kills_the_run_session_and_keeps_others() {
        use portable_pty::CommandBuilder;

        let mut app = app_with_fake_worktrees(); // main(/repo/main), feat(/repo/feat)
        app.config.repos[0].run = Some("pnpm dev".to_string());
        // Open the run tab so the run session is part of main's layout.
        app.start_run_script();

        let run = crate::session::run_session_name("main"); // wtcc-run-main
        let feat = SessionManager::session_name("feat");
        let mut a = CommandBuilder::new("printf");
        a.args(["a"]);
        let mut b = CommandBuilder::new("printf");
        b.args(["b"]);
        app.session_manager
            .insert_spawned(&run, a, &std::env::temp_dir(), 24, 80)
            .unwrap();
        app.session_manager
            .insert_spawned(&feat, b, &std::env::temp_dir(), 24, 80)
            .unwrap();

        app.remove_worktree(&PathBuf::from("/repo/main"));

        assert!(
            app.session_manager.get(&run).is_none(),
            "removing a worktree must kill its wtcc-run-<slug> session"
        );
        assert!(
            app.session_manager.get(&feat).is_some(),
            "removing one worktree must leave every other worktree's session intact"
        );
    }
}
