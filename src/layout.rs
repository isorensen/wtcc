//! The pure, per-worktree TAB model (issue #48).
//!
//! Each worktree owns an ordered list of terminal SURFACES (tabs) with an active
//! index. Tab 0 is always the AGENT (`wtcc-<slug>`, the reattach session that
//! predates tabs); additional tabs are SHELL surfaces (`wtcc-<slug>-t<n>`, with a
//! monotonic `n` so a reopened tab never collides with a live session). SPLIT
//! PANES ARE OUT OF SCOPE — this is tabs only.
//!
//! This module is intentionally pure: it mutates an in-memory model and hands the
//! caller the session names to spawn/kill. No tmux/PTY here. KNOWN GAP: the tab
//! list is in-memory and not persisted, so shell tabs are lost on wtcc restart
//! (the agent tab persists via its named tmux session as before). `close_active`
//! returning the killed name plus the kill-on-remove orphan sweep prevent leaks.

use crate::worktree::slugify;

/// Whether a tab hosts the worktree's agent (tab 0) or a plain shell surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TabKind {
    Agent,
    Shell,
}

/// A single terminal surface within a worktree: its kind, the named tmux/PTY
/// session that backs it, and the title shown in the tab strip.
#[derive(Debug, Clone)]
pub struct Tab {
    pub kind: TabKind,
    pub session: String,
    pub title: String,
}

/// One worktree's ordered tabs plus the active index and a monotonic id counter
/// for naming shell sessions.
#[derive(Debug, Clone)]
pub struct WorktreeLayout {
    pub tabs: Vec<Tab>,
    pub active: usize,
    pub next_id: usize,
}

impl WorktreeLayout {
    /// Seeds a layout with exactly one Agent tab bound to `wtcc-<slug>`. `slug` is
    /// slugified defensively so the session name is always shell/path-safe.
    pub fn new(slug: &str) -> Self {
        let slug = slugify(slug);
        let agent = Tab {
            kind: TabKind::Agent,
            session: format!("wtcc-{slug}"),
            title: "agent".to_string(),
        };
        Self {
            tabs: vec![agent],
            active: 0,
            next_id: 1,
        }
    }

    /// Appends a shell surface (`wtcc-<slug>-t<n>`), focuses it, and bumps the
    /// monotonic id. `slug` is slugified for the same safety reason as `new`.
    pub fn add_shell_tab(&mut self, slug: &str) {
        let slug = slugify(slug);
        let n = self.next_id;
        self.tabs.push(Tab {
            kind: TabKind::Shell,
            session: format!("wtcc-{slug}-t{n}"),
            title: format!("shell {n}"),
        });
        self.active = self.tabs.len() - 1;
        self.next_id += 1;
    }

    /// The currently focused tab. The model always keeps at least the agent tab,
    /// so this never panics.
    pub fn active_tab(&self) -> &Tab {
        &self.tabs[self.active]
    }

    /// Cycles focus to the next tab, wrapping at the end.
    pub fn next_tab(&mut self) {
        if !self.tabs.is_empty() {
            self.active = (self.active + 1) % self.tabs.len();
        }
    }

    /// Cycles focus to the previous tab, wrapping at the start.
    pub fn prev_tab(&mut self) {
        if !self.tabs.is_empty() {
            self.active = (self.active + self.tabs.len() - 1) % self.tabs.len();
        }
    }

    /// Closes the active tab if it is a closable shell tab, returning the removed
    /// session name for the caller to kill. Refuses (returns `None`, mutating
    /// nothing) for the agent tab (index 0) and for the only remaining tab.
    pub fn close_active(&mut self) -> Option<String> {
        if self.active == 0 || self.tabs.len() <= 1 {
            return None;
        }
        let removed = self.tabs.remove(self.active);
        if self.active >= self.tabs.len() {
            self.active = self.tabs.len() - 1;
        }
        Some(removed.session)
    }
}
