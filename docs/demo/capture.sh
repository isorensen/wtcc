#!/usr/bin/env bash
# Reproduces the README demo screenshots: builds the release binary, seeds a
# throwaway repo (`acme-api`) with a couple of worktrees and a scripted stand-in
# agent, launches wtcc in a detached tmux session, and prints the captured
# frames (the "hero" view and the `?` help overlay) to stdout.
#
# It writes nothing to your real config and cleans up after itself.
# Requires: tmux, git. Run from the repo root: docs/demo/capture.sh
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
BIN="$ROOT/target/release/wtcc"
[ -x "$BIN" ] || { echo "build first: cargo build --release"; exit 1; }

WORK="$(mktemp -d)"; CFG="$WORK/cfg"; REPO="$WORK/acme-api"
SM="wtcc_demo_$$"
cleanup() { tmux kill-session -t "$SM" 2>/dev/null || true; rm -rf "$WORK"; }
trap cleanup EXIT
mkdir -p "$CFG/wtcc"

git init -q -b main "$REPO"
git -C "$REPO" config user.email demo@acme.dev
git -C "$REPO" config user.name demo
echo "# acme-api" > "$REPO/README.md"
git -C "$REPO" add -A && git -C "$REPO" commit -qm init
git -C "$REPO" worktree add -q "$WORK/wt-login" -b feat/login
git -C "$REPO" worktree add -q "$WORK/wt-pay" -b fix/payments

# A scripted stand-in for the agent so the demo is clean and deterministic.
cat > "$WORK/agent.sh" <<'EOS'
#!/usr/bin/env bash
printf '\033[1mClaude Code\033[0m  —  acme-api · feat/login\n\n'
printf '> implement the login form validation\n\n'
printf '  ✓ read src/auth/login.ts\n  ✎ editing src/auth/validators.ts\n  ✓ added 3 tests\n\n'
printf 'Running tests…  \033[32m12 passed\033[0m\n'
sleep 600
EOS
chmod +x "$WORK/agent.sh"

cat > "$CFG/wtcc/config.toml" <<EOF
agent_cmd = "$WORK/agent.sh"

[[repos]]
name = "acme-api"
path = "$REPO"
EOF

tmux new-session -d -s "$SM" -x 92 -y 26 \
  "env XDG_CONFIG_HOME=$CFG TERM=xterm-256color $BIN"
sleep 1.5
tmux send-keys -t "$SM" 'j'; sleep 0.8          # select feat/login (the working agent)
echo "===== hero ====="
tmux capture-pane -t "$SM" -p | sed 's/[[:space:]]*$//'
tmux send-keys -t "$SM" '?'; sleep 0.6
echo "===== help ====="
tmux capture-pane -t "$SM" -p | sed 's/[[:space:]]*$//'
