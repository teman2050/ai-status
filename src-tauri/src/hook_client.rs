//! Built-in hook/notify client (`"AI STATUS" --hook <claude|codex|cursor>`).
//!
//! The AI tool invokes the app binary itself as its hook/notify command; the payload is
//! converted to a unified AgentEvent and POSTed to the local server (127.0.0.1:7799).
//! This mirrors the Python adapters (asb_hook.py / asb_notify.py / asb_cursor_hook.py) so
//! packaged builds work without Python or a repo checkout; the adapters remain for
//! development use.
//!
//! - claude: hook JSON on stdin (see claude_hooks.rs). Never writes to stdout.
//! - codex:  notification JSON as the last argv; any original notifier saved after
//!   `--chain` by codex_notify.rs is re-invoked first, so it is never swallowed.
//! - cursor: hook JSON on stdin (see cursor_hooks.rs). Gating events must be answered on
//!   stdout with a pass-through decision or Cursor blocks the user's action; all other
//!   events get no stdout at all.
//!
//! Privacy: never sends prompt content, tool_input, or code; only token counts and status.
//! Always returns quickly (2s network timeout) and must never block the calling tool.

use crate::store::{transcript_limit, Blocked};
use serde_json::{json, Value};
use std::io::Read;
use std::time::Duration;

const ADDR: &str = "127.0.0.1:7799";
const MSG_LIMIT: usize = 120;
// Skip token counting for huge transcripts to protect performance (same cap as the Python adapter).
const TRANSCRIPT_MAX_BYTES: u64 = 80 * 1024 * 1024;

// "配额" also matches Chinese-locale quota errors (bilingual detection).
const LIMIT_HINT: &[&str] = &["usage limit", "session limit", "hit your", "配额", "quota"];

/// Cumulative input+output+cache tokens for the session; None when unknown.
fn transcript_tokens(path: &str) -> Option<u64> {
    use std::io::BufRead;
    let meta = std::fs::metadata(path).ok()?;
    if meta.len() > TRANSCRIPT_MAX_BYTES {
        return None;
    }
    let file = std::fs::File::open(path).ok()?;
    let mut total: u64 = 0;
    for line in std::io::BufReader::new(file).lines() {
        let Ok(line) = line else { break };
        if !line.contains("\"usage\"") {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let usage = &v["message"]["usage"];
        for key in [
            "input_tokens",
            "output_tokens",
            "cache_creation_input_tokens",
            "cache_read_input_tokens",
        ] {
            total += usage[key].as_u64().unwrap_or(0);
        }
    }
    (total > 0).then_some(total)
}

fn workspace_from_cwd(cwd: &str) -> String {
    cwd.trim_end_matches(['/', '\\'])
        .rsplit(['/', '\\'])
        .next()
        .filter(|s| !s.is_empty())
        .unwrap_or("unknown")
        .to_string()
}

fn truncate_chars(s: &str, limit: usize) -> String {
    s.chars().take(limit).collect()
}

/// Convert a Claude Code hook payload into a unified AgentEvent; None means "not reported".
fn build_event(data: &Value) -> Option<Value> {
    let hook = data["hook_event_name"].as_str().unwrap_or("");
    let session = data["session_id"].as_str().unwrap_or("unknown");
    let cwd = data["cwd"].as_str().unwrap_or("");
    let transcript = data["transcript_path"].as_str();
    let tool_name = data["tool_name"].as_str().unwrap_or("");

    let mut base = json!({
        "tool_id": "claude_code",
        "workspace": workspace_from_cwd(cwd),
        "session_id": session,
        "task_id": session,
    });
    if let Some(t) = transcript {
        base["transcript_path"] = json!(t);
    }
    fn merged(base: &Value, extra: Value) -> Option<Value> {
        let mut out = base.clone();
        for (k, v) in extra.as_object().unwrap() {
            out[k] = v.clone();
        }
        Some(out)
    }

    match hook {
        "SessionStart" => merged(&base, json!({"event_type": "tool_connected"})),
        // text is localized on the frontend by status; no hardcoded placeholder
        "UserPromptSubmit" => merged(&base, json!({"event_type": "task_started", "message": ""})),
        "SessionEnd" => merged(&base, json!({"event_type": "tool_disconnected"})),
        // send only the tool name (language-neutral); empty -> frontend shows "Failed"
        "PostToolUseFailure" | "StopFailure" | "PermissionDenied" => {
            merged(&base, json!({"event_type": "task_error", "message": tool_name}))
        }
        "PermissionRequest" | "Elicitation" => {
            merged(&base, json!({"event_type": "task_waiting", "message": tool_name}))
        }
        "Notification" => {
            let note = data["message"].as_str().unwrap_or("");
            let low = note.to_lowercase();
            if LIMIT_HINT.iter().any(|h| low.contains(h)) {
                // paused text is localized on the frontend; no hardcoded string here
                merged(&base, json!({"event_type": "task_update", "status": "paused", "message": ""}))
            } else {
                merged(&base, json!({"event_type": "task_waiting", "message": truncate_chars(note, MSG_LIMIT)}))
            }
        }
        // PreToolUse / PostToolUse / Stop: read the transcript for tokens and quota/throttle state
        "PreToolUse" | "PostToolUse" | "Stop" => {
            if let Some(tokens) = transcript.and_then(transcript_tokens) {
                base["tokens"] = json!(tokens);
            }
            let blocked = transcript.map(transcript_limit).unwrap_or(Blocked::Clear);
            if matches!(blocked, Blocked::Quota { .. }) {
                // quota row text and countdown are localized on the frontend
                // (the server's transcript scan fills quota_reset)
                return merged(&base, json!({"event_type": "task_update", "status": "paused", "message": ""}));
            }
            if hook == "Stop" {
                // done auto-hides, no text needed
                return merged(&base, json!({"event_type": "task_done", "message": ""}));
            }
            // still running while throttled: text localized on frontend; otherwise send the tool name
            let message = if matches!(blocked, Blocked::Throttled) { "" } else { tool_name };
            merged(&base, json!({"event_type": "task_update", "status": "running", "message": message}))
        }
        _ => None,
    }
}

/// Minimal HTTP/1.1 POST over TcpStream (no extra dependencies; the server is tiny_http on localhost).
fn post_event(event: &Value) {
    use std::io::Write;
    use std::net::{SocketAddr, TcpStream};
    let body = event.to_string();
    let Ok(addr) = ADDR.parse::<SocketAddr>() else {
        return;
    };
    let timeout = Duration::from_secs(2);
    let Ok(mut stream) = TcpStream::connect_timeout(&addr, timeout) else {
        return;
    };
    let _ = stream.set_read_timeout(Some(timeout));
    let _ = stream.set_write_timeout(Some(timeout));
    let request = format!(
        "POST /api/events HTTP/1.1\r\nHost: {ADDR}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    if stream.write_all(request.as_bytes()).is_err() {
        return;
    }
    let mut resp = Vec::new();
    let _ = stream.take(4096).read_to_end(&mut resp); // drain so the server sees a clean close
}

/// Convert a Cursor hook payload into a unified AgentEvent; None means "not reported".
/// Mirrors adapters/cursor/asb_cursor_hook.py.
fn build_cursor_event(data: &Value) -> Option<Value> {
    // Cursor event -> running heartbeat; value is a fallback label when tool_name is absent.
    // Gating events are listed too: we don't register them, but a legacy or manual install may.
    const RUNNING_EVENTS: [(&str, &str); 6] = [
        ("afterFileEdit", "Edit"),
        ("beforeShellExecution", "Shell"),
        ("afterShellExecution", "Shell"),
        ("beforeReadFile", "Read"),
        ("beforeMCPExecution", "MCP"),
        ("preToolUse", "Tool"),
    ];

    let hook = data["hook_event_name"].as_str().unwrap_or("");
    let session = data["conversation_id"]
        .as_str()
        .or(data["session_id"].as_str())
        .unwrap_or("unknown");
    let root = data["workspace_roots"][0]
        .as_str()
        .or(data["cwd"].as_str())
        .unwrap_or("");
    let workspace = if root.is_empty() {
        "Cursor".to_string()
    } else {
        workspace_from_cwd(root)
    };
    let mut base = json!({
        "tool_id": "cursor",
        "workspace": workspace,
        "session_id": session,
        "task_id": session,
    });
    if let Some(t) = data["transcript_path"].as_str() {
        base["transcript_path"] = json!(t);
    }
    fn merged(base: &Value, extra: Value) -> Option<Value> {
        let mut out = base.clone();
        for (k, v) in extra.as_object().unwrap() {
            out[k] = v.clone();
        }
        Some(out)
    }

    match hook {
        // text is localized on the frontend by status; no hardcoded placeholder
        "beforeSubmitPrompt" => merged(&base, json!({"event_type": "task_started", "message": ""})),
        // turn-level status: completed = done; interrupted/errored (non-completed) -> error.
        "stop" => {
            let status = data["status"].as_str();
            if status.is_some_and(|s| s != "completed") {
                merged(&base, json!({"event_type": "task_error", "message": ""}))
            } else {
                merged(&base, json!({"event_type": "task_done", "message": ""}))
            }
        }
        "sessionEnd" => merged(&base, json!({"event_type": "task_done", "message": ""})),
        _ => {
            let fallback = RUNNING_EVENTS.iter().find(|(h, _)| *h == hook)?.1;
            let tool = data["tool_name"].as_str().filter(|s| !s.is_empty()).unwrap_or(fallback);
            merged(&base, json!({"event_type": "task_update", "status": "running", "message": tool}))
        }
    }
}

/// The stdout answer a Cursor gating event requires (pass-through: never steal the
/// decision). Non-gating events must get no stdout at all, or Cursor could misparse.
fn cursor_response(hook: &str) -> Option<&'static str> {
    match hook {
        "beforeSubmitPrompt" => Some(r#"{"continue": true}"#),
        "beforeShellExecution" | "beforeMCPExecution" => Some(r#"{"permission": "ask"}"#),
        _ => None,
    }
}

/// Convert a Codex notify payload into a unified AgentEvent; None means "not reported".
/// Mirrors adapters/codex/asb_notify.py: only approval-requested -> waiting (the rollout
/// watcher in codex.rs owns running/done/error), task_id matches its codex-<thread> rows.
fn build_codex_event(data: &Value) -> Option<Value> {
    if data["type"].as_str() != Some("approval-requested") {
        return None;
    }
    let thread = data["thread-id"]
        .as_str()
        .or(data["thread_id"].as_str())
        .unwrap_or("turn");
    let cwd = data["cwd"].as_str().unwrap_or("");
    let workspace = if cwd.is_empty() {
        "Codex".to_string()
    } else {
        workspace_from_cwd(cwd)
    };
    let task_id = format!("codex-{thread}");
    Some(json!({
        "tool_id": "codex",
        "event_type": "task_waiting",
        "workspace": workspace,
        "session_id": task_id,
        "task_id": task_id,
        "message": "", // text is localized on the frontend by status (Waiting for input)
    }))
}

/// Split the argv after `--hook codex` into (original notifier saved by codex_notify.rs,
/// notification JSON appended by Codex as the last argument).
fn codex_split(rest: &[String]) -> (&[String], Option<&str>) {
    let body = if rest.first().map(String::as_str) == Some("--chain") {
        &rest[1..]
    } else {
        rest
    };
    match body.split_last() {
        Some((payload, chain)) => (chain, Some(payload.as_str())),
        None => (&[], None),
    }
}

/// Re-invoke the user's original notifier with the notification appended, exactly as
/// Codex would have called it. Fire-and-forget: never blocks and never swallows it.
fn forward_chain(chain: &[String], payload: &str) {
    let Some((prog, args)) = chain.split_first() else {
        return;
    };
    let mut cmd = std::process::Command::new(prog);
    cmd.args(args)
        .arg(payload)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW: no console flash
    }
    let _ = cmd.spawn();
}

fn run_stdin(build: fn(&Value) -> Option<Value>, respond: fn(&str) -> Option<&'static str>) {
    let mut input = String::new();
    if std::io::stdin().read_to_string(&mut input).is_err() {
        return;
    }
    let Ok(data) = serde_json::from_str::<Value>(&input) else {
        return;
    };
    // answer gating events first so the tool unblocks before our (2s-capped) POST
    if let Some(answer) = respond(data["hook_event_name"].as_str().unwrap_or("")) {
        use std::io::Write;
        let mut out = std::io::stdout();
        let _ = out.write_all(answer.as_bytes());
        let _ = out.flush();
    }
    if let Some(event) = build(&data) {
        post_event(&event);
    }
}

fn run_codex() {
    let args: Vec<String> = std::env::args().collect();
    let Some(pos) = args.iter().position(|a| a == "--hook") else {
        return;
    };
    let rest = args.get(pos + 2..).unwrap_or(&[]);
    let (chain, payload) = codex_split(rest);
    let Some(payload) = payload else {
        return;
    };
    forward_chain(chain, payload);
    if let Ok(data) = serde_json::from_str::<Value>(payload) {
        if let Some(event) = build_codex_event(&data) {
            post_event(&event);
        }
    }
}

/// Entry point for `--hook <tool>`: never panics, always returns; stdout is written only
/// for Cursor gating answers.
pub fn run(tool: &str) {
    match tool {
        "claude" => run_stdin(build_event, |_| None),
        "cursor" => run_stdin(build_cursor_event, cursor_response),
        "codex" => run_codex(),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn payload(hook: &str) -> Value {
        json!({
            "hook_event_name": hook,
            "session_id": "s1",
            "cwd": "C:\\work\\DemoApp",
            "tool_name": "Bash",
        })
    }

    #[test]
    fn prompt_submit_starts_a_task() {
        let ev = build_event(&payload("UserPromptSubmit")).unwrap();
        assert_eq!(ev["event_type"], "task_started");
        assert_eq!(ev["workspace"], "DemoApp");
        assert_eq!(ev["task_id"], "s1");
    }

    #[test]
    fn permission_request_maps_to_waiting_with_tool_name() {
        let ev = build_event(&payload("PermissionRequest")).unwrap();
        assert_eq!(ev["event_type"], "task_waiting");
        assert_eq!(ev["message"], "Bash");
    }

    #[test]
    fn limit_notification_maps_to_paused() {
        let mut data = payload("Notification");
        data["message"] = json!("You've hit your usage limit");
        let ev = build_event(&data).unwrap();
        assert_eq!(ev["event_type"], "task_update");
        assert_eq!(ev["status"], "paused");
    }

    #[test]
    fn post_tool_use_reports_running_and_tokens() {
        let mut path = std::env::temp_dir();
        path.push(format!("asb_hookclient_tokens_{}.jsonl", std::process::id()));
        let mut f = std::fs::File::create(&path).unwrap();
        writeln!(
            f,
            r#"{{"sessionId":"s1","message":{{"usage":{{"input_tokens":10,"output_tokens":5,"cache_read_input_tokens":85}}}}}}"#
        )
        .unwrap();
        let mut data = payload("PostToolUse");
        data["transcript_path"] = json!(path.to_string_lossy());
        let ev = build_event(&data).unwrap();
        assert_eq!(ev["event_type"], "task_update");
        assert_eq!(ev["status"], "running");
        assert_eq!(ev["message"], "Bash");
        assert_eq!(ev["tokens"], 100);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn stop_on_quota_blocked_transcript_pauses_instead_of_done() {
        let mut path = std::env::temp_dir();
        path.push(format!("asb_hookclient_quota_{}.jsonl", std::process::id()));
        let mut f = std::fs::File::create(&path).unwrap();
        writeln!(
            f,
            r#"{{"sessionId":"s1","isApiErrorMessage":true,"message":"Usage limit reached. Resets at 2:50 PM"}}"#
        )
        .unwrap();
        let mut data = payload("Stop");
        data["transcript_path"] = json!(path.to_string_lossy());
        let ev = build_event(&data).unwrap();
        assert_eq!(ev["event_type"], "task_update");
        assert_eq!(ev["status"], "paused");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn plain_stop_reports_done() {
        let ev = build_event(&payload("Stop")).unwrap();
        assert_eq!(ev["event_type"], "task_done");
    }

    #[test]
    fn unknown_hook_is_ignored() {
        assert!(build_event(&payload("SomethingElse")).is_none());
    }

    // ---- cursor ----

    fn cursor_payload(hook: &str) -> Value {
        json!({
            "hook_event_name": hook,
            "conversation_id": "c1",
            "workspace_roots": ["/Users/dev/DemoApp"],
            "tool_name": "Bash",
        })
    }

    #[test]
    fn cursor_submit_starts_a_task() {
        let ev = build_cursor_event(&cursor_payload("beforeSubmitPrompt")).unwrap();
        assert_eq!(ev["event_type"], "task_started");
        assert_eq!(ev["tool_id"], "cursor");
        assert_eq!(ev["workspace"], "DemoApp");
        assert_eq!(ev["task_id"], "c1");
    }

    #[test]
    fn cursor_stop_maps_completed_to_done_and_aborted_to_error() {
        let mut data = cursor_payload("stop");
        data["status"] = json!("completed");
        assert_eq!(build_cursor_event(&data).unwrap()["event_type"], "task_done");
        data["status"] = json!("aborted");
        assert_eq!(build_cursor_event(&data).unwrap()["event_type"], "task_error");
        // no status field at all still counts as done
        assert_eq!(
            build_cursor_event(&cursor_payload("stop")).unwrap()["event_type"],
            "task_done"
        );
    }

    #[test]
    fn cursor_activity_reports_running_with_tool_name() {
        let ev = build_cursor_event(&cursor_payload("afterShellExecution")).unwrap();
        assert_eq!(ev["event_type"], "task_update");
        assert_eq!(ev["status"], "running");
        assert_eq!(ev["message"], "Bash");
        // fallback label when tool_name is absent
        let mut data = cursor_payload("afterFileEdit");
        data["tool_name"] = json!("");
        assert_eq!(build_cursor_event(&data).unwrap()["message"], "Edit");
    }

    #[test]
    fn cursor_unknown_hook_is_ignored_and_workspace_falls_back() {
        assert!(build_cursor_event(&cursor_payload("somethingElse")).is_none());
        let data = json!({"hook_event_name": "sessionEnd"});
        let ev = build_cursor_event(&data).unwrap();
        assert_eq!(ev["workspace"], "Cursor");
        assert_eq!(ev["session_id"], "unknown");
    }

    #[test]
    fn cursor_gating_events_get_pass_through_answers_only() {
        assert_eq!(cursor_response("beforeSubmitPrompt"), Some(r#"{"continue": true}"#));
        assert_eq!(cursor_response("beforeShellExecution"), Some(r#"{"permission": "ask"}"#));
        assert_eq!(cursor_response("beforeMCPExecution"), Some(r#"{"permission": "ask"}"#));
        // observational events must never get stdout output
        for hook in ["afterFileEdit", "afterShellExecution", "stop", "sessionEnd", ""] {
            assert_eq!(cursor_response(hook), None, "{hook} must stay silent");
        }
    }

    // ---- codex ----

    #[test]
    fn codex_approval_maps_to_waiting_on_the_rollout_task_row() {
        let data = json!({
            "type": "approval-requested",
            "thread-id": "t-42",
            "cwd": "/Users/dev/DemoApp",
        });
        let ev = build_codex_event(&data).unwrap();
        assert_eq!(ev["event_type"], "task_waiting");
        assert_eq!(ev["tool_id"], "codex");
        assert_eq!(ev["task_id"], "codex-t-42");
        assert_eq!(ev["workspace"], "DemoApp");
    }

    #[test]
    fn codex_other_notifications_are_left_to_the_rollout_watcher() {
        let data = json!({"type": "agent-turn-complete", "thread-id": "t-42"});
        assert!(build_codex_event(&data).is_none());
    }

    fn strs(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn codex_split_separates_chain_and_payload() {
        // plain install: only the JSON appended by Codex
        let rest = strs(&["{\"type\":\"x\"}"]);
        let (chain, payload) = codex_split(&rest);
        assert!(chain.is_empty());
        assert_eq!(payload, Some("{\"type\":\"x\"}"));

        // chained install: original notifier + its args, then the JSON
        let rest = strs(&["--chain", "C:/OpenAI/codex-computer-use.exe", "turn-ended", "{}"]);
        let (chain, payload) = codex_split(&rest);
        assert_eq!(chain, strs(&["C:/OpenAI/codex-computer-use.exe", "turn-ended"]));
        assert_eq!(payload, Some("{}"));

        // degenerate: nothing appended
        let (chain, payload) = codex_split(&[]);
        assert!(chain.is_empty());
        assert_eq!(payload, None);
    }
}
