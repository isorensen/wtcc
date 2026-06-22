# wtcc — Security & Engineering Standards

`wtcc` is an open-source **Rust TUI** that orchestrates Claude Code agents across git worktrees.
It has **no database, no web server, no cloud backend, and handles no personal data**, so most
infra/cloud/LGPD rules do not apply. The real security surface is three things: secret hygiene,
**safe subprocess execution**, and dependency auditing.

## Project facts
- Language: Rust (2024 edition). UI: `ratatui` + `crossterm`. Terminal: `portable-pty` + `vt100`/`tui-term`.
- Runs inside a host terminal (developed on ghostty / Arch / Wayland).
- Shells out to external CLIs: `git`, `gh`, `claude`, `tmux`. No network calls of its own.
- Design spec: `docs/superpowers/specs/2026-06-22-wtcc-tui-design.md`.

## Secrets (CRITICAL)
- **NEVER** hardcode tokens, API keys, or credentials. wtcc stores no secrets; auth is delegated
  to the user's own `gh` and `claude` CLIs.
- `.gitignore` covers `.env`, `*.pem`, `*.key`, `*secret*`. `gitleaks` runs in pre-commit and CI.
- If a secret is ever committed: **rotate it immediately** — removing it from history is not enough.

## Safe subprocess execution (CRITICAL for this app)
- wtcc spawns `git`, `gh`, `claude`, `tmux`.
- **ALWAYS** spawn with an explicit argument vector (`Command::new("git").args([...])`).
  **NEVER** build a shell string and run it via `sh -c` with interpolated user/repo input.
- Validate and canonicalize repository paths before use.
- Treat worktree names as untrusted: **slugify** before using them in branch names, tmux session
  names, or filesystem paths.

## Dependencies
- Run `cargo audit` (RustSec advisory DB) in CI. Keep `Cargo.lock` committed.
- Prefer `cargo-deny` to enforce allowed licenses + advisory checks.
- Keep the dependency surface minimal; review every new crate before adding it.

## Quality gates (CI — must pass before merge)
- `cargo fmt --check`
- `cargo clippy --all-targets -- -D warnings`
- `cargo test`
- `gitleaks` secret scan

## Engineering principles
- **Anti-over-engineering:** start minimal, expand on real pain. No speculative abstractions.
- TDD; coverage target ≥80%. Domain logic stays pure and unit-tested, isolated from UI.
- The TUI must never panic on expected errors (missing tools, bad paths) — surface them in the UI.
  A panic hook always restores the host terminal.

## Open-source hygiene
- MIT licensed. `SECURITY.md` describes how to report vulnerabilities privately.
- No telemetry, no analytics, no phone-home.
