# wtcc — WorkTree Command Center (design)

**Date:** 2026-06-22
**Status:** Approved (pending written-spec review)
**License:** MIT
**Author:** isorensen (eduardo@isorensen.com)

## Summary

`wtcc` is a terminal UI (TUI) "worktree coding-agents command center": a single-screen
control panel for running **Claude Code** agents in parallel, one per **git worktree**.
It is a Linux reimagining of the macOS app [Supacode](https://github.com/supabitapp/supacode),
written from scratch in Rust. It runs **inside any terminal** (developed and tested on
[ghostty](https://github.com/ghostty-org/ghostty) on Arch Linux / Wayland), using the host
terminal for GPU-accelerated rendering rather than embedding a terminal widget.

## Background & motivation

Supacode (Swift + The Composable Architecture + **libghostty**, macOS 26+) embeds a Ghostty
terminal surface per worktree. That surface is **Darwin-only**: libghostty's portable GPU
surface for Linux is roadmapped but unscheduled, so the Ghostty terminal widget **cannot be
embedded in a Linux GUI today**. The existing Linux take, [hyprcode](https://github.com/abjoru/hyprcode)
(Rust + GTK4 + libadwaita + VTE), confirms this and swaps in VTE behind a seam. hyprcode is
currently a non-usable scaffold.

`wtcc` takes a different, lighter route: instead of embedding a terminal into a GUI window, it
**is** a terminal app. The host terminal (ghostty) provides rendering; `wtcc` provides the
orchestration. This avoids the GTK4/libadwaita stack entirely, runs anywhere (not only Wayland/
Hyprland), and aims to ship a genuinely usable, tested MVP.

### Fork vs. clone

Neither a fork nor a literal clone. Supacode is Swift/macOS — there is nothing to fork into a
Rust/Linux terminal app. `wtcc` is an **independent reimplementation** that uses Supacode for
product reference and hyprcode for Linux-specific lessons (persistence model, `gh` integration).
The name is deliberately distinct: "supacode" is the original author's brand and must not be reused.

## Goals (MVP v0.1)

- Register one or more **repositories** (persisted config).
- **List / create / remove git worktrees** for a registered repo.
- Run a **persistent Claude Code agent per worktree**.
- Render the **selected agent's terminal live** as a pane beside the sidebar.
- Show **git + PR/CI status per worktree** via the `gh` CLI.
- **Sidebar** navigation, a fuzzy **command palette**, and vim-friendly keybinds.
- Restore the host terminal cleanly on exit/panic.

## Non-goals (explicitly out — YAGNI)

- No GUI (no GTK4, no Tauri, no webview).
- No agents other than `claude` — but the launch command is configurable.
- No multi-theme system (one sensible default palette).
- No mouse support beyond basic click-to-focus.
- No macOS/Windows support.
- No plugin system, no remote/multi-machine orchestration.

These are added only when there is real, observed pain — not speculatively.

## Architecture

Pattern: **TEA-style** unidirectional flow — `model → update → view`. Domain logic is isolated
from UI in pure, testable modules behind clear interfaces.

### Module boundaries

**Domain (pure / I/O via CLI, no UI dependency — unit-testable):**

| Module | Responsibility | Depends on |
|--------|----------------|-----------|
| `worktree` | git worktree ops via the `git` CLI: list / add / remove, output parsing | `git` CLI |
| `repository` | registered repositories; load/save config | `config` |
| `vcs` (a.k.a. `gh`) | per-worktree branch + PR/CI status via `gh --json`, parsing | `gh` CLI |
| `session` | spawn the agent in a PTY inside tmux; pipe bytes into the vt100 parser | `portable-pty`, `tmux` |
| `config` | `~/.config/wtcc/config.toml`: repos, keybinds, agent command, palette | `serde`/`toml` |

**UI (ratatui + crossterm):**

| Module | Responsibility |
|--------|----------------|
| `ui/sidebar` | worktree list with status glyphs (dirty/clean, agent live, PR/CI) |
| `ui/agent_pane` | embedded terminal widget rendering the PTY screen |
| `ui/palette` | fuzzy command palette (create worktree, switch repo, etc.) |
| `ui/statusbar` | keybind hints, error/toast line |

**Shell:**

| Module | Responsibility |
|--------|----------------|
| `app` | application state (model), focus management, `update` dispatch |
| `event` | crossterm input → semantic events; key routing per focus |
| `main` | terminal raw-mode setup/teardown; **panic hook restores terminal**; event loop |

### Agent terminal rendering

The selected worktree's agent is shown by rendering its PTY screen inside the TUI:

1. `session` opens a PTY (`portable-pty`) running `tmux new-session -A -s <worktree-slug> <agent-cmd>`
   (default `<agent-cmd>` = `claude`).
2. A reader thread pumps PTY bytes into a `vt100::Parser`.
3. `ui/agent_pane` renders the parser screen via the `tui-term` widget, beside the sidebar.
4. When the agent pane has focus, keystrokes are forwarded to the PTY.

`tmux new-session -A` provides **persistence**: the agent survives `wtcc` exiting and reattaches
on relaunch. This reuses hyprcode's persistence model honestly.

### Data flow

```
input ─▶ event ─▶ update(model) ─▶ side-effects (spawn git / gh / pty)
                      ▲                        │
                      └──────── model ◀────────┘
                                  │
                                  ▼
                                view  (ratatui draw)
```

PTY reads run on a background thread → `mpsc` channel → main loop wakes → redraw. Concurrency is
**threads + channels**, not tokio, to stay light. Async is adopted only if a concrete need appears.

## Error handling

- `anyhow` at boundaries (`main`, command spawns); `thiserror` for domain error enums.
- Errors surface in the status bar / a transient toast — the TUI **never panics on expected errors**
  (missing `git`/`gh`/`claude`, bad repo path, tmux not installed are reported, not fatal).
- A panic hook always restores the terminal (leave raw mode, show cursor) before printing.
- Missing external tools (`git`, `gh`, `claude`, `tmux`) are detected at startup with a clear message.

## Testing strategy

- **TDD**, coverage target **≥80%**.
- Domain unit tests: worktree-output parsing, `gh --json` parsing, config round-trip, slug generation.
- Integration tests: real `git` worktree create/list/remove against a `tempfile` temp repo
  (mirrors hyprcode's `tests/`).
- UI smoke tests: snapshot the ratatui buffer for sidebar + empty agent pane.
- External CLIs (`gh`, `claude`, `tmux`) are abstracted behind a thin trait so tests can inject fakes.

## Dependencies (kept minimal)

`ratatui`, `crossterm`, `portable-pty`, `vt100` + `tui-term`, `serde` + `toml`, `anyhow`,
`thiserror`, a fuzzy matcher (`nucleo` or `fuzzy-matcher`). Dev: `tempfile`, optionally `insta`
for snapshots. Rust 2024 edition.

## Tooling, repo & security

- Public GitHub repo `isorensen/wtcc`, MIT license.
- `/init-security`: security-focused `CLAUDE.md`, `.gitignore` (secrets patterns), pre-commit
  secret scanning, GitHub Actions CI (`fmt` + `clippy` + `test` + secret scan).
- A project-local `/issue` skill (GitHub / `gh` variant, adapted from the cockpit GitLab skill)
  to organize issues, milestones, and labels for this repo.

## Milestones (high level)

1. **Scaffold** — Cargo project, repo, MIT, CI, security baseline, README.
2. **Domain core** — config, repository, worktree (TDD).
3. **TUI shell** — ratatui loop, sidebar, focus, panic-safe teardown.
4. **Agent pane** — PTY + tmux + vt100 rendering + input forwarding.
5. **Status** — `gh` PR/CI badges; command palette.
6. **Polish** — keybinds, theme, docs; manual test inside ghostty on Wayland.

## Open questions / risks

- `tui-term` + `vt100` rendering fidelity for a full-screen TUI agent (Claude Code's own UI) inside
  a ratatui pane — needs an early spike to confirm acceptable behavior (resize, colors, cursor).
  If fidelity is poor, fall back to delegating the agent pane to a real tmux split that ghostty
  renders directly (documented as an ADR decision at that point).
- tmux as a hard dependency — acceptable for MVP (hyprcode does the same); revisit if users object.
