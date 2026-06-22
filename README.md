# wtcc — WorkTree Command Center

A lightweight **terminal UI** for running [Claude Code](https://claude.com/claude-code) agents in
parallel, one per **git worktree**. A Linux reimagining of the macOS app
[Supacode](https://github.com/supabitapp/supacode) — written from scratch in Rust, runs inside any
terminal (built and tested on [ghostty](https://github.com/ghostty-org/ghostty) on Arch / Wayland).

> **Status:** early development. See the [design spec](docs/superpowers/specs/2026-06-22-wtcc-tui-design.md).

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

## Keybindings

**Sidebar focus**

| Key | Action |
|-----|--------|
| `j` / `k` (or arrows) | Move selection |
| `Tab` | Focus the agent pane |
| `a` | Register a repository |
| `D` | Unregister the selected repository (config only; nothing on disk is deleted) |
| `n` | Add a worktree |
| `d` | Remove the selected worktree |
| `r` | Refresh worktrees |
| `:` / `Ctrl-P` | Command palette |
| `?` | Help overlay (lists all keybindings) |
| `q` / `Ctrl-Q` | Quit |

**Agent focus** — keystrokes are forwarded to the running agent.

| Key | Action |
|-----|--------|
| `Ctrl-O` | Return focus to the sidebar |
| `Ctrl-Q` | Quit |
| `Ctrl-C` | Forwarded to the agent (does **not** quit) |

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
