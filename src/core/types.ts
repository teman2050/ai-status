export type VisibleStatus =
  | "running"
  | "waiting"
  | "error"
  | "done_event"
  | "paused"
  | "stale";

export interface ToolInfo {
  tool_id: string;
  tool_name: string;
  status: string; // "connected"
  updated_at: string;
  quota?: string | null; // when exhausted: "session|9:30pm"; hide all this tool's tasks, show only the red quota row
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
