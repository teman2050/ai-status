#!/usr/bin/env python3
"""Install AI STATUS hooks into Claude Code's global settings.

The installer appends AI STATUS hook commands and preserves any existing hooks
from other tools. It backs up the settings file before writing.
"""

from __future__ import annotations

import json
import os
from pathlib import Path
import shlex
import shutil
import subprocess
import sys
import time


REPO_ROOT = Path(__file__).resolve().parents[1]
HOOK_SCRIPT = REPO_ROOT / "adapters" / "claude-code" / "asb_hook.py"
SETTINGS = Path.home() / ".claude" / "settings.json"
EVENTS = [
    "SessionStart",
    "UserPromptSubmit",
    "PreToolUse",
    "PostToolUse",
    "PostToolUseFailure",
    "PermissionRequest",
    "PermissionDenied",
    "Elicitation",
    "Notification",
    "StopFailure",
    "Stop",
    "SessionEnd",
]


def command_string() -> str:
    parts = [sys.executable, str(HOOK_SCRIPT)]
    if os.name == "nt":
        return subprocess.list2cmdline(parts)
    return shlex.join(parts)


def load_settings() -> dict:
    if not SETTINGS.exists():
        return {}
    with SETTINGS.open("r", encoding="utf-8") as f:
        return json.load(f)


def hook_exists(entries: list, command: str) -> bool:
    for entry in entries:
        for hook in entry.get("hooks", []):
            if hook.get("command") == command:
                return True
    return False


def main() -> None:
    dry_run = "--dry-run" in sys.argv
    if not HOOK_SCRIPT.exists():
        raise SystemExit(f"Hook script does not exist: {HOOK_SCRIPT}")

    command = command_string()
    settings = load_settings()
    hooks = settings.setdefault("hooks", {})
    added: list[str] = []

    for event in EVENTS:
        entries = hooks.setdefault(event, [])
        if hook_exists(entries, command):
            continue
        entries.append(
            {
                "hooks": [
                    {
                        "type": "command",
                        "command": command,
                        "timeout": 5,
                        "async": True,
                    }
                ]
            }
        )
        added.append(event)

    if dry_run:
        print(f"Settings: {SETTINGS}")
        print(f"Hook: {HOOK_SCRIPT}")
        print(f"Command: {command}")
        print(f"Would add: {added or 'nothing'}")
        return

    SETTINGS.parent.mkdir(parents=True, exist_ok=True)
    if SETTINGS.exists():
        backup = SETTINGS.with_name(
            f"{SETTINGS.name}.bak-{time.strftime('%Y%m%d-%H%M%S')}"
        )
        shutil.copy2(SETTINGS, backup)
        print(f"Backed up: {backup}")

    with SETTINGS.open("w", encoding="utf-8") as f:
        json.dump(settings, f, ensure_ascii=False, indent=2)
        f.write("\n")

    print(f"Added hooks: {added or 'nothing'}")
    print("Claude Code must start a new session before new hooks take effect.")


if __name__ == "__main__":
    main()
