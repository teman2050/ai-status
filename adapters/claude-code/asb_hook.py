#!/usr/bin/env python3
"""AI STATUS — Claude Code hook adapter.

Invoked by Claude Code hooks: reads the hook JSON on stdin, converts it to a unified
AgentEvent, and POSTs it to the local AI STATUS server (127.0.0.1:7799).

Privacy: never sends prompt content, tool_input, or code; only token counts and status.
Always exits 0 with a 2s timeout, and never blocks Claude Code.
"""
import json
import os
import re
import sys
import urllib.request

API = "http://127.0.0.1:7799/api/events"
MSG_LIMIT = 120
TRANSCRIPT_MAX_BYTES = 80 * 1024 * 1024  # skip stats for huge transcripts to protect performance
TAIL_SCAN_BYTES = 256 * 1024  # only scan the tail to judge the "current" error; covers the recent entries

# "You've hit your session limit · resets 9:30pm (Asia/Tokyo)"
LIMIT_RE = re.compile(r"resets?\s+(?:at\s+)?([0-9]{1,2}(?::[0-9]{2})?\s*(?:am|pm)?)", re.I)
# "配额" also matches Chinese-locale quota errors (bilingual detection).
LIMIT_HINT = ("usage limit", "session limit", "hit your", "配额", "quota")
THROTTLE_HINT = ("temporarily limiting", "too many requests", "overloaded", "rate limit", "429")


def transcript_stats(path):
    """Return dict: tokens, limit_active, reset_at, throttled. On failure returns all None/False.

    - tokens: cumulative input+output+cache for this session
    - limit_active: no successful message after the last limit error in the tail (currently blocked)
    - reset_at: reset time parsed from the limit text (e.g. "9:30pm")
    - throttled: a transient throttle (429/overloaded) in the tail with no successful message after it
    """
    out = {"tokens": None, "limit_active": False, "reset_at": None, "throttled": False}
    if not path or not os.path.isfile(path):
        return out
    try:
        size = os.path.getsize(path)
        if size > TRANSCRIPT_MAX_BYTES:
            return out
        # cumulative tokens (lightweight: only json-parse lines containing usage)
        total = 0
        with open(path, "r", encoding="utf-8", errors="replace") as f:
            for line in f:
                if '"usage"' not in line:
                    continue
                try:
                    usage = (json.loads(line).get("message") or {}).get("usage") or {}
                except Exception:
                    continue
                total += (
                    usage.get("input_tokens", 0)
                    + usage.get("output_tokens", 0)
                    + usage.get("cache_creation_input_tokens", 0)
                    + usage.get("cache_read_input_tokens", 0)
                )
        out["tokens"] = total if total > 0 else None

        # tail scan to judge the "current" error: is there a successful usage after limit/throttle?
        with open(path, "rb") as f:
            if size > TAIL_SCAN_BYTES:
                f.seek(size - TAIL_SCAN_BYTES)
                f.readline()  # drop the possibly-truncated partial line
            tail = f.read().decode("utf-8", errors="replace").splitlines()
        for line in tail:
            low = line.lower()
            has_usage = '"usage"' in line and '"output_tokens"' in line
            is_err = '"isapierrormessage":true' in low.replace(" ", "")
            if has_usage:
                # a successful response -> clear any earlier error verdict (recovered)
                out["limit_active"] = False
                out["throttled"] = False
            if is_err and any(h in low for h in LIMIT_HINT):
                out["limit_active"] = True
                out["throttled"] = False
                m = LIMIT_RE.search(line)
                out["reset_at"] = m.group(1).strip() if m else None
            elif is_err and any(h in low for h in THROTTLE_HINT):
                out["throttled"] = True
        return out
    except Exception:
        return out


def build_event(data: dict):
    hook = data.get("hook_event_name", "")
    session = data.get("session_id") or "unknown"
    cwd = data.get("cwd") or ""
    workspace = os.path.basename(cwd.rstrip("/")) or "unknown"
    transcript = data.get("transcript_path")
    base = {
        "tool_id": "claude_code",
        "workspace": workspace,
        "session_id": session,
        "task_id": session,
    }
    if transcript:
        base["transcript_path"] = transcript

    if hook == "SessionStart":
        return {**base, "event_type": "tool_connected"}
    if hook == "UserPromptSubmit":
        # text is localized on the frontend by status; no hardcoded placeholder
        return {**base, "event_type": "task_started", "message": ""}
    if hook == "SessionEnd":
        return {**base, "event_type": "tool_disconnected"}
    if hook == "PostToolUseFailure":
        # send only the tool name (language-neutral); empty -> frontend shows "Failed"
        tool = data.get("tool_name") or ""
        return {**base, "event_type": "task_error", "message": tool}
    if hook == "Notification":
        note = (data.get("message") or "")[:MSG_LIMIT]
        if any(h in note.lower() for h in LIMIT_HINT) or "配额" in note:
            # paused text is localized on the frontend; no hardcoded string here
            return {**base, "event_type": "task_update", "status": "paused", "message": ""}
        return {**base, "event_type": "task_waiting", "message": note}

    # PostToolUse / Stop: read the transcript to determine tokens and errors
    if hook in ("PostToolUse", "Stop"):
        st = transcript_stats(transcript)
        event = dict(base)
        if st["tokens"] is not None:
            event["tokens"] = st["tokens"]
        if st["limit_active"]:
            # quota row text and countdown are localized on the frontend
            # (the backend transcript scan fills quota_reset)
            return {**event, "event_type": "task_update", "status": "paused", "message": ""}
        if hook == "PostToolUse":
            event["event_type"] = "task_update"
            event["status"] = "running"
            # still running while throttled: text localized on frontend; otherwise send the tool name
            event["message"] = "" if st["throttled"] else (data.get("tool_name") or "")
            return event
        # Stop and not throttled -> done (done auto-hides, no text needed)
        return {**event, "event_type": "task_done", "message": ""}
    return None


def main():
    try:
        data = json.load(sys.stdin)
        event = build_event(data)
        if event is None:
            return
        req = urllib.request.Request(
            API,
            data=json.dumps(event).encode("utf-8"),
            headers={"Content-Type": "application/json"},
            method="POST",
        )
        urllib.request.urlopen(req, timeout=2)
    except Exception:
        pass  # status board not running / any error must not affect Claude Code


if __name__ == "__main__":
    main()
    sys.exit(0)
