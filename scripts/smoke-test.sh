#!/usr/bin/env bash
# Automated launch smoke test for wtcc: drives the release binary in a detached
# tmux session, verifies it renders and quits cleanly. No human interaction.
# Requires: tmux, git. Uses a throwaway XDG_CONFIG_HOME (never touches your config).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BIN="$ROOT/target/release/wtcc"
[ -x "$BIN" ] || { echo "build first: cargo build --release"; exit 1; }
command -v tmux >/dev/null || { echo "tmux is required"; exit 1; }

TMPCFG="$(mktemp -d)"
SM="wtcc_smoke_$$"
cleanup() { tmux kill-session -t "$SM" 2>/dev/null || true; rm -rf "$TMPCFG"; }
trap cleanup EXIT

tmux new-session -d -s "$SM" -x 120 -y 40 \
  "env XDG_CONFIG_HOME=$TMPCFG TERM=xterm-256color $BIN"
sleep 2

frame="$(tmux capture-pane -t "$SM" -p)"
fail=0
grep -q "repos" <<<"$frame"   || { echo "FAIL: sidebar not rendered"; fail=1; }
grep -q "agent" <<<"$frame"   || { echo "FAIL: agent pane not rendered"; fail=1; }
grep -q "palette" <<<"$frame" || { echo "FAIL: status hints not rendered"; fail=1; }

tmux send-keys -t "$SM" ':'; sleep 0.6
grep -q "command palette" <<<"$(tmux capture-pane -t "$SM" -p)" \
  || { echo "FAIL: command palette did not open"; fail=1; }

tmux send-keys -t "$SM" Escape; sleep 0.3
tmux send-keys -t "$SM" 'q'; sleep 0.8
if tmux has-session -t "$SM" 2>/dev/null; then
  echo "FAIL: did not quit on 'q'"; fail=1
fi

[ "$fail" -eq 0 ] && echo "smoke test: PASS" || { echo "smoke test: FAIL"; exit 1; }
