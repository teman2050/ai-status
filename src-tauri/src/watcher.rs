use crate::store::{IncomingEvent, Store};
use std::collections::HashSet;
#[cfg(not(windows))]
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
#[cfg(windows)]
use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
#[cfg(windows)]
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
    TH32CS_SNAPPROCESS,
};
#[cfg(windows)]
use windows_sys::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION,
};

const POLL_SECS: u64 = 3;

struct Watched {
    tool_id: &'static str,
    process_name: &'static str, // exact process image match
    #[cfg(windows)]
    path_contains: Option<&'static str>,
}

#[cfg(not(windows))]
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

#[cfg(windows)]
const WATCHED: &[Watched] = &[
    Watched {
        tool_id: "claude_code",
        process_name: "claude",
        path_contains: Some("\\claude-code\\"),
    },
    Watched {
        tool_id: "claude_code",
        process_name: "claude-code",
        path_contains: None,
    },
    Watched {
        tool_id: "codex",
        process_name: "codex",
        path_contains: None,
    },
    Watched {
        tool_id: "codex",
        process_name: "Codex",
        path_contains: None,
    },
    Watched {
        tool_id: "cursor",
        process_name: "Cursor",
        path_contains: None,
    },
];

#[cfg(not(windows))]
fn is_process_running(name: &str) -> bool {
    Command::new("pgrep")
        .args(["-x", name])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[cfg(windows)]
fn target_process_image(name: &str) -> String {
    if name.to_ascii_lowercase().ends_with(".exe") {
        name.to_ascii_lowercase()
    } else {
        format!("{}.exe", name.to_ascii_lowercase())
    }
}

#[cfg(windows)]
fn path_matches(image_path: Option<&str>, marker: Option<&str>) -> bool {
    let Some(marker) = marker else {
        return true;
    };
    image_path
        .map(|path| path.to_ascii_lowercase().replace('/', "\\").contains(marker))
        .unwrap_or(false)
}

#[cfg(windows)]
fn watched_matches_process(watched: &Watched, exe_name: &str, image_path: Option<&str>) -> bool {
    exe_name == target_process_image(watched.process_name)
        && path_matches(image_path, watched.path_contains)
}

#[cfg(windows)]
fn process_image_path(process_id: u32) -> Option<String> {
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, process_id);
        if handle.is_null() {
            return None;
        }

        let mut buf = vec![0u16; 32768];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(handle, 0, buf.as_mut_ptr(), &mut len) != 0;
        CloseHandle(handle);

        if ok && len > 0 {
            Some(String::from_utf16_lossy(&buf[..len as usize]))
        } else {
            None
        }
    }
}

#[cfg(windows)]
fn running_tools() -> HashSet<String> {
    let mut out = HashSet::new();

    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snapshot == INVALID_HANDLE_VALUE {
            return out;
        }

        let mut entry: PROCESSENTRY32W = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;

        if Process32FirstW(snapshot, &mut entry) != 0 {
            loop {
                let len = entry
                    .szExeFile
                    .iter()
                    .position(|c| *c == 0)
                    .unwrap_or(entry.szExeFile.len());
                let image = String::from_utf16_lossy(&entry.szExeFile[..len]).to_ascii_lowercase();
                let mut image_path: Option<String> = None;

                for watched in WATCHED {
                    if image != target_process_image(watched.process_name) {
                        continue;
                    }
                    if watched.path_contains.is_some() && image_path.is_none() {
                        image_path = process_image_path(entry.th32ProcessID);
                    }
                    if watched_matches_process(watched, &image, image_path.as_deref()) {
                        out.insert(watched.tool_id.to_string());
                    }
                }

                if Process32NextW(snapshot, &mut entry) == 0 {
                    break;
                }
            }
        }

        CloseHandle(snapshot);
    }

    out
}

#[cfg(not(windows))]
fn running_tools() -> HashSet<String> {
    let mut cur: HashSet<String> = HashSet::new();
    for w in WATCHED {
        if is_process_running(w.process_name) {
            cur.insert(w.tool_id.to_string());
        }
    }
    cur
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
        #[cfg(not(windows))]
        let mut claude_was_alive = false;
        loop {
            let cur = running_tools();
            for (tool, up) in diff(&prev, &cur) {
                store.lock().unwrap().apply(event(&tool, up));
            }
            prev = cur;

            #[cfg(not(windows))]
            {
                let claude_alive = is_process_running("claude");
                if claude_was_alive && !claude_alive {
                    // falling edge: all claude sessions are gone, clear the claude_code section
                    store
                        .lock()
                        .unwrap()
                        .apply(event("claude_code", false));
                }
                claude_was_alive = claude_alive;
            }

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

    #[cfg(windows)]
    #[test]
    fn windows_claude_code_requires_claude_code_path() {
        let watched = Watched {
            tool_id: "claude_code",
            process_name: "claude",
            path_contains: Some("\\claude-code\\"),
        };
        assert!(watched_matches_process(
            &watched,
            "claude.exe",
            Some(r"C:\Users\Admin\AppData\Roaming\Claude\claude-code\2.1.202\claude.exe")
        ));
        assert!(!watched_matches_process(
            &watched,
            "claude.exe",
            Some(r"C:\Users\Admin\AppData\Local\AnthropicClaude\app-1.19367.0\claude.exe")
        ));
    }
}
