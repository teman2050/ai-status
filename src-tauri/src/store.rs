use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::time::{Duration, Instant, SystemTime};

/// done_event is kept server-side for 5s, then cleared.
pub const DONE_TTL: Duration = Duration::from_secs(5);
/// After running goes silent past this TTL it becomes stale (⇄ lost, awaiting recovery) — not done, not removed.
/// During normal work PostToolUse events keep arriving; a long gap usually means the user interrupted
/// (an interrupt in Claude Code doesn't fire the Stop hook) or the session was lost. A new event restores the real status.
pub const RUNNING_STALE_TTL: Duration = Duration::from_secs(300);
/// Only reclaim a task row after 60 min of silence (back to idle, no ✓ — completion is expressed only by task_done).
pub const REMOVE_TTL: Duration = Duration::from_secs(3600);
/// paused (out of quota) is a known-reason pause; reclaim is relaxed to 3 hours (a usage reset can be far off).
pub const PAUSED_REMOVE_TTL: Duration = Duration::from_secs(10800);

/// A transcript file modified within the ttl = the session is still alive (thinking / streaming both write to it).
/// This is a more reliable heartbeat than "tool completion events": alive-but-silent is no longer misjudged as lost.
pub(crate) fn transcript_recent(path: &str, ttl: Duration) -> bool {
    fs::metadata(path)
        .and_then(|m| m.modified())
        .map(|mtime| {
            SystemTime::now()
                .duration_since(mtime)
                .map(|age| age < ttl)
                .unwrap_or(true) // an mtime in the future (clock drift) is treated as just-updated
        })
        .unwrap_or(false)
}

/// Parse the reset time from a limit error line: take what's after "resets" up to "(" or end of line.
/// The 5h limit yields "9:30pm"; the weekly limit yields e.g. "next Monday 9am" (the frontend only counts down clock times).
fn parse_reset_time(line: &str) -> Option<String> {
    let low = line.to_lowercase();
    let idx = low.find("resets")?;
    let after = line.get(idx + 6..)?.trim_start();
    let after = after.strip_prefix("at ").unwrap_or(after).trim_start();
    let end = after
        .find(['(', '"', '\\'])
        .unwrap_or(after.len())
        .min(40);
    let token = after[..end].trim().to_string();
    if token.is_empty() {
        None
    } else {
        Some(token)
    }
}

fn reset_looks_like_clock(reset: Option<&String>) -> bool {
    let Some(reset) = reset else {
        return false;
    };
    let reset = reset.trim().to_ascii_lowercase();
    let mut chars = reset.chars().peekable();
    let mut digits = 0;
    while matches!(chars.peek(), Some(c) if c.is_ascii_digit()) {
        chars.next();
        digits += 1;
    }
    if digits == 0 || digits > 2 {
        return false;
    }
    matches!(chars.peek(), Some(':') | Some('a') | Some('p'))
}

/// The kind of block currently imposed by the API (judged from the transcript tail).
#[derive(Debug, PartialEq)]
pub(crate) enum Blocked {
    /// Usage limit: kind = "weekly"/"session"/"quota" (only labeled when the message says so, otherwise neutral);
    /// reset = the raw reset-time text (may be empty).
    Quota { kind: &'static str, reset: Option<String> },
    Throttled, // transient throttle (429/overloaded), auto-retrying
    Clear,     // not blocked, or recovered
}

/// Scan the transcript tail: take the last relevant signal. A successful response (output_tokens) means recovery.
/// When quota/throttled the session is blocked and hooks stop firing, so the server scans actively, not relying on adapters.
pub(crate) fn transcript_limit(path: &str) -> Blocked {
    use std::io::{Read, Seek, SeekFrom};
    const TAIL: u64 = 128 * 1024;
    let mut f = match fs::File::open(path) {
        Ok(f) => f,
        Err(_) => return Blocked::Clear,
    };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    if f.seek(SeekFrom::Start(len.saturating_sub(TAIL))).is_err() {
        return Blocked::Clear;
    }
    let mut buf = String::new();
    if f.read_to_string(&mut buf).is_err() {
        return Blocked::Clear;
    }
    let mut state = Blocked::Clear;
    for line in buf.lines() {
        let low = line.to_lowercase();
        if line.contains("\"output_tokens\"") {
            state = Blocked::Clear; // a successful response -> recovered
        }
        if !low.replace(' ', "").contains("\"isapierrormessage\":true") {
            continue;
        }
        let throttle = low.contains("temporarily")
            || low.contains("overloaded")
            || low.contains("rate limit")
            || low.contains("429");
        let usage_limit = low.contains("usage limit")
            || low.contains("session limit")
            || low.contains("limit reached")
            || low.contains("quota")
            || line.contains("配额"); // also match Chinese-locale quota errors
        if usage_limit {
            // only label the kind when the message says so; otherwise neutral "quota", don't guess
            let reset = parse_reset_time(line);
            let kind = if low.contains("weekly") || low.contains("this week") {
                "weekly"
            } else if low.contains("session limit") || reset_looks_like_clock(reset.as_ref()) {
                "session"
            } else {
                "quota"
            };
            state = Blocked::Quota { kind, reset };
        } else if throttle {
            state = Blocked::Throttled;
        }
    }
    state
}

#[derive(Debug, Deserialize)]
pub struct IncomingEvent {
    pub tool_id: String,
    pub event_type: String,
    #[serde(default)]
    pub workspace: Option<String>,
    #[allow(dead_code)] // spec field the UI doesn't consume yet; kept for adapter-event compatibility
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub task_id: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub tokens: Option<u64>,
    #[serde(default)]
    pub transcript_path: Option<String>,
    #[serde(default)]
    pub quota_reset: Option<String>,
    #[serde(default)]
    pub timestamp: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ToolView {
    pub tool_id: String,
    pub tool_name: String,
    pub status: String,
    pub updated_at: String,
    /// Quota exhaustion is a tool-level state (the whole account hit its limit, not a single task).
    /// When Some("session|9:30pm"), the frontend hides all this tool's tasks and shows one red quota row.
    pub quota: Option<String>,
    /// Live quota usage (currently Codex only, parsed from rollout rate_limits). The frontend shows a
    /// warning when a window is close to its limit. None when unknown.
    pub quota_usage: Option<QuotaUsage>,
}

/// A tool's rate-limit usage across its windows (Codex: primary = 5h, secondary = weekly).
/// `*_used` is the percent used (0-100); `*_reset` is the epoch-seconds reset time.
#[derive(Debug, Clone, Serialize)]
pub struct QuotaUsage {
    pub h5_used: f64,
    pub h5_reset: i64,
    pub week_used: f64,
    pub week_reset: i64,
}

#[derive(Debug, Clone)]
struct TaskEntry {
    task_id: String,
    tool_id: String,
    session_id: Option<String>,
    workspace: String,
    title: String,
    summary: String,
    visible_status: String,
    tokens: Option<u64>,
    transcript_path: Option<String>,
    quota_reset: Option<String>, // when paused: "session|9:30pm" / "weekly|<reset>"
    updated_at: String,
    started: Instant,
    last_event: Instant,
    done_at: Option<Instant>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TaskView {
    pub task_id: String,
    pub tool_id: String,
    pub workspace: String,
    pub title: String,
    pub summary: String,
    pub visible_status: String,
    pub elapsed_seconds: u64,
    pub tokens: Option<u64>,
    pub quota_reset: Option<String>,
    pub updated_at: String,
}

fn display_name(tool_id: &str) -> String {
    match tool_id {
        "claude_code" => "Claude Code".to_string(),
        "codex" => "Codex".to_string(),
        "cursor" => "Cursor".to_string(),
        other => {
            let mut chars = other.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        }
    }
}

const VALID_STATUSES: [&str; 5] = ["running", "waiting", "error", "done_event", "paused"];

/// Throttle interval for the quota scan (reads the file tail) — heavier than the mtime heartbeat, no need to run every request.
const LIMIT_SCAN_INTERVAL: Duration = Duration::from_secs(5);

pub struct Store {
    tools: HashMap<String, ToolView>,
    sessions: HashMap<String, HashSet<String>>, // tool_id -> active sessions
    tasks: HashMap<String, TaskEntry>,
    network: &'static str, // ok / flaky / down, updated by the net probe thread
    last_limit_scan: Instant,
    quota_usage: HashMap<String, QuotaUsage>, // tool_id -> live window usage (Codex)
}

impl Store {
    pub fn new() -> Self {
        Store {
            tools: HashMap::new(),
            sessions: HashMap::new(),
            tasks: HashMap::new(),
            network: "ok",
            // subtract one interval so the first call scans immediately
            last_limit_scan: Instant::now() - LIMIT_SCAN_INTERVAL,
            quota_usage: HashMap::new(),
        }
    }

    /// Record a tool's live rate-limit usage (called by the Codex rollout watcher). None clears it.
    pub fn set_quota_usage(&mut self, tool_id: &str, usage: Option<QuotaUsage>) {
        match usage {
            Some(u) => {
                self.quota_usage.insert(tool_id.to_string(), u);
            }
            None => {
                self.quota_usage.remove(tool_id);
            }
        }
    }

    fn ensure_tool(&mut self, tool_id: &str, ts: &str) {
        let entry = self
            .tools
            .entry(tool_id.to_string())
            .or_insert_with(|| ToolView {
                tool_id: tool_id.to_string(),
                tool_name: display_name(tool_id),
                status: "connected".to_string(),
                updated_at: ts.to_string(),
                quota: None, // computed dynamically from tasks in tools_snapshot; placeholder here
                quota_usage: None, // filled from the quota_usage map in tools_snapshot
            });
        entry.status = "connected".to_string();
        entry.updated_at = ts.to_string();
    }

    pub fn apply(&mut self, ev: IncomingEvent) {
        let ts = ev.timestamp.clone().unwrap_or_default();
        match ev.event_type.as_str() {
            "tool_connected" => {
                self.ensure_tool(&ev.tool_id, &ts);
                if let Some(sid) = &ev.session_id {
                    self.sessions
                        .entry(ev.tool_id.clone())
                        .or_default()
                        .insert(sid.clone());
                }
            }
            "tool_disconnected" => match &ev.session_id {
                Some(sid) => {
                    let remaining = {
                        let set = self.sessions.entry(ev.tool_id.clone()).or_default();
                        set.remove(sid);
                        set.len()
                    };
                    self.tasks
                        .retain(|_, t| t.session_id.as_deref() != Some(sid.as_str()));
                    if remaining == 0 {
                        self.tools.remove(&ev.tool_id);
                        self.sessions.remove(&ev.tool_id);
                    }
                }
                None => {
                    self.tools.remove(&ev.tool_id);
                    self.sessions.remove(&ev.tool_id);
                    self.tasks.retain(|_, t| t.tool_id != ev.tool_id);
                }
            },
            "task_started" | "task_update" | "task_waiting" | "task_error" | "task_done" => {
                // a task event implies the tool is online (fallback when the adapter forgets tool_connected)
                self.ensure_tool(&ev.tool_id, &ts);
                let Some(key) = ev.task_id.clone().or_else(|| ev.session_id.clone()) else {
                    return; // can't locate the task, drop it
                };
                let task = self.tasks.entry(key.clone()).or_insert_with(|| TaskEntry {
                    task_id: key.clone(),
                    tool_id: ev.tool_id.clone(),
                    session_id: ev.session_id.clone(),
                    workspace: ev.workspace.clone().unwrap_or_default(),
                    title: ev.workspace.clone().unwrap_or_else(|| key.clone()),
                    summary: String::new(),
                    visible_status: "running".to_string(),
                    tokens: None,
                    transcript_path: ev.transcript_path.clone(),
                    quota_reset: None,
                    updated_at: ts.clone(),
                    started: Instant::now(),
                    last_event: Instant::now(),
                    done_at: None,
                });
                if let Some(ws) = &ev.workspace {
                    if !ws.is_empty() {
                        task.workspace = ws.clone();
                    }
                }
                if let Some(msg) = &ev.message {
                    task.summary = msg.clone();
                }
                if let Some(tk) = ev.tokens {
                    task.tokens = Some(tk);
                }
                if ev.transcript_path.is_some() {
                    task.transcript_path = ev.transcript_path.clone();
                }
                if ev.quota_reset.is_some() {
                    task.quota_reset = ev.quota_reset.clone();
                }
                task.updated_at = ts.clone();
                task.last_event = Instant::now();
                match ev.event_type.as_str() {
                    "task_started" => {
                        task.visible_status = "running".to_string();
                        task.started = Instant::now();
                        task.done_at = None;
                    }
                    "task_update" => {
                        if let Some(s) = &ev.status {
                            if VALID_STATUSES.contains(&s.as_str()) {
                                task.visible_status = s.clone();
                            }
                        }
                    }
                    "task_waiting" => task.visible_status = "waiting".to_string(),
                    "task_error" => task.visible_status = "error".to_string(),
                    "task_done" => {
                        task.visible_status = "done_event".to_string();
                        task.done_at = Some(Instant::now());
                    }
                    _ => {}
                }
            }
            _ => {} // ignore unknown event types
        }
    }

    /// Heartbeat + quota detection: scan each task's transcript.
    /// - file written recently -> refresh last_event (alive-but-silent isn't lost)
    /// - an active usage limit in the tail -> flip to paused (no hook needed; hooks don't fire when quota-blocked)
    pub fn refresh_liveness(&mut self) {
        // the quota scan is heavier (reads the file tail), throttled to once every 5s
        let now = Instant::now();
        let do_limit_scan = now.saturating_duration_since(self.last_limit_scan) >= LIMIT_SCAN_INTERVAL;
        if do_limit_scan {
            self.last_limit_scan = now;
        }
        for t in self.tasks.values_mut() {
            if t.done_at.is_some() {
                continue;
            }
            let Some(path) = t.transcript_path.clone() else {
                continue;
            };
            // heartbeat (mtime, lightweight) runs every time
            if transcript_recent(&path, RUNNING_STALE_TTL) {
                t.last_event = now;
            }
            // quota/throttle flip (reads the file, heavier) is throttled; bidirectional: blocked -> paused, cleared -> running
            if do_limit_scan {
                let blocked = transcript_limit(&path);
                // all display text (quota/throttle/recovery) is localized on the frontend by status; the backend sends only
                // status semantics + quota_reset ("kind|reset", for the frontend to compute the countdown / absolute reset time), no hardcoded summary.
                match (t.visible_status.as_str(), &blocked) {
                    ("running" | "waiting" | "paused", Blocked::Quota { kind, reset }) => {
                        let hint = format!("{kind}|{}", reset.clone().unwrap_or_default());
                        t.visible_status = "paused".to_string();
                        t.summary = String::new();
                        t.quota_reset = Some(hint);
                    }
                    ("running" | "waiting" | "paused", Blocked::Throttled) => {
                        t.visible_status = "paused".to_string();
                        t.summary = String::new();
                        t.quota_reset = None;
                    }
                    // paused but cleared and the session is still active -> back to running (don't get stuck)
                    ("paused", Blocked::Clear) if transcript_recent(&path, RUNNING_STALE_TTL) => {
                        t.visible_status = "running".to_string();
                        t.summary = String::new();
                        t.quota_reset = None;
                    }
                    _ => {}
                }
            }
        }
    }

    pub fn purge(&mut self, now: Instant) {
        self.tasks.retain(|_, t| match t.done_at {
            Some(done) => now.saturating_duration_since(done) < DONE_TTL,
            None => true,
        });
        if self.network != "ok" {
            return; // when offline/flaky, events naturally stop; don't misjudge silence as lost
        }
        // remove once silent past the reclaim threshold (error needs user handling, not auto-cleared; paused is a known pause, relaxed to 3h)
        self.tasks.retain(|_, t| {
            let silence = now.saturating_duration_since(t.last_event);
            match t.visible_status.as_str() {
                "error" => true,
                "paused" => silence < PAUSED_REMOVE_TTL,
                _ => silence < REMOVE_TTL,
            }
        });
        // A running task gone silent means the session is idle — waiting for the user's next input.
        // (Claude Code doesn't write the transcript while sitting at a prompt / a question.) So show
        // it as waiting, not "lost": if the process had actually died, the watcher would remove the
        // tool. paused is a known-reason pause (quota), left as-is.
        for t in self.tasks.values_mut() {
            if t.visible_status == "running"
                && now.saturating_duration_since(t.last_event) >= RUNNING_STALE_TTL
            {
                t.visible_status = "waiting".to_string();
                t.summary = String::new();
            }
        }
    }

    /// Menu-bar progress: take the top `max` task statuses by priority (for drawing the tray rings).
    /// A quota-exhausted tool collapses into a single "quota" (red ring); its tasks aren't expanded — matching the widget.
    pub fn menubar_top_statuses(&self, max: usize) -> Vec<String> {
        const ORDER: [&str; 7] = [
            "error", "quota", "waiting", "paused", "running", "stale", "done_event",
        ];
        let quota_tools: HashSet<String> = self.quota_by_tool().into_keys().collect();
        let mut v: Vec<String> = quota_tools.iter().map(|_| "quota".to_string()).collect();
        for t in self.tasks.values() {
            if quota_tools.contains(&t.tool_id) {
                continue; // a quota tool's tasks fold into the single quota ring above
            }
            v.push(t.visible_status.clone());
        }
        v.sort_by_key(|s| ORDER.iter().position(|o| *o == s.as_str()).unwrap_or(99));
        v.truncate(max);
        v
    }

    pub fn set_network(&mut self, status: &'static str) {
        self.network = status;
    }

    pub fn network(&self) -> &'static str {
        self.network
    }

    /// Tool-level quota: any session paused with a quota_reset (a real quota, not a transient throttle) -> the whole tool is exhausted.
    /// Multiple sessions share one account's limit, so taking the first matching reset hint is enough.
    fn quota_by_tool(&self) -> HashMap<String, String> {
        let mut q: HashMap<String, String> = HashMap::new();
        for t in self.tasks.values() {
            if t.visible_status == "paused" {
                if let Some(hint) = &t.quota_reset {
                    q.entry(t.tool_id.clone()).or_insert_with(|| hint.clone());
                }
            }
        }
        q
    }

    pub fn tools_snapshot(&self) -> Vec<ToolView> {
        let quota = self.quota_by_tool();
        let mut v: Vec<ToolView> = self
            .tools
            .values()
            .cloned()
            .map(|mut tv| {
                tv.quota = quota.get(&tv.tool_id).cloned();
                tv.quota_usage = self.quota_usage.get(&tv.tool_id).cloned();
                tv
            })
            .collect();
        v.sort_by(|a, b| a.tool_id.cmp(&b.tool_id));
        v
    }

    pub fn tasks_snapshot(&self, now: Instant) -> Vec<TaskView> {
        let mut entries: Vec<&TaskEntry> = self.tasks.values().collect();
        entries.sort_by(|a, b| a.tool_id.cmp(&b.tool_id).then(a.started.cmp(&b.started)));
        entries
            .iter()
            .map(|t| TaskView {
                task_id: t.task_id.clone(),
                tool_id: t.tool_id.clone(),
                workspace: t.workspace.clone(),
                title: t.title.clone(),
                summary: t.summary.clone(),
                visible_status: t.visible_status.clone(),
                elapsed_seconds: now.saturating_duration_since(t.started).as_secs(),
                tokens: t.tokens,
                quota_reset: t.quota_reset.clone(),
                updated_at: t.updated_at.clone(),
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(event_type: &str, task_id: Option<&str>) -> IncomingEvent {
        IncomingEvent {
            tool_id: "codex".to_string(),
            event_type: event_type.to_string(),
            workspace: Some("LifeAdminPet".to_string()),
            cwd: None,
            session_id: None,
            task_id: task_id.map(|s| s.to_string()),
            status: None,
            message: Some("running xcodebuild".to_string()),
            tokens: None,
            transcript_path: None,
            quota_reset: None,
            timestamp: Some("2026-07-05T10:00:00+09:00".to_string()),
        }
    }

    #[test]
    fn tool_connected_creates_connected_tool() {
        let mut s = Store::new();
        s.apply(ev("tool_connected", None));
        let tools = s.tools_snapshot();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].tool_id, "codex");
        assert_eq!(tools[0].tool_name, "Codex");
        assert_eq!(tools[0].status, "connected");
    }

    #[test]
    fn task_started_creates_running_task() {
        let mut s = Store::new();
        s.apply(ev("task_started", Some("t1")));
        let tasks = s.tasks_snapshot(Instant::now());
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].visible_status, "running");
        assert_eq!(tasks[0].workspace, "LifeAdminPet");
        assert_eq!(tasks[0].summary, "running xcodebuild");
    }

    #[test]
    fn waiting_error_done_transitions() {
        let mut s = Store::new();
        s.apply(ev("task_started", Some("t1")));
        s.apply(ev("task_waiting", Some("t1")));
        assert_eq!(s.tasks_snapshot(Instant::now())[0].visible_status, "waiting");
        s.apply(ev("task_error", Some("t1")));
        assert_eq!(s.tasks_snapshot(Instant::now())[0].visible_status, "error");
        s.apply(ev("task_done", Some("t1")));
        assert_eq!(s.tasks_snapshot(Instant::now())[0].visible_status, "done_event");
    }

    #[test]
    fn done_task_purged_after_ttl() {
        let mut s = Store::new();
        s.apply(ev("task_started", Some("t1")));
        s.apply(ev("task_done", Some("t1")));
        let now = Instant::now();
        s.purge(now);
        assert_eq!(s.tasks_snapshot(now).len(), 1, "kept within TTL");
        let later = now + Duration::from_secs(6);
        s.purge(later);
        assert!(s.tasks_snapshot(later).is_empty(), "cleared past TTL");
    }

    #[test]
    fn disconnect_removes_tool_and_its_tasks() {
        let mut s = Store::new();
        s.apply(ev("task_started", Some("t1")));
        s.apply(ev("tool_disconnected", None));
        assert!(s.tools_snapshot().is_empty());
        assert!(s.tasks_snapshot(Instant::now()).is_empty());
    }

    #[test]
    fn task_event_auto_connects_tool() {
        let mut s = Store::new();
        s.apply(ev("task_started", Some("t1")));
        assert_eq!(s.tools_snapshot().len(), 1);
    }

    fn ev_session(event_type: &str, session: &str) -> IncomingEvent {
        IncomingEvent {
            tool_id: "claude_code".to_string(),
            event_type: event_type.to_string(),
            workspace: Some("SizeKit".to_string()),
            cwd: None,
            session_id: Some(session.to_string()),
            task_id: Some(session.to_string()),
            status: None,
            message: None,
            tokens: None,
            transcript_path: None,
            quota_reset: None,
            timestamp: None,
        }
    }

    #[test]
    fn tool_stays_online_until_last_session_ends() {
        let mut s = Store::new();
        s.apply(ev_session("tool_connected", "s1"));
        s.apply(ev_session("tool_connected", "s2"));
        s.apply(ev_session("tool_disconnected", "s1"));
        assert_eq!(s.tools_snapshot().len(), 1, "s2 remains, tool should stay online");
        s.apply(ev_session("tool_disconnected", "s2"));
        assert!(s.tools_snapshot().is_empty(), "last session ended, tool disappears");
    }

    #[test]
    fn session_disconnect_removes_only_its_tasks() {
        let mut s = Store::new();
        s.apply(ev_session("tool_connected", "s1"));
        s.apply(ev_session("tool_connected", "s2"));
        s.apply(ev_session("task_started", "s1"));
        s.apply(ev_session("task_started", "s2"));
        s.apply(ev_session("tool_disconnected", "s1"));
        let tasks = s.tasks_snapshot(Instant::now());
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].task_id, "s2");
    }

    #[test]
    fn disconnect_without_session_removes_everything() {
        let mut s = Store::new();
        s.apply(ev_session("tool_connected", "s1"));
        s.apply(ev("task_started", Some("t1"))); // codex has no session
        s.apply(IncomingEvent {
            tool_id: "claude_code".to_string(),
            event_type: "tool_disconnected".to_string(),
            workspace: None,
            cwd: None,
            session_id: None,
            task_id: None,
            status: None,
            message: None,
            tokens: None,
            transcript_path: None,
            quota_reset: None,
            timestamp: None,
        });
        assert_eq!(s.tools_snapshot().len(), 1, "only codex remains");
    }

    #[test]
    fn task_started_resets_done_state() {
        let mut s = Store::new();
        s.apply(ev("task_started", Some("t1")));
        s.apply(ev("task_done", Some("t1")));
        s.apply(ev("task_started", Some("t1"))); // same session, second prompt
        assert_eq!(s.tasks_snapshot(Instant::now())[0].visible_status, "running");
        let later = Instant::now() + Duration::from_secs(10);
        s.purge(later);
        assert_eq!(s.tasks_snapshot(later).len(), 1, "done_at cleared, should not be purged");
    }

    #[test]
    fn silent_running_becomes_waiting_not_removed() {
        let mut s = Store::new();
        s.apply(ev("task_started", Some("t1")));
        let now = Instant::now();
        s.purge(now + Duration::from_secs(200));
        assert_eq!(s.tasks_snapshot(now)[0].visible_status, "running", "stays running within 5 min");
        s.purge(now + Duration::from_secs(301));
        let tasks = s.tasks_snapshot(now);
        assert_eq!(tasks.len(), 1, "a silent session isn't gone");
        assert_eq!(tasks[0].visible_status, "waiting", "silent running -> waiting for the user, not lost");
        assert_eq!(tasks[0].summary, "", "text localized on frontend, backend summary empty");
    }

    #[test]
    fn waiting_stays_waiting_when_silent() {
        let mut s = Store::new();
        s.apply(ev("task_started", Some("t1")));
        s.apply(ev("task_waiting", Some("t1")));
        let now = Instant::now();
        s.purge(now + Duration::from_secs(301));
        assert_eq!(s.tasks_snapshot(now)[0].visible_status, "waiting");
        s.purge(now + Duration::from_secs(1801));
        assert_eq!(s.tasks_snapshot(now)[0].visible_status, "waiting", "a silent waiting task stays waiting, not lost");
    }

    #[test]
    fn idle_removed_only_after_remove_ttl() {
        let mut s = Store::new();
        s.apply(ev("task_started", Some("t1")));
        let now = Instant::now();
        s.purge(now + Duration::from_secs(1800));
        assert_eq!(s.tasks_snapshot(now).len(), 1, "idle row kept up to 60 min");
        s.purge(now + Duration::from_secs(3601));
        assert!(s.tasks_snapshot(now).is_empty(), "reclaimed after 60 min of silence");
    }

    #[test]
    fn network_down_freezes_stale_detection() {
        let mut s = Store::new();
        s.apply(ev("task_started", Some("t1")));
        s.set_network("down");
        let now = Instant::now();
        s.purge(now + Duration::from_secs(3601));
        assert_eq!(s.tasks_snapshot(now)[0].visible_status, "running", "while offline, don't mark stale or reclaim");
    }

    #[test]
    fn error_task_never_staled() {
        let mut s = Store::new();
        s.apply(ev("task_started", Some("t1")));
        s.apply(ev("task_error", Some("t1")));
        let now = Instant::now();
        s.purge(now + Duration::from_secs(86_400));
        assert_eq!(s.tasks_snapshot(now)[0].visible_status, "error", "error needs user handling, not auto-cleared");
    }

    #[test]
    fn paused_not_degraded_to_stale() {
        let mut s = Store::new();
        s.apply(ev("task_started", Some("t1")));
        let mut u = ev("task_update", Some("t1"));
        u.status = Some("paused".to_string());
        s.apply(u);
        let now = Instant::now();
        s.purge(now + Duration::from_secs(3600));
        assert_eq!(s.tasks_snapshot(now)[0].visible_status, "paused", "quota pause not degraded to stale");
        s.purge(now + Duration::from_secs(10801));
        assert!(s.tasks_snapshot(now).is_empty(), "paused reclaimed only after 3 hours");
    }

    #[test]
    fn fresh_transcript_prevents_stale() {
        use std::io::Write;
        let mut path = std::env::temp_dir();
        path.push(format!("asb_test_{}.jsonl", std::process::id()));
        let mut f = std::fs::File::create(&path).unwrap();
        writeln!(f, "{{}}").unwrap();
        let p = path.to_string_lossy().to_string();

        let mut s = Store::new();
        let mut started = ev("task_started", Some("t1"));
        started.transcript_path = Some(p.clone());
        s.apply(started);
        // simulate 400s of silence (past the running threshold)
        for t in s.tasks.values_mut() {
            t.last_event = Instant::now() - Duration::from_secs(400);
        }
        // without a heartbeat, a silent running task becomes waiting (idle, awaiting the user)
        {
            let mut s2 = Store::new();
            let started2 = ev("task_started", Some("t2"));
            s2.apply(started2);
            for t in s2.tasks.values_mut() {
                t.last_event = Instant::now() - Duration::from_secs(400);
            }
            s2.purge(Instant::now());
            assert_eq!(s2.tasks_snapshot(Instant::now())[0].visible_status, "waiting");
        }
        // transcript just written; refresh_liveness pulls last_event back to now, so purge won't mark stale
        s.refresh_liveness();
        s.purge(Instant::now());
        assert_eq!(
            s.tasks_snapshot(Instant::now())[0].visible_status,
            "running",
            "an active transcript should not go stale"
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn quota_limit_flips_running_to_paused() {
        use std::io::Write;
        let mut path = std::env::temp_dir();
        path.push(format!("asb_limit_{}.jsonl", std::process::id()));
        let mut f = std::fs::File::create(&path).unwrap();
        writeln!(f, "{{\"message\":{{\"usage\":{{\"output_tokens\":5}}}}}}").unwrap();
        writeln!(
            f,
            "{{\"isApiErrorMessage\":true,\"message\":{{\"content\":\"session limit reached\"}}}}"
        )
        .unwrap();
        let p = path.to_string_lossy().to_string();

        let mut s = Store::new();
        let mut started = ev("task_started", Some("t1"));
        started.transcript_path = Some(p.clone());
        s.apply(started);
        assert_eq!(s.tasks_snapshot(Instant::now())[0].visible_status, "running");
        s.refresh_liveness(); // the server scans and finds the active limit
        assert_eq!(
            s.tasks_snapshot(Instant::now())[0].visible_status,
            "paused",
            "a quota block (no hook) should still flip to paused"
        );
        // quota is tool-level: tools_snapshot marks the tool with a quota hint, the menu bar folds into one red ring
        let tools = s.tools_snapshot();
        assert_eq!(tools.len(), 1);
        assert_eq!(
            tools[0].quota.as_deref(),
            Some("session|"),
            "tool-level quota hint should carry the kind (session here; reset raw text isn't a pure clock, so empty)"
        );
        assert_eq!(
            s.menubar_top_statuses(4),
            vec!["quota".to_string()],
            "a quota tool folds into a single quota ring in the menu bar, not per-task"
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn transient_429_not_treated_as_quota() {
        use std::io::Write;
        let mut path = std::env::temp_dir();
        path.push(format!("asb_429_{}.jsonl", std::process::id()));
        let mut f = std::fs::File::create(&path).unwrap();
        writeln!(
            f,
            "{{\"isApiErrorMessage\":true,\"message\":\"Server is temporarily limiting requests (429)\"}}"
        )
        .unwrap();
        let p = path.to_string_lossy().to_string();
        assert_eq!(
            transcript_limit(&p),
            Blocked::Throttled,
            "transient throttle -> Throttled, not quota"
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn transcript_limit_recovers_after_success() {
        use std::io::Write;
        let mut path = std::env::temp_dir();
        path.push(format!("asb_limrec_{}.jsonl", std::process::id()));
        let mut f = std::fs::File::create(&path).unwrap();
        writeln!(f, "{{\"isApiErrorMessage\":true,\"message\":\"usage limit\"}}").unwrap();
        writeln!(f, "{{\"message\":{{\"usage\":{{\"output_tokens\":9}}}}}}").unwrap();
        let p = path.to_string_lossy().to_string();
        assert_eq!(transcript_limit(&p), Blocked::Clear, "a success after a limit -> recovered");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn transcript_limit_parses_reset_time() {
        use std::io::Write;
        let mut path = std::env::temp_dir();
        path.push(format!("asb_reset_{}.jsonl", std::process::id()));
        let mut f = std::fs::File::create(&path).unwrap();
        writeln!(
            f,
            "{{\"isApiErrorMessage\":true,\"message\":\"You've hit your session limit · resets 9:30pm (Asia/Tokyo)\"}}"
        )
        .unwrap();
        let p = path.to_string_lossy().to_string();
        assert_eq!(
            transcript_limit(&p),
            Blocked::Quota {
                kind: "session",
                reset: Some("9:30pm".to_string())
            }
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn transcript_limit_detects_weekly() {
        use std::io::Write;
        let mut path = std::env::temp_dir();
        path.push(format!("asb_weekly_{}.jsonl", std::process::id()));
        let mut f = std::fs::File::create(&path).unwrap();
        writeln!(
            f,
            "{{\"isApiErrorMessage\":true,\"message\":\"You've reached your weekly usage limit · resets Monday 9am\"}}"
        )
        .unwrap();
        let p = path.to_string_lossy().to_string();
        assert_eq!(
            transcript_limit(&p),
            Blocked::Quota {
                kind: "weekly",
                reset: Some("Monday 9am".to_string())
            }
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn transcript_limit_generic_when_type_unclear() {
        use std::io::Write;
        let mut path = std::env::temp_dir();
        path.push(format!("asb_generic_{}.jsonl", std::process::id()));
        let mut f = std::fs::File::create(&path).unwrap();
        writeln!(
            f,
            "{{\"isApiErrorMessage\":true,\"message\":\"Usage limit reached\"}}"
        )
        .unwrap();
        let p = path.to_string_lossy().to_string();
        assert_eq!(
            transcript_limit(&p),
            Blocked::Quota { kind: "quota", reset: None },
            "kind unstated -> neutral quota, don't guess 5h/weekly"
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn transcript_limit_clock_reset_implies_session_limit() {
        use std::io::Write;
        let mut path = std::env::temp_dir();
        path.push(format!("asb_clock_reset_{}.jsonl", std::process::id()));
        let mut f = std::fs::File::create(&path).unwrap();
        writeln!(
            f,
            "{{\"isApiErrorMessage\":true,\"message\":\"Usage limit reached. Resets at 2:50 PM\"}}"
        )
        .unwrap();
        let p = path.to_string_lossy().to_string();
        assert_eq!(
            transcript_limit(&p),
            Blocked::Quota {
                kind: "session",
                reset: Some("2:50 PM".to_string())
            },
            "Claude's current usage-limit text has no explicit session word, but a clock reset means the 5h/session window"
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn transcript_recent_helper() {
        use std::io::Write;
        let mut path = std::env::temp_dir();
        path.push(format!("asb_recent_{}.jsonl", std::process::id()));
        let mut f = std::fs::File::create(&path).unwrap();
        writeln!(f, "x").unwrap();
        let p = path.to_string_lossy().to_string();
        assert!(transcript_recent(&p, Duration::from_secs(300)), "a just-written file counts as active");
        assert!(!transcript_recent("/no/such/file.jsonl", Duration::from_secs(300)));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn event_revives_idle_task() {
        let mut s = Store::new();
        s.apply(ev("task_started", Some("t1")));
        let now = Instant::now();
        s.purge(now + Duration::from_secs(301));
        assert_eq!(s.tasks_snapshot(now)[0].visible_status, "waiting");
        let mut update = ev("task_update", Some("t1"));
        update.status = Some("running".to_string());
        s.apply(update);
        assert_eq!(s.tasks_snapshot(Instant::now())[0].visible_status, "running", "a new event restores the real status");
    }

    #[test]
    fn menubar_top_statuses_by_priority_and_capped() {
        let mut s = Store::new();
        s.apply(ev("task_started", Some("a"))); // running
        s.apply(ev("task_started", Some("b")));
        s.apply(ev("task_error", Some("b"))); // error
        s.apply(ev("task_started", Some("c")));
        s.apply(ev("task_waiting", Some("c"))); // waiting
        let top = s.menubar_top_statuses(2);
        assert_eq!(top.len(), 2, "at most 2");
        assert_eq!(top[0], "error", "error has the highest priority");
        assert_eq!(top[1], "waiting", "then waiting");
    }

    #[test]
    fn paused_status_and_tokens_via_task_update() {
        let mut s = Store::new();
        s.apply(ev("task_started", Some("t1")));
        let mut update = ev("task_update", Some("t1"));
        update.status = Some("paused".to_string());
        update.tokens = Some(371_000);
        s.apply(update);
        let tasks = s.tasks_snapshot(Instant::now());
        assert_eq!(tasks[0].visible_status, "paused");
        assert_eq!(tasks[0].tokens, Some(371_000));
    }

    #[test]
    fn multiple_tasks_per_tool_stay_independent() {
        let mut s = Store::new();
        s.apply(ev("task_started", Some("t1")));
        s.apply(ev("task_started", Some("t2")));
        s.apply(ev("task_error", Some("t2")));
        let tasks = s.tasks_snapshot(Instant::now());
        assert_eq!(tasks.len(), 2);
        assert_eq!(
            tasks.iter().filter(|t| t.visible_status == "running").count(),
            1
        );
        assert_eq!(
            tasks.iter().filter(|t| t.visible_status == "error").count(),
            1
        );
    }
}
