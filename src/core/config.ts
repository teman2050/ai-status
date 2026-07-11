import { useEffect, useRef, useState } from "react";
import type { LangSetting } from "./i18n";
import { setLang } from "./i18n";

export interface AppConfig {
  theme: "dark" | "light";
  floating_visible: boolean;
  opacity: number; // 0.6 ~ 1.0, applies to the floating widget only
  always_on_top: boolean;
  tools_enabled: Record<string, boolean>;
  network_probe: boolean;
  launch_at_login: boolean;
  menubar_progress: boolean;
  compact: boolean; // floating widget compact (collapsed) mode
  language: LangSetting;
  claude_hooks: boolean; // auto-manage Claude Code hook entries (built-in hook client)
  codex_notify: boolean; // auto-manage the Codex notify entry (chains any original notifier)
  cursor_hooks: boolean; // auto-manage Cursor hook entries (built-in hook client)
}

export const DEFAULT_CONFIG: AppConfig = {
  theme: "dark",
  floating_visible: true,
  opacity: 0.95,
  always_on_top: true,
  tools_enabled: { claude_code: true, codex: true, cursor: true },
  network_probe: true,
  launch_at_login: false,
  menubar_progress: false,
  compact: false,
  language: "auto",
  claude_hooks: true,
  codex_notify: true,
  cursor_hooks: true,
};

export const IN_TAURI = "__TAURI_INTERNALS__" in window;

/** Apply theme and opacity to the current window's root element (shared by both surfaces). */
export function applyTheme(config: AppConfig) {
  const root = document.documentElement;
  root.setAttribute("data-theme", config.theme);
  root.style.setProperty("--op", String(config.opacity));
  setLang(config.language);
}

/**
 * Read the config and subscribe to changes. Returns [config, update].
 * update applies locally right away + persists via a Tauri command; the other window syncs via an event.
 */
export function useConfig(): [AppConfig, (patch: Partial<AppConfig>) => void] {
  const [config, setConfig] = useState<AppConfig>(DEFAULT_CONFIG);
  const saveTimer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);

  useEffect(() => {
    if (!IN_TAURI) {
      applyTheme(DEFAULT_CONFIG);
      return;
    }
    let unlisten: (() => void) | undefined;
    (async () => {
      const { invoke } = await import("@tauri-apps/api/core");
      const { listen } = await import("@tauri-apps/api/event");
      try {
        const c = await invoke<AppConfig>("get_config");
        setConfig(c);
        applyTheme(c);
      } catch {
        applyTheme(DEFAULT_CONFIG);
      }
      unlisten = await listen<AppConfig>("config-changed", (e) => {
        setConfig(e.payload);
        applyTheme(e.payload);
      });
    })();
    return () => unlisten?.();
  }, []);

  const update = (patch: Partial<AppConfig>) => {
    const next = { ...config, ...patch };
    setConfig(next); // update locally immediately
    applyTheme(next); // theme/opacity take effect instantly (pure CSS)
    if (!IN_TAURI) return;
    // debounce persistence: while dragging the slider, write to disk only once it settles
    clearTimeout(saveTimer.current);
    saveTimer.current = setTimeout(() => {
      import("@tauri-apps/api/core").then(({ invoke }) => {
        void invoke("set_config", { config: next });
      });
    }, 160);
  };

  return [config, update];
}
