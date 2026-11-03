#!/usr/bin/env bash
# Point git at the tracked hooks directory. Run once per clone.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
git config core.hooksPath tools/hooks
echo "hooks installed: $(git config core.hooksPath)"
