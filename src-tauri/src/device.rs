//! LAN device API for external hardware displays (ESP32 status screens etc.).
//!
//! Serves a compact, small-screen-friendly summary on `0.0.0.0:<device_api_port>`
//! plus a UDP discovery responder on 7789 so devices find this PC without a
//! hardcoded IP (DHCP-safe). Protocol spec: docs/device-api.md (v1, identical
//! to the external aistatusplus bridge, so existing device firmware works
//! unchanged).
//!
//! Off by default (`device_api` in config). The loopback adapter API on 7799
//! is untouched: this is a separate listener, and the only LAN-facing surface.

use crate::server::{json_response, Resp};
use crate::store::Store;
use std::net::UdpSocket;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tiny_http::{Method, Request, Server};

const PROTOCOL_VERSION: u64 = 1;
const DISCOVERY_MAGIC: &[u8] = b"AISP_DISCOVER v1";
const DISCOVERY_PORT: u16 = 7789;

// Device screens are small; truncate server-side so firmware stays dumb.
const TXT_CHARS: usize = 40;
const WS_CHARS: usize = 16;
const QUOTA_CHARS: usize = 12;
const MAX_TOOLS: usize = 6;
const MAX_TASKS_PER_TOOL: usize = 6;

struct Running {
    stop: Arc<AtomicBool>,
    http: Arc<Server>,
    threads: Vec<JoinHandle<()>>,
    port: u16,
    token: String,
}

static RUNNING: Mutex<Option<Running>> = Mutex::new(None);

/// Start/stop/reconfigure the device API to match config. Safe to call at
/// startup and again on every config change; restarts listeners only when
/// port/token actually changed.
pub fn apply(store: Arc<Mutex<Store>>, enabled: bool, port: u16, token: &str) {
    let mut guard = RUNNING.lock().unwrap();
    if let Some(r) = guard.as_ref() {
        if enabled && r.port == port && r.token == token {
            return;
        }
    }
    if let Some(r) = guard.take() {
        let Running {
            stop,
            http,
            threads,
            port: old_port,
            ..
        } = r;
        stop.store(true, Ordering::SeqCst);
        http.unblock();
        for t in threads {
            let _ = t.join();
        }
        drop(http);
        // tiny_http closes its listener asynchronously; wait until the port
        // actually refuses connections so an immediate rebind can't race the
        // dying server (which would eat requests and answer 500).
        for _ in 0..100 {
            match std::net::TcpStream::connect(("127.0.0.1", old_port)) {
                Ok(_) => std::thread::sleep(Duration::from_millis(20)),
                Err(_) => break,
            }
        }
    }
    if !enabled {
        return;
    }

    let server = match Server::http(("0.0.0.0", port)) {
        Ok(s) => Arc::new(s),
        Err(e) => {
            eprintln!("[ai-status] device API failed to start (port {port}): {e}");
            return;
        }
    };
    let stop = Arc::new(AtomicBool::new(false));
    let mut threads = Vec::new();
    {
        let http = server.clone();
        let stop_http = stop.clone();
        let tok = token.to_string();
        threads.push(std::thread::spawn(move || {
            for request in http.incoming_requests() {
                if stop_http.load(Ordering::SeqCst) {
                    break;
                }
                let resp = handle(&request, &store, &tok);
                let _ = request.respond(resp);
            }
        }));
    }
    {
        let stop_udp = stop.clone();
        threads.push(std::thread::spawn(move || discovery_loop(stop_udp, port)));
    }
    *guard = Some(Running {
        stop,
        http: server,
        threads,
        port,
        token: token.to_string(),
    });
    eprintln!("[ai-status] device API on 0.0.0.0:{port} (discovery on UDP {DISCOVERY_PORT})");
}

/// Answers device pairing broadcasts: the device learns this PC's IP from the
/// reply packet's source address, so nothing is ever hardcoded.
fn discovery_loop(stop: Arc<AtomicBool>, http_port: u16) {
    let sock = match UdpSocket::bind(("0.0.0.0", DISCOVERY_PORT)) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("[ai-status] device discovery failed to bind (UDP {DISCOVERY_PORT}): {e}");
            return;
        }
    };
    // Short read timeout so the stop flag is honored promptly.
    let _ = sock.set_read_timeout(Some(Duration::from_millis(500)));
    let name = std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "ai-status".to_string());
    // "app" identifies the protocol family; device firmware filters on it.
    let reply = serde_json::json!({
        "app": "aistatusplus", "v": PROTOCOL_VERSION, "port": http_port, "name": name,
    })
    .to_string();
    let mut buf = [0u8; 64];
    while !stop.load(Ordering::SeqCst) {
        if let Ok((n, addr)) = sock.recv_from(&mut buf) {
            if buf[..n].starts_with(DISCOVERY_MAGIC) {
                let _ = sock.send_to(reply.as_bytes(), addr);
            }
        }
    }
}

fn handle(req: &Request, store: &Arc<Mutex<Store>>, token: &str) -> Resp {
    let method = req.method().clone();
    let url = req.url().to_string();
    let path = url.split('?').next().unwrap_or("");
    if method == Method::Options {
        return json_response(204, String::new());
    }
    match (method, path) {
        (Method::Get, "/api/ping") => json_response(
            200,
            format!(r#"{{"ok":true,"app":"aistatusplus","v":{PROTOCOL_VERSION}}}"#),
        ),
        (Method::Get, "/api/device/summary") => {
            let header = req
                .headers()
                .iter()
                .find(|h| h.field.equiv("X-Token"))
                .map(|h| h.value.as_str().to_string());
            if !token_ok(&url, header.as_deref(), token) {
                return json_response(401, r#"{"ok":false,"error":"unauthorized"}"#.to_string());
            }
            let mut summary = build_summary(store);
            summary["ts"] = serde_json::json!(SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0));
            json_response(200, summary.to_string())
        }
        _ => json_response(404, r#"{"ok":false,"error":"not_found"}"#.to_string()),
    }
}

fn token_ok(url: &str, header: Option<&str>, token: &str) -> bool {
    if token.is_empty() {
        return true;
    }
    let query = url.split('?').nth(1).unwrap_or("");
    if query
        .split('&')
        .any(|kv| kv.strip_prefix("token=") == Some(token))
    {
        return true;
    }
    header == Some(token)
}

/// Char-count truncation with an ellipsis (device buffers are byte-limited;
/// the firmware assumes at most `max` chars here).
fn clip(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max - 1).collect();
    out.push('…');
    out
}

/// Desktop visible_status -> device status enum.
fn map_status(s: &str) -> &'static str {
    match s {
        "running" => "RUN",
        "paused" => "PAUSE",
        "error" => "ERR",
        "done_event" => "DONE",
        _ => "WAIT", // waiting + anything unknown: safest to ask for the user's attention
    }
}

/// Tool-level quota is "kind|reset-hint"; devices only need the hint.
fn quota_hint(q: &str) -> String {
    clip(q.splitn(2, '|').nth(1).unwrap_or(q), QUOTA_CHARS)
}

/// Content signature over the stable fields only (no elapsed minutes, tokens,
/// usage percents or timestamps), so devices can skip redraws — and screen
/// flicker — whenever nothing meaningful changed.
fn rev_of(canon: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325; // FNV-1a
    for b in canon.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{:08x}", (h ^ (h >> 32)) as u32)
}

pub fn build_summary(store: &Arc<Mutex<Store>>) -> serde_json::Value {
    let now = Instant::now();
    let (tools, tasks, network) = {
        let mut s = store.lock().unwrap();
        s.refresh_liveness();
        s.purge(now);
        (s.tools_snapshot(), s.tasks_snapshot(now), s.network())
    };

    let mut tools_out = Vec::new();
    let mut canon = String::new();
    for tool in tools.iter().take(MAX_TOOLS) {
        let mut tasks_out = Vec::new();
        for task in tasks
            .iter()
            .filter(|t| t.tool_id == tool.tool_id)
            .take(MAX_TASKS_PER_TOOL)
        {
            let st = map_status(&task.visible_status);
            let txt = clip(&task.title, TXT_CHARS);
            let sub = clip(&task.summary, TXT_CHARS);
            let ws = clip(&task.workspace, WS_CHARS);
            canon.push_str(&format!("{st}|{txt}|{sub}|{ws};"));
            let mut entry = serde_json::json!({
                "st": st, "txt": txt, "sub": sub, "ws": ws,
                "min": task.elapsed_seconds / 60,
            });
            if let Some(tok) = task.tokens {
                entry["tok"] = serde_json::json!(tok);
            }
            tasks_out.push(entry);
        }

        // Same ordering the menubar uses: error > waiting > paused > running > done.
        let tool_st = ["ERR", "WAIT", "PAUSE", "RUN", "DONE"]
            .iter()
            .find(|want| tasks_out.iter().any(|t| t["st"] == ***want))
            .copied()
            .unwrap_or("IDLE");
        let quota = tool.quota.as_deref().map(quota_hint);
        canon.push_str(&format!("{}#{}#{:?};", tool.tool_id, tool_st, quota));

        let mut entry = serde_json::json!({
            "id": tool.tool_id,
            "name": tool.tool_name,
            "st": tool_st,
            "quota": quota,
            "tasks": tasks_out,
        });
        if let Some(u) = &tool.quota_usage {
            entry["use"] = serde_json::json!({
                "h5": u.h5_used.round() as i64,
                "wk": u.week_used.round() as i64,
            });
        }
        tools_out.push(entry);
    }

    serde_json::json!({
        "ok": true,
        "v": PROTOCOL_VERSION,
        "rev": rev_of(&canon),
        "net": network,
        "tools": tools_out,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::IncomingEvent;

    fn ev(tool: &str, event_type: &str, task_id: Option<&str>) -> IncomingEvent {
        IncomingEvent {
            tool_id: tool.to_string(),
            event_type: event_type.to_string(),
            workspace: Some("aistatus".to_string()),
            cwd: None,
            session_id: None,
            task_id: task_id.map(|s| s.to_string()),
            status: None,
            message: Some("editing server.rs".to_string()),
            tokens: None,
            transcript_path: None,
            quota_reset: None,
            timestamp: Some("2026-07-12T10:00:00+08:00".to_string()),
        }
    }

    fn store_with_events(events: Vec<IncomingEvent>) -> Arc<Mutex<Store>> {
        let mut s = Store::new();
        for e in events {
            s.apply(e);
        }
        Arc::new(Mutex::new(s))
    }

    #[test]
    fn summary_shape_and_status_mapping() {
        let store = store_with_events(vec![
            ev("claude_code", "task_started", Some("t1")),
            ev("claude_code", "task_waiting", Some("t2")),
            ev("codex", "tool_connected", None),
        ]);
        let v = build_summary(&store);
        assert_eq!(v["ok"], true);
        assert_eq!(v["v"], 1);
        assert_eq!(v["rev"].as_str().unwrap().len(), 8);

        let tools = v["tools"].as_array().unwrap();
        let claude = tools.iter().find(|t| t["id"] == "claude_code").unwrap();
        // waiting outranks running at the tool level (user attention first)
        assert_eq!(claude["st"], "WAIT");
        let sts: Vec<&str> = claude["tasks"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["st"].as_str().unwrap())
            .collect();
        assert!(sts.contains(&"RUN") && sts.contains(&"WAIT"));

        let codex = tools.iter().find(|t| t["id"] == "codex").unwrap();
        assert_eq!(codex["st"], "IDLE");
        assert_eq!(codex["tasks"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn rev_stable_until_content_changes() {
        let store = store_with_events(vec![ev("claude_code", "task_started", Some("t1"))]);
        let rev1 = build_summary(&store)["rev"].as_str().unwrap().to_string();
        let rev2 = build_summary(&store)["rev"].as_str().unwrap().to_string();
        assert_eq!(rev1, rev2, "same content must keep the same rev");

        store
            .lock()
            .unwrap()
            .apply(ev("claude_code", "task_waiting", Some("t1")));
        let rev3 = build_summary(&store)["rev"].as_str().unwrap().to_string();
        assert_ne!(rev1, rev3, "status change must change the rev");
    }

    #[test]
    fn clip_is_char_based_and_utf8_safe() {
        assert_eq!(clip("short", 40), "short");
        let long = "这是一个非常长的中文任务标题用来验证截断逻辑是否按字符数正确工作没有任何问题abcdefgh";
        let clipped = clip(long, 40);
        assert_eq!(clipped.chars().count(), 40);
        assert!(clipped.ends_with('…'));
    }

    #[test]
    fn quota_hint_takes_reset_part() {
        assert_eq!(quota_hint("session|9:30pm"), "9:30pm");
        assert_eq!(quota_hint("9:30pm"), "9:30pm");
    }

    #[test]
    fn token_check() {
        assert!(token_ok("/api/device/summary", None, ""));
        assert!(token_ok("/api/device/summary?token=abc", None, "abc"));
        assert!(token_ok("/api/device/summary?x=1&token=abc", None, "abc"));
        assert!(token_ok("/api/device/summary", Some("abc"), "abc"));
        assert!(!token_ok("/api/device/summary", None, "abc"));
        assert!(!token_ok("/api/device/summary?token=wrong", Some("nope"), "abc"));
    }

    fn http_get(addr: &str, path: &str) -> (u16, String) {
        use std::io::{Read, Write};
        let mut s = std::net::TcpStream::connect(addr).unwrap();
        write!(s, "GET {path} HTTP/1.0\r\nHost: test\r\n\r\n").unwrap();
        let mut buf = String::new();
        s.read_to_string(&mut buf).unwrap();
        let code = buf.split_whitespace().nth(1).unwrap().parse().unwrap();
        let body = buf.split("\r\n\r\n").nth(1).unwrap_or("").to_string();
        (code, body)
    }

    /// Full loop against real sockets: HTTP endpoints, token gate, UDP
    /// discovery reply, and a clean stop that releases the port.
    #[test]
    fn end_to_end_http_and_discovery() {
        let store = store_with_events(vec![ev("claude_code", "task_started", Some("t1"))]);
        apply(store.clone(), true, 17788, "sek");

        let (code, body) = http_get("127.0.0.1:17788", "/api/ping");
        assert_eq!(code, 200);
        assert!(body.contains(r#""app":"aistatusplus""#), "{body}");

        let (code, body) = http_get("127.0.0.1:17788", "/api/device/summary");
        assert_eq!(code, 401);
        assert!(body.contains("unauthorized"));

        let (code, body) = http_get("127.0.0.1:17788", "/api/device/summary?token=sek");
        assert_eq!(code, 200);
        assert!(body.contains(r#""rev":""#) && body.contains(r#""st":"RUN""#), "{body}");

        let (code, _) = http_get("127.0.0.1:17788", "/api/nope");
        assert_eq!(code, 404);

        // discovery: probe like a device would (the UDP thread races startup, retry briefly)
        let sock = UdpSocket::bind("127.0.0.1:0").unwrap();
        sock.set_read_timeout(Some(Duration::from_millis(500))).unwrap();
        let mut reply = None;
        for _ in 0..6 {
            let _ = sock.send_to(DISCOVERY_MAGIC, ("127.0.0.1", DISCOVERY_PORT));
            let mut buf = [0u8; 256];
            if let Ok((n, _)) = sock.recv_from(&mut buf) {
                reply = Some(String::from_utf8_lossy(&buf[..n]).to_string());
                break;
            }
        }
        let reply = reply.expect("no discovery reply");
        assert!(reply.contains(r#""app":"aistatusplus""#) && reply.contains(r#""port":17788"#), "{reply}");

        // stop releases the port for rebinding
        apply(store.clone(), false, 17788, "");
        apply(store.clone(), true, 17788, "");
        let (code, _) = http_get("127.0.0.1:17788", "/api/ping");
        assert_eq!(code, 200);
        apply(store, false, 17788, "");
    }
}
