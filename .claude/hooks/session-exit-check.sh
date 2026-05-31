#!/bin/bash
# Stop-hook enforcement for the session exit checklist.
#
# When Claude Code is about to end a session, this script runs. If it exits 2,
# Claude Code surfaces the message back to the agent and prevents the stop —
# forcing the agent to complete the missing exit step before finishing.

set -u

# ─── PATHS ──────────────────────────────────────────────────────────────────
PROGRESS_FILE="PROGRESS.md"
FEATURE_LIST="feature_list.json"
WATCH_PATH=""
# ────────────────────────────────────────────────────────────────────────────

cd "${CLAUDE_PROJECT_DIR:-$(git rev-parse --show-toplevel 2>/dev/null)}" 2>/dev/null || exit 0

git rev-parse --git-dir >/dev/null 2>&1 || exit 0

PROGRESS_PATH="${WATCH_PATH:+$WATCH_PATH/}$PROGRESS_FILE"
FEATURE_PATH="${WATCH_PATH:+$WATCH_PATH/}$FEATURE_LIST"

# What changed in this session, relative to HEAD (staged + unstaged)
SCOPE="${WATCH_PATH:-.}"
CHANGED_FILES=$(git status --porcelain -- "$SCOPE" 2>/dev/null | awk '{print $NF}')

if [ -z "$CHANGED_FILES" ]; then
    exit 0
fi

OTHER_CHANGES=$(echo "$CHANGED_FILES" | grep -v -F "$PROGRESS_FILE" | grep -v "^$" || true)
PROGRESS_TOUCHED=$(echo "$CHANGED_FILES" | grep -F "$PROGRESS_FILE" || true)

MESSAGES=""

# Rule 1: code changed but PROGRESS.md is stale
if [ -n "$OTHER_CHANGES" ] && [ -z "$PROGRESS_TOUCHED" ] && [ -f "$PROGRESS_PATH" ]; then
    MESSAGES="${MESSAGES}- Files changed this session but $PROGRESS_FILE was not updated. Add a note for the next session under 'Current State' / 'Next Steps'.\n"
fi

# Rule 2: feature_list.json has stale "in-progress" / "active" entries
if [ -f "$FEATURE_PATH" ] && command -v python3 >/dev/null 2>&1; then
    STUCK=$(python3 -c "
import json
try:
    data = json.load(open('$FEATURE_PATH'))
    stuck = [f.get('id', '?') for f in data.get('features', [])
             if f.get('state') in ('in-progress', 'in_progress', 'active')]
    print(','.join(stuck))
except Exception:
    pass
" 2>/dev/null)
    if [ -n "$STUCK" ]; then
        MESSAGES="${MESSAGES}- $FEATURE_LIST has in-progress features: $STUCK. Either promote them to 'passing' (with evidence) or note the blocker in $PROGRESS_FILE.\n"
    fi
fi

if [ -n "$MESSAGES" ]; then
    {
        echo "Session exit checklist incomplete. Resolve the following before finishing:"
        echo ""
        printf "%b" "$MESSAGES"
        echo ""
        echo "Full checklist lives in CLAUDE.md (Session Exit Checklist section)."
    } >&2
    exit 2
fi

exit 0
