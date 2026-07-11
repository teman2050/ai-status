use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

/// User settings, persisted to config.json in the app config directory.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub theme: String,          // "dark" | "light"
    pub floating_visible: bool, // whether the floating widget is shown
    pub opacity: f64,           // 0.6 ~ 1.0, floating widget only
    pub always_on_top: bool,
    pub tools_enabled: HashMap<String, bool>, // tool_id -> shown or not
    pub network_probe: bool,    // whether to run network probing
    #[serde(default)]
    pub launch_at_login: bool, // launch at login
    #[serde(default)]
    pub menubar_progress: bool, // show progress rings in the menu bar (max 4)
    #[serde(default)]
    pub compact: bool, // floating widget compact (collapsed) mode: no title, one row per tool
    #[serde(default = "default_lang")]
    pub language: String, // "auto" | "zh" | "en"
    #[serde(default = "default_true")]
    pub claude_hooks: bool, // auto-manage Claude Code hook entries (built-in hook client)
    #[serde(default = "default_true")]
    pub codex_notify: bool, // auto-manage the Codex notify entry (chains any original notifier)
    #[serde(default = "default_true")]
    pub cursor_hooks: bool, // auto-manage Cursor hook entries (built-in hook client)
}

fn default_lang() -> String {
    "auto".to_string()
}

fn default_true() -> bool {
    true
}

impl Default for Config {
    fn default() -> Self {
        let mut tools = HashMap::new();
        tools.insert("claude_code".to_string(), true);
        tools.insert("codex".to_string(), true);
        tools.insert("cursor".to_string(), true);
        Config {
            theme: "dark".to_string(),
            floating_visible: true,
            opacity: 0.95,
            always_on_top: true,
            tools_enabled: tools,
            network_probe: true,
            launch_at_login: false,
            menubar_progress: false,
            compact: false,
            language: "auto".to_string(),
            claude_hooks: true,
            codex_notify: true,
            cursor_hooks: true,
        }
    }
}

/// Launch at login: write/remove a macOS LaunchAgent plist (no third-party deps).
pub fn apply_launch_at_login(enabled: bool) {
    let home = match std::env::var("HOME") {
        Ok(h) => h,
        Err(_) => return,
    };
    let dir = PathBuf::from(&home).join("Library/LaunchAgents");
    let plist = dir.join("com.aistatus.app.plist");
    if !enabled {
        let _ = fs::remove_file(&plist);
        return;
    }
    let exe = std::env::current_exe()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default();
    if exe.is_empty() {
        return;
    }
    let _ = fs::create_dir_all(&dir);
    let content = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
<plist version=\"1.0\"><dict>\n\
  <key>Label</key><string>com.aistatus.app</string>\n\
  <key>ProgramArguments</key><array><string>{exe}</string></array>\n\
  <key>RunAtLoad</key><true/>\n\
</dict></plist>\n"
    );
    let _ = fs::write(&plist, content);
}

/// Shared config (the net thread and commands share one Arc).
pub struct ConfigState(pub Arc<Mutex<Config>>);
/// Config directory (for saving).
pub struct ConfigDir(pub PathBuf);

fn config_path(dir: &PathBuf) -> PathBuf {
    dir.join("config.json")
}

pub fn load(dir: &PathBuf) -> Config {
    let path = config_path(dir);
    fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str::<Config>(&s).ok())
        .unwrap_or_default()
}

pub fn save(dir: &PathBuf, config: &Config) {
    let _ = fs::create_dir_all(dir);
    if let Ok(s) = serde_json::to_string_pretty(config) {
        let _ = fs::write(config_path(dir), s);
    }
}
