import { useEffect, useState } from "react";
import type { MouseEvent as ReactMouseEvent } from "react";
import { ToolSection } from "./components/ToolSection";
import { IN_TAURI, useConfig } from "./core/config";
import { t } from "./core/i18n";
import { useAgentState } from "./core/useAgentState";

const TOOLS: { id: string; name: string }[] = [
  { id: "claude_code", name: "Claude Code" },
  { id: "codex", name: "Codex" },
  { id: "cursor", name: "Cursor" },
];

function Toggle({ on, onClick }: { on: boolean; onClick: () => void }) {
  return (
    <button className={on ? "sw on" : "sw"} onClick={onClick} aria-pressed={on}>
      <span className="sw-knob" />
    </button>
  );
}

function startDrag(e: ReactMouseEvent) {
  if (e.button !== 0 || !IN_TAURI) return;
  import("@tauri-apps/api/window").then(({ getCurrentWindow }) =>
    getCurrentWindow().startDragging(),
  );
}

async function quitApp() {
  if (!IN_TAURI) return;
  const { invoke } = await import("@tauri-apps/api/core");
  void invoke("quit_app");
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

  return (
    <div className="panel">
      <header className="panel-head" data-tauri-drag-region onMouseDown={startDrag}>
        <span>AI STATUS</span>
        <span className="panel-ver">{version && `v${version}`}</span>
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
          <span>{t("network_probe")}</span>
          <Toggle on={cfg.network_probe} onClick={() => update({ network_probe: !cfg.network_probe })} />
        </div>
      </section>

      <footer className="panel-foot">
        <span className="panel-note">{t("local_note")}</span>
        <button className="panel-quit" onClick={quitApp}>{t("quit")}</button>
      </footer>
    </div>
  );
}
