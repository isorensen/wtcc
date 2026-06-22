use std::collections::HashMap;
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

    /// Activity state for the session named `name`: `None` when no such session
    /// exists, otherwise classified from its output cadence. Cheap — just reads
    /// an `Instant` under a lock.
    pub fn activity(&self, name: &str) -> ActivityState {
        activity_from_idle(self.sessions.get(name).map(Session::idle_for))
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
    fn session_name_is_slug_prefixed() {
        assert_eq!(
            SessionManager::session_name("Feature/Foo Bar"),
            "wtcc-feature-foo-bar"
        );
    }
}
