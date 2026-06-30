//! TDD RED for issue #44 — the Default-only `Theme` struct.
//!
//! Pins the documented baseline and the field invariants of `Theme::default()`.
//! Deliberately NOT a theming engine: no user config, no parser, no presets — a
//! plain `Copy` struct of named ratatui colors resolved once at startup.

use ratatui::style::Color;
use wtcc::theme::Theme;

#[test]
fn default_pr_severity_colors_match_documented_baseline() {
    let t = Theme::default();
    assert_eq!(t.pr_ok, Color::Green, "passing PR baseline is green");
    assert_eq!(t.pr_bad, Color::Red, "failing/closed PR baseline is red");
    assert_eq!(t.pr_pending, Color::Yellow, "pending PR baseline is yellow");
}

#[test]
fn default_pr_severity_colors_are_mutually_distinct() {
    let t = Theme::default();
    assert_ne!(t.pr_ok, t.pr_bad);
    assert_ne!(t.pr_ok, t.pr_pending);
    assert_ne!(t.pr_bad, t.pr_pending);
}

#[test]
fn default_focus_border_is_distinct_from_unfocused_border() {
    let t = Theme::default();
    assert_ne!(
        t.border_focus, t.border,
        "the focused pane border must be visually distinct from the unfocused one"
    );
}

#[test]
fn default_activity_states_are_distinctly_colored() {
    let t = Theme::default();
    assert_ne!(
        t.activity_working, t.activity_idle,
        "working vs idle activity glyphs must be distinguishable by color"
    );
}

#[test]
fn default_accent_is_a_real_color() {
    let t = Theme::default();
    assert_ne!(
        t.accent,
        Color::Reset,
        "accent must be a real color so repo headers and selection stand out"
    );
}

#[test]
fn default_attention_is_distinct_from_idle_and_hint() {
    let t = Theme::default();
    assert_ne!(t.attention, Color::Reset, "attention must be a real color");
    assert_ne!(
        t.attention, t.activity_idle,
        "the attention marker must not blend into the idle activity glyph"
    );
    assert_ne!(
        t.attention, t.hint,
        "attention must stand out against the dim hint color"
    );
}

#[test]
fn default_status_and_hint_are_distinct() {
    let t = Theme::default();
    assert_ne!(
        t.status, t.hint,
        "the status line must read differently from dim hints"
    );
}

#[test]
fn default_dirty_is_a_real_color() {
    let t = Theme::default();
    assert_ne!(t.dirty, Color::Reset, "dirty marker must be a real color");
}

#[test]
fn theme_is_copy() {
    // Compile-time proof the struct is `Copy` (resolved once, passed by value
    // into render with no borrow). If it were only `Clone`, the second bind
    // would move out of `t` and fail to compile.
    let t = Theme::default();
    let a = t;
    let b = t;
    let _ = (a, b);
}
