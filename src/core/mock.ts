import type { Snapshot } from "./types";

export function mockSnapshot(): Snapshot {
  return {
    network: "ok",
    tools: [
      { tool_id: "claude_code", tool_name: "Claude Code", status: "connected", updated_at: "" },
      { tool_id: "codex", tool_name: "Codex", status: "connected", updated_at: "" },
      { tool_id: "cursor", tool_name: "Cursor", status: "connected", updated_at: "" },
    ],
    tasks: [
      { task_id: "m1", tool_id: "claude_code", workspace: "SizeKit", title: "SizeKit", summary: "Generate sitemap and verify links", visible_status: "running", elapsed_seconds: 102, updated_at: "" },
      { task_id: "m2", tool_id: "claude_code", workspace: "EmojiTree", title: "EmojiTree", summary: "confirm command", visible_status: "waiting", elapsed_seconds: 31, updated_at: "" },
      { task_id: "m3", tool_id: "codex", workspace: "LifeAdminPet", title: "LifeAdminPet", summary: "Fixing an on-device crash; running xcodebuild — this long line verifies the marquee scrolls correctly", visible_status: "running", elapsed_seconds: 381, updated_at: "" },
      { task_id: "m4", tool_id: "codex", workspace: "LinkKit", title: "LinkKit", summary: "xcodebuild failed: exit 65", visible_status: "error", elapsed_seconds: 77, updated_at: "" },
      { task_id: "m5", tool_id: "codex", workspace: "ReadyKit", title: "ReadyKit", summary: "done", visible_status: "done_event", elapsed_seconds: 45, updated_at: "" },
    ],
  };
}
