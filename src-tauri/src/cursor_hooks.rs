//! Manage AI STATUS's own hook entries in Cursor's global hooks file
//! (~/.cursor/hooks.json), so packaged builds connect to Cursor out of the box without
//! Python or the repo installer script (scripts/install-cursor-hooks.py stays for
//! development).
//!
//! Ownership rule: only entries whose command ends with `--hook cursor` (this app acting
//! as the hook client) or points at the legacy Python adapter `asb_cursor_hook.py` are
//! ever touched. Entries from any other tool are preserved as-is — including their key
//! order (serde_json's preserve_order feature keeps the file layout stable).
//!
//! Only observational events plus beforeSubmitPrompt are registered. Cursor's gating
//! events (beforeShellExecution / beforeMCPExecution / beforeReadFile) block the user's
//! action until the hook answers on stdout, so we stay out of them; beforeSubmitPrompt is
//! needed for task_started and the hook client answers it with a pass-through
//! `{"continue": true}` (see hook_client::cursor_response).

use serde_json::{json, Map, Value};
use std::path::{Path, PathBuf};

/// Registered events: task start, activity heartbeats, and completion — no gating events
/// beyond beforeSubmitPrompt (see module docs).
const STEPS: [&str; 5] = [
    "beforeSubmitPrompt",
    "afterFileEdit",
    "afterShellExecution",
    "stop",
    "sessionEnd",
];

fn hooks_path() -> PathBuf {
    crate::claude_hooks::home_dir().join(".cursor").join("hooks.json")
}

fn hook_command(exe: &Path) -> String {
    format!("\"{}\" --hook cursor", exe.display())
}

/// An entry is ours when its command is this app in hook mode, or the legacy Python
/// adapter (migrated away once the app manages hooks itself).
fn is_ours(entry: &Value) -> bool {
    entry["command"].as_str().is_some_and(|c| {
        c.trim_end().ends_with("--hook cursor") || c.contains("asb_cursor_hook.py")
    })
}

/// Extract the executable path from a `"<exe>" --hook cursor` command.
fn command_exe(entry: &Value) -> Option<String> {
    let cmd = entry["command"].as_str()?.trim();
    if let Some(rest) = cmd.strip_prefix('"') {
        return rest.split('"').next().map(str::to_string);
    }
    cmd.strip_suffix("--hook cursor").map(|s| s.trim().to_string())
}

fn our_entry(command: &str) -> Value {
    json!({ "command": command })
}

/// Bring one step's entry list to the desired state; foreign entries are never touched.
/// Keeps an existing app-based entry whose executable still exists (so a dev build and an
/// installed build don't fight over the path); anything else of ours is replaced.
fn sync_step(entries: &mut Vec<Value>, command: &str, enabled: bool) {
    let mut keep: Option<Value> = None;
    if enabled {
        keep = entries
            .iter()
            .find(|e| {
                is_ours(e)
                    && command_exe(e).is_some_and(|exe| {
                        !exe.contains("asb_cursor_hook.py") && Path::new(&exe).is_file()
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

/// Install (or remove) our hook entries in the given hooks file. Returns whether the
/// file changed. A missing file is created only when enabling; an unparseable file is
/// left untouched (never clobber the user's hooks).
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
    if enabled && root["version"].is_null() {
        root["version"] = json!(1);
    }
    if root["hooks"].is_null() {
        root["hooks"] = Value::Object(Map::new());
    }
    let Some(hooks) = root["hooks"].as_object_mut() else {
        return Ok(false);
    };

    let command = hook_command(exe);
    // Sweep every step key so uninstall/migration also covers steps outside STEPS
    // (legacy installs registered gating events like beforeShellExecution).
    let mut names: Vec<String> = hooks.keys().cloned().collect();
    for step in STEPS {
        if !names.iter().any(|n| n == step) {
            names.push(step.to_string());
        }
    }
    for name in names {
        let wanted = enabled && STEPS.contains(&name.as_str());
        let entries = hooks
            .entry(name.clone())
            .or_insert_with(|| Value::Array(Vec::new()));
        let Some(list) = entries.as_array_mut() else {
            continue;
        };
        sync_step(list, &command, wanted);
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
            "hooks.json.bak-{}",
            chrono::Local::now().format("%Y%m%d-%H%M%S")
        ));
        let _ = std::fs::copy(path, backup);
    }
    std::fs::write(path, updated)?;
    Ok(true)
}

/// Sync the hook installation with the config toggle. Errors are logged, never fatal:
/// the board still shows Cursor via process detection without hooks.
pub fn sync(enabled: bool) {
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let path = hooks_path();
    match apply(&path, &exe, enabled) {
        Ok(true) => eprintln!(
            "[ai-status] cursor hooks {} in {}",
            if enabled { "installed" } else { "removed" },
            path.display()
        ),
        Ok(false) => {}
        Err(e) => eprintln!("[ai-status] cursor hooks sync failed: {e}"),
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
            p.push(format!("asb_cursor_{tag}_{}", std::process::id()));
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

    /// A hooks.json shaped like this machine's real one: another tool (PromLight) already
    /// has entries that must survive every sync untouched.
    fn foreign_hooks() -> String {
        serde_json::to_string_pretty(&json!({
            "version": 1,
            "hooks": {
                "beforeShellExecution": [
                    { "command": "python3 D:/PromLight/agent_hook.py" }
                ],
                "stop": [
                    { "command": "python3 D:/PromLight/agent_hook.py" }
                ]
            }
        }))
        .unwrap()
    }

    #[test]
    fn install_adds_all_steps_and_preserves_foreign_entries() {
        let dir = TempDir::new("install");
        let exe = fake_exe(&dir);
        let hooks = dir.path("hooks.json");
        fs::write(&hooks, foreign_hooks()).unwrap();

        assert!(apply(&hooks, &exe, true).unwrap());
        let root: Value = serde_json::from_str(&fs::read_to_string(&hooks).unwrap()).unwrap();
        assert_eq!(root["version"], 1);
        for step in STEPS {
            let entries = root["hooks"][step].as_array().unwrap();
            assert!(entries.iter().any(is_ours), "{step} missing our entry");
        }
        // the foreign entries are untouched; we never join gating events like beforeShellExecution
        let shell = root["hooks"]["beforeShellExecution"].as_array().unwrap();
        assert_eq!(shell.len(), 1);
        assert!(shell[0]["command"].as_str().unwrap().contains("PromLight"));
        let stop = root["hooks"]["stop"].as_array().unwrap();
        assert_eq!(stop.len(), 2);
        assert!(stop[0]["command"].as_str().unwrap().contains("PromLight"));
    }

    #[test]
    fn install_creates_missing_file_with_version() {
        let dir = TempDir::new("create");
        let exe = fake_exe(&dir);
        let hooks = dir.path("hooks.json");
        assert!(apply(&hooks, &exe, true).unwrap());
        let root: Value = serde_json::from_str(&fs::read_to_string(&hooks).unwrap()).unwrap();
        assert_eq!(root["version"], 1);
        assert_eq!(root["hooks"]["beforeSubmitPrompt"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn install_is_idempotent() {
        let dir = TempDir::new("idem");
        let exe = fake_exe(&dir);
        let hooks = dir.path("hooks.json");
        assert!(apply(&hooks, &exe, true).unwrap());
        assert!(!apply(&hooks, &exe, true).unwrap(), "second run must not rewrite");
    }

    #[test]
    fn uninstall_removes_only_our_entries() {
        let dir = TempDir::new("uninstall");
        let exe = fake_exe(&dir);
        let hooks = dir.path("hooks.json");
        fs::write(&hooks, foreign_hooks()).unwrap();
        apply(&hooks, &exe, true).unwrap();

        assert!(apply(&hooks, &exe, false).unwrap());
        let root: Value = serde_json::from_str(&fs::read_to_string(&hooks).unwrap()).unwrap();
        let map = root["hooks"].as_object().unwrap();
        assert_eq!(map.len(), 2, "only the foreign step keys remain");
        assert_eq!(root["hooks"]["beforeShellExecution"].as_array().unwrap().len(), 1);
        assert_eq!(root["hooks"]["stop"].as_array().unwrap().len(), 1);
        assert!(root["hooks"]["stop"][0]["command"]
            .as_str()
            .unwrap()
            .contains("PromLight"));
    }

    #[test]
    fn legacy_python_entries_are_migrated_including_gating_steps() {
        let dir = TempDir::new("migrate");
        let exe = fake_exe(&dir);
        let hooks = dir.path("hooks.json");
        // the legacy installer registered gating events (beforeShellExecution/beforeReadFile);
        // migration must remove those and re-register only STEPS
        let legacy = json!({
            "version": 1,
            "hooks": {
                "beforeSubmitPrompt": [
                    { "command": "python3 /repo/adapters/cursor/asb_cursor_hook.py" }
                ],
                "beforeShellExecution": [
                    { "command": "python3 /repo/adapters/cursor/asb_cursor_hook.py" }
                ],
                "beforeReadFile": [
                    { "command": "python3 /repo/adapters/cursor/asb_cursor_hook.py" }
                ]
            }
        });
        fs::write(&hooks, serde_json::to_string_pretty(&legacy).unwrap()).unwrap();

        assert!(apply(&hooks, &exe, true).unwrap());
        let root: Value = serde_json::from_str(&fs::read_to_string(&hooks).unwrap()).unwrap();
        assert!(root["hooks"]["beforeShellExecution"].is_null(), "gating step must be dropped");
        assert!(root["hooks"]["beforeReadFile"].is_null(), "gating step must be dropped");
        let submit = root["hooks"]["beforeSubmitPrompt"].as_array().unwrap();
        assert_eq!(submit.len(), 1);
        let cmd = submit[0]["command"].as_str().unwrap();
        assert!(cmd.ends_with("--hook cursor"), "got: {cmd}");
        assert!(!cmd.contains("asb_cursor_hook.py"));
    }

    #[test]
    fn existing_entry_with_live_exe_is_kept_even_if_path_differs() {
        let dir = TempDir::new("keep");
        let exe_a = fake_exe(&dir);
        let exe_b = dir.path("AI STATUS 2.exe");
        fs::write(&exe_b, "x").unwrap();
        let hooks = dir.path("hooks.json");
        apply(&hooks, &exe_a, true).unwrap();
        assert!(!apply(&hooks, &exe_b, true).unwrap());
        fs::remove_file(&exe_a).unwrap();
        assert!(apply(&hooks, &exe_b, true).unwrap());
        let root: Value = serde_json::from_str(&fs::read_to_string(&hooks).unwrap()).unwrap();
        let cmd = root["hooks"]["stop"][0]["command"].as_str().unwrap();
        assert!(cmd.contains("AI STATUS 2.exe"));
    }

    #[test]
    fn unparseable_hooks_are_left_untouched() {
        let dir = TempDir::new("badjson");
        let exe = fake_exe(&dir);
        let hooks = dir.path("hooks.json");
        fs::write(&hooks, "{ not json").unwrap();
        assert!(!apply(&hooks, &exe, true).unwrap());
        assert_eq!(fs::read_to_string(&hooks).unwrap(), "{ not json");
    }
}
