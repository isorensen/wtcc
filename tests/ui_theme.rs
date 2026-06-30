//! TDD RED for issue #44 — the UI reads every color from `app.theme`, and the
//! focused pane gains a distinct colored border.
//!
//! These render into a `TestBackend`-style `Buffer` and inspect cell colors, so
//! they pin the *visible contract* (which color lands where) rather than any
//! internal call shape. Subprocess hygiene is untouched: no `git`/`gh`/`tmux`
//! is spawned — worktrees are injected directly and attention is driven through
//! the public tracker, so nothing untrusted reaches a shell.

use std::path::PathBuf;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;

use wtcc::app::{App, Focus};
use wtcc::config::Config;
use wtcc::repository::Repository;
use wtcc::session::{ATTENTION_QUIET, SessionManager};
use wtcc::vcs::{ChecksState, PrState, PrStatus, VcsStatus};
use wtcc::worktree::Worktree;

/// Mirrors `ui::SIDEBAR_WIDTH` (which is `pub(crate)`); the agent pane starts at
/// this column, so its top-left corner is the first agent border cell.
const SIDEBAR_WIDTH: u16 = 34;

const FEAT_PATH: &str = "/tmp/wtcc-issue44-nonexistent/feat";

fn app_one_worktree() -> App {
    // The repo path does not exist, so the constructor's worktree refresh is a
    // no-op; we then inject a worktree directly (no git involved).
    let mut app = App::new(Config {
        repos: vec![Repository {
            name: "demo-repo".to_string(),
            path: PathBuf::from("/tmp/wtcc-issue44-nonexistent"),
            setup: None,
            archive: None,
            archived: Vec::new(),
            base_ref: None,
            copy_on_create: Vec::new(),
            run: None,
        }],
        agent_cmd: "claude".to_string(),
        notify: true,
        merge_strategy: wtcc::pr::MergeStrategy::default(),
        ..Default::default()
    });
    app.selected_repo = Some(0);
    app.worktrees = vec![Worktree {
        path: PathBuf::from(FEAT_PATH),
        branch: "feat".to_string(),
        head: "abc123".to_string(),
        is_bare: false,
        is_detached: false,
    }];
    app.selected_worktree = Some(0);
    app.status = None;
    app
}

/// Foreground color of the first cell whose rendered symbol equals `symbol`.
fn find_fg(buf: &Buffer, symbol: &str) -> Option<Color> {
    buf.content()
        .iter()
        .find(|c| c.symbol() == symbol)
        .map(|c| c.fg)
}

#[test]
fn focused_sidebar_renders_border_focus_color() {
    let mut app = app_one_worktree();
    app.focus = Focus::Sidebar;

    let area = Rect::new(0, 0, 100, 24);
    let mut buf = Buffer::empty(area);
    wtcc::ui::render(&app, area, &mut buf);

    let sidebar_corner = buf.cell((0u16, 0u16)).expect("sidebar corner cell").fg;
    assert_eq!(
        sidebar_corner, app.theme.border_focus,
        "the focused sidebar must draw its border in theme.border_focus"
    );

    let agent_corner = buf
        .cell((SIDEBAR_WIDTH, 0u16))
        .expect("agent corner cell")
        .fg;
    assert_ne!(
        agent_corner, app.theme.border_focus,
        "the unfocused agent pane must not use the focus border color"
    );
}

#[test]
fn switching_focus_to_agent_moves_the_colored_border() {
    let mut app = app_one_worktree();
    app.focus = Focus::Agent;

    let area = Rect::new(0, 0, 100, 24);
    let mut buf = Buffer::empty(area);
    wtcc::ui::render(&app, area, &mut buf);

    let agent_corner = buf
        .cell((SIDEBAR_WIDTH, 0u16))
        .expect("agent corner cell")
        .fg;
    assert_eq!(
        agent_corner, app.theme.border_focus,
        "focusing the agent pane must move the focus border onto it"
    );

    let sidebar_corner = buf.cell((0u16, 0u16)).expect("sidebar corner cell").fg;
    assert_ne!(
        sidebar_corner, app.theme.border_focus,
        "the now-unfocused sidebar must drop the focus border color"
    );
}

#[test]
fn pr_badge_cell_uses_severity_color() {
    let mut app = app_one_worktree();
    // Open PR with passing checks -> Ok severity -> theme.pr_ok on the badge.
    app.vcs_status.insert(
        PathBuf::from(FEAT_PATH),
        VcsStatus {
            dirty: false,
            pr: Some(PrStatus {
                number: 42,
                state: PrState::Open,
                checks: ChecksState::Passing,
            }),
        },
    );

    let area = Rect::new(0, 0, 100, 24);
    let mut buf = Buffer::empty(area);
    wtcc::ui::render(&app, area, &mut buf);

    let check_fg = find_fg(&buf, "✓").expect("a passing PR badge should render the ✓ glyph");
    assert_eq!(
        check_fg, app.theme.pr_ok,
        "a passing PR badge must be colored with theme.pr_ok"
    );
}

#[test]
fn statusbar_status_line_uses_status_color() {
    let mut app = app_one_worktree();
    app.status = Some("something happened".to_string());

    let area = Rect::new(0, 0, 80, 1);
    let mut buf = Buffer::empty(area);
    wtcc::ui::statusbar::render(&app, area, &mut buf);

    let fg = find_fg(&buf, "s").expect("status text should render");
    assert_eq!(
        fg, app.theme.status,
        "the status line must be colored with theme.status"
    );
}

#[test]
fn statusbar_hints_use_hint_color() {
    let mut app = app_one_worktree();
    app.status = None;
    app.focus = Focus::Sidebar; // shows the sidebar hint string "j/k move ..."

    let area = Rect::new(0, 0, 100, 1);
    let mut buf = Buffer::empty(area);
    wtcc::ui::statusbar::render(&app, area, &mut buf);

    let fg = find_fg(&buf, "j").expect("sidebar hint text should render");
    assert_eq!(
        fg, app.theme.hint,
        "sidebar hints must be colored with theme.hint"
    );
}

#[test]
fn attention_marker_uses_attention_color() {
    let mut app = app_one_worktree();

    // Flag `feat` for attention through the public tracker (Busy -> Quiet edge),
    // no real PTY/session required.
    let name = SessionManager::session_name("feat");
    app.attention
        .poll(&[(name.clone(), std::time::Duration::ZERO)], None);
    app.attention.poll(&[(name, ATTENTION_QUIET)], None);
    assert!(
        app.attention_for("feat"),
        "precondition: feat must be flagged for attention"
    );

    let area = Rect::new(0, 0, SIDEBAR_WIDTH, 8);
    let mut buf = Buffer::empty(area);
    wtcc::ui::sidebar::render(&app, area, &mut buf);

    let fg = find_fg(&buf, "◈").expect("a flagged worktree should render the ◈ attention marker");
    assert_eq!(
        fg, app.theme.attention,
        "the attention marker must read theme.attention"
    );
}
