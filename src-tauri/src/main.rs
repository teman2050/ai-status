// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // Hook-client mode: Claude Code / Codex / Cursor invoke this same binary as their
    // hook or notify command (`"AI STATUS" --hook <claude|codex|cursor>`). Handle it
    // before any Tauri setup so no window, tray, or server is touched, then exit.
    let args: Vec<String> = std::env::args().collect();
    if let Some(i) = args.iter().position(|a| a == "--hook") {
        let tool = args.get(i + 1).map(String::as_str).unwrap_or("claude");
        agent_status_board_lib::hook_client_main(tool);
        return;
    }
    agent_status_board_lib::run()
}
