//! Manage AI STATUS's own hook entries in Claude Code's global settings
//! (~/.claude/settings.json), so packaged builds connect to Claude Code out of the box
//! without Python or the repo installer scripts.
//!
//! Ownership rule: only entries whose command ends with `--hook claude` (this app acting
//! as the hook client) or points at the legacy Python adapter `asb_hook.py` are ever
//! touched. Entries from any other tool are preserved as-is — including their key order
//! (serde_json's preserve_order feature keeps the file layout stable).

use serde_json::{json, Map, Value};
use std::path::{Path, PathBuf};

/// Same event set as scripts/install-claude-hooks.py.
const EVENTS: [&str; 12] = [
    "SessionStart",
    "UserPromptSubmit",
    "PreToolUse",
    "PostToolUse",
    "PostToolUseFailure",
    "PermissionRequest",
    "PermissionDenied",
    "Elicitation",
    "Notification",
    "StopFailure",
    "Stop",
    "SessionEnd",
];

fn home_dir() -> PathBuf {
    std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_default()
}

fn settings_path() -> PathBuf {
    home_dir().join(".claude").join("settings.json")
}

fn hook_command(exe: &Path) -> String {
    format!("\"{}\" --hook claude", exe.display())
}

/// An entry is ours when any of its hook commands is this app in hook mode, or the
/// legacy Python adapter (migrated away once the app manages hooks itself).
fn is_ours(entry: &Value) -> bool {
    entry["hooks"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|h| h["command"].as_str())
        .any(|c| c.trim_end().ends_with("--hook claude") || c.contains("asb_hook.py"))
}

/// Extract the executable path from a `"<exe>" --hook claude` command.
fn command_exe(entry: &Value) -> Option<String> {
    let cmd = entry["hooks"].as_array()?.first()?["command"].as_str()?;
    let cmd = cmd.trim();
    if let Some(rest) = cmd.strip_prefix('"') {
        return rest.split('"').next().map(str::to_string);
    }
    cmd.strip_suffix("--hook claude")
        .map(|s| s.trim().to_string())
}

fn our_entry(command: &str) -> Value {
    json!({
        "hooks": [{
            "type": "command",
            "command": command,
            "timeout": 5,
            "async": true,
        }]
    })
}

/// Bring one event's entry list to the desired state; foreign entries are never touched.
/// Keeps an existing app-based entry whose executable still exists (so a dev build and an
/// installed build don't fight over the path); anything else of ours is replaced.
fn sync_event(entries: &mut Vec<Value>, command: &str, enabled: bool) {
    let mut keep: Option<Value> = None;
    if enabled {
        keep = entries
            .iter()
            .find(|e| {
                is_ours(e)
                    && command_exe(e).is_some_and(|exe| {
                        !exe.contains("asb_hook.py") && Path::new(&exe).is_file()
                    })
            })
            .cloned()
            .or_else(|| Some(our_entry(command)));
    }
    entries.retain(|e| !is_ours(e));
    if let Some(entry) = keep {
        entries.push(entry);
    }
}

/// Install (or remove) our hook entries in the given settings file. Returns whether the
/// file changed. A missing file is created only when enabling; an unparseable file is
/// left untouched (never clobber the user's settings).
fn apply(path: &Path, exe: &Path, enabled: bool) -> std::io::Result<bool> {
    let original = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            if !enabled {
                return Ok(false);
            }
            String::from("{}")
        }
        Err(e) => return Err(e),
    };
    let Ok(mut root) = serde_json::from_str::<Value>(&original) else {
        return Ok(false);
    };
    if !root.is_object() {
        return Ok(false);
    }
    if root["hooks"].is_null() {
        root["hooks"] = Value::Object(Map::new());
    }
    let Some(hooks) = root["hooks"].as_object_mut() else {
        return Ok(false);
    };

    let command = hook_command(exe);
    // Sweep every event key so uninstall/migration also covers events outside EVENTS.
    let mut names: Vec<String> = hooks.keys().cloned().collect();
    for ev in EVENTS {
        if !names.iter().any(|n| n == ev) {
            names.push(ev.to_string());
        }
    }
    for name in names {
        let wanted = enabled && EVENTS.contains(&name.as_str());
        let entries = hooks
            .entry(name.clone())
            .or_insert_with(|| Value::Array(Vec::new()));
        let Some(list) = entries.as_array_mut() else {
            continue;
        };
        sync_event(list, &command, wanted);
        if list.is_empty() {
            hooks.remove(&name);
        }
    }

    let mut updated = serde_json::to_string_pretty(&root).unwrap_or(original.clone());
    updated.push('\n');
    if updated == original {
        return Ok(false);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if path.exists() {
        let backup = path.with_file_name(format!(
            "settings.json.bak-{}",
            chrono::Local::now().format("%Y%m%d-%H%M%S")
        ));
        let _ = std::fs::copy(path, backup);
    }
    std::fs::write(path, updated)?;
    Ok(true)
}

/// Sync the hook installation with the config toggle. Errors are logged, never fatal:
/// the board still runs (process detection / transcript scan) without hooks.
pub fn sync(enabled: bool) {
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let path = settings_path();
    match apply(&path, &exe, enabled) {
        Ok(true) => eprintln!(
            "[ai-status] claude hooks {} in {}",
            if enabled { "installed" } else { "removed" },
            path.display()
        ),
        Ok(false) => {}
        Err(e) => eprintln!("[ai-status] claude hooks sync failed: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    struct TempDir(PathBuf);
    impl TempDir {
        fn new(tag: &str) -> Self {
            let mut p = std::env::temp_dir();
            p.push(format!("asb_hooks_{tag}_{}", std::process::id()));
            let _ = fs::remove_dir_all(&p);
            fs::create_dir_all(&p).unwrap();
            TempDir(p)
        }
        fn path(&self, name: &str) -> PathBuf {
            self.0.join(name)
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn fake_exe(dir: &TempDir) -> PathBuf {
        let exe = dir.path("AI STATUS.exe");
        fs::write(&exe, "x").unwrap();
        exe
    }

    fn foreign_settings() -> String {
        serde_json::to_string_pretty(&json!({
            "hooks": {
                "PreToolUse": [{
                    "matcher": "*",
                    "hooks": [{"type": "command", "command": "py -3 D:/Other/tool_hook.py PreToolUse"}]
                }]
            },
            "otherSetting": true
        }))
        .unwrap()
    }

    #[test]
    fn install_adds_all_events_and_preserves_foreign_entries() {
        let dir = TempDir::new("install");
        let exe = fake_exe(&dir);
        let settings = dir.path("settings.json");
        fs::write(&settings, foreign_settings()).unwrap();

        assert!(apply(&settings, &exe, true).unwrap());
        let root: Value = serde_json::from_str(&fs::read_to_string(&settings).unwrap()).unwrap();
        assert_eq!(root["otherSetting"], true);
        for ev in EVENTS {
            let entries = root["hooks"][ev].as_array().unwrap();
            assert!(entries.iter().any(is_ours), "{ev} missing our entry");
        }
        // the foreign PreToolUse entry is untouched and comes first
        let pre = root["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(pre.len(), 2);
        assert!(pre[0]["hooks"][0]["command"]
            .as_str()
            .unwrap()
            .contains("tool_hook.py"));
    }

    #[test]
    fn install_is_idempotent() {
        let dir = TempDir::new("idem");
        let exe = fake_exe(&dir);
        let settings = dir.path("settings.json");
        assert!(apply(&settings, &exe, true).unwrap());
        assert!(!apply(&settings, &exe, true).unwrap(), "second run must not rewrite");
    }

    #[test]
    fn uninstall_removes_only_our_entries() {
        let dir = TempDir::new("uninstall");
        let exe = fake_exe(&dir);
        let settings = dir.path("settings.json");
        fs::write(&settings, foreign_settings()).unwrap();
        apply(&settings, &exe, true).unwrap();

        assert!(apply(&settings, &exe, false).unwrap());
        let root: Value = serde_json::from_str(&fs::read_to_string(&settings).unwrap()).unwrap();
        let hooks = root["hooks"].as_object().unwrap();
        assert_eq!(hooks.len(), 1, "only the foreign event key remains");
        assert_eq!(root["hooks"]["PreToolUse"].as_array().unwrap().len(), 1);
        assert_eq!(root["otherSetting"], true);
    }

    #[test]
    fn legacy_python_entries_are_migrated_to_the_app_command() {
        let dir = TempDir::new("migrate");
        let exe = fake_exe(&dir);
        let settings = dir.path("settings.json");
        let legacy = json!({
            "hooks": {
                "Stop": [{
                    "hooks": [{"type": "command", "command": "python3 /repo/adapters/claude-code/asb_hook.py"}]
                }]
            }
        });
        fs::write(&settings, serde_json::to_string_pretty(&legacy).unwrap()).unwrap();

        assert!(apply(&settings, &exe, true).unwrap());
        let root: Value = serde_json::from_str(&fs::read_to_string(&settings).unwrap()).unwrap();
        let stop = root["hooks"]["Stop"].as_array().unwrap();
        assert_eq!(stop.len(), 1);
        let cmd = stop[0]["hooks"][0]["command"].as_str().unwrap();
        assert!(cmd.ends_with("--hook claude"), "got: {cmd}");
        assert!(!cmd.contains("asb_hook.py"));
    }

    #[test]
    fn existing_entry_with_live_exe_is_kept_even_if_path_differs() {
        let dir = TempDir::new("keep");
        let exe_a = fake_exe(&dir);
        let exe_b = dir.path("AI STATUS 2.exe");
        fs::write(&exe_b, "x").unwrap();
        let settings = dir.path("settings.json");
        apply(&settings, &exe_a, true).unwrap();
        // another build syncs: exe_a still exists, so its entries stay put
        assert!(!apply(&settings, &exe_b, true).unwrap());
        // exe_a disappears -> next sync repairs the command to exe_b
        fs::remove_file(&exe_a).unwrap();
        assert!(apply(&settings, &exe_b, true).unwrap());
        let root: Value = serde_json::from_str(&fs::read_to_string(&settings).unwrap()).unwrap();
        let cmd = root["hooks"]["Stop"][0]["hooks"][0]["command"].as_str().unwrap();
        assert!(cmd.contains("AI STATUS 2.exe"));
    }

    #[test]
    fn unparseable_settings_are_left_untouched() {
        let dir = TempDir::new("badjson");
        let exe = fake_exe(&dir);
        let settings = dir.path("settings.json");
        fs::write(&settings, "{ not json").unwrap();
        assert!(!apply(&settings, &exe, true).unwrap());
        assert_eq!(fs::read_to_string(&settings).unwrap(), "{ not json");
    }
}
