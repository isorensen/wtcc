use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use portable_pty::{CommandBuilder, MasterPty, PtySize, native_pty_system};
use vt100::Parser;

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
    pub fn spawn(
        session_name: &str,
        agent_cmd: &str,
        cwd: &Path,
        rows: u16,
        cols: u16,
    ) -> anyhow::Result<Session> {
        let mut cmd = CommandBuilder::new("tmux");
        cmd.args(["new-session", "-A", "-s", session_name]);
        // agent_cmd is split on whitespace and passed as argv (NOT through a
        // shell — no shell-metacharacter interpretation).
        for token in agent_cmd.split_whitespace() {
            cmd.arg(token);
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
        let parser = Arc::new(Mutex::new(Parser::new(rows, cols, 0)));
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

    pub fn ensure(
        &mut self,
        branch: &str,
        cwd: &Path,
        agent_cmd: &str,
        rows: u16,
        cols: u16,
    ) -> anyhow::Result<&Session> {
        let name = Self::session_name(branch);
        if !self.sessions.contains_key(&name) {
            let s = Session::spawn(&name, agent_cmd, cwd, rows, cols)?;
            self.sessions.insert(name.clone(), s);
        }
        Ok(self.sessions.get(&name).unwrap())
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
}
