# Changelog

All notable changes to this project are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to
[Semantic Versioning](https://semver.org/).

## [Unreleased]

### Fixed
- **Agent pane no longer hangs on a session whose directory was removed out-of-band**: if a
  worktree's directory was deleted while wtcc had a tmux session there, `tmux new-session -A`
  reattached to the dead session and the shell hung on `getcwd` (only the first keystroke
  echoed). wtcc now pins each session's start dir (`tmux -c <worktree>`) and, before
  reattaching, detects a session whose working directory is gone and recreates it cleanly.
  A genuinely-missing worktree dir now shows the missing-dir hint instead of spawning a
  broken shell. (#116)

## [0.9.0] - 2026-07-02

### Added
- **Install from crates.io**: `cargo install wtcc`. A tag-triggered `crates-publish` job in
  the release workflow runs `cargo publish --locked` after tests pass (gated on a
  `CARGO_REGISTRY_TOKEN` secret; the tag/version match is verified first). (#79)
- **Register a plain (non-git) directory as an agent target**: pointing wtcc at a
  directory without a `.git` entry no longer errors — it registers as a plain repo with a
  single synthetic worktree (the directory itself), so you can run a Claude agent there
  (e.g. a Drive-synced or orchestration-only folder). Git-only actions (add/remove worktree,
  rename branch, PR open/ready/merge/close) are cleanly disabled with a `not a git
  repository` status, and no `git`/`gh` is spawned for it. Existing git repos and legacy
  configs are unaffected (the `kind` field defaults to git and is omitted when git). (#102)
- **Switch repos from the keyboard**: `S` cycles to the next repo (expanding it if it
  was collapsed), so collapsed repos are reachable without the command palette or mouse.
  It complements free arrow navigation and mirrors `A` (switch agent). (#108)
- **Scroll the agent pane's terminal history**: the agent PTY now keeps a bounded
  scrollback buffer and the mouse wheel scrolls back through it over the agent pane;
  typing snaps the view back to the live bottom. (#106)
- **Select and copy text in the agent pane**: drag inside the agent pane to select text;
  releasing copies it to the system clipboard via OSC 52 (works over SSH, no clipboard
  daemon needed). The highlight clears on the next keypress. `Shift+drag` still falls back
  to the terminal's native selection. (#103)

### Changed
- **All repos start expanded on launch**: every registered repo now shows its
  worktrees/branches immediately, instead of only the first repo being expanded. The
  first repo (index 0) is still the selected/active one; runtime expand/collapse is
  unchanged. (#107)
- **Arrow keys navigate freely across repos**: `j`/`k` / Up/Down now move continuously
  through the worktrees of *all expanded repos* in sidebar order; crossing a repo
  boundary makes that repo active (the selection↔repo invariant is preserved).
  Archived-hidden worktrees are still skipped. (#108)
- **Focus jumps to the agent pane after registering a repo**: submitting the "add repo"
  prompt (`a` → path → Enter) now moves focus to the agent pane on success, so the next
  keystrokes reach the agent instead of firing sidebar shortcuts (`d`, `a`, `x`). A failed
  registration (bad/empty path, duplicate, save error) keeps focus on the sidebar. (#104)

### Fixed
- **Shift+Tab is now forwarded to the agent pane**: the agent PTY receives the standard
  back-tab sequence (`ESC [ Z`) so Claude Code's mode toggle works. Handled whether the
  terminal reports Shift+Tab as `BackTab` (legacy) or `Tab` + SHIFT (Kitty keyboard
  protocol, e.g. ghostty); plain Tab still sends `\t`. (#105)

## [0.8.7] - 2026-07-02

### Changed
- **Attention notifications now name the repo**: the "agent needs your input" desktop
  notification reads `Agent for <repo> / <branch> needs your input` instead of just the
  branch, so with several repos expanded a same-named branch (e.g. two `main`s) is no
  longer ambiguous. Display-only — the internal composite/hash session key is never shown (#99).

## [0.8.6] - 2026-07-01

### Fixed
- **Background PR/dirty-status refresh no longer takes `.git/index.lock`**: the
  per-worktree badge refresh ran `git status --porcelain` without
  `GIT_OPTIONAL_LOCKS=0`, so its index refresh briefly grabbed the worktree's
  `index.lock` and could race — and fail — a concurrent `git` command (the user's
  own, or another wtcc operation). The status check now runs read-only with
  optional locks disabled. (Also removes an intermittent CI test flake.)

## [0.8.5] - 2026-07-01

### Fixed
- **Agent/tmux sessions no longer collide across repos**: session names, per-worktree
  tab layouts, and agent presets were keyed by branch name alone, so two repos
  expanded at once (0.8.2) with a same-named branch (e.g. both `main`) shared one
  live agent session and one tab layout — restarting or removing one could reach into
  the other. Identity is now a composite `<repo>-<branch>-<hash>` key (the hash is a
  stable digest of the repo path, so even same-named or hyphen-named repos like
  `advfit` and `advfit-ui` never collide). (#89)

### Notes
- **Migration:** on upgrade, a running agent's tmux session (`wtcc-<branch>`) is
  orphaned once — reattach spawns a fresh session under the new key; the old one can
  be cleaned up with `tmux kill-session`. Any per-worktree agent presets set before
  this release (keyed by bare branch name) stop matching and fall back to the default
  agent; re-select them with `Shift+A` if needed. Most setups have none.

### Added
- **Agent surface falls back to a shell when the agent exits**: when the agent
  command in the agent tab exits (e.g. `/exit` in Claude Code), the pane now drops
  into an interactive shell in the worktree directory (your `$SHELL`, falling back
  to `/bin/sh`) instead of dying as a dead `[exited]` pane — so the surface stays
  usable and `Shift+R` can relaunch a fresh agent. tmux runs the agent under a
  fixed `sh -c` wrapper; the agent command's tokens stay discrete argv params (no
  shell interpolation). Shell and run tabs are unchanged (#80).

### Fixed
- **Uppercase keybindings (`Shift+D`/`R`/`A`/`X`) now work under the Kitty keyboard
  protocol** (e.g. ghostty). They were dead because the terminal reports
  `Shift+<letter>` as the uppercase char *with* a redundant `SHIFT` modifier, which
  the exact modifier match rejected — so removing a repo, restarting the agent,
  switching agent preset, and toggling archived were all unreachable by key (the
  command palette still worked). Chord matching now ignores `SHIFT` for character
  keys (the shift is already encoded in the character); `Ctrl` bindings and
  non-character keys are unaffected (#90).

### Added
- **Expand multiple repositories at once**: each repo header in the sidebar can be
  toggled open/closed independently (click the header, or press `Space`/`Enter`),
  so several repos can show their worktrees at the same time instead of only the
  single selected one. The header glyph reflects expansion (`▾` open / `▸`
  collapsed) while color marks the selected repo; `j`/`k` still navigate within the
  active repo. PR actions and jump-to-attention resolve worktrees by path (not by
  branch name), so same-named branches across repos never target the wrong repo (#82).

## [0.8.1] - 2026-07-01

### Fixed
- **Deleted repository directory no longer blanks the panel with a cryptic error**:
  when a registered repo's root directory is deleted from disk, wtcc now shows an
  actionable hint (`repository '<name>' directory missing — press Shift+D to remove
  it`) instead of a double-prefixed `git worktree list failed` message, and no
  longer shells out to `git` in a directory that is gone. A worktree whose own
  directory was deleted is marked `[missing]` in the sidebar and can be cleared with
  `d` (with a `git worktree prune` fallback). The missing-directory state is computed
  once per refresh, so the render path never stats the filesystem (#81).

## [0.8.0] - 2026-06-30

### Added
- **Run dev command on key** (`s`): set a per-repo `run` command (e.g. `pnpm dev`,
  `cargo test`) and press `s` to launch it in a dedicated run tab
  (`wtcc-run-<slug>`) in the worktree directory. No `run` configured → a status
  message, no tab. The command is handed to `tmux`/`$SHELL -c` as a single
  un-interpolated element, so multi-word commands and shell operators work (#56).
- **Per-worktree tabs**: a worktree can now host multiple terminal surfaces — the
  agent (tab 0) plus extra **shell tabs**. `t` opens a new shell tab, `w` closes the
  active shell tab (the agent tab is protected), `]` / `[` cycle tabs. A tab strip
  shows the titles with the active one highlighted, and each worktree remembers its
  own tabs. Closing a tab or removing a worktree kills the tab's `tmux` session, so
  nothing leaks. (Tabs are in-memory; shell tabs don't survive a wtcc restart — the
  agent tab does, as before.) (#48)

## [0.7.0] - 2026-06-30

### Added
- **Copy-on-create files**: set `copy_on_create` (relative paths like `.env`) on a
  repo and those files are copied from the repo root into each new worktree on
  creation. Paths are validated against traversal (no absolute/`..`/symlink escape),
  existing files are never clobbered (atomic create), and missing sources are
  skipped (#55).
- **Per-repo base ref** for new worktrees: set `base_ref` on a repo in the config
  and new-branch worktrees start from that ref (`git worktree add -b <branch>
  <path> <base_ref>`) instead of `HEAD`. Unset → unchanged behavior (#54).
- **Archive worktrees** (`x` to archive/unarchive, `X` to show/hide archived):
  soft-hides a worktree from the sidebar without touching git or disk. Archived
  rows are dimmed when shown and skipped by `j`/`k` navigation when hidden, so
  selection never lands on an invisible row. The archived set persists per repo (#53).
- **Per-worktree agent presets** (`A`): define named agents in the config
  (`[[agents]]` with `name`/`cmd`) and pick one per worktree; the choice persists
  and the agent restarts with the new command. Falls back to the existing
  `agent_cmd` when no presets are defined, so old configs keep working. The picker
  validates the typed name against the configured presets (which it lists), so a
  typo can't silently select the wrong agent (#52).
- **Rename a worktree's branch** (`b`): renames the branch via `git branch -m` and
  re-keys the agent's `tmux` session in place (`tmux rename-session`) so the running
  agent stays attached. The worktree directory does not move, so PR/CI and attention
  state are preserved. Rejects empty names, collisions, and detached/bare worktrees (#51).
- **GitHub PR write actions** via `gh`: open the PR in the browser (`o`), mark a
  draft ready, merge (`m`), and close — all from the palette, with merge/close
  behind a confirm overlay. Merge strategy is per-repo (`merge_strategy`, default
  `squash`). All `gh` calls are argv-only; PR creation and diff view are out of
  scope (#50).
- Per-repo **lifecycle scripts**: an optional `setup` command runs once in a new
  worktree on creation, and an optional `archive` command runs in the worktree
  before it is removed (bounded by a timeout so a hanging script can't block
  removal). Both are user-authored shell commands run via `sh -c` with the worktree
  as the working directory — the command string is never interpolated (#49).

## [0.6.0] - 2026-06-30

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

[Unreleased]: https://github.com/isorensen/wtcc/compare/v0.8.0...HEAD
[0.8.0]: https://github.com/isorensen/wtcc/compare/v0.7.0...v0.8.0
[0.7.0]: https://github.com/isorensen/wtcc/compare/v0.6.0...v0.7.0
[0.6.0]: https://github.com/isorensen/wtcc/compare/v0.5.0...v0.6.0
[0.5.0]: https://github.com/isorensen/wtcc/compare/v0.4.0...v0.5.0
[0.4.0]: https://github.com/isorensen/wtcc/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/isorensen/wtcc/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/isorensen/wtcc/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/isorensen/wtcc/releases/tag/v0.1.0
