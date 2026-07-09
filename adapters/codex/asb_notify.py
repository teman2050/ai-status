#!/usr/bin/env python3
"""Codex notify adapter.

Codex (both the CLI and the desktop app go through config.toml's `notify`) passes the
JSON of each notification as the last argv. Verified in practice: notify's thread-id ==
rollout's session_id, so here task_id = codex-<thread-id> points at the same task row as
the rollout watcher (codex.rs), avoiding duplicate rows.

Responsibility: only map approval-requested -> waiting (rollout can't see the transient
"awaiting approval" state). Running/done/error are handled by the rollout watcher.
Privacy: never sends input-messages / last-assistant-message or any content. Always exit 0.
"""
import json
import os
import sys
import urllib.request

API = "http://127.0.0.1:7799/api/events"


def main():
    raw = sys.argv[-1] if len(sys.argv) > 1 else ""
    try:
        data = json.loads(raw)
    except Exception:
        return
    if data.get("type") != "approval-requested":
        return  # everything else (agent-turn-complete, etc.) is left to the rollout watcher
    thread = data.get("thread-id") or data.get("thread_id") or "turn"
    cwd = data.get("cwd") or ""
    workspace = os.path.basename(str(cwd).rstrip("/")) or "Codex"
    task_id = f"codex-{thread}"
    event = {
        "tool_id": "codex",
        "event_type": "task_waiting",
        "workspace": workspace,
        "session_id": task_id,
        "task_id": task_id,
        "message": "",  # text is localized on the frontend by status (Waiting for input)
    }
    try:
        req = urllib.request.Request(
            API,
            data=json.dumps(event).encode("utf-8"),
            headers={"Content-Type": "application/json"},
            method="POST",
        )
        urllib.request.urlopen(req, timeout=2)
    except Exception:
        pass


if __name__ == "__main__":
    main()
    sys.exit(0)
