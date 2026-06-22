use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, Widget};

use crate::app::{App, Focus};
use crate::repository::Repository;
use crate::session::ActivityState;
use crate::worktree::Worktree;

/// One rendered row of the sidebar list, in render order. Both [`render`] and
/// [`crate::event::hit_test`] build this same sequence so the click→item mapping
/// can never drift from what is drawn. `RepoHeader`/`Worktree` carry the index a
/// click selects; the others are inert (clicking them is a no-op).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SidebarRow {
    RepoHeader(usize),
    NoWorktrees,
    Worktree(usize),
    EmptyHint,
}

/// The ordered list of sidebar rows for the current app state. This is the
/// single source of truth for row order; the renderer turns each into a
/// `ListItem` and the hit-test maps a click offset back to a row.
pub fn sidebar_rows(
    repos: &[Repository],
    worktrees: &[Worktree],
    selected_repo: Option<usize>,
) -> Vec<SidebarRow> {
    let mut rows = Vec::new();
    for ri in 0..repos.len() {
        rows.push(SidebarRow::RepoHeader(ri));
        if selected_repo == Some(ri) {
            if worktrees.is_empty() {
                rows.push(SidebarRow::NoWorktrees);
            }
            for wi in 0..worktrees.len() {
                rows.push(SidebarRow::Worktree(wi));
            }
        }
    }
    if repos.is_empty() {
        rows.push(SidebarRow::EmptyHint);
    }
    rows
}

pub fn render(app: &App, area: Rect, buf: &mut Buffer) {
    let focused = app.focus == Focus::Sidebar;
    let border_style = if focused {
        Style::default().add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    };

    let block = Block::default()
        .title(" repos ")
        .borders(Borders::ALL)
        .border_style(border_style);

    let rows = sidebar_rows(&app.config.repos, &app.worktrees, app.selected_repo);
    let items: Vec<ListItem> = rows
        .iter()
        .map(|row| match *row {
            SidebarRow::RepoHeader(ri) => {
                let glyph = if app.selected_repo == Some(ri) {
                    "▸"
                } else {
                    " "
                };
                ListItem::new(Line::from(vec![
                    Span::raw(format!("{glyph} ")),
                    Span::styled(
                        app.config.repos[ri].name.clone(),
                        Style::default().add_modifier(Modifier::BOLD),
                    ),
                ]))
            }
            SidebarRow::NoWorktrees => ListItem::new(Line::from("    (no worktrees)")),
            SidebarRow::Worktree(wi) => {
                ListItem::new(worktree_line(app, focused, wi, &app.worktrees[wi]))
            }
            SidebarRow::EmptyHint => ListItem::new(Line::from("  press a to register a repo")),
        })
        .collect();

    List::new(items).block(block).render(area, buf);
}

fn worktree_line<'a>(
    app: &App,
    focused: bool,
    index: usize,
    wt: &'a crate::worktree::Worktree,
) -> Line<'a> {
    let selected = app.selected_worktree == Some(index);
    let glyph = if selected { "●" } else { "○" };

    let label = if wt.is_bare {
        format!("{} [bare]", short_path(&wt.path))
    } else if wt.is_detached {
        format!("{} (detached)", short_path(&wt.path))
    } else {
        wt.branch.clone()
    };

    let mut style = Style::default();
    if selected && focused {
        style = style.add_modifier(Modifier::REVERSED);
    }

    let mut spans = vec![
        Span::raw("  "),
        activity_span(app.worktree_activity(&wt.branch)),
        Span::raw(format!("{glyph} ")),
        Span::styled(label, style),
    ];

    if let Some(badge) = app
        .vcs_status
        .get(&wt.path)
        .map(crate::vcs::status_badge)
        .filter(|b| !b.is_empty())
    {
        spans.push(Span::styled(
            format!(" {badge}"),
            Style::default().add_modifier(Modifier::DIM),
        ));
    }

    Line::from(spans)
}

/// A single-column glyph for the agent's activity, occupying a fixed width so
/// the selected/branch columns stay aligned regardless of state. `None` renders
/// a blank cell. Diamonds are used (not dots) so the activity marker is not
/// confused with the adjacent selection marker (`●`/`○`).
fn activity_span<'a>(state: ActivityState) -> Span<'a> {
    match state {
        ActivityState::Working => Span::styled("◆", Style::default().add_modifier(Modifier::BOLD)),
        ActivityState::Idle => Span::styled("◇", Style::default().add_modifier(Modifier::DIM)),
        ActivityState::None => Span::raw(" "),
    }
}

fn short_path(path: &std::path::Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}
