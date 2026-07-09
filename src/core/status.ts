import { t } from "./i18n";
import type { NetworkStatus, TaskInfo, VisibleStatus } from "./types";

export const DONE_TTL_MS = 4000; // done_event shows for 3~5s; use 4s

export const GLYPHS: Record<VisibleStatus, string> = {
  running: "▶",
  waiting: "?",
  error: "×",
  done_event: "✓",
  paused: "‖",
  stale: "⇄",
};

/** Aggregate badge for a tool row in compact mode: highest-priority task, else network, else idle green dot */
const BADGE_ORDER: VisibleStatus[] = [
  "error",
  "waiting",
  "paused",
  "running",
  "stale",
  "done_event",
];

export function toolBadge(
  tasks: TaskInfo[],
  network: NetworkStatus,
): { glyph: string; cls: string } {
  for (const status of BADGE_ORDER) {
    if (tasks.some((t) => t.visible_status === status)) {
      return { glyph: GLYPHS[status], cls: status };
    }
  }
  if (network === "down") return { glyph: "⇄", cls: "net-down" };
  if (network === "flaky") return { glyph: "⇄", cls: "net-flaky" };
  return { glyph: "●", cls: "online" };
}

/**
 * Brand accent color per AI tool (the section's left color bar), taken from each brand's logo.
 * It only distinguishes tools; the vivid attention stays on the status icons — the thin bar is unobtrusive.
 * Colors come from the real logos; don't change them casually:
 * - Claude / Anthropic: clay
 * - Codex: the Codex.app icon's main color (blue-violet), sampled from icon-codex-dark-color.png
 * - Cursor: a mostly black-and-white brand with no established accent, so a neutral silver-gray
 */
const TOOL_ACCENTS: Record<string, string> = {
  claude_code: "#d97757", // Anthropic clay
  codex: "#727bf7", // Codex icon blue-violet (sampled main color)
  cursor: "#9b9ea6", // monochrome brand -> neutral silver-gray
};

export function toolAccent(toolId: string): string {
  return TOOL_ACCENTS[toolId] ?? "#6b6e75";
}

/**
 * Collapsed (small-window) mode: flatten all tools' task statuses into one row of status icons,
 * without tool names. Sorted by priority; returns the badge list (caller shows at most 4).
 * Network trouble is the highest-priority badge; when everything is idle, show one online green ring.
 */
const COLLAPSED_ORDER: VisibleStatus[] = [
  "error",
  "waiting",
  "paused",
  "running",
  "stale",
  "done_event",
];

export function collapsedBadges(
  tasks: TaskInfo[],
  network: NetworkStatus,
): { cls: string; glyph: string }[] {
  const badges: { cls: string; glyph: string }[] = [];
  if (network === "down") badges.push({ cls: "net-down", glyph: "⇄" });
  else if (network === "flaky") badges.push({ cls: "net-flaky", glyph: "⇄" });

  const sorted = [...tasks].sort(
    (a, b) =>
      COLLAPSED_ORDER.indexOf(a.visible_status) -
      COLLAPSED_ORDER.indexOf(b.visible_status),
  );
  for (const t of sorted) {
    badges.push({ cls: t.visible_status, glyph: GLYPHS[t.visible_status] });
  }
  if (badges.length === 0) badges.push({ cls: "online", glyph: "●" });
  return badges;
}

/** Parse "9:30pm" / "3pm" / "15:40" into the next local timestamp it occurs; null if unparseable (e.g. a weekday) */
function parseResetToMs(reset: string, now: number): number | null {
  const m = reset.trim().match(/^(\d{1,2})(?::(\d{2}))?\s*(am|pm)?$/i);
  if (!m) return null;
  let hour = parseInt(m[1], 10);
  const min = m[2] ? parseInt(m[2], 10) : 0;
  const ap = m[3]?.toLowerCase();
  if (ap === "pm" && hour < 12) hour += 12;
  if (ap === "am" && hour === 12) hour = 0;
  const d = new Date(now);
  d.setHours(hour, min, 0, 0);
  let target = d.getTime();
  if (target <= now) target += 24 * 3600_000; // already passed -> take the next occurrence
  return target;
}

/**
 * Tool-level quota row text: shows "countdown + reset time" next to the red clock.
 * hint = "session|9:30pm" / "weekly|Monday 9am" / "quota|".
 * Recomputed each poll (~1s) so the countdown is live; when a clock time is parseable it also
 * gives the absolute reset moment.
 */
export function formatQuotaRow(hint: string, now: number = Date.now()): string {
  const [kind, reset] = hint.split("|");
  const label =
    kind === "weekly" ? t("q_weekly") : kind === "session" ? t("q_5h") : t("q_generic");
  if (!reset) return `${label} ${t("q_reached")}`; // no reset info
  const target = parseResetToMs(reset, now);
  if (target === null) return `${label} · ${reset}`; // weekday etc. can't count down -> show as-is
  const totalMin = Math.max(0, Math.floor((target - now) / 60000));
  const h = Math.floor(totalMin / 60);
  const mm = totalMin % 60;
  const cd = h > 0 ? `${h}h${mm}m` : `${mm}m`;
  const clock = new Date(target).toLocaleTimeString([], {
    hour: "2-digit",
    minute: "2-digit",
    hour12: false,
  });
  return `${label} · ${t("q_left")} ${cd} · ${clock}`;
}

/** Compact mode: countdown + reset time ("2h29m · 15:10"), no label; falls back to "reached" with no reset info. */
export function formatQuotaShort(hint: string, now: number = Date.now()): string {
  const [, reset] = hint.split("|");
  if (!reset) return t("q_reached");
  const target = parseResetToMs(reset, now);
  if (target === null) return reset; // weekday etc. can't count down -> show as-is
  const totalMin = Math.max(0, Math.floor((target - now) / 60000));
  const h = Math.floor(totalMin / 60);
  const mm = totalMin % 60;
  const cd = h > 0 ? `${h}h${mm}m` : `${mm}m`;
  const clock = new Date(target).toLocaleTimeString([], {
    hour: "2-digit",
    minute: "2-digit",
    hour12: false,
  });
  return `${cd} · ${clock}`;
}

export function formatTokens(total?: number | null): string {
  if (total == null) return "";
  if (total >= 1_000_000) return `${(total / 1_000_000).toFixed(1)}m`;
  if (total >= 1_000) return `${(total / 1_000).toFixed(1)}k`;
  return String(total);
}

export function formatElapsed(total: number): string {
  const h = Math.floor(total / 3600);
  const m = Math.floor((total % 3600) / 60);
  const s = Math.floor(total % 60);
  const mm = String(m).padStart(2, "0");
  const ss = String(s).padStart(2, "0");
  return h > 0 ? `${h}:${mm}:${ss}` : `${mm}:${ss}`;
}

/**
 * A done_event task is shown only for DONE_TTL_MS, then filtered out.
 * firstSeen is held by the caller (useRef), recording when each done task first appeared.
 * The server also clears it after 5s; this is the frontend fallback + the mock-mode implementation.
 */
export function pruneDoneTasks(
  tasks: TaskInfo[],
  firstSeen: Map<string, number>,
  now: number,
  ttlMs: number = DONE_TTL_MS,
): TaskInfo[] {
  const liveIds = new Set(tasks.map((t) => t.task_id));
  for (const id of [...firstSeen.keys()]) {
    if (!liveIds.has(id)) firstSeen.delete(id);
  }
  return tasks.filter((t) => {
    if (t.visible_status !== "done_event") {
      firstSeen.delete(t.task_id);
      return true;
    }
    if (!firstSeen.has(t.task_id)) firstSeen.set(t.task_id, now);
    return now - (firstSeen.get(t.task_id) ?? now) <= ttlMs;
  });
}
