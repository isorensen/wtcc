# Changelog

All notable changes to this project are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to
[Semantic Versioning](https://semver.org/).

## [Unreleased]

### Changed
- Migrated the terminal stack as a coordinated set: `ratatui` 0.29 → 0.30,
  `crossterm` 0.28 → 0.29 (the version `ratatui` 0.30 re-exports), `tui-term` 0.2 → 0.3,
  and `vt100` 0.15 → 0.16. These move together to resolve the `unicode-width` version
  conflict that previously blocked the bump, and the upgrade clears the transitive `lru`
  advisory (GHSA-rhfx-m35p-ff5j) by pulling `lru` 0.18 via `ratatui-core`.
  The only source-level break was `vt100`'s `set_size` moving from `Parser` to `Screen`
  (now called via `screen_mut().set_size(..)`).

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

[Unreleased]: https://github.com/isorensen/wtcc/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/isorensen/wtcc/releases/tag/v0.1.0
