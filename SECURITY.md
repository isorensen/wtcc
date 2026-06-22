# Security Policy

## Reporting a vulnerability

Please report security issues **privately**. Do not open a public issue for a
suspected vulnerability.

- Use GitHub's [private vulnerability reporting](https://docs.github.com/en/code-security/security-advisories/guidance-on-reporting-and-writing-information-about-vulnerabilities/privately-reporting-a-security-vulnerability)
  on this repository (Security tab → "Report a vulnerability"), or
- email the maintainer at the address on the GitHub profile.

You can expect an acknowledgement within a few days.

## Scope

`wtcc` is a local terminal application. It stores no secrets and makes no network
calls of its own — authentication is delegated to the user's `gh` and `claude`
CLIs. The most relevant risk classes are:

- **Subprocess safety** — `wtcc` shells out to `git`, `gh`, `claude`, and `tmux`.
  Reports about command injection via crafted repository paths, branch names, or
  worktree names are especially welcome.
- **Secret leakage** — accidental logging or persistence of tokens handled by the
  delegated CLIs.

## Supported versions

This project is pre-1.0; only the latest release on `main` is supported.
