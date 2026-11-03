#!/usr/bin/env bash
# List open, unclaimed, unblocked tasks, best first (AGENTS.md step 2). Read-only.
# Flags: --no-client, --area SLUG, --first, --limit N. See tools/claims.py.
exec python3 -I "$(dirname "$0")/claims.py" next "$@"
