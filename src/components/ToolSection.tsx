import type { CSSProperties } from "react";
import { t } from "../core/i18n";
import { formatQuotaRow, quotaWarnings, toolAccent } from "../core/status";
import type { NetworkStatus, TaskInfo, ToolInfo } from "../core/types";
import { QuotaIcon } from "./QuotaIcon";
import { TaskRow } from "./TaskRow";

/**
 * A tool section. The tool name is just a group heading with no light — lights live per row:
 * when there are tasks, each task row's own status icon speaks;
 * with no tasks and a healthy network, a single steady green dot = online;
 * on network trouble, a ⇄ row (blinking yellow = recovering, red = offline), and no "idle".
 * Each tool is distinguished by its own accent bar.
 */
export function ToolSection({
  tool,
  tasks,
  network,
}: {
  tool: ToolInfo;
  tasks: TaskInfo[];
  network: NetworkStatus;
}) {
  const style = { "--accent": toolAccent(tool.tool_id) } as CSSProperties;
  // Approaching-limit warnings (Codex 5h / weekly), shown before the account is actually blocked.
  const warnings = tool.quota_usage ? quotaWarnings(tool.quota_usage) : [];
  // Quota exhaustion is tool-level: the whole account hit its limit, so hide all its tasks and
  // show one red quota row + countdown/reset time.
  // Network trouble is a separate signal, still shown under quota (otherwise the user can't tell
  // "out of quota" apart from "offline").
  if (tool.quota) {
    return (
      <section className="tool-section" style={style}>
        <div className="tool-header">
          <span className="tool-name">{tool.tool_name}</span>
          <span className="tool-quota-blocked">
            <QuotaIcon />
            <span>{formatQuotaRow(tool.quota)}</span>
          </span>
        </div>
        {network !== "ok" && (
          <div className="task-row">
            <span className={`status-icon ${network === "down" ? "net-down" : "net-flaky"}`}>⇄</span>
            <span className="idle-text">{network === "down" ? t("net_down") : t("net_flaky")}</span>
          </div>
        )}
      </section>
    );
  }
  return (
    <section className="tool-section" style={style}>
      <div className="tool-header">
        <span className="tool-name">{tool.tool_name}</span>
        {warnings.length > 0 && (
          <span className="tool-quota-warnings" title={warnings.map((w) => w.text).join(" / ")}>
            {warnings.map((w, i) => (
              <span className="tool-quota-warning" key={w.key}>
                {i > 0 && <span className="tool-quota-sep">/</span>}
                {w.text}
              </span>
            ))}
          </span>
        )}
      </div>
      {network !== "ok" && (
        <div className="task-row">
          <span className={`status-icon ${network === "down" ? "net-down" : "net-flaky"}`}>⇄</span>
          <span className="idle-text">{network === "down" ? t("net_down") : t("net_flaky")}</span>
        </div>
      )}
      {tasks.length === 0
        ? network === "ok" && (
            <div className="task-row">
              <span className="status-icon online">●</span>
              <span className="idle-text">{t("online")}</span>
            </div>
          )
        : tasks.map((t) => <TaskRow key={t.task_id} task={t} />)}
    </section>
  );
}
