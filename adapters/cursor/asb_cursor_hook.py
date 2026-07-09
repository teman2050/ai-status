#!/usr/bin/env python3
"""AI STATUS — Cursor hook adapter.

Invoked by Cursor hooks (~/.cursor/hooks.json): reads the hook JSON on stdin, converts it
to a unified AgentEvent, and POSTs it to the local AI STATUS server (127.0.0.1:7799).

Cursor provides conversation_id / workspace_roots / tool_name / transcript_path, so its
capabilities match Claude Code (including the transcript-mtime heartbeat).
Privacy: never sends prompt content, tool_input, or code. Always exits 0 with a 2s timeout,
and never blocks Cursor.
"""
import json
import os
import sys
import urllib.request

API = "http://127.0.0.1:7799/api/events"

# Cursor event -> running heartbeat (tool activity); value is a fallback label when tool_name is absent
RUNNING_EVENTS = {
    "afterFileEdit": "Edit",
    "beforeShellExecution": "Shell",
    "afterShellExecution": "Shell",
    "beforeReadFile": "Read",
    "beforeMCPExecution": "MCP",
    "preToolUse": "Tool",
}


def build_event(data: dict):
    hook = data.get("hook_event_name", "")
    session = data.get("conversation_id") or data.get("session_id") or "unknown"
    roots = data.get("workspace_roots") or []
    root = roots[0] if roots else (data.get("cwd") or "")
    workspace = os.path.basename(str(root).rstrip("/")) or "Cursor"
    base = {
        "tool_id": "cursor",
        "workspace": workspace,
        "session_id": session,
        "task_id": session,
    }
    transcript = data.get("transcript_path")
    if transcript:
        base["transcript_path"] = transcript

    if hook == "beforeSubmitPrompt":
        # text is localized on the frontend by status; no hardcoded placeholder
        return {**base, "event_type": "task_started", "message": ""}
    if hook == "stop":
        # turn-level status: completed = done; interrupted/errored (non-completed) -> error.
        # Note: a single failed shell command does not make status != completed (the agent
        # absorbs it), so we only catch turn-level interruptions/errors, not every failed command.
        status = data.get("status")
        if status and status != "completed":
            return {**base, "event_type": "task_error", "message": ""}
        return {**base, "event_type": "task_done", "message": ""}
    if hook == "sessionEnd":
        return {**base, "event_type": "task_done", "message": ""}
    if hook in RUNNING_EVENTS:
        tool = data.get("tool_name") or RUNNING_EVENTS[hook]
        return {**base, "event_type": "task_update", "status": "running", "message": tool}
    return None


def main():
    try:
        data = json.load(sys.stdin)
        event = build_event(data)
        if event is not None:
            req = urllib.request.Request(
                API,
                data=json.dumps(event).encode("utf-8"),
                headers={"Content-Type": "application/json"},
                method="POST",
            )
            urllib.request.urlopen(req, timeout=2)
    except Exception:
        pass  # status board not running / any error must not affect Cursor


if __name__ == "__main__":
    main()
    # a Cursor command hook with no output is treated as pass-through, so it never affects the agent
    sys.exit(0)
