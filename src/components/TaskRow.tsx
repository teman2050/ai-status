import { t } from "../core/i18n";
import { formatElapsed, formatTokens } from "../core/status";
import type { TaskInfo } from "../core/types";
import { MarqueeText } from "./MarqueeText";
import { StatusIcon } from "./StatusIcon";

/**
 * The single place that localizes a task row's text: status-type text always comes from
 * i18n by visible_status; the backend / adapters should only send real activity (tool name,
 * etc.) or an empty string. That way the English UI never leaks Chinese.
 * - stale / paused (throttled): pure status, ignore the backend summary, localize directly
 * - other statuses: prefer the real summary (e.g. "Read"/"Bash"), fall back by status when empty
 */
function taskText(task: TaskInfo): string {
  switch (task.visible_status) {
    case "stale":
      return t("lost");
    case "paused":
      return t("q_throttled");
  }
  if (task.summary) return task.summary;
  switch (task.visible_status) {
    case "running":
      return t("working");
    case "waiting":
      return t("waiting_input");
    case "error":
      return t("failed");
    default:
      return "";
  }
}

export function TaskRow({ task }: { task: TaskInfo }) {
  return (
    <div className="task-row">
      <StatusIcon status={task.visible_status} />
      <span className="task-name" title={task.workspace}>{task.workspace}</span>
      <MarqueeText text={taskText(task)} />
      {task.tokens != null && (
        <span className="task-tokens">{formatTokens(task.tokens)}</span>
      )}
      <span className="task-time">{formatElapsed(task.elapsed_seconds)}</span>
    </div>
  );
}
