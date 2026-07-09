# AI-tool status detection spec

This spec summarizes how AI STATUS detects the status of each AI coding tool, so it can be **reused when adding new tools** (Trae, Tongyi Lingma, Qoder, etc.). Core principle: every tool's raw signals are converted into one set of "unified events"; the UI only consumes unified events, and adapters only do the conversion.

---

## 1. Unified status model

Each tool is a `Tool`, with several `Task`s under it (one session / one turn). A task's user-visible status is only one of these:

| visible_status | meaning | visual |
|---|---|---|
| `running` | working | green ring (gapped, spinning) |
| `waiting` | awaiting user input/confirmation/approval | yellow ring + ? |
| `error` | turn-level error/abort (**not a single failed command**) | red ring + × |
| `paused` | out of quota, awaiting reset | yellow ring + ‖, with the reset time |
| `done_event` | just finished | green check, disappears after 3~5s |
| `stale` | lost (no signal for a long time, not dead) | yellow ring + blinking ⇄ |

A tool's connection state is only `connected` (green / section appears) or `disconnected` (section disappears, **not a red light**).

> Key judgment: **a single failed shell command ≠ error**. The agent usually absorbs a failed command and keeps working; only a whole turn being interrupted/erroring counts as an error.

---

## 2. Interception mechanism priority (best to worst)

When adding a new tool, look for a signal source in this order:

1. **Official hooks (best)**: push-based, real-time, semantically clear. Claude Code, Cursor, and Codex all have hooks/notify. Configure a hook script; the tool calls it on lifecycle events, and the script converts the stdin/argv JSON into a unified event and POSTs it.
2. **Push notifications (notify)**: e.g. Codex's `notify`. On specific events (turn complete, approval requested) the tool hands JSON to a program.
3. **Log watching (polling)**: read the tool's session log (e.g. Codex's rollout jsonl), tail + parse event types. Has latency (the poll interval), and can misjudge when logs aren't written for a while — the "active window" needs to be relaxed.
4. **Process/window detection (fallback)**: `pgrep` to tell if it's online. Can only tell connection, not task status. Mind the process-name casing (CLI vs desktop app often differ).

**Combine sources when you can get several**: e.g. Codex = rollout (running/done/error) + notify (awaiting approval) + pgrep (online). The prerequisite for combining is that **the same key can point different signals at the same task row** (see below).

---

## 3. Detection recipes for the integrated tools (verified)

### 3.1 Claude Code — hooks (most complete, ~95%)
Configure the hooks in `~/.claude/settings.json` to call `asb_hook.py`:

| hook | -> unified event |
|---|---|
| SessionStart | tool_connected (with session_id) |
| UserPromptSubmit | task_started (running) |
| PostToolUse | task_update (running, message = tool name); scans the transcript to detect quota |
| PostToolUseFailure | task_error |
| Notification | task_waiting (paused if it contains "limit") |
| Stop | task_done (paused if it contains quota) |
| SessionEnd | tool_disconnected (with session_id) |

- **Heartbeat**: task events carry `transcript_path`; the server stats its mtime every second — a file modified recently = the session is alive (thinking/streaming), so it isn't misjudged as lost.
- **Quota**: scan the transcript tail for `isApiErrorMessage` containing "session/usage limit" (excluding transient throttles like 429/overloaded/rate limit), and parse "resets 9:30pm" for the reset time; **bidirectional**: once the limit clears and the session is active, paused -> running.
- **Multiple sessions**: the server counts sessions and only hides the section on the last SessionEnd.

### 3.2 Cursor — hooks (~85%)
Configure `~/.cursor/hooks.json` (schema version 1) to call `asb_cursor_hook.py`. Payload fields: `conversation_id`/`session_id`, `command`, `output`, `status`, `workspace_roots`, `transcript_path`.

| hook | -> unified event |
|---|---|
| beforeSubmitPrompt | task_started (running) |
| afterFileEdit / beforeShellExecution / afterShellExecution / beforeReadFile | task_update (running, heartbeat) |
| **stop** | `status`=="completed" -> task_done; **otherwise -> task_error** (turn-level interruption/error) |
| sessionEnd | task_done |

- `afterShellExecution` **only has output, no exit code**, so it isn't used to detect errors (and a single failed command is absorbed by the agent).
- `transcript_path` is sometimes null (e.g. beforeShellExecution), so the heartbeat only works when present.

### 3.3 Codex — rollout logs + notify (~85%)
Codex (desktop app process name `Codex`, CLI is `codex` — **pgrep both**):

- **rollout logs** (`~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl`): `codex.rs` tails the last `event_msg`: `task_started`=running / `task_complete`=done / `turn_aborted`=error. The active window is relaxed to 10 minutes to avoid false completion on long turns.
- **notify** (`config.toml`'s `notify`, also used by the desktop app): `asb_notify.py` maps `approval-requested` -> task_waiting.
- **Merge key**: verified that **notify's `thread-id` == rollout's `session_id`**; both use `task_id = codex-<id>`, so there are no duplicate rows.
- In Codex's auto-approve mode, approval-requested doesn't fire (the waiting-approval wiring is there but unused).

---

## 4. Adding a new tool (e.g. Trae)

1. **Pick tool_id / tool_name**: e.g. `trae` / `Trae`. Add a mapping in `display_name` in `store.rs`; add the process name to `WATCHED` in `watcher.rs` (online/offline); add a brand color in `toolAccent` in the frontend `status.ts`.
2. **Find a signal source** (by the priority in §2): does Trae have hooks? If yes -> write `adapters/trae/asb_trae_hook.py`, mapping lifecycle events to unified events. If no -> find logs/notify -> poll.
3. **Write the adapter**: convert the tool's raw events into unified `AgentEvent`s and POST to `http://127.0.0.1:7799/api/events`. **Send only** the tool name / project name / session id / status / token count / truncated summary — **never** prompts, code, tool_input, or full logs.
4. **Make sure tasks can be located**: multiple sources must use the same key (session/thread id or cwd) to point at the same task row.
5. **Zero frontend changes**: the UI only consumes unified Tool/Task, so adding a tool doesn't touch the UI.

### Unified event API
`POST /api/events`, body:
```json
{
  "tool_id": "trae",
  "event_type": "task_update",   // tool_connected/disconnected, task_started/update/waiting/error/done
  "workspace": "MyProject",
  "session_id": "xxx",
  "task_id": "trae-xxx",
  "status": "running",           // visible_status for task_update
  "message": "compiling",        // summary (marquee)
  "tokens": 12345,
  "transcript_path": "/path/...",// if present, the server uses its mtime as a heartbeat
  "timestamp": "2026-..."
}
```

---

## 5. Honesty principles for status semantics

- **Timeout ≠ done**: no signal for a long time -> `stale` (lost), not `done`. Completion is only triggered by an explicit completion signal.
- **Freeze stale detection while offline**: events naturally stop during network trouble, so don't misjudge silence as a dead task.
- **If you don't know, show "don't know"**: when fine-grained status isn't available, at least show "online" — never fabricate tasks or guess a plausible answer.
- **Process-name casing**: CLI and desktop-app process names often differ; pgrep must cover both.
