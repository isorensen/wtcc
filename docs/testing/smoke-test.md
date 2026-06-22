# Smoke test — wtcc

Manual + scripted end-to-end checks for the real target (running inside a terminal on
Arch/Wayland; built and exercised in [ghostty](https://github.com/ghostty-org/ghostty)).
Automated unit/integration tests cover the domain and UI buffers; this document covers the
"does the whole thing actually run" path that those can't.

## Automated launch check

`scripts/smoke-test.sh` drives the release binary in a detached `tmux` session, captures the
rendered frame, exercises the command palette, and quits — verifying startup, rendering, input,
and a clean terminal-restoring exit without any human interaction.

```sh
cargo build --release
scripts/smoke-test.sh
```

Requirements: `tmux` and `git` on PATH. It uses a throwaway `XDG_CONFIG_HOME` so it never touches
your real config. Exit code 0 = pass.

## Results — 2026-06-22 (issue #11)

Run on Arch Linux / Wayland. Release binary, `tmux` 120×40 pane.

**Phase 1 — empty config (no repos):** ✅
- Renders the `repos` sidebar (`(no repos registered)`), the `agent` pane placeholder, and the
  status-hint bar.
- `:` opens the command palette showing `Add repository` / `Quit`.
- `q` quits; the tmux session ends — clean, terminal-restoring exit.

**Phase 2 — register a real repo (`a` → path → Enter):** ✅ (full pipeline)
- The repo appears in the sidebar (`▸ <name>`) with its worktree (`● main`).
- Config is persisted to `$XDG_CONFIG_HOME/wtcc/config.toml` with the `[[repos]]` entry.
- The agent pane spawns `tmux new-session -A … claude` for the selected worktree and **renders
  live Claude Code output** through the `vt100` + `tui-term` pipeline (the embedded-PTY path works
  end-to-end, including nested tmux).

**Findings:** no rough edges in the automated run. Registration → worktree listing → agent spawn →
live PTY rendering all work. No follow-up issues filed.

## Manual checklist (interactive parts — run in ghostty)

These need a human because they exercise interactive agent I/O, persistence across restarts, and
resize — hard to assert reliably in a script.

- [ ] **Type into the agent.** Tab into the agent pane, type a prompt to Claude, confirm the caret
      is visible (issue #8) and the agent responds, rendered correctly in the pane.
- [ ] **Focus toggle.** `Ctrl-O` returns focus to the sidebar; `Ctrl-C` while focused goes to the
      agent (does not quit); `Ctrl-Q` quits from anywhere.
- [ ] **tmux persistence.** With an agent mid-task, quit wtcc (`Ctrl-Q`), relaunch it, reselect the
      worktree — the agent session reattaches with its state intact (`tmux new-session -A`).
- [ ] **Resize.** Resize the ghostty window; the agent pane resizes and reflows without corruption.
- [ ] **Multiple worktrees.** Create a worktree (`n`), switch between worktrees, confirm each keeps
      its own agent session.
- [ ] **Status badges (issue #6).** A dirty worktree shows its badge; a branch with an open PR shows
      PR/CI state (requires `gh` authenticated).

Record any rough edges as new issues.
