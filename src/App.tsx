import { useCallback, useEffect, useRef } from "react";
import type { CSSProperties, MouseEvent as ReactMouseEvent } from "react";
import { LogicalSize } from "@tauri-apps/api/dpi";
import { currentMonitor, getCurrentWindow } from "@tauri-apps/api/window";
import { QuotaIcon } from "./components/QuotaIcon";
import { ToolSection } from "./components/ToolSection";
import { useConfig } from "./core/config";
import { t } from "./core/i18n";
import { formatQuotaShort, toolAccent, toolBadge } from "./core/status";
import { useAgentState } from "./core/useAgentState";

const USE_MOCK = import.meta.env.VITE_USE_MOCK === "1";
const WINDOW_DIAGNOSTICS = import.meta.env.VITE_WINDOW_DIAGNOSTICS === "1";
const DIAG_TOGGLE_COMPACT = import.meta.env.VITE_DIAG_TOGGLE_COMPACT === "1";
const IN_TAURI = "__TAURI_INTERNALS__" in window;
const IS_WINDOWS = navigator.userAgent.includes("Windows");
const WIN_WIDTH = 360;
const MIN_WIDGET_HEIGHT = 56;
const LEGACY_MAX_HEIGHT = 520;
const LEGACY_MIN_COMPACT_HEIGHT = 56;
const LEGACY_MIN_EMPTY_HEIGHT = 128;
const LEGACY_MIN_TOOL_HEIGHT = 240;
const LEGACY_HEIGHT_SAFETY = 24;
const SCREEN_MARGIN = 24;
const HEIGHT_TOLERANCE = 2;
const FIT_RETRY_DELAYS = [0, 80, 240];
const CONTENT_HEIGHT_SAFETY = 8;
const RESIZE_DEBOUNCE_MS = 50;

const DRAG_THRESHOLD_SQ = 25; // moving > 5px (25 squared) counts as a drag, otherwise a click

function readPx(value: string | undefined) {
  return Number.parseFloat(value || "0") || 0;
}

function measureBoardContent(board: HTMLElement, content: HTMLElement) {
  const cs = getComputedStyle(board);
  const border = readPx(cs.borderTopWidth) + readPx(cs.borderBottomWidth);
  const chrome =
    readPx(cs.paddingTop) +
    readPx(cs.paddingBottom) +
    border;
  const contentRect = content.getBoundingClientRect();
  let contentHeight = content.scrollHeight;
  const children = [];
  for (const child of Array.from(content.children)) {
    const rect = child.getBoundingClientRect();
    const childStyle = getComputedStyle(child);
    contentHeight = Math.max(
      contentHeight,
      rect.bottom - contentRect.top + readPx(childStyle.marginBottom),
    );
    children.push({
      tag: child.tagName,
      className: child.className,
      height: Math.ceil(rect.height),
      bottom: Math.ceil(rect.bottom - contentRect.top),
      scrollHeight: (child as HTMLElement).scrollHeight,
      offsetHeight: (child as HTMLElement).offsetHeight,
    });
  }
  const contentTarget = Math.ceil(contentHeight + chrome + CONTENT_HEIGHT_SAFETY);
  const boardOverflow = board.scrollHeight - board.clientHeight;
  const scrollTarget =
    boardOverflow > HEIGHT_TOLERANCE
      ? Math.ceil(board.scrollHeight + border + CONTENT_HEIGHT_SAFETY)
      : 0;
  return {
    boardClientHeight: board.clientHeight,
    boardScrollHeight: board.scrollHeight,
    boardOverflow,
    chrome: Math.ceil(chrome),
    border: Math.ceil(border),
    children,
    contentHeight: Math.ceil(contentHeight),
    scrollHeight: content.scrollHeight,
    contentTarget,
    scrollTarget,
    targetBase: Math.max(contentTarget, scrollTarget),
  };
}

function clampHeight(height: number, maxHeight: number) {
  return Math.min(maxHeight, Math.max(MIN_WIDGET_HEIGHT, Math.ceil(height)));
}

async function recordWindowDiag(label: string, payload: Record<string, unknown>) {
  if (!WINDOW_DIAGNOSTICS || !IN_TAURI) return;
  try {
    const { invoke } = await import("@tauri-apps/api/core");
    await invoke("record_window_diag", { label, payload });
  } catch {
    // Diagnostics must never affect the app.
  }
}

export default function App() {
  const { tools, tasks, network } = useAgentState(USE_MOCK);
  const [cfg, updateCfg] = useConfig();
  // compact/expand is config-driven (double-click the widget or the menu-bar toggle);
  // both windows stay in sync via the config-changed event
  const compact = cfg.compact;
  const connected = tools.filter(
    (t) => t.status === "connected" && (cfg.tools_enabled[t.tool_id] ?? true),
  );
  const boardClassName = ["board", compact ? "compact" : "", IS_WINDOWS ? "win" : ""]
    .filter(Boolean)
    .join(" ");
  const layoutSignature = [
    compact ? "compact" : "expanded",
    network,
    connected.map((t) => `${t.tool_id}:${t.quota ?? ""}`).join("|"),
    tasks
      .map((t) => `${t.tool_id}:${t.task_id}:${t.visible_status}:${t.quota_reset ?? ""}`)
      .join("|"),
  ].join(";");
  const boardRef = useRef<HTMLDivElement>(null);
  const contentRef = useRef<HTMLDivElement>(null);
  const fitRun = useRef(0);
  const fitTimer = useRef<ReturnType<typeof window.setTimeout> | undefined>(undefined);
  const lastAppliedSize = useRef({ width: 0, height: 0 });
  const diagToggleStarted = useRef(false);
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

  const fitWindow = useCallback(async (reason: string) => {
    if (!IN_TAURI) return;
    const run = ++fitRun.current;
    const win = getCurrentWindow();
    const board = boardRef.current;
    const content = contentRef.current;
    if (!board || !content) return;

    const measurement = measureBoardContent(board, content);
    if (!IS_WINDOWS) {
      const minHeight = board.classList.contains("compact")
        ? LEGACY_MIN_COMPACT_HEIGHT
        : content.querySelector(".tool-section, .compact-row")
          ? LEGACY_MIN_TOOL_HEIGHT
          : LEGACY_MIN_EMPTY_HEIGHT;
      const target = Math.min(
        LEGACY_MAX_HEIGHT,
        Math.max(minHeight, measurement.targetBase + LEGACY_HEIGHT_SAFETY),
      );
      if (
        lastAppliedSize.current.width === WIN_WIDTH &&
        Math.abs(lastAppliedSize.current.height - target) <= HEIGHT_TOLERANCE
      ) {
        return;
      }
      lastAppliedSize.current = { width: WIN_WIDTH, height: target };
      try {
        await win.setSize(new LogicalSize(WIN_WIDTH, target));
      } catch {
        lastAppliedSize.current = { width: 0, height: 0 };
        // Ignore window resize failures; the next layout tick will retry.
      }
      return;
    }

    const sizing = await (async () => {
      try {
        const [scale, monitor] = await Promise.all([win.scaleFactor(), currentMonitor()]);
        if (monitor) {
          return {
            maxHeight: Math.max(
              MIN_WIDGET_HEIGHT,
              Math.floor(monitor.workArea.size.height / scale - SCREEN_MARGIN),
            ),
            scale,
          };
        }
      } catch {
        // Fall back to the browser-reported work area when monitor info is unavailable.
      }
      return {
        maxHeight: Math.max(MIN_WIDGET_HEIGHT, Math.floor(window.screen.availHeight - SCREEN_MARGIN)),
        scale: window.devicePixelRatio || 1,
      };
    })();
    if (run !== fitRun.current) return;

    const cssToLogical = Math.max(1, (window.devicePixelRatio || 1) / sizing.scale);
    const target = clampHeight(measurement.targetBase * cssToLogical, sizing.maxHeight);
    const beforeInner = await win.innerSize().catch(() => undefined);
    const beforeOuter = await win.outerSize().catch(() => undefined);
    if (
      lastAppliedSize.current.width === WIN_WIDTH &&
      Math.abs(lastAppliedSize.current.height - target) <= HEIGHT_TOLERANCE
    ) {
      void recordWindowDiag("main-fit-skip", {
        reason,
        compact,
        connected: connected.length,
        cssToLogical,
        devicePixelRatio: window.devicePixelRatio,
        measurement,
        target,
        maxHeight: sizing.maxHeight,
        scale: sizing.scale,
        beforeInner,
        beforeOuter,
      });
      return;
    }
    lastAppliedSize.current = { width: WIN_WIDTH, height: target };
    try {
      await win.setSize(new LogicalSize(WIN_WIDTH, target));
      window.setTimeout(() => {
        void (async () => {
          const afterInner = await win.innerSize().catch(() => undefined);
          const afterOuter = await win.outerSize().catch(() => undefined);
          void recordWindowDiag("main-fit-apply", {
            reason,
            compact,
            connected: connected.length,
            cssToLogical,
            devicePixelRatio: window.devicePixelRatio,
            measurement,
            target,
            maxHeight: sizing.maxHeight,
            scale: sizing.scale,
            beforeInner,
            beforeOuter,
            afterInner,
            afterOuter,
          });
        })();
      }, 80);
    } catch {
      lastAppliedSize.current = { width: 0, height: 0 };
    }
  }, [compact, connected.length]);

  const requestFit = useCallback((reason: string) => {
    window.clearTimeout(fitTimer.current);
    fitTimer.current = window.setTimeout(
      () => void fitWindow(reason),
      RESIZE_DEBOUNCE_MS,
    );
  }, [fitWindow]);

  // Re-measure natural content height. The measurement is independent of the current window
  // height, so resize notifications do not create a feedback loop.
  useEffect(() => {
    if (!IN_TAURI) return;
    const content = contentRef.current;
    if (!content) return;
    const refit = () => requestFit("content-observer");
    const ro = new ResizeObserver(refit);
    ro.observe(content);
    const mo = new MutationObserver(refit);
    mo.observe(content, { childList: true, subtree: true, characterData: true });
    refit();
    void document.fonts?.ready.then(refit);
    return () => {
      window.clearTimeout(fitTimer.current);
      ro.disconnect();
      mo.disconnect();
    };
  }, [requestFit]);

  useEffect(() => {
    if (!IN_TAURI) return;
    const timers = FIT_RETRY_DELAYS.map((delay) =>
      window.setTimeout(() => void fitWindow(`layout:${delay}`), delay),
    );
    return () => timers.forEach((timer) => window.clearTimeout(timer));
  }, [layoutSignature, fitWindow]);

  useEffect(() => {
    if (!WINDOW_DIAGNOSTICS || !DIAG_TOGGLE_COMPACT || diagToggleStarted.current) return;
    diagToggleStarted.current = true;
    const timers = [
      window.setTimeout(() => {
        void recordWindowDiag("diag-toggle-compact", { compact: true });
        updateCfg({ compact: true });
      }, 5000),
      window.setTimeout(() => {
        void recordWindowDiag("diag-toggle-compact", { compact: false });
        updateCfg({ compact: false });
      }, 9000),
    ];
    return () => {
      timers.forEach((timer) => window.clearTimeout(timer));
      diagToggleStarted.current = false;
    };
  }, []);

  return (
    <div
      ref={boardRef}
      className={boardClassName}
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
