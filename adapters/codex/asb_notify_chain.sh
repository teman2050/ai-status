#!/usr/bin/env bash
# AI STATUS chained notify: report status to the local status board.
# Locates asb_notify.py via the script's own directory, so it's portable
# (works no matter where the repo lives).
DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# If you already had another Codex notifier and want to keep it, uncomment the next
# line and point it at your notifier:
# "/path/to/your/original/notifier" "turn-ended" "$@" 2>/dev/null || true

python3 "$DIR/asb_notify.py" "$@" 2>/dev/null || true
exit 0
