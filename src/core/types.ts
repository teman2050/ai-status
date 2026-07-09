export type VisibleStatus =
  | "running"
  | "waiting"
  | "error"
  | "done_event"
  | "paused"
  | "stale";

export interface QuotaUsage {
  h5_used: number; // 5h window used %
  h5_reset: number; // epoch seconds
  week_used: number; // weekly window used %
  week_reset: number;
}

export interface ToolInfo {
  tool_id: string;
  tool_name: string;
  status: string; // "connected"
  updated_at: string;
  quota?: string | null; // when exhausted: "session|9:30pm"; hide all this tool's tasks, show only the red quota row
  quota_usage?: QuotaUsage | null; // live rate-limit usage (Codex); frontend warns when a window is low
}

export interface TaskInfo {
  task_id: string;
  tool_id: string;
  workspace: string;
  title: string;
  summary: string;
  visible_status: VisibleStatus;
  elapsed_seconds: number;
  tokens?: number | null;
  quota_reset?: string | null; // "session|9:30pm" / "weekly|Monday 9am"
  updated_at: string;
}

export type NetworkStatus = "ok" | "flaky" | "down";

export interface Snapshot {
  tools: ToolInfo[];
  tasks: TaskInfo[];
  network: NetworkStatus;
}

export const EMPTY_SNAPSHOT: Snapshot = { tools: [], tasks: [], network: "ok" };
