use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use portable_pty::{CommandBuilder, MasterPty, PtySize, native_pty_system};
use vt100::Parser;

/// A single agent terminal: a PTY running a `tmux new-session -A` attach child,
/// with a background reader thread feeding bytes into a vt100 parser.
pub struct Session {
    master: Box<dyn MasterPty + Send>,
    child: Box<dyn portable_pty::Child + Send + Sync>,
    reader: Option<JoinHandle<()>>,
    parser: Arc<Mutex<Parser>>,
    writer: Mutex<Box<dyn Write + Send>>,
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

        let parser_clone = Arc::clone(&parser);
        let handle = std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            loop {
                match reader_handle.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => parser_clone.lock().unwrap().process(&buf[..n]),
                }
            }
        });

        Ok(Session {
            master: pty.master,
            child,
            reader: Some(handle),
            parser,
            writer: Mutex::new(writer),
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
        self.parser.lock().unwrap().set_size(rows, cols);
        Ok(())
    }

    pub fn parser(&self) -> &Arc<Mutex<Parser>> {
        &self.parser
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
    fn session_name_is_slug_prefixed() {
        assert_eq!(
            SessionManager::session_name("Feature/Foo Bar"),
            "wtcc-feature-foo-bar"
        );
    }
}
