mod codex;
mod config;
mod net;
mod server;
mod store;
mod watcher;

use config::{Config, ConfigDir, ConfigState};
use std::{
    fs::OpenOptions,
    io::Write,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};
use tauri::{Emitter, Manager, State};

fn status_color(s: &str) -> (u8, u8, u8) {
    match s {
        "white" => (255, 255, 255),         // menu-bar default state: plain white ring (brand, no status color)
        "running" | "done_event" | "online" | "idle" => (44, 230, 130),
        "error" | "quota" => (255, 77, 77), // quota exhausted, same as error: red
        _ => (255, 192, 46),                // waiting / paused / stale
    }
}

/// Draw each task status as a ring, laid side by side into one RGBA image (used as the menu-bar tray icon).
/// The running ring has a gap that rotates with `phase` (a spinner animation).
/// Each ring has a dark outline so it stands out on both light and dark menu bars.
fn render_rings_rgba(statuses: &[String], phase: f32) -> (Vec<u8>, u32, u32) {
    use std::f32::consts::PI;
    let n = statuses.len().max(1) as u32;
    let cell: u32 = 44; // retina 2x
    let h = cell;
    let w = cell * n;
    let mut buf = vec![0u8; (w * h * 4) as usize];
    let cy = cell as f32 / 2.0;
    let r_out = cell as f32 * 0.41; // larger ring
    let r_in = cell as f32 * 0.26;
    let aa = 1.3f32;
    let ol = 1.6f32; // outline width
    for (i, s) in statuses.iter().enumerate() {
        let (cr, cg, cb) = status_color(s);
        let gapped = s == "running";
        let outlined = s != "white"; // the white ring drops the dark outline (looked like a black edge); colored rings keep it
        let cx = i as f32 * cell as f32 + cell as f32 / 2.0;
        let x0 = i as u32 * cell;
        for y in 0..h {
            for x in x0..(x0 + cell) {
                let dx = x as f32 + 0.5 - cx;
                let dy = y as f32 + 0.5 - cy;
                let dist = (dx * dx + dy * dy).sqrt();
                // gap (running)
                let mut in_gap = false;
                if gapped {
                    let ang = dy.atan2(dx);
                    let mut d = ang - phase;
                    while d > PI {
                        d -= 2.0 * PI;
                    }
                    while d < -PI {
                        d += 2.0 * PI;
                    }
                    in_gap = d.abs() < 0.5;
                }
                // outline (dark, slightly wider band); the white ring has no outline
                let ol_a = if in_gap || !outlined {
                    0.0
                } else {
                    (((r_out + ol) - dist) / aa)
                        .clamp(0.0, 1.0)
                        .min(((dist - (r_in - ol)) / aa).clamp(0.0, 1.0))
                };
                // colored ring (slightly narrower band, drawn over the outline)
                let c_a = if in_gap {
                    0.0
                } else {
                    ((r_out - dist) / aa)
                        .clamp(0.0, 1.0)
                        .min(((dist - r_in) / aa).clamp(0.0, 1.0))
                };
                let idx = ((y * w + x) * 4) as usize;
                if ol_a > 0.0 {
                    buf[idx] = 12;
                    buf[idx + 1] = 14;
                    buf[idx + 2] = 16;
                    buf[idx + 3] = (ol_a * 235.0) as u8;
                }
                if c_a > 0.0 {
                    buf[idx] = cr;
                    buf[idx + 1] = cg;
                    buf[idx + 2] = cb;
                    buf[idx + 3] = (c_a * 255.0) as u8;
                }
            }
        }
    }
    (buf, w, h)
}

/// Show/hide the floating widget (also writes to config and persists).
fn toggle_floating<R: tauri::Runtime>(app: &tauri::AppHandle<R>) {
    let visible = app
        .get_webview_window("main")
        .map(|w| w.is_visible().unwrap_or(false))
        .unwrap_or(false);
    apply_floating(app, !visible);
}

/// Apply the floating widget's visibility and persist it.
fn apply_floating<R: tauri::Runtime>(app: &tauri::AppHandle<R>, visible: bool) {
    if let Some(win) = app.get_webview_window("main") {
        if visible {
            let _ = win.show();
            let _ = win.set_focus();
        } else {
            let _ = win.hide();
        }
    }
    if let Some(state) = app.try_state::<ConfigState>() {
        let mut cfg = state.0.lock().unwrap();
        cfg.floating_visible = visible;
        if let Some(dir) = app.try_state::<ConfigDir>() {
            config::save(&dir.0, &cfg);
        }
        let _ = app.emit("config-changed", cfg.clone());
    }
}

fn window_diag_path<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> Result<PathBuf, String> {
    if let Ok(path) = std::env::var("AI_STATUS_DIAG_PATH") {
        return Ok(PathBuf::from(path));
    }
    let dir = app.path().app_config_dir().map_err(|e| e.to_string())?;
    Ok(dir.join("window-diagnostics.jsonl"))
}

fn write_window_diag<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    label: &str,
    payload: serde_json::Value,
) -> Result<(), String> {
    let path = window_diag_path(app)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let ts_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_millis();
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|e| e.to_string())?;
    let entry = serde_json::json!({
        "ts_ms": ts_ms,
        "label": label,
        "payload": payload,
    });
    writeln!(file, "{entry}").map_err(|e| e.to_string())
}

#[tauri::command]
fn record_window_diag(
    app: tauri::AppHandle,
    label: String,
    payload: serde_json::Value,
) -> Result<(), String> {
    write_window_diag(&app, &label, payload)
}

/// Position the window near the tray icon and keep it inside the monitor work area.
fn position_near_tray<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    win: &tauri::WebviewWindow<R>,
    rect: tauri::Rect,
) {
    use tauri::PhysicalPosition;
    let scale = win.scale_factor().unwrap_or(1.0);
    let pos = rect.position.to_physical::<f64>(scale);
    let size = rect.size.to_physical::<f64>(scale);
    let outer = win.outer_size().ok();
    let win_w = outer.as_ref().map(|s| s.width as f64).unwrap_or(400.0);
    let win_h = outer.as_ref().map(|s| s.height as f64).unwrap_or(620.0);
    let (work_x, work_y, work_w, work_h) = if let Ok(Some(mon)) = win.current_monitor() {
        let work = mon.work_area();
        (
            work.position.x as f64,
            work.position.y as f64,
            work.size.width as f64,
            work.size.height as f64,
        )
    } else {
        (0.0, 0.0, 1920.0, 1080.0)
    };
    let gap = 8.0;
    let mut x = pos.x + size.width / 2.0 - win_w / 2.0;
    x = x.clamp(work_x + gap, work_x + work_w - win_w - gap);

    let tray_center_y = pos.y + size.height / 2.0;
    let below_y = pos.y + size.height + gap;
    let above_y = pos.y - win_h - gap;
    let prefer_below = tray_center_y < work_y + work_h / 2.0;
    let mut y = if prefer_below { below_y } else { above_y };
    if y < work_y + gap {
        y = below_y;
    }
    if y + win_h > work_y + work_h - gap {
        y = above_y;
    }
    y = y.clamp(work_y + gap, work_y + work_h - win_h - gap);
    let _ = win.set_position(PhysicalPosition::new(x, y));
    let _ = write_window_diag(
        app,
        "panel-position",
        serde_json::json!({
            "scale": scale,
            "tray": {
                "x": pos.x,
                "y": pos.y,
                "width": size.width,
                "height": size.height,
            },
            "panel": {
                "width": win_w,
                "height": win_h,
                "x": x,
                "y": y,
                "prefer_below": prefer_below,
            },
            "work_area": {
                "x": work_x,
                "y": work_y,
                "width": work_w,
                "height": work_h,
            },
        }),
    );
}

/// Clicking the tray icon toggles the menu-bar panel (positioned below the icon).
fn toggle_panel_below_tray<R: tauri::Runtime>(app: &tauri::AppHandle<R>, rect: tauri::Rect) {
    if let Some(win) = app.get_webview_window("panel") {
        if win.is_visible().unwrap_or(false) {
            let _ = win.hide();
        } else {
            position_near_tray(app, &win, rect);
            let _ = win.show();
            let _ = win.set_focus();
        }
    }
}

#[tauri::command]
fn quit_app(app: tauri::AppHandle) {
    app.exit(0);
}

#[tauri::command]
fn get_config(state: State<ConfigState>) -> Config {
    state.0.lock().unwrap().clone()
}

#[tauri::command]
fn set_config(
    app: tauri::AppHandle,
    state: State<ConfigState>,
    dir: State<ConfigDir>,
    config: Config,
) {
    config::save(&dir.0, &config);
    let launch_changed = {
        let mut cur = state.0.lock().unwrap();
        let changed = cur.launch_at_login != config.launch_at_login;
        *cur = config.clone();
        changed
    };
    if launch_changed {
        config::apply_launch_at_login(config.launch_at_login);
    }
    if let Some(win) = app.get_webview_window("main") {
        let _ = win.set_always_on_top(config.always_on_top);
        if config.floating_visible {
            let _ = win.show();
        } else {
            let _ = win.hide();
        }
    }
    let _ = app.emit("config-changed", config);
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let store = Arc::new(Mutex::new(store::Store::new()));
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            get_config,
            set_config,
            quit_app,
            record_window_diag
        ])
        .setup(move |app| {
            use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};

            // load config (from the app config directory)
            let dir = app.path().app_config_dir().unwrap_or_default();
            let config = config::load(&dir);
            let cfg = Arc::new(Mutex::new(config.clone()));
            app.manage(ConfigState(cfg.clone()));
            app.manage(ConfigDir(dir));

            // start background services
            server::start(store.clone(), 7799);
            watcher::start(store.clone());
            net::start(store.clone(), cfg.clone());
            codex::start(store.clone());

            // launch at login: sync with config on startup
            config::apply_launch_at_login(config.launch_at_login);

            // menu-bar progress: when on, draw the top 4 task statuses as colored rings for the tray icon
            {
                let app_handle = app.handle().clone();
                let store_mb = store.clone();
                let cfg_mb = cfg.clone();
                std::thread::spawn(move || {
                    let mut last_sig = String::from("init");
                    let mut phase = 0.0f32;
                    loop {
                        let on = cfg_mb.lock().map(|c| c.menubar_progress).unwrap_or(false);
                        let statuses = if on {
                            store_mb.lock().unwrap().menubar_top_statuses(4)
                        } else {
                            Vec::new()
                        };
                        let has_running = statuses.iter().any(|s| s == "running");
                        let sig = format!("{on}|{statuses:?}");
                        let changed = sig != last_sig;
                        last_sig = sig;

                        if let Some(tray) = app_handle.tray_by_id("asb-tray") {
                            if !on {
                                // progress off -> plain white ring (brand icon)
                                if changed {
                                    let (rgba, w, h) =
                                        render_rings_rgba(&["white".to_string()], 0.0);
                                    let _ = tray
                                        .set_icon(Some(tauri::image::Image::new_owned(rgba, w, h)));
                                }
                            } else if statuses.is_empty() {
                                // on + idle -> a bold green ring (with outline)
                                if changed {
                                    let (rgba, w, h) =
                                        render_rings_rgba(&["online".to_string()], 0.0);
                                    let _ = tray
                                        .set_icon(Some(tauri::image::Image::new_owned(rgba, w, h)));
                                }
                            } else if has_running {
                                // has running tasks: rotate the gap each frame -> spinner animation
                                phase += 0.4;
                                let (rgba, w, h) = render_rings_rgba(&statuses, phase);
                                let _ = tray.set_icon(Some(tauri::image::Image::new_owned(rgba, w, h)));
                            } else if changed {
                                let (rgba, w, h) = render_rings_rgba(&statuses, 0.0);
                                let _ = tray.set_icon(Some(tauri::image::Image::new_owned(rgba, w, h)));
                            }
                        }
                        // refresh fast when running (for the animation), otherwise slow to save power
                        let ms = if has_running { 90 } else { 800 };
                        std::thread::sleep(std::time::Duration::from_millis(ms));
                    }
                });
            }

            // tray: left-click toggles the panel; right-click opens the menu (menu language follows config, applied on restart)
            let handle = app.handle();
            let en = config.language == "en";
            let float_label = if en { "Toggle floating window" } else { "显示 / 隐藏悬浮窗" };
            let quit_label = if en { "Quit" } else { "退出" };
            let float_item =
                tauri::menu::MenuItem::with_id(handle, "float", float_label, true, None::<&str>)?;
            let quit_item =
                tauri::menu::MenuItem::with_id(handle, "quit", quit_label, true, None::<&str>)?;
            let tray_menu = tauri::menu::Menu::with_items(handle, &[&float_item, &quit_item])?;
            // initial tray icon is the plain white ring (the update thread overwrites it by status), to avoid a green-logo flash at startup
            let (rgba, w, h) = render_rings_rgba(&["white".to_string()], 0.0);
            let init_icon = tauri::image::Image::new_owned(rgba, w, h);
            TrayIconBuilder::with_id("asb-tray")
                .icon(init_icon)
                .icon_as_template(false)
                .menu(&tray_menu)
                .show_menu_on_left_click(false)
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        rect,
                        ..
                    } = event
                    {
                        toggle_panel_below_tray(tray.app_handle(), rect);
                    }
                })
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "float" => toggle_floating(app),
                    "quit" => app.exit(0),
                    _ => {}
                })
                .build(app)?;

            if std::env::var("AI_STATUS_DIAG_OPEN_PANEL").ok().as_deref() == Some("1") {
                let app_handle = app.handle().clone();
                std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_millis(1500));
                    if let (Some(win), Some(tray)) = (
                        app_handle.get_webview_window("panel"),
                        app_handle.tray_by_id("asb-tray"),
                    ) {
                        if let Ok(Some(rect)) = tray.rect() {
                            position_near_tray(&app_handle, &win, rect);
                            let _ = win.show();
                            let _ = win.set_focus();
                        }
                    }
                });
            }

            // floating widget: apply config (always-on-top, visibility) and dock to the top-right
            if let Some(win) = app.get_webview_window("main") {
                let _ = win.set_always_on_top(config.always_on_top);
                if let Ok(Some(monitor)) = win.current_monitor() {
                    let msize = monitor.size();
                    let mpos = monitor.position();
                    let wwidth = win.outer_size().map(|s| s.width).unwrap_or(300);
                    let x = mpos.x + msize.width.saturating_sub(wwidth + 24) as i32;
                    let y = mpos.y + 48;
                    let _ = win.set_position(tauri::PhysicalPosition::new(x, y));
                }
                if !config.floating_visible {
                    let _ = win.hide();
                }
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_rings_dimensions_and_no_panic() {
        let statuses = vec![
            "error".to_string(),
            "waiting".to_string(),
            "running".to_string(),
        ];
        let (rgba, w, h) = render_rings_rgba(&statuses, 0.0);
        assert_eq!(h, 44);
        assert_eq!(w, 44 * 3);
        assert_eq!(rgba.len(), (w * h * 4) as usize);
        // the running ring has a gap: there should be some opaque pixels (the ring) and some transparent ones (gap / outside)
        let opaque = rgba.iter().skip(3).step_by(4).filter(|&&a| a > 0).count();
        assert!(opaque > 0 && (opaque as u32) < w * h);
    }

    #[test]
    fn render_rings_empty_falls_back_to_one_cell() {
        let (_, w, h) = render_rings_rgba(&[], 0.0);
        assert_eq!((w, h), (44, 44)); // n.max(1)
    }

    #[test]
    fn status_color_mapping() {
        assert_eq!(status_color("running"), (44, 230, 130));
        assert_eq!(status_color("online"), (44, 230, 130));
        assert_eq!(status_color("error"), (255, 77, 77));
        assert_eq!(status_color("waiting"), (255, 192, 46));
    }
}
