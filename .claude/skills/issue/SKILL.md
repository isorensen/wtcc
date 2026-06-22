---
name: issue
description: Handle GitHub issues end-to-end for the wtcc project. Use this skill whenever the user mentions an issue number, asks to fix a bug, wants to work on a GitHub issue, says "issue", "#17", or refers to any open issue. Also trigger when the user asks to check open issues, triage tasks, plan a milestone, seed the tracker from the design spec, or work on a reported problem. Supports "issue bootstrap" to seed labels/milestones/issues from the design spec, "issue new <description>" to create an issue, "issue list" to see open issues, and "issue milestone" to manage milestones. Ensures the full workflow runs: branch, TDD, fmt/clippy/test, version, changelog, code review, PR, CI-green merge, and release.
---

# /issue — GitHub Issue Handler (wtcc)

End-to-end workflow for resolving GitHub issues in **wtcc** — from reading the issue
to merging into `main` and cutting a release.

**Project:** wtcc — WorkTree Command Center (Rust TUI)
**Repo:** `isorensen/wtcc`
**CLI:** Always use `gh` — this is GitHub (never `glab`).
**Language:** Rust (2024). Gates: `cargo fmt`, `cargo clippy -D warnings`, `cargo test`.

Set `REPO=isorensen/wtcc` for the snippets below.

## Usage modes

- `/issue bootstrap` — **First-time setup**: create labels + milestones, and seed issues from the design spec (run once).
- `/issue <number>` — Work on an existing issue (full workflow below).
- `/issue new <description>` — Create a new issue and optionally start working on it.
- `/issue list` — List open issues, grouped by milestone.
- `/issue milestone` — Show milestone progress and what's next.

## Branching model (intentionally simple)

Small open-source repo → **trunk-based with PRs**. Branch off `main`, open a PR to `main`,
merge when CI is green. **No `develop` branch** — that dual-flow is overhead this project
doesn't need. Branch names:

- `feat/<desc>` — features
- `fix/<desc>` — bug fixes
- `refactor/<desc>` — refactoring
- `chore/<desc>` — maintenance/tooling
- `docs/<desc>` — docs only
- `test/<desc>` — test-only

`main` is protected: never commit or force-push directly; everything lands via PR.

## Bootstrap (`/issue bootstrap`)

Run **once** to seed the tracker. Confirm each list with the user before creating.

1. **Create labels** (type + priority):
   ```bash
   for l in "feature:0e8a16:New functionality" "bug:d73a4a:Something is broken" \
            "enhancement:a2eeef:Improve existing behavior" "task:c5def5:General task" \
            "refactor:fbca04:Internal restructuring" "chore:ededed:Tooling/maintenance" \
            "docs:0075ca:Documentation" "security:b60205:Security-relevant" \
            "good first issue:7057ff:Newcomer-friendly"; do
     name="${l%%:*}"; rest="${l#*:}"; color="${rest%%:*}"; desc="${rest#*:}"
     gh label create "$name" --color "$color" --description "$desc" --repo "$REPO" --force
   done
   ```

2. **Create milestones** reflecting the design spec's milestone list
   (`docs/superpowers/specs/2026-06-22-wtcc-tui-design.md`). Suggested seed:

   | Milestone | Scope |
   |-----------|-------|
   | `v0.1.0 — Scaffold & domain core` | Cargo project, config, repository, worktree domain (TDD) |
   | `v0.2.0 — TUI shell` | ratatui loop, sidebar, focus, panic-safe teardown |
   | `v0.3.0 — Agent pane` | PTY + tmux + vt100 rendering + input forwarding |
   | `v0.4.0 — Status & palette` | `gh` PR/CI badges, fuzzy command palette |
   | `v0.5.0 — Polish` | keybinds, theme, docs, manual test in ghostty/Wayland |

   ```bash
   gh api -X POST "repos/$REPO/milestones" \
     -f title="v0.1.0 — Scaffold & domain core" \
     -f description="Cargo project, config, repository, worktree domain (TDD)"
   ```

3. **Seed issues** from the spec's goals/risks. Each goal or risk becomes a focused issue under
   the right milestone, using the template below, assigned to `isorensen`.

**Gate after bootstrap:**
```bash
gh label list --repo "$REPO"
gh api "repos/$REPO/milestones?state=open" --jq '.[].title'
gh issue list --repo "$REPO"
```

## Versioning

SemVer, milestone-driven. Pre-1.0, so breaking changes bump the **minor** (0.x.0) and fixes bump
the **patch** (0.x.y). Each milestone closes with a minor bump in `Cargo.toml` + a `CHANGELOG.md`
entry + a git tag + a GitHub Release.

## Creating a new issue (`/issue new`)

1. **Determine type** (bug/feature/enhancement/task/refactor/chore/docs) and **milestone** (ask if unclear).
2. **Create it:**
   ```bash
   gh issue create --repo "$REPO" \
     --title "<title>" \
     --body "<body from template>" \
     --label "<type>" \
     --milestone "<milestone title>" \
     --assignee isorensen
   ```
3. **Show the URL.** Ask: "Quer começar a trabalhar nesta issue agora?" If yes → full workflow.

Keep titles concise (<70 chars). Issue body language: English (this is a public OSS repo).

### Issue body template

```markdown
## Context
<Why this issue exists — observed problem, scenario, motivation>

## Acceptance criteria
- [ ] <criterion 1>
- [ ] <criterion 2>

## Technical notes
<Affected modules/files, implementation considerations, known trade-offs>

## Tests
<Cases to cover: unit, integration, UI smoke>

## References
<Links to the design spec, related issues, ADRs>
```

## Working on an issue (`/issue <number>`)

### Phase 0 — Orient
1. Read `CLAUDE.md` (security + engineering standards) and the design spec if not already in context.
2. `git status` — ensure clean tree on `main` or a feature branch off `main`.

### Phase 1 — Understand
1. `gh issue view <number> --repo "$REPO"` — read body, comments, labels, milestone, linked PRs.
2. Read the code involved **before** changing it. Never modify code you haven't read.
3. Check sibling issues in the same milestone for dependencies:
   ```bash
   gh issue list --repo "$REPO" --milestone "<milestone>"
   ```

### Phase 2 — Branch
```bash
git checkout main && git pull origin main
git checkout -b <type>/<description>
```

### Phase 3 — Implement (TDD)

**Principle: simplest direct solution that satisfies the acceptance criteria + security rules.
No speculative abstractions, no premature optimization. If something worthwhile is out of scope,
file a new issue (see Scope discipline).**

4. **Write tests first** — cover the bug scenario or new behavior. Domain logic stays pure and
   unit-tested; integration tests use a real `git` repo via `tempfile`; UI gets buffer-snapshot
   smoke tests.
5. **Implement** — delegate code writing to `coder-fable` (fallback `coder-opus` if Fable is
   unavailable; `coder-sonnet` only for mechanical edits). The main conversation should not write
   non-trivial code inline — protects the context window and keeps review discipline.
6. **Run the gates:**
   ```bash
   cargo test --all
   cargo clippy --all-targets -- -D warnings
   cargo fmt --all --check
   ```

### Phase 4 — Version & docs
7. **Version bump:** usually none mid-milestone. On the milestone's last issue, bump `Cargo.toml`
   to the milestone version. Breaking pre-1.0 change → minor bump (consult user).
8. **CHANGELOG.md** — add an entry (`Keep a Changelog` sections: Added/Changed/Fixed/Removed/Security):
   ```markdown
   ## [0.1.0] - 2026-06-22
   ### Added
   - Description of change (#N)
   ```
9. **Docs** — update README/ADRs/spec if the change affects architecture or public behavior.
   Delegate non-trivial doc sync to `context-architect-documenter`.

### Phase 5 — Review (MANDATORY)
10. Re-run the gates after any doc/version edit.
11. **Code review** — for routine diffs use `/code-review` (diff-aware; `--fix` applies in working
    tree). For a large feature or when you want the "over-engineering = BLOCKER" lens isolated,
    use the `code-reviewer` subagent. **Gate:** proceed only at zero blockers; fix and re-review.
12. **Security check** — for anything touching subprocess spawning (`git`/`gh`/`claude`/`tmux`),
    path handling, or worktree/branch names, audit against `CLAUDE.md` "Safe subprocess execution".
    Argument vectors only — never `sh -c` with interpolated input. If in doubt, run `/security-review`.

### Phase 6 — Ship
13. **Ask the user before committing** — NEVER auto-commit (global rule).
14. **Commit:**
    ```
    <type>(<scope>): description

    Closes #N

    Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
    ```
    Conventional Commits: `feat`/`fix`/`refactor`/`chore`/`docs`/`test`. Scope = module touched
    (`worktree`, `session`, `ui`, `config`, `ci`, …).
15. **Push + open PR (target `main`):**
    ```bash
    git push -u origin <branch>
    gh pr create --repo "$REPO" --base main --head "<branch>" \
      --title "<title>" \
      --assignee isorensen \
      --body "$(cat <<'EOF'
## Summary
<bullets>

## Test plan
- [ ] cargo test passes
- [ ] cargo clippy clean (-D warnings)
- [ ] cargo fmt clean

Closes #N
EOF
)"
    ```
16. **Wait for CI green** — GitHub Actions runs automatically on the PR (no manual trigger needed).
    Poll with `ScheduleWakeup` rather than blocking the conversation:
    ```bash
    gh pr checks <pr-number> --repo "$REPO"            # watch until all pass
    gh run list --repo "$REPO" --branch "<branch>" -L 5
    ```
    On failure, read the log, fix the root cause (don't guess), re-run gates locally, push a new
    commit (not amend), and resume polling. **Stop and ask the user after 3 distinct failed-fix
    cycles** on the same PR (global rule).
17. **Merge when green:**
    ```bash
    gh pr merge <pr-number> --repo "$REPO" --squash --delete-branch
    ```
18. **Update the issue checklist (MANDATORY)** — mark all acceptance criteria `[x]`:
    ```bash
    gh issue view <N> --repo "$REPO" --json body --jq .body   # read, flip [ ] → [x]
    gh issue edit <N> --repo "$REPO" --body "<updated body>"
    ```
19. **Close the issue** (squash merge may not auto-close): `gh issue close <N> --repo "$REPO"`.
20. **Cleanup:** `git checkout main && git pull origin main && git branch -d <branch>`.

### Phase 7 — Release (milestone complete)
When every issue in a milestone is closed:
```bash
# 1. Confirm milestone is empty of open issues
gh issue list --repo "$REPO" --milestone "<title>" --state open
# 2. Bump Cargo.toml + finalize CHANGELOG date, commit via PR
# 3. Tag on main
git checkout main && git pull origin main
git tag -a v0.X.0 -m "Release v0.X.0 — <milestone>"
git push origin v0.X.0
# 4. Close the milestone
gh api -X PATCH "repos/$REPO/milestones/<number>" -f state=closed
# 5. GitHub Release from the CHANGELOG section
gh release create v0.X.0 --repo "$REPO" --title "v0.X.0 — <milestone>" \
  --notes "$(sed -n '/## \[0.X.0\]/,/## \[/p' CHANGELOG.md | sed '$d')"
```

## Listing issues (`/issue list`)
```bash
gh issue list --repo "$REPO" --assignee isorensen
gh issue list --repo "$REPO" --milestone "v0.1.0 — Scaffold & domain core"
```
Present grouped by milestone with simple progress bars, e.g.:
```
v0.1.0 — Scaffold & domain core  [###-------] 3/10
  #1  feat(config): XDG config load/save           (open)
  #3  test(worktree): parse `git worktree list`     (closed)
```

## Checklist summary
```
[ ] Read issue on GitHub (gh issue view)
[ ] Branch from main (feat/, fix/, …)
[ ] Write tests first (TDD)
[ ] Implement via coder-fable (coder-opus fallback; coder-sonnet for mechanical edits)
[ ] cargo test / clippy (-D warnings) / fmt — all clean
[ ] Version bump if milestone-closing; update CHANGELOG.md
[ ] Update README/ADR/spec if scope warrants
[ ] Code review (/code-review or code-reviewer) — zero blockers
[ ] Security check on subprocess/path/name handling
[ ] User confirms commit
[ ] Push + open PR (base: main)
[ ] CI green (gh pr checks) — poll via ScheduleWakeup; max 3 fix cycles
[ ] Merge squash + delete branch when green
[ ] Mark issue acceptance criteria [x]; close issue
[ ] Delete local branch
[ ] (Milestone complete) bump, tag, close milestone, GitHub Release
```

## Scope discipline
A related-but-out-of-scope concern → file a new issue under the right milestone rather than
expanding the current PR. Keep PRs focused and reviewable.

## Anti-patterns to avoid
- **Never use `glab`** — this is GitHub. Always `gh`.
- **Never commit/push to `main` directly** — everything via PR.
- **Never force-push to `main`.**
- **Never skip code review** or **merge a red PR**.
- **Never commit without user confirmation** (global rule).
- **Never bump a version without a CHANGELOG entry.**
- **Never skip updating issue checklists** — flip `[ ]`→`[x]` and verify after merge.
- **Never build a shell command string with interpolated repo paths / worktree names** — argument
  vectors only. This is the project's main security surface.
- **Never write non-trivial code inline in the main conversation** — delegate to coder agents.
