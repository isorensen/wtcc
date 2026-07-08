use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use portable_pty::{CommandBuilder, MasterPty, PtySize, native_pty_system};
use vt100::Parser;

/// Retained scrollback lines per agent PTY, enabling mouse-wheel scrollback
/// (#106). The agent is a full-screen TUI that redraws continuously, so we keep
/// a generous buffer and only ever snap back to the live bottom on a keypress.
const SCROLLBACK_LINES: usize = 5000;

/// A worktree's agent gets one of three states, derived purely from how
/// recently its PTY produced output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivityState {
    /// Output arrived within [`WORKING_WINDOW`]: the agent is doing something.
    Working,
    /// A session exists but has been quiet for longer than [`WORKING_WINDOW`].
    Idle,
    /// No session exists for this worktree.
    None,
}

/// How recently a session must have produced output to count as `Working`.
pub const WORKING_WINDOW: Duration = Duration::from_millis(1000);

/// argv for re-keying a tmux session in place: `tmux rename-session -t <old>
/// <new>`. Both names are already-slugified `wtcc-<slug>` keys, passed as
/// discrete argv elements — never via a shell.
pub fn rename_session_argv(old: &str, new: &str) -> Vec<String> {
    vec![
        "rename-session".to_string(),
        "-t".to_string(),
        old.to_string(),
        new.to_string(),
    ]
}

/// The dedicated tmux session name for a worktree's Run tab: `wtcc-run-<slug>`,
/// distinct from the agent (`wtcc-<slug>`) and shell (`wtcc-<slug>-t<n>`)
/// surfaces so kill-on-remove can target it precisely. `branch` is slugified so
/// untrusted names are safe in the session name.
pub fn run_session_name(branch: &str) -> String {
    format!("wtcc-run-{}", crate::worktree::slugify(branch))
}

/// Builds the tmux argv for a Run tab: `new-session -A -s <name> -c <cwd> <command>`.
/// SECURITY/CORRECTNESS: the user-authored `command` is the SINGLE, un-interpolated
/// trailing element — tmux hands it to `$SHELL -c "<command>"`, so the shell (not
/// wtcc) parses it. We deliberately do NOT add our own `sh -c` wrapper: tmux joins
/// trailing positional args with spaces, so `["sh","-c",command]` would collapse to
/// `sh -c <command>`, the outer `$SHELL -c` would word-split it, and the inner `sh`
/// would run only the first word (e.g. `pnpm dev` → just `pnpm`). One trailing
/// element keeps the whole command intact. `-c <cwd>` pins the start dir for a
/// FRESH create; tmux ignores it when `-A` reattaches, so the reattach-to-a-dead-cwd
/// case (#116) is handled by `reap_if_cwd_missing`, not `-c`. `-c` is kept as
/// belt-and-suspenders for the create path. No slug/branch/path is concatenated in.
pub fn run_argv(name: &str, command: &str, cwd: &Path) -> Vec<String> {
    vec![
        "new-session".to_string(),
        "-A".to_string(),
        "-s".to_string(),
        name.to_string(),
        "-c".to_string(),
        cwd.to_string_lossy().into_owned(),
        command.to_string(),
    ]
}

/// Fixed `sh -c` script (NO user input) that runs the agent with its real argv
/// (`$0`/`$@`) and, when it exits, replaces the process with an interactive
/// login shell in the same PTY/cwd so the agent tab never dies as `[exited]`.
const AGENT_FALLBACK_SCRIPT: &str = r#""$0" "$@"; exec "${SHELL:-/bin/sh}""#;

/// argv for the AGENT surface: `new-session -A -s <name> -c <cwd> sh -c <script> <agent-tokens...>`.
/// The agent command is whitespace-split into DISCRETE trailing positional params
/// (`$0`, `$@`), never interpolated into the script string — same argv the agent
/// got before, now with a shell fallback on exit. tmux execs the multi-arg command
/// vector directly (no extra word-splitting). `-c <cwd>` pins the start dir for a
/// FRESH create; tmux ignores it when `-A` reattaches, so the reattach-to-a-dead-cwd
/// case (#116) is handled by `reap_if_cwd_missing`, not `-c` (kept belt-and-suspenders).
pub fn agent_argv(session_name: &str, command: &str, cwd: &Path) -> Vec<String> {
    let mut argv = vec![
        "new-session".to_string(),
        "-A".to_string(),
        "-s".to_string(),
        session_name.to_string(),
        "-c".to_string(),
        cwd.to_string_lossy().into_owned(),
        "sh".to_string(),
        "-c".to_string(),
        AGENT_FALLBACK_SCRIPT.to_string(),
    ];
    argv.extend(command.split_whitespace().map(str::to_string));
    argv
}

/// Decides whether a tmux `#{pane_current_path}` value points at a directory that
/// no longer exists. tmux reports a removed cwd as the original path with a
/// literal `" (deleted)"` suffix (it readlinks `/proc/<pid>/cwd`), which never
/// matches a real path — so that form is correctly treated as missing. An empty
/// value (query returned nothing) is treated as present: there is nothing to reap.
///
/// Only a `NotFound` stat is treated as missing. `Path::exists()` collapses EVERY
/// `metadata` error (a transient permission flip, a stale NFS/SSHFS handle) to
/// `false`, which would reap — and thus kill — a perfectly HEALTHY session on a
/// blip. We narrow to `ErrorKind::NotFound`, the only kind that means the dir is
/// genuinely gone (this still flags tmux's `<path> (deleted)` marker, which stats
/// as NotFound).
fn cwd_is_missing(pane_path: &str) -> bool {
    let p = pane_path.trim();
    if p.is_empty() {
        return false;
    }
    matches!(std::fs::metadata(p), Err(e) if e.kind() == std::io::ErrorKind::NotFound)
}

/// If a tmux session named `name` already exists but its working directory was
/// removed out-of-band, kill it so the subsequent `new-session -A` creates a
/// fresh session in a valid cwd instead of reattaching to a dead shell.
///
/// This — NOT the `-c <cwd>` flag — is what rescues a poisoned session: tmux
/// ignores `-c` when `-A` reattaches to a pre-existing session, so the start dir
/// can only be re-pinned by killing the session and letting the next
/// `new-session` create it afresh (#116).
///
/// Returns `true` when it actually killed a session. Callers use that signal to
/// tolerate a post-reap spawn race: reaping the server's LAST session tears the
/// server down asynchronously, so the immediate recreate may need a retry.
/// Best-effort: tmux/query errors are ignored (nothing to reap => `false`).
fn reap_if_cwd_missing(name: &str) -> bool {
    let out = std::process::Command::new("tmux")
        .args([
            "display-message",
            "-p",
            "-t",
            name,
            "-F",
            "#{pane_current_path}",
        ])
        .output();
    if let Ok(o) = out
        && o.status.success()
        && cwd_is_missing(&String::from_utf8_lossy(&o.stdout))
    {
        let _ = std::process::Command::new("tmux")
            .args(["kill-session", "-t", name])
            .output();
        return true;
    }
    false
}

/// Runs `attempt` to spawn a session, retrying only when a reap just happened.
///
/// A reap that emptied the tmux server triggers an asynchronous server teardown
/// (`exit-empty on`); an immediate `new-session` can race the dying server and
/// fail with `server exited unexpectedly`. When `reaped` is true we retry a few
/// times with a short backoff to let the server settle. When `reaped` is false a
/// spawn failure is real and surfaces immediately — no sleep on the happy path.
fn spawn_after_reap<T, F>(reaped: bool, mut attempt: F) -> anyhow::Result<T>
where
    F: FnMut() -> anyhow::Result<T>,
{
    match attempt() {
        Ok(s) => Ok(s),
        Err(e) if reaped => {
            let mut last = e;
            for _ in 0..5 {
                std::thread::sleep(Duration::from_millis(80));
                match attempt() {
                    Ok(s) => return Ok(s),
                    Err(e) => last = e,
                }
            }
            Err(last)
        }
        Err(e) => Err(e),
    }
}

/// Classifies a session's activity from how long it has been idle. `None` input
/// means there is no session. Pure and total: the sole place the threshold is
/// applied, so the heuristic is unit-testable without a PTY.
pub fn activity_from_idle(idle: Option<Duration>) -> ActivityState {
    match idle {
        None => ActivityState::None,
        Some(d) if d < WORKING_WINDOW => ActivityState::Working,
        Some(_) => ActivityState::Idle,
    }
}

/// Probes a vt100 screen for its scrollback bounds `(cur, max)`, restoring the
/// original offset before returning (#122). Pure over the passed screen so it is
/// unit-testable against a hand-fed `Parser` without a PTY: `set_scrollback(usize::MAX)`
/// clamps to the oldest retained line, exposing the max offset.
pub(crate) fn scrollback_bounds(screen: &mut vt100::Screen) -> (usize, usize) {
    let cur = screen.scrollback();
    screen.set_scrollback(usize::MAX);
    let max = screen.scrollback();
    screen.set_scrollback(cur);
    (cur, max)
}

/// Snapshots EVERY logical line of the scrollback plus the visible screen into
/// plain strings, oldest first, restoring the original offset before returning
/// (#123). vt100 exposes only the visible window, so we walk the scrollback
/// offset from the oldest retained line (`max`) down to the live bottom, reading
/// the TOP visible row at each step, then append the remaining current-screen
/// rows. Pure over the passed screen: unit-testable against a hand-fed `Parser`
/// without a PTY. `max == 0` yields exactly the visible screen (`rows` lines).
pub(crate) fn scrollback_lines(screen: &mut vt100::Screen) -> Vec<String> {
    let (rows, cols) = screen.size();
    let cur = screen.scrollback();
    let (_, max) = scrollback_bounds(screen);
    let mut lines: Vec<String> = Vec::with_capacity(max + rows as usize);
    // At offset `k` the top visible row is absolute line `max - k`, so walking
    // `k` from `max` down to `0` yields lines `0..=max` oldest-first.
    for k in (0..=max).rev() {
        screen.set_scrollback(k);
        if let Some(top) = screen.rows(0, cols).next() {
            lines.push(top);
        }
    }
    // At offset 0 the top row (`max`) was already captured; append the rest of
    // the current screen (`max+1 ..= max+rows-1`).
    screen.set_scrollback(0);
    lines.extend(screen.rows(0, cols).skip(1));
    screen.set_scrollback(cur);
    lines
}

/// Char-column start offsets of every smart-case, non-overlapping occurrence of
/// `query` in `row` (#123). Char index equals cell column for the plain agent
/// rows this highlights. Smart-case: case-insensitive when `query` has no
/// uppercase, case-sensitive otherwise. Empty query → no spans.
pub(crate) fn match_columns(row: &str, query: &str) -> Vec<usize> {
    let needle: Vec<char> = query.chars().collect();
    if needle.is_empty() {
        return Vec::new();
    }
    let case_sensitive = needle.iter().any(|c| c.is_uppercase());
    let hay: Vec<char> = row.chars().collect();
    let n = needle.len();
    let eq = |a: char, b: char| {
        if case_sensitive {
            a == b
        } else {
            a.eq_ignore_ascii_case(&b)
        }
    };
    let mut cols = Vec::new();
    let mut i = 0;
    while i + n <= hay.len() {
        if (0..n).all(|j| eq(hay[i + j], needle[j])) {
            cols.push(i);
            i += n;
        } else {
            i += 1;
        }
    }
    cols
}

/// Indices of `lines` containing `query`, smart-case (#123). Shares
/// [`match_columns`]'s matcher so the lines reported here are exactly the ones
/// that render a highlight. Empty query → empty vec.
pub fn find_matches(lines: &[String], query: &str) -> Vec<usize> {
    if query.is_empty() {
        return Vec::new();
    }
    lines
        .iter()
        .enumerate()
        .filter(|(_, l)| !match_columns(l, query).is_empty())
        .map(|(i, _)| i)
        .collect()
}

/// The scrollback offset that brings absolute `line` to the viewport top (#123).
/// At offset `k` the visible window is lines `[total-rows-k, total-1-k]`, so
/// `top == line` gives `k = total - rows - line` (saturating; the real value is
/// clamped by `set_scrollback` when applied).
pub fn offset_for_line(line: usize, total: usize, rows: usize) -> usize {
    total.saturating_sub(rows).saturating_sub(line)
}

/// How long a session must stay quiet before it counts as needing attention.
/// The same fixed threshold for every session — intentionally not configurable.
pub const ATTENTION_QUIET: Duration = Duration::from_secs(10);

/// Coarse busy/quiet classification used by [`AttentionTracker`]. A session is
/// `Quiet` once it has been idle for at least [`ATTENTION_QUIET`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Busy,
    Quiet,
}

fn phase(idle: Duration) -> Phase {
    if idle >= ATTENTION_QUIET {
        Phase::Quiet
    } else {
        Phase::Busy
    }
}

/// Edge-triggered "needs attention" detector over per-session idle durations.
///
/// Pure and time-injected: callers feed a snapshot of `(session_name, idle)`
/// pairs each poll, so the heuristic is unit-testable without a PTY clock. A
/// session is flagged exactly once on the Busy→Quiet edge; it only re-fires
/// after going Busy again. The currently active session is never flagged, and
/// selecting a flagged session clears its marker. Names absent from a snapshot
/// are pruned.
#[derive(Debug, Default)]
pub struct AttentionTracker {
    phases: HashMap<String, Phase>,
    needs: HashSet<String>,
}

impl AttentionTracker {
    /// Folds a fresh snapshot into the tracker, returning the session names that
    /// newly crossed the Busy→Quiet edge this poll (suppressing `active`).
    pub fn poll(&mut self, snapshot: &[(String, Duration)], active: Option<&str>) -> Vec<String> {
        let mut fired = Vec::new();
        for (name, idle) in snapshot {
            let next = phase(*idle);
            let prev = self.phases.insert(name.clone(), next);
            // The active session, or any session back to work, is not flagged.
            if active == Some(name.as_str()) || next == Phase::Busy {
                self.needs.remove(name);
                continue;
            }
            if prev == Some(Phase::Busy) && self.needs.insert(name.clone()) {
                fired.push(name.clone());
            }
        }
        let present: HashSet<&str> = snapshot.iter().map(|(n, _)| n.as_str()).collect();
        self.phases.retain(|k, _| present.contains(k.as_str()));
        self.needs.retain(|k| present.contains(k.as_str()));
        fired
    }

    /// Whether the session named `name` is currently flagged for attention.
    pub fn needs(&self, name: &str) -> bool {
        self.needs.contains(name)
    }

    /// How many sessions are currently flagged.
    pub fn count(&self) -> usize {
        self.needs.len()
    }
}

/// A single agent terminal: a PTY running a `tmux new-session -A` attach child,
/// with a background reader thread feeding bytes into a vt100 parser.
pub struct Session {
    master: Box<dyn MasterPty + Send>,
    child: Box<dyn portable_pty::Child + Send + Sync>,
    reader: Option<JoinHandle<()>>,
    parser: Arc<Mutex<Parser>>,
    writer: Mutex<Box<dyn Write + Send>>,
    /// Instant of the most recent PTY output, bumped by the reader thread on
    /// every non-empty read. Read under the lock to derive the agent's activity.
    last_activity: Arc<Mutex<Instant>>,
}

impl Session {
    /// Spawns a named tmux/PTY surface.
    ///
    /// For an agent tab (`Some(command)`), tmux runs the agent under a fixed
    /// `sh -c` wrapper ([`agent_argv`]) that execs an interactive shell in the
    /// same PTY/cwd once the agent exits, so the surface stays usable (and `R`
    /// can relaunch a fresh agent) instead of dying as `[exited]`. The agent
    /// command is whitespace-split into DISCRETE trailing argv params — never
    /// interpolated into the script, so there is no shell-metacharacter surface.
    ///
    /// For a shell tab (`None`), behavior is unchanged: `tmux new-session -A -s
    /// <name>` launches its default shell (the user's `$SHELL`, falling back to
    /// `/bin/sh`) with no wrapper.
    pub fn spawn(
        session_name: &str,
        command: Option<&str>,
        cwd: &Path,
        rows: u16,
        cols: u16,
    ) -> anyhow::Result<Session> {
        let mut cmd = CommandBuilder::new("tmux");
        if let Some(command) = command {
            for arg in agent_argv(session_name, command, cwd) {
                cmd.arg(arg);
            }
        } else {
            // `-c <cwd>` pins the start dir only for a fresh create; on an `-A`
            // reattach tmux ignores it, so a dead-cwd session is rescued by
            // `reap_if_cwd_missing` upstream, not this flag (#116).
            let cwd_str = cwd.to_string_lossy();
            cmd.args([
                "new-session",
                "-A",
                "-s",
                session_name,
                "-c",
                cwd_str.as_ref(),
            ]);
        }
        Self::spawn_with_command(cmd, cwd, rows, cols)
    }

    fn spawn_with_command(
        mut cmd: CommandBuilder,
        cwd: &Path,
        rows: u16,
        cols: u16,
    ) -> anyhow::Result<Session> {
        cmd.cwd(cwd);
        let pty = native_pty_system().openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })?;
        let child = pty.slave.spawn_command(cmd)?;
        // Drop the slave so the PTY EOFs once the child exits; otherwise the
        // reader thread would block forever holding the write end open.
        drop(pty.slave);

        let mut reader_handle = pty.master.try_clone_reader()?;
        let writer = pty.master.take_writer()?;
        let parser = Arc::new(Mutex::new(Parser::new(rows, cols, SCROLLBACK_LINES)));
        let last_activity = Arc::new(Mutex::new(Instant::now()));

        let parser_clone = Arc::clone(&parser);
        let activity_clone = Arc::clone(&last_activity);
        let handle = std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            loop {
                match reader_handle.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        parser_clone.lock().unwrap().process(&buf[..n]);
                        *activity_clone.lock().unwrap() = Instant::now();
                    }
                }
            }
        });

        Ok(Session {
            master: pty.master,
            child,
            reader: Some(handle),
            parser,
            writer: Mutex::new(writer),
            last_activity,
        })
    }

    pub fn write_input(&self, bytes: &[u8]) -> anyhow::Result<()> {
        let mut writer = self.writer.lock().unwrap();
        writer.write_all(bytes)?;
        writer.flush()?;
        Ok(())
    }

    /// Scrolls the view back by `delta` rows (clamped to available history).
    pub fn scroll_up(&self, delta: usize) {
        let mut p = self.parser.lock().unwrap();
        let cur = p.screen().scrollback();
        p.screen_mut().set_scrollback(cur.saturating_add(delta));
    }

    /// Scrolls the view toward the live bottom by `delta` rows.
    pub fn scroll_down(&self, delta: usize) {
        let mut p = self.parser.lock().unwrap();
        let cur = p.screen().scrollback();
        p.screen_mut().set_scrollback(cur.saturating_sub(delta));
    }

    /// Snaps the view back to the live bottom (offset 0).
    pub fn scroll_to_bottom(&self) {
        self.parser.lock().unwrap().screen_mut().set_scrollback(0);
    }

    /// Rows on the visible screen — the page size for keyboard scroll paging (#122).
    pub fn view_rows(&self) -> usize {
        self.parser.lock().unwrap().screen().size().0 as usize
    }

    /// Scrolls the view to the oldest retained line (#122). `usize::MAX` is
    /// clamped to the scrollback length by vt100.
    pub fn scroll_to_top(&self) {
        self.parser
            .lock()
            .unwrap()
            .screen_mut()
            .set_scrollback(usize::MAX);
    }

    /// The current and maximum scrollback offsets `(cur, max)` under one lock,
    /// feeding the SCROLL-mode pane title `[cur/max]` indicator (#122).
    pub fn scrollback_view(&self) -> (usize, usize) {
        scrollback_bounds(self.parser.lock().unwrap().screen_mut())
    }

    /// Snapshots every logical scrollback line for an in-scroll-mode search
    /// (#123). Locks the parser and delegates to the pure [`scrollback_lines`],
    /// which restores the live offset before returning.
    pub fn scrollback_lines(&self) -> Vec<String> {
        scrollback_lines(self.parser.lock().unwrap().screen_mut())
    }

    /// Scrolls so the absolute `line` (into a [`scrollback_lines`] snapshot of
    /// length `total`) sits at the viewport top (#123). vt100 clamps the offset.
    pub fn jump_to_line(&self, line: usize, total: usize) {
        let mut p = self.parser.lock().unwrap();
        let rows = p.screen().size().0 as usize;
        p.screen_mut()
            .set_scrollback(offset_for_line(line, total, rows));
    }

    pub fn resize(&self, rows: u16, cols: u16) -> anyhow::Result<()> {
        if rows == 0 || cols == 0 {
            return Ok(());
        }
        self.master.resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })?;
        self.parser
            .lock()
            .unwrap()
            .screen_mut()
            .set_size(rows, cols);
        Ok(())
    }

    pub fn parser(&self) -> &Arc<Mutex<Parser>> {
        &self.parser
    }

    /// How long since this session last produced PTY output. A poisoned lock
    /// (reader thread panicked) degrades to `Duration::MAX` rather than
    /// panicking, which classifies as `Idle` downstream.
    pub fn idle_for(&self) -> Duration {
        match self.last_activity.lock() {
            Ok(last) => last.elapsed(),
            Err(_) => Duration::MAX,
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        // We deliberately do NOT run `tmux kill-server`/`kill-session` — the
        // tmux session must persist for reattach. Killing the local
        // `tmux new-session -A` attach child only detaches us, which is fine.
        let _ = self.child.kill();
        if let Some(h) = self.reader.take() {
            let _ = h.join();
        }
    }
}

pub struct SessionManager {
    sessions: HashMap<String, Session>,
}

impl SessionManager {
    pub fn new() -> Self {
        Self {
            sessions: HashMap::new(),
        }
    }

    pub fn session_name(branch: &str) -> String {
        format!("wtcc-{}", crate::worktree::slugify(branch))
    }

    /// Branch-keyed agent session wrapper: maps `branch` to `wtcc-<slug>` and
    /// reuses-or-spawns it via [`ensure_named`](Self::ensure_named) with the agent
    /// command. Tab 0 (the agent) reattaches exactly as before tabs existed.
    pub fn ensure(
        &mut self,
        branch: &str,
        cwd: &Path,
        agent_cmd: &str,
        rows: u16,
        cols: u16,
    ) -> anyhow::Result<&Session> {
        let name = Self::session_name(branch);
        self.ensure_named(&name, cwd, Some(agent_cmd), rows, cols)
    }

    /// The named-session primitive behind every surface (agent and shell tabs):
    /// reuses the live session registered under `name`, or spawns one running
    /// `command` (`None` => default shell) and registers it. Idempotent — a second
    /// call for a live name never respawns or errors.
    pub fn ensure_named(
        &mut self,
        name: &str,
        cwd: &Path,
        command: Option<&str>,
        rows: u16,
        cols: u16,
    ) -> anyhow::Result<&Session> {
        if !self.sessions.contains_key(name) {
            let reaped = reap_if_cwd_missing(name);
            let s = spawn_after_reap(reaped, || Session::spawn(name, command, cwd, rows, cols))?;
            self.sessions.insert(name.to_string(), s);
        }
        Ok(self.sessions.get(name).unwrap())
    }

    /// Ensures a Run-tab session named `name` running the user-authored `command`
    /// in `cwd`. tmux runs the command via `$SHELL -c "<command>"`, so `command`
    /// is passed as a single, un-interpolated trailing argv element (see [`run_argv`])
    /// and `cwd` is set on the PTY child — never string-built. Funnels into the same
    /// `Session::spawn_with_command` core as every other surface; idempotent like
    /// [`ensure_named`](Self::ensure_named).
    pub fn ensure_run(
        &mut self,
        name: &str,
        command: &str,
        cwd: &Path,
        rows: u16,
        cols: u16,
    ) -> anyhow::Result<&Session> {
        if !self.sessions.contains_key(name) {
            let reaped = reap_if_cwd_missing(name);
            let session = spawn_after_reap(reaped, || {
                let mut cmd = CommandBuilder::new("tmux");
                for arg in run_argv(name, command, cwd) {
                    cmd.arg(arg);
                }
                Session::spawn_with_command(cmd, cwd, rows, cols)
            })?;
            self.sessions.insert(name.to_string(), session);
        }
        Ok(self.sessions.get(name).unwrap())
    }

    pub fn get(&self, name: &str) -> Option<&Session> {
        self.sessions.get(name)
    }

    /// Kills the agent for the session named `name`: drops the local `Session`
    /// (its `Drop` detaches the PTY attach child) and best-effort kills the
    /// underlying tmux session so a fresh agent is spawned on reattach. `name` is
    /// already slugified (`wtcc-<slug>`); it is passed argv-only, never via a
    /// shell. A missing tmux session reports an error that is intentionally
    /// ignored — the manager-level removal is what matters and only touches the
    /// named session, never any other worktree's.
    pub fn kill(&mut self, name: &str) {
        self.sessions.remove(name);
        let _ = std::process::Command::new("tmux")
            .args(["kill-session", "-t", name])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }

    /// Re-keys the live session `old` to `new` WITHOUT killing the agent: the
    /// local `Session` is moved under the new map key and the underlying tmux
    /// session is renamed in place (argv-only `tmux rename-session`). A missing
    /// local session is a no-op (no entry is fabricated under `new`); a missing
    /// tmux session reports an error that is intentionally ignored. Only the
    /// named session is touched, never any other worktree's.
    pub fn rename(&mut self, old: &str, new: &str) {
        let Some(session) = self.sessions.remove(old) else {
            return;
        };
        self.sessions.insert(new.to_string(), session);
        let _ = std::process::Command::new("tmux")
            .args(rename_session_argv(old, new))
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }

    /// Activity state for the session named `name`: `None` when no such session
    /// exists, otherwise classified from its output cadence. Cheap — just reads
    /// an `Instant` under a lock.
    pub fn activity(&self, name: &str) -> ActivityState {
        activity_from_idle(self.sessions.get(name).map(Session::idle_for))
    }

    /// Snapshot of `(session_name, idle_duration)` for every live session, fed
    /// to [`AttentionTracker::poll`]. Cheap: one lock read per session.
    pub fn idle_durations(&self) -> Vec<(String, Duration)> {
        self.sessions
            .iter()
            .map(|(name, s)| (name.clone(), s.idle_for()))
            .collect()
    }

    pub fn resize_all(&self, rows: u16, cols: u16) {
        for s in self.sessions.values() {
            let _ = s.resize(rows, cols);
        }
    }

    /// Spawns a session from an arbitrary command and registers it under `name`.
    /// Test-only: lets UI tests build an active session without depending on
    /// `tmux` being installed on the host.
    #[cfg(test)]
    pub(crate) fn insert_spawned(
        &mut self,
        name: &str,
        cmd: CommandBuilder,
        cwd: &Path,
        rows: u16,
        cols: u16,
    ) -> anyhow::Result<()> {
        let session = Session::spawn_with_command(cmd, cwd, rows, cols)?;
        self.sessions.insert(name.to_string(), session);
        Ok(())
    }
}

impl Default for SessionManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pty_pipeline_renders_command_output() {
        let mut cmd = CommandBuilder::new("printf");
        cmd.args(["wtcc-pty-ok"]);
        let session = Session::spawn_with_command(cmd, &std::env::temp_dir(), 24, 80).unwrap();
        for _ in 0..40 {
            if session
                .parser()
                .lock()
                .unwrap()
                .screen()
                .contents()
                .contains("wtcc-pty-ok")
            {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        assert!(
            session
                .parser()
                .lock()
                .unwrap()
                .screen()
                .contents()
                .contains("wtcc-pty-ok")
        );
    }

    // --- issue #106: mouse-wheel scrollback ---------------------------------
    //
    // On a screen smaller than the output, the latest lines are visible at the
    // live bottom (offset 0). Scrolling up reveals earlier lines the screen had
    // scrolled off; snapping to bottom returns to the live latest line.
    #[test]
    fn scroll_up_reveals_history_and_bottom_snaps_back() {
        let mut cmd = CommandBuilder::new("sh");
        cmd.args(["-c", "seq 1 60"]);
        let session = Session::spawn_with_command(cmd, &std::env::temp_dir(), 5, 20).unwrap();

        // Wait until the last line is on the live screen.
        for _ in 0..40 {
            if session
                .parser()
                .lock()
                .unwrap()
                .screen()
                .contents()
                .contains("60")
            {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }

        let visible = |s: &Session| s.parser().lock().unwrap().screen().contents();
        let offset = |s: &Session| s.parser().lock().unwrap().screen().scrollback();

        assert_eq!(offset(&session), 0, "starts at the live bottom");
        assert!(visible(&session).contains("60"), "latest line is visible");

        session.scroll_up(10);
        assert!(offset(&session) > 0, "scrolling up leaves the live bottom");
        assert!(
            visible(&session).contains("50"),
            "an earlier line that was scrolled off is now visible"
        );
        assert!(
            !visible(&session).contains("60"),
            "the latest line is no longer on screen after scrolling up"
        );

        session.scroll_to_bottom();
        assert_eq!(offset(&session), 0, "snaps back to the live bottom");
        assert!(
            visible(&session).contains("60"),
            "latest line is visible again"
        );
    }

    // --- issue #122: scrollback bounds probe --------------------------------
    //
    // `scrollback_bounds` is pure over a vt100 screen, so it is unit-testable by
    // feeding a hand-built parser more lines than its screen holds — no PTY.
    #[test]
    fn scrollback_bounds_reports_max_and_restores_offset() {
        let mut parser = Parser::new(24, 80, SCROLLBACK_LINES);
        // 50 lines into a 24-row screen leaves ~26 lines in scrollback.
        for i in 0..50 {
            parser.process(format!("line {i}\r\n").as_bytes());
        }
        let (cur, max) = scrollback_bounds(parser.screen_mut());
        assert_eq!(cur, 0, "starts at the live bottom");
        assert!(max > 0, "history exists once output exceeds the screen");
        assert_eq!(
            parser.screen().scrollback(),
            0,
            "the probe restores the original offset"
        );

        // From a non-zero offset the probe still reports the same max and restores.
        parser.screen_mut().set_scrollback(5);
        let (cur, max2) = scrollback_bounds(parser.screen_mut());
        assert_eq!(cur, 5, "reads the current offset");
        assert_eq!(max2, max, "max is independent of the starting offset");
        assert_eq!(
            parser.screen().scrollback(),
            5,
            "the probe restores the non-zero offset it found"
        );
    }

    // --- issue #123: in-scroll-mode incremental search ----------------------
    //
    // The search primitives are pure over a vt100 screen / plain data, so they
    // are unit-testable against a hand-fed `Parser` with no PTY.

    #[test]
    fn scrollback_lines_snapshots_every_line_in_order_and_restores_offset() {
        let mut parser = Parser::new(5, 20, SCROLLBACK_LINES);
        for i in 0..40 {
            parser.process(format!("line {i}\r\n").as_bytes());
        }
        assert_eq!(parser.screen().scrollback(), 0, "starts at the live bottom");

        let lines = scrollback_lines(parser.screen_mut());
        assert_eq!(
            parser.screen().scrollback(),
            0,
            "the snapshot restores the original offset"
        );
        let content: Vec<String> = lines
            .iter()
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
            .collect();
        let expected: Vec<String> = (0..40).map(|i| format!("line {i}")).collect();
        assert_eq!(content, expected, "every line is captured oldest-first");

        // From a non-zero offset the snapshot still restores where it started.
        parser.screen_mut().set_scrollback(3);
        let _ = scrollback_lines(parser.screen_mut());
        assert_eq!(
            parser.screen().scrollback(),
            3,
            "the snapshot restores a non-zero starting offset"
        );
    }

    #[test]
    fn scrollback_lines_with_no_history_is_the_visible_screen() {
        // Fewer lines than the 5-row screen holds => max == 0 => exactly `rows`.
        let mut parser = Parser::new(5, 20, SCROLLBACK_LINES);
        parser.process(b"only line\r\n");
        let lines = scrollback_lines(parser.screen_mut());
        assert_eq!(lines.len(), 5, "max == 0 yields exactly the visible rows");
        assert!(lines.iter().any(|l| l.trim() == "only line"));
    }

    #[test]
    fn find_matches_is_smart_case_and_empty_query_matches_nothing() {
        let lines = vec![
            "Hello World".to_string(),
            "hello there".to_string(),
            "HELLO".to_string(),
            "goodbye".to_string(),
        ];
        // All-lowercase query => case-insensitive: matches every "hello" variant.
        assert_eq!(find_matches(&lines, "hello"), vec![0, 1, 2]);
        // A query with uppercase => case-sensitive.
        assert_eq!(find_matches(&lines, "Hello"), vec![0]);
        // Empty query matches nothing; a miss returns empty.
        assert!(find_matches(&lines, "").is_empty());
        assert!(find_matches(&lines, "zzz").is_empty());
    }

    #[test]
    fn match_columns_finds_smart_case_non_overlapping_occurrences() {
        assert_eq!(match_columns("abcabc", "bc"), vec![1, 4]);
        // Lowercase query => case-insensitive; every column preserved.
        assert_eq!(match_columns("aAaA", "a"), vec![0, 1, 2, 3]);
        // Uppercase in query => case-sensitive.
        assert_eq!(match_columns("aAaA", "A"), vec![1, 3]);
        assert!(match_columns("abc", "").is_empty());
    }

    #[test]
    fn offset_for_line_brings_a_line_to_the_top_with_saturating_clamp() {
        // total=40, rows=5: the oldest line needs the max offset.
        assert_eq!(offset_for_line(0, 40, 5), 35);
        // A line already at the live-bottom window maps to offset 0.
        assert_eq!(offset_for_line(35, 40, 5), 0);
        // A line past the bottom saturates to 0.
        assert_eq!(offset_for_line(39, 40, 5), 0);
        // Fewer lines than the screen holds => 0.
        assert_eq!(offset_for_line(0, 3, 5), 0);
    }

    #[test]
    fn activity_from_idle_classifies_thresholds() {
        assert_eq!(activity_from_idle(None), ActivityState::None);
        assert_eq!(
            activity_from_idle(Some(Duration::from_millis(0))),
            ActivityState::Working
        );
        assert_eq!(
            activity_from_idle(Some(WORKING_WINDOW - Duration::from_millis(1))),
            ActivityState::Working
        );
        // Boundary: exactly the window is no longer "Working".
        assert_eq!(
            activity_from_idle(Some(WORKING_WINDOW)),
            ActivityState::Idle
        );
        assert_eq!(
            activity_from_idle(Some(Duration::from_secs(10))),
            ActivityState::Idle
        );
    }

    #[test]
    fn idle_for_is_small_after_output_then_grows_when_quiet() {
        let mut cmd = CommandBuilder::new("printf");
        cmd.args(["wtcc-activity"]);
        let session = Session::spawn_with_command(cmd, &std::env::temp_dir(), 24, 80).unwrap();

        // Wait until the reader thread has pumped the output through.
        for _ in 0..40 {
            if session
                .parser()
                .lock()
                .unwrap()
                .screen()
                .contents()
                .contains("wtcc-activity")
            {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }

        let after_output = session.idle_for();
        assert!(
            after_output < WORKING_WINDOW,
            "expected Working-range idle right after output, got {after_output:?}"
        );

        std::thread::sleep(WORKING_WINDOW + Duration::from_millis(50));
        assert!(
            session.idle_for() > after_output,
            "idle should grow once the PTY goes quiet"
        );
        assert!(session.idle_for() >= WORKING_WINDOW);
    }

    #[test]
    fn kill_removes_named_session_and_leaves_others() {
        let mut mgr = SessionManager::new();
        let mut a = CommandBuilder::new("printf");
        a.args(["a"]);
        let mut b = CommandBuilder::new("printf");
        b.args(["b"]);
        mgr.insert_spawned("wtcc-a", a, &std::env::temp_dir(), 24, 80)
            .unwrap();
        mgr.insert_spawned("wtcc-b", b, &std::env::temp_dir(), 24, 80)
            .unwrap();

        mgr.kill("wtcc-a");

        assert!(mgr.get("wtcc-a").is_none(), "killed session must be gone");
        assert!(
            mgr.get("wtcc-b").is_some(),
            "other worktree's session must survive"
        );
    }

    #[test]
    fn kill_unknown_session_is_noop() {
        let mut mgr = SessionManager::new();
        // No local session and tmux may report no session — must not panic.
        mgr.kill("wtcc-does-not-exist");
        assert!(mgr.get("wtcc-does-not-exist").is_none());
    }

    #[test]
    fn session_name_is_slug_prefixed() {
        assert_eq!(
            SessionManager::session_name("Feature/Foo Bar"),
            "wtcc-feature-foo-bar"
        );
    }

    // --- issue #51: re-key a session WITHOUT killing the live agent ----------
    //
    // TDD RED: a branch rename must re-key the agent's tmux session in place
    // (`tmux rename-session`) so the live agent stays attached under the new
    // `wtcc-<slug>` key — never killed and respawned. The in-memory map re-key is
    // the unit-tested part; the tmux spawn stays thin and is exercised via
    // `rename_session_argv` in the integration suite.

    #[test]
    fn rename_rekeys_session_preserving_entry_and_leaving_others() {
        let mut mgr = SessionManager::new();
        let mut a = CommandBuilder::new("printf");
        a.args(["a"]);
        let mut b = CommandBuilder::new("printf");
        b.args(["b"]);
        mgr.insert_spawned("wtcc-old", a, &std::env::temp_dir(), 24, 80)
            .unwrap();
        mgr.insert_spawned("wtcc-other", b, &std::env::temp_dir(), 24, 80)
            .unwrap();

        mgr.rename("wtcc-old", "wtcc-new");

        assert!(mgr.get("wtcc-old").is_none(), "old key must be removed");
        assert!(
            mgr.get("wtcc-new").is_some(),
            "the live session entry must be preserved under the new key, not killed"
        );
        assert!(
            mgr.get("wtcc-other").is_some(),
            "renaming one session must leave every other session intact"
        );
    }

    #[test]
    fn rename_unknown_session_is_noop() {
        let mut mgr = SessionManager::new();
        // No local session and tmux may report no session — must not panic and
        // must not fabricate an entry under the new key.
        mgr.rename("wtcc-missing", "wtcc-new");
        assert!(mgr.get("wtcc-missing").is_none());
        assert!(mgr.get("wtcc-new").is_none());
    }

    // --- issue #47: attention edge detection --------------------------------
    //
    // TDD RED: these pin the pure, time-injected `AttentionTracker` contract.
    // `Phase`/`phase` are private to this module, so the tests live in-module.

    fn snap(entries: &[(&str, Duration)]) -> Vec<(String, Duration)> {
        entries
            .iter()
            .map(|(n, d)| ((*n).to_string(), *d))
            .collect()
    }

    #[test]
    fn phase_classifies_at_the_quiet_boundary() {
        assert_eq!(phase(Duration::ZERO), Phase::Busy);
        assert_eq!(
            phase(ATTENTION_QUIET - Duration::from_millis(1)),
            Phase::Busy
        );
        // Boundary: exactly the threshold counts as quiet.
        assert_eq!(phase(ATTENTION_QUIET), Phase::Quiet);
        // A poisoned-lock degradation surfaces as Duration::MAX -> quiet.
        assert_eq!(phase(Duration::MAX), Phase::Quiet);
    }

    #[test]
    fn poll_fires_once_on_busy_to_quiet_edge() {
        let mut t = AttentionTracker::default();
        // First sight Busy: establishes phase, no edge.
        assert!(
            t.poll(&snap(&[("wtcc-a", Duration::ZERO)]), None)
                .is_empty()
        );
        // Busy -> Quiet edge: fires exactly once.
        assert_eq!(
            t.poll(&snap(&[("wtcc-a", ATTENTION_QUIET)]), None),
            vec!["wtcc-a".to_string()]
        );
        assert!(t.needs("wtcc-a"));
        assert_eq!(t.count(), 1);
    }

    #[test]
    fn poll_does_not_fire_on_first_sight_quiet() {
        // No prior Busy phase -> unknown->Quiet is not an edge.
        let mut t = AttentionTracker::default();
        assert!(
            t.poll(&snap(&[("wtcc-a", ATTENTION_QUIET)]), None)
                .is_empty()
        );
        assert!(!t.needs("wtcc-a"));
    }

    #[test]
    fn poll_does_not_fire_on_quiet_to_quiet() {
        let mut t = AttentionTracker::default();
        t.poll(&snap(&[("wtcc-a", Duration::ZERO)]), None);
        t.poll(&snap(&[("wtcc-a", ATTENTION_QUIET)]), None);
        assert!(
            t.poll(&snap(&[("wtcc-a", ATTENTION_QUIET)]), None)
                .is_empty()
        );
    }

    #[test]
    fn poll_does_not_fire_on_busy_to_busy() {
        let mut t = AttentionTracker::default();
        t.poll(&snap(&[("wtcc-a", Duration::ZERO)]), None);
        assert!(
            t.poll(&snap(&[("wtcc-a", Duration::from_millis(1))]), None)
                .is_empty()
        );
    }

    #[test]
    fn poll_does_not_fire_on_quiet_to_busy() {
        let mut t = AttentionTracker::default();
        t.poll(&snap(&[("wtcc-a", Duration::ZERO)]), None);
        t.poll(&snap(&[("wtcc-a", ATTENTION_QUIET)]), None);
        // Going Busy again must not fire.
        assert!(
            t.poll(&snap(&[("wtcc-a", Duration::ZERO)]), None)
                .is_empty()
        );
    }

    #[test]
    fn poll_refires_only_after_going_busy_again() {
        let mut t = AttentionTracker::default();
        t.poll(&snap(&[("wtcc-a", Duration::ZERO)]), None);
        assert_eq!(
            t.poll(&snap(&[("wtcc-a", ATTENTION_QUIET)]), None),
            vec!["wtcc-a".to_string()]
        );
        // Staying quiet does not refire.
        assert!(
            t.poll(&snap(&[("wtcc-a", ATTENTION_QUIET)]), None)
                .is_empty()
        );
        // Back to Busy, then Quiet again: fires a second time.
        t.poll(&snap(&[("wtcc-a", Duration::ZERO)]), None);
        assert_eq!(
            t.poll(&snap(&[("wtcc-a", ATTENTION_QUIET)]), None),
            vec!["wtcc-a".to_string()]
        );
    }

    #[test]
    fn poll_suppresses_the_active_session() {
        let mut t = AttentionTracker::default();
        t.poll(&snap(&[("wtcc-a", Duration::ZERO)]), Some("wtcc-a"));
        // Edge to quiet while active -> never fires, never flagged.
        assert!(
            t.poll(&snap(&[("wtcc-a", ATTENTION_QUIET)]), Some("wtcc-a"))
                .is_empty()
        );
        assert!(!t.needs("wtcc-a"));
        assert_eq!(t.count(), 0);
    }

    #[test]
    fn poll_clears_flag_when_session_becomes_active() {
        let mut t = AttentionTracker::default();
        t.poll(&snap(&[("wtcc-a", Duration::ZERO)]), None);
        t.poll(&snap(&[("wtcc-a", ATTENTION_QUIET)]), None);
        assert!(t.needs("wtcc-a"));
        // Selecting it (now active) clears its marker and decrements the count.
        t.poll(&snap(&[("wtcc-a", ATTENTION_QUIET)]), Some("wtcc-a"));
        assert!(!t.needs("wtcc-a"));
        assert_eq!(t.count(), 0);
    }

    #[test]
    fn poll_prunes_dead_session_names() {
        let mut t = AttentionTracker::default();
        t.poll(&snap(&[("wtcc-a", Duration::ZERO)]), None);
        t.poll(&snap(&[("wtcc-a", ATTENTION_QUIET)]), None);
        assert!(t.needs("wtcc-a"));
        // Absent from the snapshot -> pruned from both phases and the flag set.
        assert!(t.poll(&snap(&[]), None).is_empty());
        assert!(!t.needs("wtcc-a"));
        assert_eq!(t.count(), 0);
    }

    #[test]
    fn poll_with_duration_max_flags_at_most_once() {
        let mut t = AttentionTracker::default();
        t.poll(&snap(&[("wtcc-a", Duration::ZERO)]), None);
        assert_eq!(
            t.poll(&snap(&[("wtcc-a", Duration::MAX)]), None),
            vec!["wtcc-a".to_string()]
        );
        assert!(t.poll(&snap(&[("wtcc-a", Duration::MAX)]), None).is_empty());
    }

    #[test]
    fn idle_durations_lists_spawned_sessions() {
        let mut mgr = SessionManager::new();
        let mut a = CommandBuilder::new("printf");
        a.args(["x"]);
        mgr.insert_spawned("wtcc-x", a, &std::env::temp_dir(), 24, 80)
            .unwrap();
        let durations = mgr.idle_durations();
        assert!(
            durations.iter().any(|(n, _)| n == "wtcc-x"),
            "idle_durations must report each live session by name"
        );
    }

    // --- issue #48: named-session primitive for per-worktree tabs ------------
    //
    // TDD RED: tabs need MULTIPLE sessions per worktree, so `Session::spawn` is
    // generalized to `command: Option<&str>` (None => default shell) and
    // `SessionManager::ensure_named(name, cwd, command, rows, cols)` becomes the
    // named-session primitive that today's `ensure(branch, ...)` wraps. The real
    // spawn stays thin and tmux-dependent, so these stay CI-safe: the signature
    // is pinned with a fn-pointer (no spawn), and idempotency is checked only
    // when a spawn actually succeeded (tmux may be absent in CI).

    #[test]
    fn spawn_signature_takes_an_optional_command() {
        // None => `tmux new-session -A -s <name>` default shell; Some => an
        // argv-split command. Pinned at compile time with no side effects.
        let _spawn: fn(&str, Option<&str>, &Path, u16, u16) -> anyhow::Result<Session> =
            Session::spawn;
    }

    #[test]
    fn ensure_named_registers_under_name_and_is_idempotent() {
        let mut mgr = SessionManager::new();
        let name = "wtcc-issue48-ensure-named-t1";
        // tmux may be unavailable in CI: tolerate a spawn error, pin behavior
        // only when the spawn actually succeeded.
        if mgr
            .ensure_named(name, &std::env::temp_dir(), Some("printf hi"), 24, 80)
            .is_ok()
        {
            assert!(mgr.get(name).is_some(), "ensure_named registers under name");
            // A second call must reuse the live session, never respawn or error.
            assert!(
                mgr.ensure_named(name, &std::env::temp_dir(), Some("printf hi"), 24, 80)
                    .is_ok(),
                "ensure_named is idempotent for an existing name"
            );
            assert!(mgr.get(name).is_some());
            mgr.kill(name); // best-effort cleanup of the real tmux session
        }
    }

    #[test]
    fn ensure_still_maps_branch_to_the_slug_session_name() {
        // The branch-keyed wrapper keeps producing `wtcc-<slug>` so tab 0 (agent)
        // reattaches exactly as before.
        assert_eq!(
            SessionManager::session_name("Feature/Big Thing"),
            "wtcc-feature-big-thing"
        );
    }

    // --- issue #56: per-repo `run` command surface --------------------------
    //
    // TDD RED: a worktree's run command gets a DEDICATED, slug-prefixed session
    // name (`wtcc-run-<slug>`) distinct from the agent (`wtcc-<slug>`) and shell
    // (`wtcc-<slug>-t<n>`) surfaces, so kill-on-remove can target it precisely.
    // The user-authored command reaches tmux (which runs it via `$SHELL -c`) as a
    // SINGLE, un-interpolated trailing argv element — no `sh -c` wrapper of our own
    // (that would be word-split, dropping every word after the first). `run_argv`
    // is the pure seam that builds it — no slug/branch/path is concatenated in.

    #[test]
    fn run_session_name_is_slug_prefixed() {
        assert_eq!(run_session_name("Feature/Foo"), "wtcc-run-feature-foo");
        assert_eq!(run_session_name("main"), "wtcc-run-main");
    }

    #[test]
    fn run_argv_passes_the_command_as_a_single_un_interpolated_element() {
        // tmux runs the trailing arg via `$SHELL -c "<command>"`, so the shell —
        // not wtcc — parses it, and metacharacters survive. Two invariants pin the
        // word-dropping bug: (1) the multi-word command is ONE verbatim trailing
        // element, and (2) there is NO `sh`/`-c` wrapper — such a wrapper would be
        // joined by tmux into `sh -c <command>`, word-split by the outer shell, and
        // run only the first word (`pnpm dev` → `pnpm`).
        let name = "wtcc-run-demo";
        let cmd = "pnpm dev && echo done";
        let argv = run_argv(name, cmd, Path::new("/w/t"));
        assert_eq!(
            argv,
            vec![
                "new-session".to_string(),
                "-A".to_string(),
                "-s".to_string(),
                name.to_string(),
                "-c".to_string(),
                "/w/t".to_string(),
                cmd.to_string(),
            ]
        );
        assert_eq!(
            argv.last().map(String::as_str),
            Some(cmd),
            "the multi-word command is one un-split, un-interpolated trailing element"
        );
        // The only `-c` is the tmux start-dir flag (`-c <cwd>`); there is NO `sh -c`
        // wrapper of our own, which tmux would join and the outer shell word-split.
        assert!(
            !argv.iter().any(|a| a == "sh"),
            "no sh wrapper: tmux would join it and the outer shell would word-split"
        );
        assert_eq!(
            argv.iter().filter(|a| a.as_str() == "-c").count(),
            1,
            "the sole `-c` is the tmux start-dir flag, not a shell wrapper"
        );
    }

    // --- issue #80: agent surface drops into a shell instead of dying --------
    //
    // TDD RED: when the agent command exits (e.g. `/exit` in Claude Code), the
    // AGENT tab must NOT die as a dead `[exited]` pane. tmux runs the agent under
    // a fixed `sh -c` wrapper that execs an interactive shell in the same PTY/cwd
    // on exit, keeping the surface usable so `R` can relaunch. The agent tokens
    // stay DISCRETE trailing argv params (`$0`/`$@`) — never interpolated into the
    // script string — so there is no shell-metacharacter surface. `agent_argv` is
    // the pure seam; shell tabs (`command=None`) and run tabs are untouched.

    #[test]
    fn agent_argv_wraps_the_command_in_a_shell_fallback() {
        let argv = agent_argv("wtcc-main", "claude --flag", Path::new("/w/t"));
        assert_eq!(
            argv,
            vec![
                "new-session".to_string(),
                "-A".to_string(),
                "-s".to_string(),
                "wtcc-main".to_string(),
                "-c".to_string(),
                "/w/t".to_string(),
                "sh".to_string(),
                "-c".to_string(),
                AGENT_FALLBACK_SCRIPT.to_string(),
                "claude".to_string(),
                "--flag".to_string(),
            ]
        );
        // The agent tokens are the DISCRETE trailing elements after the script.
        assert_eq!(
            &argv[argv.len() - 2..],
            &["claude".to_string(), "--flag".to_string()],
        );
        // The script itself carries NO agent text — it reaches the agent only as
        // separate positional params, never string-interpolated.
        assert!(
            !AGENT_FALLBACK_SCRIPT.contains("claude"),
            "agent text must never be interpolated into the script literal"
        );
    }

    #[test]
    fn agent_argv_single_token_agent() {
        let argv = agent_argv("wtcc-x", "claude", Path::new("/w/t"));
        // The start dir is pinned right after the session name (#116).
        let name_pos = argv.iter().position(|a| a == "wtcc-x").unwrap();
        assert_eq!(
            &argv[name_pos + 1..name_pos + 3],
            &["-c".to_string(), "/w/t".to_string()],
            "`-c <cwd>` follows the session name"
        );
        // Exactly one trailing agent token after the fixed script.
        assert_eq!(argv.last().map(String::as_str), Some("claude"));
        let script_pos = argv
            .iter()
            .position(|a| a == AGENT_FALLBACK_SCRIPT)
            .unwrap();
        assert_eq!(
            &argv[script_pos + 1..],
            &["claude".to_string()],
            "single-token agent is one trailing param after the script"
        );
    }

    // --- issue #116: reattaching to a session whose cwd vanished ------------

    #[test]
    fn cwd_is_missing_flags_removed_dirs_including_the_deleted_marker() {
        // A present dir is fine; an empty query result is "nothing to reap".
        assert!(!cwd_is_missing(&std::env::temp_dir().to_string_lossy()));
        assert!(!cwd_is_missing(""));
        assert!(!cwd_is_missing("   "));
        // A path that EXISTS but resolves via a non-NotFound stat must NOT be
        // flagged: only `ErrorKind::NotFound` means the dir is gone. A live file
        // stats Ok(_) => present. (We can't portably force PermissionDenied here;
        // the point is that anything other than NotFound => not missing.)
        let live_file = std::env::temp_dir().join(format!("wtcc-cwd-live-{}", std::process::id()));
        std::fs::write(&live_file, b"x").unwrap();
        assert!(
            !cwd_is_missing(&live_file.to_string_lossy()),
            "an existing path (Ok stat) is present, never reaped"
        );
        let _ = std::fs::remove_file(&live_file);
        // A path that does not exist counts as missing — and so does tmux's
        // `<path> (deleted)` marker, which it emits for a removed pane cwd (it
        // readlinks `/proc/<pid>/cwd`). That marker never resolves to a real path.
        let gone = std::env::temp_dir()
            .join(format!("wtcc-cwd-missing-{}", std::process::id()))
            .to_string_lossy()
            .into_owned();
        assert!(cwd_is_missing(&gone));
        assert!(cwd_is_missing(&format!("{gone} (deleted)")));
    }

    // --- BLOCKER fix: bounded post-reap spawn retry against a dying tmux server.
    //
    // Reaping the tmux server's LAST session tears the server down asynchronously
    // (`exit-empty on`), so the immediate `new-session` can race it and fail. The
    // retry is gated on `reaped`: only a reap justifies a retry+backoff; a plain
    // spawn failure must surface at once. These pin that contract without tmux by
    // driving `spawn_after_reap` with plain closures.

    #[test]
    fn spawn_after_reap_surfaces_first_error_without_a_reap() {
        let mut calls = 0;
        let r: anyhow::Result<()> = spawn_after_reap(false, || {
            calls += 1;
            Err(anyhow::anyhow!("boom"))
        });
        assert!(r.is_err());
        assert_eq!(
            calls, 1,
            "no reap => exactly one attempt, no retry, no sleep"
        );
    }

    #[test]
    fn spawn_after_reap_does_not_retry_when_the_first_attempt_succeeds() {
        let mut calls = 0;
        let r: anyhow::Result<u32> = spawn_after_reap(true, || {
            calls += 1;
            Ok(calls)
        });
        assert_eq!(r.unwrap(), 1);
        assert_eq!(
            calls, 1,
            "happy path never sleeps or retries, even after a reap"
        );
    }

    #[test]
    fn spawn_after_reap_retries_after_a_reap_then_succeeds() {
        let mut calls = 0;
        let r: anyhow::Result<u32> = spawn_after_reap(true, || {
            calls += 1;
            if calls < 3 {
                Err(anyhow::anyhow!("server exited unexpectedly"))
            } else {
                Ok(calls)
            }
        });
        assert_eq!(r.unwrap(), 3);
        assert_eq!(calls, 3, "retries the racing spawn until it wins");
    }

    #[test]
    fn spawn_after_reap_gives_up_after_the_bounded_retry_budget() {
        let mut calls = 0;
        let r: anyhow::Result<()> = spawn_after_reap(true, || {
            calls += 1;
            Err(anyhow::anyhow!("still dying"))
        });
        assert!(r.is_err());
        assert_eq!(
            calls, 6,
            "one initial attempt + five bounded retries, then surfaces"
        );
    }

    // Real-tmux behavior test on a PRIVATE server (`-L <socket>`) so it can never
    // touch — or be perturbed by — the user's live default-server sessions. It
    // pins the two-part #116 premise the production code relies on:
    //   (1) `new-session -A` that REATTACHES ignores `-c <cwd>`, so pinning the
    //       start dir alone cannot rescue a pre-existing session with a dead cwd;
    //   (2) killing (reaping) that session first makes the recreate honor `-c`
    //       and land in a live cwd.
    // Best-effort: skips cleanly if tmux is unavailable.
    #[test]
    fn tmux_reattach_ignores_start_dir_until_the_dead_session_is_reaped() {
        let pid = std::process::id();
        let sock = format!("wtcc-it-{pid}");
        let dead = std::env::temp_dir().join(format!("wtcc-it-dead-{pid}"));
        let live = std::env::temp_dir().join(format!("wtcc-it-live-{pid}"));
        if std::fs::create_dir_all(&dead).is_err() || std::fs::create_dir_all(&live).is_err() {
            return;
        }
        let (dead_s, live_s) = (
            dead.to_string_lossy().into_owned(),
            live.to_string_lossy().into_owned(),
        );
        let name = "s";
        let tmux = |args: &[&str]| -> Option<std::process::Output> {
            std::process::Command::new("tmux")
                .arg("-L")
                .arg(&sock)
                .args(args)
                .output()
                .ok()
        };
        let pane = || -> String {
            tmux(&[
                "display-message",
                "-p",
                "-t",
                name,
                "-F",
                "#{pane_current_path}",
            ])
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_default()
        };
        let cleanup = || {
            let _ = tmux(&["kill-server"]);
            let _ = std::fs::remove_dir_all(&dead);
            let _ = std::fs::remove_dir_all(&live);
        };

        // A detached session whose cwd is `dead`, running `cat` so the pane stays
        // alive. If tmux is unavailable/failed, skip.
        match tmux(&["new-session", "-d", "-s", name, "-c", &dead_s, "cat"]) {
            Some(o) if o.status.success() => {}
            _ => {
                cleanup();
                return;
            }
        }
        // A second, keep-alive session in a live dir so reaping the poisoned
        // session below never empties the server. With `exit-empty on`, killing
        // the server's LAST session tears the server down asynchronously and the
        // immediate recreate races the dying server (`server exited unexpectedly`);
        // production guards that with `spawn_after_reap`'s bounded retry, and the
        // real app always has other worktree sessions alive. Keeping one here makes
        // this reap→recreate assertion deterministic without a race.
        let keep_s = std::env::temp_dir().to_string_lossy().into_owned();
        let _ = tmux(&["new-session", "-d", "-s", "keepalive", "-c", &keep_s, "cat"]);
        assert!(
            pane().starts_with(&dead_s),
            "the session starts in the dead dir"
        );

        // Remove the cwd out-of-band, then reattach with a fresh `-c <live>`.
        let _ = std::fs::remove_dir_all(&dead);
        let _ = tmux(&["new-session", "-A", "-d", "-s", name, "-c", &live_s, "cat"]);
        let after_reattach = pane();
        assert!(
            cwd_is_missing(&after_reattach),
            "reattach keeps the dead cwd — pinning `-c` alone cannot fix it, got {after_reattach:?}"
        );

        // Reap the poisoned session, then recreate: now `-c <live>` takes effect.
        let _ = tmux(&["kill-session", "-t", name]);
        let _ = tmux(&["new-session", "-d", "-s", name, "-c", &live_s, "cat"]);
        let after_reap = pane();
        let landed = std::path::Path::new(&after_reap).exists() && after_reap.starts_with(&live_s);

        cleanup();
        assert!(
            landed,
            "after reaping, the recreated session lands in the live cwd, got {after_reap:?}"
        );
    }

    #[test]
    fn agent_fallback_script_pins_the_exec_shell_contract() {
        // exec replaces the process (no lingering wrapper); ${SHELL:-/bin/sh}
        // gives the user's shell with a POSIX fallback; $0/$@ run the agent with
        // its real argv before the fallback fires.
        assert!(AGENT_FALLBACK_SCRIPT.contains("exec"));
        assert!(AGENT_FALLBACK_SCRIPT.contains("${SHELL:-/bin/sh}"));
        assert!(AGENT_FALLBACK_SCRIPT.contains("\"$0\""));
        assert!(AGENT_FALLBACK_SCRIPT.contains("\"$@\""));
    }
}
