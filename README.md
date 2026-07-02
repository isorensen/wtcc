# wtcc — WorkTree Command Center

A lightweight **terminal UI** for running [Claude Code](https://claude.com/claude-code) agents in
parallel, one per **git worktree**. A Linux reimagining of the macOS app
[Supacode](https://github.com/supabitapp/supacode) — written from scratch in Rust, runs inside any
terminal (built and tested on [ghostty](https://github.com/ghostty-org/ghostty) on Arch / Wayland).

> **Status:** early development. See the [design spec](docs/superpowers/specs/2026-06-22-wtcc-tui-design.md).

## Demo

A repository's worktrees in the sidebar — each with an activity marker (`◆` working, `◇` idle) and
git/PR status — beside the live agent terminal for the selected worktree:

```text
┌ repos ─────────────────────────┐┌ agent · feat/login ────────────────────────────────────┐
│▸ acme-api                      ││Claude Code  —  acme-api · feat/login                   │
│  ◇○ main                       ││                                                        │
│  ◆● feat/login                 ││> implement the login form validation                   │
│   ○ fix/payments               ││                                                        │
│                                ││  ✓ read src/auth/login.ts                              │
│                                ││  ✎ editing src/auth/validators.ts                      │
│                                ││  ✓ added 3 tests                                       │
│                                ││                                                        │
│                                ││Running tests…  12 passed                               │
│                                ││                                                        │
└────────────────────────────────┘└────────────────────────────────────────────────────────┘
j/k move  Tab agent  n/d worktree  a/D repo  R restart  r refresh  : palette  ? help  q quit
```

Press `?` for the full keybinding overlay. These frames are reproducible with
[`docs/demo/capture.sh`](docs/demo/capture.sh) (`cargo build --release` first); it seeds a throwaway
repo and a scripted stand-in agent, so it touches neither your config nor a real Claude session.

## Why a TUI?

Supacode embeds a Ghostty terminal surface per worktree, but that surface is macOS-only —
`libghostty`'s portable GPU surface for Linux is roadmapped but unscheduled, so the Ghostty widget
**cannot be embedded in a Linux GUI today**. Rather than reach for a GUI toolkit, `wtcc` *is* a
terminal app: the host terminal (ghostty) does the rendering, `wtcc` does the orchestration. The
result is lighter, runs in any terminal, and needs no GTK/Wayland-specific stack.

## What it does

- Register repositories and manage their **git worktrees** (create / switch / remove).
- Run a **persistent Claude Code agent per worktree** (via `tmux`, so agents survive restarts).
- See the **selected agent's terminal live** beside a worktree sidebar.
- See **git + PR/CI status per worktree** via the `gh` CLI.
- Fuzzy **command palette** and vim-friendly keybinds.

## Roadmap

`wtcc` is an early-stage, open-source reimagining of the macOS app
[Supacode](https://github.com/supabitapp/supacode) for the Linux terminal. The releases below
track the path toward feature parity, plus a few terminal-native ideas of our own. Work is
organized into milestones on the [issue tracker](https://github.com/isorensen/wtcc/milestones);
each line links to an issue once filed. Scope follows a strict *start-minimal, expand-on-real-pain*
rule — speculative features (split panes, in-PTY mouse scroll) are deliberately deferred until
there is real demand.

**Shipped (v0.5.0):** repository registration · git worktrees (create / switch / remove) · a
persistent Claude Code agent per worktree (tmux-backed, survives restarts) · live agent terminal ·
git + PR/CI status badges via `gh` · fuzzy command palette · vim keybinds · basic mouse
(click-to-select).

### v0.6.0 — Usability

Everyday feel, plus the headline multi-agent gap.

| Feature | Notes |
|---|---|
| Data-driven keymap | One source of truth for dispatch, the help overlay, statusbar hints and the palette |
| Theme & visual hierarchy | Centralized colors, a focused-pane border, clearer sidebar/statusbar |
| Wheel-scroll the sidebar | Mouse wheel moves the selection |
| Fix orphaned agent sessions | Removing a worktree now kills its `tmux` session instead of leaking it |
| **Agent attention routing** | Detect when an agent goes quiet (finished / waiting for input), mark it in the sidebar, count unread, add a jump key, and optionally fire a `notify-send` desktop notification |

### v0.7.0 — Worktree & agent ops

| Feature | Notes |
|---|---|
| Repo lifecycle scripts | `setup`-on-create and `archive`-before-remove hooks |
| GitHub PR write actions | Merge · mark ready · close · open in browser, via `gh` |
| Branch rename | Renames the branch and re-keys the agent's `tmux` session |
| Per-worktree agent presets | Pick Claude Code / Codex / opencode per worktree |
| Archive worktrees | Soft-hide from the sidebar instead of deleting |
| Per-repo base ref | Branch new worktrees from a configured ref |
| Copy-on-create files | Copy `.env`-style files into a new worktree on creation |

### v0.8.0 — Multi-surface terminal

Per-worktree **tabs** — multiple agent/shell surfaces in one worktree — and a run-dev-on-key
command. *Someday-maybe:* split panes within a tab, and mouse-wheel scroll into the agent PTY.

## Keybindings

**Sidebar focus**

| Key | Action |
|-----|--------|
| `j` / `k` (or arrows) | Move selection (freely across every expanded repo's worktrees) |
| `Space` / `Enter` | Expand / collapse the selected repo |
| `S` | Switch to the next repo (also expands it if collapsed) |
| `Tab` | Focus the agent pane |
| `a` | Register a repository |
| `D` | Unregister the selected repository (config only; nothing on disk is deleted) |
| `n` | Add a worktree |
| `d` | Remove the selected worktree |
| `x` | Archive / unarchive the selected worktree (soft hide; nothing deleted) |
| `X` | Show / hide archived worktrees |
| `b` | Rename the selected worktree's branch |
| `A` | Switch the worktree's agent (from configured presets) |
| `R` | Restart the selected worktree's agent (kills its tmux session; a fresh agent respawns) |
| `r` | Refresh worktrees |
| `t` / `w` | New shell tab / close the active shell tab |
| `]` / `[` | Next / previous tab |
| `s` | Run the repo's configured `run` command in a tab |
| `g` | Jump to the next agent needing attention |
| `o` | Open the worktree's PR in the browser |
| `m` | Merge the worktree's PR (confirm first) |
| `:` / `Ctrl-P` | Command palette |
| `?` | Help overlay (lists all keybindings) |
| `q` / `Ctrl-Q` | Quit |

**Agent focus** — keystrokes are forwarded to the running agent.

| Key | Action |
|-----|--------|
| `Ctrl-O` | Return focus to the sidebar |
| `Ctrl-Q` | Quit |
| `Ctrl-C` | Forwarded to the agent (does **not** quit) |

**Text selection** — drag with the mouse in the agent pane to select text; on
release the selection is copied to your system clipboard via OSC 52 (works over
SSH and in the alternate screen). The highlight clears on the next keypress.
Since wtcc captures the mouse, hold `Shift` while dragging to fall back to your
terminal's own native selection.

## Requirements

- Rust (2024 edition)
- `git`, `tmux`
- [`gh`](https://cli.github.com/) (GitHub CLI) — for PR/CI status
- [`claude`](https://claude.com/claude-code) (Claude Code CLI) — the agent

## Build & run

```sh
cargo run
```

## Development

```sh
cargo fmt --all
cargo clippy --all-targets -- -D warnings
cargo test
pre-commit install   # gitleaks + format hooks
```

## License

[MIT](LICENSE). Security policy: [SECURITY.md](SECURITY.md).
