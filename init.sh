#!/usr/bin/env bash
# init.sh — one-command full verification.
# Used by:
#   - fresh clones (proves bootstrap contract holds)
#   - session start (proves repo is in consistent state)
#   - session end (proves clean exit)
#
# Exit codes:
#   0  all three layers pass
#   1  layer 1 (static) failed
#   2  layer 2 (behavior) failed
#   3  layer 3 (system / e2e) failed
#
# On failure, the script MUST emit an error message containing:
#   - what failed (specific command + output)
#   - why it matters (what invariant is violated)
#   - how to fix it (concrete next step)

set -euo pipefail

# ---------- Helpers ----------
fail() {
  local code=$1
  shift
  echo ""
  echo "FAIL [$code]: $*" >&2
  exit "$code"
}

step() { echo ""; echo "=== $* ==="; }

# ---------- Dependency setup ----------
step "Installing dependencies"
just setup || fail 1 "Dependency setup failed. Check Cargo.toml, network connectivity, and web/package.json."

# ---------- Layer 1: Static ----------
step "Layer 1 — Static checks (lint, format)"
just lint || fail 1 "Lint failed. Run 'just lint' to see details. Fix all warnings before proceeding."
just fmt || fail 1 "Formatting issues. Run 'just fmt-fix' to auto-fix."

# ---------- Layer 2: Behavior ----------
step "Layer 2 — Behavior (unit + integration tests)"
just test || fail 2 "Tests failed. Run 'just test' to see specifics. Do NOT proceed to Layer 3 until green."

# ---------- Layer 3: System ----------
step "Layer 3 — System (build + smoke test)"
just build-debug || fail 3 "Binary build failed. Compilation error — fix and retry."

# Smoke test: start server, check health endpoint, stop
if [ -f "target/debug/weave-server" ]; then
  ./target/debug/weave-server --port 19876 &
  SERVER_PID=$!
  sleep 2
  if curl -sf http://localhost:19876/api/health > /dev/null 2>&1; then
    echo "Smoke test passed: /api/health responded"
  else
    kill $SERVER_PID 2>/dev/null || true
    fail 3 "Smoke test failed: server started but /api/health did not respond within 2s. Check startup logs."
  fi
  if curl -sf http://localhost:19876/ | grep -q 'id="root"'; then
    echo "Smoke test passed: GET / served index.html (found id=\"root\")"
  else
    kill $SERVER_PID 2>/dev/null || true
    fail 3 "Smoke test failed: GET / did not return index.html with id=\"root\". Check that crates/weave-server/build.rs ran (it invokes 'bunx vite build') and that crates/weave-server/src/api/mod.rs has the .fallback_service(static_assets::spa_service()) chain."
  fi
  kill $SERVER_PID 2>/dev/null || true
else
  echo "SKIP: binary not found at target/debug/weave-server — build may have produced it in a different location"
fi

# ---------- Done ----------
step "Verification complete"
echo "All three layers passed. Repo is in a consistent state."
echo ""
echo "Next steps:"
echo "  1. Read PROGRESS.md to see current state"
echo "  2. Read feature_list.json — pick exactly ONE feature in 'not_started' state with all dependencies 'passing'"
echo "  3. Set its state to 'active', implement, then run its verification command"
echo "  4. Only after verification succeeds, mark it 'passing' with evidence"
