# Changelog

All notable changes to this project are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to
[Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added
- **Roadmap** section in the README documenting the v0.6.0–v0.8.0 release plan
  (feature parity with Supacode, organized by milestone).
- Agent **attention routing**: when a non-focused agent goes quiet (finished or
  waiting for input), its sidebar row shows a distinct marker and the statusbar
  shows an aggregated "N agents need input" count. Press `g` (or the palette
  "Jump to attention" command) to jump to the next agent needing attention.
  With `notify = true` in the config, a `notify-send` desktop notification fires
  per newly-flagged agent (missing `notify-send` degrades silently) (#47).
- Mouse **wheel scroll** over the sidebar moves the selection (up/down), regardless
  of which pane has focus — consistent with clicking a sidebar row. Scrolling over
  the agent pane is a no-op (#45).
- A single **`Theme`** (`src/theme.rs`, default-only) centralizes all UI colors:
  the focused pane now has a distinct colored border, and the sidebar/statusbar/PR
  badges have a clearer visual hierarchy. PR badge color reflects a derived
  severity (failing/closed → red, pending → yellow, ok → green, dirty → yellow) (#44).

### Changed
- Refactored the keymap into a single data-driven source of truth (`src/keymap.rs`):
  key dispatch, the help overlay, the statusbar hints, and the command palette are
  all derived from one binding table, so they can no longer drift. Adds a collision
  test and a completeness-by-construction help overlay. No user-visible behavior
  change (#43).

### Fixed
- Removing a worktree now kills its agent's `tmux` session (`wtcc-<slug>`) instead
  of leaking a detached session. The kill is best-effort and only on the explicit
  remove path — the restart/reattach persistence behavior is unchanged (#46).

## [0.5.0] - 2026-06-22

### Added
- CLI flags handled before the TUI starts: `--version`/`-V` and `--help`/`-h`;
  an unknown flag errors to stderr and exits non-zero. First-run empty state now
  guides the user to press `a` to register a repository (#37).
- Basic mouse support: left-click a repo or worktree row to select it, click the
  agent pane to focus it. Renderer and hit-test share one row ordering so clicks
  can't drift from what's drawn (#38).

### Fixed
- The help overlay (`?`) is sized to its content, so the Agent section no longer
  clips on short (≤24-row) terminals (#37).

## [0.4.0] - 2026-06-22

### Added
- Restart a worktree's agent from the TUI: `R` (and the "Restart agent" palette
  command), behind a confirm overlay — kills the tmux session and respawns a
  fresh agent. Useful when an agent hangs (#32).
- `n` (add worktree) now accepts an **existing** branch: if the typed branch
  already exists (local or remote) it is checked out into a new worktree;
  otherwise a new branch is created. One input, auto-detected (#33).

## [0.3.0] - 2026-06-22

### Added
- Remove/unregister a repository from the TUI: the `D` keybind and the "Remove
  repository" palette command, behind a confirm overlay. Config-only — never
  deletes anything on disk (#23).
- Per-worktree agent activity indicator in the sidebar: a diamond glyph (working
  `◆`, idle `◇`, none blank) derived from PTY output cadence, kept distinct from
  the selection marker (#24).
- Help overlay: `?` opens a centered list of all keybindings grouped by focus;
  the content is single-sourced next to the keymap to avoid drift (#25).
- README demo: a reproducible ASCII screenshot of the command center plus
  `docs/demo/capture.sh` to regenerate it (#26).

## [0.2.0] - 2026-06-22

### Added
- In-app repository registration: the `a` keybind and the "Add repository" palette
  command register a repo from inside the TUI, with `~`/relative path expansion,
  canonicalized duplicate detection, and persistence (#7).
- Per-worktree git + PR/CI status badges in the sidebar, fetched off the UI thread
  via `git` and `gh` (failures degrade to no badge, never a user error) (#6).
- A visible cursor in the agent pane when it is focused (honoring the program's own
  DECTCEM hidden state) (#8).
- ADR 0001 documenting the TUI/PTY (`vt100` + `tui-term`) terminal-backend decision (#9).
- Smoke test: `scripts/smoke-test.sh` (hands-free launch/render/quit check) plus a
  manual checklist in `docs/testing/smoke-test.md` (#11).
- CodeQL code scanning for Rust in CI (#10).

### Changed
- Migrated the terminal stack as a coordinated set: `ratatui` 0.29 → 0.30,
  `crossterm` 0.28 → 0.29 (the version `ratatui` 0.30 re-exports), `tui-term` 0.2 → 0.3,
  and `vt100` 0.15 → 0.16. These move together to resolve the `unicode-width` version
  conflict that previously blocked the bump, and the upgrade clears the transitive `lru`
  advisory (GHSA-rhfx-m35p-ff5j) by pulling `lru` 0.18 via `ratatui-core`. The only
  source-level break was `vt100`'s `set_size` moving from `Parser` to `Screen` (#12).

## [0.1.0] - 2026-06-22

### Added
- Project scaffolding: design spec, MIT license, README, `SECURITY.md`.
- Security baseline tailored to a Rust CLI: `gitleaks` pre-commit, secret `.gitignore`
  patterns, subprocess-safety + `cargo-audit` rules in `CLAUDE.md`.
- GitHub Actions CI: `fmt` + `clippy` + `test` + `gitleaks` + `cargo-audit`.
- Domain core (TDD): `config` (XDG TOML), `repository` (git-repo registration),
  `worktree` (porcelain parsing + `add`/`list`/`remove` via argv-only `git` calls, `slugify`).
- Panic-safe TUI shell: ratatui event loop, repo/worktree sidebar, command palette
  (`nucleo-matcher`), focus management, panic hook that always restores the terminal.
- Agent pane: per-worktree PTY running `tmux new-session -A` with the agent command,
  rendered live via `vt100` + `tui-term`; input forwarding in agent focus; tmux-backed
  persistence (sessions survive app exit and reattach).
- Project-local `/issue` skill (GitHub variant) for issue/milestone/PR workflow.

[Unreleased]: https://github.com/isorensen/wtcc/compare/v0.5.0...HEAD
[0.5.0]: https://github.com/isorensen/wtcc/compare/v0.4.0...v0.5.0
[0.4.0]: https://github.com/isorensen/wtcc/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/isorensen/wtcc/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/isorensen/wtcc/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/isorensen/wtcc/releases/tag/v0.1.0
