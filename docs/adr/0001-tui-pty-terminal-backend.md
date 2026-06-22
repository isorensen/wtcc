# 0001 — TUI rendering a PTY (vt100 + tui-term), not an embedded libghostty surface

- **Status:** Accepted
- **Date:** 2026-06-22
- **Context issue:** #9
- **Related:** design spec `docs/superpowers/specs/2026-06-22-wtcc-tui-design.md`

## Context

`wtcc` is a Linux reimagining of [Supacode](https://github.com/supabitapp/supacode), a macOS
"worktree coding-agents command center". Supacode is SwiftUI/AppKit and embeds a **Ghostty**
terminal surface per worktree via `libghostty`.

We needed to decide how to render each worktree's agent terminal on Linux (Arch / Wayland).
Three constraints shaped the decision:

1. **`libghostty` is not embeddable in a Linux GUI today.** Ghostty's embeddable terminal surface
   is a Darwin-only GPU surface; its portable GPU surface for Linux is roadmapped but unscheduled.
   The existing Linux take, [hyprcode](https://github.com/abjoru/hyprcode) (GTK4 + libadwaita +
   VTE), reached the same conclusion and swapped Ghostty for VTE behind a seam.
2. **The user runs the app inside [ghostty](https://github.com/ghostty-org/ghostty) on Wayland.**
   The host terminal already provides fast, GPU-accelerated rendering.
3. **Anti-over-engineering.** We want the lightest thing that ships a usable, tested MVP.

## Decision

Build `wtcc` as a **terminal UI (TUI)** using `ratatui` + `crossterm`, and render each worktree's
agent terminal **inside the TUI** by:

- spawning the agent in a PTY (`portable-pty`) wrapped in `tmux new-session -A` for persistence,
- feeding the PTY bytes into a `vt100::Parser`, and
- drawing that parser's screen with the `tui-term` `PseudoTerminal` widget in a ratatui pane.

The host terminal (ghostty) does the actual glyph rendering and GPU acceleration; `wtcc` only
emits text. We do **not** embed `libghostty`, and we do **not** pull in a GUI toolkit (GTK4/Tauri).

### Version pin (load-bearing)

`ratatui 0.29` is pinned together with `tui-term 0.2` and `vt100 0.15`. Newer `tui-term 0.3` /
`vt100 0.16` pull a `unicode-width` that conflicts with `ratatui 0.29`. Bumping any one of them in
isolation breaks the build, so Dependabot is configured to ignore minor/major bumps for
`ratatui`/`crossterm`/`tui-term`/`vt100`. The coordinated upgrade to `ratatui 0.30` is tracked in
issue #12.

> **Update (v0.2.0, #12):** the coordinated upgrade landed — `ratatui 0.30` / `crossterm 0.29` /
> `tui-term 0.3` / `vt100 0.16`. The `unicode-width` conflict is resolved, the `lru` advisory is
> cleared, and the Dependabot ignore block has been removed. The pin above is now historical.

## Consequences

**Positive**
- No GUI toolkit dependency; runs in any terminal, not only Wayland/Hyprland.
- Reuses the host terminal's GPU rendering for free — genuinely lighter than a GTK app.
- Honest use of ghostty: it is the *host*, not an embedded widget we cannot actually embed.
- Smaller surface to build, test, and maintain; the MVP shipped usable and tested.

**Negative / trade-offs**
- Rendering fidelity is bounded by `vt100` 0.15 + `tui-term` 0.2: character-cell sizing only
  (no pixel/sixel/image protocols), and weaker coverage of uncommon DEC private modes than newer
  vt100. Acceptable for Claude Code's TUI; revisit if an agent needs richer protocols.
- The ratatui-ecosystem version lock (above) defers some dependency updates until #12.

## Alternatives considered

- **Embed `libghostty` (as Supacode does).** Rejected: not embeddable in a Linux GUI today
  (Darwin-only surface; portable surface unscheduled).
- **GTK4 + libadwaita + VTE (as hyprcode does).** Rejected for this project: heavier stack tied to
  GTK/Wayland, and it does not leverage the host ghostty terminal. We deliberately took the lighter
  TUI route to differentiate and to honor the anti-over-engineering goal.
- **Tauri + xterm.js + PTY.** Rejected: bundles a webview, heaviest option, and does not use ghostty.

## Fallback

If `vt100`/`tui-term` fidelity proves inadequate for the agent's UI, the documented fallback is to
delegate the agent pane to a real `tmux` split that ghostty renders directly, rather than rendering
the PTY inside the ratatui pane. This was not necessary — the embedded-PTY spike succeeded.
