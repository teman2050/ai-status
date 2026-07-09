use crate::store::{IncomingEvent, Store};
use std::collections::HashSet;
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

const POLL_SECS: u64 = 3;

struct Watched {
    tool_id: &'static str,
    process_name: &'static str, // exact match via pgrep -x
}

const WATCHED: &[Watched] = &[
    Watched {
        tool_id: "codex",
        process_name: "codex", // CLI
    },
    Watched {
        tool_id: "codex",
        process_name: "Codex", // desktop app
    },
    Watched {
        tool_id: "cursor",
        process_name: "Cursor",
    },
];

fn is_running(name: &str) -> bool {
    Command::new("pgrep")
        .args(["-x", name])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// return a list of (tool_id, came_online) transitions; sorted for determinism
pub fn diff(prev: &HashSet<String>, cur: &HashSet<String>) -> Vec<(String, bool)> {
    let mut out: Vec<(String, bool)> = Vec::new();
    for t in cur.difference(prev) {
        out.push((t.clone(), true));
    }
    for t in prev.difference(cur) {
        out.push((t.clone(), false));
    }
    out.sort();
    out
}

fn event(tool_id: &str, connected: bool) -> IncomingEvent {
    IncomingEvent {
        tool_id: tool_id.to_string(),
        event_type: if connected {
            "tool_connected"
        } else {
            "tool_disconnected"
        }
        .to_string(),
        workspace: None,
        cwd: None,
        session_id: None,
        task_id: None,
        status: None,
        message: None,
        tokens: None,
        transcript_path: None,
        timestamp: None,
    }
}

pub fn start(store: Arc<Mutex<Store>>) {
    thread::spawn(move || {
        let mut prev: HashSet<String> = HashSet::new();
        // Claude connections are managed by hooks (SessionStart/End); this is only a fallback:
        // force-disconnect when all claude processes vanish (crash / all terminals closed, SessionEnd not fired).
        let mut claude_was_alive = false;
        loop {
            let mut cur: HashSet<String> = HashSet::new();
            for w in WATCHED {
                if is_running(w.process_name) {
                    cur.insert(w.tool_id.to_string());
                }
            }
            for (tool, up) in diff(&prev, &cur) {
                store.lock().unwrap().apply(event(&tool, up));
            }
            prev = cur;

            let claude_alive = is_running("claude");
            if claude_was_alive && !claude_alive {
                // falling edge: all claude sessions are gone, clear the claude_code section
                store
                    .lock()
                    .unwrap()
                    .apply(event("claude_code", false));
            }
            claude_was_alive = claude_alive;

            thread::sleep(Duration::from_secs(POLL_SECS));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(items: &[&str]) -> HashSet<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn diff_detects_connect_and_disconnect() {
        let transitions = diff(&set(&["codex"]), &set(&["cursor"]));
        assert_eq!(
            transitions,
            vec![("codex".to_string(), false), ("cursor".to_string(), true)]
        );
    }

    #[test]
    fn diff_empty_when_no_change() {
        assert!(diff(&set(&["codex"]), &set(&["codex"])).is_empty());
        assert!(diff(&set(&[]), &set(&[])).is_empty());
    }
}
