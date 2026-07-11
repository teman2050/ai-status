import { useEffect, useState } from "react";
import type { MouseEvent as ReactMouseEvent } from "react";
import { LogicalSize } from "@tauri-apps/api/dpi";
import { ToolSection } from "./components/ToolSection";
import { IN_TAURI, useConfig } from "./core/config";
import { t } from "./core/i18n";
import { useAgentState } from "./core/useAgentState";

const TOOLS: { id: string; name: string }[] = [
  { id: "claude_code", name: "Claude Code" },
  { id: "codex", name: "Codex" },
  { id: "cursor", name: "Cursor" },
];
const IS_WINDOWS = navigator.userAgent.includes("Windows");
const WINDOW_DIAGNOSTICS = import.meta.env.VITE_WINDOW_DIAGNOSTICS === "1";
const WINDOWS_PANEL_WIDTH = 300;
const WINDOWS_PANEL_HEIGHT = 480;

function Toggle({ on, onClick }: { on: boolean; onClick: () => void }) {
  return (
    <button className={on ? "sw on" : "sw"} onClick={onClick} aria-pressed={on}>
      <span className="sw-knob" />
    </button>
  );
}

function startDrag(e: ReactMouseEvent) {
  if (e.button !== 0 || !IN_TAURI) return;
  const target = e.target as HTMLElement | null;
  if (target?.closest("button, input")) return;
  import("@tauri-apps/api/window").then(({ getCurrentWindow }) =>
    getCurrentWindow().startDragging(),
  );
}

async function quitApp() {
  if (!IN_TAURI) return;
  const { invoke } = await import("@tauri-apps/api/core");
  void invoke("quit_app");
}

async function hidePanel() {
  if (!IN_TAURI) return;
  void recordPanelDiag("panel-close-request", {});
  const { getCurrentWindow } = await import("@tauri-apps/api/window");
  try {
    await getCurrentWindow().hide();
    void recordPanelDiag("panel-hidden", {});
  } catch (error) {
    void recordPanelDiag("panel-hide-error", { message: String(error) });
  }
}

async function recordPanelDiag(label: string, payload: Record<string, unknown>) {
  if (!WINDOW_DIAGNOSTICS || !IN_TAURI) return;
  try {
    const { invoke } = await import("@tauri-apps/api/core");
    await invoke("record_window_diag", { label, payload });
  } catch {
    // Diagnostics must never affect the panel.
  }
}

export default function Panel() {
  const { tools, tasks, network } = useAgentState(false);
  const [cfg, update] = useConfig();
  const connected = tools.filter(
    (t) => t.status === "connected" && (cfg.tools_enabled[t.tool_id] ?? true),
  );
  // read the real version (from tauri.conf.json) instead of hardcoding, so it never drifts
  const [version, setVersion] = useState("");
  useEffect(() => {
    if (!IN_TAURI) return;
    void import("@tauri-apps/api/app").then(({ getVersion }) =>
      getVersion().then(setVersion),
    );
  }, []);

  // LAN IP shown next to the device-API toggle so users can verify from a phone
  const [lanIp, setLanIp] = useState("");
  useEffect(() => {
    if (!IN_TAURI || !cfg.device_api) return;
    void import("@tauri-apps/api/core").then(({ invoke }) =>
      invoke<string>("get_lan_ip").then(setLanIp),
    );
  }, [cfg.device_api]);

  useEffect(() => {
    if (!IN_TAURI || !IS_WINDOWS) return;
    void import("@tauri-apps/api/window").then(({ getCurrentWindow }) =>
      getCurrentWindow()
        .setSize(new LogicalSize(WINDOWS_PANEL_WIDTH, WINDOWS_PANEL_HEIGHT))
        .catch((error) => {
          void recordPanelDiag("panel-size-error", { message: String(error) });
        }),
    );
  }, []);

  useEffect(() => {
    if (!WINDOW_DIAGNOSTICS || !IN_TAURI) return;
    const pointer = (event: PointerEvent) => {
      const target = event.target as HTMLElement | null;
      void recordPanelDiag("panel-pointer", {
        type: event.type,
        x: event.clientX,
        y: event.clientY,
        tag: target?.tagName,
        className: typeof target?.className === "string" ? target.className : "",
      });
    };
    document.addEventListener("pointerdown", pointer, true);
    document.addEventListener("pointerup", pointer, true);
    const timer = window.setTimeout(() => {
      void (async () => {
        const { getCurrentWindow } = await import("@tauri-apps/api/window");
        const win = getCurrentWindow();
        const inner = await win.innerSize().catch(() => undefined);
        const outer = await win.outerSize().catch(() => undefined);
        const position = await win.outerPosition().catch(() => undefined);
        const panel = document.querySelector(".panel")?.getBoundingClientRect();
        void recordPanelDiag("panel-measure", {
          inner,
          outer,
          position,
          panel: panel
            ? { width: panel.width, height: panel.height, top: panel.top, left: panel.left }
            : undefined,
        });
      })();
    }, 240);
    return () => {
      document.removeEventListener("pointerdown", pointer, true);
      document.removeEventListener("pointerup", pointer, true);
      window.clearTimeout(timer);
    };
  }, []);

  return (
    <div className={["panel", IS_WINDOWS ? "win" : ""].filter(Boolean).join(" ")}>
      <header className="panel-head">
        <span className="panel-drag" onMouseDown={startDrag}>AI STATUS</span>
        <span className="panel-head-actions">
          <span className="panel-ver">{version && `v${version}`}</span>
          <button
            type="button"
            className="panel-close"
            aria-label="Close"
            onClick={(e) => {
              e.stopPropagation();
              void hidePanel();
            }}
            onPointerUp={(e) => {
              e.stopPropagation();
              void hidePanel();
            }}
            onPointerDown={(e) => {
              e.stopPropagation();
              void hidePanel();
            }}
            onMouseDown={(e) => {
              e.stopPropagation();
              void hidePanel();
            }}
          >
            x
          </button>
        </span>
      </header>

      <section className="panel-sec">
        <div className="panel-label">{t("sec_status")}</div>
        {connected.length === 0 ? (
          <div className="panel-onboard">
            <div className="panel-onboard-title">{t("no_tools")}</div>
            <div className="panel-onboard-hint">{t("onboard_hint")}</div>
          </div>
        ) : (
          connected.map((tool) => (
            <ToolSection
              key={tool.tool_id}
              tool={tool}
              tasks={tasks.filter((tk) => tk.tool_id === tool.tool_id)}
              network={network}
            />
          ))
        )}
      </section>

      <section className="panel-sec">
        <div className="panel-label">{t("sec_floating")}</div>
        <div className="row">
          <span>{t("show_floating")}</span>
          <Toggle on={cfg.floating_visible} onClick={() => update({ floating_visible: !cfg.floating_visible })} />
        </div>
        <div className="row">
          <span>{t("compact_floating")}</span>
          <Toggle on={cfg.compact} onClick={() => update({ compact: !cfg.compact })} />
        </div>
        <div className="row">
          <span>{t("opacity")}</span>
          <input
            type="range"
            min={60}
            max={100}
            step={1}
            value={Math.round(cfg.opacity * 100)}
            onChange={(e) => update({ opacity: Number(e.target.value) / 100 })}
          />
          <span className="row-val">{Math.round(cfg.opacity * 100)}%</span>
        </div>
        <div className="row">
          <span>{t("always_on_top")}</span>
          <Toggle on={cfg.always_on_top} onClick={() => update({ always_on_top: !cfg.always_on_top })} />
        </div>
      </section>

      <section className="panel-sec">
        <div className="panel-label">{t("sec_appearance")}</div>
        <div className="row">
          <span>{t("theme")}</span>
          <span className="seg">
            <button className={cfg.theme === "dark" ? "on" : ""} onClick={() => update({ theme: "dark" })}>{t("dark")}</button>
            <button className={cfg.theme === "light" ? "on" : ""} onClick={() => update({ theme: "light" })}>{t("light")}</button>
          </span>
        </div>
        <div className="row">
          <span>{t("language")}</span>
          <span className="seg">
            <button className={cfg.language === "zh" ? "on" : ""} onClick={() => update({ language: "zh" })}>中文</button>
            <button className={cfg.language === "en" ? "on" : ""} onClick={() => update({ language: "en" })}>EN</button>
          </span>
        </div>
      </section>

      <section className="panel-sec">
        <div className="panel-label">{t("sec_tools")}</div>
        <div className="chips">
          {TOOLS.map((tool) => {
            const on = cfg.tools_enabled[tool.id] ?? true;
            return (
              <button
                key={tool.id}
                className={on ? "chip on" : "chip"}
                onClick={() =>
                  update({ tools_enabled: { ...cfg.tools_enabled, [tool.id]: !on } })
                }
              >
                {tool.name}
              </button>
            );
          })}
        </div>
      </section>

      <section className="panel-sec">
        <div className="panel-label">{t("sec_general")}</div>
        <div className="row">
          <span>{t("menubar_progress")}</span>
          <Toggle on={cfg.menubar_progress} onClick={() => update({ menubar_progress: !cfg.menubar_progress })} />
        </div>
        <div className="row">
          <span>{t("launch_at_login")}</span>
          <Toggle on={cfg.launch_at_login} onClick={() => update({ launch_at_login: !cfg.launch_at_login })} />
        </div>
        <div className="row">
          <span>{t("claude_hooks")}</span>
          <Toggle on={cfg.claude_hooks} onClick={() => update({ claude_hooks: !cfg.claude_hooks })} />
        </div>
        <div className="row">
          <span>{t("codex_notify")}</span>
          <Toggle on={cfg.codex_notify} onClick={() => update({ codex_notify: !cfg.codex_notify })} />
        </div>
        <div className="row">
          <span>{t("cursor_hooks")}</span>
          <Toggle on={cfg.cursor_hooks} onClick={() => update({ cursor_hooks: !cfg.cursor_hooks })} />
        </div>
        <div className="row">
          <span>{t("network_probe")}</span>
          <Toggle on={cfg.network_probe} onClick={() => update({ network_probe: !cfg.network_probe })} />
        </div>
      </section>

      <section className="panel-sec">
        <div className="panel-label">{t("sec_device")}</div>
        <div className="row">
          <span>{t("device_api")}</span>
          <Toggle on={cfg.device_api} onClick={() => update({ device_api: !cfg.device_api })} />
        </div>
        {cfg.device_api && (
          <>
            <div className="row">
              <span>{t("device_addr")}</span>
              <span className="row-val">
                {lanIp ? `${lanIp}:${cfg.device_api_port}` : "…"}
              </span>
            </div>
            <div className="panel-onboard-hint">{t("device_hint")}</div>
          </>
        )}
      </section>

      <footer className="panel-foot">
        <span className="panel-note">{t("local_note")}</span>
        <button className="panel-quit" onClick={quitApp}>{t("quit")}</button>
      </footer>
    </div>
  );
}
