use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, Widget};

use crate::app::{App, Focus};
use crate::session::ActivityState;

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

    let mut items: Vec<ListItem> = Vec::new();

    for (ri, repo) in app.config.repos.iter().enumerate() {
        let is_current_repo = app.selected_repo == Some(ri);
        let glyph = if is_current_repo { "▸" } else { " " };
        items.push(ListItem::new(Line::from(vec![
            Span::raw(format!("{glyph} ")),
            Span::styled(
                repo.name.clone(),
                Style::default().add_modifier(Modifier::BOLD),
            ),
        ])));

        if is_current_repo {
            if app.worktrees.is_empty() {
                items.push(ListItem::new(Line::from("    (no worktrees)")));
            }
            for (wi, wt) in app.worktrees.iter().enumerate() {
                items.push(ListItem::new(worktree_line(app, focused, wi, wt)));
            }
        }
    }

    if app.config.repos.is_empty() {
        items.push(ListItem::new(Line::from("(no repos registered)")));
    }

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
