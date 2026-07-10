use crate::store::{transcript_limit, Blocked, IncomingEvent, Store};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, SystemTime};

const POLL_SECS: u64 = 3;
const ACTIVE_WINDOW: Duration = Duration::from_secs(24 * 3600);
const MAX_TRANSCRIPTS: usize = 40;
const TAIL_BYTES: u64 = 256 * 1024;

#[derive(Debug, Clone)]
struct ClaudeQuota {
    task_id: String,
    workspace: String,
    transcript_path: String,
    quota_reset: String,
}

fn home_dir() -> PathBuf {
    std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_default()
}

fn claude_projects_dir() -> PathBuf {
    home_dir().join(".claude").join("projects")
}

fn recently_modified(path: &Path, window: Duration) -> bool {
    fs::metadata(path)
        .and_then(|m| m.modified())
        .map(|mt| {
            SystemTime::now()
                .duration_since(mt)
                .map(|age| age < window)
                .unwrap_or(true)
        })
        .unwrap_or(false)
}

fn collect_jsonl(dir: &Path, out: &mut Vec<(SystemTime, PathBuf)>) {
    let Ok(rd) = fs::read_dir(dir) else {
        return;
    };
    for entry in rd.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_jsonl(&path, out);
            continue;
        }
        if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        if !recently_modified(&path, ACTIVE_WINDOW) {
            continue;
        }
        let Ok(mtime) = fs::metadata(&path).and_then(|m| m.modified()) else {
            continue;
        };
        out.push((mtime, path));
    }
}

fn recent_transcripts(base: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    collect_jsonl(base, &mut files);
    files.sort_by(|a, b| b.0.cmp(&a.0));
    files
        .into_iter()
        .take(MAX_TRANSCRIPTS)
        .map(|(_, path)| path)
        .collect()
}

fn read_tail(path: &Path) -> Vec<String> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = match fs::File::open(path) {
        Ok(f) => f,
        Err(_) => return Vec::new(),
    };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    if f.seek(SeekFrom::Start(len.saturating_sub(TAIL_BYTES))).is_err() {
        return Vec::new();
    }
    let mut buf = String::new();
    if f.read_to_string(&mut buf).is_err() {
        return Vec::new();
    }
    buf.lines().map(|s| s.to_string()).collect()
}

fn workspace_from_pathish(value: &str) -> String {
    let trimmed = value.trim_end_matches(|c| c == '/' || c == '\\');
    trimmed
        .rsplit(|c| c == '/' || c == '\\')
        .next()
        .filter(|s| !s.is_empty())
        .unwrap_or("Claude Code")
        .to_string()
}

fn transcript_meta(path: &Path) -> (String, String) {
    let mut session_id = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("claude")
        .to_string();
    let mut workspace = path
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|s| s.to_str())
        .map(workspace_from_pathish)
        .unwrap_or_else(|| "Claude Code".to_string());

    for line in read_tail(path).into_iter().rev() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        if let Some(sid) = v.get("sessionId").and_then(|s| s.as_str()) {
            session_id = sid.to_string();
        }
        if let Some(cwd) = v.get("cwd").and_then(|s| s.as_str()) {
            workspace = workspace_from_pathish(cwd);
        }
        if session_id != "claude" && workspace != "Claude Code" {
            break;
        }
    }
    (session_id, workspace)
}

fn scan_blocked(base: &Path) -> HashMap<String, ClaudeQuota> {
    let mut active = HashMap::new();
    for path in recent_transcripts(base) {
        let Blocked::Quota { kind, reset } = transcript_limit(&path.to_string_lossy()) else {
            continue;
        };
        let (session_id, workspace) = transcript_meta(&path);
        let quota_reset = format!("{kind}|{}", reset.unwrap_or_default());
        active.insert(
            session_id.clone(),
            ClaudeQuota {
                task_id: session_id,
                workspace,
                transcript_path: path.to_string_lossy().to_string(),
                quota_reset,
            },
        );
    }
    active
}

fn quota_event(block: &ClaudeQuota) -> IncomingEvent {
    IncomingEvent {
        tool_id: "claude_code".to_string(),
        event_type: "task_update".to_string(),
        workspace: Some(block.workspace.clone()),
        cwd: None,
        session_id: Some(block.task_id.clone()),
        task_id: Some(block.task_id.clone()),
        status: Some("paused".to_string()),
        message: Some(String::new()),
        tokens: None,
        transcript_path: Some(block.transcript_path.clone()),
        quota_reset: Some(block.quota_reset.clone()),
        timestamp: None,
    }
}

fn done_event(task_id: &str) -> IncomingEvent {
    IncomingEvent {
        tool_id: "claude_code".to_string(),
        event_type: "task_done".to_string(),
        workspace: Some("Claude Code".to_string()),
        cwd: None,
        session_id: Some(task_id.to_string()),
        task_id: Some(task_id.to_string()),
        status: None,
        message: Some(String::new()),
        tokens: None,
        transcript_path: None,
        quota_reset: None,
        timestamp: None,
    }
}

pub fn start(store: Arc<Mutex<Store>>) {
    let base = claude_projects_dir();
    thread::spawn(move || {
        let mut prev: HashSet<String> = HashSet::new();
        loop {
            let active = scan_blocked(&base);
            let cur: HashSet<String> = active.keys().cloned().collect();
            {
                let mut s = store.lock().unwrap();
                for block in active.values() {
                    // Re-apply while blocked so a long weekly/session limit does not expire from silence.
                    s.apply(quota_event(block));
                }
                for task_id in prev.difference(&cur) {
                    s.apply(done_event(task_id));
                }
            }
            prev = cur;
            thread::sleep(Duration::from_secs(POLL_SECS));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn transcript_meta_reads_session_and_workspace_from_tail() {
        let mut path = std::env::temp_dir();
        path.push(format!("asb_claude_meta_{}.jsonl", std::process::id()));
        let mut f = fs::File::create(&path).unwrap();
        writeln!(
            f,
            r#"{{"sessionId":"s1","cwd":"C:\\Users\\Admin\\proj\\DemoApp","message":{{"usage":{{"output_tokens":1}}}}}}"#
        )
        .unwrap();
        let (session, workspace) = transcript_meta(&path);
        assert_eq!(session, "s1");
        assert_eq!(workspace, "DemoApp");
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn scan_blocked_detects_claude_usage_limit_without_hook() {
        let mut dir = std::env::temp_dir();
        dir.push(format!("asb_claude_projects_{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("session-a.jsonl");
        let mut f = fs::File::create(&path).unwrap();
        writeln!(
            f,
            r#"{{"sessionId":"session-a","cwd":"C:\\work\\LongTodo","isApiErrorMessage":true,"message":"Usage limit reached. Resets at 2:50 PM"}}"#
        )
        .unwrap();
        let active = scan_blocked(&dir);
        let block = active.get("session-a").unwrap();
        assert_eq!(block.workspace, "LongTodo");
        assert_eq!(block.quota_reset, "session|2:50 PM");
        let _ = fs::remove_dir_all(&dir);
    }
}
