import { useEffect, useRef } from "react";
import type { CSSProperties, MouseEvent as ReactMouseEvent } from "react";
import { LogicalSize } from "@tauri-apps/api/dpi";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { QuotaIcon } from "./components/QuotaIcon";
import { ToolSection } from "./components/ToolSection";
import { useConfig } from "./core/config";
import { t } from "./core/i18n";
import { formatQuotaShort, toolAccent, toolBadge } from "./core/status";
import { useAgentState } from "./core/useAgentState";

const USE_MOCK = import.meta.env.VITE_USE_MOCK === "1";
const IN_TAURI = "__TAURI_INTERNALS__" in window;
const WIN_WIDTH = 300;
const MAX_HEIGHT = 420; // scroll inside the board once content exceeds this
const MIN_HEIGHT = 40; // can be very short when compact

const DRAG_THRESHOLD_SQ = 25; // moving > 5px (25 squared) counts as a drag, otherwise a click

export default function App() {
  const { tools, tasks, network } = useAgentState(USE_MOCK);
  const [cfg, updateCfg] = useConfig();
  // compact/expand is config-driven (double-click the widget or the menu-bar toggle);
  // both windows stay in sync via the config-changed event
  const compact = cfg.compact;
  const connected = tools.filter(
    (t) => t.status === "connected" && (cfg.tools_enabled[t.tool_id] ?? true),
  );
  const contentRef = useRef<HTMLDivElement>(null);
  // drag vs click: don't drag on press; only startDragging once movement passes the threshold;
  // a double-click without dragging = toggle compact
  const press = useRef<{ x: number; y: number; dragging: boolean } | null>(null);

  const onBoardMouseDown = (e: ReactMouseEvent) => {
    if (e.button !== 0 || !IN_TAURI) return; // left button only; skip in browser preview
    press.current = { x: e.clientX, y: e.clientY, dragging: false };
  };
  const onBoardMouseMove = (e: ReactMouseEvent) => {
    const p = press.current;
    if (!p || p.dragging) return;
    const dx = e.clientX - p.x;
    const dy = e.clientY - p.y;
    if (dx * dx + dy * dy > DRAG_THRESHOLD_SQ) {
      p.dragging = true; // once dragging, the OS consumes the release, so no click/double-click fires
      void getCurrentWindow().startDragging();
    }
  };
  const onBoardMouseUp = () => {
    press.current = null;
  };
  const onBoardDoubleClick = () => {
    if (!IN_TAURI) return;
    updateCfg({ compact: !cfg.compact }); // double-click toggles compact/expand
  };

  // Keep the widget height glued to its content: a ResizeObserver watches the content box, and the
  // window height = content height + the card's padding/border (measured exactly, no slack), so any
  // content change resizes instantly and there's no empty space at the bottom.
  useEffect(() => {
    if (!IN_TAURI) return;
    const el = contentRef.current;
    if (!el) return;
    const board = el.parentElement; // .board: the card with padding + border
    let last = 0;
    const fit = () => {
      const cs = board ? getComputedStyle(board) : null;
      const chrome = cs
        ? parseFloat(cs.paddingTop) +
          parseFloat(cs.paddingBottom) +
          parseFloat(cs.borderTopWidth) +
          parseFloat(cs.borderBottomWidth)
        : 17;
      const target = Math.min(
        MAX_HEIGHT,
        Math.max(MIN_HEIGHT, Math.ceil(el.getBoundingClientRect().height + chrome)),
      );
      if (Math.abs(target - last) >= 1) {
        last = target;
        void getCurrentWindow().setSize(new LogicalSize(WIN_WIDTH, target));
      }
    };
    const ro = new ResizeObserver(fit);
    ro.observe(el);
    fit();
    return () => ro.disconnect();
  }, []);

  return (
    <div
      className="board"
      onMouseDown={onBoardMouseDown}
      onMouseMove={onBoardMouseMove}
      onMouseUp={onBoardMouseUp}
      onDoubleClick={onBoardDoubleClick}
    >
      <div ref={contentRef} className="board-content">
        {/* no title bar when compact, to stay minimal; the title lives only in the menu-bar panel */}
        {!compact && (
          <header className="board-header">
            <span>AI STATUS</span>
          </header>
        )}
        {connected.map((tool) => {
          const toolTasks = tasks.filter((t) => t.tool_id === tool.tool_id);
          const style = { "--accent": toolAccent(tool.tool_id) } as CSSProperties;
          if (compact) {
            // compact: tool name + a single merged ring (all tasks merged to the highest-priority status).
            // Quota exhaustion is tool-level: red clock icon + countdown, tasks not shown.
            if (tool.quota) {
              return (
                <div key={tool.tool_id} className="compact-row" style={style}>
                  <span className="tool-name">{tool.tool_name}</span>
                  <QuotaIcon />
                  <span className="quota-count">{formatQuotaShort(tool.quota)}</span>
                </div>
              );
            }
            const badge = toolBadge(toolTasks, network);
            return (
              <div key={tool.tool_id} className="compact-row" style={style}>
                <span className="tool-name">{tool.tool_name}</span>
                <span className={`status-icon ${badge.cls}`}>{badge.glyph}</span>
              </div>
            );
          }
          return (
            <ToolSection
              key={tool.tool_id}
              tool={tool}
              tasks={toolTasks}
              network={network}
            />
          );
        })}
        {connected.length === 0 && (
          <div className="board-empty">{t("no_tools")}</div>
        )}
      </div>
    </div>
  );
}
