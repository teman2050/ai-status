//! Built-in Claude Code hook client (`"AI STATUS" --hook claude`).
//!
//! Claude Code invokes the app binary itself as a hook command: the hook JSON arrives on
//! stdin, is converted to a unified AgentEvent, and is POSTed to the local server
//! (127.0.0.1:7799). This mirrors adapters/claude-code/asb_hook.py so packaged builds work
//! without Python or a repo checkout; the Python adapter remains for development use.
//!
//! Privacy: never sends prompt content, tool_input, or code; only token counts and status.
//! Always returns quickly (2s network timeout) and must never block Claude Code.

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

/// Entry point for `--hook <tool>`: never panics, never writes to stdout, always returns.
pub fn run(tool: &str) {
    if tool != "claude" {
        return;
    }
    let mut input = String::new();
    if std::io::stdin().read_to_string(&mut input).is_err() {
        return;
    }
    let Ok(data) = serde_json::from_str::<Value>(&input) else {
        return;
    };
    if let Some(event) = build_event(&data) {
        post_event(&event);
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
}
