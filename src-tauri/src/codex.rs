use crate::store::{IncomingEvent, QuotaUsage, Store};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, SystemTime};

const POLL_SECS: u64 = 5;
// consider a rollout active if written in the last ~10 min: when a turn thinks for a long time or
// runs long commands without writing logs, keep it "running" as long as the tail is still
// task_started (not complete), to avoid false-completion flicker.
const ACTIVE_WINDOW: Duration = Duration::from_secs(600);
const TAIL_BYTES: u64 = 64 * 1024;

/// Determine the current turn status from the rollout tail: take the last relevant event_msg.
/// task_started -> running; turn_aborted -> error (aborted); task_complete -> None (finished).
pub fn turn_status(tail_lines: &[String]) -> Option<&'static str> {
    let mut status = None;
    for line in tail_lines {
        if !line.contains("\"event_msg\"") {
            continue;
        }
        if line.contains("\"task_started\"") {
            status = Some("running");
        } else if line.contains("\"turn_aborted\"") {
            status = Some("error");
        } else if line.contains("\"task_complete\"") {
            status = None;
        }
    }
    status
}

fn workspace_from_cwd(cwd: &str) -> String {
    let trimmed = cwd.trim_end_matches('/');
    trimmed
        .rsplit('/')
        .next()
        .filter(|s| !s.is_empty())
        .unwrap_or("Codex")
        .to_string()
}

fn parse_session_meta(first_line: &str) -> Option<(String, String)> {
    let v: serde_json::Value = serde_json::from_str(first_line).ok()?;
    let p = v.get("payload").unwrap_or(&v);
    let sid = p.get("session_id")?.as_str()?.to_string();
    let cwd = p.get("cwd").and_then(|c| c.as_str()).unwrap_or("").to_string();
    Some((sid, workspace_from_cwd(&cwd)))
}

fn recent_day_dirs(base: &PathBuf) -> Vec<PathBuf> {
    // sessions/YYYY/MM/DD -- take the two lexicographically-largest recent day dirs (today + yesterday across midnight)
    fn largest_children(dir: &PathBuf, n: usize) -> Vec<PathBuf> {
        let mut kids: Vec<PathBuf> = fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .collect();
        kids.sort();
        kids.into_iter().rev().take(n).collect()
    }
    let mut out = Vec::new();
    for year in largest_children(base, 1) {
        for month in largest_children(&year, 1) {
            for day in largest_children(&month, 2) {
                out.push(day);
            }
        }
    }
    out
}

fn recently_modified(path: &PathBuf, window: Duration) -> bool {
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

fn read_first_line(path: &PathBuf) -> Option<String> {
    use std::io::BufRead;
    let f = fs::File::open(path).ok()?;
    std::io::BufReader::new(f).lines().next()?.ok()
}

fn read_tail(path: &PathBuf) -> Vec<String> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = match fs::File::open(path) {
        Ok(f) => f,
        Err(_) => return Vec::new(),
    };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    let start = len.saturating_sub(TAIL_BYTES);
    if f.seek(SeekFrom::Start(start)).is_err() {
        return Vec::new();
    }
    let mut buf = String::new();
    if f.read_to_string(&mut buf).is_err() {
        return Vec::new();
    }
    buf.lines().map(|s| s.to_string()).collect()
}

fn event(tool_id: &str, event_type: &str, task_id: &str, workspace: &str, msg: &str) -> IncomingEvent {
    IncomingEvent {
        tool_id: tool_id.to_string(),
        event_type: event_type.to_string(),
        workspace: Some(workspace.to_string()),
        cwd: None,
        session_id: Some(task_id.to_string()),
        task_id: Some(task_id.to_string()),
        status: None,
        message: Some(msg.to_string()),
        tokens: None,
        transcript_path: None,
        timestamp: None,
    }
}

/// Scan active rollouts once, returning task_id -> (workspace, status).
/// status = "running" or "error" (turn_aborted); finished turns are not returned.
fn scan_active(base: &PathBuf) -> HashMap<String, (String, &'static str)> {
    let mut active = HashMap::new();
    for day in recent_day_dirs(base) {
        let files = match fs::read_dir(&day) {
            Ok(rd) => rd,
            Err(_) => continue,
        };
        for entry in files.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            if !recently_modified(&path, ACTIVE_WINDOW) {
                continue;
            }
            let tail = read_tail(&path);
            let Some(status) = turn_status(&tail) else {
                continue;
            };
            // read only the first line (session_meta); don't read the whole rollout for one line (can be several MB)
            let first = read_first_line(&path);
            if let Some((sid, ws)) = first.as_deref().and_then(parse_session_meta) {
                active.insert(format!("codex-{sid}"), (ws, status));
            }
        }
    }
    active
}

/// Parse the last `token_count` event's `rate_limits` from a rollout tail.
/// primary = 5h window, secondary = weekly window; each has used_percent + resets_at (epoch seconds).
fn parse_rate_limits(tail: &[String]) -> Option<QuotaUsage> {
    for line in tail.iter().rev() {
        if !line.contains("\"rate_limits\"") {
            continue;
        }
        let v: serde_json::Value = serde_json::from_str(line).ok()?;
        // real rollout wraps it as {"type":"event_msg","payload":{"type":"token_count","rate_limits":{..}}};
        // fall back to a top-level rate_limits in case a future format flattens it.
        let rl = v
            .get("payload")
            .and_then(|p| p.get("rate_limits"))
            .or_else(|| v.get("rate_limits"))?;
        let win = |k: &str| -> (f64, i64) {
            rl.get(k)
                .map(|w| {
                    (
                        w.get("used_percent").and_then(|x| x.as_f64()).unwrap_or(0.0),
                        w.get("resets_at").and_then(|x| x.as_i64()).unwrap_or(0),
                    )
                })
                .unwrap_or((0.0, 0))
        };
        let (h5_used, h5_reset) = win("primary");
        let (week_used, week_reset) = win("secondary");
        return Some(QuotaUsage {
            h5_used,
            h5_reset,
            week_used,
            week_reset,
        });
    }
    None
}

/// Read the newest rollout file's tail and parse its latest rate-limit usage.
/// The quota is account-wide, so the most recent reading is the current one.
fn latest_rate_limits(base: &PathBuf) -> Option<QuotaUsage> {
    let mut newest: Option<(SystemTime, PathBuf)> = None;
    for day in recent_day_dirs(base) {
        let Ok(rd) = fs::read_dir(&day) else {
            continue;
        };
        for entry in rd.flatten() {
            let p = entry.path();
            if p.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            if let Ok(mt) = fs::metadata(&p).and_then(|m| m.modified()) {
                if newest.as_ref().map(|(t, _)| mt > *t).unwrap_or(true) {
                    newest = Some((mt, p));
                }
            }
        }
    }
    let (_, path) = newest?;
    parse_rate_limits(&read_tail(&path))
}

pub fn start(store: Arc<Mutex<Store>>) {
    let base = dirs_codex_sessions();
    thread::spawn(move || {
        // task_id -> last reported status, for diffing
        let mut prev: HashMap<String, &'static str> = HashMap::new();
        loop {
            let active = scan_active(&base);
            {
                let mut s = store.lock().unwrap();
                // newly appeared or status changed -> report the corresponding event
                for (id, (ws, status)) in &active {
                    if prev.get(id) != Some(status) {
                        // send only the status semantics; text is localized on the frontend, no hardcoded strings
                        let et = match *status {
                            "error" => "task_error",
                            _ => "task_started",
                        };
                        s.apply(event("codex", et, id, ws, ""));
                    }
                }
                // turns that disappeared (finished) -> done
                for id in prev.keys() {
                    if !active.contains_key(id) {
                        s.apply(event("codex", "task_done", id, "Codex", ""));
                    }
                }
            }
            // report the latest rate-limit usage (5h + weekly) for the quota warning; keep last-known if none
            if let Some(usage) = latest_rate_limits(&base) {
                store.lock().unwrap().set_quota_usage("codex", Some(usage));
            }
            let cur: HashMap<String, &'static str> =
                active.iter().map(|(k, (_, st))| (k.clone(), *st)).collect();
            prev = cur;
            thread::sleep(Duration::from_secs(POLL_SECS));
        }
    });
}

fn dirs_codex_sessions() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_default();
    PathBuf::from(home).join(".codex").join("sessions")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn turn_status_running() {
        let lines = vec![
            r#"{"type":"event_msg","payload":{"type":"task_started","turn_id":"t1"}}"#.to_string(),
            r#"{"type":"response_item","payload":{"type":"custom_tool_call"}}"#.to_string(),
        ];
        assert_eq!(turn_status(&lines), Some("running"));
    }

    #[test]
    fn turn_status_none_after_complete() {
        let lines = vec![
            r#"{"type":"event_msg","payload":{"type":"task_started","turn_id":"t1"}}"#.to_string(),
            r#"{"type":"event_msg","payload":{"type":"task_complete","turn_id":"t1"}}"#.to_string(),
        ];
        assert_eq!(turn_status(&lines), None, "complete -> inactive");
    }

    #[test]
    fn turn_status_error_on_abort() {
        let lines = vec![
            r#"{"type":"event_msg","payload":{"type":"task_started","turn_id":"t1"}}"#.to_string(),
            r#"{"type":"event_msg","payload":{"type":"turn_aborted","turn_id":"t1"}}"#.to_string(),
        ];
        assert_eq!(turn_status(&lines), Some("error"), "turn_aborted -> error");
    }

    #[test]
    fn turn_status_last_wins() {
        let lines = vec![
            r#"{"type":"event_msg","payload":{"type":"turn_aborted","turn_id":"t1"}}"#.to_string(),
            r#"{"type":"event_msg","payload":{"type":"task_started","turn_id":"t2"}}"#.to_string(),
        ];
        assert_eq!(turn_status(&lines), Some("running"), "a new turn overrides");
    }

    #[test]
    fn parse_session_meta_extracts_id_and_workspace() {
        let line = r#"{"type":"session_meta","payload":{"session_id":"abc-123","cwd":"/Users/x/proj/LifeAdminPet"}}"#;
        let (sid, ws) = parse_session_meta(line).unwrap();
        assert_eq!(sid, "abc-123");
        assert_eq!(ws, "LifeAdminPet");
    }

    #[test]
    fn workspace_from_cwd_basename() {
        assert_eq!(workspace_from_cwd("/a/b/SizeKit/"), "SizeKit");
        assert_eq!(workspace_from_cwd("/a/b/SizeKit"), "SizeKit");
        assert_eq!(workspace_from_cwd(""), "Codex");
    }

    #[test]
    fn parse_rate_limits_from_token_count() {
        // real rollout shape: primary = 5h window, secondary = weekly window
        let line = r#"{"timestamp":"2026-07-07T14:16:54","type":"event_msg","payload":{"type":"token_count","info":{},"rate_limits":{"limit_id":"codex","primary":{"used_percent":7.0,"window_minutes":300,"resets_at":1783407231},"secondary":{"used_percent":28.0,"window_minutes":10080,"resets_at":1783881282},"plan_type":"plus"}}}"#.to_string();
        let u = parse_rate_limits(&[line]).unwrap();
        assert_eq!(u.h5_used, 7.0);
        assert_eq!(u.h5_reset, 1783407231);
        assert_eq!(u.week_used, 28.0);
        assert_eq!(u.week_reset, 1783881282);
    }

    #[test]
    fn parse_rate_limits_takes_latest() {
        let lines = vec![
            r#"{"type":"event_msg","payload":{"type":"token_count","rate_limits":{"primary":{"used_percent":1.0,"resets_at":10},"secondary":{"used_percent":2.0,"resets_at":20}}}}"#.to_string(),
            r#"{"type":"response_item","payload":{"type":"custom_tool_call"}}"#.to_string(),
            r#"{"type":"event_msg","payload":{"type":"token_count","rate_limits":{"primary":{"used_percent":90.0,"resets_at":11},"secondary":{"used_percent":95.0,"resets_at":21}}}}"#.to_string(),
        ];
        let u = parse_rate_limits(&lines).unwrap();
        assert_eq!(u.h5_used, 90.0, "the latest reading wins");
        assert_eq!(u.week_used, 95.0);
    }

    #[test]
    fn parse_rate_limits_none_when_absent() {
        let lines = vec![
            r#"{"type":"event_msg","payload":{"type":"task_started","turn_id":"t1"}}"#.to_string(),
        ];
        assert!(parse_rate_limits(&lines).is_none());
    }
}
